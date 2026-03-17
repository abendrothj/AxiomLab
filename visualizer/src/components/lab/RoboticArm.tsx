import { useRef } from "react";
import { useFrame } from "@react-three/fiber";
import { animated, useSpring } from "@react-spring/three";
import * as THREE from "three";

interface Props {
  target: { x: number; y: number; z: number };
  active: boolean;
}

export default function RoboticArm({ target, active }: Props) {
  const groupRef = useRef<THREE.Group>(null);

  const { posX, posZ } = useSpring({
    posX: target.x * 0.01,
    posZ: target.z * 0.01,
    config: { mass: 1, tension: 120, friction: 26 },
  });

  const { emissiveIntensity } = useSpring({
    emissiveIntensity: active ? 1.2 : 0.3,
    config: { duration: 200 },
  });

  // Glow color
  const glowColor = active ? "#00d4ff" : "#1a4a6a";

  useFrame(() => {
    if (groupRef.current) {
      // Subtle idle sway
      groupRef.current.rotation.y = Math.sin(Date.now() * 0.0005) * 0.04;
    }
  });

  return (
    <animated.group ref={groupRef} position-x={posX} position-y={0} position-z={posZ}>
      {/* Base */}
      <mesh position={[-3.5, 0.15, 0]}>
        <boxGeometry args={[0.5, 0.3, 0.5]} />
        <meshStandardMaterial color="#2a4a5a" metalness={0.8} roughness={0.3} />
      </mesh>

      {/* Vertical column */}
      <mesh position={[-3.5, 1.1, 0]}>
        <boxGeometry args={[0.18, 1.6, 0.18]} />
        <meshStandardMaterial color="#2a5a6a" metalness={0.9} roughness={0.2} />
      </mesh>

      {/* Horizontal arm — spans the workspace */}
      <mesh position={[-1, 1.9, 0]}>
        <boxGeometry args={[5, 0.14, 0.14]} />
        <meshStandardMaterial color="#1a3a50" metalness={0.9} roughness={0.2} />
      </mesh>

      {/* End-effector (gripper head) — slides along arm */}
      <animated.group position-x={posX} position-y={0} position-z={posZ}>
        <mesh position={[-1, 1.9, 0]}>
          <boxGeometry args={[0.22, 0.22, 0.22]} />
          <animated.meshStandardMaterial
            color={glowColor}
            emissive={glowColor}
            emissiveIntensity={emissiveIntensity}
            metalness={0.6}
            roughness={0.3}
          />
        </mesh>

        {/* Gripper fingers */}
        {[-0.09, 0.09].map((dx, i) => (
          <mesh key={i} position={[-1 + dx, 1.72, 0]}>
            <boxGeometry args={[0.05, 0.24, 0.05]} />
            <animated.meshStandardMaterial
              color={glowColor}
              emissive={glowColor}
              emissiveIntensity={emissiveIntensity}
              metalness={0.6}
              roughness={0.3}
            />
          </mesh>
        ))}
      </animated.group>

      {/* Rail glow strip */}
      <mesh position={[-1, 1.97, 0]}>
        <boxGeometry args={[5, 0.03, 0.03]} />
        <meshStandardMaterial
          color={active ? "#00d4ff" : "#0a2535"}
          emissive={active ? "#00d4ff" : "#000000"}
          emissiveIntensity={active ? 0.8 : 0}
        />
      </mesh>
    </animated.group>
  );
}
