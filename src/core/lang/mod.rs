//! Language analyzers: source text in, symbols and call names out.
//!
//! Each supported language implements [`LanguageAnalyzer`]; [`analyzer_for_extension`]
//! is the only registry. Analyzers are stateless and, like the rest of `core`,
//! never touch the filesystem — they receive already-loaded source text.
//!
//! Extraction is done by walking the tree-sitter tree rather than by running
//! queries: the walk is what lets a symbol *own* everything nested inside it
//! (a nested `fn` folds into its parent instead of becoming its own node).

pub mod rust;
pub mod swift;
pub mod tsjs;

use tree_sitter::{Language, Node, Tree};

use crate::core::diff::LineRange;
use crate::core::error::AnalysisError;

/// A named, line-delimited region of a source file: the unit a review card is
/// built from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    /// The bare name as written (`bump`). Call resolution matches on this.
    pub name: String,
    /// The name including its enclosing type and modules (`Counter::bump`).
    /// Node ids are built from this, so it must be unique within a file.
    pub qualified_name: String,
    /// Language-specific kind: `"function"`, `"method"`, `"struct"`, `"enum"`,
    /// `"trait"`, … Never `"file"`, which is reserved for synthetic nodes.
    pub kind: String,
    /// 1-based, inclusive. Covers leading doc comments and attributes so that
    /// editing them counts as editing the symbol.
    ///
    /// Spans may nest: in languages that declare methods inside their type
    /// (Swift, TS), the type's span contains its members' spans. Callers
    /// attributing a line to a symbol should prefer the innermost match.
    pub start_line: u32,
    /// 1-based, inclusive.
    pub end_line: u32,
}

impl Symbol {
    /// The symbol's span as the half-open range the diff layer speaks in.
    pub fn range(&self) -> LineRange {
        LineRange::inclusive(self.start_line, self.end_line)
    }
}

/// Everything the analysis core needs from a language.
pub trait LanguageAnalyzer {
    /// Every top-level and type-level symbol in `src`, in source order.
    fn symbols(&self, src: &str) -> Result<Vec<Symbol>, AnalysisError>;

    /// Bare callee names invoked from lines inside `range`, first occurrence
    /// order, deduplicated. Unparsable sources yield an empty list rather than
    /// an error: a missing edge degrades better than a failed analysis.
    fn calls_in_range(&self, src: &str, range: LineRange) -> Vec<String>;
}

/// The analyzer registry, keyed by file extension (with or without a dot).
///
/// Returns `None` for anything unsupported; callers turn that into an
/// [`AnalysisError::UnsupportedExtension`] or a file-level node.
pub fn analyzer_for_extension(ext: &str) -> Option<Box<dyn LanguageAnalyzer>> {
    match ext.trim_start_matches('.').to_ascii_lowercase().as_str() {
        "rs" => Some(Box::new(rust::RustAnalyzer)),
        "swift" => Some(Box::new(swift::SwiftAnalyzer)),
        "ts" => Some(Box::new(tsjs::TsJsAnalyzer::new(tsjs::Dialect::TypeScript))),
        // The tsx grammar is the only one that accepts JSX, and it accepts
        // everything plain JavaScript can say, so `.js` goes through it too.
        "tsx" | "js" | "jsx" => Some(Box::new(tsjs::TsJsAnalyzer::new(tsjs::Dialect::Tsx))),
        _ => None,
    }
}

/// Whether any analyzer claims this file extension.
///
/// The shell uses it to decide which changed files are worth loading at all,
/// so the supported-language list lives in exactly one place.
pub fn supports_extension(ext: &str) -> bool {
    analyzer_for_extension(ext).is_some()
}

/// Parse `src`, treating syntax errors as a hard failure.
///
/// A tree with `ERROR` nodes would yield a plausible-looking but silently
/// truncated symbol list, so the caller is told instead and can fall back to a
/// whole-file node plus a warning.
pub(crate) fn parse(language: &Language, src: &str, label: &str) -> Result<Tree, AnalysisError> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(language)
        .map_err(|error| AnalysisError::Parse {
            path: label.to_string(),
            reason: error.to_string(),
        })?;
    let tree = parser
        .parse(src, None)
        .ok_or_else(|| AnalysisError::Parse {
            path: label.to_string(),
            reason: "the parser returned no tree".to_string(),
        })?;
    if tree.root_node().has_error() {
        return Err(AnalysisError::Parse {
            path: label.to_string(),
            reason: "source contains syntax errors".to_string(),
        });
    }
    Ok(tree)
}

/// 1-based line the node starts on.
pub(crate) fn start_line(node: Node) -> u32 {
    node.start_position().row as u32 + 1
}

/// 1-based line the node ends on.
///
/// Nodes that swallow their trailing newline (comments, notably) report an end
/// position at column 0 of the *following* line; that line is not theirs.
pub(crate) fn end_line(node: Node) -> u32 {
    let end = node.end_position();
    if end.column == 0 && end.row > node.start_position().row {
        end.row as u32
    } else {
        end.row as u32 + 1
    }
}

