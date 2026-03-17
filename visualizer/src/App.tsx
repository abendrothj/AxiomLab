import { useState, useEffect } from "react";
import "reactflow/dist/style.css";

import {
  EVENTS, StateTransitionEvent, ToolExecutionEvent, LlmTokenEvent,
  NotebookEntryEvent, Stage, STAGE_COLORS,
} from "./types";
import { eventBus } from "./eventBus";
import Sandbox3D from "./components/Sandbox3D";
import IntelPanel from "./components/IntelPanel";

// ── API ───────────────────────────────────────────────────────────────────────

const API = import.meta.env.DEV ? "http://localhost:3000/api" : "/api";
async function apiStatus() {
  return fetch(`${API}/status`).then((r) => r.json()).catch(() => ({}));
}

// ── Component ─────────────────────────────────────────────────────────────────

export default function App() {
  const [stage, setStage]             = useState<Stage>("");
  const [iteration, setIteration]     = useState(0);
  const [connected, setConnected]     = useState(false);
  const [panelOpen, setPanelOpen]     = useState(false);
  const [toolEvents, setToolEvents]   = useState<ToolExecutionEvent[]>([]);
  const [notebook, setNotebook]       = useState<NotebookEntryEvent[]>([]);
  const [transitions, setTransitions] = useState<StateTransitionEvent[]>([]);
  const [latestTool, setLatestTool]   = useState<ToolExecutionEvent | null>(null);

  // ── Hydrate from server on mount (late-joiners see full history) ──
  useEffect(() => {
    apiStatus().then((s) => {
      if (s.iteration) setIteration(s.iteration);
      if (Array.isArray(s.notebook) && s.notebook.length > 0) {
        setNotebook(s.notebook as NotebookEntryEvent[]);
      }
    });

    const connPoll = setInterval(() => {
      const bus = eventBus as unknown as { ws: WebSocket | null };
      setConnected(bus.ws?.readyState === WebSocket.OPEN);
    }, 1500);

    return () => clearInterval(connPoll);
  }, []);

  // ── Snapshot from WS (includes current notebook history) ──
  useEffect(() => {
    return eventBus.listen<{
      running: boolean; iteration: number; notebook: NotebookEntryEvent[];
    }>("snapshot", (snap) => {
      setIteration(snap.iteration);
      if (snap.notebook?.length > 0) setNotebook(snap.notebook);
      setConnected(true);
    });
  }, []);

  // ── Live events ──
  useEffect(() => {
    const unsubs = [
      eventBus.listen<LlmTokenEvent>(EVENTS.LLM_TOKEN, () => {
        // token stream not displayed publicly — only used to show activity
      }),
      eventBus.listen<StateTransitionEvent>(EVENTS.STATE_TRANSITION, (p) => {
        setStage(p.to as Stage);
        setTransitions((prev) => [...prev, p]);
        if (p.to === "Proposed") setIteration((n) => n + 1);
      }),
      eventBus.listen<ToolExecutionEvent>(EVENTS.TOOL_EXECUTION, (p) => {
        setToolEvents((prev) => [p, ...prev].slice(0, 100));
        setLatestTool(p);
      }),
      eventBus.listen<NotebookEntryEvent>(EVENTS.NOTEBOOK_ENTRY, (p) => {
        setNotebook((prev) => [...prev, p]);
      }),
    ];
    return () => unsubs.forEach((fn) => fn());
  }, []);

  const stageColor = STAGE_COLORS[stage] ?? "#3a4a5a";

  // Count recent discoveries for the badge
  const newDiscoveries = notebook.filter(
    (e) => e.outcome === "discovery" &&
    Date.now() - e.timestamp_ms < 60_000
  ).length;

  return (
    <div
      style={{
        width: "100%",
        height: "100%",
        position: "relative",
        overflow: "hidden",
        background: "#0a0b0d",
        fontFamily: '"JetBrains Mono", "Fira Code", monospace',
      }}
    >
      {/* ── Full-screen 3D sandbox ── */}
      <div style={{ position: "absolute", inset: 0 }}>
        <Sandbox3D latestTool={latestTool} />
      </div>

      {/* ── Top HUD bar ── */}
      <div
        style={{
          position: "absolute",
          top: 0,
          left: 0,
          right: 0,
          height: 52,
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          padding: "0 20px",
          background: "linear-gradient(to bottom, rgba(8,10,16,0.88) 0%, transparent 100%)",
          backdropFilter: "blur(6px)",
          zIndex: 5,
        }}
      >
        {/* Logo */}
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 10,
            fontSize: 13,
            fontWeight: 700,
            letterSpacing: "0.14em",
            color: "#00ff9d",
          }}
        >
          <svg width="20" height="20" viewBox="0 0 22 22" fill="none">
            <polygon points="11,2 20,7 20,15 11,20 2,15 2,7" stroke="#00ff9d" strokeWidth="1.5" fill="none" />
            <circle cx="11" cy="11" r="3" fill="#00ff9d" opacity="0.8" />
            <line x1="11" y1="2" x2="11" y2="6" stroke="#00ff9d" strokeWidth="1" opacity="0.5" />
            <line x1="11" y1="16" x2="11" y2="20" stroke="#00ff9d" strokeWidth="1" opacity="0.5" />
            <line x1="2" y1="7" x2="5.5" y2="9" stroke="#00ff9d" strokeWidth="1" opacity="0.5" />
            <line x1="16.5" y1="13" x2="20" y2="15" stroke="#00ff9d" strokeWidth="1" opacity="0.5" />
          </svg>
          AXIOMLAB
        </div>

        {/* Center: stage + iteration */}
        <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
          {iteration > 0 && (
            <span
              style={{
                fontSize: 10,
                color: "#3a6a8a",
                letterSpacing: "0.1em",
                background: "rgba(10,20,35,0.6)",
                padding: "3px 10px",
                borderRadius: 3,
                border: "1px solid #1a2a3a",
              }}
            >
              ITER {iteration}
            </span>
          )}
          <div
            style={{
              display: "flex",
              alignItems: "center",
              gap: 7,
              padding: "4px 12px",
              border: `1px solid ${stageColor}44`,
              borderRadius: 3,
              background: `${stageColor}0d`,
              fontSize: 11,
              letterSpacing: "0.08em",
              color: stageColor,
            }}
          >
            <span
              style={{
                width: 6,
                height: 6,
                borderRadius: "50%",
                background: stageColor,
                boxShadow: stage ? `0 0 6px ${stageColor}` : "none",
              }}
            />
            {stage || "INITIALISING"}
          </div>
        </div>

        {/* Right: connection indicator */}
        <div
          style={{
            display: "flex",
            alignItems: "center",
            gap: 6,
            fontSize: 9,
            letterSpacing: "0.1em",
            color: connected ? "#2a6a4a" : "#6a2a2a",
          }}
        >
          <span
            style={{
              width: 5,
              height: 5,
              borderRadius: "50%",
              background: connected ? "#00ff9d" : "#ff3b3b",
              boxShadow: connected ? "0 0 4px #00ff9d" : "none",
            }}
          />
          {connected ? "LIVE" : "CONNECTING"}
        </div>
      </div>

      {/* ── Intel panel toggle button — bottom right ── */}
      <button
        onClick={() => setPanelOpen((o) => !o)}
        style={{
          position: "absolute",
          bottom: 24,
          right: 24,
          zIndex: 5,
          display: "flex",
          alignItems: "center",
          gap: 8,
          padding: "10px 18px",
          background: panelOpen
            ? "rgba(0,255,157,0.12)"
            : "rgba(10,14,22,0.82)",
          backdropFilter: "blur(12px)",
          border: `1px solid ${panelOpen ? "#00ff9d66" : "#1a2a3a"}`,
          borderRadius: 6,
          color: panelOpen ? "#00ff9d" : "#4a8aaa",
          fontFamily: '"JetBrains Mono", monospace',
          fontSize: 11,
          fontWeight: 700,
          letterSpacing: "0.12em",
          cursor: "pointer",
          transition: "all 0.18s",
        }}
      >
        <span style={{ fontSize: 14 }}>⬡</span>
        INTEL
        {/* Badge for new discoveries */}
        {newDiscoveries > 0 && !panelOpen && (
          <span
            style={{
              background: "#00ff9d",
              color: "#0a0b0d",
              borderRadius: "50%",
              width: 16,
              height: 16,
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              fontSize: 9,
              fontWeight: 700,
            }}
          >
            {newDiscoveries}
          </span>
        )}
      </button>

      {/* ── Notebook entry toast — bottom left, flashes on new discovery ── */}
      <DiscoveryToast notebook={notebook} />

      {/* ── Slide-in intelligence panel ── */}
      <IntelPanel
        open={panelOpen}
        onClose={() => setPanelOpen(false)}
        notebook={notebook}
        transitions={transitions}
        toolEvents={toolEvents}
      />
    </div>
  );
}

