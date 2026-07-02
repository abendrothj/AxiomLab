const VESSEL_CAPACITIES = {
  beaker_A: 50000, beaker_B: 50000, tube_1: 2000, tube_2: 2000,
  tube_3: 2000, plate_well_A1: 300, plate_well_B1: 300, reservoir: 200000,
};

const INITIAL_LAB = {
  reagents: {
    water: { id: 'water', name: 'Water', volume_ul: 95000, concentration_m: null, nominal_ph: 7.0, is_buffer: false },
    ethanol: { id: 'ethanol', name: 'Ethanol', volume_ul: 45000, concentration_m: null, nominal_ph: null, is_buffer: false },
    buffer_7: { id: 'buffer_7', name: 'Phosphate Buffer pH 7.0', volume_ul: 28000, concentration_m: 0.1, nominal_ph: 7.0, is_buffer: true, reference_material_id: 'CRM-BUF7-001' },
    buffer_4: { id: 'buffer_4', name: 'Phthalate Buffer pH 4.0', volume_ul: 25000, concentration_m: 0.05, nominal_ph: 4.0, is_buffer: true, reference_material_id: 'CRM-BUF4-001' },
    buffer_10: { id: 'buffer_10', name: 'Borate Buffer pH 10.0', volume_ul: 25000, concentration_m: 0.05, nominal_ph: 10.0, is_buffer: true, reference_material_id: 'CRM-BUF10-001' },
    sample_a: { id: 'sample_a', name: 'Sample A (protein)', volume_ul: 8000, concentration_m: 0.002, nominal_ph: 7.2, is_buffer: false },
    nacl: { id: 'nacl', name: 'NaCl 1M', volume_ul: 15000, concentration_m: 1.0, nominal_ph: null, is_buffer: false },
  },
  vessel_contents: {
    beaker_A: [], beaker_B: [], tube_1: [], tube_2: [], tube_3: [],
    plate_well_A1: [], plate_well_B1: [], reservoir: [],
  },
  vessel_volumes: {
    beaker_A: 0, beaker_B: 0, tube_1: 0, tube_2: 0, tube_3: 0,
    plate_well_A1: 0, plate_well_B1: 0, reservoir: 0,
  },
};

const GATE_NAMES = ['Capability', 'Chemistry', 'Calibration', 'Proof', 'Approval', 'Execute', 'Audit'];
const REAGENT_COLORS = {
  water: '#4ea1ff', ethanol: '#a78bfa', buffer_7: '#6ee7b7', buffer_4: '#f59e0b',
  buffer_10: '#f472b6', sample_a: '#fb7185', nacl: '#e2e8f0',
};

function deepClone(obj) { return JSON.parse(JSON.stringify(obj)); }

