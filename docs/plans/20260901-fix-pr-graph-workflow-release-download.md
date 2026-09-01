# Fix the PR review-graph workflow's release download, plus review leftovers

## Overview

`.github/workflows/pr-graph.yml` fails on every PR: the `Download release` step
`cd`s into `$RUNNER_TEMP` and then calls `gh release download` with neither
`--repo` nor `GH_REPO`, so `gh` tries to infer the repository from the working
directory, finds no git checkout, and exits 1 with
`failed to run git: fatal: not a git repository`.

This change gives `gh` its repository context explicitly, bumps the workflow's
two `@v4` action pins to `@v7` to match `release.yml`, makes the
`export::write` parent-directory creation contractual with a test, and rewraps
one paragraph in `CLAUDE.md`.

## Context

- Files involved:
  - `.github/workflows/pr-graph.yml` — the broken download step and the two `@v4` pins
  - `.github/workflows/release.yml` — reference only; already pins `@v7` and already sets `GH_REPO: ${{ github.repository }}` on its `gh release create` step. Not modified.
  - `src/export.rs` — `write()` already does `create_dir_all` on the output path's parent
  - `tests/export_html.rs` — end-to-end temp-repo -> export tests live here
  - `CLAUDE.md` — "The Export Mode" section, the `self-\ncontained` line break
- Related patterns:
  - Both existing workflows pin actions to major tags only (`@v7`, `@v8`), never to SHAs.
  - `release.yml`'s `Publish Release` step is the in-repo precedent for handing `gh` its repo via the `GH_REPO` env var rather than a `--repo` flag; following it keeps the two workflows consistent.
  - `tests/export_html.rs` builds a temp git repo, calls `cli::prepare`, matches on `Session::Export`, and calls `gribovik::export::write`. The new test follows that shape.
- Verified facts (checked while planning, re-verify during implementation):
  - `actions/checkout` latest release: `v7.0.1`; `actions/upload-artifact` latest release: `v7.0.1` — `v7` is the current major for both.
  - `HawkeyePierce89/GRIBOVIK` has a published latest release `v0.1.0` carrying `gribovik-0.1.0-x86_64-unknown-linux-gnu.tar.gz`, so the acceptance repro can actually download something. (That binary predates `--export`; that limitation is out of scope.)
  - `actionlint` and `gh` 2.98.0 are installed locally.
- Dependencies: none added.

## Development Approach

- **Testing approach**: Regular (code first, then tests). The export
  directory-creation behavior already exists; the test pins it down rather than
  driving it.
- Complete each task fully before moving to the next.
- Verification for the workflow task is `actionlint` plus a local reproduction
  of the exact failure from the review, run from an empty non-git directory —
  both automatable from the shell.
- **CRITICAL: every task MUST include new/updated tests or an equivalent
  executable check**
- **CRITICAL: all tests must pass before starting the next task**

## Implementation Steps

### Task 1: Give `gh` its repository context and bump the action pins

**Files:**
- Modify: `.github/workflows/pr-graph.yml`

- [ ] Confirm `v7` is still the current major for both actions before pinning:
      `gh api repos/actions/checkout/releases/latest --jq .tag_name` and
      `gh api repos/actions/upload-artifact/releases/latest --jq .tag_name`.
      If either has moved past `v7`, stop and report rather than pinning to a
      stale major.
- [ ] In the `Download release` step, add `GH_REPO: ${{ github.repository }}`
      alongside the existing `GH_TOKEN` in the step's `env:` block, matching the
      `Publish Release` step in `release.yml`. Leave the `cd "$RUNNER_TEMP"`
      and the `gh release download --pattern ...` line as they are — the step
      must not depend on the working directory being a checkout.
- [ ] Bump `actions/checkout@v4` -> `actions/checkout@v7`.
- [ ] Bump `actions/upload-artifact@v4` -> `actions/upload-artifact@v7`.
- [ ] Change nothing else in the file: the trigger, `permissions: contents: read`,
      the `Generate graph` step and `if-no-files-found: ignore` all stay.
