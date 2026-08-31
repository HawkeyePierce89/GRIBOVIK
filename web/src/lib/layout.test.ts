import { describe, expect, it } from "vitest";

import type { GraphSnapshot } from "../types/snapshot";
import { DEFAULT_NODE_WIDTH, layout } from "./layout";
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
      caller!.position.x + DEFAULT_NODE_WIDTH,
    );
  });

  it("preserves input order and node data", async () => {
    const { nodes, edges } = toFlow(chain());

    const placed = await layout(nodes, edges);

    expect(placed.map((node) => node.id)).toEqual(nodes.map((node) => node.id));
    expect(placed[0]?.data).toEqual(nodes[0]?.data);
  });

  it("honours measured sizes over the defaults", async () => {
    const { nodes, edges } = toFlow(chain());

    const placed = await layout(nodes, edges, {
      "a::caller": { width: 900, height: 100 },
    });

    const caller = placed.find((node) => node.id === "a::caller");
    const callee = placed.find((node) => node.id === "a::callee");
    expect(callee!.position.x).toBeGreaterThanOrEqual(
      caller!.position.x + 900,
    );
  });

  it("returns nothing for an empty graph", async () => {
    expect(await layout([], [])).toEqual([]);
  });
});
