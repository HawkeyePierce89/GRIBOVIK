/**
 * Review state, held whole and pushed whole.
 *
 * `POST /api/state` replaces the entire object, so there is no merge to get
 * wrong: the browser owns the state while the session runs and the server just
 * persists what it is handed.
 */

import {
  createContext,
  useCallback,
  useContext,
  useRef,
  useState,
} from "react";

import {
  addComment as addCommentTo,
  setStatus as setStatusOn,
} from "../lib/review";
import type { ReviewState, Status } from "../types/snapshot";

/** What `SymbolNode` needs in order to record a verdict. */
export type ReviewApi = {
  state: ReviewState;
  setStatus: (id: string, status: Status) => void;
  addComment: (id: string, text: string) => void;
  /** Set when the last POST failed, so the UI can say so instead of lying. */
  error: string | null;
};

async function persist(state: ReviewState): Promise<void> {
  const response = await fetch("/api/state", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(state),
  });
  if (!response.ok) {
    throw new Error(`POST /api/state failed: ${response.status}`);
  }
}

/**
 * Hold `initial` and push every mutation to the server.
 *
 * The local state is updated first and kept even if the POST fails — losing
 * the reviewer's click because the server hiccuped would be worse than a
 * warning banner they can act on.
 */
export function useReviewState(initial: ReviewState): ReviewApi {
  const [state, setState] = useState<ReviewState>(initial);
  const [error, setError] = useState<string | null>(null);
  // The latest state, readable synchronously. `state` is one render behind
  // during a burst of clicks, and each POST has to carry the newest value.
  const latest = useRef(initial);
  // Posts are chained rather than fired in parallel: the endpoint replaces the
  // whole state, so two in flight at once could land out of order and persist
  // the older one over the newer.
  const pending = useRef<Promise<void>>(Promise.resolve());

  const apply = useCallback(
    (next: (current: ReviewState) => ReviewState) => {
      const updated = next(latest.current);
      if (updated === latest.current) return;
      latest.current = updated;
      setState(updated);
      // Deliberately outside the updater: React may call an updater twice (it
      // does in StrictMode) or discard its result, and a network write must
      // happen exactly once per mutation.
      pending.current = pending.current
        .then(() => persist(updated))
        .then(
          () => setError(null),
          (cause: unknown) => setError(String(cause)),
        );
    },
    [],
  );

  const setStatus = useCallback(
    (id: string, status: Status) =>
      apply((current) => setStatusOn(current, id, status)),
    [apply],
  );

  const addComment = useCallback(
    (id: string, text: string) =>
      apply((current) =>
        addCommentTo(current, id, text, new Date().toISOString()),
      ),
    [apply],
  );

  return { state, setStatus, addComment, error };
}

/**
 * Node components are rendered by React Flow, which owns their props, so the
 * review API and the highlight selection reach them through context instead.
 */
export type ReviewContextValue = ReviewApi & {
  /** Node ids the progress panel is currently pointing at. */
  highlighted: ReadonlySet<string>;
};

const noop = () => {};

export const ReviewContext = createContext<ReviewContextValue>({
  state: {},
  setStatus: noop,
  addComment: noop,
  error: null,
  highlighted: new Set<string>(),
});

export function useReview(): ReviewContextValue {
  return useContext(ReviewContext);
}
