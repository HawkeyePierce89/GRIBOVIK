# Self-contained HTML export and a PR review-graph workflow

## Overview

Add a `--export <FILE>` mode to the CLI that writes one self-contained HTML
file (inlined JS + CSS + the snapshot) openable over `file://`, and a
`pull_request` workflow that runs it with a released binary and uploads the
file as an Actions artifact.

The single-file page is produced by a **second Vite build** into `web/dist`
(`export.html`), which the existing rust-embed of `web/dist` picks up for free.
The Rust shell reads that page, injects the snapshot as a script tag, and
writes the result. Serve mode is untouched.

## Context

- Files involved:
  - Create: `web/export.html`, `web/vite.config.export.ts`,
    `web/src/lib/elk.ts`, `web/src/lib/snapshot.ts`,
    `web/src/lib/snapshot.test.ts`
  - Modify: `web/package.json`, `web/vite.config.ts`, `web/src/lib/layout.ts`,
    `web/src/App.tsx`, `web/src/types/snapshot.ts`
  - Create: `src/export.rs`, `tests/export_html.rs`,
    `.github/workflows/pr-graph.yml`
  - Modify: `src/lib.rs`, `src/cli.rs`, `src/main.rs`, `src/server/assets.rs`,
    `build.rs`, `README.md`, `CLAUDE.md`

- Related patterns:
  - `cli::prepare` already returns a `Session` enum decided before any port is
    bound; export becomes a third variant, so the "no reviewable changes" path
    is shared with serve mode unchanged.
  - `Assets` (embedded vs `--assets <DIR>`) already abstracts "where the
    frontend files come from"; export reads `export.html` through the same
    enum.
  - anyhow outside / thiserror inside: export lives in the shell, so it speaks
    `anyhow`. `src/core/` is not touched at all.
  - `release.yml` style for the workflow: `permissions: contents: read`,
    actions pinned to major tags, `set -euo pipefail` in `shell: bash` steps.

- Dependencies:
  - New frontend devDependency `vite-plugin-singlefile`.
  - No new Rust dependencies.

## Key design decisions

1. **Build-time flag `__GRIBOVIK_EXPORT__`** — a Vite `define`, `false` in
   `vite.config.ts`, `true` in `vite.config.export.ts`. Two behaviours hang off
   it, and esbuild's minifier removes the dead branch, so the export bundle
   contains neither a `Worker` reference nor the string `/api/graph`:
   - elk runs inline (no web worker) — browsers refuse workers from `file://`;
   - the snapshot loader does not fall back to `fetch("/api/graph")`.
2. **`npm run build` runs both builds** (`vite build && vite build --config
   vite.config.export.ts`, the second with `emptyOutDir: false`). This keeps
   `justfile` and `release.yml` untouched and guarantees `web/dist/export.html`
   always exists next to `index.html`.
3. **Injection anchor is `</head>`** — always present in the built page and
   independent of attribute spelling. The injected tag is a *classic* inline
   script, so it runs during parsing, before the deferred inlined module
   bundle, regardless of where the bundle sits.
4. **Escaping**: `serde_json::to_string(&snapshot)` then replace every `<` with
   the JSON escape `\u003c`. `<` never appears as JSON structure, so a blind
   replace stays valid JSON and closes `</script>` breakout.
5. **The global is `window.__GRIBOVIK_SNAPSHOT__`**, holding a `GraphSnapshot`.
   No wire-contract field changes; only a TypeScript `Window` declaration is
   added, in the same commit, per `CLAUDE.md`.

## Development Approach

- **Testing approach**: Regular (code first, then tests), except the pure
  embedding function which is written test-first (its escaping contract is the
  point).
- Complete each task fully before moving to the next.
- **CRITICAL: every task MUST include new/updated tests**
- **CRITICAL: all tests must pass before starting the next task**
- Gates per task: `cargo test`, `cargo clippy --all-targets -- -D warnings`,
  `cargo fmt --check`, and `cd web && npm test && npm run typecheck`.
- Note: `web/dist` is gitignored, so `cd web && npm run build` must be re-run
  after Task 1 before any cargo command works.

## Implementation Steps

### Task 1: Single-file frontend build

**Files:**
- Create: `web/export.html`, `web/vite.config.export.ts`, `web/src/lib/elk.ts`,
  `web/src/lib/snapshot.ts`, `web/src/lib/snapshot.test.ts`
- Modify: `web/package.json`, `web/vite.config.ts`, `web/src/lib/layout.ts`,
  `web/src/App.tsx`, `web/src/types/snapshot.ts`

- [x] add `vite-plugin-singlefile` to `devDependencies`; change the `build`
      script to `tsc --noEmit && vite build && vite build --config
      vite.config.export.ts`
