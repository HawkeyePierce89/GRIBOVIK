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

// ---------------------------------------------------------------------------
// Split scoring: a port of git's indent heuristic.
//
// Everything from here to `score_cmp` is a line-by-line port of the
// `XDF_INDENT_HEURISTIC` half of git's `xdiff/xdiffi.c` (`get_indent`,
// `struct split_measurement`, `measure_split`, the weight constants,
// `score_add_split` and `score_cmp`), reading from `&[&str]` where the C reads
// from an `xdfile_t`.
//
// The weights below are empirical — they come from the corpus fitting
// described at https://github.com/mhagger/diff-slider-tools — and they are
// git's, not ours. They must not be tuned locally: the point of the port is
// that a slidable block lands where `git diff` (and therefore GitHub) puts it,
// and any local adjustment silently breaks that agreement.
//
// The `#[allow(dead_code)]`s below are the scaffolding of the compaction pass
// that consumes these; they go away once `line_diff` calls it.
// ---------------------------------------------------------------------------

/// If a line is indented more than this, `get_indent` just returns this value.
const MAX_INDENT: i32 = 200;

/// If more than this many consecutive blank lines are found, stop counting.
const MAX_BLANKS: i32 = 20;

/// Penalty if there are no non-blank lines before the split.
const START_OF_FILE_PENALTY: i32 = 1;
/// Penalty if there are no non-blank lines after the split.
const END_OF_FILE_PENALTY: i32 = 21;
/// Multiplier for the number of blank lines around the split.
const TOTAL_BLANK_WEIGHT: i32 = -30;
/// Multiplier for the number of blank lines after the split.
const POST_BLANK_WEIGHT: i32 = 6;
/// Penalties applied if the line is indented more than its predecessor.
const RELATIVE_INDENT_PENALTY: i32 = -4;
const RELATIVE_INDENT_WITH_BLANK_PENALTY: i32 = 10;
/// Penalties applied if the line is indented less than both its predecessor
/// and its successor.
const RELATIVE_OUTDENT_PENALTY: i32 = 24;
const RELATIVE_OUTDENT_WITH_BLANK_PENALTY: i32 = 17;
/// Penalties applied if the line is indented less than its predecessor but not
/// less than its successor.
const RELATIVE_DEDENT_PENALTY: i32 = 23;
const RELATIVE_DEDENT_WITH_BLANK_PENALTY: i32 = 17;
/// Weight of the three-way comparison of two splits' effective indents. Large
/// enough to dominate the penalties, which is what makes the heuristic prefer
/// the shallower split.
const INDENT_WEIGHT: i32 = 60;
/// How far a group is slid at most.
#[allow(dead_code)]
const INDENT_HEURISTIC_MAX_SLIDING: usize = 100;

/// Whether `b` is whitespace in the C sense (`XDL_ISSPACE`).
fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// The indentation of `line` in columns, a tab advancing to the next multiple
/// of eight, clamped at [`MAX_INDENT`]. `-1` for a line that is blank or holds
/// nothing but whitespace.
///
/// Lines arrive from `split_inclusive('\n')` and still carry their terminator,
/// but `\n` and `\r` are whitespace here, so an empty line still scores `-1`.
fn get_indent(line: &str) -> i32 {
    let mut ret = 0;
    for &c in line.as_bytes() {
        if !is_space(c) {
            return ret;
        } else if c == b' ' {
            ret += 1;
        } else if c == b'\t' {
            ret += 8 - ret % 8;
        }
        // other whitespace characters are ignored

        if ret >= MAX_INDENT {
            return MAX_INDENT;
        }
    }
    // The line contains only whitespace.
    -1
}

/// What is measured about a hypothetical split position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SplitMeasurement {
    /// Is the split at the end of the file (aside from any blank lines)?
    end_of_file: bool,
    /// The indent of the line immediately following the split, or `-1` when it
    /// is blank.
    indent: i32,
    /// How many consecutive lines above the split are blank.
    pre_blank: i32,
    /// The indent of the nearest non-blank line above the split, or `-1`.
    pre_indent: i32,
    /// How many lines after the line following the split are blank.
    post_blank: i32,
    /// The indent of the nearest non-blank line after the line following the
    /// split, or `-1`.
    post_indent: i32,
}