// ── Discovery toast — shows the latest notebook entry briefly ─────────────────

function DiscoveryToast({ notebook }: { notebook: NotebookEntryEvent[] }) {
  const [visible, setVisible] = useState(false);
  const [entry, setEntry]     = useState<NotebookEntryEvent | null>(null);

  useEffect(() => {
    if (notebook.length === 0) return;
    const latest = notebook[notebook.length - 1];
    setEntry(latest);
    setVisible(true);
    const t = setTimeout(() => setVisible(false), 6000);
    return () => clearTimeout(t);
  }, [notebook.length]);

  if (!entry) return null;

  const outcomeColor = {
    discovery:    "#00ff9d",
    rejection:    "#ff3b3b",
    inconclusive: "#6c757d",
  }[entry.outcome] ?? "#3a4a5a";

  return (
    <div
      style={{
        position: "absolute",
        bottom: 24,
        left: 24,
        zIndex: 5,
        maxWidth: 380,
        padding: "12px 16px",
        background: "rgba(10,14,22,0.9)",
        backdropFilter: "blur(12px)",
        border: `1px solid ${outcomeColor}44`,
        borderLeft: `3px solid ${outcomeColor}`,
        borderRadius: "0 6px 6px 0",
        transform: visible ? "translateX(0)" : "translateX(calc(-100% - 24px))",
        transition: "transform 0.3s cubic-bezier(0.4,0,0.2,1)",
        fontFamily: '"JetBrains Mono", "Fira Code", monospace',
      }}
    >
      <div
        style={{
          fontSize: 9,
          color: outcomeColor,
          letterSpacing: "0.1em",
          marginBottom: 5,
          fontWeight: 700,
        }}
      >
        {entry.outcome.toUpperCase()}
      </div>
      <div
        style={{
          fontSize: 11,
          color: "#8ab0a0",
          lineHeight: 1.6,
          display: "-webkit-box",
          WebkitLineClamp: 3,
          WebkitBoxOrient: "vertical",
          overflow: "hidden",
        }}
      >
        {entry.entry}
      </div>
    </div>
  );
}
