import React, { useCallback, useEffect, useMemo, useState } from "react";
import { api, getToken, setToken as saveToken, tokenIsRemembered } from "./api.js";
import { formatDeadline, routeFromHash, routes } from "./operator.js";

const directiveTemplates = [
  "Calibrate the spectrophotometer with registered standards, then read tube_1 at 500 nm",
  "Dispense 50 µL of buffer into tube_1, then read absorbance at 500 nm",
  "Set the incubator to 37 °C and report current temperature",
];

function useRoute() {
  const read = () => routeFromHash(location.hash);
  const [route, setRoute] = useState(read);
  useEffect(() => {
    const update = () => setRoute(read());
    window.addEventListener("hashchange", update);
    return () => window.removeEventListener("hashchange", update);
  }, []);
  return route;
}

function useLiveEvents() {
  const [events, setEvents] = useState([]);
  const [connected, setConnected] = useState(false);

  useEffect(() => {
    const proto = location.protocol === "https:" ? "wss" : "ws";
    const ws = new WebSocket(`${proto}://${location.host}/ws`);
    ws.onopen = () => setConnected(true);
    ws.onclose = () => setConnected(false);
    ws.onerror = () => setConnected(false);
    ws.onmessage = (m) => {
      try {
        setEvents((e) => [JSON.parse(m.data), ...e].slice(0, 80));
      } catch {
        setEvents((e) => [{ event: "unparsed", raw: m.data }, ...e].slice(0, 80));
      }
    };
    return () => ws.close();
  }, []);

  return { events, connected };
}

function Panel({ title, eyebrow, children, action, className = "" }) {
  return (
    <section className={`panel ${className}`}>
      <header className="panelHeader">
        <div>
          {eyebrow && <p className="eyebrow">{eyebrow}</p>}
          <h2>{title}</h2>
        </div>
        {action && <div className="panelAction">{action}</div>}
      </header>
      {children}
    </section>
  );
}

function Metric({ label, value, tone = "neutral" }) {
  return (
    <div className={`metric ${tone}`}>
      <span>{label}</span>
      <strong>{value}</strong>
    </div>
  );
}

function Pill({ children, tone = "neutral" }) {
  return <span className={`pill ${tone}`}>{children}</span>;
}

function formatAge(secs) {
  if (!secs) return "—";
  const delta = Math.max(0, Math.floor(Date.now() / 1000) - secs);
  if (delta < 60) return `${delta}s ago`;
  if (delta < 3600) return `${Math.floor(delta / 60)}m ago`;
  return `${Math.floor(delta / 3600)}h ago`;
}

function shortHash(value) {
  return value ? `${value.slice(0, 10)}…${value.slice(-6)}` : "—";
}

function JsonBlock({ value }) {
  return <pre className="jsonBlock">{JSON.stringify(value, null, 2)}</pre>;
}

function StatusStrip({ status, audit, connected }) {
  const backendTone = status?.backend === "hardware" ? "warn" : "neutral";
  const auditTone = audit?.verified ? "good" : "warn";

  return (
    <div className="statusStrip">
      <Metric label="Execution" value={status?.running ? "Running" : "Idle"} tone={status?.running ? "warn" : "good"} />
      <Metric label="Backend" value={status?.backend || "—"} tone={backendTone} />
      <Metric label="Queue" value={status?.queue ?? "—"} tone={(status?.queue || 0) > 0 ? "warn" : "neutral"} />
      <Metric label="Approvals" value={status?.pending_approvals ?? "—"} tone={(status?.pending_approvals || 0) > 0 ? "danger" : "good"} />
      <Metric label="Audit" value={audit?.verified ? "Verified" : "Check"} tone={auditTone} />
      <Metric label="Live" value={connected ? "Connected" : "Offline"} tone={connected ? "good" : "danger"} />
    </div>
  );
}

