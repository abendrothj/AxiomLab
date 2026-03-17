import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "reactflow/dist/style.css";

import { EVENTS, StateTransitionEvent, ToolExecutionEvent, LlmTokenEvent, NotebookEntryEvent, Stage, STAGE_COLORS } from "./types";
import Sandbox3D from "./components/Sandbox3D";
import BlueprintGraph from "./components/BlueprintGraph";
import TerminalLog from "./components/TerminalLog";
import LabNotebook from "./components/LabNotebook";
import ToolEventFeed from "./components/ToolEventFeed";

// ── Styles ────────────────────────────────────────────────────────────────────

const css = {
  app: {
    display: "grid",
    gridTemplateRows: "48px 1fr",
    width: "100%",
    height: "100%",
    background: "#0a0b0d",
    color: "#00ff9d",
    fontFamily: '"JetBrains Mono", "Fira Code", monospace',
    overflow: "hidden",
  } as React.CSSProperties,

  header: {
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
    padding: "0 20px",
    background: "#0f1117",
    borderBottom: "1px solid #1a2035",
    flexShrink: 0,
  } as React.CSSProperties,

  logo: {
    display: "flex",
    alignItems: "center",
    gap: 10,
    fontSize: 14,
    fontWeight: 700,
    letterSpacing: "0.12em",
    color: "#00ff9d",
  } as React.CSSProperties,

  logoGlyph: {
    width: 22,
    height: 22,
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
  } as React.CSSProperties,

  headerCenter: {
    display: "flex",
    alignItems: "center",
    gap: 12,
  } as React.CSSProperties,

  bootBtn: (running: boolean) => ({
    display: "flex",
    alignItems: "center",
    gap: 8,
    padding: "7px 20px",
    background: running ? "transparent" : "#00ff9d18",
    border: `1px solid ${running ? "#1a2035" : "#00ff9d"}`,
    borderRadius: 3,
    color: running ? "#3a4a5a" : "#00ff9d",
    fontFamily: '"JetBrains Mono", monospace',
    fontSize: 12,
    fontWeight: 700,
    letterSpacing: "0.1em",
    cursor: running ? "not-allowed" : "pointer",
    transition: "all 0.15s",
  } as React.CSSProperties),

  stageBadge: (stage: Stage) => ({
    display: "flex",
    alignItems: "center",
    gap: 7,
    padding: "4px 12px",
    border: `1px solid ${STAGE_COLORS[stage] ?? "#3a4a5a"}`,
    borderRadius: 3,
    fontSize: 11,
    letterSpacing: "0.08em",
    color: STAGE_COLORS[stage] ?? "#3a4a5a",
  } as React.CSSProperties),

  stageDot: (stage: Stage) => ({
    width: 6,
    height: 6,
    borderRadius: "50%",
    background: STAGE_COLORS[stage] ?? "#3a4a5a",
    boxShadow: stage ? `0 0 5px ${STAGE_COLORS[stage]}` : "none",
  } as React.CSSProperties),

  body: {
    display: "grid",
    gridTemplateColumns: "60fr 40fr",
    overflow: "hidden",
    minHeight: 0,
  } as React.CSSProperties,

  leftCol: {
    display: "grid",
    gridTemplateRows: "55fr 25fr 20fr",
    borderRight: "1px solid #1a2035",
    overflow: "hidden",
    minHeight: 0,
  } as React.CSSProperties,

  rightCol: {
    display: "grid",
    gridTemplateRows: "55fr 45fr",
    overflow: "hidden",
    minHeight: 0,
  } as React.CSSProperties,

  panel: {
    overflow: "hidden",
    minHeight: 0,
    position: "relative",
  } as React.CSSProperties,

  panelDivider: {
    borderTop: "1px solid #1a2035",
  } as React.CSSProperties,
};

// ── Component ─────────────────────────────────────────────────────────────────

