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
    const snapshot = chain();
    snapshot.edges = [];
    snapshot.nodes[0]!.diff = Array.from({ length: 40 }, () => ({
      tag: "add" as const,
      old_line: null,
      new_line: 1,
      text: "x",
    }));
    const { nodes, edges } = toFlow(snapshot);

    const placed = await layout(nodes, edges);

    // Unconnected cards share a column, so each one's box has to clear the box
    // above it. A fixed height for every card is exactly what breaks this.
    const column = [...placed].sort((a, b) => a.position.y - b.position.y);
    for (let i = 1; i < column.length; i += 1) {
      const above = column[i - 1]!;
      expect(column[i]!.position.y).toBeGreaterThanOrEqual(
        above.position.y + nodeHeight(above),
      );
    }
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
    const column = [...placed].sort((a, b) => a.position.y - b.position.y);
    for (let i = 1; i < column.length; i += 1) {
      const above = column[i - 1]!;
      expect(column[i]!.position.y).toBeGreaterThanOrEqual(
        above.position.y + nodeHeight(above, commented),
      );
    }
  });

  it("leaves room below a card for comments added after layout", async () => {
    const snapshot = chain();
    snapshot.edges = [];
    const { nodes, edges } = toFlow(snapshot);

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
    const column = [...placed].sort((a, b) => a.position.y - b.position.y);
    for (let i = 1; i < column.length; i += 1) {
      const above = column[i - 1]!;
      expect(column[i]!.position.y).toBeGreaterThanOrEqual(
        above.position.y + nodeHeight(above, full),
      );
    }
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
