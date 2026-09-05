/**
 * The left panel: what is being reviewed, how much of it there is, and where
 * each piece of it lives.
 *
 * The file list is the graph's index. elk packs a real branch into tens of
 * thousands of pixels each way, so at the initial `fitView` a reviewer can see
 * the shape of the graph but not read it; clicking a row is the way back to a
 * particular file without hunting for its container across the canvas.
 */

import type { FileSummary } from "../lib/transform";

export type ProgressPanelProps = {
  /** How many cards the graph holds — changed symbols plus file catch-alls. */
  cardCount: number;
  /** One row per changed file, in the order `toFlow` found them. */
  files: FileSummary[];
  /** Called with the file's container id when its row is clicked. */
  onSelectFile: (containerId: string) => void;
};

export function ProgressPanel({
  cardCount,
  files,
  onSelectFile,
}: ProgressPanelProps) {
  // Sorted by path rather than by first appearance: the snapshot's order is
  // git's, and a reviewer looking for a file looks for it alphabetically.
  const sorted = [...files].sort((a, b) => a.file.localeCompare(b.file));

  return (
    <aside className="progress-panel">
      <h1 className="progress-title">GRIBOVIK</h1>
      {/* "cards", not "changed symbols": the count includes the synthetic
          file-level catch-alls, which are not symbols. */}
      <p className="progress-total">
        {cardCount} card{cardCount === 1 ? "" : "s"} to review
      </p>

      <ul className="file-list">
        {sorted.map((file) => (
          <li key={file.containerId}>
            <button
              type="button"
              className="file-row"
              onClick={() => {
                onSelectFile(file.containerId);
              }}
            >
              <span className="file-row-path" title={file.file}>
                {file.file}
              </span>
              <span className="file-row-count">{file.cardCount}</span>
              <span className="file-counts">
                <span className="count-added">+{file.added}</span>{" "}
                <span className="count-removed">−{file.removed}</span>
              </span>
            </button>
          </li>
        ))}
      </ul>
    </aside>
  );
}
