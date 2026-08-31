# CLAUDE.md

Guidance for working in this repository.

GRIBOVIK is a single Rust binary crate plus a Vite/React SPA it embeds. It
turns a git range into an interactive graph of changed symbols; see
`README.md` for what that means from a user's side.

## Layout

```
build.rs                 # refuses to compile without web/dist/index.html
justfile                 # build-web / build / test
src/
  main.rs                # binary entry point; the anyhow boundary
  lib.rs                 # module tree
  cli.rs                 # clap Args + prepare() -> Session
  git.rs                 # shell-out git wrapper (Repo, ChangedFile, blobs)
  pipeline.rs            # (repo, base, head) -> Analysis; git + core meet here
  review.rs              # review state under .git/gribovik/
  server/mod.rs          # axum router, AppState, graceful shutdown
  server/assets.rs       # rust-embed of web/dist, or --assets <dir> from disk
  core/                  # pure analysis (see below)
    mod.rs               # build_snapshot(): the core's single entry point
    error.rs             # AnalysisError (thiserror)
    snapshot.rs          # the wire contract
    diff.rs              # line_diff / hunks / slice_diff over `similar`
    nodes.rs             # symbols x hunks -> cards
    edges.rs             # call-name resolution
    lang/mod.rs          # LanguageAnalyzer trait + extension registry
    lang/{rust,swift,tsjs}.rs
tests/
  fixtures/{rust,swift,ts}/<case>/{before,after}.<ext>
  git_cli.rs             # temp repos through the git wrapper
  grammars_link.rs       # all three tree-sitter grammars load
  integration_repo.rs    # temp repo -> full snapshot
web/src/
  main.tsx               # React entry point (StrictMode)
  App.tsx                # fetches both APIs, lays out once, provides context
  styles.css
  types/snapshot.ts      # the other half of the wire contract
  lib/{transform,layout,review}.ts + *.test.ts
  components/{SymbolNode,DiffView,ProgressPanel}.tsx
  hooks/useReviewState.ts
```

Unit tests live in `#[cfg(test)] mod tests` next to the code they cover;
`tests/` holds only the cross-cutting ones.

## The core-purity rule

Everything under `src/core/` is **pure**: source text in, a `GraphSnapshot`
out. It must never

- run a process (`std::process::Command`, git, anything),
- touch the filesystem, or
- know that HTTP or axum exist.

The shell loads blobs and hands the core `FileInput { path, old, new }`. This
is what makes the analysis testable from fixture strings, and it is the first
thing to check when adding code: if a function in `core/` needs to read a file,
the read belongs in `pipeline.rs` instead.

## thiserror inside, anyhow outside

- `src/core/` returns `Result<_, AnalysisError>` — typed variants (parse
  failure, unsupported extension, invalid range), no `anyhow` in sight.
- `pipeline.rs`, `git.rs`, `review.rs`, `server/`, `cli.rs` return
  `anyhow::Result`, adding `.context(...)` in the reviewer's language.
- `main.rs` is the boundary: every error collapses to one `gribovik: …` line on
  stderr and exit code 1. No backtraces, no panics reaching the user.

Degradation beats failure in the analysis path. A file the parser rejects
becomes a file-level card carrying the whole diff plus a warning in
`meta.warnings`; a corrupt review-state file loads as an empty state. Only
things the reviewer must act on — not a git repository, unknown revision —
abort the run.

## The two-sided GraphSnapshot contract

The wire format is defined in exactly two places:

- `src/core/snapshot.rs`
- `web/src/types/snapshot.ts`

There is no code generation between them, so **any change to one must be
mirrored in the other in the same commit**. Field names are snake_case on the
wire on both sides; enums (`ChangeKind`, `Confidence`, `DiffTag`, `Status`)
serialize lowercase. `src/core/snapshot.rs` has serde round-trip tests
asserting the exact JSON spelling — extend them when you extend the types.

Node ids are `"<file>::<qualified_name>"`; they key both the edges and the
review state on disk, so changing how they are built invalidates every saved
review. A qualified name is *supposed* to be unique within a file, but real
languages repeat it — two `impl` blocks declaring `S::fmt`, `#[cfg]`-gated
twins, TypeScript overload signatures — so `nodes::symbol_cards` appends `#2`,
`#3` … to every occurrence past the first, and pairs the *n*-th occurrence on
the old side with the *n*-th on the new. The first occurrence keeps the plain
id, which is why the suffix costs nothing for the files that behave.

