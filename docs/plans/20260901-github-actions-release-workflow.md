# GitHub Actions release workflow building gribovik binaries

## Overview

Add a single workflow, `.github/workflows/release.yml`, that builds release
binaries of `gribovik` on five native runners and publishes them as assets of a
GitHub Release when a `v*` tag is pushed. A `workflow_dispatch` run executes the
same matrix but stops at Actions artifacts — no release. A cheap guard job runs
first and refuses a tag whose version disagrees with `Cargo.toml`, so a typo
does not cost five platform builds.

The repository has no `.github/` directory today; this ticket creates it. Plus a
few README lines pointing at the Releases page. No Rust or TypeScript source
changes.

## Context

- Create: `.github/workflows/release.yml` (the only new file)
- Modify: `README.md` (short "Install from a release" note)
- Read-only reference:
  - `justfile` — the canonical build order: `build-web` (`cd web && npm ci &&
    npm run build`) then `cargo build --release`. The workflow mirrors it.
  - `build.rs` — refuses to compile without `web/dist/index.html`, which is why
    the npm build must precede every cargo command.
  - `Cargo.toml` — `version = "0.1.0"`; the guard job compares against it.
  - `web/package.json` / `web/package-lock.json` — `npm ci` is valid; the build
    script is `tsc --noEmit && vite build`. Vite 7 declares
    `engines.node: ^20.19.0 || >=22.12.0`, so the workflow pins Node 22.
  - `CLAUDE.md` — needs no change (this is infrastructure, not crate layout).

### External facts verified during planning

- GitHub-hosted runner labels, checked against
  `docs.github.com/en/actions/reference/runners/github-hosted-runners`:
  `ubuntu-latest`, `ubuntu-24.04-arm`, `macos-latest` (Apple silicon),
  `macos-15-intel` (Intel — the documented successor to the retired
  `macos-13`), `windows-latest`. All five are current.
