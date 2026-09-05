/**
 * @vitest-environment jsdom
 */

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { ComponentProps } from "react";

import type { FileNodeData } from "../lib/transform";
import { FileNode } from "./FileNode";

afterEach(cleanup);

function draw(data: FileNodeData) {
  const props = { id: `file:${data.file}`, data } as unknown as ComponentProps<
    typeof FileNode
  >;
  // No `Handle`, so unlike a card this needs no React Flow store to render.
  render(<FileNode {...props} />);
}

describe("FileNode", () => {
  it("heads the container with the path, card count and counts", () => {
    draw({ file: "src/core/diff.rs", cardCount: 4, added: 12, removed: 5 });

    expect(screen.getByText("src/core/diff.rs")).toBeDefined();
    expect(screen.getByText("4 cards")).toBeDefined();
    expect(screen.getByText("+12")).toBeDefined();
    expect(screen.getByText("−5")).toBeDefined();
  });

  it("says `1 card` for a file with one", () => {
    draw({ file: "src/lib.rs", cardCount: 1, added: 1, removed: 0 });

    expect(screen.getByText("1 card")).toBeDefined();
  });

  it("draws no handles — edges connect cards, never files", () => {
    draw({ file: "src/lib.rs", cardCount: 1, added: 0, removed: 0 });

    expect(document.querySelector(".react-flow__handle")).toBeNull();
  });
});
