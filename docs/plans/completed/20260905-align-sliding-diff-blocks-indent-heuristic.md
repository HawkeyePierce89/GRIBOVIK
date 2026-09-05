# Align sliding diff blocks the way git does (indent heuristic)

## Overview

`line_diff` currently emits whatever `similar` produces. `similar` 3.2 runs its own
git-like compaction (`algorithms/compact.rs`: shift each change group up, then down),
which lands every slidable group in its fully-slid-down position — exactly git's
`--no-indent-heuristic` state. Git's default, and therefore GitHub's rendering, adds
`XDF_INDENT_HEURISTIC`: it slides the group back up to the position with the best
split score.

For GRIBOVIK the difference is not cosmetic. On `tests/export_html.rs` between
`126e29d~1` and `126e29d` the slid-down placement puts a stray `+#[test]` inside the
span of the *unchanged* `export_creates_missing_parent_directories`, producing a false
"modified" card, while the genuinely added function shows its own `#[test]` as context.

This plan replaces the body of `line_diff` with a faithful port of git's
`xdl_change_compact` + indent heuristic, operating on changed-flag arrays reconstructed
from `similar`'s output. Pure text in, `Vec<DiffLine>` out; no new dependencies; no git
invocation at test time.

Per the answered question, no runtime assertion is added in the node layer: an `added`
or `deleted` card can legitimately carry context lines (a renamed `impl`/class hands
every member a slice of pure context; `diff.rs`'s own
`slice_with_one_side_missing_picks_up_that_sides_lines_only` shows a deleted symbol
carrying a context `}`). The invariant is covered by the new fixture tests, and
CLAUDE.md gets an explicit paragraph explaining why the strict form is not an invariant.

## Context

Files involved:

- Modify: `src/core/diff.rs` — the whole change to the analysis lives here
- Create: `tests/fixtures/rust/slider_attribute/{before,after}.rs`
- Create: `tests/fixtures/swift/slider_attribute/{before,after}.swift`
- Create: `tests/fixtures/rust/slider_export_html/{before,after}.rs` and `expected.diff`
- Modify: `src/core/nodes.rs` — `#[cfg(test)]` module only (new card-level tests)
- Modify: `CLAUDE.md` — two short paragraphs in the diff/cards section

Reference implementation (read-only, local, no network needed):
`/Users/antonkarmanov/git/pisaka/SourcePackages/checkouts/libgit2/deps/xdiff/xdiffi.c`
— libgit2's vendored copy of git's xdiff; the indent-heuristic code is identical to
git's. The parts to port are `get_indent` (l.412), `struct split_measurement` (l.444),
`measure_split` (l.490), the weight constants (l.543-585), `score_add_split` (l.597),
`score_cmp` (l.675), `struct xdlgroup` + `group_init`/`group_next`/`group_previous`/
`group_slide_down`/`group_slide_up` (l.699-800) and `xdl_change_compact` (l.802).
If that path is unavailable, the same file is at
`/Users/antonkarmanov/Library/Developer/Xcode/DerivedData/Pisaka-*/SourcePackages/checkouts/libgit2/deps/xdiff/xdiffi.c`.

Related patterns:

- Core purity: `src/core/` never runs a process or touches the filesystem. The port is
  pure string work; `include_str!` of fixtures in `#[cfg(test)]` is the established way
  to feed it real files (see `src/core/lang/rust.rs:193`, `src/core/nodes.rs:427`).
- Unit tests live in `#[cfg(test)] mod tests` beside the code; `diff.rs` already has a
  `render()` helper rendering a diff as `(tag, old, new, text)` tuples — new tests reuse it.
- Fixture pairs live under `tests/fixtures/<lang>/<case>/{before,after}.<ext>`.

Dependencies: none added. `similar = "3.2.0"` stays as the base algorithm.

## Development Approach

- **Testing approach**: Regular (code first, then tests), except Task 3 and Task 4 where
  the fixture and its expected card set are written before the assertion is made to pass —
  those tests must fail against the current `master` behaviour if the port were reverted.
