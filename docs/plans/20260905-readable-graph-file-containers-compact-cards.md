# A readable graph: file containers, compact cards, legible edges

## Overview

Rework the SPA so the whole-graph view answers "which files changed, what
changed in each, what calls what" at fit-view, and a single click drills into
one card's diff. Four moving parts: file container nodes laid out
hierarchically by elk, collapsed cards that expand in place on click, edges
with arrowheads plus a focus/dim mode, and a left panel that lists files and
zooms to them. Frontend only — no Rust, no change to `web/src/types/snapshot.ts`.

## Context

Files involved:

- Modify: `web/src/lib/transform.ts`, `web/src/lib/layout.ts`, `web/src/App.tsx`,
  `web/src/components/SymbolNode.tsx`, `web/src/components/ProgressPanel.tsx`,
  `web/src/styles.css`, `web/src/lib/transform.test.ts`, `web/src/lib/layout.test.ts`,
  `web/vite.config.ts`, `web/package.json`
- Create: `web/src/components/FileNode.tsx`, `web/src/lib/focus.ts`,
  `web/src/lib/focus.test.ts`, `web/src/components/SymbolNode.test.tsx`,
  `web/src/components/ProgressPanel.test.tsx`
- Untouched: everything under `src/` (Rust), `web/src/types/snapshot.ts`,
  `web/src/lib/snapshot.ts`, `web/src/lib/elk.ts`, `web/src/components/DiffView.tsx`

Related patterns:

- `layout.ts` keeps its worker / 30s timeout / grid-fallback structure; the elk
  graph it builds gains one level of hierarchy, nothing else moves.
- `transform.ts` stays pure and layout-free (all positions `(0, 0)`), which is
  what makes the new container wiring unit-testable.
- Comments in this repo explain *why*; new constants (card height, container
  padding, z-index) carry the same kind of note.

Dependencies:

- New dev deps `jsdom` and `@testing-library/react` (+ `@testing-library/dom`)
  so the two presentational components have real tests. `vite.config.ts` keeps
  `environment: "node"`; the two `.test.tsx` files opt in with a
  `@vitest-environment jsdom` docblock, so the existing lib tests are unaffected.

Decisions taken (with reasons, per the ticket):

- **Edge routing: React Flow `smoothstep`, not elk's orthogonal sections.**
  Consuming elk's `sections` means a custom edge component that re-derives its
  own path from bendpoints elk computed for its own port model, and those
  bendpoints do not line up with React Flow's left/right handle centres once a
  card is expanded or dragged. `smoothstep` is built in, costs nothing per
  edge, gives the orthogonal look the ticket asks for, and stays correct when a
  node moves. Arrowhead: the existing `ARROW` `MarkerType.ArrowClosed` marker
  (already in `transform.ts`) at the callee end; `ambiguous` keeps
  `AMBIGUOUS_DASH`.
- **One selection state drives both expansion and focus.** Clicking a card
  selects it: it expands and its neighbourhood is highlighted. Escape, a second
  click on the same card, or a click on the pane/a container clears both. Hover
  is a preview that only dims, and only while nothing is selected.
- **Containers never dim.** They are the map; dimming them would defeat the
  overview the ticket is asking for.
- **Container id is `file:<path>`.** Card ids are always `<file>::<name>` and
  therefore always contain `::`, so the two id spaces cannot collide.
- **Measured elk cost (local prototype, synthetic 572 cards / 1100 edges,
  90 files):** flat `0.21 s`, hierarchical `INCLUDE_CHILDREN` `2.30 s`. Within
  the "first paint within a few seconds" budget and it runs in the worker, so
  the default ships as-is. If the real MVP snapshot measures worse,
  `elk.layered.thoroughness: "1"` was measured at `0.78 s` with an equivalent
  canvas (22713x38695 vs 22583x38443) — that is the documented lever, applied
  only if the measurement in Task 6 demands it.

## Development Approach

