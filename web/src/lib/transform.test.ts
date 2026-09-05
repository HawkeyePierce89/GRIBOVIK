import { describe, expect, it } from "vitest";

import type { GraphSnapshot, SnapshotNode } from "../types/snapshot";
import {
  AMBIGUOUS_DASH,
  ARROW,
  CARD_Z,
  CONTAINER_Z,
  EDGE_TYPE,
  containerId,
  lineCounts,
  toFlow,
} from "./transform";

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

function line(tag: "add" | "del" | "context", text = "x") {
  return { tag, old_line: null, new_line: null, text };
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

describe("lineCounts", () => {
  it("counts added and removed lines and ignores context", () => {
    expect(
      lineCounts([
        line("add"),
        line("context"),
        line("del"),
        line("add"),
        line("context"),
      ]),
    ).toEqual({ added: 2, removed: 1 });
  });

  it("counts an empty diff as no change either way", () => {
    expect(lineCounts([])).toEqual({ added: 0, removed: 0 });
  });
});

describe("containerId", () => {
  it("cannot collide with a card id, which always carries a `::`", () => {
    const card = node("src/a.rs::alpha");
    expect(containerId(card.file)).toBe("file:src/a.rs");
    expect(containerId(card.file)).not.toBe(card.id);
  });
});

describe("toFlow", () => {
  it("maps snapshot nodes to symbol nodes at the origin, inside their file", () => {
    const alpha = node("src/a.rs::alpha", {
      change: "added",
      diff: [
        { tag: "add", old_line: null, new_line: 1, text: "fn alpha() {}" },
      ],
    });

    const { nodes } = toFlow(snapshot({ nodes: [alpha] }));

    expect(nodes[1]).toEqual({
      id: "src/a.rs::alpha",
      type: "symbol",
      position: { x: 0, y: 0 },
      parentId: "file:src/a.rs",
      extent: "parent",
      zIndex: CARD_Z,
      data: { snapshot: alpha, added: 1, removed: 0 },
    });
  });

  it("emits one container per file, carrying its card count and line counts", () => {
    const { nodes } = toFlow(
      snapshot({
        nodes: [
          node("src/a.rs::alpha", { diff: [line("add"), line("del")] }),
          node("src/b.rs::beta", { diff: [line("add")] }),
          node("src/a.rs::gamma", { diff: [line("add"), line("context")] }),
        ],
      }),
    );

    const containers = nodes.filter((flow) => flow.type === "file");
    expect(containers).toEqual([
      {
        id: "file:src/a.rs",
        type: "file",
        position: { x: 0, y: 0 },
        zIndex: CONTAINER_Z,
        data: {
          file: "src/a.rs",
          cardCount: 2,
          added: 2,
          removed: 1,
        },
      },
      {
        id: "file:src/b.rs",
        type: "file",
        position: { x: 0, y: 0 },
        zIndex: CONTAINER_Z,
        data: { file: "src/b.rs", cardCount: 1, added: 1, removed: 0 },
      },
    ]);
  });

  it("puts every container before its own cards, as React Flow requires", () => {
    const { nodes } = toFlow(
      snapshot({
        nodes: [
          node("src/a.rs::alpha"),
          node("src/b.rs::beta"),
          node("src/a.rs::gamma"),
        ],
      }),
    );

    expect(nodes.map((flow) => flow.id)).toEqual([
      "file:src/a.rs",
      "src/a.rs::alpha",
      "src/a.rs::gamma",
      "file:src/b.rs",
      "src/b.rs::beta",
    ]);
    for (const [index, flow] of nodes.entries()) {
      if (flow.type !== "symbol") continue;
      const parent = nodes.findIndex((other) => other.id === flow.parentId);
      expect(parent).toBeGreaterThanOrEqual(0);
      expect(parent).toBeLessThan(index);
    }
  });

  it("gives every card the container of the file it belongs to", () => {
    const { nodes } = toFlow(
      snapshot({
        nodes: [node("src/a.rs::alpha"), node("src/b.rs::beta")],
      }),
    );

    const cards = nodes.filter((flow) => flow.type === "symbol");
    expect(cards.map((card) => [card.id, card.parentId, card.extent])).toEqual([
      ["src/a.rs::alpha", "file:src/a.rs", "parent"],
      ["src/b.rs::beta", "file:src/b.rs", "parent"],
    ]);
  });

  it("keeps the file-level card an ordinary child, not the container", () => {
    const file = node("src/a.rs::src/a.rs", {
      kind: "file",
      name: "src/a.rs",
      change: "modified",
    });

    const { nodes } = toFlow(snapshot({ nodes: [file] }));

    expect(nodes[0]?.type).toBe("file");
    expect(nodes[0]?.id).toBe("file:src/a.rs");
    expect(nodes[1]?.type).toBe("symbol");
    expect(nodes[1]?.id).toBe("src/a.rs::src/a.rs");
  });

  it("carries the whole snapshot node through as node data", () => {
    const file = node("src/a.rs::src/a.rs", { kind: "file", name: "src/a.rs" });

    const { nodes } = toFlow(snapshot({ nodes: [file] }));

    expect(nodes[1]?.type).toBe("symbol");
    expect(nodes[1]?.type === "symbol" && nodes[1].data.snapshot).toBe(file);
  });

  it("summarises the files for the navigation panel", () => {
    const { files } = toFlow(
      snapshot({
        nodes: [
          node("src/a.rs::alpha", { diff: [line("add"), line("del")] }),
          node("src/b.rs::beta", { diff: [line("del")] }),
          node("src/a.rs::gamma", { diff: [line("add")] }),
        ],
      }),
    );

    expect(files).toEqual([
      {
        file: "src/a.rs",
        containerId: "file:src/a.rs",
        cardCount: 2,
        added: 2,
        removed: 1,
      },
      {
        file: "src/b.rs",
        containerId: "file:src/b.rs",
        cardCount: 1,
        added: 0,
        removed: 1,
      },
    ]);
  });

  it("builds unanimated orthogonal edges keyed by their endpoints", () => {
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
        type: EDGE_TYPE,
        animated: false,
        markerEnd: ARROW,
        style: {},
      },
    ]);
  });

  it("points every edge at its callee, so direction survives a back-route", () => {
    const { edges } = toFlow(
      snapshot({
        nodes: [node("a::one"), node("b::two")],
        edges: [
          { from: "a::one", to: "b::two", confidence: "certain" },
          { from: "b::two", to: "a::one", confidence: "ambiguous" },
        ],
      }),
    );

    expect(edges.map((edge) => edge.markerEnd)).toEqual([ARROW, ARROW]);
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
    expect(edges.map((edge) => edge.type)).toEqual([EDGE_TYPE, EDGE_TYPE]);
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

  it("returns empty node, edge and file lists for an empty snapshot", () => {
    expect(toFlow(snapshot())).toEqual({ nodes: [], edges: [], files: [] });
  });
});