- Complete each task fully before moving to the next.
- Port the C faithfully: same constants, same comparison order, same
  `INDENT_HEURISTIC_MAX_SLIDING` clamp, same `<= 0` tie-break in `score_cmp`. Do not
  invent a simplified scoring — the acceptance test compares against real git output.
- Where git calls `XDL_BUG` (group-sync invariants), the Rust port must not panic:
  `debug_assert!` the invariant and, in release, stop compacting and return the diff as
  it stands. Degradation beats failure in the analysis path.
- **CRITICAL: every task MUST include new/updated tests**
- **CRITICAL: all tests must pass before starting next task**

## Implementation Steps

### Task 1: Port git's split scoring into diff.rs

**Files:**
- Modify: `src/core/diff.rs`

- [x] Add a private section to `diff.rs`, headed by a doc comment naming the source
      (`git's xdiff/xdiffi.c`, `XDF_INDENT_HEURISTIC`) and stating that the constants are
      empirical and must not be tuned.
- [x] Port `get_indent(line: &str) -> i32`: tab advances to the next multiple of 8, space
      counts 1, other whitespace ignored, first non-whitespace byte ends the scan, clamp at
      `MAX_INDENT = 200`, return `-1` for a line that is entirely whitespace. Lines here
      come from `split_inclusive('\n')` and still carry their terminator — `\n` and `\r`
      are whitespace, so a blank line correctly scores `-1`.
- [x] Port `SplitMeasurement { end_of_file, indent, pre_blank, pre_indent, post_blank,
      post_indent }` and `measure_split(lines: &[&str], split: usize) -> SplitMeasurement`,
      including the `MAX_BLANKS = 20` cutoff that sets the indent to 0.
- [x] Port the weight constants verbatim: `START_OF_FILE_PENALTY 1`, `END_OF_FILE_PENALTY 21`,
      `TOTAL_BLANK_WEIGHT -30`, `POST_BLANK_WEIGHT 6`, `RELATIVE_INDENT_PENALTY -4`,
      `RELATIVE_INDENT_WITH_BLANK_PENALTY 10`, `RELATIVE_OUTDENT_PENALTY 24`,
      `RELATIVE_OUTDENT_WITH_BLANK_PENALTY 17`, `RELATIVE_DEDENT_PENALTY 23`,
      `RELATIVE_DEDENT_WITH_BLANK_PENALTY 17`, `INDENT_WEIGHT 60`,
      `INDENT_HEURISTIC_MAX_SLIDING 100`.
- [x] Port `SplitScore { effective_indent, penalty }`, `score_add_split` (accumulating into
      an existing score, as the C does) and `score_cmp` returning the same signed integer.
- [x] Write unit tests for the primitives: `get_indent` on spaces, tabs, mixed tab/space,
      a blank line, a whitespace-only line, an over-indented line clamped to 200; a
      `measure_split` case at the start of file, at the end of file, and around a run of
      blank lines; a `score_cmp` case where the lower effective indent wins despite a
      higher penalty (the `INDENT_WEIGHT` dominance).
- [x] `cargo test` — must pass before Task 2.

### Task 2: Rebuild line_diff around changed-flag arrays and compaction

**Files:**
- Modify: `src/core/diff.rs`

- [x] Reconstruct changed-flag arrays from `similar`'s output: `changed_old[i]` for each
      `ChangeTag::Delete` at old index `i`, `changed_new[j]` for each `ChangeTag::Insert`
      at new index `j`. Keep the existing `split_inclusive('\n')` tokenization and the
      `\n`-only rationale in the doc comment unchanged.
- [x] Port the group cursor (`Group { start, end }`, `group_init`, `group_next`,
      `group_previous`, `group_slide_up`, `group_slide_down`), comparing lines by their
      full slice including the terminator — the same equality `similar` used.