- **Testing approach**: Regular (code first, then tests), matching the repo's
  existing `*.test.ts`-beside-the-code convention.
- Complete each task fully before moving to the next.
- Pure logic goes in `web/src/lib/*` and is tested in the node environment;
  only the two presentational components use jsdom.
- **CRITICAL: every task MUST include new/updated tests**
- **CRITICAL: all tests must pass before starting next task** — `cd web && npm test && npm run typecheck`

## Implementation Steps

### Task 1: Container nodes and per-file counters in `transform.ts`

**Files:**
- Modify: `web/src/lib/transform.ts`, `web/src/lib/transform.test.ts`

- [x] add `lineCounts(diff: DiffLine[]): { added: number; removed: number }` — counts `add` / `del` tags; export it, both the card and the container header use it
- [x] add `FileNodeData = { file: string; cardCount: number; added: number; removed: number }`, `FileFlowNode = Node<FileNodeData, "file">`, and `GraphFlowNode = SymbolFlowNode | FileFlowNode`
- [x] add `containerId(file: string): string` returning `` `file:${file}` `` with a comment on why it cannot collide with a card id
- [x] extend `SymbolNodeData` with the card's own `added`/`removed` counts so the collapsed card does not recount on every render
- [x] `toFlow` now emits, per file in first-appearance order, the container node followed by its cards — React Flow requires a parent to precede its children in the array; cards carry `parentId: containerId(file)` and `extent: "parent"`, containers carry `zIndex: 0` and cards `zIndex: 1`
- [x] `toFlow` returns a third field `files: FileSummary[]` (`{ file, containerId, cardCount, added, removed }`) for the navigation panel
- [x] give every edge `type: "smoothstep"` alongside the existing `markerEnd`/dash styling
- [x] tests: one container per file with the right `cardCount`/`+N −M`; containers precede their children; every card has the matching `parentId`; the `kind === "file"` card is an ordinary child, not the container; `files` summary matches; edges keep their endpoints, arrowhead, dash and gain `smoothstep`
- [x] run `cd web && npm test && npm run typecheck` — must pass before Task 2

### Task 2: Hierarchical layout and container-aware grid fallback

**Files:**
- Modify: `web/src/lib/layout.ts`, `web/src/lib/layout.test.ts`

- [x] replace `nodeHeight()` with a `CARD_HEIGHT` constant (collapsed card height, in the tens of px, measured against the stylesheet as the old chrome constant was) and export it; delete `CARD_CHROME_HEIGHT`/`DIFF_LINE_HEIGHT`/`MAX_DIFF_HEIGHT`
- [x] add `CONTAINER_PADDING` (`elk.padding` with a top inset large enough for the header) and a `HEADER_HEIGHT` constant shared with the stylesheet
- [x] `layout(nodes, edges)` builds a two-level elk graph: root children are the containers (each with its `elk.padding` and its cards as `children`), all edges stay at root, and `elk.hierarchyHandling: "INCLUDE_CHILDREN"` is added to `LAYOUT_OPTIONS` — verified locally to return container `x/y/width/height` and child positions relative to their container, which is exactly React Flow's `parentId` convention
- [x] read back container `width`/`height` into each container node (`width`/`height` fields) and child `x/y` into card positions; preserve input order and node data as today; keep the worker race, `workerFailure()` and `LAYOUT_TIMEOUT_MS` untouched
- [x] `gridLayout(nodes)` places one container per file in a column, sized `NODE_WIDTH + 2*pad` by `HEADER_HEIGHT + n*(CARD_HEIGHT + gap)`, with cards at container-relative positions
- [x] tests: children lie inside their container's box; containers are pairwise disjoint (extend the existing `overlapping` helper to work on absolute positions and per-node sizes); no two cards overlap on a `scattered(300)`-style multi-file graph; a cross-container edge still puts the callee's absolute x right of the caller's; card height no longer varies with diff length; grid fallback produces containers with sized boxes and non-overlapping children
- [x] run `cd web && npm test && npm run typecheck` — must pass before Task 3

