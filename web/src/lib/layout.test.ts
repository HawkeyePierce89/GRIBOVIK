import { describe, expect, it } from "vitest";

import type { GraphSnapshot } from "../types/snapshot";
import { NODE_WIDTH, gridLayout, layout, nodeHeight } from "./layout";
import { toFlow } from "./transform";

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

/**
 * A snapshot of `count` cards in components of assorted shapes: singletons,
 * pairs and short chains, with diffs of differing lengths.
 *
 * The mix is the point. Uniform singletons are packed into tidy columns by any
 * spacing at all; it is components of unequal height and width, laid out beside
 * each other, that a spacing-based headroom fails to keep apart.
 */
function scattered(count: number): GraphSnapshot {
  const snapshot = chain();
  snapshot.nodes = Array.from({ length: count }, (_, i) => ({
    id: `a::n${i}`,
    file: "a",
    name: `a::n${i}`,
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
  // where `elk.spacing.nodeNode` governs, the leftovers are packed around it,
  // and it is the two together that a spacing-based headroom fails to keep
  // apart.
  for (let i = 0; i < count; i += 1) {
    if (i % 3 === 0) continue;
    snapshot.edges.push({
      from: `a::n${i}`,
      to: `a::n${(i * 7 + 5) % count}`,
      confidence: "certain" as const,
    });
  }
  return snapshot;
}

/**
 * Every pair of cards drawn over each other, given how tall each card renders.
 *
 * Sorting by `y` and walking the list is not enough: elk packs unconnected
 * components side by side, so two cards can be vertically adjacent in that
 * order and hundreds of pixels apart horizontally. Only cards whose x ranges
 * overlap can collide, and those are the pairs to check.
 */
function overlapping(
  placed: ReturnType<typeof toFlow>["nodes"],
  heightOf: (node: (typeof placed)[number]) => number,
): string[] {
  const out: string[] = [];
  for (let i = 0; i < placed.length; i += 1) {
    for (let j = i + 1; j < placed.length; j += 1) {
      const a = placed[i]!;
      const b = placed[j]!;
      const apart =
        a.position.x + NODE_WIDTH <= b.position.x ||
        b.position.x + NODE_WIDTH <= a.position.x;
      if (apart) continue;
      const [top, bottom] =
        a.position.y < b.position.y ? [a, b] : [b, a];
      if (bottom.position.y < top.position.y + heightOf(top)) {
        out.push(`${top.id} over ${bottom.id}`);
      }
    }
  }
  return out;
}

describe("layout", () => {
  it("puts a callee to the right of its caller", async () => {
    const { nodes, edges } = toFlow(chain());

    const placed = await layout(nodes, edges);

    const caller = placed.find((node) => node.id === "a::caller");
    const callee = placed.find((node) => node.id === "a::callee");
    expect(callee!.position.x).toBeGreaterThanOrEqual(
      caller!.position.x + NODE_WIDTH,
    );
  });

  it("preserves input order and node data", async () => {
    const { nodes, edges } = toFlow(chain());

    const placed = await layout(nodes, edges);

    expect(placed.map((node) => node.id)).toEqual(nodes.map((node) => node.id));
    expect(placed[0]?.data).toEqual(nodes[0]?.data);
  });

  it("grows a card's height with its diff and caps it at the scroll height", () => {
    const [node] = toFlow(chain()).nodes;
    const withLines = (count: number) => ({
      ...node!,
      data: {
        snapshot: {
          ...node!.data.snapshot,
          diff: Array.from({ length: count }, () => ({
            tag: "add" as const,
            old_line: null,
            new_line: 1,
            text: "x",
          })),
        },
      },
    });

    const empty = nodeHeight(withLines(0));
    expect(nodeHeight(withLines(5))).toBeGreaterThan(empty);
    // `.diff` scrolls past its max height, so 200 lines is no taller than 50.
    expect(nodeHeight(withLines(200))).toEqual(nodeHeight(withLines(50)));
  });

  it("stacks cards without overlapping, whatever their diffs", async () => {
    // Enough cards that elk has to pack them into columns: two of them are
    // placed clear of each other by any spacing at all, which is what let a
    // broken headroom pass unnoticed.
    const { nodes, edges } = toFlow(scattered(300));

    const placed = await layout(nodes, edges);

    // Each card's box has to clear the box above it. A fixed height for every
    // card is exactly what breaks this.
    expect(overlapping(placed, (node) => nodeHeight(node))).toEqual([]);
  });

  it("grows a card's height with the comments already saved on it", async () => {
    const snapshot = chain();
    snapshot.edges = [];
    const { nodes, edges } = toFlow(snapshot);
    const commented = {
      "a::caller": {
        status: "pending" as const,
        comments: Array.from({ length: 3 }, () => ({
          text: "why this?",
          created_at: "2026-09-01T00:00:00Z",
        })),
      },
    };

    expect(nodeHeight(nodes[0]!, commented)).toBeGreaterThan(
      nodeHeight(nodes[0]!),
    );

    // Reopening a review must not stack a commented card into its neighbour:
    // laying out with an empty state is exactly what does that.
    const placed = await layout(nodes, edges, commented);
    expect(overlapping(placed, (node) => nodeHeight(node, commented))).toEqual(
      [],
    );
  });

  it("leaves room below a card for comments added after layout", async () => {
    const { nodes, edges } = toFlow(scattered(300));

    // Laid out with no comments at all — the first-run case.
    const placed = await layout(nodes, edges);

    // Then the reviewer fills both cards with comments, past the point where
    // the list starts scrolling. Positions do not move, so the gap left below
    // each card has to absorb every pixel that card can still grow by.
    const full = Object.fromEntries(
      nodes.map((node) => [
        node.id,
        {
          status: "pending" as const,
          comments: Array.from({ length: 20 }, () => ({
            text: "why this?",
            created_at: "2026-09-01T00:00:00Z",
          })),
        },
      ]),
    );
    expect(overlapping(placed, (node) => nodeHeight(node, full))).toEqual([]);
  });

  it("leaves the same room between cards that share a caller", async () => {
    // Two callees of one caller sit in the same layer of one component, where
    // a different elk spacing governs the gap than between the disconnected
    // components above. Both have to carry the headroom.
    const snapshot = chain();
    snapshot.nodes.push({
      ...snapshot.nodes[1]!,
      id: "a::other",
      name: "a::other",
    });
    snapshot.edges.push({
      from: "a::caller",
      to: "a::other",
      confidence: "certain",
    });
    const { nodes, edges } = toFlow(snapshot);

    const placed = await layout(nodes, edges);

    const full = Object.fromEntries(
      nodes.map((node) => [
        node.id,
        {
          status: "pending" as const,
          comments: Array.from({ length: 20 }, () => ({
            text: "why this?",
            created_at: "2026-09-01T00:00:00Z",
          })),
        },
      ]),
    );
    const callees = placed
      .filter((node) => node.id !== "a::caller")
      .sort((a, b) => a.position.y - b.position.y);
    expect(callees[1]!.position.y).toBeGreaterThanOrEqual(
      callees[0]!.position.y + nodeHeight(callees[0]!, full),
    );
  });

  it("returns nothing for an empty graph", async () => {
    expect(await layout([], [])).toEqual([]);
  });
});

describe("gridLayout", () => {
  it("keeps cards from overlapping when elk is unavailable", () => {
    const snapshot = chain();
    snapshot.nodes[1]!.file = "b";
    const { nodes } = toFlow(snapshot);

    const placed = gridLayout(nodes);

    // One column per file, and a file's cards stacked clear of each other —
    // the only property the canvas needs from a fallback.
    expect(placed[0]!.position).toEqual({ x: 0, y: 0 });
    expect(placed[1]!.position.x).toBeGreaterThanOrEqual(NODE_WIDTH);
    expect(placed[1]!.position.y).toBe(0);
  });

  it("stacks same-file cards clear of each other", () => {
    const { nodes } = toFlow(chain());

    const placed = gridLayout(nodes);

    expect(placed[1]!.position.x).toBe(placed[0]!.position.x);
    expect(placed[1]!.position.y).toBeGreaterThanOrEqual(
      placed[0]!.position.y + nodeHeight(nodes[0]!),
    );
  });

  it("leaves the same room for later comments as elk does", () => {
    const { nodes } = toFlow(chain());

    const placed = gridLayout(nodes);

    const full = {
      "a::caller": {
        status: "pending" as const,
        comments: Array.from({ length: 20 }, () => ({
          text: "why this?",
          created_at: "2026-09-01T00:00:00Z",
        })),
      },
    };
    expect(placed[1]!.position.y).toBeGreaterThanOrEqual(
      placed[0]!.position.y + nodeHeight(nodes[0]!, full),
    );
  });
});
