import { describe, expect, it } from "vitest";

import type { GraphSnapshot } from "../types/snapshot";
import {
  CARD_HEIGHT,
  HEADER_HEIGHT,
  NODE_WIDTH,
  gridLayout,
  layout,
} from "./layout";
import {
  containerId,
  toFlow,
  type FileFlowNode,
  type GraphFlowNode,
  type SymbolFlowNode,
} from "./transform";

/** The cards of a graph, without the containers `toFlow` wraps them in. */
function cards(nodes: GraphFlowNode[]): SymbolFlowNode[] {
  return nodes.filter((node): node is SymbolFlowNode => node.type === "symbol");
}

function containers(nodes: GraphFlowNode[]): FileFlowNode[] {
  return nodes.filter((node): node is FileFlowNode => node.type === "file");
}

function chain(): GraphSnapshot {
  return {
    meta: {
      repo: "/tmp/repo",
      base: "abc123",
      head: "def456",
      files_changed: 1,
      warnings: [],
    },
    nodes: ["a::caller", "a::callee"].map((id) => ({
      id,
      file: "a",
      name: id,
      kind: "function",
      change: "modified" as const,
      diff: [],
    })),
    edges: [{ from: "a::caller", to: "a::callee", confidence: "certain" }],
  };
}

/** The same two-card chain, with the callee living in a second file. */
function crossFile(): GraphSnapshot {
  const snapshot = chain();
  snapshot.nodes[1] = { ...snapshot.nodes[1]!, id: "b::callee", file: "b" };
  snapshot.edges = [
    { from: "a::caller", to: "b::callee", confidence: "certain" },
  ];
  return snapshot;
}

/**
 * A snapshot of `count` cards spread over ten files, in components of assorted
 * shapes: singletons, pairs and short chains, with diffs of differing lengths.
 *
 * The mix is the point. Uniform singletons in one file are packed into a tidy
 * column by any spacing at all; it is containers of unequal size, laid out
 * beside each other with edges crossing between them, that break a careless
 * box.
 */
function scattered(count: number): GraphSnapshot {
  const snapshot = chain();
  const file = (i: number) => `f${i % 10}`;
  snapshot.nodes = Array.from({ length: count }, (_, i) => ({
    id: `${file(i)}::n${i}`,
    file: file(i),
    name: `n${i}`,
    kind: "function",
    change: "modified" as const,
    diff: Array.from({ length: (i % 9) * 3 }, () => ({
      tag: "add" as const,
      old_line: null,
      new_line: 1,
      text: "x",
    })),
  }));
  snapshot.edges = [];
  // One large many-layered component alongside singletons: the layers are
  // where `elk.spacing.nodeNode` governs, the leftovers are packed around it.
  for (let i = 0; i < count; i += 1) {
    if (i % 3 === 0) continue;
    const to = (i * 7 + 5) % count;
    snapshot.edges.push({
      from: `${file(i)}::n${i}`,
      to: `${file(to)}::n${to}`,
      confidence: "certain" as const,
    });
  }
  return snapshot;
}

type Rect = { id: string; x: number; y: number; width: number; height: number };

/**
 * Absolute boxes for everything on the canvas.
 *
 * A card's `position` is relative to its container — React Flow's `parentId`
 * convention, and elk's — so nothing can be compared until the container's
 * origin is added back in.
 */
function rects(placed: GraphFlowNode[]): Rect[] {
  const origin = new Map(
    containers(placed).map((node) => [node.id, node.position]),
  );
  return placed.map((node) => {
    if (node.type === "file") {
      return {
        id: node.id,
        x: node.position.x,
        y: node.position.y,
        width: node.width ?? 0,
        height: node.height ?? 0,
      };
    }
    const parent = origin.get(node.parentId as string) ?? { x: 0, y: 0 };
    return {
      id: node.id,
      x: parent.x + node.position.x,
      y: parent.y + node.position.y,
      width: NODE_WIDTH,
      height: CARD_HEIGHT,
    };
  });
}

function byId(list: Rect[]): Map<string, Rect> {
  return new Map(list.map((rect) => [rect.id, rect]));
}

/** Every pair of boxes drawn over each other. */
function overlapping(boxes: Rect[]): string[] {
  const out: string[] = [];
  for (let i = 0; i < boxes.length; i += 1) {
    for (let j = i + 1; j < boxes.length; j += 1) {
      const a = boxes[i]!;
      const b = boxes[j]!;
      const apart =
        a.x + a.width <= b.x ||
        b.x + b.width <= a.x ||
        a.y + a.height <= b.y ||
        b.y + b.height <= a.y;
      if (!apart) out.push(`${a.id} over ${b.id}`);
    }
  }
  return out;
}

