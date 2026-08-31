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
} from "@xyflow/react";
import { useEffect, useMemo, useState } from "react";

import "@xyflow/react/dist/style.css";

import { ProgressPanel } from "./components/ProgressPanel";
import { SymbolNode } from "./components/SymbolNode";
import { ReviewContext, useReviewState } from "./hooks/useReviewState";
import { layout } from "./lib/layout";
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
};

export function App() {
  const [loaded, setLoaded] = useState<Loaded | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;

    void (async () => {
      try {
        const [snapshot, initialState] = await Promise.all([
          getJson<GraphSnapshot>("/api/graph"),
          getJson<ReviewState>("/api/state"),
        ]);
        const flow = toFlow(snapshot);
        const nodes = await layout(flow.nodes, flow.edges);
        if (cancelled) return;
        setLoaded({ snapshot, nodes, edges: flow.edges, initialState });
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

  const warnings = loaded.snapshot.meta.warnings;

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
              <strong>{warnings.length} warnings</strong>
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
            onNodesChange={onNodesChange}
            onEdgesChange={onEdgesChange}
            minZoom={0.05}
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
