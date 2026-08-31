# GRIBOVIK MVP — Interactive Diff Graph for Manual Code Review

## Overview

Build `gribovik`: a single-crate Rust CLI that turns a git diff (base..head) into an interactive call-graph of changed symbols, served by a local axum server to a React Flow SPA. Each changed function/method/type becomes a card with its own line diff, an approve/reject/pending status, and comments; review state persists in `.git/gribovik/`.

The work splits into three layers, built bottom-up:

1. **Pure analysis core** (`src/core/`) — git-free, HTTP-free, `thiserror`-based, fully unit-testable.
2. **Thin I/O shell** — git CLI wrapper, axum server, review-state file, clap CLI, `anyhow` at the boundary.
3. **Frontend** (`web/`) — Vite + React + TS + React Flow + elkjs, embedded via `rust-embed`.

## Context

- Files involved: greenfield repository — everything is created. Existing files: `README.md`, `LICENSE`, `.gitignore` (Rust-oriented; needs `web/node_modules`, `web/dist` entries).
- Related patterns: none yet; this plan establishes them. Toolchain present: Rust 1.92, Node 26.
- Planned layout:

```
Cargo.toml
build.rs                    # asserts web/dist exists (embed precondition)
justfile                    # `just build` = npm build + cargo build --release
src/
  main.rs                   # anyhow boundary, exit code 1 on error
  cli.rs                    # clap Args
  git.rs                    # shell-out git wrapper
  pipeline.rs               # (repo, base, head) -> GraphSnapshot: git.rs + core
  review.rs                 # review state load/save under .git/gribovik/
  server/mod.rs             # axum routes
  server/assets.rs          # rust-embed + --assets dev mode
  core/mod.rs
  core/error.rs             # thiserror AnalysisError
  core/snapshot.rs          # GraphSnapshot / Node / Edge / Meta (serde)
  core/diff.rs              # similar-based line diff + hunks
  core/lang/mod.rs          # LanguageAnalyzer trait + extension registry
  core/lang/rust.rs | swift.rs | tsjs.rs
  core/nodes.rs             # symbol/hunk intersection -> nodes
  core/edges.rs             # call-name resolution heuristics
tests/
  fixtures/{rust,swift,ts}/<case>/{before,after}.<ext>
  integration_repo.rs       # temp git repo -> full snapshot
web/
  src/types/snapshot.ts     # single source of the TS contract
  src/lib/transform.ts      # snapshot -> React Flow nodes/edges
  src/lib/layout.ts         # elkjs layered LR
  src/components/{SymbolNode,ProgressPanel,DiffView}.tsx
  src/hooks/useReviewState.ts
  src/lib/transform.test.ts
```

- Dependencies (Rust): `clap` (derive), `anyhow`, `thiserror`, `serde`/`serde_json`, `similar`, `tree-sitter` + `tree-sitter-rust` + `tree-sitter-swift` + `tree-sitter-typescript`, `axum`, `tokio` (rt-multi-thread, macros, signal), `rust-embed`, `mime_guess`, `webbrowser`; dev: `tempfile`. (No timestamp crate: `created_at` is stamped client-side, see Task 12.)
- Dependencies (web): `react`, `react-dom`, `@xyflow/react`, `elkjs`, `vite`, `typescript` (strict), `vitest`.
- Decided in Q&A: `build.rs` does **not** run npm. It verifies `web/dist/index.html` exists and fails with a message pointing at `just build-web`; the `justfile` chains the frontend build and `cargo build --release`.

## Development Approach

- **Testing approach**: TDD for the analysis core (Tasks 2–9) — fixtures and expected snapshots first; **Regular** (code first, then tests) for the server, CLI, and frontend UI.
- Complete each task fully before moving to the next.
- Rust 2021 edition, `thiserror` inside `src/core/`, `anyhow` from `main.rs`/`pipeline.rs` outward. `src/core/` must never import `std::process::Command`, axum, or read the filesystem.
- The GraphSnapshot contract lives in exactly two places: `src/core/snapshot.rs` and `web/src/types/snapshot.ts`; keep field names identical (snake_case on the wire).
- **CRITICAL: every task MUST include new/updated tests**
- **CRITICAL: all tests must pass before starting the next task** (`cargo test` for Rust tasks; `npm test` in `web/` for frontend tasks)

## Implementation Steps

### Task 1: Crate scaffolding and snapshot contract

