/// Card representing a single audit event in the chain view.
///
/// Left border colour:
///   green  = allow
///   red    = deny
///   amber  = stalled / pending_dispatch
///   slate  = other (session_start, rekor_checkpoint, …)
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
  sig?: string;
  rekor_uuid?: string;
  zk_tx_hash?: string;
  zk_status?: string;
  [key: string]: unknown;
}

interface EventBlockProps {
  entry: AuditEntry;
  lineIndex: number;
  highlighted: boolean;
  onClick: (lineIndex: number) => void;
}

function borderColor(entry: AuditEntry): string {
  if (entry.action === "pending_dispatch" || entry.action === "stalled_dispatch") return "#c97a00";
  switch (entry.decision) {
    case "allow": return "#00d4ff";
    case "deny":  return "#ff4444";
    default:      return "#1a3a50";
  }
}

function decisionBadge(entry: AuditEntry) {
  if (!entry.decision) return null;
  const color = entry.decision === "allow" ? "#00d4ff" : "#ff4444";
  const bg    = entry.decision === "allow" ? "#001a22" : "#220000";
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

  const hasSig    = Boolean(entry.sig);
  const rekorUuid = entry.rekor_uuid as string | undefined;
  const zkTxHash  = entry.zk_tx_hash as string | undefined;
  const zkStatus  = entry.zk_status as string | undefined;

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
      {(hasSig || rekorUuid || zkTxHash || zkStatus) && (
        <div style={{ marginTop: 6, display: "flex", gap: 6, flexWrap: "wrap" }}>
          {hasSig && (
            <span style={{ fontSize: 8, color: "#00aa80", background: "#001a14",
              border: "1px solid #00aa8030", padding: "1px 5px", borderRadius: 2 }}>
              ✓ sig
            </span>
          )}
          {rekorUuid && (
            <a
              href={`https://rekor.sigstore.dev/api/v1/log/entries?logIndex=${rekorUuid}`}
              target="_blank" rel="noopener noreferrer"
              onClick={(e) => e.stopPropagation()}
              style={{ fontSize: 8, color: "#8080ff", background: "#0d0d2a",
                border: "1px solid #8080ff30", padding: "1px 5px", borderRadius: 2,
                textDecoration: "none" }}
            >
              Rekor {rekorUuid.slice(0, 8)}…
            </a>
          )}
          {zkTxHash && (
            <a
              href={`https://basescan.org/tx/${zkTxHash}`}
              target="_blank" rel="noopener noreferrer"
              onClick={(e) => e.stopPropagation()}
              style={{ fontSize: 8, color: "#a060ff", background: "#15002a",
                border: "1px solid #a060ff30", padding: "1px 5px", borderRadius: 2,
                textDecoration: "none" }}
            >
              ✓ ZK on-chain
            </a>
          )}
          {!zkTxHash && zkStatus === "pending" && (
            <span style={{ fontSize: 8, color: "#808040", background: "#1a1a00",
              border: "1px solid #808040", padding: "1px 5px", borderRadius: 2 }}>
              ⏳ ZK pending
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
