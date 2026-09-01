/**
 * A node's slice of the file diff.
 *
 * Deliberately unhighlighted: the point of a card is to read a handful of
 * changed lines in context, and a syntax highlighter across three languages
 * would cost more than it adds at this size.
 */

import type { DiffLine } from "../types/snapshot";

const SIGN: Record<DiffLine["tag"], string> = {
  add: "+",
  del: "-",
  context: " ",
};

/** Right-aligned gutter cell; a blank means the line is absent on that side. */
function gutter(line: number | null): string {
  return line === null ? "" : String(line);
}

export function DiffView({ diff }: { diff: DiffLine[] }) {
  if (diff.length === 0) {
    return <p className="diff-empty">no diff lines</p>;
  }

  return (
    <div className="diff">
      {diff.map((line, index) => (
        <div
          // Line numbers are not unique across a diff (a rewrite repeats them
          // on both sides), so the index is the only stable key here.
          key={index}
          className={`diff-line diff-${line.tag}`}
        >
          <span className="diff-gutter">{gutter(line.old_line)}</span>
          <span className="diff-gutter">{gutter(line.new_line)}</span>
          <span className="diff-sign">{SIGN[line.tag]}</span>
          <span className="diff-text">{line.text}</span>
        </div>
      ))}
    </div>
  );
}
