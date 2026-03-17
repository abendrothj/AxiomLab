import { useState, useEffect, useRef } from "react";

import {
  EVENTS, StateTransitionEvent, ToolExecutionEvent, LlmTokenEvent,
  NotebookEntryEvent, Stage, STAGE_COLORS,
} from "./types";
import { eventBus } from "./eventBus";
import BlueprintGraph from "./components/BlueprintGraph";

// ── API ───────────────────────────────────────────────────────────────────────

const API = import.meta.env.DEV ? "http://localhost:3000/api" : "/api";

async function apiStatus() {
  return fetch(`${API}/status`).then((r) => r.json()).catch(() => ({}));
}

async function apiHistory(): Promise<{
  notebook:    NotebookEntryEvent[];
  transitions: StateTransitionEvent[];
  tools:       ToolExecutionEvent[];
}> {
  return fetch(`${API}/history`).then((r) => r.json()).catch(() => ({ notebook: [], transitions: [], tools: [] }));
}

// ── Tool labels & summaries (shared helpers) ──────────────────────────────────

const TOOL_LABELS: Record<string, string> = {
  move_arm: "ARM", dispense: "PUMP", aspirate: "PUMP",
  transfer: "TRANSFER", mix: "MIX", grip: "GRIP",
  centrifuge: "CENTRIFUGE", read_absorbance: "SPECTRO",
  read_ph: "pH PROBE", read_temperature: "THERMOMETER",
  read_sensor: "SENSOR", set_temperature: "HEATER",
  set_pressure: "PRESSURE", set_stir_rate: "STIRRER",
};

function summarise(tool: string, params: Record<string, unknown>, status: string): string {
  const ok = status === "success";
  switch (tool) {
    case "move_arm":         return ok ? "Arm repositioned" : "Arm move out of range";
    case "dispense":         return ok ? `Dispensed ${params["volume_ul"]}µL` : "Dispense rejected — volume exceeds limit";
    case "aspirate":         return ok ? `Aspirated ${params["volume_ul"]}µL` : "Aspirate rejected";
    case "transfer":         return ok ? "Liquid transferred" : "Transfer rejected";
    case "mix":              return ok ? `Mixed at ${params["rpm"]} rpm` : "Mix rejected";
    case "grip":             return ok ? "Labware gripped" : "Grip failed";
    case "centrifuge":       return ok ? `Centrifuge at ${params["rpm"]} rpm` : "Centrifuge rejected";
    case "read_absorbance":  return ok ? "Absorbance measured" : "Absorbance read failed";
    case "read_ph":          return ok ? "pH measured" : "pH read failed";
    case "read_temperature": return ok ? "Temperature read" : "Temperature read failed";
    case "read_sensor":      return ok ? "Sensor read" : "Sensor read failed";
    case "set_temperature":  return ok ? "Temperature set" : "Temperature rejected — out of range";
    case "set_pressure":     return ok ? "Pressure set" : "Pressure rejected — out of range";
    case "set_stir_rate":    return ok ? "Stir rate set" : "Stir rate rejected";
    default:                 return ok ? "Action completed" : "Action rejected";
  }
}

// ── Root component ────────────────────────────────────────────────────────────

