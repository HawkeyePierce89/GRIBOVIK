//! Line-level diffing.
//!
//! Produces the raw material every later stage consumes: a flat list of
//! [`DiffLine`]s carrying both sides' line numbers, the [`Hunk`]s those lines
//! group into, and a way to slice the file diff down to one symbol's span.

use similar::{ChangeTag, TextDiff};

use crate::core::snapshot::{DiffLine, DiffTag};

/// A half-open span of 1-based line numbers: `start` is included, `end` is not.
///
/// The half-open form is what lets a hunk express "nothing was removed here" as
/// a zero-length range sitting at the insertion point.
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

    /// A zero-length range marking the position before `line`.
    pub fn empty_at(line: u32) -> Self {
        Self {
            start: line,
            end: line,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.end <= self.start
    }

    pub fn contains(&self, line: u32) -> bool {
        self.start <= line && line < self.end
    }

    /// Whether the two spans touch.
    ///
    /// Zero-length ranges are positions *between* lines rather than spans, so
    /// they count as touching whenever that position falls within the other
    /// range — otherwise a pure insertion inside a symbol would look disjoint
    /// from it on the old side.
    pub fn intersects(&self, other: &LineRange) -> bool {
        if self.is_empty() || other.is_empty() {
            let (point, span) = if self.is_empty() {
                (self, other)
            } else {
                (other, self)
            };
            return span.start <= point.start && point.start <= span.end;
        }
        self.start < other.end && other.start < self.end
    }
}

/// A run of consecutive changed lines, located on both sides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hunk {
    pub old_range: LineRange,
    pub new_range: LineRange,
}

