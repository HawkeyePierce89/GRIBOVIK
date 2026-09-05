# Build gribovik from the PR's own source in the PR review-graph workflow

## Overview

`.github/workflows/pr-graph.yml` downloads the latest published release binary
and uses it to export the PR's diff graph. Replace that download with an
in-workflow build from the PR's own checkout, so the graph attached to a PR is
produced by the code under review and the workflow stops depending on releases.
Then update the documentation that still claims a release download happens.

This is a CI-only change: YAML and Markdown, no Rust or TypeScript.

## Context

- Files involved:
  - Modify: `.github/workflows/pr-graph.yml` — replace `Download release` with
    the four build steps; run the export from `target/release/gribovik`; drop
    `GH_TOKEN`/`GH_REPO`.
  - Modify: `CLAUDE.md` — "The two workflows" section (lines ~236-252).
  - Read-only reference: `.github/workflows/release.yml` — the `build` job
    already pins the exact action majors and step order to mirror.
  - Read-only reference: `justfile`, `build.rs` — the web-before-cargo ordering
    constraint.
  - `README.md` — checked: the PR-artifact paragraph (lines 122-127) says only
    "the provided GitHub Actions PR workflow ... will attach this file as an
    artifact". It does not mention the release binary, so per the ticket it is
    left unchanged.
- Related patterns:
  - `release.yml`'s `build` job is the template: `actions/setup-node@v7`
    (`node-version: '22'`, `cache: npm`, `cache-dependency-path:
    web/package-lock.json`) → `Build web` (`working-directory: web`, `npm ci &&
    npm run build`) → `actions-rust-lang/setup-rust-toolchain@v1` (`toolchain:
    stable`) → `cargo build --release --locked`.
  - `build.rs` refuses to compile without `web/dist/index.html` and
    `web/dist/export.html`, which is why the web build must strictly precede
    every cargo step.
  - CLAUDE.md conventions: actions pinned to a major tag, never a SHA; a step
    invoking `gh` sets an explicit `GH_REPO`. After this change no step in
    `pr-graph.yml` invokes `gh`, so the second rule applies to `release.yml`
    only.
- Dependencies: none new. `actionlint` is already installed locally
  (`/opt/homebrew/bin/actionlint`).

## Development Approach

- **Testing approach**: Regular. This change has no unit-testable surface — the
  verification instruments are `actionlint` on the changed workflow, a local
  reproduction of the workflow's build-and-export sequence, and the existing
  project gates.
- Complete each task fully before moving to the next.
- The one true end-to-end verification (the workflow running on GitHub) is only
  possible after push and is recorded in Post-Completion, not as a task
  checkbox.

## Implementation Steps

### Task 1: Rewrite `.github/workflows/pr-graph.yml` to build from the checkout

**Files:**
- Modify: `.github/workflows/pr-graph.yml`

- [x] Confirm the action majors to pin by reading
      `.github/workflows/release.yml`: `actions/checkout@v7`,
      `actions/setup-node@v7`, `actions-rust-lang/setup-rust-toolchain@v1`,
      `actions/upload-artifact@v7`. These are the majors the repository already
      uses successfully; reuse them verbatim and pin no SHAs.
- [x] Keep the existing header unchanged: `name: PR Graph`, `on: pull_request:
      branches: [master]`, `permissions: contents: read`, job `build-graph` on
      `ubuntu-latest`.
- [x] Keep the `Checkout` step exactly as it is — `actions/checkout@v7` with
      `fetch-depth: 0`. Do not add a `ref:`; the default merge ref is what makes
      the built binary include the PR's changes on top of master, and
      `fetch-depth: 0` stays for the merge-base computation.
- [x] Delete the `Download release` step in full, including its `env:` block
      with `GH_TOKEN` and `GH_REPO`. No `gh` invocation and no release download
      may remain anywhere in the file.
- [x] Insert, in this order and strictly before any cargo step:
      1. `actions/setup-node@v7` with `node-version: '22'`, `cache: npm`,
         `cache-dependency-path: web/package-lock.json`.
      2. `Build web`: `working-directory: web`, `run: npm ci && npm run build`.
      3. `actions-rust-lang/setup-rust-toolchain@v1` with `toolchain: stable`
         and `cache: true` (the action's default; stated explicitly here because
         the requirement names cargo build caching).
      4. `Build Rust binary`: `run: cargo build --release --locked`.
- [x] Change the `Generate graph` step to invoke `./target/release/gribovik`
      instead of `"$RUNNER_TEMP/gribovik"`, keeping everything else: the
      `BASE`/`HEAD` env from `github.event.pull_request.base.sha` /
      `head.sha`, `shell: bash`, `set -euo pipefail`, and the `--export
      review.html "$BASE" "$HEAD"` argument form.
- [x] Leave the `Upload artifact` step byte-identical:
      `actions/upload-artifact@v7`, `name: pr-graph-${{
      github.event.pull_request.number }}`, `path: review.html`,
      `if-no-files-found: ignore`.
- [x] Do not touch `.github/workflows/release.yml`, and do not extract a
      composite action — the four duplicated build steps are accepted for this
      change.