Every changed line of a file lands on some card. Symbol cards claim what falls
in their span; whatever is left over goes to the file card, decided **line by
line, not hunk by hunk** — a single hunk routinely straddles a symbol boundary,
and counting it as reviewed because the symbol claimed part of it is how
changed imports disappear from a review. The one deliberate exception is a
blank line between symbols.

The same rule applies to `ReviewState` in `src/review.rs` and its TS twin.

## The HTTP API

Three routes, and everything else falls through to the SPA assets (an unmatched
path returns `index.html` so client-side routing works):

- `GET /api/graph` — the snapshot, computed before the server binds and fixed
  for the lifetime of the process.
- `GET /api/state` — the verdicts recorded so far.
- `POST /api/state` — **replaces** the whole `ReviewState` and answers 204.

Every request must address the server by a loopback name — a middleware rejects
any other `Host` with 403. Binding loopback stops the network, but not a page
whose DNS resolves to 127.0.0.1; the graph is a diff of unpushed work.

The replace-don't-patch shape is deliberate: the browser owns the state for the
session and the server only persists it, so neither side has merge logic. The
write happens under the state mutex so overlapping posts land on disk in the
order they took it, and the client chains its posts for the same reason. Adding
a PATCH-style endpoint would put a merge on both sides of that.

Review state lives under `Repo::git_dir()`, never under `<root>/.git` built by
hand: in a linked worktree or a submodule that path is a file, not a directory.

## Adding a LanguageAnalyzer

1. Add the grammar crate to `Cargo.toml` and a case to `tests/grammars_link.rs`
   so a version mismatch fails loudly rather than at runtime.
2. Create `src/core/lang/<lang>.rs` implementing `LanguageAnalyzer`:
   - `symbols(&self, src) -> Result<Vec<Symbol>, AnalysisError>` — every
     top-level and type-level symbol in source order. Spans are 1-based and
     inclusive and should cover leading doc comments and attributes, so that
     editing them counts as editing the symbol. Nested functions and closures
     fold into their enclosing symbol rather than becoming their own; that
     falls out of walking the tree rather than running a flat query, which is
     why all three existing analyzers walk. `qualified_name` must be unique
     within a file (`Type::method` in Rust, `Type.method` elsewhere) and
     `kind` must never be `"file"` — that string is reserved for the synthetic
     file-level node (`nodes::FILE_KIND`).
   - `calls_in_range(&self, src, range) -> Vec<String>` — bare callee names
     from lines inside `range`, first-occurrence order, deduplicated. Return an
     empty list on an unparsable source rather than erroring: a missing edge
     degrades better than a failed analysis. Bare names are all the edge
     resolver has, so strip receivers and paths down to the final segment.
3. Register the extensions in `analyzer_for_extension` in
   `src/core/lang/mod.rs`. That match is the only registry —
   `supports_extension` and the pipeline's file filter both read from it, so
   nothing else needs touching to make the language show up in real runs.
4. Add `tests/fixtures/<lang>/<case>/{before,after}.<ext>` pairs covering at
   least an added function, a modified method inside a type, a deleted type,
   and whatever nesting the language makes interesting. Assert the exact symbol
   list (names, qualified names, kinds, line ranges) and the extracted call
   names.
5. Extend `tests/integration_repo.rs` if the language should appear in the
   end-to-end snapshot.

## Working on the frontend

`cargo run -- --assets web/dist` serves the SPA from disk instead of the copy
baked into the binary, so a `vite build` (or `npm run dev`'s output) is picked
up without recompiling Rust. `build.rs` still needs `web/dist/index.html` to
exist for the crate to compile at all.

## Checks before calling anything done

`build.rs` fails the compile without `web/dist/index.html`, and `web/dist` is
gitignored — so on a fresh clone `just build-web` has to run once before any
cargo command works at all.

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
cd web && npm test && npm run typecheck
```

`just test` runs the two test suites; the linters are not wired into a recipe.
