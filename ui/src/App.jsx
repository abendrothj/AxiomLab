import React, { useEffect, useState, useCallback } from "react";
import { api } from "./api.js";

function useLiveEvents() {
  const [events, setEvents] = useState([]);
  useEffect(() => {
    const proto = location.protocol === "https:" ? "wss" : "ws";
    const ws = new WebSocket(`${proto}://${location.host}/ws`);
    ws.onmessage = (m) => setEvents((e) => [JSON.parse(m.data), ...e].slice(0, 50));
    return () => ws.close();
  }, []);
  return events;
}

function Panel({ title, children, action }) {
  return (
    <section className="panel">
      <header>
        <h2>{title}</h2>
        {action}
      </header>
      {children}
    </section>
  );
}

export default function App() {
  const [status, setStatus] = useState(null);
  const [audit, setAudit] = useState(null);
  const [approvals, setApprovals] = useState([]);
  const [queue, setQueue] = useState([]);
  const [lab, setLab] = useState(null);
  const [directive, setDirective] = useState("");
  const [err, setErr] = useState("");
  const events = useLiveEvents();

  const refresh = useCallback(async () => {
    try {
      const [s, a, ap, q, l] = await Promise.all([
        api.status(), api.audit(), api.approvals(), api.queue(), api.lab(),
      ]);
      setStatus(s); setAudit(a); setApprovals(ap); setQueue(q); setLab(l); setErr("");
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
    try {
      await api.pushDirective(directive.trim());
      setDirective("");
      refresh();
    } catch (e) {
      setErr(String(e));
    }
  };

  return (
    <div className="app">
      <h1>
        AxiomLab <span className="tag">safe autonomous execution</span>
      </h1>
      {err && <div className="error">{err}</div>}

      <div className="grid">
        <Panel title="Status">
          {status ? (
            <ul className="kv">
              <li><span>Running</span><b>{String(status.running)}</b></li>
              <li><span>Iteration</span><b>{status.iteration}</b></li>
              <li><span>Backend</span><b>{status.backend}</b></li>
              <li><span>Queued</span><b>{status.queue}</b></li>
              <li><span>Pending approvals</span><b>{status.pending_approvals}</b></li>
            </ul>
          ) : "…"}
        </Panel>

        <Panel
          title="Audit chain"
          action={
            <button onClick={async () => { const v = await api.verifyAudit(); alert(v.ok ? `Verified ${v.entries_checked} entries` : `BROKEN: ${v.error}`); }}>
              Verify
            </button>
          }
        >
          {audit ? (
            <>
              <p className="muted">
                {audit.total} entries · {audit.verified ? "✓ verified" : "⚠ unverified"} · tip{" "}
                <code>{(audit.tip_hash || "—").slice(0, 12)}</code>
              </p>
              <ul className="log">
                {audit.entries.map((e, i) => (
                  <li key={i} className={e.decision === "deny" ? "deny" : "allow"}>
                    <b>{e.action}</b> <span>{e.decision}</span>
                  </li>
                ))}
              </ul>
            </>
          ) : "…"}
        </Panel>

        <Panel title="Directive queue">
          <form onSubmit={submit} className="row">
            <input value={directive} onChange={(e) => setDirective(e.target.value)} placeholder="e.g. Calibrate spectrophotometer" />
            <button type="submit">Queue</button>
          </form>
          <ul className="list">
            {queue.map((q) => (
              <li key={q.id}>
                <span className={`pill ${q.status}`}>{q.status}</span> {q.directive}
                {q.status === "pending" && <button className="link" onClick={() => api.cancelQueued(q.id).then(refresh)}>cancel</button>}
                {q.summary && <div className="muted">{q.summary}</div>}
              </li>
            ))}
          </ul>
        </Panel>

        <Panel title="Pending approvals">
          {approvals.length === 0 ? <p className="muted">none</p> : (
            <ul className="list">
              {approvals.map((a) => (
                <li key={a.id}>
                  <b>{a.tool}</b> <code>{JSON.stringify(a.params)}</code>
                  <div className="row">
                    <button onClick={() => api.resolveApproval(a.id, true, "ok").then(refresh)}>Approve</button>
                    <button className="danger" onClick={() => api.resolveApproval(a.id, false, "denied").then(refresh)}>Deny</button>
                  </div>
                </li>
              ))}
            </ul>
          )}
        </Panel>

        <Panel title="Lab state">
          {lab ? (
            <>
              <h3>Reagents</h3>
              <ul className="list">
                {Object.values(lab.reagents).map((r) => (
                  <li key={r.id}>{r.name} — {r.volume_ul} µL</li>
                ))}
                {Object.keys(lab.reagents).length === 0 && <li className="muted">empty</li>}
              </ul>
              <h3>Vessels</h3>
              <ul className="list">
                {Object.entries(lab.vessel_contents).map(([v, c]) => (
                  <li key={v}>{v}: {c.map((x) => x.reagent_id).join(", ")}</li>
                ))}
                {Object.keys(lab.vessel_contents).length === 0 && <li className="muted">empty</li>}
              </ul>
            </>
          ) : "…"}
        </Panel>

        <Panel title="Live events">
          <ul className="log">
            {events.map((e, i) => <li key={i}><code>{e.event}</code> {e.id?.slice(0, 8)}</li>)}
            {events.length === 0 && <li className="muted">listening…</li>}
          </ul>
        </Panel>
      </div>
    </div>
  );
}
