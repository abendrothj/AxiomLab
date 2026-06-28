import { useState, useEffect, useCallback } from "react";
import { QueuedProtocol, QueueStatus } from "../types";

const API = import.meta.env.DEV ? "http://localhost:3000/api" : "/api";

const STATUS_COLORS: Record<QueueStatus, string> = {
  pending:   "#a78bfa",
  running:   "#fd7e14",
  completed: "#00ff9d",
  failed:    "#ff3b3b",
};

function timeAgo(secs: number): string {
  const elapsed = Math.floor(Date.now() / 1000) - secs;
  if (elapsed < 60) return `${elapsed}s ago`;
  if (elapsed < 3600) return `${Math.floor(elapsed / 60)}m ago`;
  return `${Math.floor(elapsed / 3600)}h ago`;
}

export default function QueuePanel() {
  const [items, setItems]           = useState<QueuedProtocol[]>([]);
  const [loading, setLoading]       = useState(true);
  const [statement, setStatement]   = useState("");
  const [priority, setPriority]     = useState(100);
  const [submitting, setSubmitting] = useState(false);
  const [submitMsg, setSubmitMsg]   = useState<string | null>(null);

  const refresh = useCallback(() => {
    fetch(`${API}/queue`)
      .then((r) => r.json())
      .then((data) => {
        setItems(data.items ?? []);
        setLoading(false);
      })
      .catch(() => setLoading(false));
  }, []);

  useEffect(() => {
    refresh();
    const t = setInterval(refresh, 4000);
    return () => clearInterval(t);
  }, [refresh]);

  async function enqueue() {
    const stmt = statement.trim();
    if (!stmt) return;
    setSubmitting(true);
    try {
      const res = await fetch(`${API}/queue`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ statement: stmt, priority }),
      });
      if (res.ok) {
        const { id } = await res.json();
        setSubmitMsg(`Queued — id: ${id.slice(0, 8)}…`);
        setStatement("");
        setTimeout(() => setSubmitMsg(null), 4000);
        refresh();
      } else {
        const err = await res.json().catch(() => ({}));
        setSubmitMsg(`Error: ${err.error ?? res.status}`);
        setTimeout(() => setSubmitMsg(null), 4000);
      }
    } finally {
      setSubmitting(false);
    }
  }

  async function removeItem(id: string) {
    await fetch(`${API}/queue/${id}`, { method: "DELETE" });
    refresh();
  }

  const pending   = items.filter((i) => i.status === "pending");
  const running   = items.filter((i) => i.status === "running");
  const history   = items.filter((i) => i.status === "completed" || i.status === "failed");

  return (
    <div style={{
      flex: 1, display: "flex", overflow: "hidden", minHeight: 0,
      background: "#070912", color: "#e2e8f0",
      fontFamily: '"JetBrains Mono", "Fira Code", monospace',
    }}>
      {/* Left: queue form + live items */}
      <div style={{ width: "55%", display: "flex", flexDirection: "column", overflow: "hidden", borderRight: "1px solid #111824" }}>
        {/* Enqueue form */}
        <div style={{ padding: "20px 24px", borderBottom: "1px solid #0e1520", flexShrink: 0 }}>
          <Label>PUSH PROTOCOL DIRECTIVE</Label>
          <textarea
            value={statement}
            onChange={(e) => setStatement(e.target.value)}
            placeholder="Characterise the pH meter linearity from pH 4 to pH 10 using buffer standards. Report slope ± std-error and R²."
            rows={5}
            style={{
              marginTop: 12,
              width: "100%",
              background: "#0c1018",
              border: "1px solid #1a2a3a",
              borderRadius: 4,
              color: "#a0c4d4",
              fontSize: 12,
              fontFamily: "inherit",
              padding: "10px 12px",
              resize: "vertical",
              outline: "none",
              boxSizing: "border-box",
              lineHeight: 1.6,
            }}
          />
          <div style={{ display: "flex", alignItems: "center", gap: 12, marginTop: 10 }}>
            <div style={{ display: "flex", alignItems: "center", gap: 8, fontSize: 11 }}>
              <span style={{ color: "#2a4a6a", letterSpacing: "0.08em" }}>PRIORITY</span>
              <input
                type="number"
                min={0}
                max={255}
                value={priority}
                onChange={(e) => setPriority(Math.max(0, Math.min(255, Number(e.target.value))))}
                style={{
                  width: 56,
                  background: "#0c1018",
                  border: "1px solid #1a2a3a",
                  borderRadius: 3,
                  color: "#a0c4d4",
                  fontSize: 12,
                  fontFamily: "inherit",
                  padding: "4px 8px",
                  outline: "none",
                  textAlign: "center",
                }}
              />
              <span style={{ color: "#1a3040", fontSize: 10 }}>0–255, higher runs first</span>
            </div>
            <div style={{ flex: 1 }} />
            <button
              onClick={enqueue}
              disabled={submitting || !statement.trim()}
              style={{
                background: statement.trim() ? "#0f2a3a" : "#080d14",
                border: `1px solid ${statement.trim() ? "#00d4ff44" : "#1a2a3a"}`,
                borderRadius: 3,
                color: statement.trim() ? "#00d4ff" : "#2a4a5a",
                fontSize: 10,
                letterSpacing: "0.12em",
                padding: "6px 18px",
                cursor: statement.trim() ? "pointer" : "default",
                fontFamily: "inherit",
                transition: "all 0.15s",
              }}
            >
              {submitting ? "QUEUING…" : "QUEUE"}
            </button>
          </div>
          {submitMsg && (
            <div style={{ marginTop: 8, fontSize: 10, color: submitMsg.startsWith("Error") ? "#ff3b3b" : "#00ff9d", letterSpacing: "0.06em" }}>
              {submitMsg}
            </div>
          )}
        </div>

        {/* Active items */}
        <div style={{ flex: 1, overflowY: "auto", padding: "16px 24px" }}>
          {loading ? (
            <div style={{ fontSize: 10, color: "#1a3040" }}>loading…</div>
          ) : (
            <>
              {running.length > 0 && (
                <Section label="RUNNING">
                  {running.map((item) => (
                    <QueueCard key={item.id} item={item} onRemove={removeItem} />
                  ))}
                </Section>
              )}
              {pending.length > 0 && (
                <Section label={`PENDING (${pending.length})`}>
                  {pending.map((item) => (
                    <QueueCard key={item.id} item={item} onRemove={removeItem} />
                  ))}
                </Section>
              )}
              {running.length === 0 && pending.length === 0 && (
                <div style={{ padding: "48px 0", textAlign: "center" }}>
                  <div style={{ fontSize: 11, color: "#1a3040", letterSpacing: "0.08em", marginBottom: 8 }}>
                    Queue empty
                  </div>
                  <div style={{ fontSize: 10, color: "#0e1e2a", lineHeight: 1.6, maxWidth: 280, margin: "0 auto" }}>
                    Commissioning agenda is running automatically.
                    Push a directive above to take priority.
                  </div>
                </div>
              )}
            </>
          )}
        </div>
      </div>

      {/* Right: history */}
      <div style={{ flex: 1, display: "flex", flexDirection: "column", overflow: "hidden" }}>
        <div style={{ padding: "20px 24px 14px", borderBottom: "1px solid #0e1520", flexShrink: 0 }}>
          <Label>EXECUTION HISTORY</Label>
        </div>
        <div style={{ flex: 1, overflowY: "auto", padding: "16px 24px" }}>
          {history.length === 0 ? (
            <div style={{ fontSize: 10, color: "#1a3040", padding: "24px 0" }}>
              No completed executions yet.
            </div>
          ) : (
            history.map((item) => (
              <QueueCard key={item.id} item={item} onRemove={removeItem} />
            ))
          )}
        </div>
      </div>
    </div>
  );
}

