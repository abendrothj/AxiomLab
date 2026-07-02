import { useState, useEffect, useRef, useCallback } from 'react';
import { buildTimeline, INITIAL_LAB, VESSEL_CAPACITIES, deepClone } from './demoEngine';
import { api } from './api';

const POSITIONS = {
  home: { x: 50, y: 400 },
  reservoir: { x: 80, y: 200 },
  beaker_A: { x: 220, y: 180 },
  beaker_B: { x: 220, y: 300 },
  tube_1: { x: 380, y: 180 },
  tube_2: { x: 450, y: 180 },
  tube_3: { x: 520, y: 250 },
  plate_wells: { x: 370, y: 320 },
  spectrophotometer: { x: 620, y: 160 },
  ph_meter: { x: 620, y: 280 },
  incubator: { x: 850, y: 160 },
  centrifuge: { x: 850, y: 300 },
};

function createInitialState() {
  return {
    system: { running: false, backend: 'simulator', phase: 'idle', connected: false },
    pipeline: { activeGate: -1, results: new Array(7).fill(null) },
    lab: deepClone(INITIAL_LAB),
    arm: { x: POSITIONS.home.x, y: POSITIONS.home.y, targetX: POSITIONS.home.x, targetY: POSITIONS.home.y, carrying: null },
    instruments: {
      spectrophotometer: { state: 'idle', lastReading: null, wavelength: null },
      ph_meter: { state: 'idle', lastReading: null },
      incubator: { state: 'idle', lastReading: null, temp: 22, targetTemp: null },
      centrifuge: { state: 'idle', lastReading: null, rpm: 0 },
    },
    currentRun: null,
    currentAction: null,
    approvals: [],
    events: [],
    principal: null,
  };
}

function lerp(a, b, t) { return a + (b - a) * t; }

