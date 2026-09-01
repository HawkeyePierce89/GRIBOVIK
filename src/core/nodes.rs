//! Turning changed files into review cards.
//!
//! This is where the diff layer and the language layer meet: for every changed
//! file we diff the two revisions, ask the analyzer what symbols each side
//! declares, and hand each symbol the slice of the diff that falls inside its
//! span. Whatever the symbols do not account for becomes one synthetic
//! file-level card, so no hunk is silently dropped on the floor.

use std::collections::{HashMap, HashSet};

use crate::core::diff::{line_diff, slice_diff, LineRange, Span};
use crate::core::error::AnalysisError;
use crate::core::lang::{analyzer_for_extension, Symbol};
use crate::core::snapshot::{ChangeKind, DiffLine, DiffTag, Node};

/// The `kind` of the synthetic node carrying a file's unattributed hunks.
/// No language analyzer ever produces it, so it is safe to test against.
pub const FILE_KIND: &str = "file";

/// The pure core's input: one changed file with the text of both sides.
///
/// `None` means the file did not exist on that side — an added file has no
/// `old`, a deleted one has no `new`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileInput {
    pub path: String,
    pub old: Option<String>,
    pub new: Option<String>,
}

impl FileInput {
    /// A file that only exists in the head revision.
    pub fn added(path: impl Into<String>, new: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            old: None,
            new: Some(new.into()),
        }
    }

    /// A file that only exists in the base revision.
    pub fn deleted(path: impl Into<String>, old: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            old: Some(old.into()),
            new: None,
        }
    }

    /// A file present on both sides.
    pub fn modified(
        path: impl Into<String>,
        old: impl Into<String>,
        new: impl Into<String>,
    ) -> Self {
        Self {
            path: path.into(),
            old: Some(old.into()),
            new: Some(new.into()),
        }
    }

    /// How the file as a whole changed, which is also the change kind of its
    /// file-level node.
    fn change(&self) -> ChangeKind {
        match (&self.old, &self.new) {
            (None, _) => ChangeKind::Added,
            (_, None) => ChangeKind::Deleted,
            _ => ChangeKind::Modified,
        }
    }
}

/// Build the review cards for every changed file, in input order.
///
/// The second half of the pair is the warning list: one entry per file that
/// could not be analyzed symbol by symbol and had to fall back to a whole-file
/// card. Analysis never fails outright — a file we cannot parse still shows up
/// as a reviewable diff.
pub fn build_nodes(files: &[FileInput]) -> (Vec<Node>, Vec<String>) {
    let mut nodes = Vec::new();
    let mut warnings = Vec::new();
    for file in files {
        build_file(file, &mut nodes, &mut warnings);
    }
    (nodes, warnings)
}

/// Append one file's cards, and a warning if it degraded to a whole-file card.
fn build_file(file: &FileInput, nodes: &mut Vec<Node>, warnings: &mut Vec<String>) {
    let old_src = file.old.as_deref().unwrap_or_default();
    let new_src = file.new.as_deref().unwrap_or_default();
    let file_diff = line_diff(old_src, new_src);
    // A file can be listed as changed without its content changing (a mode or
    // rename-only entry); it has nothing to review.
    if !file_diff.iter().any(is_change) {
        return;
    }

    let (old_symbols, new_symbols) = match symbols(&file.path, old_src, new_src) {
        Ok(pair) => pair,
        Err(error) => {
            warnings.push(degraded_warning(&file.path, &error));
            nodes.push(file_node(file, file_diff));
            return;
        }
    };

    let mut symbol_nodes = symbol_cards(file, &file_diff, &old_symbols, &new_symbols);

    // Anything the symbol cards did not claim — imports, `impl` scaffolding,
    // top-level statements — is reviewed as one file-level card. Coverage is
    // decided line by line rather than hunk by hunk: a single hunk can straddle
    // a symbol boundary, and the part outside the symbol still has to be shown.
    let claimed = claimed_lines(&symbol_nodes);
    let leftovers: Vec<DiffLine> = file_diff
        .iter()
        .filter(|line| is_change(line) && !claimed.contains(&line_key(line)))
        // A blank line added or removed between two symbols is the one leftover
        // with nothing to review in it; carding it would put a file node on
        // almost every file that gained a function. The test is emptiness, not
        // blankness: a line whose *indentation* changed still changed, and
        // trimming it away made a file whose only out-of-symbol edit was
        // whitespace disappear from the review with no card and no warning.
        .filter(|line| !line.text.is_empty())
        .cloned()
        .collect();

    nodes.append(&mut symbol_nodes);
    if !leftovers.is_empty() {
        nodes.push(file_node(file, leftovers));
    }
}

