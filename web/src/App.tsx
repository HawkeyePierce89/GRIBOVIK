/**
 * The whole application: load the snapshot, lay the graph out once, and hand
 * it to React Flow.
 *
 * Layout runs on the snapshot rather than on every render — elk is async and
 * the graph does not change while the session is open.
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
import { useEffect, useState } from "react";

import "@xyflow/react/dist/style.css";

import { ProgressPanel } from "./components/ProgressPanel";
import { SymbolNode } from "./components/SymbolNode";
import { gridLayout, layout } from "./lib/layout";
import { loadSnapshot } from "./lib/snapshot";
import { toFlow, type GraphFlowNode } from "./lib/transform";
import type { GraphSnapshot } from "./types/snapshot";

const nodeTypes = { symbol: SymbolNode } satisfies NodeTypes;

/** What the initial fetch produced, once it has landed. */
type Loaded = {
  snapshot: GraphSnapshot;
  nodes: GraphFlowNode[];
  edges: Edge[];
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
        const warnings: string[] = [];
        const snapshot = await loadSnapshot();
        const flow = toFlow(snapshot);
        // Only the layout, which owns nothing, degrades.
        const nodes = await layout(flow.nodes, flow.edges).catch(
          (cause: unknown) => {
            warnings.push(`laid the graph out in a grid: ${String(cause)}`);
            return gridLayout(flow.nodes);
          },
        );
        if (cancelled) return;
        setLoaded({
          snapshot,
          nodes,
          edges: flow.edges,
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

  const warnings = loaded.warnings;

  return (
    <div className="app">
      <ProgressPanel cardCount={loaded.snapshot.nodes.length} />

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

        <ReactFlow
          nodes={nodes}
          edges={edges}
          nodeTypes={nodeTypes}
          onNodesChange={onNodesChange}
          onEdgesChange={onEdgesChange}
          // The graph is the diff, not a document to edit: Backspace would
          // otherwise delete the selected card for the rest of the session.
          // Nothing consumes new connections either, so the handles must not
          // offer to make any.
          deleteKeyCode={null}
          nodesConnectable={false}
          // A card is a whole diff — hundreds of line elements — and a real
          // branch has hundreds of cards. Keeping the ones outside the
          // viewport out of the DOM is what keeps the canvas responsive.
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
  );
}