function TokenSettings() {
  const [token, setToken] = useState(getToken());
  const [remember, setRemember] = useState(tokenIsRemembered());
  const [saved, setSaved] = useState(false);

  const persist = () => {
    saveToken(token.trim(), remember);
    setSaved(true);
    setTimeout(() => setSaved(false), 1800);
  };

  return (
    <Panel title="Access token" eyebrow="Mutating API auth">
      <p className="muted tight">Used for queue submissions when the server has JWT auth enabled.</p>
      <div className="row">
        <input
          value={token}
          onChange={(e) => setToken(e.target.value)}
          placeholder="Bearer token / JWT"
          type="password"
          autoComplete="off"
        />
        <button type="button" onClick={persist}>{saved ? "Saved" : "Save"}</button>
      </div>
      <label className="checkRow">
        <input type="checkbox" checked={remember} onChange={(e) => setRemember(e.target.checked)} />
        Remember across browser restarts. Leave unchecked for session-only storage.
      </label>
      {token && <button type="button" className="link" onClick={() => { setToken(""); saveToken(""); }}>Clear token</button>}
    </Panel>
  );
}

function CommandCenter({ directive, setDirective, submit, busy }) {
  return (
    <Panel title="Command center" eyebrow="Queue a directive" className="span2">
      <form onSubmit={submit}>
        <textarea
          value={directive}
          onChange={(e) => setDirective(e.target.value)}
          placeholder="Describe the lab objective. The LLM proposes; gates enforce."
          rows={5}
        />
        <div className="toolbar">
          <div className="templateList">
            {directiveTemplates.map((template) => (
              <button key={template} type="button" className="ghost" onClick={() => setDirective(template)}>
                {template}
              </button>
            ))}
          </div>
          <button type="submit" disabled={busy || !directive.trim()}>{busy ? "Queueing…" : "Queue directive"}</button>
        </div>
      </form>
    </Panel>
  );
}

function ApprovalCard({ approval, onResolve }) {
  const [notes, setNotes] = useState("");
  const [approver, setApprover] = useState(localStorage.getItem("axiomlab_approver") || "operator");
  const [busy, setBusy] = useState(false);

  const decide = async (approved) => {
    setBusy(true);
    localStorage.setItem("axiomlab_approver", approver || "operator");
    try {
      await onResolve(approval.id, approved, notes, approver || "operator");
    } finally {
      setBusy(false);
    }
  };

  return (
    <li className="approvalCard">
      <div className="approvalTop">
        <div>
          <Pill tone="danger">operator decision required</Pill>
          <h3>{approval.tool}</h3>
        </div>
        <code>{shortHash(approval.scope_hash)}</code>
      </div>
      <dl className="facts">
        <div><dt>Risk</dt><dd>{approval.risk_class || "Policy change"}</dd></div>
        <div><dt>Gate</dt><dd>{approval.gate || "ApprovalGate"}</dd></div>
        <div><dt>Created</dt><dd>{formatAge(approval.created_secs)}</dd></div>
        <div><dt>Deadline</dt><dd className="deadline">{formatDeadline(approval.expires_secs)}</dd></div>
      </dl>
      <div className="approvalReason">{approval.reason || "Operator approval required"}</div>
      <JsonBlock value={approval.params} />
      <div className="reviewGrid">
        <input value={approver} onChange={(e) => setApprover(e.target.value)} placeholder="approver id" />
        <input value={notes} onChange={(e) => setNotes(e.target.value)} placeholder="decision notes" />
      </div>
      <div className="row end">
        <button type="button" className="danger" disabled={busy} onClick={() => decide(false)}>Deny</button>
        <button type="button" disabled={busy} onClick={() => decide(true)}>Approve exact scope</button>
      </div>
    </li>
  );
}

function ApprovalsPanel({ approvals, resolve }) {
  return (
    <Panel title="Approval inbox" eyebrow="High-risk gates" className="priority">
      {approvals.length === 0 ? (
        <div className="emptyState">No pending approvals. High-risk actions will appear here with exact params and scope hash.</div>
      ) : (
        <ul className="stack">
          {approvals.map((approval) => <ApprovalCard key={approval.id} approval={approval} onResolve={resolve} />)}
        </ul>
      )}
    </Panel>
  );
}