function buildTimeline() {
  const T = (s) => s * 1000;
  let t = 0;
  const events = [];

  const add = (dt, data) => { t += dt; events.push({ t, ...data }); };

  // ── Phase 0: Boot ──
  add(0, { type: 'system', system: { running: false, backend: 'simulator', phase: 'booting' } });
  add(600, { type: 'system', system: { running: true, backend: 'simulator', phase: 'idle' } });

  // ── Phase 1: Calibrate Spectrophotometer ──
  add(800, { type: 'run_start', runId: 'run-1', directive: 'Calibrate the spectrophotometer with reference standards' });
  add(400, { type: 'pipeline', activeGate: 0 });
  add(300, { type: 'pipeline', activeGate: 0, gateResult: true });
  add(200, { type: 'pipeline', activeGate: 1 });
  add(250, { type: 'pipeline', activeGate: 1, gateResult: true });
  add(200, { type: 'pipeline', activeGate: 2 });
  add(250, { type: 'pipeline', activeGate: 2, gateResult: true });
  add(200, { type: 'pipeline', activeGate: 3 });
  add(250, { type: 'pipeline', activeGate: 3, gateResult: true });
  add(200, { type: 'action', tool: 'calibrate', target: 'spectrophotometer' });
  add(300, { type: 'arm', target: 'reservoir' });
  add(600, { type: 'arm', carrying: 'buffer_7' });
  add(400, { type: 'arm', target: 'spectrophotometer' });
  add(600, { type: 'arm', carrying: null });
  add(200, { type: 'instrument', id: 'spectrophotometer', state: 'reading' });
  add(600, { type: 'instrument', id: 'spectrophotometer', state: 'idle', reading: 0.82 });
  add(200, { type: 'pipeline', activeGate: 5 });
  add(200, { type: 'pipeline', activeGate: 5, gateResult: true });
  add(150, { type: 'pipeline', activeGate: 6 });
  add(200, { type: 'pipeline', activeGate: 6, gateResult: true });
  add(400, { type: 'run_end', runId: 'run-1' });

  // ── Phase 2: Sample Prep ──
  add(600, { type: 'run_start', runId: 'run-2', directive: 'Dispense 100 µL of Sample A into tube_1, then add 200 µL of buffer' });
  add(300, { type: 'pipeline', activeGate: 0 });
  add(200, { type: 'pipeline', activeGate: 0, gateResult: true });
  add(150, { type: 'pipeline', activeGate: 1 });
  add(200, { type: 'pipeline', activeGate: 1, gateResult: true });
  add(150, { type: 'pipeline', activeGate: 3 });
  add(200, { type: 'pipeline', activeGate: 3, gateResult: true });
  add(200, { type: 'action', tool: 'dispense', reagent: 'sample_a', vessel: 'tube_1', volume_ul: 100 });
  add(300, { type: 'arm', target: 'reservoir' });
  add(500, { type: 'arm', carrying: 'sample_a' });
  add(400, { type: 'arm', target: 'tube_1' });
  add(500, { type: 'arm', carrying: null });
  add(100, { type: 'dispense', vessel: 'tube_1', reagent: 'sample_a', volume_ul: 100, color: REAGENT_COLORS.sample_a });
  add(600, { type: 'arm', target: 'reservoir' });
  add(500, { type: 'arm', carrying: 'buffer_7' });
  add(400, { type: 'arm', target: 'tube_1' });
  add(500, { type: 'arm', carrying: null });
  add(100, { type: 'dispense', vessel: 'tube_1', reagent: 'buffer_7', volume_ul: 200 });
  add(300, { type: 'pipeline', activeGate: 5 });
  add(200, { type: 'pipeline', activeGate: 5, gateResult: true });
  add(150, { type: 'pipeline', activeGate: 6 });
  add(200, { type: 'pipeline', activeGate: 6, gateResult: true });
  add(400, { type: 'run_end', runId: 'run-2' });

  // ── Phase 3: pH Measurement ──
  add(600, { type: 'run_start', runId: 'run-3', directive: 'Measure pH of tube_1 and tube_2' });
  add(300, { type: 'pipeline', activeGate: 0 });
  add(200, { type: 'pipeline', activeGate: 0, gateResult: true });
  add(150, { type: 'pipeline', activeGate: 2 });
  add(200, { type: 'pipeline', activeGate: 2, gateResult: true });
  add(150, { type: 'pipeline', activeGate: 3 });
  add(200, { type: 'pipeline', activeGate: 3, gateResult: true });
  add(200, { type: 'action', tool: 'read_ph', target: 'ph_meter', vessel: 'tube_1' });
  add(300, { type: 'instrument', id: 'ph_meter', state: 'reading' });
  add(600, { type: 'instrument', id: 'ph_meter', state: 'idle', reading: 7.18 });
  add(200, { type: 'pipeline', activeGate: 6, gateResults: [true, true, true, true, true, true] });
  add(200, { type: 'pipeline', activeGate: 6, gateResult: true });
  add(400, { type: 'run_end', runId: 'run-3' });

  // ── Phase 4: Centrifuge (needs approval) ──
  add(600, { type: 'run_start', runId: 'run-4', directive: 'Centrifuge tube_3 at 5000 RPM for 5 minutes' });
  add(300, { type: 'pipeline', activeGate: 0 });
  add(200, { type: 'pipeline', activeGate: 0, gateResult: true });
  add(150, { type: 'pipeline', activeGate: 1 });
  add(200, { type: 'pipeline', activeGate: 1, gateResult: true });
  add(150, { type: 'pipeline', activeGate: 3 });
  add(200, { type: 'pipeline', activeGate: 3, gateResult: true });
  add(200, { type: 'action', tool: 'centrifuge', rpm: 5000, duration_s: 300 });
  add(300, { type: 'approval_needed', approval: { id: 'ap-1', tool: 'centrifuge', riskClass: 'Actuation', reason: 'Actuation risk requires operator sign-off', deadline: 30 } });
  add(2000, { type: 'approval_granted', approvalId: 'ap-1' });
  add(200, { type: 'pipeline', activeGate: 4 });
  add(200, { type: 'pipeline', activeGate: 4, gateResult: true });
  add(200, { type: 'instrument', id: 'centrifuge', state: 'spinning', rpm: 5000 });
  add(2500, { type: 'instrument', id: 'centrifuge', state: 'idle', rpm: 0 });
  add(200, { type: 'pipeline', activeGate: 5 });
  add(200, { type: 'pipeline', activeGate: 5, gateResult: true });
  add(150, { type: 'pipeline', activeGate: 6 });
  add(200, { type: 'pipeline', activeGate: 6, gateResult: true });
  add(400, { type: 'run_end', runId: 'run-4' });

  // ── Phase 5: Absorbance Reading ──
  add(800, { type: 'run_start', runId: 'run-5', directive: 'Read absorbance of tube_1 at 280 nm' });
  add(300, { type: 'pipeline', activeGate: 0 });
  add(200, { type: 'pipeline', activeGate: 0, gateResult: true });
  add(150, { type: 'pipeline', activeGate: 2 });
  add(200, { type: 'pipeline', activeGate: 2, gateResult: true });
  add(150, { type: 'pipeline', activeGate: 3 });
  add(200, { type: 'pipeline', activeGate: 3, gateResult: true });
  add(200, { type: 'action', tool: 'read_absorbance', target: 'spectrophotometer', vessel: 'tube_1', wavelength: 280 });
  add(300, { type: 'instrument', id: 'spectrophotometer', state: 'reading' });
  add(800, { type: 'instrument', id: 'spectrophotometer', state: 'idle', reading: 0.63 });
  add(200, { type: 'pipeline', activeGate: 6, gateResults: [true, true, true, true, true, true] });
  add(200, { type: 'pipeline', activeGate: 6, gateResult: true });
  add(400, { type: 'run_end', runId: 'run-5' });

  // Loop delay
  add(1200, { type: 'loop' });

  return events;
}

export { buildTimeline, INITIAL_LAB, VESSEL_CAPACITIES, GATE_NAMES, REAGENT_COLORS, deepClone };