export default function App() {
  const [stage, setStage] = useState<Stage>("");
  const [running, setRunning] = useState(false);
  const [tokens, setTokens] = useState<string>("");
  const [toolEvents, setToolEvents] = useState<ToolExecutionEvent[]>([]);
  const [notebookEntries, setNotebookEntries] = useState<NotebookEntryEvent[]>([]);
  const [transitions, setTransitions] = useState<StateTransitionEvent[]>([]);
  const [latestTool, setLatestTool] = useState<ToolExecutionEvent | null>(null);

  const boot = useCallback(async () => {
    if (running) return;
    setRunning(true);
    setTokens("");
    setToolEvents([]);
    setNotebookEntries([]);
    setTransitions([]);
    setStage("Proposed");
    try {
      await invoke("start_simulation");
    } catch (e) {
      console.error("start_simulation failed:", e);
      setRunning(false);
    }
  }, [running]);

  useEffect(() => {
    const unsubs: (() => void)[] = [];

    const cleanup = (p: Promise<() => void>) => {
      p.then((fn) => unsubs.push(fn));
    };

    cleanup(
      listen<LlmTokenEvent>(EVENTS.LLM_TOKEN, (ev) => {
        setTokens((t) => t + ev.payload.token);
      })
    );

    cleanup(
      listen<StateTransitionEvent>(EVENTS.STATE_TRANSITION, (ev) => {
        setStage(ev.payload.to as Stage);
        setTransitions((prev) => [...prev, ev.payload]);
        if (ev.payload.to === "Completed" || ev.payload.to === "Failed") {
          setRunning(false);
        }
      })
    );

    cleanup(
      listen<ToolExecutionEvent>(EVENTS.TOOL_EXECUTION, (ev) => {
        setToolEvents((prev) => [ev.payload, ...prev].slice(0, 50));
        setLatestTool(ev.payload);
      })
    );

    cleanup(
      listen<NotebookEntryEvent>(EVENTS.NOTEBOOK_ENTRY, (ev) => {
        setNotebookEntries((prev) => [...prev, ev.payload]);
      })
    );

    return () => unsubs.forEach((fn) => fn());
  }, []);

  const stageLabel = stage || "IDLE";

  return (
    <div style={css.app}>
      {/* ── Header ─────────────────────────────────────────── */}
      <header style={css.header}>
        <div style={css.logo}>
          <div style={css.logoGlyph}>
            <svg width="22" height="22" viewBox="0 0 22 22" fill="none">
              <polygon
                points="11,2 20,7 20,15 11,20 2,15 2,7"
                stroke="#00ff9d"
                strokeWidth="1.5"
                fill="none"
              />
              <circle cx="11" cy="11" r="3" fill="#00ff9d" opacity="0.8" />
              <line x1="11" y1="2" x2="11" y2="6" stroke="#00ff9d" strokeWidth="1" opacity="0.5" />
              <line x1="11" y1="16" x2="11" y2="20" stroke="#00ff9d" strokeWidth="1" opacity="0.5" />
              <line x1="2" y1="7" x2="5.5" y2="9" stroke="#00ff9d" strokeWidth="1" opacity="0.5" />
              <line x1="16.5" y1="13" x2="20" y2="15" stroke="#00ff9d" strokeWidth="1" opacity="0.5" />
            </svg>
          </div>
          AXIOMLAB
        </div>

        <div style={css.headerCenter}>
          <button style={css.bootBtn(running)} onClick={boot} disabled={running}>
            <span
              style={{
                width: 8,
                height: 8,
                borderRadius: "50%",
                background: running ? "#3a4a5a" : "#00ff9d",
                boxShadow: running ? "none" : "0 0 6px #00ff9d",
                flexShrink: 0,
              }}
            />
            {running ? "RUNNING..." : "BOOT LAB"}
          </button>
        </div>

        <div style={css.stageBadge(stage)}>
          <div style={css.stageDot(stage)} />
          {stageLabel}
        </div>
      </header>

      {/* ── Body ───────────────────────────────────────────── */}
      <div style={css.body}>
        {/* Left column */}
        <div style={css.leftCol}>
          <div style={css.panel}>
            <Sandbox3D latestTool={latestTool} />
          </div>
          <div style={{ ...css.panel, ...css.panelDivider }}>
            <TerminalLog tokens={tokens} />
          </div>
          <div style={{ ...css.panel, ...css.panelDivider }}>
            <ToolEventFeed events={toolEvents} />
          </div>
        </div>

        {/* Right column */}
        <div style={css.rightCol}>
          <div style={css.panel}>
            <BlueprintGraph transitions={transitions} />
          </div>
          <div style={{ ...css.panel, ...css.panelDivider }}>
            <LabNotebook entries={notebookEntries} />
          </div>
        </div>
      </div>
    </div>
  );
}