/// Classify every symbol of both sides and turn the changed ones into cards.
///
/// Deleted symbols come first, then the head revision's symbols in source
/// order; a symbol present on both sides whose span holds no changed line is
/// dropped, since there is nothing to review about it.
///
/// A qualified name is supposed to be unique within a file, but languages let
/// it repeat — two `impl` blocks declaring `S::fmt`, `#[cfg]`-gated twins,
/// TypeScript overload signatures. Occurrences are therefore paired up by
/// position: the *n*-th `S::fmt` of the old side is the previous revision of
/// the *n*-th `S::fmt` of the new side, and every occurrence past the first
/// gets a `#n` suffix so node ids stay unique.
fn symbol_cards(
    file: &FileInput,
    file_diff: &[DiffLine],
    old_symbols: &[Symbol],
    new_symbols: &[Symbol],
) -> Vec<Node> {
    let (old_occurrences, old_nth) = occurrences(old_symbols);
    let (new_occurrences, new_nth) = occurrences(new_symbols);
    let old_spans = spans(old_symbols);
    let new_spans = spans(new_symbols);

    let mut nodes = Vec::new();
    for ((symbol, nth), span) in old_symbols.iter().zip(&old_nth).zip(&old_spans) {
        // The same name declared as often on the new side means this
        // occurrence survived; a shrinking count deletes the trailing ones.
        if new_occurrences
            .count_of(symbol.qualified_name.as_str())
            .is_some_and(|count| count > *nth)
        {
            continue;
        }
        let diff = slice_diff(file_diff, Some(span), None);
        // The same guard the modified branch applies. Occurrences are paired
        // positionally, so removing the *first* of two `impl S { fn fmt }`
        // blocks marks the surviving one deleted: its span holds no changed
        // line, and carding it asks for a verdict on a deletion the card does
        // not show. The removed lines are still reviewed — they land on the
        // occurrence they were paired with, and on the file card.
        if diff.iter().any(is_change) {
            nodes.push(symbol_node(file, symbol, ChangeKind::Deleted, *nth, diff));
        }
    }
    for ((symbol, nth), span) in new_symbols.iter().zip(&new_nth).zip(&new_spans) {
        match old_occurrences.index_of(symbol.qualified_name.as_str(), *nth) {
            Some(before) => {
                let diff = slice_diff(file_diff, Some(&old_spans[before]), Some(span));
                if diff.iter().any(is_change) {
                    nodes.push(symbol_node(file, symbol, ChangeKind::Modified, *nth, diff));
                }
            }
            None => {
                let diff = slice_diff(file_diff, None, Some(span));
                // And the same guard again. A symbol whose qualifier changed
                // but whose body did not — renaming the `impl` block, the
                // class, the `extension` target — is absent from the old side
                // under its new name, so every member of the renamed type
                // arrives here with a slice of pure context. Carding those
                // asks for a verdict on an addition the card does not show;
                // the line that actually changed is already on the file card.
                if diff.iter().any(is_change) {
                    nodes.push(symbol_node(file, symbol, ChangeKind::Added, *nth, diff));
                }
            }
        }
    }
    nodes
}

/// One side's symbols indexed by qualified name, keeping every declaration in
/// source order rather than letting the first one win. The values are positions
/// in the side's symbol list, so a lookup also addresses that symbol's [`Span`].
struct Occurrences<'a>(HashMap<&'a str, Vec<usize>>);

impl Occurrences<'_> {
    /// Where the `nth` declaration of `name` sits in the side's symbol list, if
    /// the side has that many.
    fn index_of(&self, name: &str, nth: usize) -> Option<usize> {
        self.0.get(name).and_then(|all| all.get(nth)).copied()
    }

    /// How many times `name` is declared, or `None` when it is not.
    fn count_of(&self, name: &str) -> Option<usize> {
        self.0.get(name).map(Vec::len)
    }
}

/// Index `symbols` by name and tell each one which occurrence of its name it
/// is, so the two results can be zipped with the input.
fn occurrences(symbols: &[Symbol]) -> (Occurrences<'_>, Vec<usize>) {
    let mut index: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut nth = Vec::with_capacity(symbols.len());
    for (position, symbol) in symbols.iter().enumerate() {
        let all = index.entry(symbol.qualified_name.as_str()).or_default();
        nth.push(all.len());
        all.push(position);
    }
    (Occurrences(index), nth)
}

/// What each symbol of one side actually claims: its own span with the spans of
/// the symbols declared inside it carved out.
///
/// Quadratic in the symbol count of a single file, which is a few dozen at
/// worst and pure integer comparison — next to the parse that produced the
/// symbols it does not register.
fn spans(symbols: &[Symbol]) -> Vec<Span> {
    (0..symbols.len()).map(|i| carve(symbols, i)).collect()
}

/// The span the symbol at `index` claims among `symbols` — every symbol
/// declared inside it subtracted from its own range.
///
/// Shared with edge resolution so that the lines a card draws arrows from are
/// the same lines it shows: a Swift or TypeScript type whose span contains its
/// methods must not be credited with the calls their bodies make.
pub fn carve(symbols: &[Symbol], index: usize) -> Span {
    let outer = symbols[index].range();
    let inner: Vec<LineRange> = symbols
        .iter()
        .enumerate()
        .filter(|&(i, other)| nests_within((i, other.range()), (index, outer)))
        .map(|(_, other)| other.range())
        .collect();
    Span::new(outer, inner)
}

