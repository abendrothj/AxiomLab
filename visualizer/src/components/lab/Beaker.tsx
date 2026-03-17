import { useRef, useState, useEffect } from "react";
import { useFrame } from "@react-three/fiber";
import { animated, useSpring } from "@react-spring/three";
import * as THREE from "three";

interface Props {
  id: string;
  position: [number, number, number];
  liquidHeight: number;   // 0–1 fill fraction
  maxSafeLimit: number;   // 0–1 fraction at which amber ring sits
  rejected: boolean;
}

export default function Beaker({ id: _id, position, liquidHeight, maxSafeLimit, rejected }: Props) {
  const [showError, setShowError] = useState(false);
  const errorTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Flash error shield for 1500ms on rejection
  useEffect(() => {
    if (rejected) {
      setShowError(true);
      if (errorTimer.current) clearTimeout(errorTimer.current);
      errorTimer.current = setTimeout(() => setShowError(false), 1500);
    }
    return () => {
      if (errorTimer.current) clearTimeout(errorTimer.current);
    };
  }, [rejected]);

  const { fillY } = useSpring({
    fillY: Math.max(0, Math.min(1, liquidHeight)),
    config: { mass: 1, tension: 80, friction: 20 },
  });

  const { shieldOpacity } = useSpring({
    shieldOpacity: showError ? 0.45 : 0,
    config: { duration: 150 },
  });

  const glassHeight = 1.0;
  const glassRadius = 0.28;

  // Pulse the liquid on rejection
  const liquidRef = useRef<THREE.Mesh>(null);
  useFrame(() => {
    if (liquidRef.current && showError) {
      const t = Date.now() * 0.008;
      (liquidRef.current.material as THREE.MeshStandardMaterial).emissiveIntensity =
        0.3 + Math.abs(Math.sin(t)) * 0.5;
    } else if (liquidRef.current) {
      (liquidRef.current.material as THREE.MeshStandardMaterial).emissiveIntensity = 0.15;
    }
  });

  return (
    <group position={position}>
      {/* Glass cylinder — outer */}
      <mesh>
        <cylinderGeometry args={[glassRadius, glassRadius * 0.9, glassHeight, 24, 1, true]} />
        <meshPhysicalMaterial
          color="#c8eeff"
          transparent
          opacity={0.28}
          roughness={0}
          metalness={0}
          side={THREE.DoubleSide}
          depthWrite={false}
        />
      </mesh>

      {/* Base disk */}
      <mesh position={[0, -glassHeight / 2, 0]}>
        <cylinderGeometry args={[glassRadius * 0.9, glassRadius * 0.9, 0.04, 24]} />
        <meshStandardMaterial color="#1a2e40" metalness={0.3} roughness={0.6} />
      </mesh>

      {/* Liquid fill — animated height */}
      <animated.mesh
        ref={liquidRef}
        position-y={fillY.to((v) => -glassHeight / 2 + (v * glassHeight) / 2 + 0.02)}
        scale-y={fillY.to((v) => Math.max(0.001, v))}
      >
        <cylinderGeometry args={[glassRadius * 0.85, glassRadius * 0.82, glassHeight, 24]} />
        <meshStandardMaterial
          color={showError ? "#ff3b3b" : "#00d4ff"}
          emissive={showError ? "#ff1a00" : "#0066ff"}
          emissiveIntensity={0.15}
          transparent
          opacity={0.72}
        />
      </animated.mesh>

      {/* Amber max-safe ring */}
      <mesh position={[0, -glassHeight / 2 + maxSafeLimit * glassHeight, 0]} rotation={[Math.PI / 2, 0, 0]}>
        <torusGeometry args={[glassRadius + 0.015, 0.012, 8, 32]} />
        <meshStandardMaterial
          color="#ffaa00"
          emissive="#ffaa00"
          emissiveIntensity={0.6}
        />
      </mesh>

      {/* Error shield — red translucent dome */}
      <animated.mesh>
        <cylinderGeometry args={[glassRadius + 0.04, glassRadius * 0.95, glassHeight + 0.08, 24, 1, true]} />
        <animated.meshPhysicalMaterial
          color="#ff3b3b"
          transparent
          opacity={shieldOpacity}
          roughness={0.2}
          side={THREE.DoubleSide}
          depthWrite={false}
        />
      </animated.mesh>

      {/* Label glyph at base */}
      <mesh position={[0, -glassHeight / 2 - 0.1, 0]} rotation={[-Math.PI / 2, 0, 0]}>
        <ringGeometry args={[glassRadius * 0.3, glassRadius * 0.42, 6]} />
        <meshStandardMaterial color="#1a3a4a" />
      </mesh>
    </group>
  );
}
