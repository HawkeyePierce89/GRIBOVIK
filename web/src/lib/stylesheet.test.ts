/**
 * The constants `layout.ts` computes with are the stylesheet's numbers copied
 * by hand — layout runs before a single card exists to measure, so there is
 * nothing to read them off. Nothing else notices when the two drift: the unit
 * tests never load CSS and the browser never loads the constants, so a card
 * that grew by four pixels would simply overlap the one elk placed below it,
 * and a taller header would hide the top card behind the file path. This is
 * the only place the two halves are compared.
 */

import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

import { DIMMED, FOCUSED } from "./focus";
import { CARD_HEIGHT, HEADER_HEIGHT, NODE_WIDTH } from "./layout";

// Read off disk rather than imported: Vitest stubs every CSS import to an
// empty string, `?raw` included, which would make the comparison below pass
// against nothing at all.
const css = readFileSync(new URL("../styles.css", import.meta.url), "utf8");

/** The value of `property` inside the first `selector { … }` block. */
function declaration(selector: string, property: string): string {
  const block = new RegExp(
    // `replaceAll`: a string pattern to `replace` takes the first match only,
    // so a compound selector would keep unescaped dots that match any
    // character — and the `[^}]*` below would then assert against whatever
    // block that happened to hit.
    `${selector.replaceAll(".", "\\.")}\\s*\\{([^}]*)\\}`,
  ).exec(css);
  expect(block, `no ${selector} block in styles.css`).not.toBeNull();
  const value = new RegExp(`(?:^|;)\\s*${property}\\s*:\\s*([^;]+);`, "m").exec(
    block![1]!,
  );
  expect(value, `no ${property} on ${selector}`).not.toBeNull();
  return value![1]!.trim();
}

describe("layout constants against styles.css", () => {
  it("`.symbol-node` is CARD_HEIGHT tall and NODE_WIDTH wide", () => {
    expect(declaration(".symbol-node", "height")).toBe(`${CARD_HEIGHT}px`);
    expect(declaration(".symbol-node", "width")).toBe(`${NODE_WIDTH}px`);
  });

  it("`.file-header` is HEADER_HEIGHT tall, which elk reserves as padding", () => {
    expect(declaration(".file-header", "height")).toBe(`${HEADER_HEIGHT}px`);
  });
});

describe("the card's internals against its fixed box", () => {
  // `CARD_HEIGHT` is a constant elk laid the graph out with, so the card's box
  // cannot grow to fit its contents — and `rem` resolves against the *root*
  // font size, which the reviewer's browser sets, not the `14px` on `body`.
  // A single `rem` inside the card is enough to push the row past the border
  // at a 20px root, and neither the layout tests nor the component tests load
  // CSS, so nothing else would notice.
  for (const [selector, property] of [
    [".symbol-node", "padding"],
    [".symbol-row", "gap"],
    [".symbol-name", "font-size"],
    [".symbol-kind", "font-size"],
    [".symbol-counts", "font-size"],
    [".badge", "font-size"],
    [".badge", "padding"],
  ] as const) {
    it(`\`${selector}\` sizes \`${property}\` in px, not rem`, () => {
      expect(declaration(selector, property)).not.toMatch(/rem\b/);
    });
  }
});

describe("focus classes against styles.css", () => {
  // `focus.ts` puts these two strings on nodes and edges and the stylesheet is
  // the only thing that acts on them. Renaming either half alone leaves every
  // unit test green and the canvas with no dimming and no highlight at all —
  // a feature silently gone, with nothing to fail.
  it("styles the class `focus.ts` dims with", () => {
    expect(css).toContain(`.react-flow__node.${DIMMED}`);
    expect(css).toContain(`.react-flow__edge.${DIMMED}`);
  });

  it("styles the class `focus.ts` highlights an edge with", () => {
    expect(css).toContain(`.react-flow__edge.${FOCUSED}`);
  });
});

describe("the expanded overlay's load-bearing declarations", () => {
  // Both are decisions CLAUDE.md calls load-bearing, and both fail silently:
  // `position: static` puts the diff back in the flow, so the card grows and
  // the layout elk computed no longer matches; dropping `user-select` hands
  // the box back React Flow's `none` and the diff stops being copyable.
  it("`.symbol-expanded` is positioned out of the flow", () => {
    expect(declaration(".symbol-expanded", "position")).toBe("absolute");
  });

  it("`.symbol-expanded` takes text selection back from React Flow", () => {
    expect(declaration(".symbol-expanded", "user-select")).toBe("text");
  });
});
