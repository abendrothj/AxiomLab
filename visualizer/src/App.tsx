import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { load } from "@tauri-apps/plugin-store";
import "reactflow/dist/style.css";

import {
  EVENTS, StateTransitionEvent, ToolExecutionEvent, LlmTokenEvent,
  NotebookEntryEvent, Stage, STAGE_COLORS,
} from "./types";
import Sandbox3D from "./components/Sandbox3D";
import BlueprintGraph from "./components/BlueprintGraph";
import TerminalLog from "./components/TerminalLog";
import LabNotebook from "./components/LabNotebook";
import ToolEventFeed from "./components/ToolEventFeed";

// ── Persistence ───────────────────────────────────────────────────────────────

const STORE_FILE = "axiomlab-notebook.json";
const STORE_KEY  = "notebook_entries";

async function loadPersistedEntries(): Promise<NotebookEntryEvent[]> {
  try {
    const store = await load(STORE_FILE, { autoSave: false, defaults: {} });
    const saved = await store.get<NotebookEntryEvent[]>(STORE_KEY);
    return saved ?? [];
  } catch {
    return [];
  }
}

async function persistEntries(entries: NotebookEntryEvent[]): Promise<void> {
  try {
    const store = await load(STORE_FILE, { autoSave: false, defaults: {} });
    await store.set(STORE_KEY, entries);
    await store.save();
  } catch (e) {
    console.warn("notebook persist failed:", e);
  }
}

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
    gap: 8,
  } as React.CSSProperties,

  btn: (variant: "boot" | "stop" | "clear") => {
    const colors = {
      boot:  { border: "#00ff9d", color: "#00ff9d", bg: "#00ff9d18" },
      stop:  { border: "#ff3b3b", color: "#ff3b3b", bg: "#ff3b3b18" },
      clear: { border: "#3a4a5a", color: "#3a5a6a", bg: "transparent" },
    }[variant];
    return {
      display: "flex",
      alignItems: "center",
      gap: 7,
      padding: "6px 16px",
      background: colors.bg,
      border: `1px solid ${colors.border}`,
      borderRadius: 3,
      color: colors.color,
      fontFamily: '"JetBrains Mono", monospace',
      fontSize: 11,
      fontWeight: 700,
      letterSpacing: "0.1em",
      cursor: "pointer",
    } as React.CSSProperties;
  },

  iterBadge: {
    padding: "4px 10px",
    border: "1px solid #1a2035",
    borderRadius: 3,
    fontSize: 10,
    color: "#3a6a8a",
    letterSpacing: "0.08em",
  } as React.CSSProperties,

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
  const [iteration, setIteration] = useState(0);
  const [tokens, setTokens] = useState("");
  const [toolEvents, setToolEvents] = useState<ToolExecutionEvent[]>([]);
  const [notebookEntries, setNotebookEntries] = useState<NotebookEntryEvent[]>([]);
  const [transitions, setTransitions] = useState<StateTransitionEvent[]>([]);
  const [latestTool, setLatestTool] = useState<ToolExecutionEvent | null>(null);
  const entriesRef = useRef<NotebookEntryEvent[]>([]);

  // ── Load persisted notebook on mount ──
  useEffect(() => {
    loadPersistedEntries().then((saved) => {
      if (saved.length > 0) {
        setNotebookEntries(saved);
        entriesRef.current = saved;
      }
    });
  }, []);

  // ── Boot ──
  const boot = useCallback(async () => {
    if (running) return;
    setRunning(true);
    setTokens("");
    setToolEvents([]);
    setTransitions([]);
    setStage("Proposed");
    setIteration(1);
    try {
      await invoke("start_simulation");
    } catch (e) {
      console.error("start_simulation failed:", e);
      setRunning(false);
    }
  }, [running]);

  // ── Stop ──
  const stop = useCallback(async () => {
    try {
      await invoke("stop_simulation");
    } catch (e) {
      console.error("stop_simulation failed:", e);
    }
    setRunning(false);
    setStage("");
  }, []);

  // ── Clear notebook ──
  const clearNotebook = useCallback(async () => {
    setNotebookEntries([]);
    entriesRef.current = [];
    await persistEntries([]);
  }, []);

  // ── Event listeners ──
  useEffect(() => {
    const unsubs: (() => void)[] = [];
    const cleanup = (p: Promise<() => void>) => p.then((fn) => unsubs.push(fn));

    cleanup(listen<LlmTokenEvent>(EVENTS.LLM_TOKEN, (ev) => {
      setTokens((t) => t + ev.payload.token);
    }));

    cleanup(listen<StateTransitionEvent>(EVENTS.STATE_TRANSITION, (ev) => {
      setStage(ev.payload.to as Stage);
      setTransitions((prev) => [...prev, ev.payload]);
      // Track experiment iterations — each Proposed→... cycle is a new iteration
      if (ev.payload.from === "" || ev.payload.to === "Proposed") {
        setIteration((n) => n + 1);
        setTokens(""); // clear terminal between experiments
      }
    }));

    cleanup(listen<ToolExecutionEvent>(EVENTS.TOOL_EXECUTION, (ev) => {
      setToolEvents((prev) => [ev.payload, ...prev].slice(0, 50));
      setLatestTool(ev.payload);
    }));

    cleanup(listen<NotebookEntryEvent>(EVENTS.NOTEBOOK_ENTRY, (ev) => {
      setNotebookEntries((prev) => {
        const next = [...prev, ev.payload];
        entriesRef.current = next;
        // Persist asynchronously — don't block the render
        persistEntries(next);
        return next;
      });
    }));

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
              <polygon points="11,2 20,7 20,15 11,20 2,15 2,7" stroke="#00ff9d" strokeWidth="1.5" fill="none" />
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
          {!running ? (
            <button style={css.btn("boot")} onClick={boot}>
              <span style={{ width: 7, height: 7, borderRadius: "50%", background: "#00ff9d", boxShadow: "0 0 6px #00ff9d", flexShrink: 0 }} />
              BOOT LAB
            </button>
          ) : (
            <button style={css.btn("stop")} onClick={stop}>
              <span style={{ width: 7, height: 7, borderRadius: "2px", background: "#ff3b3b", flexShrink: 0 }} />
              STOP
            </button>
          )}
          {running && iteration > 0 && (
            <div style={css.iterBadge}>ITER {iteration}</div>
          )}
          <button style={css.btn("clear")} onClick={clearNotebook} title="Clear persisted notebook">
            CLR LOG
          </button>
        </div>

        <div style={css.stageBadge(stage)}>
          <div style={css.stageDot(stage)} />
          {stageLabel}
        </div>
      </header>

      {/* ── Body ───────────────────────────────────────────── */}
      <div style={css.body}>
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