### Task 3: Focus and dimming as a pure module

**Files:**
- Create: `web/src/lib/focus.ts`, `web/src/lib/focus.test.ts`

- [x] `neighbourhood(edges, id): { nodes: Set<string>; edges: Set<string> }` — the card itself, its direct callers and callees, and every incident edge
- [x] `applyFocus(nodes, edges, focusId, expandedId)` returns new node/edge arrays: outside the neighbourhood gets `className: "dimmed"`, incident edges get `"focused"`, containers are never dimmed, the expanded card gets `data.expanded = true` and `zIndex: EXPANDED_Z`; with `focusId === null` and `expandedId === null` it returns the arrays unchanged in content
- [x] tests: a selected card keeps itself, its callers and its callees undimmed and dims a third card; incident edges are `focused` and others `dimmed`; containers are never dimmed; the expanded card is the only one with `data.expanded` and an elevated `zIndex`; a null focus dims nothing
- [x] run `cd web && npm test && npm run typecheck` — must pass before Task 4

### Task 4: Compact card, expanded overlay, file container component

**Files:**
- Modify: `web/src/components/SymbolNode.tsx`, `web/src/styles.css`, `web/vite.config.ts`, `web/package.json`
- Create: `web/src/components/FileNode.tsx`, `web/src/components/SymbolNode.test.tsx`

- [ ] add `jsdom`, `@testing-library/react` and `@testing-library/dom` as dev deps; keep `environment: "node"` in `vite.config.ts` and opt the `.test.tsx` files into jsdom with a `@vitest-environment jsdom` docblock
- [ ] `SymbolNode` renders collapsed by default: name, kind, change badge, `+N −M`, on a fixed `CARD_HEIGHT` row; the file path moves off the card (its container header carries it) and the name gets an ellipsis so a long name cannot grow the box
- [ ] when `data.expanded`, the card additionally renders the existing `DiffView` in an absolutely positioned panel anchored under the collapsed row (`.symbol-expanded`), keeping the `nowheel nodrag` wrapper and the `.diff` scroll behaviour — the node's own box keeps its collapsed size, so nothing re-flows and the canvas cannot jump
- [ ] `FileNode`: a container box with a header showing the file path, `N cards` and `+N −M`; sized by the `width`/`height` layout wrote; not connectable, no handles
- [ ] styles: `.file-node` (header bar, translucent body, no clipping of children), `.symbol-node` collapsed variant, `.symbol-expanded` overlay, `.dimmed` (reduced opacity for nodes and `.react-flow__edge`), `.focused` edge stroke; a minimap `nodeColor` so containers do not black out the map
- [ ] register `file: FileNode` in `nodeTypes` in `App.tsx`
- [ ] tests (`SymbolNode.test.tsx`, rendered inside `ReactFlowProvider` so `Handle` has its store): collapsed renders name, kind, badge and `+N −M` and no diff lines; `data.expanded` renders the diff; a `kind === "file"` card shows the `file` badge
- [ ] run `cd web && npm test && npm run typecheck` — must pass before Task 5

### Task 5: App wiring — click to expand, Escape, focus, and the file panel

**Files:**
- Modify: `web/src/App.tsx`, `web/src/components/ProgressPanel.tsx`
- Create: `web/src/components/ProgressPanel.test.tsx`

