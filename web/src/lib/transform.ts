/**
 * Snapshot → React Flow. Pure and layout-free: positions are all `(0, 0)`
 * until `layout.ts` places them, which keeps this module trivially testable.
 */

import { MarkerType, type Edge, type Node } from "@xyflow/react";

import type { DiffLine, GraphSnapshot, SnapshotNode } from "../types/snapshot";

/** What a `SymbolNode` component receives. */
export type SymbolNodeData = {
  snapshot: SnapshotNode;
  /**
   * The card's own added/removed line counts. Computed here, once, because a
   * collapsed card shows them on every render and a card's diff never
   * changes for the lifetime of the session.
   */
  added: number;
  removed: number;
  /**
   * Set by `focus.ts` on the selected card, and by nothing else: an expanded
   * card draws its diff in an overlay anchored to its collapsed box. Optional
   * because `toFlow` emits every card collapsed.
   */
  expanded?: boolean;
};

export type SymbolFlowNode = Node<SymbolNodeData, "symbol">;

/** What a `FileNode` container receives: the header, and nothing else. */
export type FileNodeData = {
  file: string;
  cardCount: number;
  added: number;
  removed: number;
};

export type FileFlowNode = Node<FileNodeData, "file">;

/** Everything the canvas holds: one container per file, and its cards. */
export type GraphFlowNode = SymbolFlowNode | FileFlowNode;

/** One row of the navigation panel's file list. */
export type FileSummary = {
  file: string;
  containerId: string;
  cardCount: number;
  added: number;
  removed: number;
};

/**
 * Containers sit under their cards, and an expanded card sits over both — see
 * `focus.ts` for the top of that stack.
 */
export const CONTAINER_Z = 0;
export const CARD_Z = 1;

/** The dash pattern that marks an edge the resolver was unsure about. */
export const AMBIGUOUS_DASH = "6 4";

/**
 * Every edge is drawn caller → callee.
 *
 * The arrowhead is not decoration: this is a call graph, and left-to-right
 * placement only reads as direction until the layout has to route an edge
 * backwards — which a cycle, or a callee that also calls its caller's
 * neighbour, produces on any real branch. Without a head those two cards are
 * connected and nothing says which one calls the other.
 */
export const ARROW = { type: MarkerType.ArrowClosed, width: 18, height: 18 };

/**
 * Orthogonal segments rather than a bezier: a card is a rectangle in a
 * rectangle, and right-angled edges read as wiring between boxes where a
 * curve reads as a swoosh across them. React Flow computes the path from the
 * handles it actually rendered, so it stays correct when a card is dragged —
 * which elk's own `sections`, computed for elk's port model, would not.
 */
export const EDGE_TYPE = "smoothstep";

/** Edge id for a `(from, to)` pair; the resolver already deduplicated these. */
export function edgeId(from: string, to: string): string {
  return `${from}->${to}`;
}

/**
 * Node id of the container holding `file`'s cards.
 *
 * A card id is `"<file>::<qualified_name>"` and therefore always carries a
 * `::`, so keeping container ids free of `::` keeps the two id spaces apart.
 * A `file:` prefix on a bare path does not do that on its own: a path may
 * contain a colon, and a container for `a.ts::b.tsx` would then collide with
 * a card for a file literally named `file:a.ts`. Percent-escaping every colon
 * — and the `%` that makes the escape injective — leaves the prefix's own
 * colon as the only one in the result. Paths carrying neither character, which
 * is every path anyone commits, come out unchanged.
 */
export function containerId(file: string): string {
  return `file:${file.replaceAll("%", "%25").replaceAll(":", "%3A")}`;
}

/** How many lines a diff adds and removes; context lines count for neither. */
export function lineCounts(diff: DiffLine[]): {
  added: number;
  removed: number;
} {
  let added = 0;
  let removed = 0;
  for (const line of diff) {
    if (line.tag === "add") added += 1;
    else if (line.tag === "del") removed += 1;
  }
  return { added, removed };
}

