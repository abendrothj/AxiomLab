import React from 'react';
import { REAGENT_COLORS, VESSEL_CAPACITIES } from '../demoEngine';

const VESSEL_DEFS = {
  beaker_A: { x: 160, y: 80, w: 80, h: 110, type: 'beaker', label: 'Beaker A' },
  beaker_B: { x: 160, y: 220, w: 80, h: 110, type: 'beaker', label: 'Beaker B' },
  tube_1: { x: 300, y: 80, w: 44, h: 130, type: 'tube', label: 'Tube 1' },
  tube_2: { x: 370, y: 80, w: 44, h: 130, type: 'tube', label: 'Tube 2' },
  tube_3: { x: 300, y: 240, w: 44, h: 130, type: 'tube', label: 'Tube 3' },
  plate_wells: { x: 390, y: 240, w: 100, h: 75, type: 'plate', label: 'Well Plate' },
  reservoir: { x: 30, y: 60, w: 100, h: 180, type: 'reservoir', label: 'Reagents' },
};

const INST_DEFS = {
  spectrophotometer: { x: 570, y: 50, w: 190, h: 100, label: 'Spectrophotometer', icon: 'spec' },
  ph_meter: { x: 570, y: 170, w: 190, h: 85, label: 'pH Meter', icon: 'ph' },
  incubator: { x: 790, y: 50, w: 170, h: 100, label: 'Incubator', icon: 'incubator' },
  centrifuge: { x: 790, y: 170, w: 170, h: 100, label: 'Centrifuge', icon: 'centrifuge' },
};

function Vessel({ id, def, contents, volume, highlight }) {
  const { x, y, w, h, type } = def;
  const capacity = VESSEL_CAPACITIES[id] || 1000;
  const fillPct = Math.min(volume / capacity, 0.9);
  const glow = highlight ? { filter: 'url(#glow)' } : {};

  if (type === 'reservoir') {
    return (
      <g {...glow}>
        <rect x={x} y={y} width={w} height={h} rx={6} fill="none" stroke="#4a5568" strokeWidth={1.5} />
        <clipPath id={`res-clip`}>
          <rect x={x + 2} y={y + h - fillPct * (h - 4)} width={w - 4} height={fillPct * (h - 4)} rx={3} />
        </clipPath>
        <rect x={x + 2} y={y + 2} width={w - 4} height={h - 4} rx={3} fill="#1a2332" />
        <rect x={x + 2} y={y + h - fillPct * (h - 4)} width={w - 4} height={fillPct * (h - 4)} fill="#2d6a8a" opacity={0.6} rx={3} />
        <text x={x + w / 2} y={y - 6} textAnchor="middle" fill="#8b9bb4" fontSize={10} fontFamily="Inter, sans-serif">{def.label}</text>
        {volume > 0 && <text x={x + w / 2} y={y + 15} textAnchor="middle" fill="#5a6a7e" fontSize={8} fontFamily="Inter, sans-serif">{(volume / 1000).toFixed(1)}L</text>}
      </g>
    );
  }

  if (type === 'beaker') {
    const topW = w;
    const botW = w * 0.72;
    const dw = (topW - botW) / 2;
    const lipH = 6;
    return (
      <g {...glow}>
        <clipPath id={`clip-${id}`}>
          <polygon points={`${x},${y + lipH} ${x + topW},${y + lipH} ${x + dw + botW},${y + h} ${x + dw},${y + h}`} />
        </clipPath>
        <polygon points={`${x},${y + lipH} ${x + topW},${y + lipH} ${x + dw + botW},${y + h} ${x + dw},${y + h}`} fill="none" stroke="#4a5568" strokeWidth={1.5} />
        <line x1={x - 3} y1={y} x2={x + topW + 3} y2={y} stroke="#4a5568" strokeWidth={1.5} />
        <rect x={x + 2} y={y + lipH + 2} width={topW - 4} height={h - lipH} fill="#1a2332" opacity={0.0} />
        <rect x={x} y={y + h - fillPct * (h - lipH)} width={topW} height={fillPct * (h - lipH)} fill={getFillColor(contents)} opacity={0.7} clipPath={`url(#clip-${id})`} />
        <text x={x + w / 2} y={y - 6} textAnchor="middle" fill="#8b9bb4" fontSize={10} fontFamily="Inter, sans-serif">{def.label}</text>
        {volume > 0 && <text x={x + w / 2} y={y + h / 2 + 4} textAnchor="middle" fill="#cbd5e1" fontSize={9} fontFamily="Inter, sans-serif">{formatVol(volume)}</text>}
      </g>
    );
  }

  if (type === 'tube') {
    const br = 8;
    return (
      <g {...glow}>
        <clipPath id={`clip-${id}`}>
          <rect x={x} y={y} width={w} height={h - br} rx={0} />
          <rect x={x} y={y + h - br * 2} width={w} height={br * 2} rx={br} />
        </clipPath>
        <rect x={x} y={y} width={w} height={h - br} fill="none" stroke="#4a5568" strokeWidth={1.5} />
        <path d={`M${x},${y + h - br} L${x},${y + h - br} A${br},${br} 0 0,0 ${x + w},${y + h - br}`} fill="none" stroke="#4a5568" strokeWidth={1.5} />
        <line x1={x - 2} y1={y - 2} x2={x + w + 2} y2={y - 2} stroke="#4a5568" strokeWidth={1.5} />
        <rect x={x} y={y + h - fillPct * h} width={w} height={fillPct * h} fill={getFillColor(contents)} opacity={0.7} clipPath={`url(#clip-${id})`} />
        <text x={x + w / 2} y={y - 8} textAnchor="middle" fill="#8b9bb4" fontSize={10} fontFamily="Inter, sans-serif">{def.label}</text>
        {volume > 0 && <text x={x + w / 2} y={y + h / 2 + 4} textAnchor="middle" fill="#cbd5e1" fontSize={8} fontFamily="Inter, sans-serif">{formatVol(volume)}</text>}
      </g>
    );
  }

  if (type === 'plate') {
    const cols = 4, rows = 3;
    const cw = w / cols, ch = h / rows;
    const wells = [];
    for (let r = 0; r < rows; r++) {
      for (let c = 0; c < cols; c++) {
        const wx = x + c * cw + cw / 2;
        const wy = y + r * ch + ch / 2;
        const active = r === 0 && c === 0 && fillPct > 0;
        wells.push(
          <circle key={`${r}-${c}`} cx={wx} cy={wy} r={cw * 0.28}
            fill={active ? (getFillColor(contents)) : '#1a2332'}
            stroke={active ? getFillColor(contents) : '#3a4a5e'} strokeWidth={1} opacity={active ? 0.8 : 0.6} />
        );
      }
    }
    return (
      <g {...glow}>
        <rect x={x} y={y} width={w} height={h} rx={4} fill="none" stroke="#4a5568" strokeWidth={1.5} />
        {wells}
        <text x={x + w / 2} y={y - 6} textAnchor="middle" fill="#8b9bb4" fontSize={10} fontFamily="Inter, sans-serif">{def.label}</text>
      </g>
    );
  }

  return null;
}