- [x] declare `__GRIBOVIK_EXPORT__: boolean` (ambient declaration in
      `web/src/types/snapshot.ts` or a small `web/src/vite-env.d.ts`) and set
      `define: { __GRIBOVIK_EXPORT__: "false" }` in `web/vite.config.ts`
- [x] create `web/export.html` — the same shell as `web/index.html`, pointing
      at `/src/main.tsx`
- [x] create `web/vite.config.export.ts`: react + `viteSingleFile()`,
      `define: { __GRIBOVIK_EXPORT__: "true" }`,
      `build: { emptyOutDir: false, rollupOptions: { input: "export.html" } }`
- [x] extract the elk instance out of `layout.ts` into `web/src/lib/elk.ts`,
      exporting `elk` and `workerFailure(): Promise<never>`; skip the worker
      factory entirely when `__GRIBOVIK_EXPORT__` is true (the existing
      `typeof Worker === "undefined"` guard stays for node tests); keep the
      grid fallback and `LAYOUT_TIMEOUT_MS` in `layout.ts` untouched
- [x] create `web/src/lib/snapshot.ts` with
      `loadSnapshot(): Promise<GraphSnapshot>` — return
      `window.__GRIBOVIK_SNAPSHOT__` when present; otherwise throw a clear
      error if `__GRIBOVIK_EXPORT__`, else `fetch("/api/graph")` exactly as
      today
- [x] add the `declare global { interface Window { __GRIBOVIK_SNAPSHOT__?:
      GraphSnapshot } }` declaration alongside the wire types
- [x] rewrite `App.tsx`'s loading effect to call `loadSnapshot()` instead of
      its inline `getJson("/api/graph")`; everything else in the effect stays
- [x] write `web/src/lib/snapshot.test.ts`: global present wins; global absent
      falls back to a mocked `fetch("/api/graph")`; a failing fetch rejects
- [x] update `web/src/lib/layout.test.ts` imports if the elk extraction moves
      any symbol it uses
- [x] run `npm run build` and assert by inspection that `web/dist/export.html`
      exists, `web/dist/index.html` is unchanged in shape, and `export.html`
      contains no `src="/assets/`, no `href="/assets/`, and no `/api/`
- [x] run `cd web && npm test && npm run typecheck` — must pass

### Task 2: The Rust export module

**Files:**
- Create: `src/export.rs`
- Modify: `src/lib.rs`, `src/server/assets.rs`, `build.rs`

- [x] make `Assets::read` public so a non-server caller can pull one file out
      of either the embedded tree or the `--assets` directory
- [x] create `src/export.rs` with the module doc explaining why the page is
      built by Vite and only *injected* here
- [x] TDD the pure function
      `embed_snapshot(page: &str, snapshot: &GraphSnapshot) -> Result<String>`:
      serialize, replace every `<` with `\u003c`, insert
      `<script id="gribovik-snapshot">window.__GRIBOVIK_SNAPSHOT__ = …;</script>`
      immediately before `</head>`; error when the anchor is absent
- [x] add the shell half: `write(assets: &Assets, snapshot, path: &Path)` —
      read `export.html` (error naming the assets directory when it is
      missing), embed, `fs::write` with `.context(...)` naming the path
- [x] register `pub mod export;` in `src/lib.rs`
- [x] extend `build.rs` to require `web/dist/export.html` alongside
      `index.html`, with the same one-line instruction in the panic message
- [x] unit tests in `src/export.rs`: a diff line containing `</script>`
      round-trips (extract the injected JSON, `serde_json::from_str` back to a
      `GraphSnapshot`, assert equality) and the output contains no literal
      `</script>` before the tag we emit; a page without `</head>` is an error;
      the embedded `export.html` asset contains the `</head>` anchor
- [x] run `cargo test`, `cargo clippy --all-targets -- -D warnings`,
      `cargo fmt --check` — must pass

### Task 3: The `--export` CLI mode

**Files:**
- Modify: `src/cli.rs`, `src/main.rs`

- [x] add `--export <FILE>` to `Args` as `Option<PathBuf>` with
      `conflicts_with_all = ["port", "no_open"]`
- [x] add `Session::Export { snapshot: Box<GraphSnapshot>, assets: Assets,
      path: PathBuf }`
- [x] in `prepare`, validate the `--assets` directory against `export.html`
      when exporting and `index.html` otherwise, still *before* the analysis;
      return `Session::Export` instead of `Session::Serve` when `--export` is
      set. The `NoChanges` branch is reached first and unchanged, so no file is
      written for an empty range
- [x] wire `Session::Export` in `main.rs`: call `export::write(...)`, print the
      path on stdout, return `Ok(())`