/// Diff `old` against `new`, emitting one [`DiffLine`] per line of the result.
///
/// Every line of both revisions appears exactly once: context lines carry both
/// line numbers, additions only `new_line`, deletions only `old_line`. Trailing
/// newlines are stripped from `text`.
pub fn line_diff(old: &str, new: &str) -> Vec<DiffLine> {
    let diff = TextDiff::from_lines(old, new);
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

/// Group consecutive non-context lines into hunks.
///
/// A hunk that only adds lines gets a zero-length `old_range` at the position
/// the lines were inserted, and vice versa for a pure deletion.
pub fn hunks(diff: &[DiffLine]) -> Vec<Hunk> {
    let mut hunks = Vec::new();
    // Where the next line on each side would land, so a hunk that touches only
    // one side still knows its position on the other.
    let mut old_cursor = 1;
    let mut new_cursor = 1;
    let mut open: Option<Hunk> = None;

    for line in diff {
        match line.tag {
            DiffTag::Context => {
                if let Some(hunk) = open.take() {
                    hunks.push(hunk);
                }
            }
            _ => {
                let hunk = open.get_or_insert(Hunk {
                    old_range: LineRange::empty_at(old_cursor),
                    new_range: LineRange::empty_at(new_cursor),
                });
                if let Some(old) = line.old_line {
                    hunk.old_range.end = old + 1;
                }
                if let Some(new) = line.new_line {
                    hunk.new_range.end = new + 1;
                }
            }
        }
        if let Some(old) = line.old_line {
            old_cursor = old + 1;
        }
        if let Some(new) = line.new_line {
            new_cursor = new + 1;
        }
    }
    if let Some(hunk) = open {
        hunks.push(hunk);
    }
    hunks
}

/// Select the diff lines belonging to a symbol's span.
///
/// A line is kept when it sits inside the symbol's old span or its new span,
/// so a modified symbol picks up its deletions and its additions alike. Pass
/// `None` for the side a symbol does not exist on (added or deleted symbols).
pub fn slice_diff(
    diff: &[DiffLine],
    old_range: Option<LineRange>,
    new_range: Option<LineRange>,
) -> Vec<DiffLine> {
    diff.iter()
        .filter(|line| {
            let in_old = matches!((old_range, line.old_line), (Some(r), Some(l)) if r.contains(l));
            let in_new = matches!((new_range, line.new_line), (Some(r), Some(l)) if r.contains(l));
            in_old || in_new
        })
        .cloned()
        .collect()
}

/// Drop one trailing line terminator, leaving any other whitespace alone.
fn strip_eol(text: &str) -> &str {
    text.strip_suffix('\n')
        .map(|t| t.strip_suffix('\r').unwrap_or(t))
        .unwrap_or(text)
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

    fn ranges(hunks: &[Hunk]) -> Vec<((u32, u32), (u32, u32))> {
        hunks
            .iter()
            .map(|h| {
                (
                    (h.old_range.start, h.old_range.end),
                    (h.new_range.start, h.new_range.end),
                )
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
        // Nothing was removed, so the old side is a zero-length range parked at
        // the line the insertion happened before.
        assert_eq!(ranges(&hunks(&diff)), vec![((3, 3), (3, 5))]);
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
        assert_eq!(ranges(&hunks(&diff)), vec![((2, 4), (2, 2))]);
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
        assert_eq!(ranges(&hunks(&diff)), vec![((2, 3), (2, 3))]);
    }

    #[test]
    fn whole_file_rewrite_is_one_hunk() {
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
        assert_eq!(ranges(&hunks(&diff)), vec![((1, 3), (1, 3))]);
    }

    #[test]
    fn added_file_has_no_old_side() {
        let diff = line_diff("", "a\nb\n");
        assert_eq!(
            render(&diff),
            vec![("add", None, Some(1), "a"), ("add", None, Some(2), "b")]
        );
        assert_eq!(ranges(&hunks(&diff)), vec![((1, 1), (1, 3))]);
    }

    #[test]
    fn deleted_file_has_no_new_side() {
        let diff = line_diff("a\nb\n", "");
        assert_eq!(
            render(&diff),
            vec![("del", Some(1), None, "a"), ("del", Some(2), None, "b")]
        );
        assert_eq!(ranges(&hunks(&diff)), vec![((1, 3), (1, 1))]);
    }

    #[test]
    fn identical_sources_produce_no_hunks() {
        let diff = line_diff("a\nb\n", "a\nb\n");
        assert!(diff.iter().all(|l| l.tag == DiffTag::Context));
        assert!(hunks(&diff).is_empty());
    }

    #[test]
    fn context_lines_split_hunks() {
        let diff = line_diff("a\nb\nc\nd\ne\n", "A\nb\nc\nd\nE\n");
        assert_eq!(
            ranges(&hunks(&diff)),
            vec![((1, 2), (1, 2)), ((5, 6), (5, 6))]
        );
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

        let b = slice_diff(
            &diff,
            Some(LineRange::inclusive(5, 7)),
            Some(LineRange::inclusive(5, 7)),
        );
        assert_eq!(
            render(&b),
            vec![
                ("context", Some(5), Some(5), "fn b() {"),
                ("del", Some(6), None, "    two();"),
                ("add", None, Some(6), "    TWO();"),
                ("context", Some(7), Some(7), "}"),
            ]
        );

        let a = slice_diff(
            &diff,
            Some(LineRange::inclusive(1, 3)),
            Some(LineRange::inclusive(1, 3)),
        );
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
        let deleted = slice_diff(&diff, Some(LineRange::inclusive(1, 3)), None);
        assert_eq!(
            render(&deleted),
            vec![
                ("del", Some(1), None, "fn gone() {"),
                ("del", Some(2), None, "    old_body();"),
                ("context", Some(3), Some(3), "}"),
            ]
        );

        // An added one has no span in the old revision.
        let added = slice_diff(&diff, None, Some(LineRange::inclusive(1, 3)));
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
        assert!(!range.is_empty());
    }

    #[test]
    fn zero_length_ranges_touch_the_span_they_sit_in() {
        let body = LineRange::inclusive(10, 20);
        // A pure insertion inside the body reports an empty old range there.
        assert!(LineRange::empty_at(15).intersects(&body));
        assert!(body.intersects(&LineRange::empty_at(15)));
        // Boundaries count: inserting right before the first line, or right
        // after the last, still belongs to the symbol.
        assert!(LineRange::empty_at(10).intersects(&body));
        assert!(LineRange::empty_at(21).intersects(&body));
        assert!(!LineRange::empty_at(9).intersects(&body));
        assert!(!LineRange::empty_at(22).intersects(&body));
    }

    #[test]
    fn non_empty_ranges_intersect_on_overlap_only() {
        let body = LineRange::inclusive(10, 20);
        assert!(body.intersects(&LineRange::inclusive(20, 30)));
        assert!(body.intersects(&LineRange::inclusive(1, 10)));
        assert!(!body.intersects(&LineRange::inclusive(21, 30)));
        assert!(!body.intersects(&LineRange::inclusive(1, 9)));
    }
}