export function useLabState() {
  const [state, setState] = useState(createInitialState);
  const stateRef = useRef(state);
  stateRef.current = state;

  const isDemo = useRef(typeof window !== 'undefined' && window.location.search.includes('demo'));
  const demoRef = useRef(null);
  const wsRef = useRef(null);
  const pollRef = useRef(null);
  const animRef = useRef(null);

  const updateState = useCallback((patch) => {
    setState(prev => {
      const next = { ...prev };
      for (const [key, val] of Object.entries(patch)) {
        if (key === 'lab') {
          next.lab = deepClone(val);
        } else if (key === 'pipeline') {
          next.pipeline = { ...prev.pipeline, ...val };
        } else if (key === 'instruments') {
          next.instruments = { ...prev.instruments, ...val };
        } else if (key === 'system') {
          next.system = { ...prev.system, ...val };
        } else if (key === 'arm') {
          next.arm = { ...prev.arm, ...val };
        } else if (key === 'events') {
          next.events = [...prev.events, ...val].slice(-60);
        } else {
          next[key] = val;
        }
      }
      return next;
    });
  }, []);

  // Arm animation loop
  useEffect(() => {
    function animate() {
      setState(prev => {
        const arm = prev.arm;
        const dx = arm.targetX - arm.x;
        const dy = arm.targetY - arm.y;
        const dist = Math.sqrt(dx * dx + dy * dy);
        if (dist < 1.5) return prev;
        const speed = 0.12;
        return {
          ...prev,
          arm: {
            ...arm,
            x: lerp(arm.x, arm.targetX, speed),
            y: lerp(arm.y, arm.targetY, speed),
          },
        };
      });
      animRef.current = requestAnimationFrame(animate);
    }
    animRef.current = requestAnimationFrame(animate);
    return () => cancelAnimationFrame(animRef.current);
  }, []);

  // Demo mode
  useEffect(() => {
    if (!isDemo.current) return;

    const timeline = buildTimeline();
    let idx = 0;
    let startTime = null;
    const labState = deepClone(INITIAL_LAB);
    const gateResults = new Array(7).fill(null);

    function tick() {
      const now = performance.now();
      if (!startTime) startTime = now;
      const elapsed = now - startTime;

      while (idx < timeline.length && timeline[idx].t <= elapsed) {
        const ev = timeline[idx];
        switch (ev.type) {
          case 'system':
            updateState({ system: { ...stateRef.current.system, ...ev.system } });
            break;
          case 'run_start':
            updateState({ currentRun: { id: ev.runId, directive: ev.directive }, pipeline: { results: new Array(7).fill(null), activeGate: -1 }, events: [{ event: 'run_started', id: ev.runId, directive: ev.directive }] });
            gateResults.fill(null);
            break;
          case 'run_end':
            updateState({ currentRun: null, pipeline: { activeGate: -1, results: new Array(7).fill(null) }, currentAction: null, events: [{ event: 'run_completed', id: ev.runId }] });
            break;
          case 'pipeline': {
            const results = [...gateResults];
            if (ev.gateResult !== undefined) results[ev.activeGate] = ev.gateResult;
            else gateResults[ev.activeGate] = null;
            updateState({ pipeline: { activeGate: ev.activeGate, results } });
            break;
          }
          case 'action':
            updateState({ currentAction: { tool: ev.tool, target: ev.target, vessel: ev.vessel, reagent: ev.reagent } });
            break;
          case 'arm': {
            if (ev.target) {
              const pos = POSITIONS[ev.target] || POSITIONS.home;
              updateState({ arm: { targetX: pos.x, targetY: pos.y, carrying: ev.carrying !== undefined ? ev.carrying : stateRef.current.arm.carrying } });
            } else if (ev.carrying !== undefined) {
              updateState({ arm: { carrying: ev.carrying } });
            }
            break;
          }
          case 'dispense': {
            const vessel = ev.vessel;
            const vol = ev.volume_ul || 0;
            labState.vessel_volumes[vessel] = (labState.vessel_volumes[vessel] || 0) + vol;
            const existing = labState.vessel_contents[vessel] || [];
            const entry = existing.find(c => c.reagent_id === ev.reagent);
            if (entry) entry.volume_ul += vol;
            else existing.push({ reagent_id: ev.reagent, volume_ul: vol, concentration_m: 0 });
            labState.vessel_contents[vessel] = existing;
            if (ev.reagent && labState.reagents[ev.reagent]) {
              labState.reagents[ev.reagent].volume_ul = Math.max(0, labState.reagents[ev.reagent].volume_ul - vol);
            }
            updateState({ lab: deepClone(labState) });
            break;
          }
          case 'instrument': {
            const insts = { ...stateRef.current.instruments };
            insts[ev.id] = { ...insts[ev.id], state: ev.state, lastReading: ev.reading ?? insts[ev.id].lastReading, rpm: ev.rpm ?? insts[ev.id].rpm };
            updateState({ instruments: insts });
            break;
          }
          case 'approval_needed':
            updateState({ approvals: [ev.approval] });
            break;
          case 'approval_granted':
            updateState({ approvals: [] });
            break;
          case 'loop':
            startTime = performance.now();
            idx = -1;
            gateResults.fill(null);
            Object.assign(labState, deepClone(INITIAL_LAB));
            updateState({
              lab: deepClone(INITIAL_LAB),
              pipeline: { activeGate: -1, results: new Array(7).fill(null) },
              approvals: [],
              currentRun: null,
              currentAction: null,
              instruments: createInitialState().instruments,
            });
            break;
        }
        idx++;
      }

      if (idx >= timeline.length) {
        startTime = performance.now();
        idx = 0;
        gateResults.fill(null);
        Object.assign(labState, deepClone(INITIAL_LAB));
        updateState({
          lab: deepClone(INITIAL_LAB),
          pipeline: { activeGate: -1, results: new Array(7).fill(null) },
          approvals: [],
          currentRun: null,
          currentAction: null,
        });
      }

      demoRef.current = requestAnimationFrame(tick);
    }

    demoRef.current = requestAnimationFrame(tick);
    return () => { if (demoRef.current) cancelAnimationFrame(demoRef.current); };
  }, [updateState]);

  // Live mode: WebSocket + polling
  useEffect(() => {
    if (isDemo.current) return;

    updateState({ system: { connected: false } });

    const protocol = location.protocol === 'https:' ? 'wss' : 'ws';
    const wsUrl = `${protocol}://${location.host}/ws`;
    let ws;

    function connect() {
      ws = new WebSocket(wsUrl);
      wsRef.current = ws;
      ws.onopen = () => updateState({ system: { connected: true } });
      ws.onclose = () => {
        updateState({ system: { connected: false } });
        setTimeout(connect, 3000);
      };
      ws.onerror = () => ws.close();
      ws.onmessage = (msg) => {
        try {
          const ev = JSON.parse(msg.data);
          updateState({ events: [ev] });
        } catch (_) {}
      };
    }

    connect();

    async function refresh() {
      try {
        const [status, lab, queue, approvals, audit] = await Promise.all([
          api.status(), api.lab(), api.queue(), api.approvals(), api.audit(),
        ]);
        updateState({
          system: { running: status.running, backend: status.backend, phase: status.running ? 'running' : 'idle' },
          lab: lab,
        });
      } catch (_) {}
    }

    refresh();
    pollRef.current = setInterval(refresh, 3000);

    return () => {
      clearInterval(pollRef.current);
      if (wsRef.current) wsRef.current.close();
    };
  }, [updateState]);

  const submit = useCallback(async (directive) => {
    if (isDemo.current) return;
    try {
      await api.pushDirective(directive);
      updateState({ events: [{ event: 'queued', directive }] });
    } catch (e) {
      console.error('Submit failed:', e);
    }
  }, [updateState]);

  const approveAction = useCallback(async (approvalId, approved) => {
    if (isDemo.current) return;
    try {
      await api.resolveApproval(approvalId, approved, approved ? 'Approved' : 'Denied');
    } catch (e) {
      console.error('Approval failed:', e);
    }
  }, []);

  return { state, isDemo: isDemo.current, submit, approveAction };
}
