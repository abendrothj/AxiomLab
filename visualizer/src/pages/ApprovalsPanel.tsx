import { useState, useEffect, useCallback } from "react";
import { eventBus } from "../eventBus";
import { EVENTS, PendingApprovalInfo, StalledResponse, ToolExecutionEvent } from "../types";

const API = import.meta.env.DEV ? "http://localhost:3000/api" : "/api";

function getToken(): string | null {
  return localStorage.getItem("axiomlab_token");
}

// ── Section label ─────────────────────────────────────────────────────────────

function SectionLabel({ children }: { children: React.ReactNode }) {
  return (
    <div style={{ fontSize: 9, letterSpacing: "0.14em", color: "#1a3a4a", fontWeight: 700 }}>
      {children}
    </div>
  );
}

// ── Risk class badge ──────────────────────────────────────────────────────────

function RiskBadge({ risk }: { risk?: string }) {
  const colors: Record<string, string> = {
    Destructive:    "#ff3b3b",
    Actuation:      "#fd7e14",
    LiquidHandling: "#0d6efd",
    ReadOnly:       "#20c997",
  };
  const color = colors[risk ?? ""] ?? "#3a4a5a";
  return (
    <span style={{
      fontSize: 9, letterSpacing: "0.08em", padding: "2px 7px",
      borderRadius: 2, border: `1px solid ${color}44`,
      color, background: `${color}11`,
    }}>
      {risk ?? "UNKNOWN"}
    </span>
  );
}

// ── Relative time ─────────────────────────────────────────────────────────────

function reltime(secs: number): string {
  const diff = Math.floor(Date.now() / 1000) - secs;
  if (diff < 60) return `${diff}s ago`;
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
  return `${Math.floor(diff / 3600)}h ago`;
}

// ── Approve modal ─────────────────────────────────────────────────────────────

function ApproveModal({
  info,
  onClose,
  onResult,
}: {
  info: PendingApprovalInfo;
  onClose: () => void;
  onResult: (msg: string, ok: boolean) => void;
}) {
  const [bundle, setBundle] = useState("");
  const [busy, setBusy] = useState(false);

  async function submit() {
    setBusy(true);
    let parsed: unknown[] | null = null;
    if (bundle.trim()) {
      try {
        parsed = JSON.parse(bundle);
      } catch {
        onResult("Invalid JSON bundle — check the format.", false);
        setBusy(false);
        return;
      }
    }
    try {
      const res = await fetch(`${API}/approvals/submit`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ pending_id: info.pending_id, bundle: parsed }),
      });
      const json = await res.json();
      if (res.ok) {
        onResult("Approval submitted.", true);
        onClose();
      } else {
        onResult(json.error ?? "Submission failed.", false);
      }
    } catch {
      onResult("Network error.", false);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div style={{
      position: "fixed", inset: 0, background: "rgba(7,9,18,0.88)",
      display: "flex", alignItems: "center", justifyContent: "center", zIndex: 100,
    }} onClick={onClose}>
      <div style={{
        background: "#0c1018", border: "1px solid #1a3a50", borderRadius: 6,
        padding: "28px 28px 24px", width: 480, maxWidth: "92vw",
      }} onClick={(e) => e.stopPropagation()}>
        <div style={{ fontSize: 11, fontWeight: 700, letterSpacing: "0.14em", color: "#e2e8f0", marginBottom: 6 }}>
          APPROVE ACTION
        </div>
        <div style={{ fontSize: 10, color: "#3a5a6a", marginBottom: 18 }}>
          {info.tool_name} — pending_id: {info.pending_id}
        </div>
        {info.session_nonce && (
          <div style={{
            fontSize: 10, color: "#1a4a5a", background: "#080d14",
            border: "1px solid #0f2030", borderRadius: 3, padding: "6px 10px", marginBottom: 14,
            fontFamily: "monospace",
          }}>
            approvalctl sign --pending-id {info.pending_id} --session-nonce {info.session_nonce}
          </div>
        )}
        <div style={{ fontSize: 9, color: "#1a3a4a", marginBottom: 6, letterSpacing: "0.1em" }}>
          APPROVAL BUNDLE JSON (leave empty to deny)
        </div>
        <textarea
          value={bundle}
          onChange={(e) => setBundle(e.target.value)}
          placeholder='[{"signer_id": "alice", "sig_b64": "...", "timestamp": ...}]'
          style={{
            width: "100%", height: 100, resize: "vertical",
            background: "#080d14", border: "1px solid #1a2a3a",
            borderRadius: 3, color: "#8ab0bc", fontSize: 11,
            fontFamily: '"JetBrains Mono", "Fira Code", monospace',
            padding: "8px 10px", outline: "none", boxSizing: "border-box",
          }}
        />
        <div style={{ display: "flex", gap: 8, marginTop: 16, justifyContent: "flex-end" }}>
          <button onClick={onClose} disabled={busy} style={ghostBtn}>CANCEL</button>
          <button onClick={submit} disabled={busy} style={{ ...ghostBtn, color: bundle.trim() ? "#00d4ff" : "#ff4444", borderColor: bundle.trim() ? "#00d4ff44" : "#ff444444" }}>
            {bundle.trim() ? (busy ? "SUBMITTING…" : "APPROVE") : (busy ? "DENYING…" : "DENY")}
          </button>
        </div>
      </div>
    </div>
  );
}

