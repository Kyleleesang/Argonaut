import { Suspense, useEffect, useRef, useState } from "react";
import { Canvas, useFrame } from "@react-three/fiber";
import { CameraControls, Environment } from "@react-three/drei";
import * as THREE from "three";
import { BODIES, CORE } from "./config";
import { Planet } from "./Planet";
import { BlueStar } from "./BlueStar";
import { NebulaSky } from "./NebulaSky";
import { Hologram } from "./Hologram";

// Flip to true AFTER placing an HDRI at public/hdr/space.hdr (see ASSETS.md).
// Image-based lighting is a big realism lever — real reflections + ambient.
const USE_HDRI = false;

// An invisible sphere that makes a body clickable. `visible={false}` keeps it
// out of the render but still raycastable, and the slight oversize gives small
// bodies a forgiving hit area.
function PickTarget({ radius, position, onSelect }) {
  return (
    <mesh
      position={position}
      visible={false}
      onClick={(e) => { e.stopPropagation(); onSelect(); }}
      onPointerOver={() => { document.body.style.cursor = "pointer"; }}
      onPointerOut={() => { document.body.style.cursor = "auto"; }}
    >
      <sphereGeometry args={[radius * 1.15, 24, 24]} />
    </mesh>
  );
}

// Pins its children to a moving object's live world position. Used to keep the
// Citadel readout beside the orbiting station instead of the pulsar. Cost is
// one getWorldPosition per frame -- drei's Html re-projects every frame
// regardless, so following a moving target is effectively free.
function Follow({ targetRef, children }) {
  const g = useRef();
  useFrame(() => {
    if (g.current && targetRef.current) targetRef.current.getWorldPosition(g.current.position);
  });
  return <group ref={g}>{children}</group>;
}

// One body's orbit: an inclined plane containing a group that spins about its
// own Y. Driven from absolute elapsed time rather than accumulated `+= dt` so
// the phase can't drift after a stutter or a tab-out.
function OrbitGroup({ orbit, children }) {
  const spin = useRef();
  useFrame((state) => {
    if (!spin.current) return;
    const period = orbit?.period ?? 90;
    spin.current.rotation.y =
      (orbit?.phase ?? 0) + (state.clock.elapsedTime * Math.PI * 2) / period;
  });
  return (
    <group rotation={[orbit?.inclination ?? 0, 0, 0]}>
      <group ref={spin}>{children}</group>
    </group>
  );
}

// Camera offset for a three-quarter SUNLIT view of a body.
//
// The star sits at the origin, so offsetting the camera radially outward -- the
// obvious reading of "stand back from it" -- puts the camera squarely on the
// body's night side and renders it as a black disc. Swinging the offset back
// toward the star and round to one side gives a gibbous phase: mostly lit, but
// with a terminator still in frame so the body reads as a sphere rather than a
// flat lit circle (which is what a head-on "full" phase looks like).
const _UP = new THREE.Vector3(0, 1, 0);
const _rad = new THREE.Vector3();
const _tan = new THREE.Vector3();
const _off = new THREE.Vector3();
function sunlitOffset(p, d) {
  _rad.copy(p).normalize();
  _tan.crossVectors(_UP, _rad);
  if (_tan.lengthSq() < 1e-6) _tan.set(1, 0, 0); // body directly over a pole
  _tan.normalize();
  return _off
    .copy(_rad).multiplyScalar(-0.62)  // back toward the star (~52 deg phase)
    .addScaledVector(_tan, 0.72)       // ... swung round to one side
    .addScaledVector(_UP, 0.31)        // ... and lifted a little
    .normalize()
    .multiplyScalar(d);
}

function Rig({ focus, controls, bodyRefs }) {
  const following = useRef(false);
  const v = useRef(new THREE.Vector3());
  const lastAz = useRef(null);

  useEffect(() => {
    const c = controls.current;
    if (!c) return;
    following.current = false;
    lastAz.current = null;
    if (focus < 0) {
      c.setLookAt(0, 24, 72, 0, 0, 0, true);
      return;
    }
    const el = bodyRefs.current[focus];
    if (!el) return;
    el.getWorldPosition(v.current); // bodies move, so read the LIVE position
    const { x, y, z } = v.current;
    const o = sunlitOffset(v.current, BODIES[focus].framing);
    c.setLookAt(x + o.x, y + o.y, z + o.z, x, y, z, true);
    // Hand over to per-frame following once the cinematic fly-to has landed;
    // following during the transition would cancel it.
    const id = setTimeout(() => { following.current = true; }, 1300);
    return () => clearTimeout(id);
  }, [focus, controls, bodyRefs]);

  // Keep the camera glued to the focused body as it orbits. `moveTo` shifts the
  // orbit centre AND the camera together, so the framing survives and the user
  // can still drag/zoom around the body while it travels.
  useFrame(() => {
    const c = controls.current;
    if (!c || focus < 0 || !following.current) return;
    const el = bodyRefs.current[focus];
    if (!el) return;
    el.getWorldPosition(v.current);
    // Co-orbit. `moveTo` only translates, so as the body sweeps around the star
    // its sun direction rotates out from under a fixed camera offset and the
    // sunlit framing decays back to the night side within tens of seconds.
    // Rotating the camera about the body by the same azimuth the body travelled
    // holds the phase angle. Applied as a delta, so the user's own dragging is
    // preserved rather than overwritten.
    const az = Math.atan2(v.current.z, v.current.x);
    if (lastAz.current !== null) {
      let dAz = az - lastAz.current;
      if (dAz > Math.PI) dAz -= Math.PI * 2;
      if (dAz < -Math.PI) dAz += Math.PI * 2;
      // Sign is empirical: camera-controls' azimuth runs opposite to
      // atan2(z, x) here, and getting it backwards DOUBLES the drift
      // instead of cancelling it (verified on screen, not by reasoning).
      if (Math.abs(dAz) > 1e-7) c.rotate(-dAz, 0, false);
    }
    lastAz.current = az;
    c.moveTo(v.current.x, v.current.y, v.current.z, false);
  });
  return null;
}