export default function App() {
  const [stage, setStage]           = useState<Stage>("");
  const [iteration, setIteration]   = useState(0);
  const [connected, setConnected]   = useState(false);
  const [thinking, setThinking]     = useState(false);
  const [toolEvents, setToolEvents]     = useState<ToolExecutionEvent[]>([]);
  const [notebook, setNotebook]         = useState<NotebookEntryEvent[]>([]);
  const [transitions, setTransitions]   = useState<StateTransitionEvent[]>([]);
  const thinkTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    // Load full history from DB first — survives server restarts
    apiHistory().then((h) => {
      if (h.notebook.length > 0)    setNotebook(h.notebook);
      if (h.transitions.length > 0) setTransitions(h.transitions);
      if (h.tools.length > 0)       setToolEvents([...h.tools].reverse());
    });

    // Current iteration from in-memory status
    apiStatus().then((s) => {
      if (s.iteration) setIteration(s.iteration);
    });

    const connPoll = setInterval(() => {
      const bus = eventBus as unknown as { ws: WebSocket | null };
      setConnected(bus.ws?.readyState === WebSocket.OPEN);
    }, 1500);
    return () => clearInterval(connPoll);
  }, []);

  useEffect(() => {
    return eventBus.listen<{ running: boolean; iteration: number; notebook: NotebookEntryEvent[] }>(
      "snapshot", (snap) => {
        setIteration(snap.iteration);
        // DB history is already loaded — only fall back to snapshot if DB was empty
        setNotebook((prev) => prev.length > 0 ? prev : (snap.notebook ?? []));
        setConnected(true);
      }
    );
  }, []);

  useEffect(() => {
    const unsubs = [
      eventBus.listen<LlmTokenEvent>(EVENTS.LLM_TOKEN, () => {
        setThinking(true);
        if (thinkTimer.current) clearTimeout(thinkTimer.current);
        thinkTimer.current = setTimeout(() => setThinking(false), 2500);
      }),
      eventBus.listen<StateTransitionEvent>(EVENTS.STATE_TRANSITION, (p) => {
        setStage(p.to as Stage);
        setTransitions((prev) => [...prev, p]);
        if (p.to === "Proposed") setIteration((n) => n + 1);
        if (p.to !== "Executing") setThinking(false);
      }),
      eventBus.listen<ToolExecutionEvent>(EVENTS.TOOL_EXECUTION, (p) => {
        setToolEvents((prev) => [p, ...prev].slice(0, 60));
        setThinking(false);
      }),
      eventBus.listen<NotebookEntryEvent>(EVENTS.NOTEBOOK_ENTRY, (p) => {
        setNotebook((prev) => [...prev, p]);
      }),
    ];
    return () => unsubs.forEach((fn) => fn());
  }, []);

  const stageColor = STAGE_COLORS[stage] ?? "#3a4a5a";

  return (
    <div style={{
      width: "100%", height: "100%",
      display: "flex", flexDirection: "column",
      background: "#070912",
      fontFamily: '"JetBrains Mono", "Fira Code", monospace',
      color: "#e2e8f0",
      overflow: "hidden",
    }}>
      <Header stage={stage} stageColor={stageColor} iteration={iteration} connected={connected} />

      <div style={{ flex: 1, display: "flex", overflow: "hidden", minHeight: 0 }}>
        <LivePanel
          toolEvents={toolEvents}
          stage={stage}
          stageColor={stageColor}
          thinking={thinking}
        />
        <div style={{ width: 1, background: "#111824", flexShrink: 0 }} />

        {/* Blueprint */}
        <div style={{ width: "30%", display: "flex", flexDirection: "column", overflow: "hidden", minHeight: 0 }}>
          <div style={{ padding: "18px 16px 14px", borderBottom: "1px solid #0e1520", flexShrink: 0 }}>
            <span style={{ fontSize: 11, fontWeight: 700, letterSpacing: "0.14em", color: "#e2e8f0" }}>BLUEPRINT</span>
          </div>
          <div style={{ flex: 1, minHeight: 0 }}>
            <BlueprintGraph transitions={transitions} />
          </div>
        </div>

        <div style={{ width: 1, background: "#111824", flexShrink: 0 }} />
        <DiscoveriesPanel notebook={notebook} />
      </div>
    </div>
  );
}

// ── Header ────────────────────────────────────────────────────────────────────

function Header({ stage, stageColor, iteration, connected }: {
  stage: string; stageColor: string; iteration: number; connected: boolean;
}) {
  return (
    <div style={{
      height: 52,
      display: "flex",
      alignItems: "center",
      justifyContent: "space-between",
      padding: "0 24px",
      borderBottom: "1px solid #111824",
      background: "rgba(7,9,18,0.98)",
      flexShrink: 0,
      gap: 16,
    }}>
      {/* Logo */}
      <div style={{
        display: "flex", alignItems: "center", gap: 10,
        fontSize: 13, fontWeight: 700, letterSpacing: "0.14em", color: "#00d4ff",
      }}>
        <svg width="18" height="18" viewBox="0 0 22 22" fill="none">
          <polygon points="11,2 20,7 20,15 11,20 2,15 2,7" stroke="#00d4ff" strokeWidth="1.5" fill="none" />
          <circle cx="11" cy="11" r="3" fill="#00d4ff" opacity="0.85" />
        </svg>
        AXIOMLAB
      </div>

      {/* Center: stage + iteration */}
      <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
        {iteration > 0 && (
          <span style={{
            fontSize: 10, color: "#2a4a6a", letterSpacing: "0.1em",
            background: "rgba(10,20,35,0.6)", padding: "3px 10px",
            borderRadius: 3, border: "1px solid #1a2a3a",
          }}>
            ITER {iteration}
          </span>
        )}
        <div style={{
          display: "flex", alignItems: "center", gap: 7,
          padding: "4px 12px",
          border: `1px solid ${stageColor}33`,
          borderRadius: 3,
          background: `${stageColor}0a`,
          fontSize: 11, letterSpacing: "0.08em", color: stageColor,
        }}>
          <span style={{
            width: 6, height: 6, borderRadius: "50%", background: stageColor,
            boxShadow: stage ? `0 0 7px ${stageColor}` : "none",
          }} />
          {stage || "INITIALISING"}
        </div>
      </div>

      {/* Connection */}
      <div style={{
        display: "flex", alignItems: "center", gap: 6,
        fontSize: 9, letterSpacing: "0.1em",
        color: connected ? "#1a5a3a" : "#5a1a1a",
      }}>
        <span style={{
          width: 5, height: 5, borderRadius: "50%",
          background: connected ? "#00d4ff" : "#ff3b3b",
          boxShadow: connected ? "0 0 5px #00d4ff" : "none",
        }} />
        {connected ? "LIVE" : "CONNECTING"}
      </div>
    </div>
  );
}

