import { useRef, useMemo, Suspense } from "react";
import { useFrame } from "@react-three/fiber";
import { useTexture } from "@react-three/drei";
import * as THREE from "three";

/*
 * A futuristic orbital station that revolves around the neutron star (the
 * "Security" node). Built procedurally from primitives and skinned with real
 * CC0 2K PBR metal maps (Poly Haven "metal_plate"): a central hub + command
 * dome, a spinning habitat ring on spokes (artificial gravity), solar-panel
 * arrays, antennae, and emissive windows / nav lights.
 *
 * Metalness is kept moderate (~0.5) on purpose: with no HDRI environment map
 * wired in, a fully-metallic hull would reflect only black. At 0.5 the lit
 * diffuse from the star + galactic fill reads as proper metal, and the normal/
 * roughness maps still give it crisp panel detail.
 */

const HULL = {
  map: "/textures/station_hull_diff.jpg",
  normalMap: "/textures/station_hull_nor.jpg",
  roughnessMap: "/textures/station_hull_rough.jpg",
  metalnessMap: "/textures/station_hull_metal.jpg",
};

function metalMaterial(maps, rx, ry) {
  const clone = (t) => {
    if (!t) return null;
    const c = t.clone();
    c.wrapS = c.wrapT = THREE.RepeatWrapping;
    c.repeat.set(rx, ry);
    c.anisotropy = 16;
    c.needsUpdate = true;
    return c;
  };
  const map = clone(maps.map);
  if (map) map.colorSpace = THREE.SRGBColorSpace;
  return new THREE.MeshStandardMaterial({
    map,
    normalMap: clone(maps.normalMap),
    roughnessMap: clone(maps.roughnessMap),
    metalnessMap: clone(maps.metalnessMap),
    metalness: 0.5,
    roughness: 1.0,
    color: new THREE.Color("#aeb6c2"),
  });
}

function StationModel({ station }) {
  const maps = useTexture(HULL);
  const spinRef = useRef();
  const ringRef = useRef();

  const hullFine = useMemo(() => metalMaterial(maps, 2, 2), [maps]);
  const hullRing = useMemo(() => metalMaterial(maps, 14, 2), [maps]);
  const windowMat = useMemo(
    () => new THREE.MeshStandardMaterial({ color: "#04121f", emissive: new THREE.Color("#7fd6ff"), emissiveIntensity: 2.6, roughness: 0.3 }),
    []
  );
  const panelMat = useMemo(
    () => new THREE.MeshStandardMaterial({ color: "#0a1a3a", emissive: new THREE.Color("#0e2a63"), emissiveIntensity: 0.6, metalness: 0.6, roughness: 0.3 }),
    []
  );
  const ringLightMat = useMemo(() => new THREE.MeshBasicMaterial({ color: "#8fe0ff" }), []);
  const navRed = useMemo(() => new THREE.MeshBasicMaterial({ color: "#ff5a4d" }), []);
  const navGrn = useMemo(() => new THREE.MeshBasicMaterial({ color: "#48ff9b" }), []);

  useFrame((_, dt) => {
    if (ringRef.current) ringRef.current.rotation.y += dt * 0.45; // habitat spin
    if (spinRef.current) spinRef.current.rotation.y += dt * 0.05; // slow overall drift
  });

  const s = station.scale ?? 0.85;
  return (
    <group ref={spinRef} scale={s}>
      {/* Central hub */}
      <mesh material={hullFine} castShadow>
        <cylinderGeometry args={[0.28, 0.28, 0.95, 32]} />
      </mesh>
      {/* Command dome + a ring of lit windows */}
      <mesh position={[0, 0.62, 0]} material={hullFine} castShadow>
        <sphereGeometry args={[0.3, 32, 24]} />
      </mesh>
      <mesh position={[0, 0.5, 0]} material={windowMat}>
        <cylinderGeometry args={[0.31, 0.31, 0.09, 32, 1, true]} />
      </mesh>
      {/* Lower docking module */}
      <mesh position={[0, -0.62, 0]} material={hullFine} castShadow>
        <cylinderGeometry args={[0.2, 0.24, 0.5, 24]} />
      </mesh>
      <mesh position={[0, -0.9, 0]} material={hullFine}>
        <cylinderGeometry args={[0.12, 0.12, 0.18, 16]} />
      </mesh>

      {/* Habitat ring on spokes (spins for artificial gravity) */}
      <group ref={ringRef}>
        <mesh rotation={[Math.PI / 2, 0, 0]} material={hullRing} castShadow>
          <torusGeometry args={[1.0, 0.12, 20, 72]} />
        </mesh>
        <mesh rotation={[Math.PI / 2, 0, 0]} material={ringLightMat}>
          <torusGeometry args={[1.0, 0.055, 12, 72]} />
        </mesh>
        {[0, 1, 2, 3].map((i) => (
          <group key={i} rotation={[0, (i * Math.PI) / 2, 0]}>
            <mesh position={[0.5, 0, 0]} rotation={[0, 0, Math.PI / 2]} material={hullFine}>
              <cylinderGeometry args={[0.03, 0.03, 1.0, 10]} />
            </mesh>
          </group>
        ))}
      </group>

      {/* Solar-panel arrays on booms (±X) */}
      {[1, -1].map((dir) => (
        <group key={dir}>
          <mesh position={[dir * 0.8, 0, 0]} rotation={[0, 0, Math.PI / 2]} material={hullFine}>
            <cylinderGeometry args={[0.03, 0.03, 1.1, 10]} />
          </mesh>
          {[-0.45, 0.45].map((oz, j) => (
            <mesh key={j} position={[dir * 1.5, 0, oz]} material={panelMat}>
              <boxGeometry args={[0.95, 0.02, 0.4]} />
            </mesh>
          ))}
        </group>
      ))}

      {/* Antenna + nav lights */}
      <group rotation={[0.2, 0, 0.3]}>
        <mesh position={[0.16, 1.05, 0.1]} material={hullFine}>
          <cylinderGeometry args={[0.012, 0.012, 0.55, 8]} />
        </mesh>
        <mesh position={[0.16, 1.34, 0.1]} material={navRed}>
          <sphereGeometry args={[0.03, 8, 8]} />
        </mesh>
      </group>
      <mesh position={[-0.22, -0.2, 0.28]} material={navGrn}>
        <sphereGeometry args={[0.028, 8, 8]} />
      </mesh>
      <mesh position={[0.32, 0.1, -0.28]} material={navRed}>
        <sphereGeometry args={[0.024, 8, 8]} />
      </mesh>
    </group>
  );
}

export function SpaceStation({ station }) {
  const orbit = useRef();
  useFrame((_, dt) => {
    if (orbit.current) orbit.current.rotation.y += dt * ((Math.PI * 2) / (station.period ?? 40));
  });
  return (
    <group ref={orbit} rotation={[station.inclination ?? 0.3, 0, 0]}>
      <group position={[station.distance ?? 3.6, 0, 0]}>
        <Suspense fallback={null}>
          <StationModel station={station} />
        </Suspense>
      </group>
    </group>
  );
}
