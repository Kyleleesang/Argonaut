import { useRef, useEffect, useMemo, Suspense, Component } from "react";
import { useFrame } from "@react-three/fiber";
import { useTexture } from "@react-three/drei";
import * as THREE from "three";
import { NeutronStar } from "./NeutronStar";

/*
 * A body renders in one of two modes:
 *   - TexturedPlanet: real PBR maps from public/textures/ (the photoreal path)
 *   - PlainPlanet:    a clean colored PBR sphere (so the app boots with no assets)
 * Both share the atmosphere + ring add-ons. Realism is unlocked by ASSETS.md.
 */
// A failed/bad texture must never take down the scene. This boundary catches a
// load error (e.g. a wrong-format file) and falls back to the plain sphere.
class TexBoundary extends Component {
  constructor(p) { super(p); this.state = { failed: false }; }
  static getDerivedStateFromError() { return { failed: true }; }
  componentDidCatch(e) { console.warn("[Planet] texture failed → plain material:", e?.message || e); }
  render() { return this.state.failed ? this.props.fallback : this.props.children; }
}

export function Planet({ data }) {
  if (data.emissive) return <StarBody data={data} />;
  if (!data.textures) return <PlainPlanet data={data} />;
  const fallback = <PlainPlanet data={data} />;
  return (
    <TexBoundary fallback={fallback}>
      <Suspense fallback={fallback}>
        <TexturedPlanet data={data} />
      </Suspense>
    </TexBoundary>
  );
}

function sunDirFor(position) {
  // The star sits at the origin; light direction on this body points to origin.
  const v = new THREE.Vector3(-position[0], -position[1], -position[2]);
  if (v.lengthSq() === 0) v.set(0, 0, 1);
  return v.normalize();
}

function PlainPlanet({ data }) {
  const ref = useRef();
  useFrame((_, dt) => { if (ref.current) ref.current.rotation.y += dt * 0.03; });
  return (
    <group position={data.position}>
      <mesh ref={ref} castShadow receiveShadow>
        <sphereGeometry args={[data.radius, 96, 96]} />
        <meshStandardMaterial
          color={data.color || "#888888"}
          roughness={data.roughness ?? 0.9}
          metalness={data.metalness ?? 0.0}
        />
      </mesh>
      {data.atmosphere && <Atmosphere radius={data.radius} atmosphere={data.atmosphere} sunDir={sunDirFor(data.position)} />}
      {data.rings && <Rings radius={data.radius} rings={data.rings} />}
    </group>
  );
}

function TexturedPlanet({ data }) {
  const ref = useRef();
  const cloudRef = useRef();
  const maps = useTexture(data.textures);

  useEffect(() => {
    // Albedo + night-lights are color data (sRGB); normal/roughness/bump are linear.
    if (maps.map) maps.map.colorSpace = THREE.SRGBColorSpace;
    if (maps.emissiveMap) maps.emissiveMap.colorSpace = THREE.SRGBColorSpace;
    if (maps.cloudsMap) maps.cloudsMap.colorSpace = THREE.SRGBColorSpace;
    Object.values(maps).forEach((t) => {
      if (t && t.isTexture) t.anisotropy = 16;
    });
  }, [maps]);

  useFrame((_, dt) => {
    if (ref.current) ref.current.rotation.y += dt * 0.02;
    if (cloudRef.current) cloudRef.current.rotation.y += dt * 0.028; // clouds drift faster
  });

  return (
    <group position={data.position}>
      <mesh ref={ref} castShadow receiveShadow>
        <sphereGeometry args={[data.radius, 160, 160]} />
        <meshStandardMaterial
          map={maps.map || null}
          normalMap={maps.normalMap || null}
          bumpMap={maps.bumpMap || null}
          bumpScale={data.bumpScale ?? 0.04}
          roughnessMap={maps.roughnessMap || null}
          metalnessMap={maps.metalnessMap || null}
          emissiveMap={maps.emissiveMap || null}
          emissive={maps.emissiveMap ? new THREE.Color("#fff2cc") : new THREE.Color("#000000")}
          emissiveIntensity={maps.emissiveMap ? 1.0 : 0.0}
          color={data.color ? new THREE.Color(data.color) : new THREE.Color("#ffffff")}
          roughness={data.roughness ?? 1.0}
          metalness={data.metalness ?? 0.0}
        />
      </mesh>

      {maps.cloudsMap && (
        <mesh ref={cloudRef} scale={1.01}>
          <sphereGeometry args={[data.radius, 96, 96]} />
          <meshStandardMaterial map={maps.cloudsMap} transparent opacity={0.85} depthWrite={false} />
        </mesh>
      )}

      {data.atmosphere && <Atmosphere radius={data.radius} atmosphere={data.atmosphere} sunDir={sunDirFor(data.position)} />}
      {data.rings && <Rings radius={data.radius} rings={data.rings} />}
    </group>
  );
}