// ── Subcomponents ─────────────────────────────────────────────────────────────

function Section({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div style={{ marginBottom: 20 }}>
      <Label>{label}</Label>
      <div style={{ marginTop: 10, display: "flex", flexDirection: "column", gap: 8 }}>
        {children}
      </div>
    </div>
  );
}

function QueueCard({ item, onRemove }: { item: QueuedProtocol; onRemove: (id: string) => void }) {
  const color = STATUS_COLORS[item.status];
  const canRemove = item.status === "pending";

  return (
    <div style={{
      padding: "12px 14px",
      background: "#0c1018",
      border: `1px solid ${color}1a`,
      borderLeft: `3px solid ${color}`,
      borderRadius: "0 4px 4px 0",
    }}>
      <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 8 }}>
        <span style={{
          fontSize: 8,
          letterSpacing: "0.12em",
          color: color,
          background: `${color}18`,
          border: `1px solid ${color}33`,
          borderRadius: 2,
          padding: "2px 7px",
          fontWeight: 700,
        }}>
          {item.status.toUpperCase()}
        </span>
        <span style={{ fontSize: 9, color: "#2a4a5a", letterSpacing: "0.08em" }}>
          P{item.priority}
        </span>
        <span style={{ flex: 1 }} />
        <span style={{ fontSize: 9, color: "#1a3040" }}>{timeAgo(item.added_at_secs)}</span>
        {canRemove && (
          <button
            onClick={() => onRemove(item.id)}
            title="Remove from queue"
            style={{
              background: "transparent",
              border: "1px solid #2a1a1a",
              borderRadius: 2,
              color: "#5a2020",
              fontSize: 9,
              cursor: "pointer",
              padding: "2px 7px",
              fontFamily: "inherit",
              letterSpacing: "0.08em",
              transition: "color 0.1s, border-color 0.1s",
            }}
            onMouseEnter={(e) => { (e.target as HTMLElement).style.color = "#ff3b3b"; (e.target as HTMLElement).style.borderColor = "#ff3b3b44"; }}
            onMouseLeave={(e) => { (e.target as HTMLElement).style.color = "#5a2020"; (e.target as HTMLElement).style.borderColor = "#2a1a1a"; }}
          >
            REMOVE
          </button>
        )}
      </div>
      <p style={{
        margin: 0, fontSize: 11, color: "#6a8a9a", lineHeight: 1.6,
        whiteSpace: "pre-wrap", wordBreak: "break-word",
        display: "-webkit-box",
        WebkitLineClamp: item.status === "pending" ? 3 : 2,
        WebkitBoxOrient: "vertical",
        overflow: "hidden",
      }}>
        {item.statement}
      </p>
      {item.result_summary && (
        <p style={{ margin: "8px 0 0", fontSize: 10, color: item.status === "completed" ? "#00c87a" : "#7a3a3a", lineHeight: 1.5 }}>
          {item.result_summary}
        </p>
      )}
      {item.experiment_id && (
        <div style={{ marginTop: 6, fontSize: 9, color: "#1a3040", letterSpacing: "0.06em" }}>
          exp {item.experiment_id.slice(0, 20)}…
        </div>
      )}
    </div>
  );
}

function Label({ children }: { children: React.ReactNode }) {
  return (
    <div style={{ fontSize: 9, letterSpacing: "0.14em", color: "#1a3a4a", fontWeight: 700 }}>
      {children}
    </div>
  );
}