// ── Live Panel (left) ─────────────────────────────────────────────────────────

function LivePanel({ toolEvents, stage, stageColor, thinking }: {
  toolEvents: ToolExecutionEvent[];
  stage: string;
  stageColor: string;
  thinking: boolean;
}) {
  return (
    <div style={{
      width: "38%",
      display: "flex",
      flexDirection: "column",
      overflow: "hidden",
      background: "#070912",
    }}>
      {/* Section: current state */}
      <div style={{
        padding: "20px 22px 16px",
        borderBottom: "1px solid #0e1520",
        flexShrink: 0,
      }}>
        <SectionLabel>CURRENT STATE</SectionLabel>
        <div style={{
          marginTop: 10,
          padding: "14px 16px",
          background: "#0c1018",
          border: `1px solid ${stageColor}22`,
          borderLeft: `3px solid ${stageColor}`,
          borderRadius: "0 4px 4px 0",
        }}>
          <div style={{
            fontSize: 16, fontWeight: 700, letterSpacing: "0.06em",
            color: stageColor, lineHeight: 1,
          }}>
            {stage || "INITIALISING"}
          </div>
          <div style={{ marginTop: 10, height: 18, display: "flex", alignItems: "center" }}>
            {thinking ? <ThinkingDots /> : (
              <span style={{ fontSize: 10, color: "#1a3040", letterSpacing: "0.06em" }}>idle</span>
            )}
          </div>
        </div>
      </div>

      {/* Section: activity feed — all events, newest first */}
      <div style={{
        flex: 1, display: "flex", flexDirection: "column", overflow: "hidden",
        padding: "16px 22px 0",
        minHeight: 0,
      }}>
        <SectionLabel>ACTIVITY LOG</SectionLabel>
        <div style={{ flex: 1, overflowY: "auto", marginTop: 8, paddingBottom: 16 }}>
          {toolEvents.length === 0 ? (
            <div style={{ fontSize: 10, color: "#1a3040", padding: "6px 0" }}>
              waiting for activity...
            </div>
          ) : (
            toolEvents.map((ev, i) => (
              <ToolCard key={i} event={ev} prominent={i === 0} />
            ))
          )}
        </div>
      </div>
    </div>
  );
}

// ── Tool card ─────────────────────────────────────────────────────────────────

function ToolCard({ event: ev, prominent = false }: { event: ToolExecutionEvent; prominent?: boolean }) {
  const ok = ev.status === "success";
  const label = TOOL_LABELS[ev.tool] ?? ev.tool.toUpperCase();
  const summary = summarise(ev.tool, ev.params, ev.status);
  const dotColor = ok ? "#00d4ff" : "#ff4444";

  return (
    <div style={{
      display: "flex", alignItems: "center", gap: 10,
      padding: prominent ? "10px 12px" : "7px 0",
      marginBottom: prominent ? "8px" : 0,
      background: prominent ? "#0c1018" : "transparent",
      border: prominent ? `1px solid ${ok ? "#00d4ff18" : "#ff444420"}` : "none",
      borderBottom: prominent ? `1px solid ${ok ? "#00d4ff18" : "#ff444420"}` : "1px solid #0a1018",
      borderRadius: prominent ? 4 : 0,
      animation: prominent ? "fadeSlideIn 0.2s ease-out" : "none",
      flexShrink: 0,
    }}>
      <span style={{
        width: prominent ? 7 : 5,
        height: prominent ? 7 : 5,
        borderRadius: "50%",
        background: dotColor,
        boxShadow: `0 0 ${prominent ? 6 : 4}px ${dotColor}${prominent ? "99" : "55"}`,
        flexShrink: 0,
      }} />
      <span style={{
        fontSize: 9,
        color: ok ? "#1a5a6a" : "#5a2020",
        letterSpacing: "0.08em",
        minWidth: 76,
        flexShrink: 0,
      }}>
        {label}
      </span>
      <span style={{
        fontSize: prominent ? 12 : 11,
        color: ok ? (prominent ? "#a0ccd8" : "#5a8090") : (prominent ? "#b07070" : "#7a4a4a"),
        flex: 1,
        overflow: "hidden",
        textOverflow: "ellipsis",
        whiteSpace: "nowrap",
      }}>
        {summary}
      </span>
    </div>
  );
}

// ── Thinking dots ─────────────────────────────────────────────────────────────

