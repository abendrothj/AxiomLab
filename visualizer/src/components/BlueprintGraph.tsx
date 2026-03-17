import { useState, useEffect, useCallback } from "react";
import ReactFlow, {
  Node, Edge, Background, Controls,
  Handle, Position, NodeProps,
  MarkerType, useReactFlow, ReactFlowProvider,
} from "reactflow";
import "reactflow/dist/style.css";

import { StateTransitionEvent, STAGE_COLORS } from "../types";

// ── Constants ─────────────────────────────────────────────────────────────────

const NODE_H    = 72;   // px between node centres
const INIT_SHOW = 5;
const LOAD_STEP = 5;

// ── Custom node ───────────────────────────────────────────────────────────────

function StageNode({ data }: NodeProps) {
  const color  = STAGE_COLORS[data.stage as string] ?? "#3a4a5a";
  const isLast = data.isLast as boolean;

  return (
    <>
      <Handle
        type="target"
        position={Position.Top}
        style={{ background: "#1a3050", border: "none", width: 6, height: 6 }}
      />
      <div style={{
        width: 176,
        padding: "9px 12px 9px 14px",
        background: isLast ? "#0e1520" : "#0b0e18",
        border: `1px solid ${isLast ? color + "44" : "#131d2a"}`,
        borderLeft: `3px solid ${color}`,
        borderRadius: "0 6px 6px 0",
        fontFamily: '"JetBrains Mono", monospace',
        boxShadow: isLast ? `0 0 12px ${color}18` : "none",
      }}>
        <div style={{
          fontSize: 11, fontWeight: 700,
          color: isLast ? color : color + "cc",
          letterSpacing: "0.06em",
          lineHeight: 1,
        }}>
          {data.stage as string}
        </div>
        <div style={{
          fontSize: 9, color: "#1e3a4a",
          marginTop: 5, letterSpacing: "0.04em",
          display: "flex", gap: 6,
        }}>
          <span>{data.expId as string}</span>
          <span style={{ color: "#152535" }}>·</span>
          <span>{data.time as string}</span>
        </div>
      </div>
      <Handle
        type="source"
        position={Position.Bottom}
        style={{ background: "#1a3050", border: "none", width: 6, height: 6 }}
      />
    </>
  );
}

// Defined outside component — required by ReactFlow for stable reference
const nodeTypes = { stage: StageNode };

// ── Inner graph (must be inside ReactFlowProvider) ────────────────────────────

function BlueprintInner({ transitions }: { transitions: StateTransitionEvent[] }) {
  const [showCount, setShowCount] = useState(INIT_SHOW);
  const { fitView } = useReactFlow();

  // Clamp to available
  const count   = Math.min(showCount, transitions.length);
  const hasMore = transitions.length > count;
  const slice   = transitions.slice(-count);

  // Build nodes
  const nodes: Node[] = slice.map((t, i) => {
    const isLast  = i === slice.length - 1;
    const ageSecs = Math.floor((Date.now() - t.timestamp_ms) / 1000);
    const timeStr = ageSecs < 60 ? `${ageSecs}s` : `${Math.floor(ageSecs / 60)}m`;
    return {
      id:       `n${transitions.length - count + i}`,
      type:     "stage",
      draggable: false,
      selectable: false,
      position: { x: 0, y: i * NODE_H },
      data: {
        stage:  t.to,
        expId:  `#${t.experiment_id.replace(/[^0-9]/g, "").slice(0, 4) || (transitions.length - count + i + 1)}`,
        time:   timeStr,
        isLast,
      },
    };
  });

  // Build edges
  const edges: Edge[] = nodes.slice(1).map((node, i) => ({
    id:        `e${i}`,
    source:    nodes[i].id,
    target:    node.id,
    type:      "smoothstep",
    animated:  i === nodes.length - 2,
    style:     { stroke: "#1e3048", strokeWidth: 1.5 },
    markerEnd: { type: MarkerType.ArrowClosed, color: "#1e3048", width: 14, height: 14 },
  }));

  // Fit viewport to last 5 nodes whenever new transitions arrive
  useEffect(() => {
    if (nodes.length === 0) return;
    const last5 = nodes.slice(-5).map((n) => ({ id: n.id }));
    const timer = setTimeout(() => {
      fitView({ nodes: last5, duration: 450, padding: 0.35 });
    }, 60);
    return () => clearTimeout(timer);
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [transitions.length, fitView]);

  const loadMore = useCallback(() => {
    setShowCount((c) => c + LOAD_STEP);
  }, []);

  return (
    <div style={{ width: "100%", height: "100%", position: "relative" }}>
      {hasMore && (
        <button
          onClick={loadMore}
          style={{
            position: "absolute", top: 8, left: "50%",
            transform: "translateX(-50%)",
            zIndex: 10,
            padding: "4px 14px",
            background: "#0a1220",
            border: "1px solid #1a2a3a",
            borderRadius: 20,
            color: "#2a5a7a",
            fontSize: 9,
            letterSpacing: "0.1em",
            cursor: "pointer",
            fontFamily: '"JetBrains Mono", monospace',
            whiteSpace: "nowrap",
          }}
        >
          ↑ {Math.min(LOAD_STEP, transitions.length - count)} EARLIER
        </button>
      )}

      <ReactFlow
        nodes={nodes}
        edges={edges}
        nodeTypes={nodeTypes}
        proOptions={{ hideAttribution: true }}
        panOnDrag
        zoomOnScroll
        zoomOnPinch
        minZoom={0.3}
        maxZoom={2.5}
        defaultEdgeOptions={{ type: "smoothstep" }}
      >
        <Background
          color="#131d2a"
          gap={20}
          size={0.6}
          style={{ background: "#070912" }}
        />
        <Controls
          showInteractive={false}
          style={{
            background: "#0b0e18",
            border: "1px solid #131d2a",
            borderRadius: 4,
          }}
        />
      </ReactFlow>
    </div>
  );
}

// ── Public component ──────────────────────────────────────────────────────────

export default function BlueprintGraph({ transitions }: { transitions: StateTransitionEvent[] }) {
  if (transitions.length === 0) {
    return (
      <div style={{
        width: "100%", height: "100%",
        display: "flex", flexDirection: "column",
        alignItems: "center", justifyContent: "center",
        gap: 10, color: "#1a3040",
      }}>
        <svg width="28" height="28" viewBox="0 0 28 28" fill="none" opacity={0.35}>
          <circle cx="14" cy="7"  r="4" stroke="#00d4ff" strokeWidth="1.2" />
          <circle cx="14" cy="21" r="4" stroke="#00d4ff" strokeWidth="1.2" />
          <line x1="14" y1="11" x2="14" y2="17" stroke="#00d4ff" strokeWidth="1.2" />
        </svg>
        <span style={{ fontSize: 10, letterSpacing: "0.08em" }}>
          waiting for transitions...
        </span>
      </div>
    );
  }

  return (
    <ReactFlowProvider>
      <BlueprintInner transitions={transitions} />
    </ReactFlowProvider>
  );
}
