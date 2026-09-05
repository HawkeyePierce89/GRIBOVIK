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

import { CARD_HEIGHT, HEADER_HEIGHT, NODE_WIDTH } from "./layout";

// Read off disk rather than imported: Vitest stubs every CSS import to an
// empty string, `?raw` included, which would make the comparison below pass
// against nothing at all.
const css = readFileSync(new URL("../styles.css", import.meta.url), "utf8");

/** The value of `property` inside the first `selector { … }` block. */
function declaration(selector: string, property: string): string {
  const block = new RegExp(
    `${selector.replace(".", "\\.")}\\s*\\{([^}]*)\\}`,
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
