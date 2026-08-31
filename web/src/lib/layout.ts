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

/** Card size assumed before the DOM has measured anything. */
export const DEFAULT_NODE_WIDTH = 420;
export const DEFAULT_NODE_HEIGHT = 260;

const elk = new ELK();

const LAYOUT_OPTIONS: Record<string, string> = {
  "elk.algorithm": "layered",
  "elk.direction": "RIGHT",
  "elk.layered.spacing.nodeNodeBetweenLayers": "120",
  "elk.spacing.nodeNode": "60",
  "elk.layered.nodePlacement.strategy": "NETWORK_SIMPLEX",
};

/** Per-node measured size, when the DOM already knows better than the default. */
export type NodeSize = { width: number; height: number };

/**
 * Place `nodes` with elk and return them with `position` filled in. Edges are
 * only read, never modified; the returned array preserves the input order.
 */
export async function layout(
  nodes: SymbolFlowNode[],
  edges: Edge[],
  sizes: Record<string, NodeSize> = {},
): Promise<SymbolFlowNode[]> {
  if (nodes.length === 0) return [];

  const graph = {
    id: "root",
    layoutOptions: LAYOUT_OPTIONS,
    children: nodes.map((node) => ({
      id: node.id,
      width: sizes[node.id]?.width ?? DEFAULT_NODE_WIDTH,
      height: sizes[node.id]?.height ?? DEFAULT_NODE_HEIGHT,
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