function getFillColor(contents) {
  if (!contents || contents.length === 0) return '#2d6a8a';
  const last = contents[contents.length - 1];
  return REAGENT_COLORS[last.reagent_id] || '#4ea1ff';
}

function formatVol(ul) {
  if (ul >= 1000) return `${(ul / 1000).toFixed(1)} mL`;
  return `${Math.round(ul)} µL`;
}

function RoboticArm({ arm, active }) {
  const { x, y, carrying } = arm;
  const baseX = Math.max(30, Math.min(580, x));
  const baseY = 440;
  const colH = baseY - y - 20;
  const armLen = Math.abs(x - baseX);

  return (
    <g>
      {/* Rail */}
      <rect x={20} y={baseY + 10} width={560} height={8} rx={4} fill="#1e293b" stroke="#334155" strokeWidth={1} />
      {/* Base slider */}
      <rect x={baseX - 12} y={baseY} width={24} height={18} rx={3} fill="#475569" stroke="#64748b" strokeWidth={1} />
      {/* Column */}
      <rect x={baseX - 4} y={baseY - colH - 5} width={8} height={colH + 5} rx={2} fill="#475569" stroke="#64748b" strokeWidth={1} />
      {/* Horizontal arm */}
      <rect x={baseX} y={baseY - colH - 8} width={x > baseX ? armLen : -armLen} height={6} rx={3}
        fill="#475569" stroke="#64748b" strokeWidth={1}
        transform={x < baseX ? undefined : undefined}
        style={x < baseX ? { transformOrigin: `${baseX}px ${baseY - colH - 5}px` } : {}} />
      {/* Actually let me just draw it relative to baseX */}
      {/* Pipette tip */}
      <rect x={x - 4} y={y - 25} width={5} height={25} rx={1} fill="#64748b" />
      {carrying && (
        <rect x={x - 6} y={y - 35} width={8} height={12} rx={2}
          fill={REAGENT_COLORS[carrying] || '#4ea1ff'} opacity={0.9} />
      )}
    </g>
  );
}

