/// Two-panel audit chain explorer.
///
/// Left  — visual chain: EventBlock cards connected by HashLink connectors.
///          Filter bar: action, decision, since.
///          Subscribes to WebSocket for live updates.
///
/// Right — raw log panel: formatted JSONL, monospace, line-numbered.
///          Clicking a block highlights the corresponding line (and vice-versa).
///          "Copy line" and "Download full log" buttons.

import { useState, useEffect, useRef, useCallback } from "react";
import { EVENTS } from "../types";
import { eventBus } from "../eventBus";
import EventBlock, { AuditEntry } from "../components/chain/EventBlock";
import HashLink from "../components/chain/HashLink";

const API = import.meta.env.DEV ? "http://localhost:3000/api" : "/api";

// ── Helpers ───────────────────────────────────────────────────────────────────

function isChainBroken(a: AuditEntry, b: AuditEntry): boolean {
  // b is older (lower in the list); a.prev_hash should equal b.entry_hash
  if (!a.prev_hash || !b.entry_hash) return false;
  return a.prev_hash !== b.entry_hash;
}

// ── Main component ────────────────────────────────────────────────────────────

export default function ChainExplorer() {
  const [entries, setEntries]         = useState<AuditEntry[]>([]);
  const [loading, setLoading]         = useState(true);
  const [filterAction, setFilterAction] = useState("");
  const [filterDecision, setFilterDecision] = useState("");
  const [selectedLine, setSelectedLine] = useState<number | null>(null);
  const [rawLog, setRawLog]           = useState<string[]>([]);
  const logRef   = useRef<HTMLDivElement>(null);
  const lineRefs = useRef<(HTMLDivElement | null)[]>([]);

  // ── Load initial audit data ─────────────────────────────────────────────────
  useEffect(() => {
    const params = new URLSearchParams({ limit: "200" });
    fetch(`${API}/audit?${params}`)
      .then((r) => r.json())
      .then((data: AuditEntry[]) => {
        setEntries(data);
        setLoading(false);
      })
      .catch(() => setLoading(false));

    // Raw log for right panel
    fetch(`${API}/audit/raw`)
      .then((r) => r.text())
      .then((text) => {
        setRawLog(text.split("\n").filter((l) => l.trim().length > 0));
      })
      .catch(() => {});
  }, []);

  // ── Refresh helper ──────────────────────────────────────────────────────────
  const refresh = useCallback(() => {
    const params = new URLSearchParams({ limit: "200" });
    fetch(`${API}/audit?${params}`)
      .then((r) => r.json())
      .then((data: AuditEntry[]) => setEntries(data))
      .catch(() => {});
    fetch(`${API}/audit/raw`)
      .then((r) => r.text())
      .then((text) => setRawLog(text.split("\n").filter((l) => l.trim().length > 0)))
      .catch(() => {});
  }, []);

  // ── Subscribe to WebSocket tool events → refresh audit data ─────────────────
  useEffect(() => {
    // Whenever a tool executes or a state transition happens, new audit entries
    // may have been written. Re-fetch the audit log from the API.
    const unsubs = [
      eventBus.listen(EVENTS.TOOL_EXECUTION, () => refresh()),
      eventBus.listen(EVENTS.STATE_TRANSITION, () => refresh()),
    ];
    // Also poll every 30s as a safety net
    const poll = setInterval(refresh, 30_000);
    return () => { unsubs.forEach((fn) => fn()); clearInterval(poll); };
  }, [refresh]);

  // ── Filtering ───────────────────────────────────────────────────────────────
  const filtered = entries.filter((e) => {
    if (filterAction && e.action !== filterAction) return false;
    if (filterDecision && e.decision !== filterDecision) return false;
    return true;
  });

  // Newest first for display
  const displayed = [...filtered].reverse();

  // ── Cross-panel selection ───────────────────────────────────────────────────
  const handleBlockClick = useCallback((lineIndex: number) => {
    setSelectedLine(lineIndex);
    // Scroll right panel to the corresponding line
    const el = lineRefs.current[lineIndex];
    if (el) el.scrollIntoView({ behavior: "smooth", block: "center" });
  }, []);

  const handleLineClick = useCallback((lineIndex: number) => {
    setSelectedLine(lineIndex);
  }, []);

  // ── Unique action types for filter dropdown ─────────────────────────────────
  const actionTypes = Array.from(new Set(entries.map((e) => e.action).filter(Boolean))) as string[];

  // ── Download full raw log ───────────────────────────────────────────────────
  const downloadRaw = () => {
    const blob = new Blob([rawLog.join("\n")], { type: "text/plain" });
    const url  = URL.createObjectURL(blob);
    const a    = document.createElement("a");
    a.href = url; a.download = "audit.jsonl"; a.click();
    URL.revokeObjectURL(url);
  };

  return (
    <div style={{
      flex: 1, display: "flex", overflow: "hidden", minHeight: 0,
      background: "#070912",
    }}>
      {/* ── LEFT PANEL: visual chain ─────────────────────────────────────── */}
      <div style={{
        width: "52%", display: "flex", flexDirection: "column",
        overflow: "hidden", borderRight: "1px solid #111824",
      }}>
        {/* Filter bar */}
        <div style={{
          padding: "12px 18px", borderBottom: "1px solid #0e1520",
          display: "flex", alignItems: "center", gap: 10, flexShrink: 0,
          flexWrap: "wrap",
        }}>
          <Label>FILTER</Label>

          <select
            value={filterAction}
            onChange={(e) => setFilterAction(e.target.value)}
            style={selectStyle}
          >
            <option value="">all actions</option>
            {actionTypes.map((a) => (
              <option key={a} value={a}>{a}</option>
            ))}
          </select>

          <select
            value={filterDecision}
            onChange={(e) => setFilterDecision(e.target.value)}
            style={selectStyle}
          >
            <option value="">all decisions</option>
            <option value="allow">allow</option>
            <option value="deny">deny</option>
          </select>

          <span style={{ marginLeft: "auto", fontSize: 9, color: "#1a3040" }}>
            {displayed.length} / {entries.length}
          </span>
        </div>

        {/* Chain scroll area */}
        <div style={{ flex: 1, overflowY: "auto", padding: "16px 18px 24px" }}>
          {loading ? (
            <div style={{ fontSize: 10, color: "#1a3040", padding: "24px 0" }}>
              Loading audit log…
            </div>
          ) : displayed.length === 0 ? (
            <div style={{ fontSize: 10, color: "#1a3040", padding: "24px 0" }}>
              No events match the current filter.
            </div>
          ) : (
            displayed.map((entry, i) => {
              const lineIndex = entries.indexOf(entry);
              const next = displayed[i + 1];
              const broken = next ? isChainBroken(entry, next) : false;
              return (
                <div key={lineIndex} style={{ display: "flex", flexDirection: "column" }}>
                  <EventBlock
                    entry={entry}
                    lineIndex={lineIndex}
                    highlighted={selectedLine === lineIndex}
                    onClick={handleBlockClick}
                  />
                  {i < displayed.length - 1 && (
                    <div style={{ display: "flex", justifyContent: "center" }}>
                      <HashLink
                        prevHash={entry.prev_hash ?? ""}
                        entryHash={entry.entry_hash ?? ""}
                        broken={broken}
                      />
                    </div>
                  )}
                </div>
              );
            })
          )}
        </div>
      </div>

      {/* ── RIGHT PANEL: raw log ─────────────────────────────────────────── */}
      <div style={{
        flex: 1, display: "flex", flexDirection: "column", overflow: "hidden",
      }}>
        {/* Raw log header */}
        <div style={{
          padding: "12px 18px", borderBottom: "1px solid #0e1520",
          display: "flex", alignItems: "center", gap: 10, flexShrink: 0,
        }}>
          <Label>RAW LOG</Label>
          <span style={{ fontSize: 9, color: "#1a3040" }}>{rawLog.length} lines</span>
          <button onClick={downloadRaw} style={btnStyle}>
            ↓ download
          </button>
        </div>

        {/* Log lines */}
        <div
          ref={logRef}
          style={{
            flex: 1, overflowY: "auto",
            fontFamily: '"JetBrains Mono", "Fira Code", monospace',
            fontSize: 10, lineHeight: 1.7,
            padding: "8px 0",
          }}
        >
          {rawLog.map((line, i) => (
            <div
              key={i}
              ref={(el) => { lineRefs.current[i] = el; }}
              onClick={() => handleLineClick(i)}
              style={{
                display: "flex", alignItems: "flex-start", gap: 0,
                background: selectedLine === i ? "#0f1a26" : "transparent",
                borderLeft: selectedLine === i ? "2px solid #00d4ff" : "2px solid transparent",
                cursor: "pointer",
                transition: "background 0.1s",
              }}
            >
              {/* Line number */}
              <span style={{
                minWidth: 40, textAlign: "right", paddingRight: 12,
                color: "#1a3040", userSelect: "none", flexShrink: 0,
                paddingTop: 1,
              }}>
                {i + 1}
              </span>

              {/* Line content */}
              <span style={{
                color: selectedLine === i ? "#7ab8cc" : "#3a6070",
                flex: 1, overflowX: "hidden",
                whiteSpace: "pre-wrap", wordBreak: "break-all",
              }}>
                {line}
              </span>

              {/* Copy button (shown on hover via JS workaround — always rendered small) */}
              <button
                onClick={(e) => { e.stopPropagation(); navigator.clipboard.writeText(line); }}
                title="Copy line"
                style={{
                  ...btnStyle,
                  marginRight: 8, flexShrink: 0, opacity: 0.4,
                  fontSize: 8, padding: "1px 4px",
                }}
              >
                copy
              </button>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

// ── Style helpers ─────────────────────────────────────────────────────────────

function Label({ children }: { children: React.ReactNode }) {
  return (
    <span style={{
      fontSize: 9, letterSpacing: "0.14em", color: "#1a3a4a", fontWeight: 700,
    }}>
      {children}
    </span>
  );
}

const selectStyle: React.CSSProperties = {
  background: "#0d1824",
  border: "1px solid #1a2a3a",
  borderRadius: 3,
  color: "#5a8090",
  fontSize: 9,
  padding: "3px 8px",
  fontFamily: '"JetBrains Mono", "Fira Code", monospace',
  cursor: "pointer",
  outline: "none",
};

const btnStyle: React.CSSProperties = {
  background: "#0d1824",
  border: "1px solid #1a2a3a",
  borderRadius: 3,
  color: "#3a6070",
  fontSize: 9,
  padding: "3px 8px",
  fontFamily: '"JetBrains Mono", "Fira Code", monospace',
  cursor: "pointer",
  letterSpacing: "0.06em",
};
