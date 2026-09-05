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
  ReactFlowProvider,
  useEdgesState,
  useNodesState,
  useReactFlow,
  type Edge,
  type Node,
  type NodeTypes,
} from "@xyflow/react";
import { useCallback, useEffect, useMemo, useState } from "react";

import "@xyflow/react/dist/style.css";

import { FileNode } from "./components/FileNode";
import { ProgressPanel } from "./components/ProgressPanel";
import { SymbolNode } from "./components/SymbolNode";
import { applyFocus } from "./lib/focus";
import { gridLayout, layout } from "./lib/layout";
import { loadSnapshot } from "./lib/snapshot";
import { toFlow, type FileSummary, type GraphFlowNode } from "./lib/transform";
import type { GraphSnapshot } from "./types/snapshot";

const nodeTypes = { symbol: SymbolNode, file: FileNode } satisfies NodeTypes;

/**
 * What the minimap paints a node with.
 *
 * Its default reads the node's own background, and a container's is a
 * translucent panel over the canvas — which the minimap composites to nearly
 * black, so a graph of containers turns the map into a slab. Painting the
 * containers as outlines of where the files are, and the cards a shade
 * brighter, is what keeps the map legible.
 */
function minimapColor(node: Node): string {
  return node.type === "file" ? "#2c3245" : "#4a5573";
}

/** What the initial fetch produced, once it has landed. */
type Loaded = {
  snapshot: GraphSnapshot;
  nodes: GraphFlowNode[];
  edges: Edge[];
  files: FileSummary[];
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
          files: flow.files,
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

/**
 * The provider has to sit above the panel, not just above the canvas: the
 * panel's rows call `fitView`, and `useReactFlow` reads the same store the
 * `ReactFlow` instance below writes to.
 */
function Graph({ loaded }: { loaded: Loaded }) {
  return (
    <ReactFlowProvider>
      <Canvas loaded={loaded} />
    </ReactFlowProvider>
  );
}

function Canvas({ loaded }: { loaded: Loaded }) {
  const [nodes, , onNodesChange] = useNodesState(loaded.nodes);
  const [edges, , onEdgesChange] = useEdgesState(loaded.edges);
  const { fitView } = useReactFlow();

  // Selection is durable — it expands the card and holds the highlight until
  // it is dismissed. Hover is a preview that only dims, and only while
  // nothing is selected, so that moving the pointer across the canvas cannot
  // take a reviewer's reading position away from them.
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [hoverId, setHoverId] = useState<string | null>(null);
  const focusId = selectedId ?? hoverId;

  // Focus is derived, never stored: `nodes` keeps whatever React Flow's own
  // change handlers wrote (a drag, a resize), and the appearance is recomputed
  // over it. Laying the graph out again on a click would move the card the
  // reviewer just aimed at.
  const display = useMemo(
    () => applyFocus(nodes, edges, focusId, selectedId),
    [nodes, edges, focusId, selectedId],
  );

  const clear = useCallback(() => {
    setSelectedId(null);
  }, []);

  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") setSelectedId(null);
    }
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
    };
  }, []);

  const onNodeClick = useCallback((_event: React.MouseEvent, node: Node) => {
    // A container is the map, not a card: clicking one is how a reviewer puts
    // the diff away without aiming at empty canvas.
    if (node.type !== "symbol") {
      setSelectedId(null);
      return;
    }
    setSelectedId((current) => (current === node.id ? null : node.id));
  }, []);

  const onNodeMouseEnter = useCallback((_event: React.MouseEvent, node: Node) => {
    setHoverId(node.id);
  }, []);

  const onNodeMouseLeave = useCallback(() => {
    setHoverId(null);
  }, []);

  const onSelectFile = useCallback(
    (containerId: string) => {
      void fitView({
        nodes: [{ id: containerId }],
        duration: 400,
        padding: 0.2,
        // Without a ceiling a one-card file fills the viewport at a zoom that
        // says nothing about where it sits in the graph.
        maxZoom: 1,
      });
    },
    [fitView],
  );

  const warnings = loaded.warnings;

  return (
    <div className="app">
      <ProgressPanel
        cardCount={loaded.snapshot.nodes.length}
        files={loaded.files}
        onSelectFile={onSelectFile}
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

        <ReactFlow
          nodes={display.nodes}
          edges={display.edges}
          nodeTypes={nodeTypes}
          onNodesChange={onNodesChange}
          onEdgesChange={onEdgesChange}
          onNodeClick={onNodeClick}
          onNodeMouseEnter={onNodeMouseEnter}
          onNodeMouseLeave={onNodeMouseLeave}
          onPaneClick={clear}
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
          <MiniMap pannable zoomable nodeColor={minimapColor} />
          <Controls />
        </ReactFlow>
      </main>
    </div>
  );
}