export default function App() {
  const [focus, setFocus] = useState(-1);
  // `picked` is deliberately separate from `focus`: focus drives the camera and
  // starts on the Core, but nothing should be annotated until the user actually
  // asks for it, so this starts null and only a tab/body click sets it.
  const [picked, setPicked] = useState(null);
  const controls = useRef();
  const stationRef = useRef(); // live world position of Citadel's orbiting station
  const bodyRefs = useRef([]); // live world positions of the orbiting bodies
  const select = (i) => { setFocus(i); setPicked(i); };

  return (
    <>
      <Canvas
        dpr={[1, 2]}
        gl={{ antialias: true, toneMapping: THREE.ACESFilmicToneMapping, toneMappingExposure: 1.0 }}
        camera={{ position: [0, 24, 72], fov: 45, near: 0.1, far: 6000 }}
      >
        <color attach="background" args={["#03040a"]} />

        {/* Stable nebula backdrop (no fullscreen/depth hacks → no strobe). */}
        <NebulaSky />

        {/* The blue star at the origin is the system's light source. */}
        <pointLight position={[0, 0, 0]} intensity={3.4} decay={0} color="#e6ecff" />
        {/* Faint galactic fill so night sides aren't pure black: cool ambient
            starlight + a hemisphere term (dim nebula glow from "above", darker
            below) that mimics scattered light from the rest of the galaxy. */}
        <ambientLight intensity={0.1} color="#3a4a6b" />
        <hemisphereLight args={["#4a5488", "#0a1420", 0.3]} />

        {/* Only the star needs this Suspense — Planet handles its own texture
            suspense per body, so a slow map can't blank the whole system. */}
        <Suspense fallback={null}>
          {USE_HDRI && <Environment files="/hdr/space.hdr" background={false} />}
          <BlueStar radius={CORE.radius} />
        </Suspense>
        <PickTarget radius={CORE.radius} position={[0, 0, 0]} onSelect={() => select(-1)} />
        {picked === -1 && (
          <Hologram body={CORE} offset={[CORE.radius * 1.15, CORE.radius * 0.75, 0]} focused />
        )}

        {/* Each body travels inside its own orbit group, and everything pinned
            to that body — its pick target and its readout — rides along inside
            it, so nothing has to be re-synced to the motion by hand. */}
        {BODIES.map((b, i) => (
          <OrbitGroup key={b.name} orbit={b.orbit}>
            <group ref={(el) => (bodyRefs.current[i] = el)} position={b.position}>
              <Planet data={b} stationRef={b.station ? stationRef : undefined} />
              <PickTarget radius={b.radius} position={[0, 0, 0]} onSelect={() => select(i)} />
              {picked === i && !b.station && (
                <Hologram
                  body={b}
                  offset={[b.radius * 1.25, b.radius * 0.85, 0]}
                  focused
                />
              )}
            </group>
          </OrbitGroup>
        ))}

        {/* Station-bearing bodies pin their readout to the station instead, so
            it tracks the station's own orbit. Lives at scene root because
            Follow works in world space. */}
        {picked !== null && picked >= 0 && BODIES[picked].station && (
          <Follow targetRef={stationRef}>
            <Hologram body={BODIES[picked]} offset={[1.5, 0.9, 0]} focused />
          </Follow>
        )}

        <CameraControls ref={controls} makeDefault minDistance={2} maxDistance={800} />
        <Rig focus={focus} controls={controls} bodyRefs={bodyRefs} />

        {/* Postprocessing (EffectComposer) removed for stability: its depth-stencil
            handling flickered / black-screened on some GPUs and fought version
            pins. Glow now comes from emissive materials + the raymarched
            volumetric coronae + ACES tonemapping. A version-matched bloom can
            be re-added later once stable. */}
      </Canvas>

      <div className="hud">
        <div className="brand">
          <p className="eyebrow">{focus < 0 ? `Core · ${CORE.cls}` : `${BODIES[focus].name} · ${BODIES[focus].role}`}</p>
          <h1>ARGONAUT</h1>
          <p className="tag">Gravitational System</p>
        </div>

        <nav className="tabs" aria-label="Celestial bodies">
          <button className={focus < 0 ? "on" : ""} onClick={() => select(-1)}>Core</button>
          {BODIES.map((b, i) => (
            <button key={b.name} className={focus === i ? "on" : ""} onClick={() => select(i)}>
              {b.name}
            </button>
          ))}
        </nav>

        <div className="hint">Drag orbit · Scroll zoom<br />Click a tab to travel</div>
      </div>
    </>
  );
}
