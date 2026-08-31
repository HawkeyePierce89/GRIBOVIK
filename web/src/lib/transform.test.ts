import { describe, expect, it } from "vitest";

import type { GraphSnapshot, SnapshotNode } from "../types/snapshot";
import { AMBIGUOUS_DASH, toFlow } from "./transform";

function node(id: string, overrides: Partial<SnapshotNode> = {}): SnapshotNode {
  return {
    id,
    file: id.split("::")[0] ?? "",
    name: id.split("::")[1] ?? "",
    kind: "function",
    change: "modified",
    diff: [],
    ...overrides,
  };
}

function snapshot(overrides: Partial<GraphSnapshot> = {}): GraphSnapshot {
  return {
    meta: {
      repo: "/tmp/repo",
      base: "abc123",
      head: "def456",
      files_changed: 2,
      warnings: [],
    },
    nodes: [],
    edges: [],
    ...overrides,
  };
}

describe("toFlow", () => {
  it("maps snapshot nodes to symbol nodes at the origin", () => {
    const alpha = node("src/a.rs::alpha", {
      change: "added",
      diff: [{ tag: "add", old_line: null, new_line: 1, text: "fn alpha() {}" }],
    });

    const { nodes } = toFlow(snapshot({ nodes: [alpha] }));

    expect(nodes).toEqual([
      {
        id: "src/a.rs::alpha",
        type: "symbol",
        position: { x: 0, y: 0 },
        data: { snapshot: alpha },
      },
    ]);
  });

  it("carries the whole snapshot node through as node data", () => {
    const file = node("src/a.rs::src/a.rs", {
      kind: "file",
      name: "src/a.rs",
      change: "modified",
    });

    const { nodes } = toFlow(snapshot({ nodes: [file] }));

    expect(nodes[0]?.data.snapshot).toBe(file);
  });

  it("builds unanimated edges keyed by their endpoints", () => {
    const { edges } = toFlow(
      snapshot({
        nodes: [node("src/b.rs::B::beta"), node("src/a.rs::alpha")],
        edges: [
          {
            from: "src/b.rs::B::beta",
            to: "src/a.rs::alpha",
            confidence: "certain",
          },
        ],
      }),
    );

    expect(edges).toEqual([
      {
        id: "src/b.rs::B::beta->src/a.rs::alpha",
        source: "src/b.rs::B::beta",
        target: "src/a.rs::alpha",
        animated: false,
        style: {},
      },
    ]);
  });

  it("dashes ambiguous edges and leaves certain ones solid", () => {
    const { edges } = toFlow(
      snapshot({
        nodes: [node("a::one"), node("b::two"), node("c::three")],
        edges: [
          { from: "a::one", to: "b::two", confidence: "ambiguous" },
          { from: "a::one", to: "c::three", confidence: "certain" },
        ],
      }),
    );

    expect(edges[0]?.style).toEqual({ strokeDasharray: AMBIGUOUS_DASH });
    expect(edges[1]?.style).toEqual({});
  });

  it("drops edges whose endpoints are missing from the snapshot", () => {
    const { edges } = toFlow(
      snapshot({
        nodes: [node("a::one")],
        edges: [
          { from: "a::one", to: "gone::two", confidence: "certain" },
          { from: "gone::two", to: "a::one", confidence: "certain" },
          { from: "nowhere::x", to: "nothing::y", confidence: "ambiguous" },
        ],
      }),
    );

    expect(edges).toEqual([]);
  });

  it("returns empty node and edge lists for an empty snapshot", () => {
    expect(toFlow(snapshot())).toEqual({ nodes: [], edges: [] });
  });
});
