# GRIBOVIK

GRIBOVIK explores the project’s sprawling mycelial network.

It turns a git range into an **interactive diff graph**: every changed
function, method, or type becomes a card carrying its own line diff, and the
calls between those cards become edges. You explore the graph in a browser
instead of scrolling a flat diff.

```
gribovik                     # review this branch against origin/master
```

## What it does

1. Resolves a revision range (`base..head`) with plain `git`.
2. Parses the before/after text of every changed file with tree-sitter and
   works out which symbols the diff actually touched.
3. Classifies each touched symbol as **added**, **modified**, or **deleted**,
   and slices out just the diff lines belonging to it.
4. Resolves calls between changed symbols into edges, nearest first: the
   caller's own file, then its directory, then the whole graph — `certain` when
   the callee is unambiguous, `ambiguous` (drawn dashed) when several changed
   symbols in the tier that answered share the name. A name shared across
   distant directories draws nothing, since proximity is the only evidence
   there is.
5. Serves the result on `localhost` to a React Flow SPA laid out left-to-right
   with elkjs.

Changed lines that fall outside every symbol — import blocks, top-level
constants, `impl` scaffolding — are collected into a synthetic **file-level**
card, so a hunk that straddles a symbol boundary is reviewed on both cards
rather than only on the symbol's. Blank lines added or removed between symbols
are the one exception: they carry nothing to review and would otherwise put a
file card on almost every file that gained a function.

Where a run of added or deleted lines could equally be reported one symbol
earlier or later — an added function whose first line repeats the line above it
— GRIBOVIK places it the way `git diff` does, using git's own indent heuristic.
An added function keeps its own attribute or doc comment instead of handing it
to the function below, and the function below stays off the graph. On
repetitive text the underlying line matching can still differ from git's, so
the two are not guaranteed identical line for line.

GRIBOVIK compares two **commits**. Working-tree changes — unstaged, or staged
but not committed — are not part of any revision range and will not appear;
commit or stash before reviewing.

Renames are not tracked. Git reports them as a delete of the old path plus an
add of the new one, and GRIBOVIK reviews them that way: every symbol in the
file shows up once as deleted and once as added.

## Supported languages

| Language | Extensions |
| --- | --- |
| Rust | `.rs` |
| Swift | `.swift` |
| TypeScript / JavaScript | `.ts`, `.tsx`, `.js`, `.jsx` |

Changed files with any other extension are ignored. A file whose syntax the
parser rejects still gets a file-level card with the whole diff, plus a warning
in the banner at the top of the page. A file that is not UTF-8 text, or that
git cannot produce for one side of the range, is left out of the graph
altogether and named in the same banner.

## In the browser

