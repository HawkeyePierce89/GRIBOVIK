//! Call-edge resolution.
//!
//! The graph's arrows are a heuristic: we have bare callee names taken from a
//! caller's body and a list of changed symbols, and no type information to
//! match them with. Resolution is therefore proximity-based — a name is looked
//! up in the caller's own file first, then in its directory, then in the whole
//! graph — and a name that still matches several symbols yields one
//! `ambiguous` edge per candidate rather than a guess.
//!
//! Only changed symbols are candidates: a call into untouched code is not part
//! of the review and produces no edge.

use std::collections::HashMap;

use crate::core::lang::{analyzer_for_extension, LanguageAnalyzer, Symbol};
use crate::core::nodes::{extension, FileInput, FILE_KIND};
use crate::core::snapshot::{ChangeKind, Confidence, Edge, Node};

/// Resolve the calls between `nodes`, which must be the cards `build_nodes`
/// produced for these same `files`.
///
/// Edges are emitted in caller order, then in the order the calls appear in the
/// caller's body; `(from, to)` pairs are unique.
pub fn build_edges(files: &[FileInput], nodes: &[Node]) -> Vec<Edge> {
    let sources: Vec<Analyzed> = files.iter().filter_map(Analyzed::of).collect();
    let by_path: HashMap<&str, &Analyzed> = sources
        .iter()
        .map(|analyzed| (analyzed.file.path.as_str(), analyzed))
        .collect();

    let callables: Vec<Callable> = nodes
        .iter()
        .filter(|node| node.kind != FILE_KIND)
        .filter_map(|node| {
            by_path
                .get(node.file.as_str())
                .and_then(|a| a.callable(node))
        })
        .collect();

    // Bare name -> every changed symbol that answers to it, in node order.
    let mut index: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, callable) in callables.iter().enumerate() {
        index.entry(callable.name).or_default().push(i);
    }

    let mut edges = Vec::new();
    for caller in &callables {
        let calls = caller
            .analyzer
            .calls_in_range(caller.src, caller.symbol.range());
        for call in &calls {
            let Some(candidates) = index.get(call.as_str()) else {
                continue;
            };
            let winners = resolve(caller, candidates, &callables);
            // A single candidate is the answer; several mean the name alone
            // cannot decide, so the reviewer is shown all of them.
            let confidence = if winners.len() == 1 {
                Confidence::Certain
            } else {
                Confidence::Ambiguous
            };
            for &winner in &winners {
                let callee = callables[winner].node;
                // A symbol calling itself says nothing about the graph.
                if callee.id == caller.node.id {
                    continue;
                }
                edges.push(Edge {
                    from: caller.node.id.clone(),
                    to: callee.id.clone(),
                    confidence,
                });
            }
        }
    }
    dedup(edges)
}

/// The candidates that win the call, tried nearest-first: the caller's own
/// file, then its directory, then the rest of the graph.
///
/// The caller itself stays in the running so that a recursive call resolves
/// locally instead of falling through to an unrelated same-named symbol; the
/// self-edge is dropped afterwards.
fn resolve(caller: &Callable, candidates: &[usize], callables: &[Callable]) -> Vec<usize> {
    let same_file: Vec<usize> = candidates
        .iter()
        .copied()
        .filter(|&i| callables[i].node.file == caller.node.file)
        .collect();
    if !same_file.is_empty() {
        return same_file;
    }
    let dir = directory(&caller.node.file);
    let same_dir: Vec<usize> = candidates
        .iter()
        .copied()
        .filter(|&i| directory(&callables[i].node.file) == dir)
        .collect();
    if !same_dir.is_empty() {
        return same_dir;
    }
    candidates.to_vec()
}

/// Collapse repeated `(from, to)` pairs, keeping the first position and the
/// strongest confidence seen for the pair.
fn dedup(edges: Vec<Edge>) -> Vec<Edge> {
    let mut out: Vec<Edge> = Vec::with_capacity(edges.len());
    for edge in edges {
        match out
            .iter_mut()
            .find(|kept| kept.from == edge.from && kept.to == edge.to)
        {
            Some(kept) => {
                if edge.confidence == Confidence::Certain {
                    kept.confidence = Confidence::Certain;
                }
            }
            None => out.push(edge),
        }
    }
    out
}

