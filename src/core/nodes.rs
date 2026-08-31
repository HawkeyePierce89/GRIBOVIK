//! Turning changed files into review cards.
//!
//! This is where the diff layer and the language layer meet: for every changed
//! file we diff the two revisions, ask the analyzer what symbols each side
//! declares, and hand each symbol the slice of the diff that falls inside its
//! span. Whatever the symbols do not account for becomes one synthetic
//! file-level card, so no hunk is silently dropped on the floor.

use std::collections::{HashMap, HashSet};

use crate::core::diff::{hunks, line_diff, slice_diff, Hunk};
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
    // top-level statements — is reviewed as one file-level card.
    let leftovers: Vec<Hunk> = hunks(&file_diff)
        .into_iter()
        .filter(|hunk| !is_covered(hunk, &symbol_nodes))
        .collect();

    nodes.append(&mut symbol_nodes);
    if !leftovers.is_empty() {
        let lines = file_diff
            .iter()
            .filter(|line| leftovers.iter().any(|hunk| belongs_to(line, hunk)))
            .cloned()
            .collect();
        nodes.push(file_node(file, lines));
    }
}

/// Classify every symbol of both sides and turn the changed ones into cards.
///
/// Deleted symbols come first, then the head revision's symbols in source
/// order; a symbol present on both sides whose span holds no changed line is
/// dropped, since there is nothing to review about it.
fn symbol_cards(
    file: &FileInput,
    file_diff: &[DiffLine],
    old_symbols: &[Symbol],
    new_symbols: &[Symbol],
) -> Vec<Node> {
    let surviving: HashSet<&str> = new_symbols
        .iter()
        .map(|symbol| symbol.qualified_name.as_str())
        .collect();
    // First declaration wins, matching the order the analyzer reports.
    let mut previous: HashMap<&str, &Symbol> = HashMap::new();
    for symbol in old_symbols {
        previous
            .entry(symbol.qualified_name.as_str())
            .or_insert(symbol);
    }

    let mut nodes = Vec::new();
    for symbol in old_symbols {
        if surviving.contains(symbol.qualified_name.as_str()) {
            continue;
        }
        let diff = slice_diff(file_diff, Some(symbol.range()), None);
        nodes.push(symbol_node(file, symbol, ChangeKind::Deleted, diff));
    }
    for symbol in new_symbols {
        match previous.get(symbol.qualified_name.as_str()) {
            Some(before) => {
                let diff = slice_diff(file_diff, Some(before.range()), Some(symbol.range()));
                if diff.iter().any(is_change) {
                    nodes.push(symbol_node(file, symbol, ChangeKind::Modified, diff));
                }
            }
            None => {
                let diff = slice_diff(file_diff, None, Some(symbol.range()));
                nodes.push(symbol_node(file, symbol, ChangeKind::Added, diff));
            }
        }
    }
    nodes
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

fn symbol_node(file: &FileInput, symbol: &Symbol, change: ChangeKind, diff: Vec<DiffLine>) -> Node {
    Node {
        id: format!("{}::{}", file.path, symbol.qualified_name),
        file: file.path.clone(),
        name: symbol.qualified_name.clone(),
        kind: symbol.kind.clone(),
        change,
        diff,
    }
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

/// Whether any card already shows one of this hunk's lines.
///
/// Coverage is decided on the lines that actually landed on a card rather than
/// on span overlap: a hunk sitting between two symbols would "touch" both by
/// the range arithmetic while belonging to neither.
fn is_covered(hunk: &Hunk, cards: &[Node]) -> bool {
    cards
        .iter()
        .flat_map(|node| node.diff.iter())
        .any(|line| belongs_to(line, hunk))
}

/// Whether a changed line sits inside `hunk` on either side.
fn belongs_to(line: &DiffLine, hunk: &Hunk) -> bool {
    is_change(line)
        && (line.old_line.is_some_and(|l| hunk.old_range.contains(l))
            || line.new_line.is_some_and(|l| hunk.new_range.contains(l)))
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
            ]
        );
    }

    /// Languages that nest methods inside their type produce both cards: the
    /// member's card is the focused one, the type's card shows the change in
    /// the context of the whole declaration.
    #[test]
    fn a_typescript_class_and_its_methods_are_both_cards() {
        let file = FileInput::modified("web/counter.ts", TS_MODIFIED_BEFORE, TS_MODIFIED_AFTER);
        assert_eq!(
            outline(&[file]),
            vec![
                row(
                    "web/counter.ts::Counter",
                    "class",
                    "modified",
                    "=1/1 =2/2 =3/3 =4/4 =5/5 =6/6 =7/7 =8/8 -9 +9 +10 =10/11 +12 +13 +14 =11/18"
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
        assert_eq!(ids, vec!["src/a.rs::Counter", "src/a.rs::Counter::bump"]);
        assert!(nodes.iter().all(|node| node.file == "src/a.rs"));
        assert_eq!(nodes[1].name, "Counter::bump");
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