// ── Pending approval card ─────────────────────────────────────────────────────

function PendingCard({
  info,
  onDeny,
  onApprove,
}: {
  info: PendingApprovalInfo;
  onDeny: () => void;
  onApprove: () => void;
}) {
  const [expanded, setExpanded] = useState(false);

  return (
    <div style={{
      background: "#0b0e18", border: "1px solid #1a2a3a",
      borderLeft: "3px solid #fd7e14", borderRadius: "0 6px 6px 0",
      padding: "16px 18px", marginBottom: 10,
      animation: "fadeSlideIn 0.2s ease-out",
    }}>
      {/* Header row */}
      <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", marginBottom: 10 }}>
        <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
          <span style={{ fontSize: 12, fontWeight: 700, letterSpacing: "0.06em", color: "#e2e8f0" }}>
            {info.tool_name}
          </span>
          <RiskBadge risk={info.risk_class} />
          {info.protocol_step && (
            <span style={{ fontSize: 9, color: "#2a4a6a", letterSpacing: "0.06em" }}>
              step {info.protocol_step.step_index + 1}/{info.protocol_step.total_steps}
            </span>
          )}
        </div>
        <span style={{ fontSize: 9, color: "#1a3040" }}>{reltime(info.queued_at)}</span>
      </div>

      {/* Hypothesis */}
      {info.hypothesis && (
        <div style={{ fontSize: 11, color: "#5a8090", marginBottom: 10, lineHeight: 1.5 }}>
          {info.hypothesis}
        </div>
      )}

      {/* Protocol step description */}
      {info.protocol_step?.description && (
        <div style={{
          fontSize: 10, color: "#1a4a5a", background: "#080d14",
          border: "1px solid #0f2030", borderRadius: 3, padding: "5px 9px", marginBottom: 10,
        }}>
          {info.protocol_step.description}
        </div>
      )}

      {/* Params toggle */}
      <button onClick={() => setExpanded((v) => !v)} style={{ ...ghostBtn, marginBottom: expanded ? 8 : 0 }}>
        {expanded ? "▲ HIDE PARAMS" : "▼ SHOW PARAMS"}
      </button>
      {expanded && (
        <pre style={{
          fontSize: 10, color: "#3a6a7a", background: "#080d14",
          border: "1px solid #0f2030", borderRadius: 3, padding: "8px 10px",
          overflowX: "auto", margin: "0 0 10px",
          fontFamily: '"JetBrains Mono", "Fira Code", monospace',
        }}>
          {JSON.stringify(info.params, null, 2)}
        </pre>
      )}

      {/* Actions */}
      <div style={{ display: "flex", gap: 8 }}>
        <button onClick={onDeny} style={{ ...ghostBtn, color: "#ff4444", borderColor: "#ff444433" }}>
          DENY
        </button>
        <button onClick={onApprove} style={{ ...ghostBtn, color: "#00d4ff", borderColor: "#00d4ff33" }}>
          APPROVE…
        </button>
      </div>
    </div>
  );
}

