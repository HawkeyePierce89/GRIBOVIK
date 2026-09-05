/**
 * @vitest-environment jsdom
 *
 * The lib tests run in node; only the two component tests need a DOM, so they
 * opt in here rather than the config switching the whole suite over.
 */

import { ReactFlowProvider } from "@xyflow/react";
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { ComponentProps } from "react";

import type { SymbolNodeData } from "../lib/transform";
import type { DiffLine, SnapshotNode } from "../types/snapshot";
import { SymbolNode } from "./SymbolNode";

afterEach(cleanup);

function line(
  tag: DiffLine["tag"],
  text: string,
  old_line: number | null = null,
  new_line: number | null = null,
): DiffLine {
  return { tag, old_line, new_line, text };
}

function card(overrides: Partial<SnapshotNode> = {}): SnapshotNode {
  return {
    id: "src/a.rs::alpha",
    file: "src/a.rs",
    name: "alpha",
    kind: "function",
    change: "modified",
    diff: [line("add", "let x = 1;", null, 4)],
    ...overrides,
  };
}

/**
 * `Handle` reads React Flow's store, so a bare `render` of the node throws.
 * The provider is the smallest thing that supplies one.
 */
function draw(data: SymbolNodeData) {
  // The component only ever reads `data`; the rest of `NodeProps` is React
  // Flow's business and never reaches the markup, so the cast stands in for
  // two dozen fields no assertion here would look at.
  const props = { id: data.snapshot.id, data } as unknown as ComponentProps<
    typeof SymbolNode
  >;
  render(
    <ReactFlowProvider>
      <SymbolNode {...props} />
    </ReactFlowProvider>,
  );
}

describe("SymbolNode", () => {
  it("collapsed shows the name, kind, badge and counts but no diff", () => {
    draw({ snapshot: card(), added: 3, removed: 1 });

    expect(screen.getByText("alpha")).toBeDefined();
    expect(screen.getByText("function")).toBeDefined();
    expect(screen.getByText("modified")).toBeDefined();
    expect(screen.getByText("+3")).toBeDefined();
    expect(screen.getByText("−1")).toBeDefined();
    // The diff belongs to the expanded overlay; a collapsed card must not
    // carry hundreds of line elements the reviewer cannot see.
    expect(screen.queryByText("let x = 1;")).toBeNull();
    expect(document.querySelector(".symbol-expanded")).toBeNull();
  });

  it("the file path is not on the card — its container header carries it", () => {
    draw({ snapshot: card(), added: 0, removed: 0 });

    expect(screen.queryByText("src/a.rs")).toBeNull();
  });

  it("renders the diff in an overlay once expanded", () => {
    draw({ snapshot: card(), added: 3, removed: 1, expanded: true });

    expect(screen.getByText("let x = 1;")).toBeDefined();
    const overlay = document.querySelector(".symbol-expanded");
    expect(overlay).not.toBeNull();
    // The wheel and drag opt-outs are what let the diff scroll inside a canvas
    // that would otherwise zoom or pan under the pointer.
    expect(overlay?.className).toContain("nowheel");
    expect(overlay?.className).toContain("nodrag");
  });

  it("a file-level card is badged `file` and shows no kind", () => {
    draw({
      snapshot: card({ id: "src/a.rs::src/a.rs", name: "src/a.rs", kind: "file" }),
      added: 0,
      removed: 0,
    });

    expect(screen.getByText("file")).toBeDefined();
    expect(document.querySelector(".symbol-kind")).toBeNull();
  });
});
