/**
 * The whole application: load the snapshot and the review state, lay the graph
 * out once, and hand it to React Flow.
 *
 * Layout runs on the snapshot rather than on every render — elk is async and
 * the graph does not change while the session is open, only the review state
 * layered on top of it does.
 */

import {
  Background,
  Controls,
  MiniMap,
  ReactFlow,
  useEdgesState,
  useNodesState,
  type Edge,
  type NodeTypes,
  type ReactFlowInstance,
} from "@xyflow/react";
import { useEffect, useMemo, useRef, useState } from "react";

import "@xyflow/react/dist/style.css";

import { ProgressPanel } from "./components/ProgressPanel";
import { SymbolNode } from "./components/SymbolNode";
import { ReviewContext, useReviewState } from "./hooks/useReviewState";
import { gridLayout, layout } from "./lib/layout";
import { nodeIdsWithStatus } from "./lib/review";
import { toFlow, type SymbolFlowNode } from "./lib/transform";
import type { GraphSnapshot, ReviewState, Status } from "./types/snapshot";

const nodeTypes = { symbol: SymbolNode } satisfies NodeTypes;

async function getJson<T>(url: string): Promise<T> {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`GET ${url} failed: ${response.status}`);
  return (await response.json()) as T;
}

/** What the initial pair of fetches produced, once both have landed. */
type Loaded = {
  snapshot: GraphSnapshot;
  nodes: SymbolFlowNode[];
  edges: Edge[];
  initialState: ReviewState;
  /** `meta.warnings` plus anything that degraded on the way in. */
  warnings: string[];
};

export function App() {
  const [loaded, setLoaded] = useState<Loaded | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;

    void (async () => {
      try {
        // Both fetches are fatal, and the state one is fatal for a sharper
        // reason than the snapshot: `POST /api/state` replaces the whole
        // object, so starting from an empty review after a failed read means
        // the reviewer's first click writes that emptiness over everything
        // they had recorded before. Degrading here destroys data instead of
        // saving it. Only the layout, which owns nothing, degrades.
        const warnings: string[] = [];
        const [snapshot, initialState] = await Promise.all([
          getJson<GraphSnapshot>("/api/graph"),
          getJson<ReviewState>("/api/state"),
        ]);
        const flow = toFlow(snapshot);
        // Laying out with the saved state, not an empty one: cards that
        // already carry comments render taller than their diff alone implies.
        const nodes = await layout(flow.nodes, flow.edges, initialState).catch(
          (cause: unknown) => {
            warnings.push(`laid the graph out in a grid: ${String(cause)}`);
            return gridLayout(flow.nodes, initialState);
          },
        );
        if (cancelled) return;
        setLoaded({
          snapshot,
          nodes,
          edges: flow.edges,
          initialState,
          warnings: [...snapshot.meta.warnings, ...warnings],
        });
      } catch (cause: unknown) {
        if (!cancelled) setLoadError(String(cause));
      }
    })();

    return () => {
      cancelled = true;
    };
  }, []);

  if (loadError !== null) {
    return <p className="fatal">Could not load the review: {loadError}</p>;
  }
  if (loaded === null) {
    return <p className="loading">Loading…</p>;
  }

  // Remounting on the graph keeps `useNodesState` seeded correctly without an
  // effect that copies props into state.
  return <Graph key={loaded.snapshot.meta.head} loaded={loaded} />;
}

function Graph({ loaded }: { loaded: Loaded }) {
  const [nodes, , onNodesChange] = useNodesState(loaded.nodes);
  const [edges, , onEdgesChange] = useEdgesState(loaded.edges);
  const review = useReviewState(loaded.initialState);

  const [selected, setSelected] = useState<Status | null>(null);

  const nodeIds = useMemo(
    () => loaded.snapshot.nodes.map((node) => node.id),
    [loaded.snapshot],
  );

  // Derived rather than captured at click time, so marking a card while a
  // status is highlighted moves it in or out of the highlight immediately.
  const highlighted = useMemo<ReadonlySet<string>>(
    () =>
      selected === null
        ? new Set<string>()
        : new Set(nodeIdsWithStatus(review.state, nodeIds, selected)),
    [selected, review.state, nodeIds],
  );

  const context = useMemo(
    () => ({ ...review, highlighted }),
    [review, highlighted],
  );

  // A branch worth reviewing lays out tens of thousands of pixels wide, so
  // `fitView` parks the first paint at a zoom where a card is a few pixels
  // across and the highlight ring is sub-pixel. Lighting the matching cards is
  // only useful if the viewport also goes to them.
  const flow = useRef<ReactFlowInstance<SymbolFlowNode, Edge> | null>(null);
  // Read through a ref so the pan fires when the reviewer picks a status and
  // not every time a verdict changes which cards match — approving a card
  // while "Pending" is lit must not yank the canvas out from under the click.
  const highlightedRef = useRef(highlighted);
  highlightedRef.current = highlighted;

  useEffect(() => {
    const instance = flow.current;
    if (instance === null || selected === null) return;
    const ids = [...highlightedRef.current].map((id) => ({ id }));
    if (ids.length === 0) return;
    // `maxZoom` keeps the last pending card from filling the screen at 8x.
    void instance.fitView({
      nodes: ids,
      duration: 400,
      maxZoom: 1,
      padding: 0.2,
    });
  }, [selected]);

  const warnings = loaded.warnings;

  return (
    <ReviewContext.Provider value={context}>
      <div className="app">
        <ProgressPanel
          state={review.state}
          nodeIds={nodeIds}
          selected={selected}
          onSelect={setSelected}
        />

        <main className="canvas">
          {warnings.length > 0 && (
            <div className="warnings" role="status">
              <strong>
                {warnings.length} warning{warnings.length === 1 ? "" : "s"}
              </strong>
              <ul>
                {warnings.map((warning, index) => (
                  <li key={index}>{warning}</li>
                ))}
              </ul>
            </div>
          )}

          {review.error !== null && (
            <div className="warnings error" role="alert">
              Review state was not saved: {review.error}
            </div>
          )}

          <ReactFlow
            nodes={nodes}
            edges={edges}
            nodeTypes={nodeTypes}
            onInit={(instance) => {
              flow.current = instance;
            }}
            onNodesChange={onNodesChange}
            onEdgesChange={onEdgesChange}
            // The graph is the diff, not a document to edit: Backspace would
            // otherwise delete the selected card for the rest of the session
            // while the progress panel — counting the snapshot, not the canvas
            // — kept asking the reviewer to act on it. Nothing consumes new
            // connections either, so the handles must not offer to make any.
            deleteKeyCode={null}
            nodesConnectable={false}
            // A card is a whole diff — hundreds of line elements — and a real
            // branch has hundreds of cards. Keeping the ones outside the
            // viewport out of the DOM is what keeps clicking Approve from
            // re-laying-out every diff on the canvas.
            onlyRenderVisibleElements
            // Low enough that `fitView` can actually fit the graph. elk packs
            // the components a diff of a few hundred cards produces into tens
            // of thousands of pixels each way; clamping at a zoom that cannot
            // hold that leaves the first paint parked on one slab of the canvas
            // with no sign of where the rest of it went.
            minZoom={0.005}
            fitView
          >
            <Background />
            <MiniMap pannable zoomable />
            <Controls />
          </ReactFlow>
        </main>
      </div>
    </ReviewContext.Provider>
  );
}
