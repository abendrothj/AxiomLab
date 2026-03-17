import { useReducer } from "react";
import { Canvas } from "@react-three/fiber";
import { OrbitControls, Grid, Environment } from "@react-three/drei";

import { ToolExecutionEvent, DispenseParams, MoveArmParams, AbsorbanceParams, TemperatureParams, StirParams } from "../types";
import RoboticArm from "./lab/RoboticArm";
import Beaker from "./lab/Beaker";
import Sensor from "./lab/Sensor";
import HotPlate from "./lab/HotPlate";
import Spectro from "./lab/Spectro";

// ── Lab State ─────────────────────────────────────────────────────────────────

interface BeakerState {
  liquidHeight: number;   // 0–1
  maxSafeLimit: number;   // 0–1  (amber ring position)
  rejected: boolean;
}

interface SensorState {
  reading: number;
  active: boolean;
  unit: string;
}

interface LabState {
  armTarget: { x: number; y: number; z: number };
  armActive: boolean;
  beakers: Record<string, BeakerState>;
  sensors: Record<string, SensorState>;
  hotplates: Record<string, { temperature_mk: number }>;
  spectroActive: boolean;
  centrifugeRpm: number;
}

const INITIAL_STATE: LabState = {
  armTarget: { x: 0, y: 200, z: 0 },
  armActive: false,
  beakers: {
    Beaker_A: { liquidHeight: 0, maxSafeLimit: 0.6, rejected: false },
    Beaker_B: { liquidHeight: 0, maxSafeLimit: 0.6, rejected: false },
    Vessel_C: { liquidHeight: 0, maxSafeLimit: 0.7, rejected: false },
  },
  sensors: {
    Sensor_pH:   { reading: 7.0, active: false, unit: " pH" },
    Sensor_Temp: { reading: 298.15, active: false, unit: " K" },
    Sensor_Pressure: { reading: 101.325, active: false, unit: " kPa" },
  },
  hotplates: {
    HotPlate_1: { temperature_mk: 0 },
    HotPlate_2: { temperature_mk: 0 },
  },
  spectroActive: false,
  centrifugeRpm: 0,
};

// ── Reducer ───────────────────────────────────────────────────────────────────

type Action =
  | { type: "TOOL_EVENT"; payload: ToolExecutionEvent };

const MAX_VOLUME_UL = 1000; // matches CapabilityPolicy default_lab