// ── Stalled recovery card ─────────────────────────────────────────────────────

function StalledCard({
  approvalId,
  onRecover,
  onCancel,
}: {
  approvalId: string;
  onRecover: () => void;
  onCancel: () => void;
}) {
  return (
    <div style={{
      background: "#0b0e18", border: "1px solid #2a1a0a",
      borderLeft: "3px solid #fd7e14", borderRadius: "0 6px 6px 0",
      padding: "14px 18px", marginBottom: 10,
    }}>
      <div style={{ fontSize: 10, color: "#fd7e14", marginBottom: 8, letterSpacing: "0.06em" }}>
        STALLED — no dispatch_complete recorded
      </div>
      <div style={{ fontSize: 10, color: "#3a5a6a", fontFamily: "monospace", marginBottom: 12 }}>
        {approvalId}
      </div>
      <div style={{ display: "flex", gap: 8 }}>
        <button onClick={onRecover} style={{ ...ghostBtn, color: "#00d4ff", borderColor: "#00d4ff33" }}>
          RECOVER
        </button>
        <button onClick={onCancel} style={{ ...ghostBtn, color: "#ff4444", borderColor: "#ff444433" }}>
          CANCEL
        </button>
      </div>
    </div>
  );
}

// ── Toast ─────────────────────────────────────────────────────────────────────

function Toast({ msg, ok }: { msg: string; ok: boolean }) {
  return (
    <div style={{
      position: "fixed", bottom: 24, right: 24,
      background: ok ? "#0a1f18" : "#1a0a0a",
      border: `1px solid ${ok ? "#00d4ff44" : "#ff444444"}`,
      borderRadius: 4, padding: "10px 16px",
      fontSize: 11, color: ok ? "#00d4ff" : "#ff4444",
      zIndex: 200, animation: "fadeSlideIn 0.2s ease-out",
    }}>
      {msg}
    </div>
  );
}

// ── Shared ghost button style ─────────────────────────────────────────────────

const ghostBtn: React.CSSProperties = {
  background: "transparent",
  border: "1px solid #1a3a50",
  borderRadius: 3,
  color: "#2a5a7a",
  fontSize: 9, letterSpacing: "0.12em",
  padding: "4px 12px",
  cursor: "pointer",
  fontFamily: '"JetBrains Mono", "Fira Code", monospace',
};

// ── Main panel ────────────────────────────────────────────────────────────────