function Instrument({ id, def, state: inst }) {
  const { x, y, w, h, label, icon } = def;
  const isActive = inst.state === 'reading' || inst.state === 'spinning';

  return (
    <g>
      <rect x={x} y={y} width={w} height={h} rx={6}
        fill={isActive ? '#1a2740' : '#111827'}
        stroke={isActive ? '#4ea1ff' : '#334155'} strokeWidth={1.5}
        style={isActive ? { filter: 'url(#glow)' } : {}} />

      {/* Instrument-specific icons */}
      {icon === 'spec' && (
        <>
          <rect x={x + 12} y={y + h / 2 - 5} width={20} height={10} rx={2} fill="#1e293b" stroke="#4a5568" strokeWidth={1} />
          <line x1={x + 34} y1={y + h / 2} x2={x + 50} y2={y + h / 2} stroke={isActive ? '#fbbf24' : '#374151'} strokeWidth={2} strokeDasharray={isActive ? '3,2' : '0'} />
          {inst.state === 'reading' && <circle cx={x + 50} cy={y + h / 2} r={3} fill="#fbbf24" opacity={0.8} />}
        </>
      )}
      {icon === 'ph' && (
        <>
          <rect x={x + 15} y={y + 20} width={12} height={h - 40} rx={2} fill="#1e293b" stroke="#4a5568" strokeWidth={1} />
          <circle cx={x + 21} cy={y + h - 18} r={4} fill={isActive ? '#6ee7b7' : '#374151'} stroke={isActive ? '#6ee7b7' : '#4a5568'} strokeWidth={1} />
          <line x1={x + 21} y1={y + 22} x2={x + 21} y2={y + h - 22} stroke="#4a5568" strokeWidth={1} />
        </>
      )}
      {icon === 'incubator' && (
        <>
          <rect x={x + 15} y={y + 20} width={w - 30} height={h - 40} rx={3} fill="#1e293b" stroke="#334155" strokeWidth={1} />
          <text x={x + w / 2} y={y + h / 2 + 5} textAnchor="middle" fill={isActive ? '#f87171' : '#4a5568'} fontSize={16} fontFamily="monospace">{inst.temp}°C</text>
        </>
      )}
      {icon === 'centrifuge' && (
        <g transform={`translate(${x + w / 2}, ${y + h / 2})`}>
          <circle cx={0} cy={0} r={22} fill="#1e293b" stroke="#334155" strokeWidth={1.5} />
          <circle cx={0} cy={0} r={14} fill="#0f172a" stroke="#334155" strokeWidth={1} />
          {inst.state === 'spinning' ? (
            <>
              <animateTransform attributeName="transform" type="rotate" from="0" to="360" dur="0.6s" repeatCount="indefinite" additive="sum" />
              <line x1={-16} y1={0} x2={16} y2={0} stroke="#f87171" strokeWidth={2.5} />
              <line x1={0} y1={-16} x2={0} y2={16} stroke="#f87171" strokeWidth={2.5} />
            </>
          ) : (
            <>
              <line x1={-14} y1={0} x2={14} y2={0} stroke="#374151" strokeWidth={2} />
              <line x1={0} y1={-14} x2={0} y2={14} stroke="#374151" strokeWidth={2} />
            </>
          )}
          <circle cx={0} cy={0} r={4} fill="#475569" />
        </g>
      )}

      <text x={x + w / 2} y={y - 8} textAnchor="middle" fill="#8b9bb4" fontSize={10} fontFamily="Inter, sans-serif">{label}</text>
      {inst.lastReading !== null && (
        <text x={x + w / 2} y={y + h - 8} textAnchor="middle" fill="#6ee7b7" fontSize={9} fontFamily="monospace">
          {inst.lastReading.toFixed(inst.lastReading < 10 ? 2 : 0)} {inst.state === 'idle' ? '✓' : ''}
        </text>
      )}
    </g>
  );
}

