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

Hunks that fall outside every symbol — import blocks, top-level constants,
whitespace at the edges — are collected into a synthetic **file-level** card so
nothing in the diff goes unreviewed.

## Supported languages

| Language | Extensions |
| --- | --- |
| Rust | `.rs` |
| Swift | `.swift` |
| TypeScript / JavaScript | `.ts`, `.tsx`, `.js`, `.jsx` |

Changed files with any other extension are ignored. A file whose syntax the
parser rejects still gets a file-level card with the whole diff, plus a warning
in the banner at the top of the page.

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

GRIBOVIK prints the URL it bound to and runs until `Ctrl+C`. If the range
contains no reviewable changes it says so and exits 0 without binding a port.
Any failure — not a git repository, no `origin/master` or `origin/main`, an
unknown revision, a port already in use — is reported as a single line on
stderr with exit code 1.

## Where review state lives

Statuses and comments are written to:

```
<repo>/.git/gribovik/<base>..<head>.json
```

Slashes in revision names become `-`, so `origin/master..HEAD` is stored as
`origin-master..HEAD.json`. The file is JSON keyed by node id
(`<file>::<qualified_name>`), written atomically, and stable across saves so it
diffs cleanly if you ever open it. Because it lives inside `.git/` it is never
committed, is not shared with anyone else, and disappears with the clone. A
missing or corrupt file is treated as an empty review rather than an error.

Re-running GRIBOVIK on the same `base..head` picks the marks back up.

## Development

```sh
just test         # cargo test, then npm test in web/
cargo test
cd web && npm test && npm run typecheck
```

While iterating on the UI, run `cargo run -- --assets web/dist` (or point
`--assets` at any `vite build` output) so the server reads the SPA from disk
instead of the embedded copy.

See `CLAUDE.md` for the crate layout and the rules the codebase holds itself
to.

## License

See `LICENSE`.
