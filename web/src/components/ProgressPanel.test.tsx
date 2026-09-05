/**
 * @vitest-environment jsdom
 *
 * The lib tests run in node; only the two component tests need a DOM, so they
 * opt in here rather than the config switching the whole suite over.
 */

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { FileSummary } from "../lib/transform";
import { ProgressPanel } from "./ProgressPanel";

afterEach(cleanup);

const FILES: FileSummary[] = [
  {
    file: "src/z.rs",
    containerId: "file:src/z.rs",
    cardCount: 1,
    added: 4,
    removed: 0,
  },
  {
    file: "src/a.rs",
    containerId: "file:src/a.rs",
    cardCount: 3,
    added: 7,
    removed: 2,
  },
];

describe("ProgressPanel", () => {
  it("shows the total card count", () => {
    render(
      <ProgressPanel cardCount={4} files={FILES} onSelectFile={() => {}} />,
    );

    expect(screen.getByText("4 cards to review")).toBeDefined();
  });

  it("singularises the total for a one-card review", () => {
    render(
      <ProgressPanel
        cardCount={1}
        files={FILES.slice(0, 1)}
        onSelectFile={() => {}}
      />,
    );

    expect(screen.getByText("1 card to review")).toBeDefined();
  });

  it("renders one row per file with its counts, sorted by path", () => {
    render(
      <ProgressPanel cardCount={4} files={FILES} onSelectFile={() => {}} />,
    );

    const rows = screen
      .getAllByRole("button")
      .map((row) => row.textContent ?? "");
    // The snapshot's order is git's; the panel's is alphabetical.
    expect(rows).toHaveLength(2);
    expect(rows[0]).toContain("src/a.rs");
    expect(rows[0]).toContain("3");
    expect(rows[0]).toContain("+7");
    expect(rows[0]).toContain("−2");
    expect(rows[1]).toContain("src/z.rs");
    expect(rows[1]).toContain("+4");
    expect(rows[1]).toContain("−0");
  });

  it("clicking a row asks for that file's container", () => {
    const onSelectFile = vi.fn();
    render(
      <ProgressPanel cardCount={4} files={FILES} onSelectFile={onSelectFile} />,
    );

    screen.getByText("src/z.rs").click();

    expect(onSelectFile).toHaveBeenCalledTimes(1);
    // The container id, not the path: it is what `fitView` addresses.
    expect(onSelectFile).toHaveBeenCalledWith("file:src/z.rs");
  });
});
