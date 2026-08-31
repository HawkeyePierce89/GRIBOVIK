/**
 * Pure review-state operations.
 *
 * The components and the `useReviewState` hook only wire these into React and
 * the network; every decision about what a mutation means lives here, where it
 * can be tested without a DOM.
 */

import type {
  Comment,
  NodeReview,
  ReviewState,
  Status,
} from "../types/snapshot";

/**
 * The verdict a node carries when the reviewer has not touched it.
 *
 * Frozen because `reviewFor` hands this exact object out for every untouched
 * node: mutating it in one place would change the default everywhere.
 */
export const PENDING: NodeReview = Object.freeze({
  status: "pending",
  comments: Object.freeze([]) as readonly Comment[] as Comment[],
});

/** How many nodes sit in each status; the three always sum to the node count. */
export type StatusCounts = {
  approved: number;
  rejected: number;
  pending: number;
};

/** The review recorded for `id`, or the pending default for an untouched node. */
export function reviewFor(state: ReviewState, id: string): NodeReview {
  return state[id] ?? PENDING;
}

/** The status of `id`; a node with no entry counts as `pending`. */
export function statusOf(state: ReviewState, id: string): Status {
  return reviewFor(state, id).status;
}

/**
 * Tally `nodeIds` by status. Only the ids passed in are counted, so state left
 * over from a node that vanished between runs cannot inflate the totals.
 */
export function countStatuses(
  state: ReviewState,
  nodeIds: readonly string[],
): StatusCounts {
  const counts: StatusCounts = { approved: 0, rejected: 0, pending: 0 };
  for (const id of nodeIds) counts[statusOf(state, id)] += 1;
  return counts;
}

/** The subset of `nodeIds` currently in `status`, in the order given. */
export function nodeIdsWithStatus(
  state: ReviewState,
  nodeIds: readonly string[],
  status: Status,
): string[] {
  return nodeIds.filter((id) => statusOf(state, id) === status);
}

/**
 * Return a copy of `state` with `id` set to `status`, preserving its comments.
 *
 * Returning to `pending` with nothing else recorded drops the entry entirely:
 * an absent entry already means pending, and keeping an empty one would make
 * the persisted file grow with every click that undoes another.
 */
export function setStatus(
  state: ReviewState,
  id: string,
  status: Status,
): ReviewState {
  const current = reviewFor(state, id);
  if (status === "pending" && current.comments.length === 0) {
    if (!(id in state)) return state;
    const next = { ...state };
    delete next[id];
    return next;
  }
  return { ...state, [id]: { ...current, status } };
}

/**
 * Return a copy of `state` with a comment appended to `id`. `created_at` is
 * supplied by the caller — the backend treats it as an opaque string, so the
 * clock stays on the client side and out of this function.
 *
 * Blank comments are ignored: the add-comment box submits on Enter, and an
 * empty note is a stray keystroke rather than a reviewer's intent.
 */
export function addComment(
  state: ReviewState,
  id: string,
  text: string,
  createdAt: string,
): ReviewState {
  const trimmed = text.trim();
  if (trimmed === "") return state;

  const current = reviewFor(state, id);
  const comment: Comment = { text: trimmed, created_at: createdAt };
  return {
    ...state,
    [id]: { ...current, comments: [...current.comments, comment] },
  };
}