/// The directory part of a repository-relative path, `""` at the root.
fn directory(path: &str) -> &str {
    match path.rfind(['/', '\\']) {
        Some(slash) => &path[..slash],
        None => "",
    }
}

/// One changed file, parsed once, with its symbols reachable by qualified name.
struct Analyzed<'a> {
    file: &'a FileInput,
    analyzer: Box<dyn LanguageAnalyzer>,
    old: HashMap<String, Symbol>,
    new: HashMap<String, Symbol>,
}

impl<'a> Analyzed<'a> {
    /// `None` for files no analyzer handles; those only ever produced a
    /// file-level card, which takes no part in the call graph.
    fn of(file: &'a FileInput) -> Option<Self> {
        let analyzer = analyzer_for_extension(extension(&file.path))?;
        Some(Self {
            old: side_symbols(analyzer.as_ref(), file.old.as_deref()),
            new: side_symbols(analyzer.as_ref(), file.new.as_deref()),
            analyzer,
            file,
        })
    }

    /// Pair a card with the revision it should be read from: a deleted symbol
    /// only exists in the base revision, everything else in the head.
    fn callable<'s>(&'s self, node: &'s Node) -> Option<Callable<'s>> {
        let (symbols, src) = match node.change {
            ChangeKind::Deleted => (&self.old, self.file.old.as_deref()),
            _ => (&self.new, self.file.new.as_deref()),
        };
        let symbol = symbols.get(&node.name)?;
        Some(Callable {
            node,
            name: symbol.name.as_str(),
            symbol,
            src: src.unwrap_or_default(),
            analyzer: self.analyzer.as_ref(),
        })
    }
}

/// A card that can call and be called: a changed symbol, located in the
/// revision its change kind points at.
struct Callable<'a> {
    node: &'a Node,
    /// The bare name calls are matched against.
    name: &'a str,
    symbol: &'a Symbol,
    src: &'a str,
    analyzer: &'a dyn LanguageAnalyzer,
}

