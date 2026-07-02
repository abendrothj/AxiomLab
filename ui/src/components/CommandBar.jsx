import React, { useState } from 'react';

export default function CommandBar({ onSubmit, isDemo, currentRun }) {
  const [text, setText] = useState('');

  const handleSubmit = (e) => {
    e.preventDefault();
    if (!text.trim() || isDemo) return;
    onSubmit(text.trim());
    setText('');
  };

  const templates = [
    'Dispense 100 µL of Sample A into tube_1',
    'Read absorbance of tube_1 at 280 nm',
    'Calibrate the spectrophotometer with reference standards',
  ];

  if (isDemo) {
    return (
      <div className="command-bar demo-mode">
        <div className="demo-badge">DEMO MODE</div>
        <span className="demo-text">Automated scenario playing — add <code>?demo</code> to URL to re-activate</span>
      </div>
    );
  }

  return (
    <div className="command-bar">
      <form onSubmit={handleSubmit} className="command-form">
        <input
          type="text"
          value={text}
          onChange={(e) => setText(e.target.value)}
          placeholder={currentRun ? 'Run in progress…' : 'Enter a natural-language directive…'}
          disabled={!!currentRun}
          className="command-input"
        />
        <button type="submit" disabled={!text.trim() || !!currentRun} className="command-submit">
          Queue
        </button>
      </form>
      <div className="command-templates">
        {templates.map((t, i) => (
          <button key={i} className="template-btn" onClick={() => setText(t)} disabled={!!currentRun}>
            {t}
          </button>
        ))}
      </div>
    </div>
  );
}
