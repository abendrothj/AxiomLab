import { useState, useEffect, useCallback } from "react";
import { eventBus } from "../eventBus";
import { EVENTS, Reagent, VesselContribution, CalibrationEntry, ToolExecutionEvent } from "../types";

const API = import.meta.env.DEV ? "http://localhost:3000/api" : "/api";

// ── Helpers ───────────────────────────────────────────────────────────────────

function SectionLabel({ children }: { children: React.ReactNode }) {
  return (
    <div style={{ fontSize: 9, letterSpacing: "0.14em", color: "#1a3a4a", fontWeight: 700 }}>
      {children}
    </div>
  );
}

function relExpiry(secs?: number): { label: string; color: string } | null {
  if (secs == null) return null;
  const diff = secs - Math.floor(Date.now() / 1000);
  if (diff < 0) return { label: "EXPIRED", color: "#ff4444" };
  if (diff < 7 * 86400) return { label: `${Math.ceil(diff / 86400)}d left`, color: "#fd7e14" };
  return { label: `${Math.ceil(diff / 86400)}d left`, color: "#20c997" };
}

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

// ── Reagent row ───────────────────────────────────────────────────────────────

function ReagentRow({ r }: { r: Reagent }) {
  const expiry = relExpiry(r.expiry_secs);
  return (
    <div style={{
      display: "grid",
      gridTemplateColumns: "1fr 90px 80px 80px 90px",
      gap: 8,
      padding: "9px 0",
      borderBottom: "1px solid #0a1018",
      alignItems: "center",
      fontSize: 11,
    }}>
      <div>
        <span style={{ color: "#9ab0bc" }}>{r.name}</span>
        {r.cas_number && (
          <span style={{ fontSize: 9, color: "#1a3a4a", marginLeft: 8 }}>{r.cas_number}</span>
        )}
      </div>
      <div style={{ fontSize: 9, color: "#1a3a4a", fontFamily: "monospace" }}>{r.lot_number}</div>
      <div style={{ color: "#5a8090", textAlign: "right" }}>
        {r.volume_ul.toLocaleString()}µL
      </div>
      <div style={{ textAlign: "right" }}>
        {r.concentration != null
          ? <span style={{ fontSize: 10, color: "#3a6a7a" }}>{r.concentration} {r.concentration_unit ?? "M"}</span>
          : <span style={{ fontSize: 9, color: "#1a2a3a" }}>—</span>
        }
      </div>
      <div style={{ textAlign: "right" }}>
        {expiry
          ? <span style={{ fontSize: 9, color: expiry.color }}>{expiry.label}</span>
          : <span style={{ fontSize: 9, color: "#1a2a3a" }}>—</span>
        }
      </div>
    </div>
  );
}

// ── Vessel card ───────────────────────────────────────────────────────────────

function VesselCard({ id, contents }: { id: string; contents: VesselContribution[] }) {
  const totalUl = contents.reduce((s, c) => s + c.volume_ul, 0);
  return (
    <div style={{
      background: "#0b0e18", border: "1px solid #111824",
      borderLeft: "3px solid #1a3a50", borderRadius: "0 6px 6px 0",
      padding: "12px 16px", marginBottom: 8,
    }}>
      <div style={{ display: "flex", alignItems: "baseline", gap: 10, marginBottom: 8 }}>
        <span style={{ fontSize: 11, fontWeight: 700, color: "#e2e8f0" }}>{id}</span>
        <span style={{ fontSize: 9, color: "#1a4a5a" }}>
          {totalUl > 0 ? `${totalUl.toLocaleString(undefined, { maximumFractionDigits: 1 })}µL total` : "empty"}
        </span>
      </div>
      {contents.length === 0 ? (
        <div style={{ fontSize: 9, color: "#1a2a3a" }}>no contents recorded</div>
      ) : (
        contents.map((c, i) => (
          <div key={i} style={{
            display: "flex", alignItems: "center", gap: 10,
            fontSize: 10, color: "#3a6a7a",
            padding: "3px 0",
            borderTop: i > 0 ? "1px solid #080d14" : "none",
          }}>
            <span style={{ minWidth: 120 }}>{c.reagent_id}</span>
            <span style={{ color: "#1a4a5a" }}>{c.volume_ul.toFixed(1)}µL</span>
            {c.concentration_m > 0 && (
              <span style={{ color: "#1a3a4a" }}>{c.concentration_m.toExponential(2)} M</span>
            )}
          </div>
        ))
      )}
    </div>
  );
}

// ── Calibration status grid ───────────────────────────────────────────────────

function CalibrationGrid({ entries }: { entries: CalibrationEntry[] }) {
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
      {entries.map((e) => {
        const dotColor = !e.calibrated ? "#ff4444" : !e.valid ? "#fd7e14" : "#00d4ff";
        const label = !e.calibrated ? "NOT CALIBRATED" : !e.valid ? "EXPIRED" : "VALID";
        return (
          <div key={e.instrument} style={{
            display: "flex", alignItems: "center", gap: 12,
            padding: "8px 14px",
            background: "#0b0e18", border: "1px solid #111824", borderRadius: 4,
          }}>
            <span style={{
              width: 7, height: 7, borderRadius: "50%",
              background: dotColor,
              boxShadow: `0 0 6px ${dotColor}88`,
              flexShrink: 0,
            }} />
            <span style={{ fontSize: 11, color: "#9ab0bc", flex: 1 }}>
              {e.instrument.replace(/_/g, " ").toUpperCase()}
            </span>
            <span style={{ fontSize: 9, color: dotColor, letterSpacing: "0.06em" }}>{label}</span>
            {e.valid && e.valid_until_secs && (() => {
              const diff = e.valid_until_secs - Math.floor(Date.now() / 1000);
              return (
                <span style={{ fontSize: 9, color: "#1a3a4a" }}>
                  {diff > 0 ? `${Math.ceil(diff / 3600)}h remaining` : ""}
                </span>
              );
            })()}
          </div>
        );
      })}
    </div>
  );
}