- [x] Verify: `actionlint .github/workflows/pr-graph.yml` passes clean.
- [x] Verify: `grep -n 'gh \|gh release\|GH_TOKEN\|GH_REPO\|RUNNER_TEMP'
      .github/workflows/pr-graph.yml` returns nothing.
- [x] Verify the step order mechanically: in the file, the `Build web` step's
      line number is lower than every line mentioning `cargo`.

### Task 2: Reproduce the workflow's sequence locally

**Files:**
- No file changes; this task validates Task 1's step order against the real
  build.

- [x] Run the web build the workflow runs: `cd web && npm ci && npm run build`,
      and confirm both `web/dist/index.html` and `web/dist/export.html` exist
      afterwards (the two files `build.rs` demands).
- [x] Run `cargo build --release --locked` and confirm `target/release/gribovik`
      is produced — this is the same command and the same `--locked` flag the
      workflow will use, so a stale `Cargo.lock` fails here rather than in CI.
- [x] Exercise the export command form against a real range:
      `./target/release/gribovik --export /tmp/gribovik-pr-graph-check.html
      HEAD~1 HEAD`, and confirm it exits 0. This checks the invocation form the
      `Generate graph` step now uses from the repo root; note that on a range
      with no reviewable changes the tool reports that and writes no file, which
      is exactly the path `if-no-files-found: ignore` covers.
- [x] Remove `/tmp/gribovik-pr-graph-check.html` if it was created.

### Task 3: Update `CLAUDE.md` "The two workflows"

**Files:**
- Modify: `CLAUDE.md`

- [x] Rewrite the opening paragraph so it describes what the workflow now does:
      `release.yml` builds the binaries on a `v*` tag; `pr-graph.yml` builds
      gribovik from the PR's own checkout and uploads its `--export` output as a
      per-PR artifact, so the graph on a PR is produced by the code under
      review. Say that both workflows share the same build order — setup-node
      with npm caching, `npm ci && npm run build` in `web/`, then the stable
      toolchain and `cargo build --release --locked` — because `build.rs`
      refuses to compile without `web/dist/index.html` and
      `web/dist/export.html`, and note that the four steps are deliberately
      duplicated rather than factored into a composite action.
- [x] Keep the "actions pinned to a major tag, never a SHA" bullet as is — it
      still holds in both workflows.
- [x] Keep the `GH_REPO` bullet, but reword it so it no longer cites
      `pr-graph.yml`'s download as the motivating example (that step is gone).
      Attribute the rule to `release.yml`'s publish step and state the general
      reason: a step that `cd`s outside the checkout has no working directory
      for `gh` to read a repository from and exits with `failed to run git:
      fatal: not a git repository`, so the rule is applied unconditionally and
      needs no judgement call.
- [x] Delete the whole paragraph beginning "`pr-graph.yml` is only ever as
      current as the latest *published* release" — the constraint no longer
      exists, and with it the sentence about cutting a release before a workflow
      step depending on new behavior can go green.
- [x] Re-read the edited section top to bottom and confirm no remaining sentence
      is false after the change: no claim of a release download, no claim that
      the PR graph lags the latest release, and the two surviving conventions
      still describe the files as they now are.
- [x] Confirm the rest of `CLAUDE.md` is unaffected: `grep -n 'release\|pr-graph'
      CLAUDE.md` and check that every remaining hit is still accurate.
- [x] Confirm `README.md` needs no edit: `grep -n 'release' README.md` — the
      PR-artifact paragraph must contain no mention of the release binary.
      (Verified during exploration; re-check so the claim is not stale.)

### Task 4: Verify acceptance criteria

- [x] `actionlint .github/workflows/*.yml` — a workflow changed, so this gate
      applies.
- [x] `cargo test`
- [x] `cargo clippy --all-targets -- -D warnings`
- [x] `cargo fmt --check`
- [x] `cd web && npm test && npm run typecheck`
- [x] `git diff --stat` — confirm the change touches only
      `.github/workflows/pr-graph.yml` and `CLAUDE.md`; no Rust or TypeScript
      file may appear.

### Task 5: Update documentation

- [ ] `README.md`: no change expected (its PR-artifact paragraph does not
      mention the release binary). If Task 3's re-check found otherwise, correct
      just that sentence.
- [ ] `CLAUDE.md`: already updated in Task 3; confirm the diff of that file
      contains the rewritten description and the deleted release-currency
      paragraph, and nothing else.

## Post-Completion (manual, on GitHub)

These cannot be run locally and are not task checkboxes:

- Push the branch and open a PR against `master`.
- Watch the `build-graph` check on that PR. It must go green. Because this PR
  touches only YAML and Markdown, the expected outcome is gribovik reporting no
  reviewable changes, no `review.html` written, and the upload step passing
  quietly via `if-no-files-found: ignore` — the already-verified empty-PR path.
  This run is the one end-to-end verification that the new build-from-source
  workflow works.
- If the check fails, read the failing step's log before changing anything: a
  web-build failure means the ordering or Node setup is wrong, a cargo failure
  means `--locked` or the toolchain step, and an export failure means the binary
  path.