/// Where a symbol really starts: the first line of the run of doc comments and
/// attributes immediately above `node`, so that editing `#[derive(..)]` or a
/// doc comment lands on the symbol rather than in the file-level catch-all.
///
/// `prelude_kinds` names the sibling node kinds that count as part of the
/// preamble; a blank line between them and the declaration ends the run.
pub(crate) fn leading_line(node: Node, prelude_kinds: &[&str]) -> u32 {
    let mut start = start_line(node);
    let mut current = node;
    while let Some(previous) = current.prev_sibling() {
        if !prelude_kinds.contains(&previous.kind()) || end_line(previous) + 1 < start {
            break;
        }
        start = start_line(previous);
        current = previous;
    }
    start
}

/// The source text a node spans.
pub(crate) fn text<'a>(node: Node, src: &'a str) -> &'a str {
    node.utf8_text(src.as_bytes()).unwrap_or("")
}

/// Text of a named field, or `""` when the field is absent.
pub(crate) fn field_text<'a>(node: Node, field: &str, src: &'a str) -> &'a str {
    node.child_by_field_name(field)
        .map(|child| text(child, src))
        .unwrap_or("")
}

/// Visit `node` and every descendant, parents before children.
///
/// Iterative rather than recursive: a vendored or generated source can nest
/// expressions thousands deep, and a stack overflow aborts the process outright
/// instead of degrading into the empty call list the API promises.
pub(crate) fn for_each_descendant(node: Node, visit: &mut dyn FnMut(Node)) {
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        visit(current);
        let mut cursor = current.walk();
        let before = stack.len();
        stack.extend(current.children(&mut cursor));
        // Children were pushed in source order and pop in reverse, so undo that
        // and keep the parents-before-children, left-to-right visit order.
        stack[before..].reverse();
    }
}

/// Append `name` unless it is empty or already present.
pub(crate) fn push_unique(out: &mut Vec<String>, name: &str) {
    if !name.is_empty() && !out.iter().any(|existing| existing == name) {
        out.push(name.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supports_exactly_the_six_source_extensions() {
        for ext in ["rs", "swift", "ts", "tsx", "js", "jsx"] {
            assert!(supports_extension(ext), "{ext} should be supported");
            assert!(supports_extension(&format!(".{ext}")), ".{ext} too");
        }
        for ext in ["md", "png", "toml", ""] {
            assert!(!supports_extension(ext), "{ext} should not be supported");
        }
    }

    #[test]
    fn registry_resolves_rust_sources() {
        let analyzer = analyzer_for_extension("rs").expect("`.rs` is supported");
        let symbols = analyzer.symbols("fn solo() {}\n").unwrap();
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "solo");
    }

    #[test]
    fn registry_resolves_swift_sources() {
        let analyzer = analyzer_for_extension("swift").expect("`.swift` is supported");
        let symbols = analyzer.symbols("func solo() {}\n").unwrap();
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "solo");
    }

    #[test]
    fn registry_resolves_every_typescript_and_javascript_extension() {
        for ext in ["ts", "tsx", "js", "jsx"] {
            let analyzer =
                analyzer_for_extension(ext).unwrap_or_else(|| panic!("`.{ext}` is supported"));
            let symbols = analyzer.symbols("function solo() {}\n").unwrap();
            assert_eq!(symbols.len(), 1, "{ext}");
            assert_eq!(symbols[0].name, "solo", "{ext}");
        }
    }

    #[test]
    fn registry_picks_the_jsx_capable_grammar_for_tsx_and_jsx() {
        let component = "const A = () => <div />;\n";
        for ext in ["tsx", "jsx", "js"] {
            let analyzer = analyzer_for_extension(ext).expect("supported");
            assert!(
                analyzer.symbols(component).is_ok(),
                "{ext} should parse JSX"
            );
        }
        // `.ts` deliberately does not: there `<` opens a type argument list.
        let plain = analyzer_for_extension("ts").expect("supported");
        assert!(plain.symbols(component).is_err());
    }

    #[test]
    fn registry_accepts_a_leading_dot_and_mixed_case() {
        assert!(analyzer_for_extension(".rs").is_some());
        assert!(analyzer_for_extension("RS").is_some());
        assert!(analyzer_for_extension(".Swift").is_some());
    }

    #[test]
    fn registry_declines_unsupported_extensions() {
        for ext in ["md", "png", "", "rust"] {
            assert!(
                analyzer_for_extension(ext).is_none(),
                "{ext} should not resolve to an analyzer"
            );
        }
    }

    #[test]
    fn symbol_range_is_the_inclusive_span() {
        let symbol = Symbol {
            name: "f".to_string(),
            qualified_name: "f".to_string(),
            kind: "function".to_string(),
            start_line: 3,
            end_line: 7,
        };
        assert_eq!(symbol.range(), LineRange::new(3, 8));
        assert!(symbol.range().contains(7));
        assert!(!symbol.range().contains(8));
    }

    #[test]
    fn parse_failures_carry_the_label_and_a_reason() {
        let language: Language = tree_sitter_rust::LANGUAGE.into();
        let error = parse(&language, "fn broken( {", "rust").unwrap_err();
        assert_eq!(
            error,
            AnalysisError::Parse {
                path: "rust".to_string(),
                reason: "source contains syntax errors".to_string(),
            }
        );
    }

    #[test]
    fn push_unique_keeps_first_occurrence_order() {
        let mut out = Vec::new();
        push_unique(&mut out, "beta");
        push_unique(&mut out, "alpha");
        push_unique(&mut out, "beta");
        push_unique(&mut out, "");
        assert_eq!(out, vec!["beta".to_string(), "alpha".to_string()]);
    }
}