function QueuePanel({ queue, cancel }) {
  const ordered = [...queue].reverse();
  return (
    <Panel title="Directive queue" eyebrow="Work tracking">
      {ordered.length === 0 ? <div className="emptyState">No queued directives.</div> : (
        <ul className="timeline">
          {ordered.map((item) => (
            <li key={item.id}>
              <div className="timelineHead">
                <Pill tone={item.status}>{item.status}</Pill>
                <span>{formatAge(item.created_secs)}</span>
              </div>
              <p>{item.directive}</p>
              {item.summary && <p className="summary">{item.summary}</p>}
              {item.status === "pending" && <button type="button" className="link" onClick={() => cancel(item.id)}>Cancel pending directive</button>}
            </li>
          ))}
        </ul>
      )}
    </Panel>
  );
}

function AuditPanel({ audit, verify }) {
  return (
    <Panel
      title="Audit chain"
      eyebrow="Tamper evidence"
      action={<button type="button" className="secondary" onClick={verify}>Verify chain</button>}
    >
      {!audit ? "…" : (
        <>
          <div className="auditSummary">
            <Pill tone={audit.verified ? "good" : "warn"}>{audit.verified ? "verified" : "unverified"}</Pill>
            <span>{audit.total} entries</span>
            <code>tip {shortHash(audit.tip_hash)}</code>
          </div>
          <ul className="auditLog">
            {audit.entries.map((entry, index) => (
              <li key={`${entry.hash || index}-${index}`} className={entry.decision === "deny" ? "deny" : "allow"}>
                <div>
                  <b>{entry.action}</b>
                  <span>{entry.decision}</span>
                </div>
                <small>{entry.reason || entry.tool || "recorded"}</small>
              </li>
            ))}
          </ul>
        </>
      )}
    </Panel>
  );
}

function AgendaPanel({ agenda }) {
  return (
    <Panel title="Commissioning agenda" eyebrow="Readiness checklist">
      <ul className="checklist">
        {(agenda || []).map((item) => (
          <li key={item.key}>
            <Pill tone={item.status === "completed" ? "good" : "pending"}>{item.status}</Pill>
            <span>{item.statement}</span>
          </li>
        ))}
      </ul>
    </Panel>
  );
}

function LabPanel({ lab }) {
  const reagents = Object.values(lab?.reagents || {});
  const vessels = Object.entries(lab?.vessel_contents || {});
  const maxVol = Math.max(1, ...reagents.map((r) => Number(r.volume_ul || 0)));

  return (
    <Panel title="Lab inventory" eyebrow="Current state">
      <h3>Reagents</h3>
      {reagents.length === 0 ? <p className="muted">No registered reagents.</p> : (
        <ul className="inventory">
          {reagents.map((r) => (
            <li key={r.id}>
              <div><b>{r.name}</b><span>{r.volume_ul} µL</span></div>
              <div className="bar"><span style={{ width: `${Math.min(100, (Number(r.volume_ul || 0) / maxVol) * 100)}%` }} /></div>
              {r.reference_material_id && <small>reference: {r.reference_material_id}</small>}
            </li>
          ))}
        </ul>
      )}
      <h3>Vessels</h3>
      {vessels.length === 0 ? <p className="muted">No vessel contents recorded.</p> : (
        <ul className="vessels">
          {vessels.map(([vessel, contents]) => (
            <li key={vessel}><b>{vessel}</b><span>{contents.map((x) => x.reagent_id).join(", ") || "empty"}</span></li>
          ))}
        </ul>
      )}
    </Panel>
  );
}

function EventsPanel({ events, connected }) {
  return (
    <Panel title="Live event stream" eyebrow={connected ? "WebSocket connected" : "WebSocket offline"}>
      <ul className="eventLog">
        {events.length === 0 && <li className="muted">Waiting for server events…</li>}
        {events.map((event, index) => (
          <li key={index}>
            <code>{event.event || "event"}</code>
            <span>{event.id ? event.id.slice(0, 8) : event.directive || event.raw || ""}</span>
          </li>
        ))}
      </ul>
    </Panel>
  );
}

