/**
 * @vitest-environment jsdom
 *
 * The interaction model, which lives nowhere else.
 *
 * `transform`, `layout` and `focus` are pure and tested directly; what ties
 * them into a reviewable canvas — which click expands, which four gestures
 * dismiss, and what each of them has to clear — exists only in `App`. Every
 * bug found in it so far has been a dismissal path that cleared one of the two
 * ids and not the other, which no test of a pure module can see.
 *
 * `loadSnapshot` and `layout` are stubbed because neither belongs to what is
 * under test: one is a `fetch`, the other an elk worker that jsdom has no
 * business starting. Everything below them — `toFlow`, `applyFocus`, React
 * Flow itself — is the real thing.
 */

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { GraphFlowNode } from "./lib/transform";
import type { GraphSnapshot } from "./types/snapshot";

const loadSnapshot = vi.fn<() => Promise<GraphSnapshot>>();
const fitView = vi.fn<(options?: { nodes?: { id: string }[] }) => Promise<boolean>>(
  () => Promise.resolve(true),
);

vi.mock("./lib/snapshot", () => ({
  loadSnapshot: () => loadSnapshot(),
}));

// The grid fallback, not elk: it is synchronous, deterministic, and places
// cards in the same container-relative coordinates the real layout does.
vi.mock("./lib/layout", async (importOriginal) => {
  const real = await importOriginal<typeof import("./lib/layout")>();
  return {
    ...real,
    layout: (nodes: GraphFlowNode[]) => Promise.resolve(real.gridLayout(nodes)),
  };
});

vi.mock("@xyflow/react", async (importOriginal) => {
  const real = await importOriginal<typeof import("@xyflow/react")>();
  return { ...real, useReactFlow: () => ({ ...real.useReactFlow(), fitView }) };
});

import { App } from "./App";

/**
 * React Flow measures its nodes and its pane through APIs jsdom does not
 * implement. Stubbing them is what lets the canvas mount at all; nothing below
 * asserts on a measurement, only on which nodes and classes are rendered.
 */
class ResizeObserverStub {
  observe() {}
  disconnect() {}
  unobserve() {}
}
Object.defineProperty(window, "ResizeObserver", {
  writable: true,
  value: ResizeObserverStub,
});
Object.defineProperty(window, "DOMMatrixReadOnly", {
  writable: true,
  value: class {
    m22 = 1;
    constructor(_transform?: string) {}
  },
});
Object.defineProperty(HTMLElement.prototype, "offsetWidth", {
  configurable: true,
  value: 800,
});
Object.defineProperty(HTMLElement.prototype, "offsetHeight", {
  configurable: true,
  value: 600,
});
Object.defineProperty(SVGElement.prototype, "getBBox", {
  configurable: true,
  value: () => ({ x: 0, y: 0, width: 0, height: 0 }),
});

beforeEach(() => {
  loadSnapshot.mockReset();
  fitView.mockClear();
  loadSnapshot.mockResolvedValue(snapshot());
});

afterEach(cleanup);

function snapshot(): GraphSnapshot {
  const card = (file: string, name: string) => ({
    id: `${file}::${name}`,
    file,
    name,
    kind: "function",
    change: "modified" as const,
    diff: [
      {
        tag: "add" as const,
        old_line: null,
        new_line: 1,
        text: `body of ${name}`,
      },
    ],
  });
  return {
    meta: {
      repo: "/tmp/repo",
      base: "aaa",
      head: "bbb",
      files_changed: 2,
      warnings: [],
    },
    nodes: [
      card("src/a.rs", "caller"),
      card("src/a.rs", "callee"),
      card("src/b.rs", "stranger"),
    ],
    edges: [
      {
        from: "src/a.rs::caller",
        to: "src/a.rs::callee",
        confidence: "certain" as const,
      },
    ],
  };
}

/** Render and wait for the snapshot fetch and the layout to settle. */
async function mount() {
  render(<App />);
  expect(await screen.findByText("caller")).toBeDefined();
}

/** The React Flow node wrapper for a card, which is what carries `dimmed`. */
function nodeEl(id: string): HTMLElement {
  const el = document.querySelector<HTMLElement>(`[data-id="${id}"]`);
  expect(el, `no node rendered for ${id}`).not.toBeNull();
  return el!;
}

/** The card's own control — a `role="button"` row inside the wrapper. */
function row(id: string): HTMLElement {
  const el = nodeEl(id).querySelector<HTMLElement>(".symbol-row");
  expect(el, `no row inside ${id}`).not.toBeNull();
  return el!;
}

function isDimmed(id: string): boolean {
  return nodeEl(id).classList.contains("dimmed");
}

const CALLER = "src/a.rs::caller";
const CALLEE = "src/a.rs::callee";
const STRANGER = "src/b.rs::stranger";

