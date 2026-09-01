//! Line-level diffing.
//!
//! Produces the raw material every later stage consumes: a flat list of
//! [`DiffLine`]s carrying both sides' line numbers, and a way to slice the file
//! diff down to one symbol's span.

use similar::{ChangeTag, TextDiff};

use crate::core::snapshot::{DiffLine, DiffTag};

/// A half-open span of 1-based line numbers: `start` is included, `end` is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineRange {
    pub start: u32,
    pub end: u32,
}

impl LineRange {
    /// Build from half-open bounds.
    pub fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }

    /// Build from the inclusive `start_line..=end_line` form symbols use.
    pub fn inclusive(start_line: u32, end_line: u32) -> Self {
        Self {
            start: start_line,
            end: end_line.saturating_add(1),
        }
    }

    pub fn contains(&self, line: u32) -> bool {
        self.start <= line && line < self.end
    }

    /// The lines both ranges cover, or `None` when they are disjoint.
    pub fn intersect(&self, other: &LineRange) -> Option<LineRange> {
        let start = self.start.max(other.start);
        let end = self.end.min(other.end);
        (start < end).then(|| LineRange::new(start, end))
    }
}

/// Diff `old` against `new`, emitting one [`DiffLine`] per line of the result.
///
/// Every line of both revisions appears exactly once: context lines carry both
/// line numbers, additions only `new_line`, deletions only `old_line`. Trailing
/// newlines are stripped from `text`.
///
/// Lines are split on `\n` and nothing else. `similar`'s own line tokenizer
/// also breaks on a bare `\r`, but tree-sitter advances a row only on `\n` —
/// and the two numberings have to agree, because [`slice_diff`] indexes this
/// list with spans the language layer reported. One stray `CR` in a file would
/// otherwise shift every symbol below it by a line, quietly handing cards the
/// wrong diff.
pub fn line_diff(old: &str, new: &str) -> Vec<DiffLine> {
    let old_lines: Vec<&str> = old.split_inclusive('\n').collect();
    let new_lines: Vec<&str> = new.split_inclusive('\n').collect();
    let diff = TextDiff::configure()
        .newline_terminated(true)
        .diff_slices(&old_lines, &new_lines);
    diff.iter_all_changes()
        .map(|change| {
            let tag = match change.tag() {
                ChangeTag::Equal => DiffTag::Context,
                ChangeTag::Delete => DiffTag::Del,
                ChangeTag::Insert => DiffTag::Add,
            };
            DiffLine {
                tag,
                old_line: change.old_index().map(|i| i as u32 + 1),
                new_line: change.new_index().map(|i| i as u32 + 1),
                text: strip_eol(change.value()).to_string(),
            }
        })
        .collect()
}

/// One symbol's claim on a side: its span, minus the spans nested inside it.
///
/// Swift and TypeScript declare methods inside their type, so the type's span
/// contains its members' — and [`crate::core::lang::Symbol::start_line`] asks
/// callers to attribute a line to the *innermost* symbol containing it. Doing
/// it by plain containment instead puts a one-line edit to a method on the
/// method card *and* on the enclosing class card: the reviewer is asked for the
/// same verdict twice and the progress panel counts the work twice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    outer: LineRange,
    inner: Vec<LineRange>,
}

impl Span {
    /// A span with `inner` carved out of `outer`.
    pub fn new(outer: LineRange, inner: Vec<LineRange>) -> Self {
        Self { outer, inner }
    }

    /// A span with nothing nested inside it — the common case, and the only
    /// one in a language whose symbols never nest.
    pub fn whole(outer: LineRange) -> Self {
        Self {
            outer,
            inner: Vec::new(),
        }
    }

    /// Whether this symbol is the innermost one covering `line`.
    pub fn claims(&self, line: u32) -> bool {
        self.outer.contains(line) && !self.inner.iter().any(|r| r.contains(line))
    }

    /// The uncarved range, every line of which this span either claims or has
    /// ceded to another symbol. Nothing outside it is ever claimed, which is
    /// what lets a caller bound a scan by it instead of by the whole file.
    pub fn outer(&self) -> LineRange {
        self.outer
    }
}

/// Select the diff lines belonging to a symbol's span.
///
/// A line is kept when the symbol claims it on the old side or on the new side,
/// so a modified symbol picks up its deletions and its additions alike. Pass
/// `None` for the side a symbol does not exist on (added or deleted symbols).
pub fn slice_diff(
    diff: &[DiffLine],
    old_span: Option<&Span>,
    new_span: Option<&Span>,
) -> Vec<DiffLine> {
    diff.iter()
        .filter(|line| {
            let in_old = matches!((old_span, line.old_line), (Some(s), Some(l)) if s.claims(l));
            let in_new = matches!((new_span, line.new_line), (Some(s), Some(l)) if s.claims(l));
            in_old || in_new
        })
        .cloned()
        .collect()
}

