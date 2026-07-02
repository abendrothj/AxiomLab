import React from 'react';

export default function StatusBar({ system, pipeline, currentRun, isDemo }) {
  const { running, backend, phase, connected } = system;
  const passedGates = pipeline.results.filter(r => r === true).length;

  return (
    <div className="status-bar">
      <div className="status-left">
        <span className="logo-text">Axiom<span className="logo-accent">Lab</span></span>
        {isDemo && <span className="demo-pill">DEMO</span>}
      </div>
      <div className="status-center">
        <div className="status-item">
          <span className="si-dot" data-state={running ? 'running' : phase === 'booting' ? 'booting' : 'idle'} />
          <span className="si-label">{running ? 'Running' : phase === 'booting' ? 'Booting' : 'Idle'}</span>
        </div>
        <div className="status-item">
          <span className="si-label">Backend</span>
          <span className="si-value">{backend}</span>
        </div>
        <div className="status-item">
          <span className="si-label">{isDemo ? 'Live' : 'WS'}</span>
          <span className="si-dot" data-state={connected || isDemo ? 'connected' : 'disconnected'} style={{ width: 8, height: 8, borderRadius: '50%' }} />
        </div>
        {currentRun && (
          <div className="status-item">
            <span className="si-label">Gates</span>
            <span className="si-value">{passedGates}/7</span>
          </div>
        )}
      </div>
      <div className="status-right">
        {currentRun && <span className="run-dir" title={currentRun.directive}>{currentRun.directive.slice(0, 40)}…</span>}
      </div>
    </div>
  );
}
