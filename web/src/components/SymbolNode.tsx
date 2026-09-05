/**
 * One reviewable card: a changed symbol, collapsed to a single row.
 *
 * The collapsed row is the whole node as far as the layout is concerned — its
 * height is `layout.ts`'s `CARD_HEIGHT`, a constant. Expanding draws the diff
 * in an overlay anchored under that row rather than growing the box, so
 * selecting a card never moves its neighbours and elk never runs twice.
 */

import { Handle, Position, type NodeProps } from "@xyflow/react";

import type { SymbolFlowNode } from "../lib/transform";
import { DiffView } from "./DiffView";

/** Last segment of a git path, which uses `/` on every platform. */
function basename(path: string): string {
  return path.slice(path.lastIndexOf("/") + 1);
}

export function SymbolNode({ data }: NodeProps<SymbolFlowNode>) {
  const node = data.snapshot;

  // A file-level node is a catch-all for hunks outside every symbol, so its
  // badge names that rather than repeating the change kind.
  const badge = node.kind === "file" ? "file" : node.change;

  // A symbol card is named by its symbol, but the file-level node's `name`
  // *is* the file path — which now sits in the container header directly
  // above it. Showing it again would print the path twice, and print it
  // through an ellipsis that eats the tail, the end of a path that identifies
  // it. The basename says which file without repeating the header, and
  // `title` still carries the whole path.
  const label = node.kind === "file" ? basename(node.name) : node.name;

  return (
    <div className="symbol-node">
      <Handle type="target" position={Position.Left} isConnectable={false} />

      {/* The collapsed row, not the whole card, is the control: the expanded
          overlay below holds selectable diff text, which has no business
          inside a `button`. `click()` rather than a callback threaded through
          `data` — the click bubbles to the wrapper React Flow hangs
          `onNodeClick` off, so Enter and Space reach the canvas by exactly the
          path a pointer does, and this component stays a pure function of
          `data`. React Flow's own Enter/Space handler is dead here: it is
          gated on `isSelectable`, and every node is emitted `selectable:
          false`. */}
      <div
        className="symbol-row"
        role="button"
        tabIndex={0}
        aria-expanded={data.expanded === true}
        onKeyDown={(event) => {
          if (event.key !== "Enter" && event.key !== " ") return;
          // Space scrolls the page otherwise, and the canvas is the page.
          event.preventDefault();
          // A held Enter autorepeats, and each repeat toggles the selection —
          // which toggles `onlyRenderVisibleElements` and so mounts and
          // unmounts the whole graph, ~114 ms a time on the MVP snapshot.
          // Leaning on the key would freeze the canvas rather than open a
          // card.
          if (event.repeat) return;
          event.currentTarget.click();
        }}
      >
        {/* The path is the container header's job now; a card only has to say
            which symbol it is, and `title` keeps the full name reachable when
            the ellipsis takes it. */}
        <span className="symbol-name" title={node.name}>
          {label}
        </span>
        {node.kind !== "file" && <span className="symbol-kind">{node.kind}</span>}
        <span className="symbol-counts">
          <span className="count-added">+{data.added}</span>{" "}
          <span className="count-removed">−{data.removed}</span>
        </span>
        <span className={`badge badge-${badge}`}>{badge}</span>
      </div>

      {data.expanded === true && (
        // React Flow hangs `onNodeClick` off the wrapper this sits inside, and
        // `nowheel`/`nodrag` opt out of the wheel and the drag but not the
        // click. Without the stop, selecting a line of the diff — or reaching
        // for its scrollbar — bubbles up and collapses the card being read.
        // Selecting text is only possible because the stylesheet gives this
        // box `user-select: text` back; both halves are needed for a copy.
        <div
          className="symbol-expanded nowheel nodrag"
          onClick={(event) => {
            event.stopPropagation();
          }}
        >
          <DiffView diff={node.diff} />
        </div>
      )}

      <Handle type="source" position={Position.Right} isConnectable={false} />
    </div>
  );
}
