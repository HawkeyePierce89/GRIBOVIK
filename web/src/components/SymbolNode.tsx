/**
 * One reviewable card: what changed, and the reviewer's verdict on it.
 */

import { Handle, Position, type NodeProps } from "@xyflow/react";
import { useState } from "react";

import { useReview } from "../hooks/useReviewState";
import { reviewFor } from "../lib/review";
import type { SymbolFlowNode } from "../lib/transform";
import type { Status } from "../types/snapshot";
import { DiffView } from "./DiffView";

const STATUSES: Status[] = ["approved", "rejected", "pending"];

const LABEL: Record<Status, string> = {
  approved: "Approve",
  rejected: "Reject",
  pending: "Pending",
};

export function SymbolNode({ data }: NodeProps<SymbolFlowNode>) {
  const node = data.snapshot;
  const { state, setStatus, addComment, highlighted } = useReview();
  const review = reviewFor(state, node.id);
  const [draft, setDraft] = useState("");

  // A file-level node is a catch-all for hunks outside every symbol, so its
  // badge names that rather than repeating the change kind.
  const badge = node.kind === "file" ? "file" : node.change;

  const classes = [
    "symbol-node",
    `status-${review.status}`,
    highlighted.has(node.id) ? "highlighted" : "",
  ]
    .filter(Boolean)
    .join(" ");

  function submitComment(event: React.FormEvent) {
    event.preventDefault();
    addComment(node.id, draft);
    setDraft("");
  }

  return (
    <div className={classes}>
      <Handle type="target" position={Position.Left} />

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

      <div className="status-buttons nodrag">
        {STATUSES.map((status) => (
          <button
            key={status}
            type="button"
            className={review.status === status ? "active" : ""}
            aria-pressed={review.status === status}
            onClick={() => setStatus(node.id, status)}
          >
            {LABEL[status]}
          </button>
        ))}
      </div>

      {review.comments.length > 0 && (
        <ul className="comments nowheel nodrag">
          {review.comments.map((comment, index) => (
            // Two comments can share a timestamp and a body; only position is
            // guaranteed unique, and comments are append-only.
            <li key={index}>
              <time dateTime={comment.created_at}>{comment.created_at}</time>
              <span>{comment.text}</span>
            </li>
          ))}
        </ul>
      )}

      <form className="comment-form nodrag" onSubmit={submitComment}>
        <input
          type="text"
          value={draft}
          placeholder="Add a comment"
          aria-label={`Add a comment on ${node.name}`}
          onChange={(event) => setDraft(event.target.value)}
        />
        <button type="submit">Add</button>
      </form>

      <Handle type="source" position={Position.Right} />
    </div>
  );
}
