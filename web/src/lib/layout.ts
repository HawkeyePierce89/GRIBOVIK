/**
 * Graph layout via elkjs.
 *
 * Layered left-to-right reads like a call chain: callers on the left, callees
 * to their right. elk runs in a web worker inside the browser bundle, so this
 * is async and must be awaited before the nodes are handed to React Flow.
 */

import ELK from "elkjs/lib/elk.bundled.js";
import type { Edge } from "@xyflow/react";

import { reviewFor } from "./review";
import type { SymbolFlowNode } from "./transform";
import type { ReviewState } from "../types/snapshot";

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
/** A `.comments li` is a timestamp line over a text line. */
const COMMENT_HEIGHT = 34;
/** `.comments` scrolls past `max-height: 8rem`. */
const MAX_COMMENTS_HEIGHT = 128;

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
 *
 * `state` is the review the cards will render with — comments are part of the
 * card, so reopening a review that already has some makes every commented card
 * taller than a diff-only estimate predicts, and elk stacks them into each
 * other. It defaults to empty for a first run, where no card has comments yet.
 */
export function nodeHeight(
  node: SymbolFlowNode,
  state: ReviewState = {},
): number {
  const lines = node.data.snapshot.diff.length;
  const comments = reviewFor(state, node.id).comments.length;
  return (
    CARD_CHROME_HEIGHT +
    Math.min(lines * DIFF_LINE_HEIGHT, MAX_DIFF_HEIGHT) +
    Math.min(comments * COMMENT_HEIGHT, MAX_COMMENTS_HEIGHT)
  );
}

/**
 * Place `nodes` with elk and return them with `position` filled in. Edges are
 * only read, never modified; the returned array preserves the input order.
 */
export async function layout(
  nodes: SymbolFlowNode[],
  edges: Edge[],
  state: ReviewState = {},
): Promise<SymbolFlowNode[]> {
  if (nodes.length === 0) return [];

  const graph = {
    id: "root",
    layoutOptions: LAYOUT_OPTIONS,
    children: nodes.map((node) => ({
      id: node.id,
      width: NODE_WIDTH,
      height: nodeHeight(node, state),
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
