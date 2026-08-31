/**
 * Snapshot → React Flow. Pure and layout-free: positions are all `(0, 0)`
 * until `layout.ts` places them, which keeps this module trivially testable.
 */

import type { Edge, Node } from "@xyflow/react";

import type { GraphSnapshot, SnapshotNode } from "../types/snapshot";

/** What a `SymbolNode` component receives. */
export type SymbolNodeData = {
  snapshot: SnapshotNode;
};

export type SymbolFlowNode = Node<SymbolNodeData, "symbol">;

/** The dash pattern that marks an edge the resolver was unsure about. */
export const AMBIGUOUS_DASH = "6 4";

/** Edge id for a `(from, to)` pair; the resolver already deduplicated these. */
export function edgeId(from: string, to: string): string {
  return `${from}->${to}`;
}

/**
 * Convert a snapshot into React Flow's node/edge shapes.
 *
 * Edges pointing at a node that is not in the snapshot are dropped rather than
 * rendered: React Flow warns and skips them anyway, and a silently missing
 * arrow is better than a console full of noise.
 */
export function toFlow(snapshot: GraphSnapshot): {
  nodes: SymbolFlowNode[];
  edges: Edge[];
} {
  const nodes: SymbolFlowNode[] = snapshot.nodes.map((node) => ({
    id: node.id,
    type: "symbol",
    position: { x: 0, y: 0 },
    data: { snapshot: node },
  }));

  const known = new Set(snapshot.nodes.map((node) => node.id));
  const edges: Edge[] = snapshot.edges
    .filter((edge) => known.has(edge.from) && known.has(edge.to))
    .map((edge) => ({
      id: edgeId(edge.from, edge.to),
      source: edge.from,
      target: edge.to,
      animated: false,
      style:
        edge.confidence === "ambiguous"
          ? { strokeDasharray: AMBIGUOUS_DASH }
          : {},
    }));

  return { nodes, edges };
}