/// Whether `inner` is declared inside `outer`, each given as its position in
/// the analyzer's source-order symbol list paired with its line range.
///
/// Containment alone is not enough, because a declaration whose body opens and
/// closes on its own line — `struct P { func f() {} }` — reports the same range
/// for the type and its member, and a plain subrange test would leave the line
/// on both cards. Analyzers walk the tree, so an enclosing symbol is always
/// emitted before the symbols it contains; on a tie the later position is the
/// inner one. Comparing positions also keeps a symbol from nesting within
/// itself, which a `>=`/`<=` test on ranges alone would allow.
fn nests_within(inner: (usize, LineRange), outer: (usize, LineRange)) -> bool {
    let (inner_pos, inner_range) = inner;
    let (outer_pos, outer_range) = outer;
    inner_range.start >= outer_range.start
        && inner_range.end <= outer_range.end
        && (inner_range.start > outer_range.start
            || inner_range.end < outer_range.end
            || inner_pos > outer_pos)
}

/// Parse both sides with the analyzer the extension selects.
fn symbols(path: &str, old: &str, new: &str) -> Result<(Vec<Symbol>, Vec<Symbol>), AnalysisError> {
    let ext = extension(path);
    let analyzer = analyzer_for_extension(ext)
        .ok_or_else(|| AnalysisError::UnsupportedExtension(ext.to_string()))?;
    let old_symbols = analyzer.symbols(old).map_err(|e| e.with_path(path))?;
    let new_symbols = analyzer.symbols(new).map_err(|e| e.with_path(path))?;
    Ok((old_symbols, new_symbols))
}

fn symbol_node(
    file: &FileInput,
    symbol: &Symbol,
    change: ChangeKind,
    nth: usize,
    diff: Vec<DiffLine>,
) -> Node {
    let suffix = occurrence_suffix(nth);
    Node {
        id: format!("{}::{}{suffix}", file.path, symbol.qualified_name),
        file: file.path.clone(),
        name: symbol.qualified_name.clone(),
        kind: symbol.kind.clone(),
        change,
        diff,
    }
}

/// The `#n` suffix that keeps repeated qualified names apart in node ids, for
/// the 0-based `nth` occurrence of a name.
///
/// The first declaration keeps the plain id, so the overwhelming majority of
/// nodes — and the review state keyed on them — are unaffected by the
/// existence of this suffix.
fn occurrence_suffix(nth: usize) -> String {
    if nth == 0 {
        String::new()
    } else {
        format!("#{}", nth + 1)
    }
}

/// Recover the 0-based occurrence index that [`occurrence_suffix`] encoded in a
/// symbol node's id.
///
/// The id is the only place the occurrence survives — `name` stays the plain
/// qualified name for every twin — so anything that needs to know *which*
/// `S::fmt` a card is has to read it back from here. A file-level node, or any
/// id without a suffix, is the first occurrence.
pub fn occurrence_of(node: &Node) -> usize {
    let prefix = format!("{}::{}", node.file, node.name);
    node.id
        .strip_prefix(&prefix)
        .and_then(|rest| rest.strip_prefix('#'))
        .and_then(|nth| nth.parse::<usize>().ok())
        .map_or(0, |nth| nth.saturating_sub(1))
}

/// The catch-all card: the file itself, named by its path.
fn file_node(file: &FileInput, diff: Vec<DiffLine>) -> Node {
    Node {
        // `<file>` cannot collide with a qualified symbol name, so the id stays
        // unique however the file is named.
        id: format!("{}::<file>", file.path),
        file: file.path.clone(),
        name: file.path.clone(),
        kind: FILE_KIND.to_string(),
        change: file.change(),
        diff,
    }
}

fn degraded_warning(path: &str, error: &AnalysisError) -> String {
    match error {
        // The variant carries only the extension, so name the file here.
        AnalysisError::UnsupportedExtension(ext) => {
            format!("{path}: unsupported file extension `{ext}`; showing the whole file diff")
        }
        // Parse failures were already relabeled with the path.
        other => format!("{other}; showing the whole file diff"),
    }
}

/// Identify a diff line by the position it occupies on both sides. Every line
/// of the diff carries at least one side, and no two lines share a pair.
fn line_key(line: &DiffLine) -> (Option<u32>, Option<u32>) {
    (line.old_line, line.new_line)
}

/// Every changed line the symbol cards already show.
fn claimed_lines(cards: &[Node]) -> HashSet<(Option<u32>, Option<u32>)> {
    cards
        .iter()
        .flat_map(|node| node.diff.iter())
        .filter(|line| is_change(line))
        .map(line_key)
        .collect()
}

fn is_change(line: &DiffLine) -> bool {
    line.tag != DiffTag::Context
}

