import { useCallback, useRef } from "react";
import ReactFlow, {
  Background,
  Controls,
  Node,
  Edge,
  BackgroundVariant,
  useNodesState,
  useEdgesState,
  Handle,
  Position,
  NodeProps,
} from "reactflow";

import { StateTransitionEvent, STAGE_COLORS } from "../types";

// ── Custom Node ───────────────────────────────────────────────────────────────

interface StateNodeData {
  label: string;
  stage: string;
  experimentId: string;
  timestamp: string;
}

function StateNode({ data }: NodeProps<StateNodeData>) {
  const color = STAGE_COLORS[data.stage] ?? "#3a4a5a";
  return (
    <div
      style={{
        background: "#0f1117",
        border: `1px solid ${color}`,
        borderRadius: 4,
        padding: "8px 14px",
        minWidth: 160,
        boxShadow: `0 0 8px ${color}44`,
        fontFamily: '"JetBrains Mono", monospace',
        fontSize: 11,
      }}
    >
      <Handle type="target" position={Position.Top} style={{ background: color, border: "none" }} />

      <div style={{ color, fontWeight: 700, letterSpacing: "0.06em", marginBottom: 3 }}>
        {data.stage || "IDLE"}
      </div>
      <div style={{ color: "#3a5a6a", fontSize: 10 }}>
        {data.experimentId ? `exp:${data.experimentId.slice(0, 8)}` : "—"}
      </div>
      <div style={{ color: "#3a4a5a", fontSize: 9, marginTop: 2 }}>{data.timestamp}</div>

      <Handle type="source" position={Position.Bottom} style={{ background: color, border: "none" }} />
    </div>
  );
}

// Defined OUTSIDE the parent component — required by React Flow for stability
const NODE_TYPES = { stateNode: StateNode };

// ── Component ─────────────────────────────────────────────────────────────────

interface BlueprintGraphProps {
  transitions: StateTransitionEvent[];
}

export default function BlueprintGraph({ transitions }: BlueprintGraphProps) {
  const [nodes, setNodes, onNodesChange] = useNodesState<StateNodeData>([]);
  const [edges, setEdges, onEdgesChange] = useEdgesState([]);

  // Track layout position — ref avoids double-render
  const yOffsetRef = useRef(0);
  const lastNodeIdRef = useRef<string | null>(null);
  const seenRef = useRef(new Set<string>());

  // Process new transitions that arrive
  const prevLen = useRef(0);
  if (transitions.length > prevLen.current) {
    const newTransitions = transitions.slice(prevLen.current);
    prevLen.current = transitions.length;

    newTransitions.forEach((t) => {
      // Deduplicate by (from→to + timestamp)
      const key = `${t.from}->${t.to}-${t.timestamp_ms}`;
      if (seenRef.current.has(key)) return;
      seenRef.current.add(key);

      const nodeId = `node-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`;
      const ts = new Date(t.timestamp_ms).toLocaleTimeString("en-US", {
        hour: "2-digit",
        minute: "2-digit",
        second: "2-digit",
      });

      const newNode: Node<StateNodeData> = {
        id: nodeId,
        type: "stateNode",
        position: { x: 60, y: yOffsetRef.current },
        data: {
          label: t.to,
          stage: t.to,
          experimentId: t.experiment_id,
          timestamp: ts,
        },
      };

      yOffsetRef.current += 120;

      setNodes((prev) => [...prev, newNode]);

      if (lastNodeIdRef.current) {
        const prevNodeId = lastNodeIdRef.current;
        const color = STAGE_COLORS[t.to] ?? "#3a4a5a";
        const newEdge: Edge = {
          id: `edge-${prevNodeId}-${nodeId}`,
          source: prevNodeId,
          target: nodeId,
          animated: true,
          style: { stroke: color, strokeWidth: 1.5, opacity: 0.7 },
        };
        setEdges((prev) => [...prev, newEdge]);
      }

      lastNodeIdRef.current = nodeId;
    });
  }

  const onNodesChangeHandler = useCallback(onNodesChange, [onNodesChange]);
  const onEdgesChangeHandler = useCallback(onEdgesChange, [onEdgesChange]);

  const isEmpty = nodes.length === 0;

  return (
    <div style={{ width: "100%", height: "100%", position: "relative" }}>
      <ReactFlow
        nodes={nodes}
        edges={edges}
        onNodesChange={onNodesChangeHandler}
        onEdgesChange={onEdgesChangeHandler}
        nodeTypes={NODE_TYPES}
        fitView
        fitViewOptions={{ padding: 0.3 }}
        minZoom={0.3}
        maxZoom={2}
        proOptions={{ hideAttribution: true }}
      >
        <Background
          variant={BackgroundVariant.Dots}
          color="#1a2035"
          gap={18}
          size={1}
        />
        <Controls
          showInteractive={false}
          style={{ background: "#0f1117", border: "1px solid #1a2035" }}
        />
      </ReactFlow>

      {/* Empty state overlay */}
      {isEmpty && (
        <div
          style={{
            position: "absolute",
            inset: 0,
            display: "flex",
            flexDirection: "column",
            alignItems: "center",
            justifyContent: "center",
            pointerEvents: "none",
            gap: 8,
          }}
        >
          <div style={{ color: "#1a2a3a", fontSize: 11, letterSpacing: "0.12em" }}>
            REASONING GRAPH
          </div>
          <div style={{ color: "#0d1a25", fontSize: 10 }}>
            boot lab to begin
          </div>
        </div>
      )}

      {/* Panel label */}
      <div
        style={{
          position: "absolute",
          top: 8,
          left: 10,
          fontSize: 9,
          color: "#1a3a4a",
          letterSpacing: "0.14em",
          pointerEvents: "none",
        }}
      >
        BLUEPRINT
      </div>
    </div>
  );
}