- Latest stable action majors (GitHub releases API): `actions/checkout@v7`,
  `actions/setup-node@v7`, `actions/upload-artifact@v7`,
  `actions/download-artifact@v8`, `actions-rust-lang/setup-rust-toolchain@v1`
  (v1.17.0; it installs the toolchain *and* wraps `Swatinem/rust-cache`, and
  unlike `dtolnay/rust-toolchain` it carries a real major-version tag, which is
  what the ticket's pinning convention asks for).
- Local verification tooling present: `actionlint` 1.7.12 and `shellcheck`
  (actionlint automatically shellchecks `run:` bodies when shellcheck is on
  PATH). PyYAML is *not* installed, but actionlint parses the YAML itself, so
  YAML validity is still covered.

### Decisions already settled

- **Archive naming** (answered in this session): strip the leading `v`. Tag
  `v0.1.0` produces `gribovik-0.1.0-x86_64-unknown-linux-gnu.tar.gz`.
- **Dispatch version label**: the short commit SHA, as the ticket suggests —
  `gribovik-<short-sha>-<target>.tar.gz`. Keeps a manual build visibly distinct
  from a released version.
- **No `--target` flag on cargo**: the ticket specifies `cargo build --release
  --locked` and every runner is native. Instead each job asserts `rustc -vV`'s
  `host:` equals `matrix.target`, which proves nativeness for one line and keeps
  the binary at the plain `target/release/` path.
- **No `SHA256SUMS`**: optional in the ticket; omitted to keep the file small.

## Development Approach

- **Testing approach**: Regular — write the workflow, then verify it.
- This task produces CI configuration, not library code, so there is no unit
  test framework that can exercise it. Each task's verification items are the
  real, runnable checks: `actionlint` (YAML parse + expression/context
  validation + shellcheck of every `run:` body) and `bash -n` on the extracted
  shell snippets. These are the "tests must pass before the next task" gate.
- Complete each task fully — `actionlint` clean — before starting the next.
- **CRITICAL: every task must end with its verification commands passing.**
- **CRITICAL: all checks must pass before starting the next task.**

## Implementation Steps

### Task 1: Workflow skeleton, triggers, permissions, and the guard job

**Files:**
- Create: `.github/workflows/release.yml`

- [x] Create `.github/workflows/` and `release.yml` with `name: release` and
      triggers: `on.push.tags: ['v*']` and `on.workflow_dispatch:`.
- [x] Set a top-level `permissions: contents: read` (the least the checkout
      needs); the write scope is granted per-job in Task 3, not globally.
- [x] Add job `guard` (`runs-on: ubuntu-latest`) with
      `outputs.version: ${{ steps.resolve.outputs.version }}`. Steps:
      `actions/checkout@v7`, then a `shell: bash` step `id: resolve` that:
      - reads the crate version with
        `cargo_version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)`
        (first `version =` line is the `[package]` one, before any
        `[dependencies]`);
      - on a tag run (`[ "$GITHUB_REF_TYPE" = tag ]`), strips the leading `v`
        (`version="${GITHUB_REF_NAME#v}"`) and fails with
        `echo "::error::tag $GITHUB_REF_NAME implies version $version but Cargo.toml declares $cargo_version"; exit 1`
        on mismatch;
      - otherwise (dispatch) sets `version="$(git rev-parse --short HEAD)"` —
        no tag is required, so a manual run never trips the guard;
      - writes `echo "version=$version" >> "$GITHUB_OUTPUT"`.
- [x] Start the script with `set -euo pipefail`; read `GITHUB_REF_TYPE` /
      `GITHUB_REF_NAME` as environment variables rather than `${{ }}`
      interpolation, so no expression is spliced into a shell string.
- [x] Verify: `actionlint .github/workflows/release.yml` exits 0 (this parses
      the YAML, type-checks the expressions and shellchecks the `run:` body).
- [x] Verify: extract the `run:` body to a temp file and confirm `bash -n`
      accepts it; then exercise the version-extraction sed against the real
      `Cargo.toml` in a local shell and confirm it prints exactly `0.1.0`.

### Task 2: The five-target build matrix

**Files:**
- Modify: `.github/workflows/release.yml`

- [x] Add job `build` with `needs: guard`, `runs-on: ${{ matrix.os }}`,
      `strategy.fail-fast: false` (a failing target should not hide the other
      four; the release job is still blocked by `needs`, so there is no partial
      release either way), and `strategy.matrix.include` of exactly five
      entries: `x86_64-unknown-linux-gnu`/`ubuntu-latest`,
      `aarch64-unknown-linux-gnu`/`ubuntu-24.04-arm`,
      `aarch64-apple-darwin`/`macos-latest`,
      `x86_64-apple-darwin`/`macos-15-intel`,
      `x86_64-pc-windows-msvc`/`windows-latest`.
- [x] Steps, in the order `just build` uses:
      1. `actions/checkout@v7`;
      2. `actions/setup-node@v7` with `node-version: '22'`, `cache: npm`,
         `cache-dependency-path: web/package-lock.json`;
      3. `npm ci && npm run build` with `working-directory: web` — **before**
         any cargo step, because `build.rs` needs `web/dist/index.html`;
      4. `actions-rust-lang/setup-rust-toolchain@v1` with `toolchain: stable`
         (this also turns on Rust build caching);
      5. a `shell: bash` step asserting the runner is native:
         `host=$(rustc -vV | sed -n 's/^host: //p')` compared against
         `$TARGET` (passed via `env: TARGET: ${{ matrix.target }}`), erroring
         with `::error::` on mismatch;
      6. `cargo build --release --locked`.
- [x] Add two mutually exclusive packaging steps that write into `dist/`, named
      `gribovik-${VERSION}-${TARGET}` where `VERSION` is
      `${{ needs.guard.outputs.version }}` and `TARGET` is `${{ matrix.target }}`,
      both passed through `env:` rather than interpolated into the script body:
      - `if: runner.os != 'Windows'`, `shell: bash`:
        `mkdir -p dist && tar -czf "dist/${NAME}.tar.gz" -C target/release gribovik`
        (`-C` keeps the binary at the archive root, with no `target/release/`
        prefix);
      - `if: runner.os == 'Windows'`, `shell: pwsh`:
        `New-Item -ItemType Directory -Force dist` then
        `Compress-Archive -Path target/release/gribovik.exe -DestinationPath "dist/$env:NAME.zip"`.
        PowerShell is used deliberately here — `Compress-Archive` is built in,
        which avoids assuming `7z` or a bsdtar-flavoured `tar` on the Windows
        image. Every *other* multi-line snippet in the file carries an explicit
        `shell: bash`, because the Windows runner's default is `pwsh`.
- [x] Add `actions/upload-artifact@v7` with
      `name: gribovik-${{ matrix.target }}` (artifact names must be unique per
      job under the v4+ backend), `path: dist/*`, and
      `if-no-files-found: error` so a silently empty archive fails the job.
- [x] Verify: `actionlint .github/workflows/release.yml` exits 0 — this catches
      an unknown runner label, a bad `needs.guard.outputs.version` reference, and
      any shellcheck complaint in the new `run:` bodies.
- [x] Verify: `bash -n` each new bash snippet, and sanity-check the tar
      invocation locally against a throwaway file to confirm the archive holds a
      bare `gribovik` entry with no directory prefix.

### Task 3: The publish job

**Files:**
- Modify: `.github/workflows/release.yml`

- [ ] Add job `release` with `needs: [guard, build]` and
      `if: github.ref_type == 'tag'` — with no `always()`, `needs` already means
      the job is skipped unless all five builds succeeded, so a failed target
      can never produce a partial release.
- [ ] Give it `runs-on: ubuntu-latest` and job-scoped
      `permissions: contents: write`; the top-level default stays `read`, so
      nothing else in the workflow can write.
- [ ] Add `actions/download-artifact@v8` with `path: dist`,
      `pattern: gribovik-*` and `merge-multiple: true`, collapsing the five
      per-target artifacts into one flat `dist/`.
- [ ] Add a `shell: bash` step with `set -euo pipefail` that lists `dist`,
      asserts it holds exactly 5 files (`::error::` and exit 1 otherwise), then
      runs `gh release create "$TAG" --title "$TAG" --generate-notes dist/*`
      with `env: GH_TOKEN: ${{ github.token }}` and `TAG: ${{ github.ref_name }}`.
      `gh` is preinstalled on `ubuntu-latest`, so no third-party action and no
      extra token are needed.
- [x] Verify: `actionlint .github/workflows/release.yml` exits 0.
- [ ] Verify: `bash -n` the publish snippet; confirm by reading the file that
      the trigger→job graph is exactly `tag push → guard → 5 builds → release`
      and `dispatch → guard (label only) → 5 builds → no release`.

### Task 4: README note

**Files:**
- Modify: `README.md`

- [ ] Add a short `## Install from a release` section immediately before the
      existing `## Build` heading (line ~69) — a couple of sentences saying
      tagged releases carry prebuilt binaries for the five targets, linking
      `https://github.com/HawkeyePierce89/GRIBOVIK/releases`, naming the
      `gribovik-<version>-<target>.tar.gz` / `.zip` convention, and noting that
      `git` still has to be on the `PATH` at runtime. Building from source stays
      the section right below.
- [ ] Do not restructure anything else in the README, and leave `CLAUDE.md`
      untouched — the workflow is infrastructure, and the README note is the one
      place its existence is documented.
- [ ] Verify: `grep -n "Install from a release" README.md` finds the new
      heading and the surrounding sections are intact.

### Task 5: Verify acceptance criteria

- [ ] Run `actionlint .github/workflows/release.yml` a final time — must exit 0
      with no output.
- [ ] Confirm each archive name is spelled exactly
      `gribovik-<version>-<target>.tar.gz` / `.zip` by grepping the workflow for
      the `NAME` construction, and confirm the leading `v` is stripped
      (`${GITHUB_REF_NAME#v}`), matching the settled naming decision.
- [ ] Confirm the workflow requests `contents: write` only on the `release` job
      and `contents: read` at the top level.
- [ ] Run the repo's existing checks — the branch must be green even though the
      workflow does not touch them:
      `just build-web` (needed once for `build.rs`), then `cargo test`,
      `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`, and
      `cd web && npm test && npm run typecheck`.

## Post-Completion (manual, not automatable here)

- The workflow can only be fully exercised on GitHub. After merge, trigger it
  once via **Run workflow** (`workflow_dispatch`) to confirm all five runners
  build and upload artifacts, before pushing a real `v*` tag.
- If the Intel macOS build is ever dropped by GitHub, `macos-15-intel` is the
  line to revisit.
