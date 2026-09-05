/**
 * Graph layout via elkjs.
 *
 * Layered left-to-right reads like a call chain: callers on the left, callees
 * to their right. Cards are nested one level deep, inside the container node
 * their file owns, so the whole-graph view is a map of files first and of
 * symbols second. Layout is async and must be awaited before the nodes are
 * handed to React Flow.
 */

import type { Edge } from "@xyflow/react";

import type { GraphFlowNode } from "./transform";

/** Card width, fixed by `.symbol-node` in the stylesheet. */
export const NODE_WIDTH = 420;

/**
 * How tall a *collapsed* card renders, measured against the stylesheet rather
 * than the DOM — layout runs once, before any card exists to measure.
 * `.symbol-node` is a single row: 0.75rem of padding above and below, a 1px
 * border on each side, and a badge-height line of content.
 *
 * It is a constant, not a function of the diff, and that is the whole point of
 * the collapsed card: expanding one draws its diff in an overlay anchored to
 * this box, so the node's own size never changes and the layout never has to
 * run again. Sizing cards by their diff length is what used to make the canvas
 * a wall of unequal boxes.
 */
export const CARD_HEIGHT = 52;

/**
 * The container header — file path, card count, `+N −M` — as `.file-header`
 * renders it. Layout has to reserve it: elk knows only that a container has
 * padding, and a card placed under the top inset would sit behind the bar.
 */
export const HEADER_HEIGHT = 36;

/** Breathing room between a container's border and the cards inside it. */
const CONTAINER_INSET = 16;

/**
 * `elk.padding` for a container: the header, plus the same inset all round.
 * elk's own spelling — `[top=…,left=…,bottom=…,right=…]` — is the only format
 * the layered algorithm parses for this option.
 */
const CONTAINER_PADDING = `[top=${HEADER_HEIGHT + CONTAINER_INSET},left=${CONTAINER_INSET},bottom=${CONTAINER_INSET},right=${CONTAINER_INSET}]`;

/** The gap the grid fallback leaves between boxes, containers and cards alike. */
const GRID_GAP = 24;

/**
 * elk in a real web worker, so the tab stays alive while it thinks.
 *
 * `elk.bundled.js` defaults to a *fake* worker that runs the algorithm inline:
 * a few hundred cards is ten seconds of frozen page before the first card
 * appears. Supplying a factory is what moves it off the main thread — the
 * bundled build honours one. Outside a browser (the unit tests run in node)
 * there is no `Worker`, and the inline fallback is what we want anyway.
 */
import { elk, workerFailure } from "./elk";

/**
 * `BRANDES_KOEPF` rather than `NETWORK_SIMPLEX`: the simplex placement is
 * superlinear in edges and dominates the wait before the first card appears.
 * Measured on this repository's own diff (569 cards, 1070 edges — an ordinary
 * feature branch): 13.8 s against 0.47 s, for a placement difference a
 * reviewer has to look for. The worker keeps the tab alive during that wait,
 * but a blank "Loading…" is a blank page either way.
 */
const LAYOUT_OPTIONS: Record<string, string> = {
  "elk.algorithm": "layered",
  "elk.direction": "RIGHT",
  "elk.layered.spacing.nodeNodeBetweenLayers": "120",
  // The visible gap between cards. `nodeNode` separates cards inside one
  // connected component; anything the edge resolver found no caller for is a
  // component of its own, and those are packed by `componentComponent` — most
  // of a real graph, and 20px apart if left at its default.
  "elk.spacing.nodeNode": "60",
  "elk.spacing.componentComponent": "60",
  "elk.layered.nodePlacement.strategy": "BRANDES_KOEPF",
  // Every edge lives at the root while its endpoints live inside containers.
  // Without this, elk treats a container as opaque and refuses to route
  // through it, and cross-file calls stop influencing where files land.
  "elk.hierarchyHandling": "INCLUDE_CHILDREN",
};

/** The cards a container holds, in the order `toFlow` emitted them. */
function groupByContainer(
  nodes: GraphFlowNode[],
): Map<string, GraphFlowNode[]> {
  const groups = new Map<string, GraphFlowNode[]>();
  for (const node of nodes) {
    if (node.type === "file") {
      if (!groups.has(node.id)) groups.set(node.id, []);
      continue;
    }
    // `toFlow` gives every card a `parentId`, but React Flow's type does not,
    // and a cast would hand elk a child under the group key `undefined` —
    // which elk rejects, taking the whole graph down the grid fallback for one
    // malformed node. Dropping the card costs that card its position and
    // nothing else.
    const parent = node.parentId;
    if (parent === undefined) continue;
    const cards = groups.get(parent);
    if (cards === undefined) groups.set(parent, [node]);
    else cards.push(node);
  }
  return groups;
}

/** A container wide and tall enough for `count` cards stacked in a column. */
function containerBox(count: number): { width: number; height: number } {
  return {
    width: NODE_WIDTH + 2 * CONTAINER_INSET,
    height:
      HEADER_HEIGHT +
      CONTAINER_INSET +
      Math.max(count, 1) * (CARD_HEIGHT + GRID_GAP),
  };
}

