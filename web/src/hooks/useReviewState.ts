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
  useMemo,
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
  /** The comment a reviewer has started typing on a card, `""` if none. */
  getDraft: (id: string) => string;
  setDraft: (id: string, text: string) => void;
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

  // Half-written comments, held outside the card that renders them.
  //
  // `onlyRenderVisibleElements` unmounts a card the moment it leaves the
  // viewport, taking any state the card owns with it — a reviewer mid-sentence
  // who pans the canvas would lose what they had typed, with nothing to say it
  // had happened. A ref rather than state on purpose: a draft is read once when
  // a card mounts and written on every keystroke, and putting it in the context
  // value would re-render every card on the canvas per character typed.
  const drafts = useRef(new Map<string, string>());
  const getDraft = useCallback((id: string) => drafts.current.get(id) ?? "", []);
  const setDraft = useCallback((id: string, text: string) => {
    if (text === "") drafts.current.delete(id);
    else drafts.current.set(id, text);
  }, []);

  // Memoized because `App` builds the context value from this object: a fresh
  // literal every render makes the context value fresh every render too, and a
  // context change bypasses React Flow's node memoization — every card would
  // re-render on every pan, drag and selection, not just on a verdict.
  return useMemo(
    () => ({ state, setStatus, addComment, getDraft, setDraft, error }),
    [state, setStatus, addComment, getDraft, setDraft, error],
  );
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
  getDraft: () => "",
  setDraft: noop,
  error: null,
  highlighted: new Set<string>(),
});

export function useReview(): ReviewContextValue {
  return useContext(ReviewContext);
}
