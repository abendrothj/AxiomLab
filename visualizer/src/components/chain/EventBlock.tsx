/// Card representing a single audit event in the chain view.
///
/// Left border colour:
///   cyan   = allow
///   red    = deny
///   amber  = stalled / pending_dispatch / safety gate denial
///   indigo = rekor_checkpoint
///   slate  = other (session_start, …)
///
/// Click to expand the full JSON payload.

import { useState } from "react";

export interface AuditEntry {
  seq?: number;
  unix_secs?: number;
  action?: string;
  decision?: string;
  tool?: string;
  params?: unknown;
  reason?: string;
  entry_hash?: string;
  prev_hash?: string;
  entry_sig_b64?: string;
  rekor_uuid?: string;
  [key: string]: unknown;
}

interface EventBlockProps {
  entry: AuditEntry;
  lineIndex: number;
  highlighted: boolean;
  onClick: (lineIndex: number) => void;
}

function isSafetyGateDeny(entry: AuditEntry): boolean {
  if (entry.decision !== "deny") return false;
  const r = (entry.reason ?? "").toLowerCase();
  return (
    r.includes("sandbox") ||
    r.includes("calibration_required") ||
    r.includes("calibration for") ||
    r.includes("high-risk action denied") ||
    r.includes("approval violation") ||
    r.includes("operator denied") ||
    r.includes("revoked") ||
    r.includes("proof policy") ||
    r.includes("fail-closed")
  );
}

function borderColor(entry: AuditEntry): string {
  if (entry.action === "rekor_checkpoint") return "#6366f1";
  if (entry.action === "pending_dispatch" || entry.action === "stalled_dispatch") return "#c97a00";
  if (isSafetyGateDeny(entry)) return "#f59e0b";
  switch (entry.decision) {
    case "allow": return "#00d4ff";
    case "deny":  return "#ff4444";
    default:      return "#1a3a50";
  }
}

function decisionBadge(entry: AuditEntry) {
  if (!entry.decision) return null;
  const gate = isSafetyGateDeny(entry);
  const color = entry.decision === "allow" ? "#00d4ff" : gate ? "#f59e0b" : "#ff4444";
  const bg    = entry.decision === "allow" ? "#001a22" : gate ? "#1a0e00" : "#220000";
  return (
    <span style={{
      fontSize: 8, letterSpacing: "0.1em",
      color, background: bg,
      border: `1px solid ${color}30`,
      padding: "1px 5px", borderRadius: 2,
    }}>
      {entry.decision.toUpperCase()}
    </span>
  );
}

function formatTime(unix_secs?: number): string {
  if (!unix_secs) return "";
  const d = new Date(unix_secs * 1000);
  return d.toLocaleTimeString("en-GB", { hour12: false });
}

export default function EventBlock({ entry, lineIndex, highlighted, onClick }: EventBlockProps) {
  const [expanded, setExpanded] = useState(false);
  const bc = borderColor(entry);
  const gate = isSafetyGateDeny(entry);

  const hasSig    = Boolean(entry.entry_sig_b64);
  const rekorUuid = entry.rekor_uuid as string | undefined;
  const isRekorCheckpoint = entry.action === "rekor_checkpoint";

  return (
    <div
      onClick={() => { onClick(lineIndex); setExpanded((e) => !e); }}
      style={{
        borderLeft: `3px solid ${bc}`,
        background: highlighted ? "#0f1a26" : "#0b0e18",
        border: `1px solid ${highlighted ? "#1a3a50" : "#111824"}`,
        borderLeftColor: bc,
        borderLeftWidth: 3,
        borderRadius: "0 4px 4px 0",
        padding: "10px 14px",
        cursor: "pointer",
        transition: "background 0.15s",
        flexShrink: 0,
      }}
    >
      {/* Top row */}
      <div style={{ display: "flex", alignItems: "center", gap: 8, flexWrap: "wrap" }}>
        {decisionBadge(entry)}
        {gate && (
          <span style={{
            fontSize: 8, color: "#f59e0b", background: "#1a0e00",
            border: "1px solid #f59e0b40", padding: "1px 5px", borderRadius: 2,
            letterSpacing: "0.1em",
          }}>
            SAFETY GATE
          </span>
        )}
        <span style={{ fontSize: 10, color: "#5a8090", letterSpacing: "0.08em" }}>
          {entry.action ?? "event"}
        </span>
        {entry.tool && (
          <span style={{ fontSize: 9, color: "#1a4a5a", background: "#0d1824",
            padding: "1px 6px", borderRadius: 2, letterSpacing: "0.06em" }}>
            {entry.tool}
          </span>
        )}
        <span style={{ marginLeft: "auto", fontSize: 9, color: "#1a3040" }}>
          {formatTime(entry.unix_secs)}
        </span>
      </div>

      {/* Hash row */}
      <div style={{ marginTop: 5, fontSize: 8, color: "#1a3040", fontFamily: "monospace", letterSpacing: "0.06em" }}>
        <span title={entry.entry_hash}>
          sha: {entry.entry_hash ? entry.entry_hash.slice(0, 8) + "…" + entry.entry_hash.slice(-4) : "—"}
        </span>
        <span style={{ marginLeft: 12 }} title={entry.prev_hash}>
          prev: {entry.prev_hash ? entry.prev_hash.slice(0, 8) + "…" : "genesis"}
        </span>
      </div>

      {/* Badge row */}
      {(hasSig || rekorUuid || isRekorCheckpoint) && (
        <div style={{ marginTop: 6, display: "flex", gap: 6, flexWrap: "wrap" }}>
          {hasSig && (
            <span style={{ fontSize: 8, color: "#00aa80", background: "#001a14",
              border: "1px solid #00aa8030", padding: "1px 5px", borderRadius: 2 }}>
              ✓ signed
            </span>
          )}
          {rekorUuid && (
            <a
              href={`https://rekor.sigstore.dev/api/v1/log/entries/${rekorUuid}`}
              target="_blank" rel="noopener noreferrer"
              onClick={(e) => e.stopPropagation()}
              style={{ fontSize: 8, color: "#818cf8", background: "#0d0d2a",
                border: "1px solid #818cf830", padding: "1px 5px", borderRadius: 2,
                textDecoration: "none" }}
            >
              ⬡ Rekor {rekorUuid.slice(0, 8)}…
            </a>
          )}
          {isRekorCheckpoint && !rekorUuid && (
            <span style={{ fontSize: 8, color: "#6366f1", background: "#0d0d2a",
              border: "1px solid #6366f130", padding: "1px 5px", borderRadius: 2 }}>
              ⬡ rekor checkpoint
            </span>
          )}
        </div>
      )}

      {/* Expanded JSON */}
      {expanded && (
        <pre style={{
          marginTop: 10,
          fontSize: 9,
          color: "#4a7a8a",
          background: "#070912",
          border: "1px solid #0e1824",
          borderRadius: 3,
          padding: "10px 12px",
          overflowX: "auto",
          whiteSpace: "pre-wrap",
          wordBreak: "break-all",
          lineHeight: 1.6,
        }}>
          {JSON.stringify(entry, null, 2)}
        </pre>
      )}
    </div>
  );
}