/// A split's badness, smaller being better on both fields.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct SplitScore {
    effective_indent: i32,
    penalty: i32,
}

/// Measure a hypothetical split of `lines` above index `split`.
#[allow(dead_code)]
fn measure_split(lines: &[&str], split: usize) -> SplitMeasurement {
    let mut m = SplitMeasurement {
        end_of_file: split >= lines.len(),
        indent: if split >= lines.len() {
            -1
        } else {
            get_indent(lines[split])
        },
        pre_blank: 0,
        pre_indent: -1,
        post_blank: 0,
        post_indent: -1,
    };

    for i in (0..split.min(lines.len())).rev() {
        m.pre_indent = get_indent(lines[i]);
        if m.pre_indent != -1 {
            break;
        }
        m.pre_blank += 1;
        if m.pre_blank == MAX_BLANKS {
            m.pre_indent = 0;
            break;
        }
    }

    for line in lines.iter().skip(split.saturating_add(1)) {
        m.post_indent = get_indent(line);
        if m.post_indent != -1 {
            break;
        }
        m.post_blank += 1;
        if m.post_blank == MAX_BLANKS {
            m.post_indent = 0;
            break;
        }
    }

    m
}

/// Accumulate the badness of the split described by `m` into `s`, as the C
/// does — a group's score is the sum over its two ends.
#[allow(dead_code)]
fn score_add_split(m: &SplitMeasurement, s: &mut SplitScore) {
    if m.pre_indent == -1 && m.pre_blank == 0 {
        s.penalty += START_OF_FILE_PENALTY;
    }

    if m.end_of_file {
        s.penalty += END_OF_FILE_PENALTY;
    }

    // The number of blank lines following the split, including the line
    // immediately after it.
    let post_blank = if m.indent == -1 { 1 + m.post_blank } else { 0 };
    let total_blank = m.pre_blank + post_blank;

    s.penalty += TOTAL_BLANK_WEIGHT * total_blank;
    s.penalty += POST_BLANK_WEIGHT * post_blank;

    let indent = if m.indent != -1 {
        m.indent
    } else {
        m.post_indent
    };
    let any_blanks = total_blank != 0;

    // Note that the effective indent is -1 at the end of the file.
    s.effective_indent += indent;

    if indent == -1 || m.pre_indent == -1 {
        // No additional adjustments needed.
    } else if indent > m.pre_indent {
        // The line is indented more than its predecessor.
        s.penalty += if any_blanks {
            RELATIVE_INDENT_WITH_BLANK_PENALTY
        } else {
            RELATIVE_INDENT_PENALTY
        };
    } else if indent == m.pre_indent {
        // Same indentation as its predecessor: no adjustment.
    } else if m.post_indent != -1 && m.post_indent > indent {
        // Indented less than its predecessor, and the following line is
        // indented more — likely the start of a block.
        s.penalty += if any_blanks {
            RELATIVE_OUTDENT_WITH_BLANK_PENALTY
        } else {
            RELATIVE_OUTDENT_PENALTY
        };
    } else {
        // That was probably the end of a block.
        s.penalty += if any_blanks {
            RELATIVE_DEDENT_WITH_BLANK_PENALTY
        } else {
            RELATIVE_DEDENT_PENALTY
        };
    }
}

