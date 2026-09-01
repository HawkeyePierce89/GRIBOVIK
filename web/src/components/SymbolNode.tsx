/**
 * One reviewable card: a changed symbol and its slice of the diff.
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

      <header className="symbol-header">
        <span className="symbol-file" title={node.file}>
          {node.file}
        </span>
        <span className={`badge badge-${badge}`}>{badge}</span>
      </header>

      <h2 className="symbol-name">
        {node.name}
        {node.kind !== "file" && <span className="symbol-kind">{node.kind}</span>}
      </h2>

      <div className="nowheel nodrag">
        <DiffView diff={node.diff} />
      </div>

      <Handle type="source" position={Position.Right} isConnectable={false} />
    </div>
  );
}