export default function ApprovalsPanel() {
  const [pending, setPending]       = useState<PendingApprovalInfo[]>([]);
  const [stalled, setStalled]       = useState<string[]>([]);
  const [modalInfo, setModalInfo]   = useState<PendingApprovalInfo | null>(null);
  const [toast, setToast]           = useState<{ msg: string; ok: boolean } | null>(null);
  const [loading, setLoading]       = useState(true);

  function showToast(msg: string, ok: boolean) {
    setToast({ msg, ok });
    setTimeout(() => setToast(null), 3500);
  }

  const refresh = useCallback(async () => {
    try {
      const [pendRes, stalledRes] = await Promise.all([
        fetch(`${API}/approvals/pending`).then((r) => r.json()),
        fetch(`${API}/approvals/stalled`).then((r) => r.json()),
      ]);
      setPending(Array.isArray(pendRes) ? pendRes : []);
      const sr = stalledRes as StalledResponse;
      setStalled(sr.approval_ids ?? []);
    } catch {
      // silent — keep showing last known state
    } finally {
      setLoading(false);
    }
  }, []);

  // Initial load + 5s poll
  useEffect(() => {
    refresh();
    const t = setInterval(refresh, 5000);
    return () => clearInterval(t);
  }, [refresh]);

  // Refresh on tool execution events (approvals resolve when dispatches happen)
  useEffect(() => {
    return eventBus.listen<ToolExecutionEvent>(EVENTS.TOOL_EXECUTION, () => {
      refresh();
    });
  }, [refresh]);

  async function deny(pendingId: string) {
    try {
      const res = await fetch(`${API}/approvals/submit`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ pending_id: pendingId, bundle: null }),
      });
      if (res.ok) {
        showToast("Action denied.", true);
        refresh();
      } else {
        const j = await res.json();
        showToast(j.error ?? "Deny failed.", false);
      }
    } catch {
      showToast("Network error.", false);
    }
  }

  async function recover(id: string, cancel: boolean) {
    const token = getToken();
    const url = `${API}/approvals/recover/${id}${cancel ? "/cancel" : ""}`;
    try {
      const res = await fetch(url, {
        method: "POST",
        headers: token ? { Authorization: `Bearer ${token}` } : {},
      });
      if (res.ok) {
        showToast(cancel ? "Recovery cancelled." : "Recovery triggered.", true);
        refresh();
      } else if (res.status === 401) {
        showToast("Unauthorised — set localStorage.axiomlab_token to your operator JWT.", false);
      } else {
        showToast("Recovery failed.", false);
      }
    } catch {
      showToast("Network error.", false);
    }
  }

  const totalPending = pending.length + stalled.length;

  return (
    <div style={{
      flex: 1, display: "flex", flexDirection: "column", overflow: "hidden",
      background: "#070912",
    }}>
      {/* Panel header */}
      <div style={{
        padding: "18px 28px 14px", borderBottom: "1px solid #0e1520",
        flexShrink: 0, display: "flex", alignItems: "baseline", gap: 12,
      }}>
        <span style={{ fontSize: 11, fontWeight: 700, letterSpacing: "0.14em", color: "#e2e8f0" }}>
          APPROVALS
        </span>
        {totalPending > 0 && (
          <span style={{
            fontSize: 9, padding: "2px 7px", borderRadius: 2,
            background: "#2a1000", border: "1px solid #fd7e1444", color: "#fd7e14",
            letterSpacing: "0.06em",
          }}>
            {totalPending} PENDING
          </span>
        )}
        <button onClick={refresh} style={{ ...ghostBtn, marginLeft: "auto" }}>REFRESH</button>
      </div>

      <div style={{ flex: 1, overflowY: "auto", padding: "20px 28px" }}>
        {loading ? (
          <div style={{ fontSize: 10, color: "#1a3040", padding: "8px 0" }}>Loading…</div>
        ) : (
          <>
            {/* Pending approvals */}
            <div style={{ marginBottom: 28 }}>
              <SectionLabel>PENDING OPERATOR APPROVAL</SectionLabel>
              <div style={{ marginTop: 12 }}>
                {pending.length === 0 ? (
                  <div style={{ fontSize: 10, color: "#1a3040", padding: "6px 0" }}>
                    No actions awaiting approval.
                  </div>
                ) : (
                  pending.map((info) => (
                    <PendingCard
                      key={info.pending_id}
                      info={info}
                      onDeny={() => deny(info.pending_id)}
                      onApprove={() => setModalInfo(info)}
                    />
                  ))
                )}
              </div>
            </div>

            {/* Stalled recoveries */}
            {stalled.length > 0 && (
              <div>
                <SectionLabel>STALLED — RECOVERY REQUIRED</SectionLabel>
                <div style={{ marginTop: 12 }}>
                  {stalled.map((id) => (
                    <StalledCard
                      key={id}
                      approvalId={id}
                      onRecover={() => recover(id, false)}
                      onCancel={() => recover(id, true)}
                    />
                  ))}
                </div>
              </div>
            )}
          </>
        )}
      </div>

      {/* Approve modal */}
      {modalInfo && (
        <ApproveModal
          info={modalInfo}
          onClose={() => setModalInfo(null)}
          onResult={(msg, ok) => {
            showToast(msg, ok);
            if (ok) refresh();
          }}
        />
      )}

      {toast && <Toast msg={toast.msg} ok={toast.ok} />}
    </div>
  );
}
