/**
 * Graph layout via elkjs.
 *
 * Layered left-to-right reads like a call chain: callers on the left, callees
 * to their right. Layout is async and must be awaited before the nodes are
 * handed to React Flow.
 */

import ELK from "elkjs/lib/elk.bundled.js";
import type { Edge } from "@xyflow/react";

import type { SymbolFlowNode } from "./transform";

/** Card width, fixed by `.symbol-node` in the stylesheet. */
export const NODE_WIDTH = 420;

/**
 * Everything on a card except the diff: header, name, padding and gaps.
 * Measured against the stylesheet rather than the DOM — layout runs once,
 * before any card has rendered.
 */
const CARD_CHROME_HEIGHT = 96;
/** `.diff-line` is `12px/1.45` monospace. */
const DIFF_LINE_HEIGHT = 18;
/** `.diff` scrolls past `max-height: 16rem`, so taller cards do not exist. */
const MAX_DIFF_HEIGHT = 256;

/**
 * elk in a real web worker, so the tab stays alive while it thinks.
 *
 * `elk.bundled.js` defaults to a *fake* worker that runs the algorithm inline:
 * a few hundred cards is ten seconds of frozen page before the first card
 * appears. Supplying a factory is what moves it off the main thread — the
 * bundled build honours one. Outside a browser (the unit tests run in node)
 * there is no `Worker`, and the inline fallback is what we want anyway.
 */
/**
 * Rejects if a layout worker ever fails.
 *
 * elkjs resolves its layout promise from the worker's `onmessage` and installs
 * no `onerror`, so a worker script that fails to load simply never answers.
 * Racing against this turns that silence into a rejection the caller can fall
 * back from, instead of a page that says "Loading…" until the timeout.
 */
let workerFailed: Promise<never> = new Promise(() => {});

const elk = new ELK(
  typeof Worker === "undefined"
    ? {}
    : {
        workerFactory: () => {
          const worker = new Worker(
            new URL("elkjs/lib/elk-worker.min.js", import.meta.url),
            { type: "module" },
          );
          workerFailed = new Promise((_, reject) => {
            worker.addEventListener("error", (event) =>
              reject(new Error(`layout worker failed: ${event.message}`)),
            );
          });
          return worker;
        },
      },
);

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
};

/**
 * How tall a card will render. A card's height is driven by how many diff
 * lines it carries, and telling elk that one card is 200px and another 430px
 * is what keeps a long diff from being drawn over its neighbour.
 */
export function nodeHeight(node: SymbolFlowNode): number {
  const lines = node.data.snapshot.diff.length;
  return (
    CARD_CHROME_HEIGHT + Math.min(lines * DIFF_LINE_HEIGHT, MAX_DIFF_HEIGHT)
  );
}

/**
 * Positions without elk: one column per file, cards stacked in snapshot order.
 *
 * Layout is the one part of the load that is purely cosmetic — the diff is
 * readable without it — so a worker that fails to start must not cost the
 * reviewer the whole review. The columns are wide enough that no two cards
 * overlap, which is the only property the canvas actually needs.
 */
export function gridLayout(nodes: SymbolFlowNode[]): SymbolFlowNode[] {
  const columns = new Map<string, number>();
  const nextY = new Map<string, number>();
  return nodes.map((node) => {
    const file = node.data.snapshot.file;
    if (!columns.has(file)) columns.set(file, columns.size);
    const y = nextY.get(file) ?? 0;
    nextY.set(file, y + nodeHeight(node) + 60);
    return {
      ...node,
      position: { x: (columns.get(file) as number) * (NODE_WIDTH + 120), y },
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
 * Place `nodes` with elk and return them with `position` filled in. Edges are
 * only read, never modified; the returned array preserves the input order.
 *
 * Rejects if elk does not answer within [`LAYOUT_TIMEOUT_MS`].
 */
export async function layout(
  nodes: SymbolFlowNode[],
  edges: Edge[],
): Promise<SymbolFlowNode[]> {
  if (nodes.length === 0) return [];

  const graph = {
    id: "root",
    layoutOptions: LAYOUT_OPTIONS,
    children: nodes.map((node) => ({
      id: node.id,
      width: NODE_WIDTH,
      height: nodeHeight(node),
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
    workerFailed,
    limit.promise,
  ]).finally(limit.cancel);
  const placed = new Map(
    (laid.children ?? []).map((child) => [child.id, child]),
  );

  return nodes.map((node) => {
    const child = placed.get(node.id);
    return {
      ...node,
      position: { x: child?.x ?? 0, y: child?.y ?? 0 },
    };
  });
}