// ── Main panel ────────────────────────────────────────────────────────────────

export default function LabPanel() {
  const [reagents, setReagents]         = useState<Reagent[]>([]);
  const [vessels, setVessels]           = useState<Record<string, VesselContribution[]>>({});
  const [calibration, setCalibration]   = useState<CalibrationEntry[]>([]);
  const [loading, setLoading]           = useState(true);

  const refresh = useCallback(async () => {
    try {
      const [rRes, vRes, cRes] = await Promise.all([
        fetch(`${API}/lab/reagents`).then((r) => r.json()),
        fetch(`${API}/lab/vessels`).then((r) => r.json()),
        fetch(`${API}/lab/calibration-status`).then((r) => r.json()),
      ]);
      setReagents(Array.isArray(rRes) ? rRes : []);
      setVessels(vRes && typeof vRes === "object" ? vRes : {});
      setCalibration(Array.isArray(cRes) ? cRes : []);
    } catch {
      // keep last known state
    } finally {
      setLoading(false);
    }
  }, []);

  // Load on mount + 10s poll
  useEffect(() => {
    refresh();
    const t = setInterval(refresh, 10_000);
    return () => clearInterval(t);
  }, [refresh]);

  // Live update when a tool executes (lab state may have changed)
  useEffect(() => {
    return eventBus.listen<ToolExecutionEvent>(EVENTS.TOOL_EXECUTION, (ev) => {
      if (ev.status === "success") refresh();
    });
  }, [refresh]);

  const vesselEntries = Object.entries(vessels);

  return (
    <div style={{
      flex: 1, display: "flex", overflow: "hidden", background: "#070912",
    }}>
      {/* Left column: reagents + calibration */}
      <div style={{
        width: "55%", display: "flex", flexDirection: "column",
        overflow: "hidden", borderRight: "1px solid #111824",
      }}>
        {/* Reagents */}
        <div style={{ flex: 1, overflow: "hidden", display: "flex", flexDirection: "column" }}>
          <div style={{
            padding: "18px 24px 14px", borderBottom: "1px solid #0e1520",
            flexShrink: 0, display: "flex", alignItems: "baseline", gap: 10,
          }}>
            <SectionLabel>REAGENT INVENTORY</SectionLabel>
            {!loading && (
              <span style={{ fontSize: 9, color: "#1a3a4a" }}>{reagents.length} registered</span>
            )}
            <button onClick={refresh} style={{ ...ghostBtn, marginLeft: "auto" }}>REFRESH</button>
          </div>

          <div style={{ flex: 1, overflowY: "auto", padding: "0 24px 16px" }}>
            {loading ? (
              <div style={{ fontSize: 10, color: "#1a3040", padding: "12px 0" }}>Loading…</div>
            ) : reagents.length === 0 ? (
              <div style={{ fontSize: 10, color: "#1a3040", padding: "12px 0" }}>
                No reagents registered.
              </div>
            ) : (
              <>
                {/* Column headers */}
                <div style={{
                  display: "grid",
                  gridTemplateColumns: "1fr 90px 80px 80px 90px",
                  gap: 8, padding: "10px 0 6px",
                  fontSize: 8, letterSpacing: "0.12em", color: "#1a3a4a",
                  borderBottom: "1px solid #0e1520",
                }}>
                  <span>NAME / CAS</span>
                  <span>LOT</span>
                  <span style={{ textAlign: "right" }}>VOLUME</span>
                  <span style={{ textAlign: "right" }}>CONC.</span>
                  <span style={{ textAlign: "right" }}>EXPIRY</span>
                </div>
                {reagents.map((r) => <ReagentRow key={r.id} r={r} />)}
              </>
            )}
          </div>
        </div>

        {/* Calibration status */}
        <div style={{ flexShrink: 0, borderTop: "1px solid #0e1520" }}>
          <div style={{ padding: "14px 24px 10px", borderBottom: "1px solid #0e1520" }}>
            <SectionLabel>CALIBRATION STATUS</SectionLabel>
          </div>
          <div style={{ padding: "12px 24px 16px" }}>
            {calibration.length === 0 ? (
              <div style={{ fontSize: 10, color: "#1a3040" }}>No calibration data.</div>
            ) : (
              <CalibrationGrid entries={calibration} />
            )}
          </div>
        </div>
      </div>

      {/* Right column: vessels */}
      <div style={{ flex: 1, display: "flex", flexDirection: "column", overflow: "hidden" }}>
        <div style={{
          padding: "18px 24px 14px", borderBottom: "1px solid #0e1520",
          flexShrink: 0, display: "flex", alignItems: "baseline", gap: 10,
        }}>
          <SectionLabel>VESSEL CONTENTS</SectionLabel>
          {!loading && (
            <span style={{ fontSize: 9, color: "#1a3a4a" }}>{vesselEntries.length} vessels</span>
          )}
        </div>

        <div style={{ flex: 1, overflowY: "auto", padding: "16px 24px" }}>
          {loading ? (
            <div style={{ fontSize: 10, color: "#1a3040" }}>Loading…</div>
          ) : vesselEntries.length === 0 ? (
            <div style={{ fontSize: 10, color: "#1a3040" }}>
              No vessel contents recorded.
            </div>
          ) : (
            vesselEntries.map(([id, contents]) => (
              <VesselCard key={id} id={id} contents={contents} />
            ))
          )}
        </div>
      </div>
    </div>
  );
}
