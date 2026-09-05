# CLAUDE.md

Guidance for working in this repository.

GRIBOVIK is a single Rust binary crate plus a Vite/React SPA it embeds. It
turns a git range into an interactive graph of changed symbols; see
`README.md` for what that means from a user's side.

## Layout

```
.github/workflows/       # release.yml on a v* tag, pr-graph.yml on a PR
build.rs                 # refuses to compile without web/dist/{index,export}.html
justfile                 # build-web / build / test
src/
  main.rs                # binary entry point; the anyhow boundary
  lib.rs                 # module tree
  cli.rs                 # clap Args + prepare() -> Session
  export.rs              # HTML injection for the --export mode
  git.rs                 # shell-out git wrapper (Repo, ChangedFile, blobs)
  pipeline.rs            # (repo, base, head) -> Analysis; git + core meet here
  server/mod.rs          # axum router, AppState, graceful shutdown
  server/assets.rs       # rust-embed of web/dist, or --assets <dir> from disk
  core/                  # pure analysis (see below)
    mod.rs               # build_snapshot(): the core's single entry point
    error.rs             # AnalysisError (thiserror)
    snapshot.rs          # the wire contract
    diff.rs              # LineRange, line_diff / slice_diff; git's indent heuristic
    nodes.rs             # symbols x hunks -> cards
    edges.rs             # call-name resolution
    lang/mod.rs          # LanguageAnalyzer trait + extension registry
    lang/{rust,swift,tsjs}.rs
tests/
  fixtures/{rust,swift,ts}/<case>/{before,after}.<ext>  # + expected.diff where git is the oracle
  export_html.rs         # end-to-end temp repo -> single file export
  git_cli.rs             # temp repos through the git wrapper
  grammars_link.rs       # all three tree-sitter grammars load
  integration_repo.rs    # temp repo -> full snapshot
web/
  export.html            # single-file shell for the --export mode
  vite.config.export.ts  # inline-everything build config
  src/
    main.tsx               # React entry point (StrictMode)
    App.tsx                # fetches the graph, lays out once, renders the canvas
    styles.css
    types/snapshot.ts      # the other half of the wire contract
    lib/{transform,layout,elk,snapshot,focus}.ts + *.test.ts
    components/{SymbolNode,FileNode,DiffView,ProgressPanel}.tsx + *.test.tsx
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
- `pipeline.rs`, `git.rs`, `server/`, `cli.rs` return `anyhow::Result`, adding
  `.context(...)` in the reviewer's language.
- `main.rs` is the boundary: every error collapses to one `gribovik: …` line on
  stderr and exit code 1. No backtraces, no panics reaching the user.

Degradation beats failure in the analysis path. A file the parser rejects
becomes a file-level card carrying the whole diff plus a warning in
`meta.warnings`. Only things the reviewer must act on — not a git repository,
unknown revision — abort the run.

The same rule covers a broken *internal* invariant, not just rejected input.
Where git's `xdiff` calls `XDL_BUG`, `diff.rs`'s port `debug_assert!`s the
group-sync invariant and, in release, stops compacting and returns the diff as
it stands. Sliding a change group is a semantics-preserving rewrite, so a
half-compacted pair of flag arrays is still a correct diff — where an `unwrap`
that only holds if the port is bug-free would panic in the reviewer's face.
Note this one degradation writes no `meta.warnings` entry: `core::diff` has no
handle on the snapshot's warning list, and the only way to reach the branch is
a bug in the port itself.

## The two-sided GraphSnapshot contract

The wire format is defined in exactly two places:

- `src/core/snapshot.rs`
- `web/src/types/snapshot.ts`

There is no code generation between them, so **any change to one must be
mirrored in the other in the same commit**. Field names are snake_case on the
wire on both sides; enums (`ChangeKind`, `Confidence`, `DiffTag`) serialize
lowercase. `src/core/snapshot.rs` has serde round-trip tests asserting the
exact JSON spelling — extend them when you extend the types.

Node ids are `"<file>::<qualified_name>"`; they key the edges, so two nodes
must never share one. A qualified name is *supposed* to be unique within a file, but real
languages repeat it — two `impl` blocks declaring `S::fmt`, `#[cfg]`-gated
twins, TypeScript overload signatures — so `nodes::symbol_cards` appends `#2`,
`#3` … to every occurrence past the first, and pairs the *n*-th occurrence on
the old side with the *n*-th on the new. The first occurrence keeps the plain
id, which is why the suffix costs nothing for the files that behave.