/**
 * Positions without elk: one container per file in a row, cards stacked inside
 * it in snapshot order.
 *
 * Layout is the one part of the load that is purely cosmetic — the diff is
 * readable without it — so a worker that fails to start must not cost the
 * reviewer the whole review. The containers are wide enough that no two cards
 * overlap, which is the only property the canvas actually needs.
 */
export function gridLayout(nodes: GraphFlowNode[]): GraphFlowNode[] {
  const groups = groupByContainer(nodes);
  const columnX = new Map<string, number>();
  let x = 0;
  for (const parent of groups.keys()) {
    columnX.set(parent, x);
    x += containerBox(groups.get(parent)!.length).width + GRID_GAP;
  }

  const rank = new Map<string, number>();
  for (const cards of groups.values()) {
    cards.forEach((card, index) => rank.set(card.id, index));
  }

  return nodes.map((node) => {
    if (node.type === "file") {
      const box = containerBox(groups.get(node.id)?.length ?? 0);
      return {
        ...node,
        position: { x: columnX.get(node.id) ?? 0, y: 0 },
        width: box.width,
        height: box.height,
      };
    }
    // A card's position is relative to its container, which is React Flow's
    // `parentId` convention and elk's too — the fallback has to agree with the
    // real layout or a failed worker would scatter the cards across the pane.
    const index = rank.get(node.id) ?? 0;
    return {
      ...node,
      position: {
        x: CONTAINER_INSET,
        y: HEADER_HEIGHT + CONTAINER_INSET + index * (CARD_HEIGHT + GRID_GAP),
      },
    };
  });
}

/**
 * How long elk gets before the caller stops waiting for it.
 *
 * Not a performance budget — measured layout of an ordinary branch is under a
 * second. It is the only way out of a worker that never answers: elkjs wraps
 * the worker in a promise it resolves from `onmessage` and installs no error
 * handler, so a worker script that fails to load leaves a promise that never
 * settles and a page stuck on "Loading…" forever. Rejecting is what makes the
 * caller's grid fallback — written for exactly this — reachable.
 */
const LAYOUT_TIMEOUT_MS = 30_000;

/** Reject after `ms`, and never keep a timer alive past the race. */
function timeout(ms: number): { promise: Promise<never>; cancel: () => void } {
  let timer: ReturnType<typeof setTimeout>;
  const promise = new Promise<never>((_, reject) => {
    timer = setTimeout(
      () => reject(new Error(`layout did not finish within ${ms}ms`)),
      ms,
    );
  });
  return { promise, cancel: () => clearTimeout(timer) };
}

/**
 * Place `nodes` with elk and return them with `position` filled in, and
 * containers additionally with the `width`/`height` elk sized them to. Edges
 * are only read, never modified; the returned array preserves the input order.
 *
 * The elk graph is two levels deep — root holds the containers, a container
 * holds its cards — while every edge stays at the root, because an edge
 * between two files has no container to belong to. elk answers in the same
 * shape: container coordinates are absolute, a card's are relative to the
 * container holding it, which is exactly what React Flow's `parentId` expects.
 *
 * Rejects if elk does not answer within [`LAYOUT_TIMEOUT_MS`].
 */
export async function layout(
  nodes: GraphFlowNode[],
  edges: Edge[],
): Promise<GraphFlowNode[]> {
  if (nodes.length === 0) return [];

  const groups = groupByContainer(nodes);
  const graph = {
    id: "root",
    layoutOptions: LAYOUT_OPTIONS,
    children: [...groups].map(([parent, cards]) => ({
      id: parent,
      layoutOptions: { "elk.padding": CONTAINER_PADDING },
      children: cards.map((card) => ({
        id: card.id,
        width: NODE_WIDTH,
        height: CARD_HEIGHT,
      })),
    })),
    edges: edges.map((edge) => ({
      id: edge.id,
      sources: [edge.source],
      targets: [edge.target],
    })),
  };

  const limit = timeout(LAYOUT_TIMEOUT_MS);
  const laid = await Promise.race([
    elk.layout(graph),
    workerFailure(),
    limit.promise,
  ]).finally(limit.cancel);

  const placed = new Map<
    string,
    { x?: number; y?: number; width?: number; height?: number }
  >();
  for (const container of laid.children ?? []) {
    placed.set(container.id, container);
    for (const card of container.children ?? []) placed.set(card.id, card);
  }

  return nodes.map((node) => {
    const box = placed.get(node.id);
    const position = { x: box?.x ?? 0, y: box?.y ?? 0 };
    if (node.type !== "file") return { ...node, position };
    const fallback = containerBox(groups.get(node.id)?.length ?? 0);
    return {
      ...node,
      position,
      width: box?.width ?? fallback.width,
      height: box?.height ?? fallback.height,
    };
  });
}