export default function LabScene({ lab, arm, instruments, pipeline, currentRun, currentAction, highlightVessel }) {
  return (
    <svg viewBox="0 0 1000 510" className="lab-scene" xmlns="http://www.w3.org/2000/svg">
      <defs>
        <filter id="glow" x="-20%" y="-20%" width="140%" height="140%">
          <feGaussianBlur stdDeviation="3" result="blur" />
          <feMerge>
            <feMergeNode in="blur" />
            <feMergeNode in="SourceGraphic" />
          </feMerge>
        </filter>
        <filter id="glow-green" x="-20%" y="-20%" width="140%" height="140%">
          <feGaussianBlur stdDeviation="2.5" result="blur" />
          <feMerge>
            <feMergeNode in="blur" />
            <feMergeNode in="SourceGraphic" />
          </feMerge>
        </filter>
        <linearGradient id="bgGrad" x1="0%" y1="0%" x2="100%" y2="100%">
          <stop offset="0%" stopColor="#0f172a" />
          <stop offset="100%" stopColor="#020617" />
        </linearGradient>
      </defs>

      {/* Background */}
      <rect x={0} y={0} width={1000} height={510} fill="url(#bgGrad)" rx={10} />

      {/* Grid lines */}
      {[0, 1, 2, 3, 4, 5].map(i =>
        <line key={`h${i}`} x1={0} y1={i * 85} x2={1000} y2={i * 85} stroke="#1e293b" strokeWidth={0.5} />
      )}
      {[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11].map(i =>
        <line key={`v${i}`} x1={i * 85} y1={0} x2={i * 85} y2={510} stroke="#1e293b" strokeWidth={0.5} />
      )}

      {/* Subtle bench surface texture */}
      <rect x={0} y={395} width={620} height={115} fill="#0a101e" stroke="#1e293b" strokeWidth={1} />
      <line x1={0} y1={397} x2={620} y2={397} stroke="#162032" strokeWidth={1} />

      {/* Vessels */}
      {Object.entries(VESSEL_DEFS).map(([id, def]) => (
        <Vessel key={id} id={id} def={def}
          contents={lab.vessel_contents[id] || []}
          volume={lab.vessel_volumes[id] || 0}
          highlight={highlightVessel === id} />
      ))}

      {/* Robotic Arm */}
      <RoboticArm arm={arm} active={currentAction !== null} />

      {/* Instruments */}
      {Object.entries(INST_DEFS).map(([id, def]) => (
        <Instrument key={id} id={id} def={def} state={instruments[id] || { state: 'idle', lastReading: null }} />
      ))}

      {/* Pipeline indicator dots (top of instrument section) */}
      <PipelineIndicators pipeline={pipeline} currentRun={currentRun} />

      {/* Current action display */}
      {currentRun && (
        <g>
          <rect x={10} y={10} width={350} height={26} rx={5} fill="#111827" stroke="#334155" strokeWidth={1} />
          <text x={20} y={28} fill="#8b9bb4" fontSize={10} fontFamily="monospace">
            {currentRun.directive.length > 50 ? currentRun.directive.slice(0, 48) + '…' : currentRun.directive}
          </text>
          <circle cx={370} cy={23} r={4} fill="#4ea1ff" className="pulse-dot" />
        </g>
      )}
      {!currentRun && (
        <text x={20} y={26} fill="#374151" fontSize={11} fontFamily="Inter, sans-serif">
          {pipeline.activeGate < 0 ? 'Awaiting directive…' : 'Idle'}
        </text>
      )}
    </svg>
  );
}

function PipelineIndicators({ pipeline, currentRun }) {
  const names = ['CAP', 'CHM', 'CAL', 'PRF', 'APR', 'EXE', 'AUD'];
  const tooltips = ['Capability', 'Chemistry', 'Calibration', 'Proof', 'Approval', 'Execute', 'Audit'];
  return (
    <g>
      {names.map((name, i) => {
        const cx = 580 + i * 40;
        const cy = 5;
        const r = 6;
        let fill = '#1e293b';
        let stroke = '#334155';
        let glow = false;

        if (pipeline.results[i] === true) {
          fill = '#166534';
          stroke = '#22c55e';
          glow = true;
        } else if (pipeline.activeGate === i) {
          fill = '#1e3a5f';
          stroke = '#4ea1ff';
          glow = true;
        } else if (pipeline.results[i] === false) {
          fill = '#7f1d1d';
          stroke = '#ef4444';
        }

        return (
          <g key={i}>
            {glow && <circle cx={cx} cy={cy} r={r + 3} fill="none" stroke={stroke} opacity={0.3} strokeWidth={2} />}
            <circle cx={cx} cy={cy} r={r} fill={fill} stroke={stroke} strokeWidth={1.5} />
            <text x={cx} y={cy + 3} textAnchor="middle" fill={glow ? '#e2e8f0' : '#475569'} fontSize={6} fontFamily="Inter, sans-serif" fontWeight="bold">{name}</text>
            {i < 6 && <line x1={cx + r + 2} y1={cy} x2={cx + 40 - r - 2} y2={cy} stroke="#1e293b" strokeWidth={1} />}
          </g>
        );
      })}
    </g>
  );
}

export { VESSEL_DEFS, REAGENT_COLORS, formatVol };
