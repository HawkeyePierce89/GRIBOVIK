/**
 * The container one file's cards sit in: a box with a header naming the file.
 *
 * It carries no handles and takes part in no edge — edges connect cards, and a
 * container that could be connected would offer the reviewer a call graph
 * between files that the analysis never computed. Its size is whatever
 * `layout.ts` wrote onto the node, so the box is exactly the one elk packed
 * the cards into.
 */

import type { NodeProps } from "@xyflow/react";

import type { FileFlowNode } from "../lib/transform";

export function FileNode({ data }: NodeProps<FileFlowNode>) {
  return (
    <div className="file-node">
      <header className="file-header">
        <span className="file-path" title={data.file}>
          {data.file}
        </span>
        <span className="file-count">
          {data.cardCount} card{data.cardCount === 1 ? "" : "s"}
        </span>
        <span className="file-counts">
          <span className="count-added">+{data.added}</span>{" "}
          <span className="count-removed">−{data.removed}</span>
        </span>
      </header>
    </div>
  );
}