**Files:**
- Create: `Cargo.toml`, `src/main.rs`, `src/core/mod.rs`, `src/core/error.rs`, `src/core/snapshot.rs`
- Modify: `.gitignore`

- [x] Create the binary crate `gribovik` (edition 2021) with the Rust dependencies listed in Context; pin mutually compatible `tree-sitter` and grammar crate versions and confirm `cargo build` links all three grammars
- [x] Define serde types in `src/core/snapshot.rs`: `GraphSnapshot { meta, nodes, edges }`, `Meta { repo, base, head, files_changed, warnings: Vec<String> }`, `Node { id, file, name, kind, change, diff: Vec<DiffLine> }`, `DiffLine { tag, old_line: Option<u32>, new_line: Option<u32>, text }`, `Edge { from, to, confidence }`; enums `ChangeKind {Added,Modified,Deleted}`, `Confidence {Certain,Ambiguous}`, `DiffTag {Add,Del,Context}` serialized lowercase
- [x] Define `AnalysisError` in `src/core/error.rs` with `thiserror` (parse failure, unsupported extension, invalid range)
- [x] Add `web/node_modules`, `web/dist`, `.ralphex/` to `.gitignore`
- [x] Write tests: serde round-trip of a hand-built `GraphSnapshot` asserting exact JSON field names and enum spellings
- [x] Run `cargo test` — must pass before Task 2

### Task 2: Git CLI wrapper

**Files:**
- Create: `src/git.rs`, `tests/git_cli.rs`

- [x] Implement `Repo::discover(cwd) -> Result<Repo>` via `git rev-parse --show-toplevel`, with a human-readable "not a git repository" error
- [x] Implement `resolve_base(explicit: Option<&str>)`: with no argument, probe `origin/master` then `origin/main` via `git rev-parse --verify`, error clearly if neither exists; then `git merge-base <base> <head>`
- [x] Implement `changed_files(base, head) -> Vec<ChangedFile{path, status}>` via `git diff --name-status <base> <head>` (map A/M/D; treat R as delete+add of the two paths)
- [x] Implement `blob(rev, path) -> Option<String>` via `git show <rev>:<path>`, returning `None` for missing paths and skipping non-UTF-8 blobs with a warning
- [x] Write tests in `tests/git_cli.rs`: build temp repos with `tempfile` + `git init`/commits and assert discovery, base fallback (origin/master vs origin/main via a local bare remote), unknown-revision error text, name-status parsing, and blob reads
- [x] Run `cargo test` — must pass before Task 3

### Task 3: Line diff engine

**Files:**
- Create: `src/core/diff.rs`

- [x] Implement `line_diff(old: &str, new: &str) -> Vec<DiffLine>` with `similar::TextDiff::from_lines`, emitting every line with its tag and 1-based `old_line`/`new_line` (context lines carry both)
- [x] Implement `hunks(diff: &[DiffLine]) -> Vec<Hunk{old_range, new_range}>` grouping consecutive non-context lines (empty side ranges represented as zero-length at the insertion point)
- [x] Implement `slice_diff(diff, old_range, new_range) -> Vec<DiffLine>` selecting the lines belonging to a symbol's old and/or new line span, preserving order
- [x] Write unit tests: pure insertion, pure deletion, modification in the middle, whole-file rewrite, empty old (added file) and empty new (deleted file); assert exact line numbers and hunk ranges
- [x] Run `cargo test` — must pass before Task 4

### Task 4: LanguageAnalyzer trait and Rust analyzer

**Files:**
- Create: `src/core/lang/mod.rs`, `src/core/lang/rust.rs`, `tests/fixtures/rust/*`

- [x] Define `Symbol { name, qualified_name, kind, start_line, end_line }` and the trait `LanguageAnalyzer { fn symbols(&self, src: &str) -> Result<Vec<Symbol>, AnalysisError>; fn calls_in_range(&self, src: &str, range) -> Vec<String>; }` plus `analyzer_for_extension(ext) -> Option<Box<dyn LanguageAnalyzer>>` covering `.rs .swift .ts .tsx .js .jsx` (registry in place; `.rs` registered here, the Swift and TS/JS arms land with their analyzers in Tasks 5-6)
- [x] Implement the Rust analyzer with tree-sitter queries: `function_item`, `impl_item`→`function_item` (qualified `Type::method`), `struct_item`, `enum_item`, `trait_item`; nested functions attributed to the enclosing symbol (not emitted separately, their lines fold into the parent)
- [x] Implement `calls_in_range` for Rust over `call_expression` nodes, branching on the `function` field: an `identifier` yields that name, a `scoped_identifier` (e.g. `Type::new`) yields its final segment, and a `field_expression` (method call `x.foo()`) yields the `field_identifier` — tree-sitter-rust has no `method_call_expression` node; also collect `struct_expression` type names
- [x] Add fixtures under `tests/fixtures/rust/` (before/after pairs covering added fn, modified method in impl, deleted struct, nested fn)
- [x] Write unit tests asserting the exact symbol list (names, kinds, qualified names, line ranges) and extracted call names for each fixture, including a method call, a `Type::assoc()` call, and a plain function call
- [x] Run `cargo test` — must pass before Task 5

