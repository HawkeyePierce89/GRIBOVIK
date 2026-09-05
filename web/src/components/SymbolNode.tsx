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

export function SymbolNode({ data }: NodeProps<SymbolFlowNode>) {
  const node = data.snapshot;

  // A file-level node is a catch-all for hunks outside every symbol, so its
  // badge names that rather than repeating the change kind.
  const badge = node.kind === "file" ? "file" : node.change;

  return (
    <div className="symbol-node">
      <Handle type="target" position={Position.Left} isConnectable={false} />

      <div className="symbol-row">
        {/* The path is the container header's job now; a card only has to say
            which symbol it is, and `title` keeps the full name reachable when
            the ellipsis takes it. */}
        <span className="symbol-name" title={node.name}>
          {node.name}
        </span>
        {node.kind !== "file" && <span className="symbol-kind">{node.kind}</span>}
        <span className="symbol-counts">
          <span className="count-added">+{data.added}</span>{" "}
          <span className="count-removed">−{data.removed}</span>
        </span>
        <span className={`badge badge-${badge}`}>{badge}</span>
      </div>

      {data.expanded === true && (
        <div className="symbol-expanded nowheel nodrag">
          <DiffView diff={node.diff} />
        </div>
      )}

      <Handle type="source" position={Position.Right} isConnectable={false} />
    </div>
  );
}