function ThinkingDots() {
  return (
    <div style={{ display: "flex", alignItems: "center", gap: 5 }}>
      {[0, 1, 2, 3, 4].map((i) => (
        <span key={i} style={{
          width: 4, height: 4, borderRadius: "50%",
          background: "#00d4ff",
          display: "inline-block",
          animation: `pulse-dot 1.2s ease-in-out ${i * 0.18}s infinite`,
        }} />
      ))}
      <span style={{ fontSize: 9, color: "#2a6a8a", letterSpacing: "0.1em", marginLeft: 6 }}>
        PROCESSING
      </span>
    </div>
  );
}

// ── Journal panel (right) ────────────────────────────────────────────────────

function DiscoveriesPanel({ notebook }: { notebook: NotebookEntryEvent[] }) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const prevLen = useRef(0);

  useEffect(() => {
    if (notebook.length > prevLen.current) {
      prevLen.current = notebook.length;
      if (scrollRef.current) scrollRef.current.scrollTop = 0;
    }
  }, [notebook.length]);

  const reversed = [...notebook].reverse();

  return (
    <div style={{
      flex: 1,
      display: "flex",
      flexDirection: "column",
      overflow: "hidden",
      background: "#070912",
    }}>
      <div style={{
        padding: "18px 24px 14px",
        borderBottom: "1px solid #0e1520",
        flexShrink: 0,
        display: "flex",
        alignItems: "baseline",
        gap: 12,
      }}>
        <span style={{ fontSize: 11, fontWeight: 700, letterSpacing: "0.14em", color: "#e2e8f0" }}>
          JOURNAL
        </span>
        {notebook.length > 0 && (
          <span style={{ fontSize: 10, color: "#1a3a4a" }}>
            {notebook.length} {notebook.length === 1 ? "entry" : "entries"}
          </span>
        )}
      </div>

      <div ref={scrollRef} style={{ flex: 1, overflowY: "auto", padding: "16px 24px" }}>
        {reversed.length === 0 ? (
          <EmptyState />
        ) : (
          <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
            {reversed.map((entry, i) => (
              <JournalCard key={reversed.length - 1 - i} entry={entry} fresh={i === 0} />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

// ── Journal card ──────────────────────────────────────────────────────────────

function JournalCard({ entry, fresh }: { entry: NotebookEntryEvent; fresh: boolean }) {
  const elapsed = (() => {
    const secs = Math.floor((Date.now() - entry.timestamp_ms) / 1000);
    if (secs < 60) return `${secs}s ago`;
    return `${Math.floor(secs / 60)}m ago`;
  })();

  return (
    <div style={{
      padding: "16px 18px",
      background: "#0b0e18",
      border: "1px solid #111824",
      borderLeft: "3px solid #1a3a50",
      borderRadius: "0 6px 6px 0",
      animation: fresh ? "fadeSlideIn 0.25s ease-out" : "none",
    }}>
      <div style={{
        display: "flex", alignItems: "center",
        justifyContent: "space-between",
        marginBottom: 10,
      }}>
        {entry.tool_that_triggered ? (
          <span style={{
            fontSize: 9, color: "#1a3a4a", background: "#0d1824",
            padding: "2px 7px", borderRadius: 2, letterSpacing: "0.06em",
          }}>
            {entry.tool_that_triggered}
          </span>
        ) : <span />}
        <span style={{ fontSize: 9, color: "#1a3040" }}>{elapsed}</span>
      </div>
      <p style={{
        fontSize: 13, color: "#9ab0bc", lineHeight: 1.7,
        whiteSpace: "pre-wrap", wordBreak: "break-word", margin: 0,
      }}>
        {entry.entry}
      </p>
    </div>
  );
}

// ── Small helpers ─────────────────────────────────────────────────────────────

function SectionLabel({ children }: { children: React.ReactNode }) {
  return (
    <div style={{
      fontSize: 9, letterSpacing: "0.14em", color: "#1a3a4a", fontWeight: 700,
    }}>
      {children}
    </div>
  );
}


function EmptyState() {
  return (
    <div style={{
      padding: "48px 0",
      display: "flex",
      flexDirection: "column",
      alignItems: "center",
      gap: 14,
      color: "#1a3040",
    }}>
      <svg width="32" height="32" viewBox="0 0 32 32" fill="none" opacity={0.4}>
        <circle cx="16" cy="16" r="14" stroke="#00d4ff" strokeWidth="1.5" />
        <path d="M16 10v6l4 2" stroke="#00d4ff" strokeWidth="1.5" strokeLinecap="round" />
      </svg>
      <div style={{ fontSize: 11, letterSpacing: "0.08em" }}>
        Waiting for discoveries...
      </div>
      <div style={{ fontSize: 10, color: "#122030", maxWidth: 240, textAlign: "center", lineHeight: 1.6 }}>
        The AI will document its findings here as it explores the constraint space.
      </div>
    </div>
  );
}