function reducer(state: LabState, action: Action): LabState {
  if (action.type !== "TOOL_EVENT") return state;

  const ev = action.payload;
  const params = ev.params;
  const success = ev.status === "success";

  switch (ev.tool) {
    case "move_arm": {
      const p = params as unknown as MoveArmParams;
      return {
        ...state,
        armTarget: { x: p.x ?? 0, y: p.y ?? 200, z: p.z ?? 0 },
        armActive: success,
      };
    }

    case "dispense": {
      const p = params as unknown as DispenseParams;
      const beakerId = p.pump_id ?? ev.target;
      if (!state.beakers[beakerId]) return state;

      if (!success) {
        return {
          ...state,
          beakers: {
            ...state.beakers,
            [beakerId]: { ...state.beakers[beakerId], rejected: true },
          },
        };
      }

      const fillDelta = (p.volume_ul ?? 0) / MAX_VOLUME_UL;
      const prev = state.beakers[beakerId].liquidHeight;
      return {
        ...state,
        beakers: {
          ...state.beakers,
          [beakerId]: {
            ...state.beakers[beakerId],
            liquidHeight: Math.min(1, prev + fillDelta),
            rejected: false,
          },
        },
      };
    }

    case "aspirate": {
      // Reverse of dispense
      const p = params as unknown as DispenseParams;
      const beakerId = p.pump_id ?? ev.target;
      if (!state.beakers[beakerId]) return state;
      const fillDelta = (p.volume_ul ?? 0) / MAX_VOLUME_UL;
      const prev = state.beakers[beakerId].liquidHeight;
      return {
        ...state,
        beakers: {
          ...state.beakers,
          [beakerId]: {
            ...state.beakers[beakerId],
            liquidHeight: Math.max(0, prev - fillDelta),
            rejected: !success,
          },
        },
      };
    }

    case "read_absorbance": {
      const p = params as unknown as AbsorbanceParams;
      const wavelength = p.wavelength_nm ?? 595;
      return {
        ...state,
        spectroActive: success,
        sensors: {
          ...state.sensors,
          Sensor_Absorbance: {
            reading: wavelength,
            active: success,
            unit: " nm",
          },
        },
      };
    }

    case "set_temperature": {
      const p = params as unknown as TemperatureParams;
      const plateId = ev.target in state.hotplates ? ev.target : "HotPlate_1";
      return {
        ...state,
        hotplates: {
          ...state.hotplates,
          [plateId]: { temperature_mk: success ? (p.target_mk ?? 0) : state.hotplates[plateId]?.temperature_mk ?? 0 },
        },
      };
    }

    case "read_temperature": {
      const p = params as unknown as TemperatureParams;
      const vesselId = (params as { vessel_id?: string }).vessel_id ?? ev.target;
      const sensorKey = `Sensor_Temp_${vesselId}`;
      return {
        ...state,
        sensors: {
          ...state.sensors,
          Sensor_Temp: {
            reading: (p.target_mk ?? 298000) / 1000,
            active: success,
            unit: " K",
          },
          [sensorKey]: {
            reading: (p.target_mk ?? 298000) / 1000,
            active: success,
            unit: " K",
          },
        },
      };
    }

    case "stir": {
      const p = params as unknown as StirParams;
      return {
        ...state,
        centrifugeRpm: success ? (p.rpm ?? 0) : 0,
      };
    }

    case "read_ph": {
      const reading = success ? (params as { value?: number }).value ?? 7.0 : state.sensors["Sensor_pH"]?.reading ?? 7.0;
      return {
        ...state,
        sensors: {
          ...state.sensors,
          Sensor_pH: { ...state.sensors["Sensor_pH"], reading, active: success },
        },
      };
    }

    case "read_pressure": {
      return {
        ...state,
        sensors: {
          ...state.sensors,
          Sensor_Pressure: {
            ...state.sensors["Sensor_Pressure"],
            reading: success ? ((params as { value?: number }).value ?? 101.325) : state.sensors["Sensor_Pressure"]?.reading ?? 101.325,
            active: success,
          },
        },
      };
    }

    case "centrifuge": {
      return { ...state, centrifugeRpm: success ? ((params as { rpm?: number }).rpm ?? 0) : 0 };
    }

    default:
      return state;
  }
}

// ── Scene ─────────────────────────────────────────────────────────────────────

