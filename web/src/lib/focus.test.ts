import { describe, expect, it } from "vitest";

import type { GraphSnapshot } from "../types/snapshot";
import {
  DIMMED,
  EXPANDED_Z,
  FOCUSED,
  applyFocus,
  neighbourhood,
} from "./focus";
import { containerId, edgeId, toFlow } from "./transform";

/**
 * Two files, four cards, and a neighbourhood that does not cover them all:
 * `a::z` calls `a::x` calls `a::y` calls `b::w`. Focusing `a::x` leaves `b::w`
 * two hops away, which is what makes the dimming visible at all.
 */
function graph(): GraphSnapshot {
  const ids = ["a.rs::x", "a.rs::y", "a.rs::z", "b.rs::w"];
  return {
    meta: {
      repo: "/tmp/repo",
      base: "abc123",
      head: "def456",
      files_changed: 2,
      warnings: [],
    },
    nodes: ids.map((id) => ({
      id,
      file: id.split("::")[0]!,
      name: id.split("::")[1]!,
      kind: "function",
      change: "modified" as const,
      diff: [],
    })),
    edges: [
      { from: "a.rs::z", to: "a.rs::x", confidence: "certain" as const },
      { from: "a.rs::x", to: "a.rs::y", confidence: "certain" as const },
      { from: "a.rs::y", to: "b.rs::w", confidence: "certain" as const },
    ],
  };
}

const flow = () => toFlow(graph());

/** The class the focus pass put on `id`, or `undefined` if it left it alone. */
function classOf(
  items: { id: string; className?: string }[],
  id: string,
): string | undefined {
  return items.find((item) => item.id === id)?.className;
}

describe("neighbourhood", () => {
  it("holds the card, its callers, its callees and their edges", () => {
    const near = neighbourhood(flow().edges, "a.rs::x");

    expect([...near.nodes].sort()).toEqual(["a.rs::x", "a.rs::y", "a.rs::z"]);
    expect([...near.edges].sort()).toEqual(
      [edgeId("a.rs::x", "a.rs::y"), edgeId("a.rs::z", "a.rs::x")].sort(),
    );
  });

  it("stops at one hop rather than walking the whole component", () => {
    expect(neighbourhood(flow().edges, "a.rs::x").nodes.has("b.rs::w")).toBe(
      false,
    );
  });

  it("gives a card nothing calls a neighbourhood of itself alone", () => {
    const near = neighbourhood([], "a.rs::x");

    expect([...near.nodes]).toEqual(["a.rs::x"]);
    expect(near.edges.size).toBe(0);
  });
});

describe("applyFocus", () => {
  it("dims every card outside the focused card's neighbourhood", () => {
    const { nodes, edges } = flow();

    const focused = applyFocus(nodes, edges, "a.rs::x", null);

    expect(classOf(focused.nodes, "a.rs::x")).toBeUndefined();
    expect(classOf(focused.nodes, "a.rs::y")).toBeUndefined();
    expect(classOf(focused.nodes, "a.rs::z")).toBeUndefined();
    expect(classOf(focused.nodes, "b.rs::w")).toBe(DIMMED);
  });

  it("never dims a container, whichever card is focused", () => {
    const { nodes, edges } = flow();

    const focused = applyFocus(nodes, edges, "a.rs::x", "a.rs::x");

    expect(classOf(focused.nodes, containerId("a.rs"))).toBeUndefined();
    // `b.rs` holds only a dimmed card and is still the map of where it lives.
    expect(classOf(focused.nodes, containerId("b.rs"))).toBeUndefined();
  });

  it("focuses the incident edges and dims the rest", () => {
    const { nodes, edges } = flow();

    const focused = applyFocus(nodes, edges, "a.rs::x", null);

    expect(classOf(focused.edges, edgeId("a.rs::z", "a.rs::x"))).toBe(FOCUSED);
    expect(classOf(focused.edges, edgeId("a.rs::x", "a.rs::y"))).toBe(FOCUSED);
    expect(classOf(focused.edges, edgeId("a.rs::y", "b.rs::w"))).toBe(DIMMED);
  });

  it("keeps the edges' endpoints and styling", () => {
    const { nodes, edges } = flow();

    const focused = applyFocus(nodes, edges, "a.rs::x", null);

    expect(focused.edges.map((edge) => [edge.source, edge.target])).toEqual(
      edges.map((edge) => [edge.source, edge.target]),
    );
    expect(focused.edges[0]!.type).toBe(edges[0]!.type);
    expect(focused.edges[0]!.markerEnd).toBe(edges[0]!.markerEnd);
  });

  it("expands exactly one card and lifts it over everything else", () => {
    const { nodes, edges } = flow();

    const focused = applyFocus(nodes, edges, "a.rs::x", "a.rs::x");

    const expanded = focused.nodes.filter(
      (node) => node.type === "symbol" && node.data.expanded === true,
    );
    expect(expanded.map((node) => node.id)).toEqual(["a.rs::x"]);
    expect(expanded[0]!.zIndex).toBe(EXPANDED_Z);
    // The dimmed cards keep the ordinary stacking order they were given.
    expect(classOf(focused.nodes, "b.rs::w")).toBe(DIMMED);
    expect(focused.nodes.find((node) => node.id === "b.rs::w")!.zIndex).toBe(
      nodes.find((node) => node.id === "b.rs::w")!.zIndex,
    );
  });

  it("expands a card without dimming anything when only hover is unset", () => {
    const { nodes, edges } = flow();

    const focused = applyFocus(nodes, edges, null, "a.rs::x");

    expect(focused.nodes.some((node) => node.className === DIMMED)).toBe(false);
    expect(focused.edges.some((edge) => edge.className !== undefined)).toBe(
      false,
    );
  });

  it("dims nothing when nothing is selected or hovered", () => {
    const { nodes, edges } = flow();

    const focused = applyFocus(nodes, edges, null, null);

    expect(focused.nodes).toEqual(nodes);
    expect(focused.edges).toEqual(edges);
  });

  it("dims nothing when the focus is a container, which calls nothing", () => {
    const { nodes, edges } = flow();

    const focused = applyFocus(nodes, edges, containerId("a.rs"), null);

    expect(focused.nodes.some((node) => node.className === DIMMED)).toBe(false);
    expect(focused.edges.some((edge) => edge.className !== undefined)).toBe(
      false,
    );
  });

  it("leaves the input arrays untouched", () => {
    const { nodes, edges } = flow();
    const before = structuredClone({ nodes, edges });

    applyFocus(nodes, edges, "a.rs::x", "a.rs::x");

    expect({ nodes, edges }).toEqual(before);
  });
});
