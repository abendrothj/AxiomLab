import { useRef } from "react";
import { useFrame } from "@react-three/fiber";
import { animated, useSpring } from "@react-spring/three";
import * as THREE from "three";

interface Props {
  position: [number, number, number];
  temperature_mk: number; // millikelvin — 0 = off, e.g. 373000 = 100°C
}

// Converts millikelvin temperature to glow intensity (0–3)
function tempToIntensity(mk: number): number {
  return Math.min(3.0, mk / 400000);
}

// Interpolates color from cool blue → amber → red based on temperature
function tempToColor(mk: number): string {
  const t = Math.min(1, mk / 600000);
  if (t < 0.4) {
    // off to warm amber
    const s = t / 0.4;
    const r = Math.round(s * 255);
    const g = Math.round(s * 100);
    return `rgb(${r},${g},0)`;
  } else {
    // amber to red
    const s = (t - 0.4) / 0.6;
    const r = 255;
    const g = Math.round((1 - s) * 100);
    return `rgb(${r},${g},0)`;
  }
}

export default function HotPlate({ position, temperature_mk }: Props) {
  const surfaceRef = useRef<THREE.Mesh>(null);

  const intensity = tempToIntensity(temperature_mk);
  const color = tempToColor(temperature_mk);

  const { glowIntensity } = useSpring({
    glowIntensity: intensity,
    config: { mass: 2, tension: 40, friction: 20 },
  });

  // Subtle shimmer when hot
  useFrame(() => {
    if (surfaceRef.current && temperature_mk > 50000) {
      const mat = surfaceRef.current.material as THREE.MeshStandardMaterial;
      const shimmer = Math.abs(Math.sin(Date.now() * 0.003)) * 0.15;
      mat.emissiveIntensity = intensity + shimmer;
    }
  });

  return (
    <group position={position}>
      {/* Base housing */}
      <mesh position={[0, -0.06, 0]}>
        <boxGeometry args={[0.9, 0.12, 0.9]} />
        <meshStandardMaterial color="#1e3040" metalness={0.7} roughness={0.4} />
      </mesh>

      {/* Heating surface */}
      <animated.mesh ref={surfaceRef} position={[0, 0.01, 0]}>
        <cylinderGeometry args={[0.38, 0.38, 0.05, 32]} />
        <animated.meshStandardMaterial
          color={color}
          emissive={color}
          emissiveIntensity={glowIntensity}
          metalness={0.5}
          roughness={0.5}
        />
      </animated.mesh>

      {/* Heating coil pattern (decorative rings) */}
      {[0.12, 0.22, 0.32].map((r, i) => (
        <mesh key={i} position={[0, 0.04, 0]} rotation={[Math.PI / 2, 0, 0]}>
          <torusGeometry args={[r, 0.012, 12, 48]} />
          <animated.meshStandardMaterial
            color={color}
            emissive={color}
            emissiveIntensity={glowIntensity.to((v) => v * 0.7)}
          />
        </mesh>
      ))}

      {/* Control knob */}
      <mesh position={[0.42, 0, 0.12]} rotation={[0, 0, Math.PI / 2]}>
        <cylinderGeometry args={[0.04, 0.04, 0.06, 12]} />
        <meshStandardMaterial color="#1a3040" metalness={0.8} roughness={0.3} />
      </mesh>

      {/* Temperature indicator LED */}
      <mesh position={[0.42, 0.05, -0.1]}>
        <sphereGeometry args={[0.025, 8, 8]} />
        <animated.meshStandardMaterial
          color={temperature_mk > 0 ? "#ff4400" : "#1a1a1a"}
          emissive={temperature_mk > 0 ? "#ff2200" : "#000000"}
          emissiveIntensity={temperature_mk > 0 ? 1.5 : 0}
        />
      </mesh>

    </group>
  );
}