- [x] Port `xdl_change_compact` as
      `compact(lines: &[&str], changed: &mut [bool], other_changed: &[bool])`: the
      up-then-down shift loop until the group size stabilises, `earliest_end` /
      `end_matching_other` bookkeeping, the "align with the other file's group" branch, and
      the indent-heuristic branch with the `earliest_end` / `g.end - groupsize - 1` /
      `g.end - INDENT_HEURISTIC_MAX_SLIDING` lower bound on `shift` and the `<= 0`
      tie-break that prefers the latest best shift.
- [x] Call it twice, exactly as git does: once for the old side with the new side as
      "other", then once for the new side with the (already updated) old side as "other".
- [x] Re-emit `Vec<DiffLine>` from the two flag arrays: at each position emit the maximal
      run of changed old lines as `del`, then the maximal run of changed new lines as `add`,
      then one `context` line carrying both numbers. This preserves the existing ordering
      (`del`s before `add`s within a replacement) and the invariant that every line of both
      revisions appears exactly once. Keep `strip_eol` for the text.
- [x] Replace the `XDL_BUG` sites with `debug_assert!` plus a release-mode early return
      that leaves the diff in its last consistent state; document why (degradation beats
      failure).
- [x] Keep all existing `diff.rs` tests unchanged and green
      (`pure_insertion_numbers_both_sides`, `whole_file_rewrite_replaces_every_line`, the
      slice tests, the CRLF test, …).
- [x] Add unit tests on small synthetic strings: a two-line block inserted between two
      identical `#[test]`-like lines lands *above* the shared line, not below; a group that
      cannot slide is left untouched; a group whose slide is bounded by the start of file
      and one bounded by the end of file; an insertion whose slide range would align it with
      a change on the other side (the `end_matching_other` branch) is placed there rather
      than by the heuristic.
- [x] `cargo test` — must pass before Task 3.

### Task 3: Slider fixtures and card-level tests (Rust and Swift)

**Files:**
- Create: `tests/fixtures/rust/slider_attribute/before.rs`, `after.rs`
- Create: `tests/fixtures/swift/slider_attribute/before.swift`, `after.swift`
- Modify: `src/core/nodes.rs` (`#[cfg(test)]` module only)
- Modify: `src/core/lang/swift.rs` (`#[cfg(test)]` module only)

- [x] Rust fixture: `before.rs` holds two top-level `#[test] fn` declarations separated by a
      blank line; `after.rs` inserts a third `#[test] fn` before the second one, again with a
      blank line between. Every function starts with the identical line `#[test]` — the
      condition that makes the block slidable. Keep them small (a handful of lines each).
- [x] Swift fixture: the same shape with three top-level `@MainActor func` declarations,
      since Swift attributes parse inside the declaration and so are part of the symbol's
      span, making `@MainActor` the analogue of `#[test]`.
- [x] In `src/core/lang/swift.rs` tests, assert the outline of the Swift fixture in the
      existing `name | qualified_name | kind | start-end` style, so the assumption that the
      span starts on the attribute line fails loudly if it does not hold. (If it does not,
      switch the Swift fixture's shared leading line to an identical `///` doc comment,
      which `lang::leading_line` provably absorbs, and keep the rest of the test as is.)
- [x] In `src/core/nodes.rs` tests, for each language: `build_nodes` on
      `FileInput::modified(path, BEFORE, AFTER)` yields exactly one node — the added
      function — with `ChangeKind::Added` and every line of its diff tagged `add`; the
      pre-existing function produces no node; no file-level node appears (the leftover blank
      line is empty and is filtered).
- [x] Symmetric deletion test: `FileInput::modified(path, AFTER, BEFORE)` on the same Rust
      fixture pair yields exactly one node, `ChangeKind::Deleted`, every line tagged `del`.
      Name it so the reversal is obvious in the test name and add a one-line comment saying
      the reversed pair *is* the symmetric case.
- [x] While writing the fixtures, cross-check each expectation once against
      `git diff --no-index --indent-heuristic` on the two fixture files (a development-time
      check run by hand; no git call enters any test).
- [x] `cargo test` — must pass before Task 4.

### Task 4: The real-pair regression test against git's own output