### Task 5: Swift analyzer

**Files:**
- Create: `src/core/lang/swift.rs`, `tests/fixtures/swift/*`

- [x] Implement symbol extraction with tree-sitter-swift: `function_declaration`, methods inside `class_declaration`/struct/enum/extension (qualified `Type.method`), plus the type declarations themselves; attribute nested closures/functions to the parent
- [x] Implement `calls_in_range`: `call_expression` with `simple_identifier` and `navigation_expression` suffixes, returning the bare callee name
- [x] Add before/after fixtures covering a free function, a method in a class, an extension method, and a deleted struct
- [x] Write unit tests mirroring Task 4's assertions for Swift
- [x] Run `cargo test` — must pass before Task 6

### Task 6: TypeScript/JavaScript analyzer

**Files:**
- Create: `src/core/lang/tsjs.rs`, `tests/fixtures/ts/*`

- [x] Implement one analyzer parameterized by dialect (`typescript` for `.ts`, `tsx` for `.tsx`/`.jsx`, `tsx` grammar also used for `.js` to accept JSX-free JS)
- [x] Extract `function_declaration`, `method_definition` inside `class_declaration` (qualified `Class.method`), arrow/function expressions bound to a `variable_declarator` or exported const, `class_declaration`, `interface_declaration`, `type_alias_declaration`, `enum_declaration`
- [x] Implement `calls_in_range`: `call_expression` with identifier callee or `member_expression` property name, plus `new_expression` constructor names
- [x] Add before/after fixtures covering an arrow-const function, a class method, an interface, and a `.tsx` component
- [x] Write unit tests asserting symbols and calls per dialect, including that `.tsx` parses JSX
- [x] Run `cargo test` — must pass before Task 7

### Task 7: Node construction and change classification

**Files:**
- Create: `src/core/nodes.rs`

- [x] Define the pure core entry input `FileInput { path, old: Option<String>, new: Option<String> }` and implement `build_nodes(&[FileInput]) -> (Vec<Node>, Vec<String> /*warnings*/)`
- [x] For each file: pick the analyzer by extension, parse old and new sources, compute the line diff and hunks, then classify — symbol present only in `new` → `added`, only in `old` → `deleted`, in both and intersecting a hunk → `modified`; symbols in both with no intersecting hunk are dropped
- [x] Attach each node's own diff via `slice_diff` over its old/new line span; set `id = "<file>::<qualified_name>"`
- [x] Emit one synthetic file-level node (`kind: "file"`, name = file path) per file whose hunks fall outside every symbol range, carrying just those hunk lines; on parse failure or unsupported extension emit a file-level node with the whole file diff plus a warning string
- [x] Write unit tests over the Task 4–6 fixtures: expected node ids, kinds, change kinds, and per-node diff line numbers; plus a deliberately malformed source asserting graceful degradation + warning
- [x] Run `cargo test` — must pass before Task 8

### Task 8: Call-edge resolution

**Files:**
- Create: `src/core/edges.rs`

- [x] Build an index from bare symbol name → changed-node candidates (skipping file-level nodes)
- [x] For each non-file-level node, collect `calls_in_range` over its new-version span (old span for deleted nodes) and resolve each call name in priority order: candidate in the same file → candidate in the same directory → unique candidate anywhere in the graph
- [x] Mark an edge `ambiguous` when the winning tier still has multiple candidates (emit one edge per candidate); produce no edge when nothing matches; drop self-edges and deduplicate `(from, to)` keeping the strongest confidence
- [x] Write unit tests covering same-file resolution, cross-file unique-name resolution, same-directory preference over a distant file, ambiguous multi-candidate, and a call into unchanged code producing no edge
- [x] Run `cargo test` — must pass before Task 9