describe("layout", () => {
  it("puts a callee to the right of its caller across containers", async () => {
    const { nodes, edges } = toFlow(crossFile());

    const placed = await layout(nodes, edges);

    const box = byId(rects(placed));
    const caller = box.get("a::caller")!;
    const callee = box.get("b::callee")!;
    expect(callee.x).toBeGreaterThanOrEqual(caller.x + NODE_WIDTH);
  });

  it("preserves input order and node data", async () => {
    const { nodes, edges } = toFlow(chain());

    const placed = await layout(nodes, edges);

    expect(placed.map((node) => node.id)).toEqual(nodes.map((node) => node.id));
    expect(placed[0]?.data).toEqual(nodes[0]?.data);
  });

  it("sizes every card alike, however long its diff", async () => {
    const { nodes, edges } = toFlow(scattered(30));

    const placed = await layout(nodes, edges);

    // Nothing on a collapsed card grows with the diff, so nothing in the
    // layout may either: expansion draws over the neighbours instead of
    // asking for a second layout pass.
    const heights = new Set(
      rects(placed)
        .filter((rect) => rect.id.includes("::"))
        .map((rect) => rect.height),
    );
    expect([...heights]).toEqual([CARD_HEIGHT]);
  });

  it("keeps every card inside its own container's box", async () => {
    const { nodes, edges } = toFlow(scattered(120));

    const placed = await layout(nodes, edges);

    const box = byId(rects(placed));
    const escaped: string[] = [];
    for (const card of cards(placed)) {
      const inner = box.get(card.id)!;
      const outer = box.get(card.parentId as string)!;
      const inside =
        inner.x >= outer.x &&
        inner.y >= outer.y &&
        inner.x + inner.width <= outer.x + outer.width &&
        inner.y + inner.height <= outer.y + outer.height;
      if (!inside) escaped.push(`${card.id} outside ${outer.id}`);
    }
    expect(escaped).toEqual([]);
  });

  it("leaves the header row clear of the topmost card", async () => {
    const { nodes, edges } = toFlow(scattered(40));

    const placed = await layout(nodes, edges);

    // Positions are container-relative, so the header inset is readable
    // directly: a card at y < HEADER_HEIGHT would render behind the bar.
    for (const card of cards(placed)) {
      expect(card.position.y).toBeGreaterThanOrEqual(HEADER_HEIGHT);
    }
  });

  it("draws no two containers over each other", async () => {
    const { nodes, edges } = toFlow(scattered(300));

    const placed = await layout(nodes, edges);

    const boxes = rects(placed).filter((rect) => rect.id.startsWith("file:"));
    expect(overlapping(boxes)).toEqual([]);
  });

  it("stacks cards without overlapping, whatever their diffs", async () => {
    // Enough cards that elk has to pack them into columns: two of them are
    // placed clear of each other by any spacing at all, which is what lets a
    // broken box pass unnoticed.
    const { nodes, edges } = toFlow(scattered(300));

    const placed = await layout(nodes, edges);

    const boxes = rects(placed).filter((rect) => !rect.id.startsWith("file:"));
    expect(overlapping(boxes)).toEqual([]);
  });

  it("returns nothing for an empty graph", async () => {
    expect(await layout([], [])).toEqual([]);
  });
});

describe("gridLayout", () => {
  it("gives every container a sized box and no two overlap", () => {
    const { nodes } = toFlow(scattered(40));

    const placed = gridLayout(nodes);

    for (const container of containers(placed)) {
      expect(container.width).toBeGreaterThan(NODE_WIDTH);
      expect(container.height).toBeGreaterThan(HEADER_HEIGHT);
    }
    expect(
      overlapping(rects(placed).filter((rect) => rect.id.startsWith("file:"))),
    ).toEqual([]);
  });

  it("keeps cards from overlapping when elk is unavailable", () => {
    const { nodes } = toFlow(scattered(40));

    const placed = gridLayout(nodes);

    const boxes = rects(placed);
    expect(
      overlapping(boxes.filter((rect) => !rect.id.startsWith("file:"))),
    ).toEqual([]);
    const box = byId(boxes);
    for (const card of cards(placed)) {
      const inner = box.get(card.id)!;
      const outer = box.get(card.parentId as string)!;
      expect(inner.x).toBeGreaterThanOrEqual(outer.x);
      expect(inner.y + inner.height).toBeLessThanOrEqual(
        outer.y + outer.height,
      );
    }
  });

  it("gives each file its own column", () => {
    const snapshot = chain();
    snapshot.nodes[1]!.file = "b";
    snapshot.nodes[1]!.id = "b::callee";
    snapshot.edges = [];
    const { nodes } = toFlow(snapshot);

    const placed = containers(gridLayout(nodes));

    expect(placed[0]!.id).toBe(containerId("a"));
    expect(placed[0]!.position.x).toBe(0);
    expect(placed[1]!.position.x).toBeGreaterThanOrEqual(NODE_WIDTH);
  });

  it("stacks same-file cards clear of each other", () => {
    const { nodes } = toFlow(chain());

    const placed = cards(gridLayout(nodes));

    expect(placed[1]!.position.x).toBe(placed[0]!.position.x);
    expect(placed[1]!.position.y).toBeGreaterThanOrEqual(
      placed[0]!.position.y + CARD_HEIGHT,
    );
  });
});
