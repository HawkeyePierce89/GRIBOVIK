/**
 * The GraphSnapshot wire contract.
 *
 * This module is one of exactly two places the contract is defined; the other
 * is `src/core/snapshot.rs`. Field names are snake_case on the wire and must
 * stay identical on both sides.
 */

/** A complete analysis result: what changed, and how it calls itself. */
export interface GraphSnapshot {
  meta: Meta;
  nodes: SnapshotNode[];
  edges: SnapshotEdge[];
}

/** Context about the analyzed revision range. */
export interface Meta {
  /** Absolute path to the repository root. */
  repo: string;
  base: string;
  head: string;
  /** Number of files that contributed to the graph. */
  files_changed: number;
  /** Non-fatal problems worth surfacing to the reviewer. */
  warnings: string[];
}

/** One reviewable card: a changed symbol, or a file-level catch-all. */
export interface SnapshotNode {
  /** `"<file>::<qualified_name>"`, unique within a snapshot. */
  id: string;
  file: string;
  name: string;
  /**
   * Language-specific symbol kind (`"function"`, `"method"`, `"struct"`, …)
   * or `"file"` for the synthetic file-level node.
   */
  kind: string;
  change: ChangeKind;
  /** The lines of the overall file diff that belong to this node. */
  diff: DiffLine[];
}

/** A single line of a unified diff, carrying its position on both sides. */
export interface DiffLine {
  tag: DiffTag;
  /** 1-based line number in the old revision; `null` for added lines. */
  old_line: number | null;
  /** 1-based line number in the new revision; `null` for deleted lines. */
  new_line: number | null;
  /** Line content without its trailing newline. */
  text: string;
}

/** A resolved call from one changed symbol to another. */
export interface SnapshotEdge {
  /** Node id of the caller. */
  from: string;
  /** Node id of the callee. */
  to: string;
  confidence: Confidence;
}

/** How a symbol changed between base and head. */
export type ChangeKind = "added" | "modified" | "deleted";

/** How sure the resolver is that an edge points at the right callee. */
export type Confidence = "certain" | "ambiguous";

/** Which side of the diff a line belongs to. */
export type DiffTag = "add" | "del" | "context";

/**
 * Node id → the reviewer's verdict. Mirrors `ReviewState` in `src/review.rs`;
 * a node with no entry counts as `pending`.
 */
export type ReviewState = Record<string, NodeReview>;

/** Everything the reviewer recorded about one node. */
export interface NodeReview {
  status: Status;
  comments: Comment[];
  /**
   * Digest of the diff the verdict was recorded against, owned by the server:
   * it stamps every entry on `POST /api/state` and drops the statuses whose
   * digest no longer matches when the next run loads the file. The client
   * carries the field back and forth without reading it, so the two sides
   * never have to agree on a hash — which is why it is optional here and
   * absent on an entry the client has just created.
   */
  fingerprint?: string;
}

/** Where a node stands in the review. */
export type Status = "approved" | "rejected" | "pending";

/** One free-text note attached to a node. */
export interface Comment {
  text: string;
  /** ISO-8601, stamped client-side; the backend treats it as opaque. */
  created_at: string;
}