- [x] tests in `src/cli.rs`: `--export` parses; `--export` with `--port`
      errors; `--export` with `--no-open` errors; an empty range with
      `--export` still yields `Session::NoChanges`; `--export` with an
      `--assets` directory lacking `export.html` errors before the analysis;
      a changed range with `--export` yields `Session::Export` carrying the
      given path
- [x] run `cargo test`, `cargo clippy --all-targets -- -D warnings`,
      `cargo fmt --check` — must pass

### Task 4: End-to-end export test

**Files:**
- Create: `tests/export_html.rs`

- [ ] build a temp repo with a changed `.rs` file (following the pattern in
      `tests/integration_repo.rs`), run `cli::prepare` with `--export` into a
      `TempDir`, and execute the export
- [ ] assert: exactly one file exists in the output directory; the HTML
      contains `__GRIBOVIK_SNAPSHOT__`, an inline `<script`, an inline
      `<style`, and the changed symbol's name; it contains no `/assets/` and
      no `/api/`
- [ ] assert a range with no reviewable changes writes no file at all
- [ ] confirm the existing server tests (`/api/graph`, the SPA shell, the
      `/assets/` asset served with its own content type) are untouched and
      still green
- [ ] run `cargo test` — must pass

### Task 5: The PR workflow

**Files:**
- Create: `.github/workflows/pr-graph.yml`

- [ ] `on: pull_request: branches: [master]`, `permissions: contents: read`
- [ ] `actions/checkout@v7` with `fetch-depth: 0` so the merge base of the two
      PR SHAs is reachable
- [ ] a `shell: bash`, `set -euo pipefail` step that runs `gh release download`
      for the `*x86_64-unknown-linux-gnu.tar.gz` asset of the latest release
      into `$RUNNER_TEMP`, unpacks it and `chmod +x` the binary
      (`GH_TOKEN: ${{ github.token }}`)
- [ ] run the binary as `gribovik --export review.html "$BASE" "$HEAD"` with
      `BASE: ${{ github.event.pull_request.base.sha }}` and
      `HEAD: ${{ github.event.pull_request.head.sha }}` — the real PR head, not
      the synthetic merge commit
- [ ] `actions/upload-artifact@v7` with a PR-numbered artifact name,
      `path: review.html`, `if-no-files-found: ignore` so an empty PR passes
- [ ] run `actionlint` on the new workflow — must pass (install it locally if
      absent, e.g. `go install github.com/rhysd/actionlint/cmd/actionlint@latest`
      or `brew install actionlint`)

### Task 6: Documentation

**Files:**
- Modify: `README.md`, `CLAUDE.md`

- [ ] README: add `--export <FILE>` to the flags table, plus a few sentences on
      what the file is (one self-contained page, opens by double-click, no
      server) and the PR workflow's artifact in the reviewer's terms —
      download it from the PR's Checks tab and open it locally, since GitHub
      does not render artifact HTML in the browser
- [ ] README: note in the Build section that `npm run build` now produces both
      `dist/index.html` and `dist/export.html`
- [ ] README: keep every added line wrapped at ~78 columns
- [ ] CLAUDE.md: add `src/export.rs`, `web/export.html`,
      `web/vite.config.export.ts`, `web/src/lib/{elk,snapshot}.ts` and
      `tests/export_html.rs` to the layout tree; state that `build.rs` now also
      requires `web/dist/export.html`; add a short note that `--export` is a
      second output for the same snapshot and that the injection anchor is
      `</head>`. Leave the "one route" HTTP API section as-is — it is still
      true
- [ ] verify no other CLAUDE.md sentence has become false

### Task 7: Verify acceptance criteria

- [ ] `cd web && npm ci && npm run build` (regenerates both dist outputs)
- [ ] `cargo test`
- [ ] `cargo clippy --all-targets -- -D warnings`
- [ ] `cargo fmt --check`
- [ ] `cd web && npm test && npm run typecheck`
- [ ] `actionlint .github/workflows/pr-graph.yml`
- [ ] `cargo run -- --export /tmp/review.html <base> <head>` in this repo and
      confirm one file is produced and it contains no `/assets/` or `/api/`

## Post-Completion (manual)

These are for the human, not the agent:

- **Open the exported file in a real browser** (double-click, `file://`):
  the graph renders, cards show their diffs, edges are drawn, the minimap and
  controls work, and the browser console shows no errors and no failed network
  requests. A large graph may freeze the tab for a few seconds before first
  paint — that is expected without the worker.
- **Re-check serve mode by hand**: `cargo run -- --assets web/dist` still
  fetches `/api/graph` and renders identically.
- **Cut a release** containing `--export` (tag per `release.yml`). The PR
  workflow downloads the *latest release* binary, so it stays non-functional —
  the download step will fetch a binary that rejects `--export` — until such a
  release exists. This is deliberate and out of scope for this change.