/// Negative when `s1` is the better split, positive when `s2` is.
///
/// Only the *sign* of the effective-indent difference is used, weighted by
/// [`INDENT_WEIGHT`], so a shallower split wins over any accumulation of
/// penalties short of that weight.
#[allow(dead_code)]
fn score_cmp(s1: &SplitScore, s2: &SplitScore) -> i32 {
    let cmp_indents = i32::from(s1.effective_indent > s2.effective_indent)
        - i32::from(s1.effective_indent < s2.effective_indent);

    INDENT_WEIGHT * cmp_indents + (s1.penalty - s2.penalty)
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

    #[test]
    fn get_indent_counts_spaces_and_tab_stops() {
        assert_eq!(get_indent("x\n"), 0);
        assert_eq!(get_indent("    x\n"), 4);
        // A tab advances to the next multiple of eight, whatever came before.
        assert_eq!(get_indent("\tx\n"), 8);
        assert_eq!(get_indent(" \tx\n"), 8);
        assert_eq!(get_indent("       \tx\n"), 8);
        assert_eq!(get_indent("\t\tx\n"), 16);
        assert_eq!(get_indent("\t  x\n"), 10);
    }

    #[test]
    fn get_indent_is_minus_one_for_lines_without_content() {
        // Lines still carry their terminator here, and it is whitespace.
        assert_eq!(get_indent("\n"), -1);
        assert_eq!(get_indent("   \n"), -1);
        assert_eq!(get_indent("\t\r\n"), -1);
        assert_eq!(get_indent(""), -1);
    }

    #[test]
    fn get_indent_clamps_at_max_indent() {
        let deep = format!("{}x\n", " ".repeat(300));
        assert_eq!(get_indent(&deep), MAX_INDENT);
    }

    #[test]
    fn measure_split_at_start_of_file_has_no_predecessor() {
        let lines = ["a\n", "    b\n"];
        let m = measure_split(&lines, 0);
        assert_eq!(
            m,
            SplitMeasurement {
                end_of_file: false,
                indent: 0,
                pre_blank: 0,
                pre_indent: -1,
                post_blank: 0,
                post_indent: 4,
            }
        );
    }

    #[test]
    fn measure_split_past_the_last_line_is_end_of_file() {
        let lines = ["a\n"];
        let m = measure_split(&lines, 1);
        assert_eq!(
            m,
            SplitMeasurement {
                end_of_file: true,
                indent: -1,
                pre_blank: 0,
                pre_indent: 0,
                post_blank: 0,
                post_indent: -1,
            }
        );
    }

    #[test]
    fn measure_split_counts_the_blank_run_on_each_side() {
        let lines = ["a\n", "\n", "\n", "    b\n"];

        // Below the blank run: two blank lines above, nothing after.
        let below = measure_split(&lines, 3);
        assert_eq!(below.indent, 4);
        assert_eq!(below.pre_blank, 2);
        assert_eq!(below.pre_indent, 0);
        assert_eq!(below.post_blank, 0);
        assert_eq!(below.post_indent, -1);

        // Above it: the split line is itself blank, and one more follows.
        let above = measure_split(&lines, 1);
        assert_eq!(above.indent, -1);
        assert_eq!(above.pre_blank, 0);
        assert_eq!(above.pre_indent, 0);
        assert_eq!(above.post_blank, 1);
        assert_eq!(above.post_indent, 4);
    }

    #[test]
    fn score_add_split_accumulates_the_end_of_file_penalty() {
        let lines = ["a\n"];
        let mut score = SplitScore::default();
        score_add_split(&measure_split(&lines, 1), &mut score);
        // 21 for the end of file, -30 for the one blank "line" past it, +6 for
        // that same line counting as post-blank; the effective indent is -1.
        assert_eq!(
            score,
            SplitScore {
                effective_indent: -1,
                penalty: -3,
            }
        );
    }

    #[test]
    fn score_cmp_lets_the_shallower_split_outweigh_a_worse_penalty() {
        let shallow = SplitScore {
            effective_indent: 0,
            penalty: 50,
        };
        let deep = SplitScore {
            effective_indent: 4,
            penalty: 0,
        };
        // INDENT_WEIGHT is 60, so a whole 50 points of penalty is not enough.
        assert!(score_cmp(&shallow, &deep) < 0);
        assert!(score_cmp(&deep, &shallow) > 0);

        // At equal indents the penalty is all there is.
        let same_indent = SplitScore {
            effective_indent: 0,
            penalty: 1,
        };
        assert!(score_cmp(&same_indent, &shallow) < 0);
        assert_eq!(score_cmp(&shallow, &shallow), 0);
    }
}
