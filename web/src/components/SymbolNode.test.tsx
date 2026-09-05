/**
 * @vitest-environment jsdom
 *
 * The lib tests run in node; only the component tests need a DOM, so each of
 * them opts in here rather than the config switching the whole suite over.
 */

import { ReactFlowProvider } from "@xyflow/react";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
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
function draw(data: SymbolNodeData, onWrapperClick?: () => void) {
  // The component only ever reads `data`; the rest of `NodeProps` is React
  // Flow's business and never reaches the markup, so the cast stands in for
  // two dozen fields no assertion here would look at.
  const props = { id: data.snapshot.id, data } as unknown as ComponentProps<
    typeof SymbolNode
  >;
  render(
    <ReactFlowProvider>
      {/* React Flow's own wrapper is where `onNodeClick` is hung; this div
          stands in for it, so a click that escapes the card reaches the spy
          exactly as it would reach the canvas. */}
      <div onClick={onWrapperClick}>
        <SymbolNode {...props} />
      </div>
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

  it("a click inside the expanded diff never reaches the canvas", () => {
    // Otherwise selecting a line of the diff, or reaching for its scrollbar,
    // toggles the selection off and closes the card being read.
    const clicked = vi.fn();
    draw({ snapshot: card(), added: 1, removed: 0, expanded: true }, clicked);

    fireEvent.click(screen.getByText("let x = 1;"));

    expect(clicked).not.toHaveBeenCalled();
  });

  it("a click on the collapsed row does reach the canvas", () => {
    const clicked = vi.fn();
    draw({ snapshot: card(), added: 1, removed: 0 }, clicked);

    fireEvent.click(screen.getByText("alpha"));

    expect(clicked).toHaveBeenCalledTimes(1);
  });

  it("Enter and Space on the collapsed row reach the canvas as a click", () => {
    // The only keyboard path to a diff. React Flow's own Enter/Space handler
    // is gated on `isSelectable`, which every node is emitted without, so
    // without this the card is mouse-only.
    for (const key of ["Enter", " "]) {
      const clicked = vi.fn();
      draw({ snapshot: card(), added: 1, removed: 0 }, clicked);

      fireEvent.keyDown(screen.getByRole("button"), { key });

      expect(clicked, `${key} did not reach the canvas`).toHaveBeenCalledTimes(
        1,
      );
      cleanup();
    }
  });

  it("leaves other keys alone, so Escape still reaches the window", () => {
    const clicked = vi.fn();
    draw({ snapshot: card(), added: 1, removed: 0 }, clicked);

    fireEvent.keyDown(screen.getByRole("button"), { key: "Escape" });

    expect(clicked).not.toHaveBeenCalled();
  });

  it("the collapsed row is a focusable button reporting its expanded state", () => {
    draw({ snapshot: card(), added: 1, removed: 0 });
    const row = screen.getByRole("button");
    expect(row.tabIndex).toBe(0);
    expect(row.getAttribute("aria-expanded")).toBe("false");

    cleanup();

    draw({ snapshot: card(), added: 1, removed: 0, expanded: true });
    expect(screen.getByRole("button").getAttribute("aria-expanded")).toBe(
      "true",
    );
  });

  it("badges an added and a deleted card by their change kind", () => {
    draw({ snapshot: card({ change: "added" }), added: 4, removed: 0 });
    expect(document.querySelector(".badge-added")?.textContent).toBe("added");

    cleanup();

    draw({ snapshot: card({ change: "deleted" }), added: 0, removed: 4 });
    expect(document.querySelector(".badge-deleted")?.textContent).toBe(
      "deleted",
    );
  });

  it("says so rather than drawing an empty panel for a card with no diff", () => {
    draw({ snapshot: card({ diff: [] }), added: 0, removed: 0, expanded: true });

    expect(screen.getByText("no diff lines")).toBeDefined();
    expect(document.querySelector(".diff")).toBeNull();
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

  it("a file-level card names the basename, not the path its container already shows", () => {
    draw({
      snapshot: card({
        id: "src/core/lang/tsjs.rs::<file>",
        file: "src/core/lang/tsjs.rs",
        name: "src/core/lang/tsjs.rs",
        kind: "file",
      }),
      added: 0,
      removed: 0,
    });

    const name = document.querySelector(".symbol-name");
    expect(name?.textContent).toBe("tsjs.rs");
    // The whole path stays reachable — the card is the only place it is
    // pinned to the node rather than to the container.
    expect(name?.getAttribute("title")).toBe("src/core/lang/tsjs.rs");
  });

  it("a symbol card still shows its qualified name in full", () => {
    draw({
      snapshot: card({ name: "Repo::changed_files" }),
      added: 1,
      removed: 0,
    });

    expect(document.querySelector(".symbol-name")?.textContent).toBe(
      "Repo::changed_files",
    );
  });
});
