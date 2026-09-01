# GRIBOVIK

GRIBOVIK explores the project’s sprawling mycelial network.

It turns a git range into an **interactive diff graph**: every changed
function, method, or type becomes a card carrying its own line diff, and the
calls between those cards become edges. You review the graph in a browser —
approve, reject, or leave pending, and leave comments — and GRIBOVIK remembers
your marks between runs.

```
gribovik                     # review this branch against origin/master
```

## What it does

1. Resolves a revision range (`base..head`) with plain `git`.
2. Parses the before/after text of every changed file with tree-sitter and
   works out which symbols the diff actually touched.
3. Classifies each touched symbol as **added**, **modified**, or **deleted**,
   and slices out just the diff lines belonging to it.
4. Resolves calls between changed symbols into edges — `certain` when the
   callee is unambiguous, `ambiguous` (drawn dashed) when several changed
   symbols share the name.
5. Serves the result on `localhost` to a React Flow SPA laid out left-to-right
   with elkjs, and persists your review state under `.git/gribovik/`.

Changed lines that fall outside every symbol — import blocks, top-level
constants, `impl` scaffolding — are collected into a synthetic **file-level**
card, so a hunk that straddles a symbol boundary is reviewed on both cards
rather than only on the symbol's. Blank lines added or removed between symbols
are the one exception: they carry nothing to review and would otherwise put a
file card on almost every file that gained a function.

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
catch-alls — and how many are approved, rejected, or still pending. Clicking a counter highlights exactly those cards; clicking
it again clears the highlight. **Approved cards fade to 45% opacity** — that is
deliberate, not a rendering glitch, so the canvas visibly empties as you work.

Each card carries its file path, a change badge, its slice of the diff, the
three verdict buttons, and a comment box. The graph pans and zooms, with a
minimap and controls in the corners; a dashed edge is one the call resolver was
not sure about. Warnings from the analysis sit in a banner at the top, and a
failed save shows a red banner at the bottom rather than silently losing marks.

## Build

The binary embeds the frontend, so the SPA must be built first. `build.rs`
refuses to compile without `web/dist/index.html`.

```sh
just build        # npm ci && npm run build in web/, then cargo build --release
```

Or without `just`:

```sh
cd web && npm ci && npm run build && cd ..
cargo build --release
```

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
| `--port <PORT>` | `0` | Port to serve on. `0` asks the OS for a free one. |
| `--no-open` | off | Print the URL instead of opening a browser. |
| `--assets <DIR>` | embedded | Serve the frontend from a directory on disk instead of the build baked into the binary. Useful with `web/dist` while working on the UI. |

The server binds loopback only, and answers only requests addressed to a
loopback name — `localhost`, `127.0.0.1` or `[::1]`, with or without a port.
Anything else gets a 403, so a web page cannot reach your unpushed diff by
pointing its own hostname at 127.0.0.1.

GRIBOVIK prints the URL it bound to and runs until `Ctrl+C`. If the range
contains no reviewable changes it says so and exits 0 without binding a port.
Any failure — not a git repository, no `origin/master` or `origin/main`, an
unknown revision, a port already in use — is reported as a single line on
stderr with exit code 1.

## Where review state lives

Statuses and comments are written to:

```
<git-dir>/gribovik/<merge-base>..<head>.json
```

`<git-dir>` is whatever `git rev-parse --git-common-dir` reports — usually
`<repo>/.git`, but a linked worktree or a submodule keeps its data elsewhere,
and every worktree of a repository shares one review.

The base component is the **resolved merge base**, not the revision you typed.
The head component is the revision you typed, except that a bare `HEAD` — the
default — is expanded to the branch you are on, or to the short commit if the
checkout is detached. Two branches cut from the same commit share a merge base,
so without that expansion they would share a review file and each would open on
the other's verdicts. `gribovik origin/master` on `feature/x` therefore writes
something like `9f3c1e…a2..feature%2Fx.json`; anything outside
`[A-Za-z0-9._-]` is percent-encoded, so two branch names can never land on one
file.

One consequence is worth knowing: fetching new commits onto the base branch or
rebasing moves the merge base, which starts a fresh review file — the old marks
stay on disk under the old merge base but are no longer loaded.

The file is JSON keyed by node id (`<file>::<qualified_name>`), written
atomically, and stable across saves so it diffs cleanly if you ever open it.
Because it lives inside the git directory it is never committed, is not shared
with anyone else, and disappears with the clone. A missing or corrupt file is
treated as an empty review rather than an error.

Re-running GRIBOVIK on the same `base..head` picks the marks back up — except
for the cards whose code changed in the meantime. Each entry records a digest of
the diff it was decided on, and a card whose diff no longer matches opens as
**pending** again, because an approval of the previous version is not an
approval of this one. Comments are kept either way; they are your words, not a
verdict. A state file written before this digest existed carries none, and is
read the safe way round: every status in it comes back as pending.

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
