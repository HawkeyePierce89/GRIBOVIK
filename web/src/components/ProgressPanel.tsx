/**
 * The header panel: what is being reviewed, and how much of it there is.
 */

export type ProgressPanelProps = {
  /** How many cards the graph holds — changed symbols plus file catch-alls. */
  cardCount: number;
};

export function ProgressPanel({ cardCount }: ProgressPanelProps) {
  return (
    <aside className="progress-panel">
      <h1 className="progress-title">GRIBOVIK</h1>
      {/* "cards", not "changed symbols": the count includes the synthetic
          file-level catch-alls, which are not symbols. */}
      <p className="progress-total">
        {cardCount} card{cardCount === 1 ? "" : "s"} to review
      </p>
    </aside>
  );
}
