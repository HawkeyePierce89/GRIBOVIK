/**
 * Review progress, and a way to find what is left.
 *
 * Clicking a counter highlights exactly the nodes in that status and clears
 * the previous highlight; clicking the active counter again clears it. The
 * panel only reports *which* status is wanted — the set of ids is derived from
 * the live review state, so a card approved while "Pending" is showing drops
 * out of the highlight instead of staying lit.
 */

import { countStatuses } from "../lib/review";
import type { ReviewState, Status } from "../types/snapshot";

const STATUSES: Status[] = ["approved", "rejected", "pending"];

const LABEL: Record<Status, string> = {
  approved: "Approved",
  rejected: "Rejected",
  pending: "Pending",
};

export type ProgressPanelProps = {
  state: ReviewState;
  /** Ids of the nodes currently in the graph; state for others is ignored. */
  nodeIds: string[];
  /** Which counter is active, or `null` when nothing is highlighted. */
  selected: Status | null;
  onSelect: (status: Status | null) => void;
};

export function ProgressPanel({
  state,
  nodeIds,
  selected,
  onSelect,
}: ProgressPanelProps) {
  const counts = countStatuses(state, nodeIds);

  return (
    <aside className="progress-panel">
      <h1 className="progress-title">GRIBOVIK</h1>
      <p className="progress-total">{nodeIds.length} changed symbols</p>
      <div className="progress-counters">
        {STATUSES.map((status) => (
          <button
            key={status}
            type="button"
            className={`counter counter-${status} ${
              selected === status ? "active" : ""
            }`}
            aria-pressed={selected === status}
            onClick={() => onSelect(selected === status ? null : status)}
          >
            <span className="counter-value">{counts[status]}</span>
            <span className="counter-label">{LABEL[status]}</span>
          </button>
        ))}
      </div>
    </aside>
  );
}