export default function App() {
  const route = useRoute();
  const [status, setStatus] = useState(null);
  const [audit, setAudit] = useState(null);
  const [approvals, setApprovals] = useState([]);
  const [queue, setQueue] = useState([]);
  const [agenda, setAgenda] = useState([]);
  const [lab, setLab] = useState(null);
  const [directive, setDirective] = useState("");
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);
  const { events, connected } = useLiveEvents();

  const refresh = useCallback(async () => {
    try {
      const [s, a, ap, q, ag, l] = await Promise.all([
        api.status(),
        api.audit(),
        api.approvals(),
        api.queue(),
        api.agenda(),
        api.lab(),
      ]);
      setStatus(s);
      setAudit(a);
      setApprovals(ap);
      setQueue(q);
      setAgenda(ag);
      setLab(l);
      setErr("");
    } catch (e) {
      setErr(String(e));
    }
  }, []);

  useEffect(() => {
    refresh();
    const t = setInterval(refresh, 3000);
    return () => clearInterval(t);
  }, [refresh, events.length]);

  const submit = async (e) => {
    e.preventDefault();
    if (!directive.trim()) return;
    setBusy(true);
    try {
      await api.pushDirective(directive.trim());
      setDirective("");
      await refresh();
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };

  const resolve = async (id, approved, notes, approverId) => {
    try {
      await api.resolveApproval(id, approved, notes, approverId);
      await refresh();
    } catch (e) {
      setErr(String(e));
    }
  };

  const cancel = async (id) => {
    try {
      await api.cancelQueued(id);
      await refresh();
    } catch (e) {
      setErr(String(e));
    }
  };

  const verify = async () => {
    try {
      const result = await api.verifyAudit();
      await refresh();
      setErr(result.ok ? "" : `Audit verification failed: ${result.error}`);
      if (result.ok) window.alert(`Verified ${result.entries_checked} entries; ${result.signatures_verified} signatures checked.`);
    } catch (e) {
      setErr(String(e));
    }
  };

  const priorityText = useMemo(() => {
    if ((approvals?.length || 0) > 0) return `${approvals.length} approval${approvals.length === 1 ? "" : "s"} require review`;
    if ((queue?.filter((q) => q.status === "running").length || 0) > 0) return "Protocol running";
    return "System idle; ready for directives";
  }, [approvals, queue]);

  return (
    <div className="app">
      <header className="hero">
        <div>
          <p className="eyebrow">AxiomLab operator console</p>
          <h1>Supervise autonomous lab execution</h1>
          <p>{priorityText}</p>
        </div>
        <button type="button" className="secondary" onClick={refresh}>Refresh</button>
      </header>
      <nav className="navTabs" aria-label="Operator console sections">
        {routes.map(([key, label]) => (
          <a key={key} href={`#/${key}`} className={route === key ? "active" : ""}>
            {label}
            {key === "approvals" && approvals.length > 0 && <span>{approvals.length}</span>}
          </a>
        ))}
      </nav>

      {err && <div className="error">{err}</div>}
      <StatusStrip status={status} audit={audit} connected={connected} />

      {route === "overview" && (
        <main className="grid">
          <ApprovalsPanel approvals={approvals} resolve={resolve} />
          <CommandCenter directive={directive} setDirective={setDirective} submit={submit} busy={busy} />
          <QueuePanel queue={queue.slice(-5)} cancel={cancel} />
          <AgendaPanel agenda={agenda} />
          <EventsPanel events={events.slice(0, 12)} connected={connected} />
        </main>
      )}
      {route === "approvals" && <main className="singleView"><ApprovalsPanel approvals={approvals} resolve={resolve} /></main>}
      {route === "runs" && (
        <main className="grid">
          <CommandCenter directive={directive} setDirective={setDirective} submit={submit} busy={busy} />
          <QueuePanel queue={queue} cancel={cancel} />
          <EventsPanel events={events} connected={connected} />
        </main>
      )}
      {route === "audit" && <main className="singleView"><AuditPanel audit={audit} verify={verify} /></main>}
      {route === "lab" && (
        <main className="grid twoCol">
          <LabPanel lab={lab} />
          <AgendaPanel agenda={agenda} />
        </main>
      )}
      {route === "settings" && <main className="singleView narrow"><TokenSettings /></main>}
    </div>
  );
}