function StarBody({ data }) {
  // Neutron star / pulsar — full plasma-and-beams treatment, plus its own light.
  return (
    <group position={data.position}>
      <NeutronStar
        radius={data.radius}
        spin={data.spin ?? 0.9}
        cold={data.cold ?? "#2b5bff"}
        hot={data.hot ?? "#eaf1ff"}
        glow={data.glow ?? data.emissive ?? "#9fc4ff"}
        beamColor={data.beamColor ?? "#cfe0ff"}
        beamLength={data.beamLength ?? 10}
        beams={data.beams ?? true}
      />
      <pointLight intensity={data.lightIntensity ?? 1.4} distance={0} decay={0} color={data.emissive ?? "#cfe0ff"} />
    </group>
  );
}

/* ── Atmosphere: back-side fresnel scattering, brighter on the lit limb ────── */
const atmoVert = /* glsl */ `
  varying vec3 vNormalW;
  varying vec3 vPosW;
  void main() {
    vNormalW = normalize(mat3(modelMatrix) * normal);
    vec4 wp = modelMatrix * vec4(position, 1.0);
    vPosW = wp.xyz;
    gl_Position = projectionMatrix * viewMatrix * wp;
  }
`;
const atmoFrag = /* glsl */ `
  uniform vec3 uDay;
  uniform vec3 uNight;
  uniform vec3 uSunDir;
  uniform float uFalloff;
  uniform float uStrength;
  varying vec3 vNormalW;
  varying vec3 vPosW;
  void main() {
    vec3 V = normalize(cameraPosition - vPosW);
    float fres = pow(1.0 - max(dot(vNormalW, V), 0.0), uFalloff);
    float sun = clamp(dot(vNormalW, uSunDir) * 0.5 + 0.5, 0.0, 1.0);
    vec3 col = mix(uNight, uDay, sun);        // sunset colour near the terminator
    float a = fres * uStrength;
    gl_FragColor = vec4(col * a, a);
  }
`;

function Atmosphere({ radius, atmosphere, sunDir }) {
  const uniforms = useMemo(
    () => ({
      uDay: { value: new THREE.Color(atmosphere.day) },
      uNight: { value: new THREE.Color(atmosphere.night) },
      uSunDir: { value: sunDir },
      uFalloff: { value: atmosphere.falloff ?? 3.0 },
      uStrength: { value: atmosphere.strength ?? 1.0 },
    }),
    [atmosphere, sunDir]
  );
  return (
    <mesh scale={1.03}>
      <sphereGeometry args={[radius, 64, 64]} />
      <shaderMaterial
        vertexShader={atmoVert}
        fragmentShader={atmoFrag}
        uniforms={uniforms}
        transparent
        blending={THREE.AdditiveBlending}
        side={THREE.BackSide}
        depthWrite={false}
      />
    </mesh>
  );
}

/* ── Rings: a lit, banded, gappy disc + an orbiting field of asteroid rocks ── */

// A ring annulus whose UVs are remapped so a horizontal strip texture (x = radius)
// wraps correctly from the inner edge out to the outer edge.
function ringGeometry(inner, outer, segments = 256) {
  const geo = new THREE.RingGeometry(inner, outer, segments, 1);
  const pos = geo.attributes.position;
  const uv = geo.attributes.uv;
  const v = new THREE.Vector2();
  for (let i = 0; i < pos.count; i++) {
    v.set(pos.getX(i), pos.getY(i));
    const u = (v.length() - inner) / (outer - inner);
    uv.setXY(i, u, 0.5);
  }
  return geo;
}

