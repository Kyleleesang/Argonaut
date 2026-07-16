import { Suspense, useEffect, useRef, useState } from "react";
import { Canvas } from "@react-three/fiber";
import { CameraControls, Environment } from "@react-three/drei";
import * as THREE from "three";
import { BODIES, CORE } from "./config";
import { Planet } from "./Planet";
import { NeutronStar } from "./NeutronStar";
import { NebulaSky } from "./NebulaSky";

// Flip to true AFTER placing an HDRI at public/hdr/space.hdr (see ASSETS.md).
// Image-based lighting is a big realism lever — real reflections + ambient.
const USE_HDRI = false;

function Rig({ focus, controls }) {
  useEffect(() => {
    const c = controls.current;
    if (!c) return;
    if (focus < 0) {
      c.setLookAt(0, 9, 26, 0, 0, 0, true);
      return;
    }
    const b = BODIES[focus];
    const [x, y, z] = b.position;
    const len = Math.hypot(x, y, z) || 1;
    const d = b.framing;
    const cx = x + (x / len) * d;
    const cy = y + d * 0.35 + 1.2;
    const cz = z + (z / len) * d;
    c.setLookAt(cx, cy, cz, x, y, z, true); // true = smooth cinematic transition
  }, [focus, controls]);
  return null;
}

export default function App() {
  const [focus, setFocus] = useState(-1);
  const controls = useRef();

  return (
    <>
      <Canvas
        dpr={[1, 2]}
        gl={{ antialias: true, toneMapping: THREE.ACESFilmicToneMapping, toneMappingExposure: 1.0 }}
        camera={{ position: [0, 9, 26], fov: 45, near: 0.1, far: 6000 }}
      >
        <color attach="background" args={["#03040a"]} />

        {/* Stable nebula backdrop (no fullscreen/depth hacks → no strobe). */}
        <NebulaSky />

        {/* The neutron star at the origin is the system's light (blue-white, hot). */}
        <pointLight position={[0, 0, 0]} intensity={3.4} decay={0} color="#e6ecff" />
        {/* Faint galactic fill so night sides aren't pure black: cool ambient
            starlight + a hemisphere term (dim nebula glow from "above", darker
            below) that mimics scattered light from the rest of the galaxy. */}
        <ambientLight intensity={0.1} color="#3a4a6b" />
        <hemisphereLight args={["#4a5488", "#0a1420", 0.3]} />

        <Suspense fallback={null}>
          {USE_HDRI && <Environment files="/hdr/space.hdr" background={false} />}
          <NeutronStar radius={2.2} spin={0.5} beamLength={5} />
          {BODIES.map((b) => (
            <Planet key={b.name} data={b} />
          ))}
        </Suspense>

        <CameraControls ref={controls} makeDefault minDistance={2} maxDistance={800} />
        <Rig focus={focus} controls={controls} />

        {/* Postprocessing (EffectComposer) removed for stability: its depth-stencil
            handling flickered / black-screened on some GPUs and fought version
            pins. Glow now comes from emissive materials + the neutron star's
            additive corona shells + ACES tonemapping. A version-matched bloom
            can be re-added later once stable. */}
      </Canvas>

      <div className="hud">
        <div className="brand">
          <p className="eyebrow">{focus < 0 ? `Core · ${CORE.cls}` : `${BODIES[focus].name} · ${BODIES[focus].role}`}</p>
          <h1>ARGONAUT</h1>
          <p className="tag">Gravitational System</p>
        </div>

        <nav className="tabs" aria-label="Celestial bodies">
          <button className={focus < 0 ? "on" : ""} onClick={() => setFocus(-1)}>Core</button>
          {BODIES.map((b, i) => (
            <button key={b.name} className={focus === i ? "on" : ""} onClick={() => setFocus(i)}>
              {b.name}
            </button>
          ))}
        </nav>

        <div className="hint">Drag orbit · Scroll zoom<br />Click a tab to travel</div>
      </div>
    </>
  );
}