### Task 9: Snapshot assembly and end-to-end analysis

**Files:**
- Create: `src/pipeline.rs`, `tests/integration_repo.rs`
- Modify: `src/core/mod.rs`

- [x] Expose the pure core API `core::build_snapshot(meta_inputs, files: &[FileInput]) -> GraphSnapshot` wiring nodes + edges + meta (warnings collected, `files_changed` counted)
- [x] Implement `pipeline::analyze(repo, base, head) -> anyhow::Result<GraphSnapshot>`: resolve revisions via `git.rs`, filter changed files by the six supported extensions, load old/new blobs, and call `core::build_snapshot`
- [x] Return a distinguishable "no changes" outcome when the filtered file list is empty so the CLI can print a message and skip the server
- [x] Write `tests/integration_repo.rs`: create a temp repo, commit a Rust + Swift + TS baseline, commit modifications (add a fn, modify a method, delete a type, edit imports), run `pipeline::analyze` and assert the complete snapshot — node ids/kinds/changes, at least one certain edge and one ambiguous edge, and file-level nodes for the import-only change
- [x] Run `cargo test` — must pass before Task 10

### Task 10: Review state persistence

**Files:**
- Create: `src/review.rs`

- [x] Define `ReviewState = BTreeMap<String, NodeReview { status: Status, comments: Vec<Comment{text, created_at}> }>` with `Status {Approved, Rejected, Pending}` serialized lowercase; `created_at` is an opaque string supplied by the client
- [x] Implement `state_path(repo, base, head)` → `<repo>/.git/gribovik/<base>..<head>.json`, sanitizing `/` in revision names to `-`; create the directory on write
- [x] Implement `load(path) -> ReviewState` (missing/corrupt file → empty state, corrupt logged as a warning) and `save(path, &ReviewState)` writing atomically via temp file + rename
- [x] Write unit tests in a temp dir: save→load round trip, missing file, corrupt JSON, path construction for branch names containing slashes
- [x] Run `cargo test` — must pass before Task 11

### Task 11: Frontend scaffold, contract types, and transform

**Files:**
- Create: `web/package.json`, `web/tsconfig.json`, `web/vite.config.ts`, `web/index.html`, `web/src/main.tsx`, `web/src/types/snapshot.ts`, `web/src/lib/transform.ts`, `web/src/lib/layout.ts`, `web/src/lib/transform.test.ts`

- [x] Scaffold Vite + React + TypeScript (`strict: true`) with `@xyflow/react`, `elkjs`, `vitest`; add `npm run build` producing `web/dist` and `npm test`
- [x] Write `web/src/types/snapshot.ts` mirroring `src/core/snapshot.rs` exactly (`GraphSnapshot`, `SnapshotNode`, `SnapshotEdge`, `DiffLine`, `ReviewState`, `NodeReview`)
- [x] Implement `transform.ts`: `toFlow(snapshot) -> { nodes: Node[]; edges: Edge[] }` — node `type: "symbol"`, `data` carrying the snapshot node, edges with `animated: false` and `style.strokeDasharray` set for `ambiguous`, dropping edges whose endpoints are missing
- [x] Implement `layout.ts`: elkjs `layered` with `elk.direction: RIGHT`, returning positioned nodes
- [x] Write `transform.test.ts` (vitest): a fixture snapshot → expected React Flow nodes/edges, including dashed styling for ambiguous edges and dropping of dangling edges
- [x] Run `npm test` and `npm run build` in `web/` — both must pass before Task 12

### Task 12: Frontend UI — symbol cards, progress panel, state sync

**Files:**
- Create: `web/src/App.tsx`, `web/src/components/SymbolNode.tsx`, `web/src/components/DiffView.tsx`, `web/src/components/ProgressPanel.tsx`, `web/src/hooks/useReviewState.ts`, `web/src/styles.css`
- Modify: `web/src/main.tsx`