- [ ] `Loaded` carries the `files` summary from `toFlow`; `Graph` wraps its subtree in `<ReactFlowProvider>` so the panel can call `fitView`
- [ ] `Graph` holds `selectedId` and `hoverId`; `focusId = selectedId ?? hoverId`; display nodes/edges come from `useMemo(() => applyFocus(nodes, edges, focusId, selectedId), …)` over the `useNodesState`/`useEdgesState` arrays, so drags and the layout survive a selection
- [ ] `onNodeClick` toggles `selectedId` for a `symbol` node and clears it for a container; `onPaneClick` clears it; a window `keydown` effect clears it on Escape
- [ ] `onNodeMouseEnter`/`Leave` set `hoverId` (preview only — selection remains the durable way in)
- [ ] `ProgressPanel` gains the file list: total card count on top, then one scrollable row per file with its path, card count and `+N −M`, sorted by path; clicking a row calls the `onSelectFile` prop
- [ ] `App` passes `onSelectFile` = `fitView({ nodes: [{ id: containerId }], duration: 400, padding: 0.2, maxZoom: 1 })` from `useReactFlow()`
- [ ] tests (`ProgressPanel.test.tsx`, jsdom): the total card count renders; one row per file with its counts; clicking a row calls `onSelectFile` with that file's container id
- [ ] run `cd web && npm test && npm run typecheck` — must pass before Task 6

### Task 6: Verify acceptance criteria and record the layout timings

**Files:**
- Modify: `docs/plans/20260905-readable-graph-file-containers-compact-cards.md` (Verification notes section)

- [ ] `cd web && npm test && npm run typecheck` green
- [ ] `cd web && npm run build` — both `dist/index.html` and `dist/export.html` produced
- [ ] `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` green (nothing under `src/` changed; this confirms it)
- [ ] `git diff --stat` shows no change under `src/` and none to `web/src/types/snapshot.ts`
- [ ] measure elk on the real MVP snapshot: export `gribovik --export /tmp/mvp.html 0dcb3f1~1 0dcb3f1`, then time `layout()` on its snapshot (a throwaway node script against the built `lib/layout.ts` inputs, flat vs hierarchical) and write both numbers into the Verification notes section of this plan file
- [ ] if hierarchical exceeds a few seconds, add `"elk.layered.thoroughness": "1"` to `LAYOUT_OPTIONS` with a comment carrying the measured before/after, and re-run the layout tests

### Task 7: Update documentation

**Files:**
- Modify: `README.md`, `CLAUDE.md`

- [ ] rewrite README's "In the browser" section: file containers with path / card count / `+N −M`, collapsed cards, click to expand a card's diff over its neighbours, Escape or a click on the canvas to collapse, arrowheads pointing caller→callee with dashed ambiguous edges, selection dimming everything outside a card's neighbourhood, and the left panel's file list zooming to a container
- [ ] update the `web/src/` layout tree in CLAUDE.md with `lib/focus.ts` and `components/FileNode.tsx`
- [ ] add a short paragraph to CLAUDE.md's frontend section recording the two decisions a future change would otherwise re-litigate: layout sizes cards by their *collapsed* height and expansion must never re-run it, and edge paths are React Flow `smoothstep` rather than elk's `sections`
- [ ] confirm the two-sided-`GraphSnapshot` section still reads true — no contract change was made
- [ ] run `cd web && npm test && npm run typecheck` and the three Rust gates one final time

## Post-Completion Manual Verification

Not automatable; run these by hand in a real browser after Task 7.

1. `cd web && npm run build`, then `cargo run -- --assets web/dist 85fbd06~1 85fbd06` (104 cards). Confirm: file containers with a path / count / `+N −M` header; collapsed cards legible at the initial `fitView`; clicking a card expands its diff over its neighbours with no canvas jump; Escape and a pane click collapse it; edges carry arrowheads at the callee end and ambiguous ones are dashed; selecting a card dims everything outside its neighbourhood; clicking a file in the left panel zooms to that container; pan/zoom stays smooth.
2. Repeat with the MVP range `cargo run -- --assets web/dist 0dcb3f1~1 0dcb3f1` (572 cards, ~1100 edges) and note the wait before first paint.
3. `cargo run -- --export /tmp/graph.html 85fbd06~1 85fbd06`, open `/tmp/graph.html` over `file://`, and confirm the same behaviour including click-to-expand and the file panel.

## Verification notes

(filled in during Task 6 — elk flat vs hierarchical timings on the MVP snapshot)
