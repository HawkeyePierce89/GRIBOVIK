/**
 * Graph layout via elkjs.
 *
 * Layered left-to-right reads like a call chain: callers on the left, callees
 * to their right. elk runs in a web worker inside the browser bundle, so this
 * is async and must be awaited before the nodes are handed to React Flow.
 */

import ELK from "elkjs/lib/elk.bundled.js";
import type { Edge } from "@xyflow/react";

import type { SymbolFlowNode } from "./transform";

/** Card width, fixed by `.symbol-node` in the stylesheet. */
export const NODE_WIDTH = 420;

/**
 * Everything on a card except the diff: header, name, buttons, comment form,
 * padding and gaps. Measured against the stylesheet rather than the DOM —
 * layout runs once, before any card has rendered.
 */
const CARD_CHROME_HEIGHT = 176;
/** `.diff-line` is `12px/1.45` monospace. */
const DIFF_LINE_HEIGHT = 18;
/** `.diff` scrolls past `max-height: 16rem`, so taller cards do not exist. */
const MAX_DIFF_HEIGHT = 256;

const elk = new ELK();

const LAYOUT_OPTIONS: Record<string, string> = {
  "elk.algorithm": "layered",
  "elk.direction": "RIGHT",
  "elk.layered.spacing.nodeNodeBetweenLayers": "120",
  "elk.spacing.nodeNode": "60",
  "elk.layered.nodePlacement.strategy": "NETWORK_SIMPLEX",
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
 * Place `nodes` with elk and return them with `position` filled in. Edges are
 * only read, never modified; the returned array preserves the input order.
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

  const laid = await elk.layout(graph);
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