/// The extension of `path`, without its dot, or `""` when it has none.
pub(crate) fn extension(path: &str) -> &str {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    match name.rfind('.') {
        // A leading dot names a hidden file, not an extension.
        Some(dot) if dot > 0 => &name[dot + 1..],
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use std::slice;

    use super::*;

    const RUST_ADDED_BEFORE: &str = include_str!("../../tests/fixtures/rust/added_fn/before.rs");
    const RUST_ADDED_AFTER: &str = include_str!("../../tests/fixtures/rust/added_fn/after.rs");
    const RUST_MODIFIED_BEFORE: &str =
        include_str!("../../tests/fixtures/rust/modified_method/before.rs");
    const RUST_MODIFIED_AFTER: &str =
        include_str!("../../tests/fixtures/rust/modified_method/after.rs");
    const RUST_DELETED_BEFORE: &str =
        include_str!("../../tests/fixtures/rust/deleted_struct/before.rs");
    const RUST_DELETED_AFTER: &str =
        include_str!("../../tests/fixtures/rust/deleted_struct/after.rs");
    const RUST_NESTED_BEFORE: &str = include_str!("../../tests/fixtures/rust/nested_fn/before.rs");
    const RUST_NESTED_AFTER: &str = include_str!("../../tests/fixtures/rust/nested_fn/after.rs");
    const SWIFT_EXTENSION_BEFORE: &str =
        include_str!("../../tests/fixtures/swift/extension_method/before.swift");
    const SWIFT_EXTENSION_AFTER: &str =
        include_str!("../../tests/fixtures/swift/extension_method/after.swift");
    const SWIFT_DELETED_BEFORE: &str =
        include_str!("../../tests/fixtures/swift/deleted_struct/before.swift");
    const SWIFT_DELETED_AFTER: &str =
        include_str!("../../tests/fixtures/swift/deleted_struct/after.swift");
    const TS_MODIFIED_BEFORE: &str =
        include_str!("../../tests/fixtures/ts/modified_method/before.ts");
    const TS_MODIFIED_AFTER: &str =
        include_str!("../../tests/fixtures/ts/modified_method/after.ts");
    const TSX_BEFORE: &str = include_str!("../../tests/fixtures/ts/component/before.tsx");
    const TSX_AFTER: &str = include_str!("../../tests/fixtures/ts/component/after.tsx");

    /// Every card rendered as `id | kind | change | lines`, where each diff
    /// line shows as `+new`, `-old` or `=old/new`. Expectations then read as a
    /// table and a mismatch diffs line by line.
    fn outline(files: &[FileInput]) -> Vec<String> {
        let (nodes, _) = build_nodes(files);
        nodes.iter().map(render).collect()
    }

    fn render(node: &Node) -> String {
        let lines: Vec<String> = node
            .diff
            .iter()
            .map(|line| match line.tag {
                DiffTag::Add => format!("+{}", line.new_line.expect("added lines have a new line")),
                DiffTag::Del => format!(
                    "-{}",
                    line.old_line.expect("deleted lines have an old line")
                ),
                DiffTag::Context => format!(
                    "={}/{}",
                    line.old_line.expect("context lines have both"),
                    line.new_line.expect("context lines have both")
                ),
            })
            .collect();
        let change = match node.change {
            ChangeKind::Added => "added",
            ChangeKind::Modified => "modified",
            ChangeKind::Deleted => "deleted",
        };
        format!(
            "{} | {} | {} | {}",
            node.id,
            node.kind,
            change,
            lines.join(" ")
        )
    }

    fn row(id: &str, kind: &str, change: &str, lines: &str) -> String {
        format!("{id} | {kind} | {change} | {lines}")
    }

    fn warnings_of(files: &[FileInput]) -> Vec<String> {
        build_nodes(files).1
    }

    #[test]
    fn an_added_function_is_the_only_card() {
        let file = FileInput::modified("src/a.rs", RUST_ADDED_BEFORE, RUST_ADDED_AFTER);
        assert_eq!(
            outline(&[file]),
            // `greet` is untouched and therefore not worth reviewing; the blank
            // line 4 rides along in `greet_all`'s hunk, so no file card either.
            vec![row(
                "src/a.rs::greet_all",
                "function",
                "added",
                "+5 +6 +7 +8 +9"
            )]
        );
    }

    #[test]
    fn a_modified_method_carries_only_its_own_lines() {
        let file = FileInput::modified("src/a.rs", RUST_MODIFIED_BEFORE, RUST_MODIFIED_AFTER);
        assert_eq!(
            outline(&[file]),
            vec![
                row(
                    "src/a.rs::Counter::bump",
                    "method",
                    "modified",
                    "=10/10 -11 +11 +12 =12/13"
                ),
                row("src/a.rs::Counter::log", "method", "added", "+15"),
                // The old `}` closing the impl block aligns with the one
                // closing `step`, so it lands on `step`'s card as context.
                row("src/a.rs::step", "function", "added", "+18 +19 =13/20"),
                // The new `}` closing the impl block belongs to no symbol.
                row("src/a.rs::<file>", "file", "modified", "+16"),
            ]
        );
    }

    #[test]
    fn deleted_symbols_keep_their_base_revision_lines() {
        let file = FileInput::modified("src/a.rs", RUST_DELETED_BEFORE, RUST_DELETED_AFTER);
        assert_eq!(
            outline(&[file]),
            vec![
                // Deleted symbols come first, in base-revision order.
                row("src/a.rs::Legacy", "struct", "deleted", "-1 -2 -3"),
                row("src/a.rs::Legacy::id", "method", "deleted", "-6 -7 -8"),
                row(
                    "src/a.rs::keep",
                    "function",
                    "modified",
                    "=11/1 -12 +2 =13/3"
                ),
                // The `impl Legacy {` / `}` scaffolding around the deleted
                // method is inside no symbol's span and is reviewed here.
                row("src/a.rs::<file>", "file", "modified", "-5 -9"),
            ]
        );
    }

    #[test]
    fn a_nested_function_shows_up_on_its_parents_card() {
        let file = FileInput::modified("src/a.rs", RUST_NESTED_BEFORE, RUST_NESTED_AFTER);
        assert_eq!(
            outline(&[file]),
            vec![row(
                "src/a.rs::total",
                "function",
                "modified",
                "=1/1 -2 -3 +2 +3 =4/4 =5/5 -6 +6 =7/7"
            )]
        );
    }

    #[test]
    fn swift_extension_methods_are_cards_of_their_type() {
        let file = FileInput::modified(
            "App/Point.swift",
            SWIFT_EXTENSION_BEFORE,
            SWIFT_EXTENSION_AFTER,
        );
        assert_eq!(
            outline(&[file]),
            vec![
                row(
                    "App/Point.swift::Point.magnitude",
                    "method",
                    "modified",
                    "=7/7 -8 +8 +9 =9/13"
                ),
                row(
                    "App/Point.swift::Point.moved",
                    "method",
                    "added",
                    "+11 +12 =9/13"
                ),
            ]
        );
    }

    /// The whole point of carving nested spans out: a method body edit is the
    /// method's alone. Without it the enclosing type card repeats every one of
    /// those lines, and the reviewer votes twice on the same change.
    #[test]
    fn an_edit_inside_a_method_does_not_card_its_enclosing_type() {
        let before = "class Service {\n  a() { return 1; }\n  get() { return 2; }\n}\n";
        let after = "class Service {\n  a() { return 1; }\n  get() { return 22; }\n}\n";
        let file = FileInput::modified("web/svc.ts", before, after);
        assert_eq!(
            outline(&[file]),
            vec![row(
                "web/svc.ts::Service.get",
                "method",
                "modified",
                "-3 +3"
            )]
        );
    }

    /// A declaration written on one line reports the same range for the type
    /// and its member. Containment alone cannot separate them, and the line
    /// used to land on both cards — two verdicts on one edit, counted twice in
    /// the progress panel.
    #[test]
    fn a_one_line_type_does_not_share_its_change_with_its_member() {
        let file = FileInput::modified(
            "web/a.ts",
            "export class A { m() { return 1; } }\n",
            "export class A { m() { return 2; } }\n",
        );
        assert_eq!(
            outline(&[file]),
            vec![row("web/a.ts::A.m", "method", "modified", "-1 +1")]
        );
    }

    /// The same collision through the Swift analyzer.
    #[test]
    fn a_one_line_swift_struct_does_not_share_its_change_with_its_member() {
        let file = FileInput::modified(
            "app/a.swift",
            "struct P { func f() -> Int { return 1 } }\n",
            "struct P { func f() -> Int { return 2 } }\n",
        );
        assert_eq!(
            outline(&[file]),
            vec![row("app/a.swift::P.f", "method", "modified", "-1 +1")]
        );
    }

    /// Same rule through the Swift analyzer, whose type spans nest for the same
    /// reason TypeScript's do.
    #[test]
    fn a_swift_struct_does_not_repeat_its_methods_changes() {
        let before =
            "struct S {\n  func a() -> Int { return 1 }\n  func get() -> Int { return 2 }\n}\n";
        let after =
            "struct S {\n  func a() -> Int { return 1 }\n  func get() -> Int { return 22 }\n}\n";
        let file = FileInput::modified("App/S.swift", before, after);
        assert_eq!(
            outline(&[file]),
            vec![row("App/S.swift::S.get", "method", "modified", "-3 +3")]
        );
    }

    /// A class-level edit still gets the type its own card: the carve-out
    /// removes the members' lines, not the declaration's.
    #[test]
    fn a_type_level_edit_still_cards_the_type() {
        let before = "class Service {\n  a() { return 1; }\n}\n";
        let after = "class Service extends Base {\n  a() { return 1; }\n}\n";
        let file = FileInput::modified("web/svc.ts", before, after);
        assert_eq!(
            outline(&[file]),
            vec![row(
                "web/svc.ts::Service",
                "class",
                "modified",
                "-1 +1 =3/3"
            )]
        );
    }

    /// A bare `CR` is a line break to `similar` but not to tree-sitter. If the
    /// diff adopted `similar`'s count, every symbol below the `CR` would be
    /// sliced one line short — here `b` would end at the changed line and lose
    /// its closing brace. `b` legitimately starts at 4: the comment above it is
    /// leading documentation, which spans always cover.
    #[test]
    fn a_bare_carriage_return_does_not_shift_symbol_spans() {
        let before = "fn a() -> u32 {\n    1\n}\n// no\rte\nfn b() -> u32 {\n    2\n}\n";
        let after = "fn a() -> u32 {\n    1\n}\n// no\rte\nfn b() -> u32 {\n    999\n}\n";
        let file = FileInput::modified("src/lib.rs", before, after);
        assert_eq!(
            outline(&[file]),
            vec![row(
                "src/lib.rs::b",
                "function",
                "modified",
                "=4/4 =5/5 -6 +6 =7/7"
            )]
        );
    }

    #[test]
    fn swift_deleted_type_and_extension_method_are_separate_cards() {
        let file = FileInput::modified(
            "App/Legacy.swift",
            SWIFT_DELETED_BEFORE,
            SWIFT_DELETED_AFTER,
        );
        assert_eq!(
            outline(&[file]),
            vec![
                row("App/Legacy.swift::Legacy", "struct", "deleted", "-1 -2 -3"),
                row(
                    "App/Legacy.swift::Legacy.describe",
                    "method",
                    "deleted",
                    "-6 -7 -8"
                ),
                row(
                    "App/Legacy.swift::keep",
                    "function",
                    "modified",
                    "-11 -12 +1 +2 =13/3"
                ),
                // The `extension Point {` / `}` scaffolding belongs to no
                // symbol, so it lands on the file card.
                row("App/Legacy.swift::<file>", "file", "modified", "-5 -9"),
            ]
        );
    }

    /// Languages that nest methods inside their type produce a card per member
    /// plus, when there is class-level scaffolding to review, one for the type.
    /// The type does *not* repeat its members' changes: every line goes to the
    /// innermost symbol containing it, so `Counter` keeps the blank separator
    /// and the closing brace while `bump`'s edit belongs to `bump` alone. The
    /// constructor's lines are absent for the same reason — they are the
    /// constructor's, and it has nothing to review.
    #[test]
    fn a_typescript_class_does_not_repeat_its_methods_changes() {
        let file = FileInput::modified("web/counter.ts", TS_MODIFIED_BEFORE, TS_MODIFIED_AFTER);
        assert_eq!(
            outline(&[file]),
            vec![
                row(
                    "web/counter.ts::Counter",
                    "class",
                    "modified",
                    "=1/1 =2/2 =3/3 =7/7 +12 +14 =11/18"
                ),
                row(
                    "web/counter.ts::Counter.bump",
                    "method",
                    "modified",
                    "=8/8 -9 +9 +10 =10/11"
                ),
                row("web/counter.ts::Counter.log", "method", "added", "+13"),
                row(
                    "web/counter.ts::step",
                    "function",
                    "added",
                    "+16 +17 =11/18"
                ),
            ]
        );
    }

    #[test]
    fn a_tsx_component_yields_interface_and_arrow_cards() {
        let file = FileInput::modified("web/Badge.tsx", TSX_BEFORE, TSX_AFTER);
        assert_eq!(
            outline(&[file]),
            vec![
                row(
                    "web/Badge.tsx::BadgeProps",
                    "interface",
                    "added",
                    "+3 +4 +5 +6"
                ),
                row(
                    "web/Badge.tsx::Badge",
                    "function",
                    "modified",
                    "-3 -4 +8 +9 =5/10 =6/11"
                ),
                row(
                    "web/Badge.tsx::Panel",
                    "function",
                    "modified",
                    "=8/13 =9/14 =10/15 -11 +16 =12/17 =13/18 =14/19"
                ),
            ]
        );
    }

    #[test]
    fn changes_outside_every_symbol_become_one_file_card() {
        let file = FileInput::modified(
            "src/a.rs",
            "use std::fmt;\n\nfn keep() {}\n",
            "use std::io;\n\nfn keep() {}\n",
        );
        assert_eq!(
            outline(slice::from_ref(&file)),
            vec![row("src/a.rs::<file>", "file", "modified", "-1 +1")]
        );
        assert!(
            warnings_of(&[file]).is_empty(),
            "an import edit is not a failure"
        );
    }

    /// The file card carries the leftover hunks only — the ones symbol cards
    /// already show are not repeated.
    #[test]
    fn the_file_card_holds_only_the_unclaimed_hunks() {
        let file = FileInput::modified(
            "src/a.rs",
            "use std::fmt;\n\nfn keep() {\n    1\n}\n",
            "use std::io;\n\nfn keep() {\n    2\n}\n",
        );
        assert_eq!(
            outline(&[file]),
            vec![
                row("src/a.rs::keep", "function", "modified", "=3/3 -4 +4 =5/5"),
                row("src/a.rs::<file>", "file", "modified", "-1 +1"),
            ]
        );
    }

    #[test]
    fn an_unparsable_source_degrades_to_a_whole_file_card() {
        let file = FileInput::modified("src/a.rs", "fn keep() {}\n", "fn keep( {\n");
        assert_eq!(
            outline(slice::from_ref(&file)),
            vec![row("src/a.rs::<file>", "file", "modified", "-1 +1")]
        );
        assert_eq!(
            warnings_of(&[file]),
            vec![
                "failed to parse src/a.rs: source contains syntax errors; showing the whole file diff"
                    .to_string()
            ]
        );
    }

    #[test]
    fn an_unsupported_extension_degrades_to_a_whole_file_card() {
        let file = FileInput::modified("docs/readme.md", "old\nsame\n", "new\nsame\n");
        assert_eq!(
            outline(slice::from_ref(&file)),
            // The whole diff, context included: there is no symbol structure to
            // slice it against.
            vec![row(
                "docs/readme.md::<file>",
                "file",
                "modified",
                "-1 +1 =2/2"
            )]
        );
        assert_eq!(
            warnings_of(&[file]),
            vec![
                "docs/readme.md: unsupported file extension `md`; showing the whole file diff"
                    .to_string()
            ]
        );
    }

    #[test]
    fn a_file_without_an_extension_is_unsupported() {
        for path in ["Makefile", ".gitignore", "src/.hidden"] {
            let file = FileInput::modified(path, "a\n", "b\n");
            let (nodes, warnings) = build_nodes(&[file]);
            assert_eq!(nodes.len(), 1, "{path}");
            assert_eq!(nodes[0].kind, FILE_KIND, "{path}");
            assert_eq!(warnings.len(), 1, "{path}");
        }
    }

    #[test]
    fn an_added_file_yields_added_symbols() {
        let file = FileInput::added("src/a.rs", "fn fresh() -> u32 {\n    1\n}\n");
        assert_eq!(
            outline(&[file]),
            vec![row("src/a.rs::fresh", "function", "added", "+1 +2 +3")]
        );
    }

    #[test]
    fn a_deleted_file_yields_deleted_symbols() {
        let file = FileInput::deleted("src/a.rs", "fn gone() -> u32 {\n    1\n}\n");
        assert_eq!(
            outline(&[file]),
            vec![row("src/a.rs::gone", "function", "deleted", "-1 -2 -3")]
        );
    }

    /// A whole-file card inherits the file's own change kind, so a new
    /// unsupported file is not reported as a modification.
    #[test]
    fn the_file_card_inherits_the_files_change_kind() {
        let (nodes, _) = build_nodes(&[
            FileInput::added("docs/new.md", "hello\n"),
            FileInput::deleted("docs/old.md", "bye\n"),
        ]);
        assert_eq!(nodes[0].change, ChangeKind::Added);
        assert_eq!(nodes[1].change, ChangeKind::Deleted);
    }

    #[test]
    fn a_file_whose_content_is_unchanged_yields_nothing() {
        let source = "fn same() {}\n";
        let (nodes, warnings) = build_nodes(&[FileInput::modified("src/a.rs", source, source)]);
        assert!(nodes.is_empty());
        assert!(warnings.is_empty());
    }

    #[test]
    fn cards_follow_the_input_file_order() {
        let nodes = outline(&[
            FileInput::added("src/b.rs", "fn beta() {}\n"),
            FileInput::added("src/a.rs", "fn alpha() {}\n"),
        ]);
        assert_eq!(
            nodes,
            vec![
                row("src/b.rs::beta", "function", "added", "+1"),
                row("src/a.rs::alpha", "function", "added", "+1"),
            ]
        );
    }

    #[test]
    fn ids_are_the_file_path_and_the_qualified_name() {
        let (nodes, _) = build_nodes(&[FileInput::added(
            "src/a.rs",
            "struct Counter;\n\nimpl Counter {\n    fn bump(&self) {}\n}\n",
        )]);
        let ids: Vec<&str> = nodes.iter().map(|node| node.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "src/a.rs::Counter",
                "src/a.rs::Counter::bump",
                // The `impl Counter {` line and its `}` are outside both spans.
                "src/a.rs::<file>",
            ]
        );
        assert!(nodes.iter().all(|node| node.file == "src/a.rs"));
        assert_eq!(nodes[1].name, "Counter::bump");
    }

    /// The regression that motivates line-level leftover detection: a single
    /// hunk covering both a top-level statement and the first line of the
    /// symbol below it used to count as fully reviewed once the symbol claimed
    /// its share, and the statement vanished from the snapshot entirely.
    #[test]
    fn a_hunk_straddling_a_symbol_boundary_is_reviewed_on_both_cards() {
        let file = FileInput::modified(
            "src/a.rs",
            "use std::fmt;\nfn a() -> u32 { 1 }\n",
            "use std::io;\nfn a() -> u32 { 2 }\n",
        );
        assert_eq!(
            outline(&[file]),
            vec![
                row("src/a.rs::a", "function", "modified", "-2 +2"),
                row("src/a.rs::<file>", "file", "modified", "-1 +1"),
            ]
        );
    }

    /// Node ids key the edges and the review state on disk, so a file that
    /// declares one qualified name twice — two trait `impl`s, `#[cfg]` twins,
    /// TypeScript overloads — must still produce distinct ids.
    #[test]
    fn repeated_qualified_names_get_distinct_ids() {
        let (nodes, _) = build_nodes(&[FileInput::added(
            "src/s.rs",
            "struct S;\n\
             impl Display for S {\n    fn fmt(&self) -> u32 { 1 }\n}\n\
             impl Debug for S {\n    fn fmt(&self) -> u32 { 2 }\n}\n",
        )]);
        let ids: Vec<&str> = nodes.iter().map(|node| node.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "src/s.rs::S",
                "src/s.rs::S::fmt",
                "src/s.rs::S::fmt#2",
                "src/s.rs::<file>",
            ]
        );
        // Both cards keep the name the reviewer recognizes; only the id differs.
        assert_eq!(nodes[1].name, "S::fmt");
        assert_eq!(nodes[2].name, "S::fmt");
    }

    /// Occurrences are paired by position, so editing the second of two
    /// same-named methods diffs it against the second one, not the first.
    #[test]
    fn repeated_names_are_paired_by_occurrence_across_revisions() {
        let file = FileInput::modified(
            "src/s.rs",
            "impl A for S {\n    fn go(&self) -> u32 { 1 }\n}\n\
             impl B for S {\n    fn go(&self) -> u32 { 2 }\n}\n",
            "impl A for S {\n    fn go(&self) -> u32 { 1 }\n}\n\
             impl B for S {\n    fn go(&self) -> u32 { 3 }\n}\n",
        );
        assert_eq!(
            outline(&[file]),
            // The first `go` is untouched and produces no card at all.
            vec![row("src/s.rs::S::go#2", "method", "modified", "-5 +5")]
        );
    }

    /// Positional pairing marks the *surviving* twin deleted when the first of
    /// two same-named declarations is removed. That card's span holds no
    /// changed line, and a "deleted" card showing nothing deleted is a verdict
    /// the reviewer cannot give; the removed lines are reviewed on the cards
    /// that do carry them.
    #[test]
    fn a_deleted_card_with_nothing_deleted_in_it_is_not_emitted() {
        let file = FileInput::modified(
            "src/s.rs",
            "impl A for S {\n    fn go(&self) { alpha(); }\n}\n\
             impl B for S {\n    fn go(&self) { beta(); }\n}\n",
            "impl B for S {\n    fn go(&self) { beta(); }\n}\n",
        );
        let cards = outline(&[file]);
        assert!(
            !cards.iter().any(|card| card.contains("deleted")),
            "an empty deleted card survived: {cards:?}"
        );
        assert!(
            cards.iter().any(|card| card.contains("<file>")),
            "the removed lines have to land somewhere: {cards:?}"
        );
    }

    /// Renaming a type moves every member to a name the old side never had,
    /// so each one reaches the added branch with a slice of pure context. An
    /// "added" card showing nothing added is the same verdict the reviewer
    /// cannot give; the renamed line is carded on the file.
    #[test]
    fn an_added_card_with_nothing_added_in_it_is_not_emitted() {
        let file = FileInput::modified(
            "src/s.rs",
            "impl Foo {\n    fn go(&self) { alpha(); }\n}\n",
            "impl Bar {\n    fn go(&self) { alpha(); }\n}\n",
        );
        let cards = outline(&[file]);
        assert_eq!(
            cards,
            vec![row("src/s.rs::<file>", FILE_KIND, "modified", "-1 +1")],
            "an empty added card survived: {cards:?}"
        );
    }

    /// The blank-line exception is about *empty* leftovers. Trimming instead
    /// dropped a line whose indentation changed, and a file whose only
    /// out-of-symbol edit was whitespace vanished from the review entirely —
    /// no card, no warning.
    #[test]
    fn a_whitespace_only_leftover_still_gets_a_file_card() {
        let file = FileInput::modified(
            "src/a.rs",
            "use std::fmt;\n   \nfn keep() {}\n",
            "use std::fmt;\n\nfn keep() {}\n",
        );
        assert_eq!(
            outline(&[file]),
            // The replacement line *is* empty, so only the removal is carded —
            // which is the point: the change is no longer invisible.
            vec![row("src/a.rs::<file>", FILE_KIND, "modified", "-2")]
        );
    }

    /// A truly blank line between symbols is still not worth a card.
    #[test]
    fn an_added_blank_line_between_symbols_still_gets_no_file_card() {
        let file = FileInput::modified(
            "src/a.rs",
            "fn a() {}\nfn b() {}\n",
            "fn a() {}\n\nfn b() {}\n",
        );
        assert_eq!(outline(&[file]), Vec::<String>::new());
    }

    /// `} // end` belongs to the symbol the brace closes. Absorbing it into the
    /// next symbol's preamble put the line on two cards, so changing it asked
    /// for two verdicts on one edit.
    #[test]
    fn a_trailing_comment_is_not_absorbed_by_the_next_symbol() {
        let file = FileInput::modified(
            "src/a.rs",
            "fn a() {\n} // trailing\nfn b() {\n}\n",
            "fn a() {\n} // TRAILING\nfn b() {\n}\n",
        );
        assert_eq!(
            outline(&[file]),
            vec![row("src/a.rs::a", "function", "modified", "=1/1 -2 +2")]
        );
    }

    #[test]
    fn warnings_are_collected_across_files_in_order() {
        let warnings = warnings_of(&[
            FileInput::modified("docs/a.md", "a\n", "b\n"),
            FileInput::added("src/ok.rs", "fn fine() {}\n"),
            FileInput::modified("src/bad.rs", "fn keep() {}\n", "fn keep( {\n"),
        ]);
        assert_eq!(warnings.len(), 2);
        assert!(warnings[0].starts_with("docs/a.md: unsupported"));
        assert!(warnings[1].starts_with("failed to parse src/bad.rs"));
    }
}