/// Drop one trailing line terminator, leaving any other whitespace alone.
///
/// A lone trailing `\r` counts: a file whose last line ends in a bare `CR` has
/// no newline to strip, and the terminator would otherwise be rendered as part
/// of the card's text.
fn strip_eol(text: &str) -> &str {
    match text.strip_suffix('\n') {
        Some(t) => t.strip_suffix('\r').unwrap_or(t),
        None => text.strip_suffix('\r').unwrap_or(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compact rendering so expectations read like a diff instead of a wall of
    /// struct literals: `("context", old, new, text)`.
    fn render(diff: &[DiffLine]) -> Vec<(&'static str, Option<u32>, Option<u32>, &str)> {
        diff.iter()
            .map(|line| {
                let tag = match line.tag {
                    DiffTag::Add => "add",
                    DiffTag::Del => "del",
                    DiffTag::Context => "context",
                };
                (tag, line.old_line, line.new_line, line.text.as_str())
            })
            .collect()
    }

    #[test]
    fn pure_insertion_numbers_both_sides() {
        let diff = line_diff("a\nb\nc\n", "a\nb\nX\nY\nc\n");
        assert_eq!(
            render(&diff),
            vec![
                ("context", Some(1), Some(1), "a"),
                ("context", Some(2), Some(2), "b"),
                ("add", None, Some(3), "X"),
                ("add", None, Some(4), "Y"),
                ("context", Some(3), Some(5), "c"),
            ]
        );
    }

    #[test]
    fn pure_deletion_numbers_both_sides() {
        let diff = line_diff("a\nb\nc\nd\n", "a\nd\n");
        assert_eq!(
            render(&diff),
            vec![
                ("context", Some(1), Some(1), "a"),
                ("del", Some(2), None, "b"),
                ("del", Some(3), None, "c"),
                ("context", Some(4), Some(2), "d"),
            ]
        );
    }

    #[test]
    fn modification_in_the_middle() {
        let diff = line_diff("a\nb\nc\n", "a\nB\nc\n");
        assert_eq!(
            render(&diff),
            vec![
                ("context", Some(1), Some(1), "a"),
                ("del", Some(2), None, "b"),
                ("add", None, Some(2), "B"),
                ("context", Some(3), Some(3), "c"),
            ]
        );
    }

    #[test]
    fn whole_file_rewrite_replaces_every_line() {
        let diff = line_diff("a\nb\n", "x\ny\n");
        assert_eq!(
            render(&diff),
            vec![
                ("del", Some(1), None, "a"),
                ("del", Some(2), None, "b"),
                ("add", None, Some(1), "x"),
                ("add", None, Some(2), "y"),
            ]
        );
    }

    #[test]
    fn added_file_has_no_old_side() {
        let diff = line_diff("", "a\nb\n");
        assert_eq!(
            render(&diff),
            vec![("add", None, Some(1), "a"), ("add", None, Some(2), "b")]
        );
    }

    #[test]
    fn deleted_file_has_no_new_side() {
        let diff = line_diff("a\nb\n", "");
        assert_eq!(
            render(&diff),
            vec![("del", Some(1), None, "a"), ("del", Some(2), None, "b")]
        );
    }

    #[test]
    fn identical_sources_are_all_context() {
        let diff = line_diff("a\nb\n", "a\nb\n");
        assert!(diff.iter().all(|l| l.tag == DiffTag::Context));
    }

    #[test]
    fn trailing_newline_is_stripped_but_content_whitespace_is_not() {
        let diff = line_diff("", "  indented  \r\nno trailing newline");
        assert_eq!(
            render(&diff),
            vec![
                ("add", None, Some(1), "  indented  "),
                ("add", None, Some(2), "no trailing newline"),
            ]
        );
    }

    #[test]
    fn slice_keeps_only_the_symbols_own_lines() {
        // Two functions, only the second one changes.
        let old = "fn a() {\n    one();\n}\n\nfn b() {\n    two();\n}\n";
        let new = "fn a() {\n    one();\n}\n\nfn b() {\n    TWO();\n}\n";
        let diff = line_diff(old, new);

        let b_span = Span::whole(LineRange::inclusive(5, 7));
        let b = slice_diff(&diff, Some(&b_span), Some(&b_span));
        assert_eq!(
            render(&b),
            vec![
                ("context", Some(5), Some(5), "fn b() {"),
                ("del", Some(6), None, "    two();"),
                ("add", None, Some(6), "    TWO();"),
                ("context", Some(7), Some(7), "}"),
            ]
        );

        let a_span = Span::whole(LineRange::inclusive(1, 3));
        let a = slice_diff(&diff, Some(&a_span), Some(&a_span));
        assert!(a.iter().all(|l| l.tag == DiffTag::Context));
    }

    #[test]
    fn slice_with_one_side_missing_picks_up_that_sides_lines_only() {
        let old = "fn gone() {\n    old_body();\n}\n";
        let new = "fn fresh() {\n    new_body();\n}\n";
        let diff = line_diff(old, new);

        // A deleted symbol has no span in the new revision. The closing brace
        // is byte-identical on both sides, so the diff calls it context — it
        // still belongs to the symbol because it sits inside the old span.
        let whole = Span::whole(LineRange::inclusive(1, 3));
        let deleted = slice_diff(&diff, Some(&whole), None);
        assert_eq!(
            render(&deleted),
            vec![
                ("del", Some(1), None, "fn gone() {"),
                ("del", Some(2), None, "    old_body();"),
                ("context", Some(3), Some(3), "}"),
            ]
        );

        // An added one has no span in the old revision.
        let added = slice_diff(&diff, None, Some(&whole));
        assert_eq!(
            render(&added),
            vec![
                ("add", None, Some(1), "fn fresh() {"),
                ("add", None, Some(2), "    new_body();"),
                ("context", Some(3), Some(3), "}"),
            ]
        );
    }

    #[test]
    fn slice_with_no_span_at_all_is_empty() {
        let diff = line_diff("a\n", "b\n");
        assert!(slice_diff(&diff, None, None).is_empty());
    }

    #[test]
    fn inclusive_spans_convert_to_half_open() {
        let range = LineRange::inclusive(4, 6);
        assert_eq!(range, LineRange::new(4, 7));
        assert!(range.contains(4));
        assert!(range.contains(6));
        assert!(!range.contains(7));
    }
}