function LabScene({ state }: { state: LabState }) {
  return (
    <>
      {/* Scene background & atmosphere */}
      <color attach="background" args={["#0e1620"]} />
      <fog attach="fog" args={["#0e1620", 16, 38]} />

      {/* Lab room walls — give the void a boundary */}
      {/* Back wall */}
      <mesh position={[0, 4, -5]} receiveShadow>
        <planeGeometry args={[22, 14]} />
        <meshStandardMaterial color="#141e2a" roughness={0.9} metalness={0.1} />
      </mesh>
      {/* Left wall */}
      <mesh position={[-8, 4, 0]} rotation={[0, Math.PI / 2, 0]} receiveShadow>
        <planeGeometry args={[18, 14]} />
        <meshStandardMaterial color="#111a26" roughness={0.9} metalness={0.1} />
      </mesh>
      {/* Right wall */}
      <mesh position={[8, 4, 0]} rotation={[0, -Math.PI / 2, 0]} receiveShadow>
        <planeGeometry args={[18, 14]} />
        <meshStandardMaterial color="#111a26" roughness={0.9} metalness={0.1} />
      </mesh>
      {/* Ceiling */}
      <mesh position={[0, 10, 0]} rotation={[Math.PI / 2, 0, 0]} receiveShadow>
        <planeGeometry args={[22, 18]} />
        <meshStandardMaterial color="#0d1520" roughness={1.0} metalness={0} />
      </mesh>
      {/* Ceiling strip lights (emissive panels) */}
      {[-4, 0, 4].map((x, i) => (
        <mesh key={i} position={[x, 9.5, -1]}>
          <boxGeometry args={[1.2, 0.05, 6]} />
          <meshStandardMaterial color="#c8e8ff" emissive="#b0d8ff" emissiveIntensity={1.2} />
        </mesh>
      ))}

      {/* Lighting */}
      <ambientLight intensity={1.1} color="#d4eaff" />
      <directionalLight position={[5, 10, 5]} intensity={2.0} color="#ffffff" castShadow />
      <directionalLight position={[-6, 8, -4]} intensity={1.0} color="#ddeeff" />
      <pointLight position={[0, 6, 0]} intensity={2.0} color="#ffffff" />
      <pointLight position={[-4, 4, 2]} intensity={1.2} color="#b0d8ff" />
      <pointLight position={[4, 4, -2]} intensity={1.2} color="#c0ffe0" />
      <spotLight position={[0, 9, 2]} intensity={4.0} angle={0.7} penumbra={0.5} color="#ffffff" castShadow />

      {/* Floor grid */}
      <Grid
        args={[20, 20]}
        position={[0, -0.55, 0]}
        cellSize={1}
        cellThickness={0.6}
        cellColor="#2a3a55"
        sectionSize={4}
        sectionThickness={1.2}
        sectionColor="#1a5090"
        fadeDistance={18}
        fadeStrength={1.2}
        infiniteGrid
      />

      {/* Lab bench surface */}
      <mesh position={[0, -0.28, 0]} receiveShadow>
        <boxGeometry args={[14, 0.12, 6]} />
        <meshStandardMaterial color="#1a2a3a" metalness={0.4} roughness={0.6} />
      </mesh>

      {/* Robotic arm */}
      <RoboticArm target={state.armTarget} active={state.armActive} />

      {/* Beakers */}
      <Beaker
        id="Beaker_A"
        position={[-1, 0, -0.5]}
        liquidHeight={state.beakers["Beaker_A"]?.liquidHeight ?? 0}
        maxSafeLimit={state.beakers["Beaker_A"]?.maxSafeLimit ?? 0.6}
        rejected={state.beakers["Beaker_A"]?.rejected ?? false}
      />
      <Beaker
        id="Beaker_B"
        position={[1, 0, -0.5]}
        liquidHeight={state.beakers["Beaker_B"]?.liquidHeight ?? 0}
        maxSafeLimit={state.beakers["Beaker_B"]?.maxSafeLimit ?? 0.6}
        rejected={state.beakers["Beaker_B"]?.rejected ?? false}
      />
      <Beaker
        id="Vessel_C"
        position={[0, 0, 0.9]}
        liquidHeight={state.beakers["Vessel_C"]?.liquidHeight ?? 0}
        maxSafeLimit={state.beakers["Vessel_C"]?.maxSafeLimit ?? 0.7}
        rejected={state.beakers["Vessel_C"]?.rejected ?? false}
      />

      {/* Hot plates */}
      <HotPlate position={[0, -0.22, -1.8]} temperature_mk={state.hotplates["HotPlate_1"]?.temperature_mk ?? 0} />
      <HotPlate position={[2.5, -0.22, -1.8]} temperature_mk={state.hotplates["HotPlate_2"]?.temperature_mk ?? 0} />

      {/* Sensors */}
      <Sensor
        position={[-2.8, 0, 0.2]}
        label="pH"
        reading={state.sensors["Sensor_pH"]?.reading ?? 7}
        active={state.sensors["Sensor_pH"]?.active ?? false}
        unit=" pH"
      />
      <Sensor
        position={[-2.2, 0, 1.0]}
        label="Temp"
        reading={state.sensors["Sensor_Temp"]?.reading ?? 298.15}
        active={state.sensors["Sensor_Temp"]?.active ?? false}
        unit=" K"
      />
      <Sensor
        position={[-3.2, 0, 1.0]}
        label="Pressure"
        reading={state.sensors["Sensor_Pressure"]?.reading ?? 101.325}
        active={state.sensors["Sensor_Pressure"]?.active ?? false}
        unit=" kPa"
      />

      {/* Spectrophotometer */}
      <Spectro
        position={[4.5, 0.2, -1.5]}
        active={state.spectroActive}
        beamTarget={[-1, 0.2, -0.5]}
      />

      {/* Centrifuge (simplified rotor visual) */}
      <CentrifugeModel position={[3.5, 0, 1.2]} rpm={state.centrifugeRpm} />

      {/* Ambient environment */}
      <Environment preset="warehouse" />
    </>
  );
}

