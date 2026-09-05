/**
 * Selection as a pure function over the graph.
 *
 * A reviewer reading one card wants two things at once: that card's diff, and
 * which cards it talks to. Both come from a single selected id — the card
 * expands, and everything outside its neighbourhood fades — so this module is
 * given the current arrays and hands back the ones React Flow should draw. It
 * is pure and derives nothing from React, which is what lets the canvas keep
 * its own dragged positions in `useNodesState` and re-derive the *appearance*
 * on every selection without ever laying the graph out again.
 */

import type { Edge } from "@xyflow/react";

import type { GraphFlowNode } from "./transform";

/** Faded: outside the selection's neighbourhood. */
export const DIMMED = "dimmed";

/** Emphasised: an edge incident to the selected card. */
export const FOCUSED = "focused";

/**
 * The expanded card's stacking order.
 *
 * Its diff panel is drawn outside the collapsed box, over whatever the layout
 * put below it, so it has to sit above every container (`CONTAINER_Z`) and
 * every other card (`CARD_Z`). One card is expanded at a time, so a single
 * value above both is the whole stack.
 */
export const EXPANDED_Z = 1000;

/**
 * The card itself, its direct callers and callees, and every edge incident to
 * it.
 *
 * One hop, not the transitive closure: two hops out from a well-connected
 * card is most of the graph, and a highlight that covers most of the graph
 * highlights nothing.
 */
export function neighbourhood(
  edges: Edge[],
  id: string,
): { nodes: Set<string>; edges: Set<string> } {
  const nodes = new Set<string>([id]);
  const incident = new Set<string>();
  for (const edge of edges) {
    if (edge.source === id) {
      nodes.add(edge.target);
      incident.add(edge.id);
    } else if (edge.target === id) {
      nodes.add(edge.source);
      incident.add(edge.id);
    }
  }
  return { nodes, edges: incident };
}

/**
 * Apply the current selection to the arrays the canvas renders.
 *
 * `focusId` drives the dimming (a hover preview or the selection itself) and
 * `expandedId` — always the selection — the one card that shows its diff.
 * With neither set the inputs are returned as they are: the base arrays never
 * carry a `className`, so there is nothing to clear, and handing React Flow
 * the same references is one fewer reason for it to re-render every card.
 *
 * Containers are deliberately exempt from dimming. They are the map the
 * reviewer navigates by — file path, card count, `+N −M` — and fading them
 * while reading one card would take away the only thing that says *where* the
 * card is.
 */
export function applyFocus(
  nodes: GraphFlowNode[],
  edges: Edge[],
  focusId: string | null,
  expandedId: string | null,
): { nodes: GraphFlowNode[]; edges: Edge[] } {
  // A container has no calls, so focusing one would dim every card on the
  // canvas and highlight nothing. Only a card's neighbourhood means anything.
  const isCard =
    focusId !== null &&
    nodes.some((node) => node.type === "symbol" && node.id === focusId);
  const focus = isCard ? focusId : null;

  // After `focus`, not before: a container is the biggest hover target on the
  // canvas and resolves to no focus at all, so pointing at one would otherwise
  // return a content-identical array with a new identity — and React Flow
  // rebuilds its whole node lookup for every one of those.
  if (focus === null && expandedId === null) return { nodes, edges };

  const near = focus === null ? null : neighbourhood(edges, focus);

  return {
    nodes: nodes.map((node) => {
      // Containers are exempt from both: they never dim, and a file is not a
      // card with a diff to expand.
      if (node.type !== "symbol") return node;
      // `!expanded` first: the two ids arrive independently, so a caller
      // previewing one card's neighbourhood while another is open would
      // otherwise draw the open diff at the dimmed opacity — fading the one
      // card the reviewer asked to read.
      const expanded = node.id === expandedId;
      const dimmed = !expanded && near !== null && !near.nodes.has(node.id);
      if (!dimmed && !expanded) return node;
      return {
        ...node,
        ...(dimmed ? { className: DIMMED } : {}),
        ...(expanded
          ? { zIndex: EXPANDED_Z, data: { ...node.data, expanded: true } }
          : {}),
      };
    }),
    edges:
      near === null
        ? edges
        : edges.map((edge) => ({
            ...edge,
            className: near.edges.has(edge.id) ? FOCUSED : DIMMED,
          })),
  };
}