function RingBand({ inner, outer, texture }) {
  const map = useTexture(texture);
  const geo = useMemo(() => ringGeometry(inner, outer), [inner, outer]);
  useEffect(() => {
    map.colorSpace = THREE.SRGBColorSpace;
    map.wrapS = map.wrapT = THREE.ClampToEdgeWrapping;
    map.anisotropy = 16;
  }, [map]);
  return (
    <mesh geometry={geo}>
      <meshStandardMaterial
        map={map}
        side={THREE.DoubleSide}
        transparent
        depthWrite={false}
        roughness={1.0}
        metalness={0.0}
      />
    </mesh>
  );
}

// Thousands of instanced rocks scattered through the ring plane — the actual
// "asteroid" particulate that gives the rings depth and a real granular edge.
function AsteroidField({ inner, outer, count = 5000, thickness = 0.05, color = "#c9b79a" }) {
  const mesh = useMemo(() => {
    const geo = new THREE.IcosahedronGeometry(1, 0); // faceted low-poly rock
    const mat = new THREE.MeshStandardMaterial({ roughness: 1.0, metalness: 0.0 });
    const im = new THREE.InstancedMesh(geo, mat, count);
    const m = new THREE.Matrix4();
    const p = new THREE.Vector3();
    const q = new THREE.Quaternion();
    const e = new THREE.Euler();
    const s = new THREE.Vector3();
    const base = new THREE.Color(color);
    const c = new THREE.Color();
    const gapLo = inner + (outer - inner) * 0.60; // a Cassini-like gap
    const gapHi = inner + (outer - inner) * 0.68;
    let i = 0, guard = 0;
    while (i < count && guard < count * 4) {
      guard++;
      // Area-weighted radius so density is even across the disc.
      const r = Math.sqrt(THREE.MathUtils.lerp(inner * inner, outer * outer, Math.random()));
      if (r > gapLo && r < gapHi && Math.random() < 0.85) continue; // carve the gap
      const a = Math.random() * Math.PI * 2;
      const z = (Math.random() - 0.5) * thickness * 2.0 * (outer - inner) * 0.15;
      p.set(Math.cos(a) * r, Math.sin(a) * r, z);
      const scale = THREE.MathUtils.lerp(0.008, 0.05, Math.pow(Math.random(), 2.0)) * (outer - inner);
      s.set(scale * (0.6 + Math.random()), scale * (0.6 + Math.random()), scale * (0.6 + Math.random()));
      e.set(Math.random() * 6.28, Math.random() * 6.28, Math.random() * 6.28);
      q.setFromEuler(e);
      im.setMatrixAt(i, m.compose(p, q, s));
      c.copy(base).offsetHSL((Math.random() - 0.5) * 0.05, (Math.random() - 0.5) * 0.15, (Math.random() - 0.5) * 0.35);
      im.setColorAt(i, c);
      i++;
    }
    im.count = i;
    im.instanceMatrix.needsUpdate = true;
    if (im.instanceColor) im.instanceColor.needsUpdate = true;
    return im;
  }, [inner, outer, count, thickness, color]);

  useEffect(() => () => { mesh.geometry.dispose(); mesh.material.dispose(); }, [mesh]);
  return <primitive object={mesh} />;
}

function Rings({ radius, rings }) {
  const spin = useRef();
  const inner = radius * rings.inner;
  const outer = radius * rings.outer;
  const tiltX = rings.tiltX ?? 0.42;
  const tiltZ = rings.tiltZ ?? 0.18;
  const plainGeo = useMemo(() => ringGeometry(inner, outer), [inner, outer]);
  useFrame((_, dt) => { if (spin.current) spin.current.rotation.z += dt * (rings.spin ?? 0.03); });
  return (
    <group rotation={[-Math.PI / 2 + tiltX, 0, tiltZ]}>
      <group ref={spin}>
        {rings.texture ? (
          <Suspense fallback={null}>
            <RingBand inner={inner} outer={outer} texture={rings.texture} />
          </Suspense>
        ) : (
          <mesh geometry={plainGeo}>
            <meshStandardMaterial color={rings.color || "#c9b79a"} side={THREE.DoubleSide} transparent opacity={0.55} roughness={1} />
          </mesh>
        )}
        {rings.asteroids && (
          <AsteroidField inner={inner} outer={outer} count={rings.asteroids} color={rings.color || "#c9b79a"} />
        )}
      </group>
    </group>
  );
}