- [ ] Run `actionlint .github/workflows/pr-graph.yml` — must pass with no output.
- [ ] Reproduce the review's failure scenario and prove it now passes: in a
      fresh empty directory that is not inside any git working tree (e.g. a
      `mktemp -d` outside the repo), with `GH_TOKEN` from `gh auth token` and
      `GH_REPO=HawkeyePierce89/GRIBOVIK` exported the same way the workflow
      exports it, run the step's snippet body
      (`gh release download --pattern '*x86_64-unknown-linux-gnu.tar.gz'`,
      then `tar -xzf ./*-x86_64-unknown-linux-gnu.tar.gz`, then `chmod +x gribovik`)
      and assert it exits 0 and leaves an executable `gribovik` in that
      directory. Clean up the temp directory afterwards.
- [ ] Optionally, as a negative control, run the same snippet once without
      `GH_REPO` set and confirm it still fails with the
      `not a git repository` error — this is what makes the fix demonstrably
      the cause of the pass.

### Task 2: Make the export parent-directory creation contractual

**Files:**
- Modify: `tests/export_html.rs`
- Modify: `src/export.rs` (doc comment only)

- [ ] Decide the deviation in favor of the current behavior: `export::write`
      keeps creating missing parent directories, so `--export out/review.html`
      works in CI without a preceding `mkdir`. No behavior change to the code.
- [ ] Update the doc comment on `export::write` in `src/export.rs` so the
      directory creation is documented as intended contract, not an incidental
      detail, in the surrounding style (one short sentence; no new module docs).
- [ ] Add a test `export_creates_missing_parent_directories` to
      `tests/export_html.rs`, following the shape of
      `export_html_writes_a_self_contained_page`: build a temp repo with a
      change, target a nested path under a fresh `TempDir` whose intermediate
      directories do not exist yet (e.g. `out/nested/review.html`), assert
      `gribovik::export::write` returns `Ok`, that the file exists, and that it
      contains `__GRIBOVIK_SNAPSHOT__`.
- [ ] Run `cargo test` — all tests must pass before Task 3.

### Task 3: Rewrap the CLAUDE.md paragraph

**Files:**
- Modify: `CLAUDE.md`

- [ ] In "The Export Mode", rewrap the paragraph so `self-contained` is not
      split across the line break (currently `self-` at the end of one line and
      `contained` at the start of the next, which renders as `self- contained`).
      Keep the surrounding lines within the file's existing wrap width and
      change no wording.
- [ ] Verify no other hyphen-at-end-of-line splits were introduced by the
      rewrap: `grep -n -- '-$' CLAUDE.md` should not report a line inside this
      section.
- [ ] No code changes, so no new tests; the check above is the verification.

### Task 4: Verify acceptance criteria

- [ ] `cargo test` — passes.
- [ ] `cargo clippy --all-targets -- -D warnings` — clean.
- [ ] `cargo fmt --check` — clean.
- [ ] `cd web && npm test && npm run typecheck` — passes (unchanged; run to
      confirm nothing regressed).
- [ ] `actionlint .github/workflows/pr-graph.yml` — passes.
- [ ] Re-run the Task 1 local download repro once more end to end and confirm
      it exits 0.
- [ ] Confirm `git diff --stat` touches only `.github/workflows/pr-graph.yml`,
      `src/export.rs`, `tests/export_html.rs` and `CLAUDE.md` — no
      `release.yml`, no frontend files, no serve-mode code.

### Task 5: Update documentation

- [ ] `README.md`: no user-facing change here (the `--export` behavior is
      unchanged and the workflow is CI-internal) — confirm by grepping for
      `pr-graph` and `--export` in `README.md` and leave it alone unless it
      documents the old workflow behavior.
- [ ] `CLAUDE.md`: already handled in Task 3; confirm no further internal
      pattern changed that would need documenting (this change touches none of
      the core-purity, error-contract or snapshot-contract boundaries).

## Post-Completion (manual, out of this plan's scope)

- Cutting a release whose binary actually supports `--export`, so the workflow
  becomes end-to-end functional. Until then the `Generate graph` step will
  still fail on the old binary — a known, accepted limitation.