**Files:**
- Create: `tests/fixtures/rust/slider_export_html/before.rs`, `after.rs`, `expected.diff`
- Modify: `src/core/diff.rs` (`#[cfg(test)]` module only)
- Modify: `src/core/nodes.rs` (`#[cfg(test)]` module only)

- [x] Materialise the fixtures with
      `git show 126e29d~1:tests/export_html.rs > tests/fixtures/rust/slider_export_html/before.rs`
      and `git show 126e29d:tests/export_html.rs > .../after.rs` (197 and 231 lines).
- [x] Materialise the expectation with
      `git diff --no-index --indent-heuristic --unified=100000 before.rs after.rs > expected.diff`,
      so the whole file appears and the comparison covers context lines too. Leave the file
      exactly as git wrote it and document its provenance in the test's doc comment instead
      (the exact command and the revision pair).
- [x] Add a test in `diff.rs` that `include_str!`s the three files, parses `expected.diff`
      into `(tag, old_line, new_line)` triples — skipping the `diff`/`index`/`---`/`+++`
      lines, reading the starting line numbers from the single `@@` header and then
      advancing per line for ` `, `-`, `+` — and asserts equality against `line_diff`'s
      output projected to the same triples. The comparison is on tags and line numbers, not
      on hunk headers or text.
- [x] Add a test in `nodes.rs` on the same pair: `build_nodes` produces a node for
      `export_with_no_changes_exits_zero_through_the_binary` with `ChangeKind::Added`, and
      **no** node whose id ends in `export_creates_missing_parent_directories` — the exact
      defect from the ticket, caught in CI rather than by hand.
- [x] `cargo test` — must pass before Task 5.

### Task 5: Document the behaviour in CLAUDE.md

**Files:**
- Modify: `CLAUDE.md`

- [x] In the "two-sided GraphSnapshot contract" section (the paragraph block about diffs and
      cards), add a short paragraph: the line diff post-processes slider ambiguity the way
      git does — a run of inserted or deleted lines that begins and ends on a line identical
      to its neighbour has several valid placements, `similar` returns the fully-slid-down
      one (git's `--no-indent-heuristic`), and `diff.rs` slides it back with a faithful port
      of git's `XDF_INDENT_HEURISTIC` scoring. State why GRIBOVIK cannot tolerate the raw
      placement: a misplaced block hands lines to a neighbouring symbol, so the difference
      is a false review card, not a cosmetic one. Note that the constants are git's and are
      not to be tuned locally, and name the fixture that pins the behaviour.
- [x] Add a second short paragraph recording the deliberate absence of a strict
      added/deleted-versus-tag assertion: an `added` card carries no `del` lines and a
      `deleted` card no `add` lines by construction of `slice_diff`, but **context lines on
      an added or deleted card are legitimate** — a renamed `impl`, class or `extension`
      gives every member a slice of pure context, and a symbol sharing its closing brace
      with the previous revision keeps that brace as context. The slider fixtures, not a
      `debug_assert!`, are what catch a misalignment.
- [x] No test item: documentation only. `cargo test` must still pass.

### Task 6: Verify acceptance criteria

- [x] `cargo test`
- [x] `cargo clippy --all-targets -- -D warnings`
- [x] `cargo fmt --check`
- [x] `cd web && npm test && npm run typecheck` (unchanged, but the gate is part of the
      ticket; `just build-web` first if `web/dist` is absent, since `build.rs` refuses to
      compile without `web/dist/index.html` and `web/dist/export.html`)
- [x] Confirm no fixture, serde round-trip or integration test regressed and that
      `tests/integration_repo.rs` and `tests/export_html.rs` still pass unchanged

## Post-Completion (manual verification)

- Build the web assets once if needed (`just build-web`), then run
  `cargo run -- --export /tmp/slider.html 126e29d~1 126e29d` in this repository and confirm
  the graph contains no card for `export_creates_missing_parent_directories`, and that
  `export_with_no_changes_exits_zero_through_the_binary` appears as an added symbol whose
  every line is an addition.
- Spot-check one unrelated recent range (for example `cfab0d6~1..cfab0d6`) to confirm the
  post-pass did not disturb ordinary diffs.