// ── Centrifuge ─────────────────────────────────────────────────────────────────

import { useRef } from "react";
import { useFrame } from "@react-three/fiber";
import * as THREE from "three";

function CentrifugeModel({ position, rpm }: { position: [number, number, number]; rpm: number }) {
  const rotorRef = useRef<THREE.Mesh>(null);
  const speed = (rpm / 60) * Math.PI * 2; // rad/s → per-frame at 60fps: /60

  useFrame((_state, delta) => {
    if (rotorRef.current && rpm > 0) {
      rotorRef.current.rotation.y += speed * delta;
    }
  });

  return (
    <group position={position}>
      {/* Housing */}
      <mesh>
        <cylinderGeometry args={[0.45, 0.42, 0.6, 20]} />
        <meshStandardMaterial color="#0d1e2a" metalness={0.7} roughness={0.4} />
      </mesh>

      {/* Lid (open) */}
      <mesh position={[0, 0.32, 0.35]} rotation={[-Math.PI / 4, 0, 0]}>
        <cylinderGeometry args={[0.44, 0.44, 0.06, 20]} />
        <meshStandardMaterial color="#0a1820" metalness={0.7} roughness={0.5} />
      </mesh>

      {/* Rotor */}
      <mesh ref={rotorRef} position={[0, 0.1, 0]}>
        <cylinderGeometry args={[0.32, 0.32, 0.12, 8]} />
        <meshStandardMaterial
          color={rpm > 0 ? "#1a4060" : "#0d2030"}
          emissive={rpm > 0 ? "#0066aa" : "#000000"}
          emissiveIntensity={rpm > 0 ? 0.4 : 0}
          metalness={0.9}
          roughness={0.2}
        />
      </mesh>

      {/* Rotor arms */}
      {[0, 1, 2, 3].map((i) => (
        <mesh
          key={i}
          ref={i === 0 ? rotorRef : undefined}
          position={[0, 0.1, 0]}
          rotation={[0, (i * Math.PI) / 2, 0]}
        >
          <boxGeometry args={[0.6, 0.06, 0.08]} />
          <meshStandardMaterial color="#1a3a50" metalness={0.85} roughness={0.2} />
        </mesh>
      ))}

      {/* Speed indicator LED */}
      <mesh position={[0.42, 0.28, 0]}>
        <sphereGeometry args={[0.022, 8, 8]} />
        <meshStandardMaterial
          color={rpm > 0 ? "#ff8800" : "#1a1a00"}
          emissive={rpm > 0 ? "#ff6600" : "#000000"}
          emissiveIntensity={rpm > 0 ? 2 : 0}
        />
      </mesh>
    </group>
  );
}

// ── Main Component ─────────────────────────────────────────────────────────────

interface Sandbox3DProps {
  latestTool: ToolExecutionEvent | null;
}

export default function Sandbox3D({ latestTool }: Sandbox3DProps) {
  const [labState, dispatch] = useReducer(reducer, INITIAL_STATE);

  // Dispatch whenever a new tool event arrives
  const prevTool = useRef<ToolExecutionEvent | null>(null);
  if (latestTool && latestTool !== prevTool.current) {
    prevTool.current = latestTool;
    dispatch({ type: "TOOL_EVENT", payload: latestTool });
  }

  return (
    <Canvas
      camera={{ position: [0, 8, 12], fov: 50, near: 0.1, far: 100 }}
      style={{ width: "100%", height: "100%", background: "#111820" }}
      shadows
    >
      <LabScene state={labState} />
      <OrbitControls
        enableDamping
        dampingFactor={0.06}
        minPolarAngle={0.2}
        maxPolarAngle={Math.PI / 2.1}
        minDistance={4}
        maxDistance={25}
      />
    </Canvas>
  );
}