/// Index one side's symbols by qualified name; first declaration wins, as in
/// node construction. An unparsable side simply has no symbols.
fn side_symbols(analyzer: &dyn LanguageAnalyzer, src: Option<&str>) -> HashMap<String, Symbol> {
    let Some(symbols) = src.and_then(|src| analyzer.symbols(src).ok()) else {
        return HashMap::new();
    };
    let mut out = HashMap::new();
    for symbol in symbols {
        out.entry(symbol.qualified_name.clone()).or_insert(symbol);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::nodes::build_nodes;

    /// Every edge as `from -> to (confidence)`, in emission order.
    fn edges_of(files: &[FileInput]) -> Vec<String> {
        let (nodes, _) = build_nodes(files);
        build_edges(files, &nodes).iter().map(render).collect()
    }

    fn render(edge: &Edge) -> String {
        let confidence = match edge.confidence {
            Confidence::Certain => "certain",
            Confidence::Ambiguous => "ambiguous",
        };
        format!("{} -> {} ({confidence})", edge.from, edge.to)
    }

    fn rust_helper(body: &str) -> String {
        format!("fn helper() -> u32 {{\n    {body}\n}}\n")
    }

    fn rust_caller() -> String {
        "fn caller() -> u32 {\n    helper()\n}\n".to_string()
    }

    #[test]
    fn a_call_resolves_within_the_file() {
        let source = format!("{}\n{}", rust_helper("1"), rust_caller());
        assert_eq!(
            edges_of(&[FileInput::added("src/a.rs", source)]),
            vec!["src/a.rs::caller -> src/a.rs::helper (certain)"]
        );
    }

    /// With nothing nearer to match, a name that is unique in the whole graph
    /// is still good enough to draw a confident arrow.
    #[test]
    fn a_unique_name_resolves_across_directories() {
        assert_eq!(
            edges_of(&[
                FileInput::added("src/one/a.rs", rust_helper("1")),
                FileInput::added("src/two/b.rs", rust_caller()),
            ]),
            vec!["src/two/b.rs::caller -> src/one/a.rs::helper (certain)"]
        );
    }

    #[test]
    fn the_callers_own_file_wins_over_a_neighbouring_one() {
        let source = format!("{}\n{}", rust_helper("1"), rust_caller());
        assert_eq!(
            edges_of(&[
                FileInput::added("src/a.rs", source),
                FileInput::added("src/b.rs", rust_helper("2")),
            ]),
            vec!["src/a.rs::caller -> src/a.rs::helper (certain)"]
        );
    }

    #[test]
    fn the_callers_own_directory_wins_over_a_distant_one() {
        assert_eq!(
            edges_of(&[
                FileInput::added("src/far/other.rs", rust_helper("2")),
                FileInput::added("src/app/util.rs", rust_helper("1")),
                FileInput::added("src/app/main.rs", rust_caller()),
            ]),
            vec!["src/app/main.rs::caller -> src/app/util.rs::helper (certain)"]
        );
    }

    /// Two equally distant candidates cannot be told apart without type
    /// information, so both are drawn and marked as guesses.
    #[test]
    fn an_undecidable_name_yields_one_ambiguous_edge_per_candidate() {
        assert_eq!(
            edges_of(&[
                FileInput::added("src/x/a.rs", rust_helper("1")),
                FileInput::added("src/y/b.rs", rust_helper("2")),
                FileInput::added("src/app/main.rs", rust_caller()),
            ]),
            vec![
                "src/app/main.rs::caller -> src/x/a.rs::helper (ambiguous)",
                "src/app/main.rs::caller -> src/y/b.rs::helper (ambiguous)",
            ]
        );
    }

    /// The graph only contains changed symbols; a call reaching into untouched
    /// code has no card to point at.
    #[test]
    fn a_call_into_unchanged_code_produces_no_edge() {
        let file = FileInput::modified(
            "src/a.rs",
            "fn helper() -> u32 {\n    1\n}\n\nfn caller() -> u32 {\n    0\n}\n",
            "fn helper() -> u32 {\n    1\n}\n\nfn caller() -> u32 {\n    helper()\n}\n",
        );
        let (nodes, _) = build_nodes(std::slice::from_ref(&file));
        assert_eq!(nodes.len(), 1, "only `caller` changed");
        assert!(edges_of(&[file]).is_empty());
    }

    #[test]
    fn a_symbol_calling_itself_gets_no_edge() {
        let source = "fn walk(n: u32) -> u32 {\n    if n == 0 { 0 } else { walk(n - 1) }\n}\n";
        assert!(edges_of(&[FileInput::added("src/a.rs", source)]).is_empty());
    }

    /// Recursion keeps the local symbol from being confused with a same-named
    /// one elsewhere: the self-edge is dropped, not redirected.
    #[test]
    fn recursion_does_not_fall_through_to_a_distant_namesake() {
        assert_eq!(
            edges_of(&[
                FileInput::added("src/a.rs", "fn walk(n: u32) -> u32 {\n    walk(n - 1)\n}\n"),
                FileInput::added("src/b.rs", "fn walk() {}\n"),
            ]),
            Vec::<String>::new()
        );
    }

    /// A method reached as `Type::method` is indexed under its bare name, so a
    /// qualified call finds it.
    #[test]
    fn a_qualified_call_matches_the_bare_method_name() {
        let source = "\
struct Counter {
    hits: u32,
}

impl Counter {
    fn new() -> Self {
        Counter { hits: 0 }
    }
}

fn start() -> Counter {
    Counter::new()
}
";
        assert_eq!(
            edges_of(&[FileInput::added("src/a.rs", source)]),
            vec![
                // `Counter { hits: 0 }` is a use of the struct itself.
                "src/a.rs::Counter::new -> src/a.rs::Counter (certain)",
                // `Counter::new()` names only its final segment, so the call
                // points at the method and not at the type.
                "src/a.rs::start -> src/a.rs::Counter::new (certain)",
            ]
        );
    }

    #[test]
    fn a_method_call_matches_the_receivers_method() {
        let source = "\
struct Counter;

impl Counter {
    fn bump(&self) {}
}

fn run(counter: &Counter) {
    counter.bump();
}
";
        assert_eq!(
            edges_of(&[FileInput::added("src/a.rs", source)]),
            vec!["src/a.rs::run -> src/a.rs::Counter::bump (certain)"]
        );
    }

    /// A deleted symbol no longer exists in the head revision, so its calls are
    /// read from the base one.
    #[test]
    fn a_deleted_caller_is_read_from_the_base_revision() {
        let file = FileInput::modified(
            "src/a.rs",
            "fn helper() -> u32 {\n    1\n}\n\nfn gone() -> u32 {\n    helper()\n}\n",
            "fn helper() -> u32 {\n    2\n}\n",
        );
        assert_eq!(
            edges_of(&[file]),
            vec!["src/a.rs::gone -> src/a.rs::helper (certain)"]
        );
    }

    /// File-level cards are diffs, not symbols: they neither call nor are
    /// called, however much code their lines contain.
    #[test]
    fn file_level_cards_take_no_part_in_the_graph() {
        let files = [
            FileInput::added("src/a.rs", rust_helper("1")),
            // Unparsable, so `src/b.rs` degrades to a single file-level card.
            FileInput::added("src/b.rs", "fn caller( {\n    helper()\n}\n"),
        ];
        let (nodes, warnings) = build_nodes(&files);
        assert_eq!(warnings.len(), 1);
        assert!(nodes.iter().any(|node| node.kind == FILE_KIND));
        assert!(edges_of(&files).is_empty());
    }

    /// Nested spans mean a type's card sees its members' calls too, which is
    /// what makes the type card a useful summary of the change.
    #[test]
    fn typescript_calls_resolve_through_the_enclosing_class() {
        let source = "\
function helper(): number {
  return 1;
}

export class Counter {
  bump(): number {
    return helper();
  }
}
";
        assert_eq!(
            edges_of(&[FileInput::added("web/counter.ts", source)]),
            vec![
                "web/counter.ts::Counter -> web/counter.ts::helper (certain)",
                "web/counter.ts::Counter.bump -> web/counter.ts::helper (certain)",
            ]
        );
    }

    #[test]
    fn swift_calls_resolve_across_files() {
        assert_eq!(
            edges_of(&[
                FileInput::added("App/Util.swift", "func helper() -> Int {\n    1\n}\n"),
                FileInput::added(
                    "App/Main.swift",
                    "func run() -> Int {\n    return helper()\n}\n"
                ),
            ]),
            vec!["App/Main.swift::run -> App/Util.swift::helper (certain)"]
        );
    }

    #[test]
    fn a_repeated_pair_is_kept_once_at_its_strongest_confidence() {
        let pair = |confidence| Edge {
            from: "a".to_string(),
            to: "b".to_string(),
            confidence,
        };
        let other = Edge {
            from: "a".to_string(),
            to: "c".to_string(),
            confidence: Confidence::Ambiguous,
        };
        assert_eq!(
            dedup(vec![
                pair(Confidence::Ambiguous),
                other.clone(),
                pair(Confidence::Certain),
            ]),
            // The pair keeps its first position and gains the better verdict.
            vec![pair(Confidence::Certain), other.clone()]
        );
        assert_eq!(
            dedup(vec![pair(Confidence::Certain), pair(Confidence::Ambiguous)]),
            vec![pair(Confidence::Certain)]
        );
    }

    #[test]
    fn directories_are_the_path_up_to_the_last_separator() {
        assert_eq!(directory("src/app/main.rs"), "src/app");
        assert_eq!(directory("main.rs"), "");
        assert_eq!(directory("src\\app\\main.rs"), "src\\app");
    }

    #[test]
    fn an_empty_graph_has_no_edges() {
        assert!(build_edges(&[], &[]).is_empty());
    }
}