The left panel counts the cards — changed symbols plus the file-level
catch-alls — and lists every file the graph holds a card for, with its card
count and `+N −M`. A file with an unsupported extension is not analysed and so
is not in the panel either, and neither is one the analysis produced no card
for at all. The counts are totals over that file's cards rather than `git diff
--stat`'s, which run a few lines higher — a blank line gained between two
symbols is the one change GRIBOVIK deliberately does not card, and a file whose
*only* change is that blank line drops out of the panel with it.

Clicking a file zooms the canvas to that file's container, which is how you get
back to a particular file without hunting for it: elk packs a real branch into
tens of thousands of pixels each way, so the initial fit shows the shape of the
graph rather than its text.

The graph itself is grouped by file. Each file is a **container** box headed
with its path, how many cards it holds, and its `+N −M`; the cards inside it
are the symbols that changed. A card is collapsed by default — name, kind, a
change badge, and its own `+N −M` on one line — so that a screenful of graph
stays readable.

**Click a card to expand it.** Its diff opens in a panel over the cards below,
without moving anything: the card's own box keeps its collapsed size, so the
canvas never jumps. Selecting a card also dims everything outside its
neighbourhood — the card, whatever calls it, and whatever it calls — leaving
just the conversation that card takes part in. Press Escape, click the same
card again, or click a container or the empty canvas to collapse and undim.
Hovering a card previews the same dimming while nothing is selected. Cards are
reachable from the keyboard too: Tab moves between them and Enter or Space
opens the one you land on. While nothing is open only the cards in the
viewport are drawn, so Tab reaches those alone and the way to a distant one is
the file panel first; opening a card draws the whole graph — the diff panel
falls outside the card's own box, so the cards around it cannot be dropped —
and Tab then walks every card in the range.

Edges run caller → callee with an arrowhead at the callee's end; a dashed one
is a call the resolver was not sure about. The graph pans and zooms, with a
minimap and controls in the corners. Warnings from the analysis sit in a
banner at the top.

## Install from a release

Tagged releases carry prebuilt binaries for five targets on the [Releases page](https://github.com/HawkeyePierce89/GRIBOVIK/releases). Download the archive for your platform (named `gribovik-<version>-<target>.tar.gz` or `.zip`), extract it, and place the `gribovik` binary anywhere on your `PATH`. Note that `git` still must be on your `PATH` at runtime.

## Build

The binary embeds the frontend, so the SPA must be built first. `build.rs`
refuses to compile without `web/dist/index.html` and `web/dist/export.html`.

```sh
just build        # npm ci && npm run build in web/, then cargo build --release
```

Or without `just`:

```sh
cd web && npm ci && npm run build && cd ..
cargo build --release
```

Note that `npm run build` produces both `dist/index.html` (the SPA shell) and
`dist/export.html` (the single-file shell used by `--export`).

The binary lands at `target/release/gribovik`; copy it anywhere on your `PATH`.

Requirements: a Rust 2021 toolchain, Node (for the frontend build only), and
`git` on the `PATH` at runtime.

## Usage

Run it from anywhere inside a git working tree — the repository root is
discovered automatically.

```sh
gribovik                     # base = merge base with origin/master (or origin/main), head = HEAD
gribovik main                # base = merge base with main, head = HEAD
gribovik main feature-x      # explicit base and head
```

With no positional arguments GRIBOVIK probes `origin/master`, then
`origin/main`, and errors if neither exists. In every form the base is the
**merge base** with the named revision, so a stale branch shows only your own
work.

### Flags

| Flag | Default | Meaning |
| --- | --- | --- |
| `--export <FILE>` | none | Write the graph to a self-contained HTML file instead of starting a server. |
| `--port <PORT>` | `0` | Port to serve on. `0` asks the OS for a free one. |
| `--no-open` | off | Print the URL instead of opening a browser. |
| `--assets <DIR>` | embedded | Serve the frontend from a directory on disk instead of the build baked into the binary. Useful with `web/dist` while working on the UI. |

When using `--export`, GRIBOVIK writes a single, self-contained HTML file with
all data and scripts inlined. This file opens instantly in a browser by
double-clicking it, with no server required. If your project uses the provided
GitHub Actions PR workflow, it will attach this file as an artifact: download
it from the PR's Checks tab, extract it, and open it locally, as GitHub does
not render artifact HTML directly in the browser. That workflow builds GRIBOVIK
from the PR's own checkout, so the file it produces is as trustworthy as the
branch it came from: a PR that edits the exporter or the frontend can put
anything it likes in the page you are about to open. Read such a diff before
opening the artifact.

The server binds loopback only, and answers only requests addressed to a
loopback name — `localhost`, `127.0.0.1` or `[::1]`, with or without a port.
Anything else gets a 403, so a web page cannot reach your unpushed diff by
pointing its own hostname at 127.0.0.1.

GRIBOVIK prints the URL it bound to and runs until `Ctrl+C`. If the range
contains no reviewable changes it says so and exits 0 without binding a port.
Any failure — not a git repository, no `origin/master` or `origin/main`, an
unknown revision, a port already in use — is reported as a single line on
stderr with exit code 1.

## Development

`build.rs` refuses to compile without `web/dist/index.html`, so on a fresh
clone the frontend has to be built once before any cargo command works:

```sh
just build-web    # or: cd web && npm ci && npm run build
```

Then:

```sh
just test         # cargo test, then npm test in web/
cargo test
cd web && npm test && npm run typecheck
```

While iterating on the UI, run `cargo run -- --assets web/dist` (or point
`--assets` at any `vite build` output) so the server reads the SPA from disk
instead of the embedded copy. To use Vite's dev server, start gribovik on the
port `web/vite.config.ts` proxies `/api` to — `cargo run -- --port 7777
--assets web/dist`, then `npm run dev` — since the default `--port 0` picks a
port the proxy has no way to guess.

See `CLAUDE.md` for the crate layout and the rules the codebase holds itself
to.

## License

See `LICENSE`.
