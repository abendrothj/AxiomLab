import { useRef } from "react";
import { useFrame } from "@react-three/fiber";
import { animated, useSpring } from "@react-spring/three";
import { Text } from "@react-three/drei";
import * as THREE from "three";

interface Props {
  position: [number, number, number];
  label: string;
  reading: number;
  active: boolean;
  unit?: string;
}

export default function Sensor({ position, label: _label, reading, active, unit = "" }: Props) {
  const indicatorRef = useRef<THREE.Mesh>(null);

  const { scale, emissive } = useSpring({
    scale: active ? 1.4 : 1.0,
    emissive: active ? 0.9 : 0.1,
    config: { mass: 0.5, tension: 200, friction: 15 },
  });

  // Pulse indicator on active
  useFrame(() => {
    if (indicatorRef.current) {
      const mat = indicatorRef.current.material as THREE.MeshStandardMaterial;
      if (active) {
        mat.emissiveIntensity = 0.5 + Math.abs(Math.sin(Date.now() * 0.004)) * 0.6;
      } else {
        mat.emissiveIntensity = 0.05;
      }
    }
  });

  const readingStr = active ? `${reading.toFixed(2)}${unit}` : "---";

  return (
    <group position={position}>
      {/* Cylindrical probe body */}
      <mesh>
        <cylinderGeometry args={[0.06, 0.08, 0.7, 12]} />
        <meshStandardMaterial color="#1e3a4a" metalness={0.8} roughness={0.3} />
      </mesh>

      {/* Probe tip */}
      <mesh position={[0, -0.4, 0]}>
        <cylinderGeometry args={[0.035, 0.01, 0.2, 10]} />
        <meshStandardMaterial color="#2a6a8a" metalness={0.9} roughness={0.1} />
      </mesh>

      {/* Indicator sphere */}
      <animated.mesh ref={indicatorRef} position={[0, 0.42, 0]} scale={scale}>
        <sphereGeometry args={[0.07, 16, 16]} />
        <animated.meshStandardMaterial
          color={active ? "#00ff9d" : "#1a3a2a"}
          emissive={active ? "#00ff9d" : "#000000"}
          emissiveIntensity={emissive}
        />
      </animated.mesh>

      {/* Floating readout — only shown when active */}
      {active && (
        <Text
          position={[0, 0.85, 0]}
          fontSize={0.12}
          color="#00ff9d"
          anchorX="center"
          anchorY="bottom"
          font={undefined}
        >
          {readingStr}
        </Text>
      )}

      {/* Cable stub */}
      <mesh position={[0.08, 0.3, 0]} rotation={[0, 0, -0.4]}>
        <cylinderGeometry args={[0.015, 0.015, 0.35, 6]} />
        <meshStandardMaterial color="#0a1520" />
      </mesh>
    </group>
  );
}
