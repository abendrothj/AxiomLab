import React from 'react';
import { useLabState } from './useLabState';
import LabScene from './components/LabScene';
import PipelineViz from './components/PipelineViz';
import CommandBar from './components/CommandBar';
import ApprovalToast from './components/ApprovalToast';
import StatusBar from './components/StatusBar';

export default function App() {
  const { state, isDemo, submit, approveAction } = useLabState();
  const { system, pipeline, lab, arm, instruments, currentRun, currentAction, approvals } = state;

  const highlightVessel = currentAction?.vessel || null;

  return (
    <div className="app-shell">
      <StatusBar system={system} pipeline={pipeline} currentRun={currentRun} isDemo={isDemo} />

      <div className="main-content">
        <div className="lab-viewport">
          <LabScene
            lab={lab}
            arm={arm}
            instruments={instruments}
            pipeline={pipeline}
            currentRun={currentRun}
            currentAction={currentAction}
            highlightVessel={highlightVessel}
          />
        </div>

        <div className="side-panel">
          <PipelineViz pipeline={pipeline} currentRun={currentRun} />

          <div className="panel-section">
            <h3 className="panel-heading">Instruments</h3>
            {Object.entries(instruments).map(([id, inst]) => (
              <div key={id} className={`inst-row ${inst.state !== 'idle' ? 'inst-active' : ''}`}>
                <span className="inst-name">{formatInstName(id)}</span>
                <span className="inst-state">
                  {inst.state === 'reading' ? 'Measuring…' :
                   inst.state === 'spinning' ? `${inst.rpm} RPM` :
                   inst.lastReading != null ? inst.lastReading.toFixed(inst.lastReading < 10 ? 2 : 0) :
                   'Idle'}
                </span>
              </div>
            ))}
          </div>

          <div className="panel-section">
            <h3 className="panel-heading">Vessels</h3>
            {Object.entries(lab.vessel_contents).filter(([, c]) => c.length > 0).map(([name, contents]) => {
              const vol = lab.vessel_volumes[name] || 0;
              const maxVol = 50000;
              const pct = Math.min(vol / maxVol * 100, 100);
              return (
                <div key={name} className="vessel-row">
                  <span className="vessel-name">{formatVesselName(name)}</span>
                  <div className="vessel-bar-track">
                    <div className="vessel-bar-fill" style={{ width: `${pct}%` }} />
                  </div>
                  <span className="vessel-vol">{vol >= 1000 ? `${(vol / 1000).toFixed(1)}mL` : `${Math.round(vol)}µL`}</span>
                </div>
              );
            })}
            {Object.values(lab.vessel_contents).every(c => c.length === 0) && (
              <div className="empty-state">All vessels empty</div>
            )}
          </div>

          <div className="panel-section">
            <h3 className="panel-heading">Reagent Stock</h3>
            {Object.entries(lab.reagents).map(([id, r]) => (
              <div key={id} className="reagent-row">
                <span className="reagent-dot" style={{ background: reagentColor(id) }} />
                <span className="reagent-name">{r.name}</span>
                <span className="reagent-vol">{r.volume_ul >= 1000 ? `${(r.volume_ul / 1000).toFixed(0)}L` : `${Math.round(r.volume_ul)}mL`}</span>
              </div>
            ))}
          </div>
        </div>
      </div>

      <CommandBar onSubmit={submit} isDemo={isDemo} currentRun={currentRun} />
      <ApprovalToast approvals={approvals} onResolve={approveAction} />
    </div>
  );
}

function formatInstName(id) {
  const map = { spectrophotometer: 'Spectrophotometer', ph_meter: 'pH Meter', incubator: 'Incubator', centrifuge: 'Centrifuge' };
  return map[id] || id;
}

function formatVesselName(name) {
  return name.replace(/_/g, ' ').replace(/\b\w/g, c => c.toUpperCase());
}

function reagentColor(id) {
  const colors = { water: '#4ea1ff', ethanol: '#a78bfa', buffer_7: '#6ee7b7', buffer_4: '#f59e0b', buffer_10: '#f472b6', sample_a: '#fb7185', nacl: '#e2e8f0' };
  return colors[id] || '#6b7280';
}
