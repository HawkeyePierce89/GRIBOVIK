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
 * Its default is one flat colour for every node: React Flow reads a node's
 * inline `style` prop for a fill and falls back to a single stylesheet
 * variable, and `transform.ts` gives no node a `style`. A container is a
 * rectangle drawn under its own cards, so painting both the same shade fills
 * the map with featureless file-shaped slabs. Two shades keep the two levels
 * apart — where the files are, and where the cards inside them are.
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

  // Both ids, not just the selection: `focusId` falls back to the hover, so
  // dropping the selection while the pointer still rests on the card would
  // collapse the diff and leave the dimming exactly as it was.
  const clear = useCallback(() => {
    setSelectedId(null);
    setHoverId(null);
  }, []);

  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (event.key !== "Escape") return;
      clear();
    }
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [clear]);

  const onNodeClick = useCallback(
    (_event: React.MouseEvent, node: Node) => {
      // A container is the map, not a card: clicking one is how a reviewer
      // puts the diff away without aiming at empty canvas.
      if (node.type !== "symbol") {
        clear();
        return;
      }
      // Clicking the open card again is a dismissal like the other three, so
      // it goes through `clear`: the pointer is still resting on that card, so
      // dropping only the selection would collapse the diff and leave the
      // whole graph dimmed around it.
      if (node.id === selectedId) {
        clear();
        return;
      }
      setSelectedId(node.id);
    },
    [clear, selectedId],
  );

  const onNodeMouseEnter = useCallback((_event: React.MouseEvent, node: Node) => {
    setHoverId(node.id);
  }, []);

  // Also the pan/zoom handler: with `onlyRenderVisibleElements` on, moving the
  // viewport while the pointer rests on a card can unmount that card, and a
  // removed element delivers no `mouseleave` — so the preview dimming would
  // stay on the canvas around a card that is no longer in it.
  const clearHover = useCallback(() => {
    setHoverId(null);
  }, []);

  const onSelectFile = useCallback(
    (containerId: string) => {
      // Asking for a file is navigation, not a refinement of the current
      // reading position: without this the reviewer arrives at the container
      // they picked with every card in it dimmed by a selection made
      // somewhere else, and the open diff still off-screen.
      clear();
      void fitView({
        nodes: [{ id: containerId }],
        duration: 400,
        padding: 0.2,
        // Without a ceiling a one-card file fills the viewport at a zoom that
        // says nothing about where it sits in the graph.
        maxZoom: 1,
      });
    },
    [clear, fitView],
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
          onNodeMouseLeave={clearHover}
          onMoveStart={clearHover}
          onPaneClick={clear}
          // The graph is the diff, not a document to edit: Backspace would
          // otherwise delete the selected card for the rest of the session.
          // Nothing consumes new connections either, so the handles must not
          // offer to make any.
          deleteKeyCode={null}
          nodesConnectable={false}
          // React Flow's node focus is an affordance it cannot honour here:
          // it makes every node a tab stop announced as "press enter or space
          // to select", but gates that handler on `isSelectable`, and every
          // node is emitted `selectable: false` for the z-index reasons
          // `transform.ts` spells out. Hundreds of tab stops that do nothing,
          // and a container has nothing to activate in the first place. The
          // keyboard path a card does have lives on `.symbol-row`, which
          // `SymbolNode` makes a real `role="button"`.
          nodesFocusable={false}
          // And edges are not a keyboard destination either. React Flow makes
          // every edge a tab stop announced as "Edge from X to Y" — up to 934
          // of them on the MVP snapshot, interleaved with the card rows that
          // are the actual keyboard path, and *all* of them the moment a card
          // is open and culling stands down.
          edgesFocusable={false}
          // A real branch is hundreds of cards and twice as many edges, and
          // keeping the ones outside the viewport out of the DOM is what keeps
          // the canvas responsive. It has to stand down while a card is open,
          // though: React Flow decides visibility from a node's measured box,
          // and an expanded card's diff is drawn *outside* that box on
          // purpose. Panning the 52px row off-screen would cull the node and
          // blank the panel the reviewer is still reading. What that costs is
          // bounded by the *graph*, not by the one card: opening a card mounts
          // every node and edge the diff has — measured at 632 nodes and 934
          // edges, ~114 ms, on the 572-card MVP snapshot — and unmounts them
          // again on collapse. Acceptable today, and the thing to revisit if a
          // larger range makes the click feel slow: the way out is to draw the
          // overlay outside the culled node (a viewport portal) so culling can
          // stay on unconditionally.
          onlyRenderVisibleElements={selectedId === null}
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