describe("App", () => {
  it("draws a container per file and every card inside it", async () => {
    await mount();

    // The header on the container, not the row in the panel — both name the
    // file, and only one of them is on the canvas.
    expect(screen.getByText("src/a.rs", { selector: ".file-path" })).toBeDefined();
    expect(screen.getByText("src/b.rs", { selector: ".file-path" })).toBeDefined();
    expect(nodeEl(CALLER).parentElement).toBe(nodeEl(STRANGER).parentElement);
    expect(nodeEl("file:src/a.rs")).toBeDefined();
    expect(nodeEl("file:src/b.rs")).toBeDefined();
  });

  it("clicking a card opens its diff and dims outside its neighbourhood", async () => {
    await mount();
    expect(screen.queryByText("body of caller")).toBeNull();

    fireEvent.click(row(CALLER));

    expect(screen.getByText("body of caller")).toBeDefined();
    // The callee is one hop out, so it stays lit; the other file does not.
    expect(isDimmed(CALLEE)).toBe(false);
    expect(isDimmed(STRANGER)).toBe(true);
  });

  it("a container never dims, so the reviewer keeps the map", async () => {
    await mount();

    fireEvent.click(row(CALLER));

    expect(nodeEl("file:src/b.rs").classList.contains("dimmed")).toBe(false);
  });

  it("clicking the open card again closes it and undims", async () => {
    await mount();
    fireEvent.click(row(CALLER));
    // The pointer is resting on the card it just closed: dropping only the
    // selection would collapse the diff and leave the graph dimmed around it.
    fireEvent.mouseEnter(nodeEl(CALLER));

    fireEvent.click(row(CALLER));

    expect(screen.queryByText("body of caller")).toBeNull();
    expect(isDimmed(STRANGER)).toBe(false);
  });

  it("Escape closes the diff and undims, hover included", async () => {
    await mount();
    fireEvent.click(row(CALLER));
    fireEvent.mouseEnter(nodeEl(CALLER));

    fireEvent.keyDown(window, { key: "Escape" });

    expect(screen.queryByText("body of caller")).toBeNull();
    expect(isDimmed(STRANGER)).toBe(false);
  });

  it("clicking a container closes the diff and undims, hover included", async () => {
    await mount();
    fireEvent.click(row(CALLER));
    fireEvent.mouseEnter(nodeEl(CALLER));

    fireEvent.click(nodeEl("file:src/b.rs"));

    expect(screen.queryByText("body of caller")).toBeNull();
    expect(isDimmed(STRANGER)).toBe(false);
  });

  it("hovering a card previews the dimming without opening it", async () => {
    await mount();

    fireEvent.mouseEnter(nodeEl(CALLER));

    expect(isDimmed(STRANGER)).toBe(true);
    expect(screen.queryByText("body of caller")).toBeNull();
  });

  it("hovering another card cannot take the reviewer's open diff away", async () => {
    await mount();
    fireEvent.click(row(CALLER));

    fireEvent.mouseEnter(nodeEl(STRANGER));

    // Selection wins over hover, or moving the pointer across the canvas
    // would move the highlight off whatever is being read.
    expect(screen.getByText("body of caller")).toBeDefined();
    expect(isDimmed(STRANGER)).toBe(true);
  });

  it("hovering a container dims nothing — a file has no neighbourhood", async () => {
    await mount();

    fireEvent.mouseEnter(nodeEl("file:src/a.rs"));

    expect(isDimmed(STRANGER)).toBe(false);
  });

  it("picking a file zooms to it and drops any reading position first", async () => {
    await mount();
    fireEvent.click(row(CALLER));

    fireEvent.click(screen.getByText("src/b.rs", { selector: "bdi" }));

    // Without the clear, the reviewer arrives at the file they asked for with
    // every card in it dimmed by a selection made somewhere else.
    expect(screen.queryByText("body of caller")).toBeNull();
    expect(isDimmed(STRANGER)).toBe(false);
    expect(fitView).toHaveBeenCalledTimes(1);
    expect(fitView.mock.calls[0]![0]).toMatchObject({
      nodes: [{ id: "file:src/b.rs" }],
    });
  });

  // Deliberately not tested here: `onlyRenderVisibleElements` standing down
  // while a card is open. React Flow culls by a node's *measured* box, jsdom
  // measures nothing, and the stubs above hand it one fixed size — so a test
  // of culling would assert against the stub rather than the canvas.

  it("keeps the card's own box collapsed so the layout never re-runs", async () => {
    await mount();

    fireEvent.click(row(CALLER));

    // The diff is drawn in an overlay *inside* the node, not appended after
    // it: growing the node's box is what would make elk's placement wrong.
    const overlay = nodeEl(CALLER).querySelector(".symbol-expanded");
    expect(overlay).not.toBeNull();
    expect(overlay!.closest(".symbol-node")).not.toBeNull();
  });

  it("surfaces the snapshot's warnings", async () => {
    const withWarning = snapshot();
    withWarning.meta.warnings = ["src/x.rs: not utf-8"];
    loadSnapshot.mockResolvedValue(withWarning);

    await mount();

    expect(screen.getByText("src/x.rs: not utf-8")).toBeDefined();
    expect(screen.getByText("1 warning")).toBeDefined();
  });

  it("reports a failed load instead of hanging on Loading…", async () => {
    loadSnapshot.mockRejectedValue(new Error("GET /api/graph failed: 500"));

    render(<App />);

    expect(await screen.findByText(/Could not load the review/)).toBeDefined();
  });
});
