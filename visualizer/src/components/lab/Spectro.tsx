import { useRef } from "react";
import { useFrame } from "@react-three/fiber";
import { animated, useSpring } from "@react-spring/three";
import * as THREE from "three";

interface Props {
  position: [number, number, number];
  active: boolean;
  beamTarget?: [number, number, number]; // world-space target for the beam
}

export default function Spectro({ position, active, beamTarget }: Props) {
  const beamRef = useRef<THREE.Mesh>(null);

  const { beamOpacity, beamScale } = useSpring({
    beamOpacity: active ? 0.85 : 0,
    beamScale: active ? 1 : 0.001,
    config: { duration: 300 },
  });

  // Beam shimmer
  useFrame(() => {
    if (beamRef.current && active) {
      const mat = beamRef.current.material as THREE.MeshStandardMaterial;
      mat.emissiveIntensity = 1.2 + Math.sin(Date.now() * 0.01) * 0.4;
    }
  });

  // Default beam direction: shoot along +X toward center stage
  const beamLength = beamTarget
    ? Math.sqrt(
        Math.pow(beamTarget[0] - position[0], 2) +
          Math.pow(beamTarget[2] - position[2], 2)
      )
    : 3.5;

  return (
    <group position={position}>
      {/* Instrument body */}
      <mesh>
        <boxGeometry args={[0.7, 0.35, 0.45]} />
        <meshStandardMaterial color="#1e3545" metalness={0.75} roughness={0.35} />
      </mesh>

      {/* Display panel recess */}
      <mesh position={[0, 0.08, 0.22]}>
        <boxGeometry args={[0.42, 0.16, 0.02]} />
        <meshStandardMaterial
          color={active ? "#001a10" : "#080e12"}
          emissive={active ? "#00ff9d" : "#000000"}
          emissiveIntensity={active ? 0.4 : 0}
        />
      </mesh>

      {/* Display "scanlines" */}
      {active &&
        [-0.04, 0, 0.04].map((dy, i) => (
          <mesh key={i} position={[0, 0.08 + dy, 0.225]}>
            <boxGeometry args={[0.35, 0.018, 0.005]} />
            <meshStandardMaterial
              color="#00ff9d"
              emissive="#00ff9d"
              emissiveIntensity={0.6}
              transparent
              opacity={0.5}
            />
          </mesh>
        ))}

      {/* Source lamp port */}
      <mesh position={[0.36, 0, 0]} rotation={[0, 0, Math.PI / 2]}>
        <cylinderGeometry args={[0.04, 0.06, 0.12, 12]} />
        <meshStandardMaterial color="#1a3040" metalness={0.9} roughness={0.2} />
      </mesh>

      {/* Cyan analysis beam */}
      <animated.mesh
        ref={beamRef}
        position={[0.36 + beamLength / 2, 0, 0]}
        rotation={[0, 0, Math.PI / 2]}
        scale-y={beamScale}
      >
        <cylinderGeometry args={[0.015, 0.015, beamLength, 8]} />
        <animated.meshStandardMaterial
          color="#00d4ff"
          emissive="#00d4ff"
          emissiveIntensity={1.2}
          transparent
          opacity={beamOpacity}
          depthWrite={false}
        />
      </animated.mesh>

      {/* Power LED */}
      <mesh position={[-0.32, 0.1, 0.22]}>
        <sphereGeometry args={[0.018, 8, 8]} />
        <meshStandardMaterial
          color={active ? "#00ff9d" : "#0a2015"}
          emissive={active ? "#00ff9d" : "#000000"}
          emissiveIntensity={active ? 2 : 0}
        />
      </mesh>

      {/* Feet */}
      {[
        [-0.28, -0.2],
        [0.28, -0.2],
        [-0.28, 0.2],
        [0.28, 0.2],
      ].map(([fx, fz], i) => (
        <mesh key={i} position={[fx, -0.19, fz]}>
          <cylinderGeometry args={[0.03, 0.03, 0.04, 8]} />
          <meshStandardMaterial color="#0a1520" />
        </mesh>
      ))}
    </group>
  );
}
