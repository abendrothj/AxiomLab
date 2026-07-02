import React from 'react';

export default function PipelineViz({ pipeline, currentRun }) {
  const gates = [
    { key: 'cap', label: 'Capability', desc: 'Operational parameter bounds' },
    { key: 'chem', label: 'Chemistry', desc: 'Reagent compatibility check' },
    { key: 'cal', label: 'Calibration', desc: 'Valid instrument calibration' },
    { key: 'proof', label: 'Proof', desc: 'Formally verified safety bounds' },
    { key: 'appr', label: 'Approval', desc: 'Operator sign-off' },
    { key: 'exec', label: 'Execute', desc: 'Instrument dispatch' },
    { key: 'audit', label: 'Audit', desc: 'Signed chain entry' },
  ];

  return (
    <div className="pipeline-viz">
      {gates.map((gate, i) => {
        let cls = 'gate-node';
        if (pipeline.results[i] === true) cls += ' passed';
        else if (pipeline.activeGate === i) cls += ' active';
        else if (pipeline.results[i] === false) cls += ' rejected';

        return (
          <div key={gate.key} className={cls} title={`${gate.label}: ${gate.desc}`}>
            <div className="gate-dot">
              {pipeline.results[i] === true ? '✓' : pipeline.results[i] === false ? '✗' : ''}
            </div>
            <span className="gate-label">{gate.label}</span>
          </div>
        );
      })}
    </div>
  );
}