/**
 * Convert a snapshot into React Flow's node/edge shapes.
 *
 * Cards are grouped into one container node per file, in the order the files
 * first appear in the snapshot. A container is emitted immediately before its
 * own cards because React Flow resolves `parentId` against the nodes it has
 * already seen — a child ahead of its parent in the array is an error.
 *
 * Edges pointing at a node that is not in the snapshot are dropped rather than
 * rendered: React Flow warns and skips them anyway, and a silently missing
 * arrow is better than a console full of noise.
 */
export function toFlow(snapshot: GraphSnapshot): {
  nodes: GraphFlowNode[];
  edges: Edge[];
  files: FileSummary[];
} {
  const byFile = new Map<string, SnapshotNode[]>();
  for (const node of snapshot.nodes) {
    const cards = byFile.get(node.file);
    if (cards === undefined) byFile.set(node.file, [node]);
    else cards.push(node);
  }

  const nodes: GraphFlowNode[] = [];
  const files: FileSummary[] = [];

  for (const [file, cards] of byFile) {
    const parent = containerId(file);
    let added = 0;
    let removed = 0;

    // Built before the container is pushed, because the container's header
    // sums what its cards carry. That is a total over the file's *cards*, not
    // over the file's diff: `nodes.rs` drops a leftover line whose text is
    // empty rather than carding a file for a blank line gained between two
    // symbols, so a file that grew a function typically reads a handful of
    // lines under `git diff --stat`. Every line worth reviewing is counted —
    // which is what the header is for — but the number is not git's.
    const children: SymbolFlowNode[] = cards.map((card) => {
      const counts = lineCounts(card.diff);
      added += counts.added;
      removed += counts.removed;
      return {
        id: card.id,
        type: "symbol",
        position: { x: 0, y: 0 },
        parentId: parent,
        // A card belongs to its file: dragging one must not smuggle it into
        // the box of a file it did not change.
        extent: "parent",
        zIndex: CARD_Z,
        // Unselectable for the same reason as the container below, one step
        // removed: a card carries a `parentId`, and `getElevatedEdgeZIndex`
        // gives every edge the higher z of its two endpoints whenever either
        // one has a parent. So a card left selected in React Flow's store
        // holds its own edges 1000 above every card they cross. `App` clears
        // its selection on Escape, on a container and from the file panel,
        // and none of those three paths touch React Flow's — which would
        // leave the dismissed card's arrows painted over the graph until some
        // other card was clicked. Nothing reads React Flow's `selected`.
        selectable: false,
        data: { snapshot: card, added: counts.added, removed: counts.removed },
      };
    });

    nodes.push({
      id: parent,
      type: "file",
      position: { x: 0, y: 0 },
      zIndex: CONTAINER_Z,
      // React Flow's own selection is unused — `App` tracks the open card in
      // its own state — but `elevateNodesOnSelect` is on by default and adds
      // 1000 to a selected node's z. A container is a translucent box that elk
      // routes edges *through*, so letting it be selected would paint its
      // panel over every edge crossing it (including its own file's) until
      // something else was clicked. `onNodeClick` still fires without this, so
      // clicking a container to dismiss a diff keeps working.
      selectable: false,
      // A container is the map, and React Flow marks every *draggable* node
      // `nopan`, which makes its whole box reject the pane's drag filter.
      // Containers blanket the canvas and most of one is card-free
      // background, so leaving them draggable means the surface a reviewer
      // naturally grabs to pan does not pan — it rips that file's box, and
      // via `extent: "parent"` every card in it, out of the elk layout with
      // no way back short of a reload. `onNodeClick` fires without this too.
      draggable: false,
      data: { file, cardCount: cards.length, added, removed },
    });
    nodes.push(...children);
    files.push({
      file,
      containerId: parent,
      cardCount: cards.length,
      added,
      removed,
    });
  }

  const known = new Set(snapshot.nodes.map((node) => node.id));
  const edges: Edge[] = snapshot.edges
    .filter((edge) => known.has(edge.from) && known.has(edge.to))
    .map((edge) => ({
      id: edgeId(edge.from, edge.to),
      source: edge.from,
      target: edge.to,
      type: EDGE_TYPE,
      animated: false,
      markerEnd: ARROW,
      style:
        edge.confidence === "ambiguous"
          ? { strokeDasharray: AMBIGUOUS_DASH }
          : {},
    }));

  return { nodes, edges, files };
}