- [x] Implement `App.tsx`: fetch `/api/graph` and `/api/state` on load, run elk layout, render `<ReactFlow>` with pan/zoom, `<MiniMap>`, `<Controls>`, and a warnings banner from `meta.warnings`
- [x] Implement `SymbolNode`: file path, symbol name, change badge (added/modified/deleted/file), `DiffView` with `+`/`-`/context line coloring and old/new line gutters (no syntax highlighting), approve/reject/pending buttons, comment list and add-comment input; approved nodes render at reduced opacity
- [x] Implement `useReviewState`: holds the whole `ReviewState`, POSTs the full object to `/api/state` on every mutation, and stamps `created_at` on new comments with `new Date().toISOString()`
- [x] Implement `ProgressPanel`: Approved/Rejected/Pending counters derived from state over current graph node ids; clicking a counter highlights the matching nodes (selected class / border emphasis) and clears the previous highlight
- [x] Write vitest tests for the counter-derivation and status-mutation helpers (pure functions extracted from the components), including nodes with no state entry counting as pending
- [x] Run `npm test` and `npm run build` — both must pass before Task 13

### Task 13: HTTP server

**Files:**
- Create: `src/server/mod.rs`, `src/server/assets.rs`

- [x] Implement `assets.rs`: `rust-embed` over `web/dist` with SPA fallback to `index.html` and `mime_guess` content types; when `--assets <dir>` is given, serve from that directory on disk instead
- [x] Implement the axum router: `GET /` and `/*path` (assets), `GET /api/graph` (the precomputed snapshot), `GET /api/state`, `POST /api/state` (replace whole state, persist via `review::save`)
- [x] Share the snapshot and a `Mutex<ReviewState>` through axum state; bind to `--port` or port 0 and report the actually bound address; handle Ctrl+C via graceful shutdown
- [x] Write tests hitting the router in-process with `tower::ServiceExt::oneshot`: `/api/graph` returns the snapshot JSON, `POST /api/state` then `GET /api/state` round-trips and writes the file, unknown path falls back to `index.html`
- [x] Run `cargo test` — must pass before Task 14

### Task 14: CLI wiring, build integration, error handling

**Files:**
- Create: `src/cli.rs`, `build.rs`, `justfile`
- Modify: `src/main.rs`, `Cargo.toml`

- [ ] Define clap args: optional positional `base` and `head`, `--port`, `--no-open`, `--assets <dir>`
- [ ] Wire `main.rs`: discover repo → resolve revisions → `pipeline::analyze` → on empty diff print "no changes" and exit 0 without starting the server → otherwise load review state, start the server, print the URL, open the browser unless `--no-open`, and exit 0 on Ctrl+C
- [ ] Map all `anyhow` errors to a single human-readable stderr line and exit code 1 (not a git repo, missing origin/master|main, unknown revision, port in use)
- [ ] Add `build.rs` that fails with a clear message ("run `just build-web` first") unless `web/dist/index.html` exists, with `cargo:rerun-if-changed=web/dist`
- [ ] Add a `justfile` with `build-web` (`npm ci && npm run build` in `web/`), `build` (`build-web` + `cargo build --release`), and `test` (`cargo test` + `npm test`)
- [ ] Write tests: clap argument parsing for the zero/one/two-positional forms and flag defaults; a temp-repo test asserting the empty-diff path returns the "no changes" outcome without binding a port
- [ ] Run `cargo test` — must pass before Task 15

### Task 15: Verify acceptance criteria

- [ ] Run the full Rust suite: `cargo test`
- [ ] Run the frontend suite: `npm test` in `web/`
- [ ] Run linters: `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`, `tsc --noEmit`
- [ ] Run `just build` and confirm `cargo build --release` produces a binary with embedded assets
- [ ] Optional (only if `cargo llvm-cov --version` succeeds; skip and note it otherwise): measure `src/core/` coverage with `cargo llvm-cov --lib`, aiming for 80%+, and add tests for uncovered branches

### Task 16: Update documentation

- [ ] Update `README.md`: what GRIBOVIK does, install/build (`just build`), usage of all three invocation forms and flags, where review state is stored, supported languages
- [ ] Create `CLAUDE.md`: crate layout, the core-purity rule (`src/core/` has no git/HTTP/filesystem access), the `thiserror`/`anyhow` split, the two-sided GraphSnapshot contract, and how to add a new `LanguageAnalyzer`

## Post-Completion Verification (manual)

- Run `gribovik` in a real repository with Rust/Swift/TS changes and confirm the graph renders with elk layout, working pan/zoom/minimap, correct badges, and dashed ambiguous edges.
- Set statuses and add comments, restart the CLI on the same base..head, and confirm they persist.
- Run with `--no-open` and `--port`, and with `--assets web/dist` against a `vite dev` build.
- Confirm `cargo build --release` succeeds on macOS, Windows, and Linux.