Every changed line of a file lands on **exactly one** card. Symbol cards claim
what falls in their span, and in Swift and TypeScript — where a type's span
contains its members' — a line goes to the *innermost* symbol holding it, so a
method-body edit is the method's alone and the enclosing class is not asked for
a second verdict on the same change. `diff::Span` is the carved span:
`nodes::carve` subtracts every nested sibling range from each symbol's, taking
the later position in the analyzer's source-order list as the inner one when a
one-line declaration gives a type and its member the same range.
`edges.rs` carves the same way, so the call sites a card draws arrows from are
the call sites its diff shows. Carving subtracts more than containment: two
declarations that merely overlap — the line holding one's closing brace and the
next one's header, `} fn b() {` — would otherwise both claim it, so the later
declaration takes the lines they share.
Whatever no symbol claims goes to the file card, decided **line by
line, not hunk by hunk** — a single hunk routinely straddles a symbol boundary,
and counting it as reviewed because the symbol claimed part of it is how
changed imports disappear from a review. The one deliberate exception is a
blank line between symbols.

Which lines a card claims depends on where the line diff *puts* a change, and
that placement is ambiguous whenever a run of inserted or deleted lines begins
and ends on a line identical to its neighbour — an inserted `#[test] fn` between
two others can equally be reported one function earlier or later. `similar`
returns the fully-slid-down placement (git's `--no-indent-heuristic`), so
`diff.rs` slides the block back with a faithful port of git's
`XDF_INDENT_HEURISTIC` split scoring from `xdiff/xdiffi.c`, run over
changed-flag arrays after `similar` and before `DiffLine`s are emitted. For
GRIBOVIK this is not cosmetic: a block placed one line too low hands its
leading `#[test]` to the *next* symbol's span, which turns an untouched
function into a "modified" card and strips the added function of its own
attribute. The weights are git's empirical constants and must not be tuned
locally — GitHub renders the same placement, and a reviewer comparing the two
should see one answer.

That agreement is bounded, and the bound is worth knowing before anyone goes
looking for a bug in the scoring. The port decides *where a slidable block
lands*; it does not decide *which equal lines match*, and that comes from
`similar`'s Myers, not xdiff's. On repetitive input the two pick different —
equally minimal — edit scripts, and the compaction pass then has a different
starting point: git's first pass aligns the old side against the new side's
*uncompacted* flags, so a hunk that changes both sides can still land a line
off. `line_diff("struct S;\nfn a() {\nfn a() {\n        deep();\n", "struct
S;\nfn a() {\n        deep();\n        deep();\n")` is the smallest case —
git pairs the deletion with the addition, we report them a line apart. Closing
that gap means porting `xdl_do_diff` too, which is a separate job. So a
GRIBOVIK-vs-GitHub difference is not by itself evidence the weights are wrong;
check whether the two sides agree on the edit script first.

`tests/fixtures/rust/slider_export_html/` pins the placement against
`expected.diff`, verbatim
`git diff --no-index --indent-heuristic --unified=100000 before.rs after.rs`
over `tests/export_html.rs` at `126e29d~1` and `126e29d` — one hunk, so context
lines are compared too, and the fixture's blank context lines are a single
space that a trailing-whitespace strip would destroy.
`tests/fixtures/{rust,swift,ts}/slider_attribute/` pin the card-level consequence.

There is deliberately no assertion that an `added` card contains only `add`
lines and a `deleted` card only `del` lines. The half of that is true by
construction — `slice_diff` on a range present in one revision only cannot
produce the other tag — but **context lines on an added or deleted card are
legitimate**: a renamed `impl`, class or `extension` hands every member a slice
of pure context, and a symbol sharing its closing brace with the previous
revision keeps that brace as context. A `debug_assert!` on the strict form
would fire on correct diffs, so the slider fixtures above, not an assertion,
are what catch a misalignment.

## The HTTP API

One route, and everything else falls through to the SPA assets (an unmatched
path returns `index.html` so client-side routing works):

- `GET /api/graph` — the snapshot, computed before the server binds and fixed
  for the lifetime of the process.

Every request must address the server by a loopback name — a middleware rejects
any other `Host` with 403. Binding loopback stops the network, but not a page
whose DNS resolves to 127.0.0.1; the graph is a diff of unpushed work.

## The Export Mode

`--export <FILE>` provides a second output for the same snapshot. It reads the
`export.html` shell built by Vite and injects the JSON snapshot in an inline
script tag. The injection anchor is `</head>`. This produces a single,
self-contained file that browsers will gladly open from a `file://` URI.

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
   - `calls_in_spans(&self, src, spans) -> Vec<Vec<String>>` — bare callee
     names from the lines each span claims, first-occurrence order,
     deduplicated, one list per span. A `diff::Span`, not a plain range,
     because a type's range contains its methods' and a card's arrows must
     come from the lines that card shows; `Span::whole(range)` covers a symbol
     with nothing nested. Return empty lists on an unparsable source rather
     than erroring: a missing edge degrades better than a failed analysis.
     Bare names are all the edge resolver has, so strip receivers and paths
     down to the final segment.

     Implement it through `lang::calls_by_span`, which owns the parse, the
     walk and the routing of each node to the span claiming its line, and asks
     the language only for a `fn(Node, &str, &mut Vec<String>)` that appends
     what one node calls. Answering one span at a time is a parse and a full
     tree walk per symbol — quadratic in a file's symbol count, and eighteen
     seconds of silence before the server binds on a generated file with two
     thousand functions. `calls_in_span` remains as a single-span convenience.
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
baked into the binary, so a `vite build` is picked up without recompiling Rust.
`build.rs` still needs `web/dist/index.html` and `web/dist/export.html` to exist
for the crate to compile at all.

For `npm run dev` instead, the server has to be on the port
`web/vite.config.ts` proxies `/api` to — `--port` defaults to 0, which is an
OS-assigned port the proxy cannot find:

```sh
cargo run -- --port 7777 --assets web/dist   # then npm run dev in web/
```

Component tests run in jsdom, the rest in node. `vite.config.ts` sets
`environment: "node"` for the whole suite — the `lib/` tests are pure and a DOM
would only cost startup — and each `*.test.tsx` opts itself in with a
`/** @vitest-environment jsdom */` docblock on the first line. That is what
`jsdom`, `@testing-library/react` and `@testing-library/dom` are dev
dependencies for. A component rendering a React Flow `Handle` — every node type
does — has to be wrapped in `<ReactFlowProvider>` in the test, since the handle
reads the instance store and throws without one.

The canvas adds a second id space on top of the snapshot's: a container is
`file:<path>` with every `:` in the path percent-escaped, a card keeps
`<file>::<qualified_name>`. A card id always carries a `::` and an escaped
container id never can, which is what keeps the two apart — the prefix alone
would not, since a path may itself contain a colon and `file:` + `a.ts::b.tsx`
is exactly the card id of `b.tsx` in a file named `file:a.ts`. `toFlow` emits each container
immediately before its own cards — React Flow resolves `parentId` against the
nodes it has already seen, so a child ahead of its parent in the array is an
error rather than a misplacement.

Two decisions in the canvas are load-bearing enough that changing them by
accident breaks something a test will not catch. **The layout sizes every card
by its collapsed height** (`CARD_HEIGHT` in `layout.ts`, and `HEADER_HEIGHT`
for the container header elk reserves as top padding — both are the
stylesheet's numbers copied by hand, and `lib/stylesheet.test.ts` is the only
thing comparing the two halves), and expanding a card must never re-run it: `SymbolNode` draws the
diff in an absolutely positioned `.symbol-expanded` overlay, so the node's own
box keeps the size elk gave it and the canvas cannot shift under the reviewer
mid-click. Anything that makes a card's box grow with its content brings back
the re-layout — and with it a graph that jumps every time you open a diff.
**Edge paths are React Flow's `smoothstep`**, not the `sections` elk computes.
elk's bendpoints are derived for its own port model and stop lining up with
React Flow's left/right handle centres the moment a node is dragged, so
consuming them would mean a custom edge component that is wrong exactly when it
matters; `smoothstep` gives the same orthogonal look, costs nothing per edge,
and stays correct when a node moves.

The overlay costs the canvas two things that are easy to reintroduce. React
Flow culls by a node's *measured* box, so `onlyRenderVisibleElements` has to
stand down while a card is expanded or panning the collapsed row off-screen
blanks the diff still filling the viewport. And React Flow hangs `onNodeClick`
off the wrapper the overlay renders inside, where `nodrag`/`nowheel` opt out of
the drag and the wheel but not the click — so `.symbol-expanded` stops click
propagation itself, or selecting a line of the diff closes the card.

## The two workflows

`release.yml` builds the binaries on a `v*` tag. `pr-graph.yml` builds gribovik
from the PR's own checkout and uploads its `--export` output as a per-PR
artifact, so the graph attached to a PR is produced by the code under review.
Both share the same build order — `setup-node` with npm caching, `npm ci && npm
run build` in `web/`, then the stable toolchain and `cargo build --release
--locked` — because `build.rs` refuses to compile without `web/dist/index.html`
and `web/dist/export.html`, so the web build must strictly precede every cargo
step. Those four steps are deliberately duplicated across the two files rather
than factored into a composite action — not verbatim, though: `release.yml`
interposes a `Verify native target` check before its cargo step for the
five-target matrix, and only `pr-graph.yml` spells out `cache: true` on the
toolchain action, which is that action's default either way.
`setup-rust-toolchain` also exports `RUSTFLAGS: -D warnings` by default, so a
PR that merely warns fails `build-graph` and gets no graph — which matches the
gate below treating warnings as errors. Building the PR takes minutes where the
old release download took seconds, so `pr-graph.yml` carries a `concurrency`
group cancelling a superseded run and a `timeout-minutes` on the job. Building
from the checkout also means `pr-graph.yml` executes PR-authored code — npm
lifecycle scripts, `build.rs`, proc macros, the built binary — where the
release download executed only a trusted artifact, so it stays on
`pull_request` (never `pull_request_target`) with a read-only token and no
secrets, and checks out with `persist-credentials: false`: the analysis shells
out only to local `rev-parse`, `merge-base`, `diff` and `show`, so nothing
after the checkout needs the token that `actions/checkout` would otherwise
leave in `.git/config`. Those measures bound what a hostile PR reaches on the
*runner*; they say nothing about the artifact, which is now written by the PR's
own exporter and frontend and is therefore untrusted content in the reviewer's
browser — README says so where it tells a reviewer to open the file. Two
conventions hold in both:

- Actions are pinned to a major tag (`@v7`, `@v8`), never to a SHA.
- Any step invoking `gh` sets `GH_REPO: ${{ github.repository }}` in its own
  `env:` block — unconditionally, never left to inference. `release.yml`'s
  publish step is the one that invokes `gh` today. A step that `cd`s outside the
  checkout has no working directory for `gh` to read a repository from and exits
  with `failed to run git: fatal: not a git repository`; the steps that do stay
  in the workspace name it anyway, so the rule needs no judgement call.

## Checks before calling anything done

`build.rs` fails the compile without `web/dist/index.html` and
`web/dist/export.html`, and `web/dist` is gitignored — so on a fresh clone
`just build-web` has to run once before any cargo command works at all.

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
cd web && npm test && npm run typecheck
actionlint .github/workflows/*.yml   # only when a workflow changed
```

`just test` runs the two test suites; the linters are not wired into a recipe.
