//! Line-level diffing.
//!
//! Produces the raw material every later stage consumes: a flat list of
//! [`DiffLine`]s carrying both sides' line numbers, and a way to slice the file
//! diff down to one symbol's span.

use similar::{DiffOp, TextDiff};

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

    // `similar` hands back an edit script; git's compaction works on the pair
    // of changed-flag arrays behind it, so rebuild them. A non-`Equal` op
    // carries the exact range it touches on each side — empty on the side a
    // pure insertion or deletion leaves alone — so no per-line bookkeeping is
    // needed and no line can be silently dropped.
    let mut changed_old = vec![false; old_lines.len()];
    let mut changed_new = vec![false; new_lines.len()];
    for op in diff.ops() {
        if !matches!(op, DiffOp::Equal { .. }) {
            changed_old[op.old_range()].fill(true);
            changed_new[op.new_range()].fill(true);
        }
    }

    // Exactly git's order (`xdl_do_diff`): the old side against the new, then
    // the new side against the old *as already compacted*.
    compact(&old_lines, &mut changed_old, &changed_new);
    compact(&new_lines, &mut changed_new, &changed_old);

    emit(&old_lines, &new_lines, &changed_old, &changed_new)
}

/// Turn the two changed-flag arrays back into the flat list.
///
/// At every position the maximal run of changed old lines is emitted first,
/// then the maximal run of changed new lines, then the one unchanged line that
/// carries both numbers — which keeps a replacement's deletions ahead of its
/// insertions, the order `similar` produced and every caller reads.
fn emit(
    old_lines: &[&str],
    new_lines: &[&str],
    changed_old: &[bool],
    changed_new: &[bool],
) -> Vec<DiffLine> {
    let mut out = Vec::with_capacity(old_lines.len() + new_lines.len());
    let (mut i, mut j) = (0usize, 0usize);

    loop {
        while i < old_lines.len() && changed_old[i] {
            out.push(DiffLine {
                tag: DiffTag::Del,
                old_line: Some(i as u32 + 1),
                new_line: None,
                text: strip_eol(old_lines[i]).to_string(),
            });
            i += 1;
        }
        while j < new_lines.len() && changed_new[j] {
            out.push(DiffLine {
                tag: DiffTag::Add,
                old_line: None,
                new_line: Some(j as u32 + 1),
                text: strip_eol(new_lines[j]).to_string(),
            });
            j += 1;
        }
        if i == old_lines.len() || j == new_lines.len() {
            // Both sides hold the same number of unchanged lines, so they run
            // out together; anything else means the flags disagree.
            debug_assert!(
                i == old_lines.len() && j == new_lines.len(),
                "unchanged lines out of step: old {i}/{}, new {j}/{}",
                old_lines.len(),
                new_lines.len()
            );
            // In release, flush whatever the other side still holds rather
            // than dropping it: a line reported as changed when it was only
            // unpaired is a diff a reviewer can still read, where a silently
            // truncated one loses changes no card would ever show.
            while i < old_lines.len() {
                out.push(DiffLine {
                    tag: DiffTag::Del,
                    old_line: Some(i as u32 + 1),
                    new_line: None,
                    text: strip_eol(old_lines[i]).to_string(),
                });
                i += 1;
            }
            while j < new_lines.len() {
                out.push(DiffLine {
                    tag: DiffTag::Add,
                    old_line: None,
                    new_line: Some(j as u32 + 1),
                    text: strip_eol(new_lines[j]).to_string(),
                });
                j += 1;
            }
            break;
        }
        out.push(DiffLine {
            tag: DiffTag::Context,
            old_line: Some(i as u32 + 1),
            new_line: Some(j as u32 + 1),
            text: strip_eol(old_lines[i]).to_string(),
        });
        i += 1;
        j += 1;
    }

    out
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
const INDENT_HEURISTIC_MAX_SLIDING: isize = 100;

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
fn score_cmp(s1: &SplitScore, s2: &SplitScore) -> i32 {
    let cmp_indents = i32::from(s1.effective_indent > s2.effective_indent)
        - i32::from(s1.effective_indent < s2.effective_indent);

    INDENT_WEIGHT * cmp_indents + (s1.penalty - s2.penalty)
}

// ---------------------------------------------------------------------------
// Change compaction: a port of git's `xdl_change_compact`.
//
// `similar` runs its own git-like compaction and leaves every slidable group
// in its fully-slid-down position — git's `--no-indent-heuristic` state. What
// follows is the rest of git's `xdiff/xdiffi.c`: the group cursor
// (`struct xdlgroup`, `group_init`, `group_next`, `group_previous`,
// `group_slide_up`, `group_slide_down`) and `xdl_change_compact` itself, run
// with `XDF_INDENT_HEURISTIC` unconditionally set, so a slid block lands where
// `git diff` puts it.
//
// Where the C calls `XDL_BUG` on a broken group-sync invariant, the port
// `debug_assert!`s and, in release, stops compacting and leaves the diff in
// its last consistent state: sliding is a semantics-preserving rewrite, so a
// half-compacted pair of flag arrays is still a correct diff, and degradation
// beats a panic in the analysis path.
// ---------------------------------------------------------------------------

/// A run of changed lines, `start..end`, plus the cursor moves over it.
///
/// `start` is the index of the first changed line, `end` the index of the
/// first unchanged line after the group; an empty group has `start == end` and
/// sits above the unchanged line at that index. The C indexes an `rchg` array
/// padded with a zero at `-1` and at `n`, which is why it needs no bounds
/// checks; here the bounds are checked instead.
#[derive(Debug, Clone, Copy)]
struct Group {
    start: usize,
    end: usize,
}

impl Group {
    /// The first (possibly empty) group of `changed`.
    fn init(changed: &[bool]) -> Self {
        let mut end = 0;
        while end < changed.len() && changed[end] {
            end += 1;
        }
        Self { start: 0, end }
    }

    fn is_empty(&self) -> bool {
        self.start == self.end
    }

    /// Move to the next (possibly empty) group. `false` at the end of file.
    fn next(&mut self, changed: &[bool]) -> bool {
        if self.end == changed.len() {
            return false;
        }
        self.start = self.end + 1;
        self.end = self.start;
        while self.end < changed.len() && changed[self.end] {
            self.end += 1;
        }
        true
    }

    /// Move to the previous (possibly empty) group. `false` at the start.
    fn previous(&mut self, changed: &[bool]) -> bool {
        if self.start == 0 {
            return false;
        }
        self.end = self.start - 1;
        self.start = self.end;
        while self.start > 0 && changed[self.start - 1] {
            self.start -= 1;
        }
        true
    }

    /// Slide toward the end of the file, absorbing any group bumped into.
    ///
    /// Possible exactly when the first line of the group repeats the line just
    /// below it — the same equality `similar` diffed on, terminator included.
    fn slide_down(&mut self, lines: &[&str], changed: &mut [bool]) -> bool {
        if self.end < lines.len() && lines[self.start] == lines[self.end] {
            changed[self.start] = false;
            self.start += 1;
            changed[self.end] = true;
            self.end += 1;
            while self.end < changed.len() && changed[self.end] {
                self.end += 1;
            }
            true
        } else {
            false
        }
    }

    /// Slide toward the start of the file, absorbing any group bumped into.
    fn slide_up(&mut self, lines: &[&str], changed: &mut [bool]) -> bool {
        if self.start > 0 && self.end > 0 && lines[self.start - 1] == lines[self.end - 1] {
            self.start -= 1;
            changed[self.start] = true;
            self.end -= 1;
            changed[self.end] = false;
            while self.start > 0 && changed[self.start - 1] {
                self.start -= 1;
            }
            true
        } else {
            false
        }
    }
}

/// Shift every change group of one side into its most intuitive position.
///
/// `changed` is the side being compacted, `other_changed` the same file's
/// counterpart: the two are walked in lockstep, group *k* of one always facing
/// group *k* of the other, because both sides hold the same unchanged lines in
/// the same order.
fn compact(lines: &[&str], changed: &mut [bool], other_changed: &[bool]) {
    debug_assert_eq!(lines.len(), changed.len());

    let mut g = Group::init(changed);
    let mut go = Group::init(other_changed);

    loop {
        // An empty group in the side being compacted has nothing to slide.
        if !g.is_empty() {
            let mut groupsize;
            let mut earliest_end;
            let mut end_matching_other;

            // Shift up and then down as far as possible, merging whatever is
            // bumped into, until the group stops growing.
            loop {
                groupsize = g.end - g.start;

                // The last `end` at which this group lines up with a group of
                // changed lines in the other file, if any.
                end_matching_other = None;

                while g.slide_up(lines, changed) {
                    let synced = go.previous(other_changed);
                    debug_assert!(synced, "group sync broken sliding up");
                    if !synced {
                        return;
                    }
                }

                // The highest this group can be shifted.
                earliest_end = g.end;

                if !go.is_empty() {
                    end_matching_other = Some(g.end);
                }

                while g.slide_down(lines, changed) {
                    let synced = go.next(other_changed);
                    debug_assert!(synced, "group sync broken sliding down");
                    if !synced {
                        return;
                    }
                    if !go.is_empty() {
                        end_matching_other = Some(g.end);
                    }
                }

                if groupsize == g.end - g.start {
                    break;
                }
            }

            // The group now sits as far down as it can, so every remaining
            // choice is an upwards shift.
            if g.end == earliest_end {
                // It could not be shifted at all.
            } else if end_matching_other.is_some() {
                // Line the group up with the last group of changes on the
                // other side that it can align with.
                while go.is_empty() {
                    let slid = g.slide_up(lines, changed);
                    debug_assert!(slid, "match disappeared");
                    if !slid {
                        return;
                    }
                    let synced = go.previous(other_changed);
                    debug_assert!(synced, "group sync broken sliding to match");
                    if !synced {
                        return;
                    }
                }
            } else {
                let best_shift = best_indent_shift(lines, g.end, groupsize, earliest_end);
                while g.end > best_shift {
                    let slid = g.slide_up(lines, changed);
                    debug_assert!(slid, "best shift unreached");
                    if !slid {
                        return;
                    }
                    let synced = go.previous(other_changed);
                    debug_assert!(synced, "group sync broken sliding to blank line");
                    if !synced {
                        return;
                    }
                }
            }
        }

        // Move past the just-processed group.
        if !g.next(changed) {
            break;
        }
        let synced = go.next(other_changed);
        debug_assert!(synced, "group sync broken moving to next group");
        if !synced {
            return;
        }
    }

    // Hoisted out of the assertion: `debug_assert!` is compiled out in release,
    // and a cursor move inside one would make the two profiles walk `go` a
    // different number of times.
    let other_ran_out = !go.next(other_changed);
    debug_assert!(other_ran_out, "group sync broken at end of file");
}

/// The `end` index this group should be slid to, per the indent heuristic.
///
/// A group of pure additions or deletions implies two splits — one above it
/// and one below — and each candidate position scores the sum of the two. The
/// lowest score wins, and `score_cmp`'s `<= 0` tie-break keeps the *latest*
/// best position, as the C does.
fn best_indent_shift(
    lines: &[&str],
    group_end: usize,
    groupsize: usize,
    earliest_end: usize,
) -> usize {
    let group_end = group_end as isize;
    let groupsize = groupsize as isize;

    let mut shift = earliest_end as isize;
    shift = shift.max(group_end - groupsize - 1);
    shift = shift.max(group_end - INDENT_HEURISTIC_MAX_SLIDING);

    let mut best_shift: Option<usize> = None;
    let mut best_score = SplitScore::default();

    while shift <= group_end {
        let mut score = SplitScore::default();
        score_add_split(&measure_split(lines, shift as usize), &mut score);
        // `shift` never drops below `groupsize`: `earliest_end` is the group's
        // `end` with its `start` at 0 or above, so the clamp above cannot take
        // it negative. `max(0)` only spares the cast an assumption.
        score_add_split(
            &measure_split(lines, (shift - groupsize).max(0) as usize),
            &mut score,
        );
        if best_shift.is_none() || score_cmp(&score, &best_score) <= 0 {
            best_score = score;
            best_shift = Some(shift as usize);
        }
        shift += 1;
    }

    best_shift.unwrap_or(group_end as usize)
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

    /// The last line of a file can end in a bare `CR` with no `\n` to strip it,
    /// and that terminator must not survive into the card's text.
    #[test]
    fn a_final_bare_carriage_return_is_stripped_too() {
        assert_eq!(
            render(&line_diff("", "a\nlast\r")),
            vec![("add", None, Some(1), "a"), ("add", None, Some(2), "last")]
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

    /// The case the whole port exists for. `similar` leaves the inserted block
    /// fully slid down, which puts the new function's `#[test]` on the line
    /// *above* the block and hands the pre-existing `fn b` a stray attribute.
    /// Git's heuristic slides it back so the block is the whole function.
    ///
    /// Cross-checked against
    /// `git diff --no-index --indent-heuristic` on the same two texts.
    #[test]
    fn an_inserted_function_takes_its_own_attribute_line() {
        let old = "#[test]\nfn a() {}\n\n#[test]\nfn b() {}\n";
        let new = "#[test]\nfn a() {}\n\n#[test]\nfn new() {}\n\n#[test]\nfn b() {}\n";
        assert_eq!(
            render(&line_diff(old, new)),
            vec![
                ("context", Some(1), Some(1), "#[test]"),
                ("context", Some(2), Some(2), "fn a() {}"),
                ("context", Some(3), Some(3), ""),
                ("add", None, Some(4), "#[test]"),
                ("add", None, Some(5), "fn new() {}"),
                ("add", None, Some(6), ""),
                ("context", Some(4), Some(7), "#[test]"),
                ("context", Some(5), Some(8), "fn b() {}"),
            ]
        );
    }

    #[test]
    fn a_group_that_cannot_slide_is_left_where_it_is() {
        // "Q" repeats neither neighbour, so the insertion has one position.
        assert_eq!(
            render(&line_diff("a\nb\nc\n", "a\nQ\nb\nc\n")),
            vec![
                ("context", Some(1), Some(1), "a"),
                ("add", None, Some(2), "Q"),
                ("context", Some(2), Some(3), "b"),
                ("context", Some(3), Some(4), "c"),
            ]
        );
    }

    /// The slide range runs into the start of the file, and the shallower
    /// split wins there: the addition takes line 1, not line 2.
    #[test]
    fn a_slide_bounded_by_the_start_of_file_can_still_shift_up() {
        assert_eq!(
            render(&line_diff("x\n    y\n", "x\nx\n    y\n")),
            vec![
                ("add", None, Some(1), "x"),
                ("context", Some(1), Some(2), "x"),
                ("context", Some(2), Some(3), "    y"),
            ]
        );
    }

    /// A function appended after one that ends in the same `}`: the block can
    /// slide up over that brace, and the end of the file wins.
    #[test]
    fn a_slide_bounded_by_the_end_of_file_stays_at_the_end() {
        let old = "fn a() {\n    one();\n}\n";
        let new = "fn a() {\n    one();\n}\n\nfn b() {\n    two();\n}\n";
        assert_eq!(
            render(&line_diff(old, new)),
            vec![
                ("context", Some(1), Some(1), "fn a() {"),
                ("context", Some(2), Some(2), "    one();"),
                ("context", Some(3), Some(3), "}"),
                ("add", None, Some(4), ""),
                ("add", None, Some(5), "fn b() {"),
                ("add", None, Some(6), "    two();"),
                ("add", None, Some(7), "}"),
            ]
        );
    }

    /// When a slidable insertion can be positioned against a deletion on the
    /// other side, git puts it there and never consults the heuristic — the
    /// two changes read as one replacement instead of two unrelated hunks.
    #[test]
    fn a_slide_that_can_meet_the_other_sides_change_is_aligned_with_it() {
        let old = "a\nb\nX\na\nb\nz\n";
        let new = "a\nb\na\nb\na\nb\nz\n";
        assert_eq!(
            render(&line_diff(old, new)),
            vec![
                ("context", Some(1), Some(1), "a"),
                ("context", Some(2), Some(2), "b"),
                ("del", Some(3), None, "X"),
                ("add", None, Some(3), "a"),
                ("add", None, Some(4), "b"),
                ("context", Some(4), Some(5), "a"),
                ("context", Some(5), Some(6), "b"),
                ("context", Some(6), Some(7), "z"),
            ]
        );
    }

    /// The other side's change can also already be facing the group at the
    /// *top* of its slide range, before it has slid down at all — here the
    /// inserted `a` reaches the deletion of `X` only by sliding as far up as it
    /// can go. Cross-checked against `git diff --no-index --indent-heuristic`.
    #[test]
    fn a_slide_already_facing_the_other_sides_change_at_its_top_stays_there() {
        assert_eq!(
            render(&line_diff("X\na\na\nz\n", "a\na\na\nz\n")),
            vec![
                ("del", Some(1), None, "X"),
                ("add", None, Some(1), "a"),
                ("context", Some(2), Some(2), "a"),
                ("context", Some(3), Some(3), "a"),
                ("context", Some(4), Some(4), "z"),
            ]
        );
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

    /// A blank run longer than [`MAX_BLANKS`] stops the scan on either side:
    /// the count sticks at the bound and the indent is reported as `0` rather
    /// than that of the non-blank line beyond it. Without the clamp the loops
    /// are unbounded over a long run of blank lines.
    #[test]
    fn measure_split_stops_counting_after_max_blanks() {
        // "a", 25 blank lines, "    b" — a run five longer than the bound.
        let mut lines = vec!["a\n"];
        lines.extend(std::iter::repeat_n("\n", 25));
        lines.push("    b\n");

        // Scanning upwards from the non-blank line at the bottom.
        let below = measure_split(&lines, 26);
        assert_eq!(below.indent, 4);
        assert_eq!(below.pre_blank, MAX_BLANKS);
        assert_eq!(below.pre_indent, 0);

        // Scanning downwards from the non-blank line at the top.
        let above = measure_split(&lines, 0);
        assert_eq!(above.indent, 0);
        assert_eq!(above.post_blank, MAX_BLANKS);
        assert_eq!(above.post_indent, 0);
    }

    /// Every candidate position scores the same in a run of identical lines,
    /// and `score_cmp`'s `<= 0` tie-break has to leave the group at the
    /// *latest* of them — git's answer, and the one `similar` already produced.
    #[test]
    fn best_indent_shift_keeps_the_latest_of_equally_good_positions() {
        let lines = ["x\n"; 10];
        assert_eq!(best_indent_shift(&lines, 8, 1, 4), 8);
    }

    /// A group is never slid further up than its own length: the two splits
    /// being measured are `groupsize` apart and must not cross. Lines 0..17
    /// here are unindented and would score better, but sit out of reach.
    #[test]
    fn best_indent_shift_never_reaches_past_the_groups_own_length() {
        let mut lines = vec!["a\n"; 17];
        lines.extend(std::iter::repeat_n("        b\n", 13));

        assert_eq!(best_indent_shift(&lines, 20, 2, 0), 20 - 2 - 1);
    }

    /// [`INDENT_HEURISTIC_MAX_SLIDING`] bounds the search independently of the
    /// group's length, which is the only thing keeping the scan off a group of
    /// hundreds of slidable lines. Without it the shallow region below 150 is
    /// reachable and wins.
    #[test]
    fn best_indent_shift_stops_at_the_max_sliding_bound() {
        let mut lines = vec!["a\n"; 150];
        lines.extend(std::iter::repeat_n("        b\n", 150));

        assert_eq!(
            best_indent_shift(&lines, 250, 120, 0),
            250 - INDENT_HEURISTIC_MAX_SLIDING as usize
        );
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

    const EXPORT_HTML_BEFORE: &str =
        include_str!("../../tests/fixtures/rust/slider_export_html/before.rs");
    const EXPORT_HTML_AFTER: &str =
        include_str!("../../tests/fixtures/rust/slider_export_html/after.rs");
    const EXPORT_HTML_EXPECTED: &str =
        include_str!("../../tests/fixtures/rust/slider_export_html/expected.diff");

    /// Read a unified diff back into `(tag, old_line, new_line)` triples.
    ///
    /// Only ever fed the one fixture below, so it handles exactly what git
    /// wrote there: the `diff`/`index`/`---`/`+++` preamble is skipped, the
    /// single `@@` header supplies the two starting line numbers, and every
    /// line after it advances the side(s) it belongs to.
    fn parse_unified(text: &str) -> Vec<(&'static str, Option<u32>, Option<u32>)> {
        let mut out = Vec::new();
        let (mut old, mut new) = (0u32, 0u32);
        let mut in_hunk = false;

        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("@@ -") {
                let (old_part, rest) = rest.split_once(" +").expect("@@ header has both sides");
                let (new_part, _) = rest.split_once(" @@").expect("@@ header is terminated");
                let start = |spec: &str| -> u32 {
                    spec.split(',')
                        .next()
                        .expect("a line spec starts with a number")
                        .parse()
                        .expect("a line spec starts with a number")
                };
                old = start(old_part);
                new = start(new_part);
                in_hunk = true;
                continue;
            }
            if !in_hunk {
                continue;
            }
            match line.as_bytes().first() {
                Some(b' ') => {
                    out.push(("context", Some(old), Some(new)));
                    old += 1;
                    new += 1;
                }
                Some(b'-') => {
                    out.push(("del", Some(old), None));
                    old += 1;
                }
                Some(b'+') => {
                    out.push(("add", None, Some(new)));
                    new += 1;
                }
                // "\ No newline at end of file" carries no line of its own.
                Some(b'\\') => {}
                // A blank context line is written as a single space, so an
                // empty line here means the fixture lost its trailing
                // whitespace. Skipping it silently would desynchronise both
                // counters and report a slider bug that is not there.
                other => panic!("malformed fixture line: {other:?} in {line:?}"),
            }
        }
        out
    }

    /// The pair from the ticket, checked against git's own answer.
    ///
    /// The fixtures are `tests/export_html.rs` at `126e29d~1` and `126e29d`,
    /// materialised with `git show <rev>:tests/export_html.rs`. `expected.diff`
    /// is verbatim
    /// `git diff --no-index --indent-heuristic --unified=100000 before.rs after.rs`
    /// — the whole file in one hunk, so context lines are compared too. Only
    /// tags and line numbers are compared; hunk headers and text are git's
    /// formatting, not our contract.
    #[test]
    fn the_export_html_pair_matches_gits_indent_heuristic_output() {
        let ours: Vec<(&'static str, Option<u32>, Option<u32>)> =
            line_diff(EXPORT_HTML_BEFORE, EXPORT_HTML_AFTER)
                .iter()
                .map(|line| {
                    let tag = match line.tag {
                        DiffTag::Add => "add",
                        DiffTag::Del => "del",
                        DiffTag::Context => "context",
                    };
                    (tag, line.old_line, line.new_line)
                })
                .collect();

        assert_eq!(ours, parse_unified(EXPORT_HTML_EXPECTED));
    }
}
