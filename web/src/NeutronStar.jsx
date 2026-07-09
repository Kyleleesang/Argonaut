import { useRef, useMemo } from "react";
import { useFrame } from "@react-three/fiber";
import * as THREE from "three";

/*
 * A neutron star / pulsar. Reused for both the central Core light source and the
 * smaller "Pulsar" body. Layers, back-to-front:
 *   1. core     — animated blue plasma, differential-rotation banding, hot
 *                 magnetic-pole caps, filaments. Emissive-bright.
 *   2. beams    — two opposing relativistic jets along a tilted magnetic axis,
 *                 spinning about the spin axis → the classic lighthouse sweep.
 *   3. corona   — a tight additive fresnel halo + a wide faint magnetosphere,
 *                 both breathing with the pulse (stands in for screen bloom).
 * Everything is a standard mesh (no fullscreen/depth hacks) so it composites
 * cleanly without an EffectComposer.
 */

/* ── Surface ─────────────────────────────────────────────────────────────── */
const surfVert = /* glsl */ `
  varying vec3 vObj;
  varying vec3 vN;
  varying vec3 vP;
  void main(){
    vObj = position;
    vN = normalize(mat3(modelMatrix) * normal);
    vec4 wp = modelMatrix * vec4(position, 1.0);
    vP = wp.xyz;
    gl_Position = projectionMatrix * viewMatrix * wp;
  }
`;
const surfFrag = /* glsl */ `
  uniform float uTime;
  uniform vec3  uCold;
  uniform vec3  uHot;
  varying vec3 vObj;
  varying vec3 vN;
  varying vec3 vP;
  float hash(vec3 p){ p=fract(p*0.3183099+0.1); p*=17.0; return fract(p.x*p.y*p.z*(p.x+p.y+p.z)); }
  float vn(vec3 p){
    vec3 i=floor(p),f=fract(p); f=f*f*(3.0-2.0*f);
    float n=mix(mix(mix(hash(i+vec3(0,0,0)),hash(i+vec3(1,0,0)),f.x),
                    mix(hash(i+vec3(0,1,0)),hash(i+vec3(1,1,0)),f.x),f.y),
                mix(mix(hash(i+vec3(0,0,1)),hash(i+vec3(1,0,1)),f.x),
                    mix(hash(i+vec3(0,1,1)),hash(i+vec3(1,1,1)),f.x),f.y),f.z);
    return n;
  }
  float fbm(vec3 p){ float s=0.0,a=0.5; for(int i=0;i<6;i++){ s+=a*vn(p); p=p*2.03+7.1; a*=0.5; } return s; }
  void main(){
    vec3 n = normalize(vObj);
    // Differential rotation: higher latitudes lag, so shear the noise by latitude.
    float lat = n.y;
    vec3 q = n * 3.0;
    q.xz *= mat2(cos(uTime*0.10*(1.0-0.5*abs(lat))), -sin(uTime*0.10),
                 sin(uTime*0.10),  cos(uTime*0.10*(1.0-0.5*abs(lat))));
    // Domain-warped plasma + fine granulation.
    vec3 warp = vec3(fbm(q+vec3(0.0,uTime*0.05,0.0)), fbm(q+5.2), fbm(q+9.7));
    float plasma = fbm(q*1.3 + warp*1.8);
    float gran   = fbm(n*14.0 - vec3(0.0,0.0,uTime*0.25));
    float t = 0.55 + 0.42*plasma + 0.16*gran;
    // Hot magnetic caps + bright filaments in the plasma.
    float pole = smoothstep(0.55, 1.0, abs(lat));
    float fil  = pow(smoothstep(0.62, 0.95, plasma), 2.0);
    vec3 col = mix(uCold, uHot, clamp(t,0.0,1.0));
    col *= 1.0 + 2.4*pole + 1.6*fil;
    // Limb brightening (emissive plasma glows hotter at the edge).
    vec3 V = normalize(cameraPosition - vP);
    float rim = pow(1.0 - max(dot(vN, V), 0.0), 2.0);
    col += uHot * rim * 1.1;
    // Pulse flicker.
    col *= 0.92 + 0.08*sin(uTime*4.0 + plasma*7.0);
    gl_FragColor = vec4(col, 1.0);
  }
`;

/* ── Corona shells ───────────────────────────────────────────────────────── */
const coronaVert = /* glsl */ `
  varying vec3 vN; varying vec3 vP;
  void main(){
    vN = normalize(mat3(modelMatrix) * normal);
    vec4 wp = modelMatrix * vec4(position, 1.0);
    vP = wp.xyz;
    gl_Position = projectionMatrix * viewMatrix * wp;
  }
`;
const coronaFrag = /* glsl */ `
  uniform vec3 uColor; uniform float uPow; uniform float uStrength; uniform float uPulse;
  varying vec3 vN; varying vec3 vP;
  void main(){
    vec3 V = normalize(cameraPosition - vP);
    float f = pow(1.0 - max(dot(vN, V), 0.0), uPow);
    gl_FragColor = vec4(uColor * f * uStrength * uPulse, f);
  }
`;

function Glow({ scale, color, pow, strength, pulseRef }) {
  const uniforms = useMemo(
    () => ({
      uColor: { value: new THREE.Color(color) },
      uPow: { value: pow },
      uStrength: { value: strength },
      uPulse: { value: 1 },
    }),
    [color, pow, strength]
  );
  useFrame(() => { if (pulseRef) uniforms.uPulse.value = pulseRef.current; });
  return (
    <mesh scale={scale}>
      <sphereGeometry args={[1, 48, 48]} />
      <shaderMaterial
        vertexShader={coronaVert}
        fragmentShader={coronaFrag}
        uniforms={uniforms}
        transparent
        blending={THREE.AdditiveBlending}
        side={THREE.BackSide}
        depthWrite={false}
      />
    </mesh>
  );
}

/* ── Relativistic jets (pulsar beams) ────────────────────────────────────── */
const beamVert = /* glsl */ `
  varying vec2 vUv;
  void main(){ vUv = uv; gl_Position = projectionMatrix * modelViewMatrix * vec4(position,1.0); }
`;
const beamFrag = /* glsl */ `
  uniform vec3 uColor; uniform float uPulse;
  varying vec2 vUv;
  void main(){
    // v: 0 at base (near star) → 1 at tip. Bright, tight near the star; fades out.
    float along = pow(1.0 - vUv.y, 1.6);
    // Soft, hollow-cone edges read as a light shaft.
    float a = along * (0.35 + 0.65*uPulse);
    gl_FragColor = vec4(uColor * (0.6 + 1.4*along) * uPulse, a);
  }
`;

function Beam({ length, baseR, color, pulseRef, dir }) {
  const uniforms = useMemo(
    () => ({ uColor: { value: new THREE.Color(color) }, uPulse: { value: 1 } }),
    [color]
  );
  useFrame(() => { if (pulseRef) uniforms.uPulse.value = pulseRef.current; });
  // Cone points +Y by default; tip outward, base at the star.
  return (
    <mesh position={[0, dir * length * 0.5, 0]} rotation={[dir > 0 ? 0 : Math.PI, 0, 0]}>
      <coneGeometry args={[baseR, length, 40, 1, true]} />
      <shaderMaterial
        vertexShader={beamVert}
        fragmentShader={beamFrag}
        uniforms={uniforms}
        transparent
        blending={THREE.AdditiveBlending}
        side={THREE.DoubleSide}
        depthWrite={false}
      />
    </mesh>
  );
}

export function NeutronStar({
  radius = 2.2,
  spin = 0.5,
  cold = "#123ffa",
  hot = "#dbe6ff",
  glow = "#5ab0ff",
  beams = true,
  beamColor = "#bcd8ff",
  beamLength = 8, // jet length as a multiple of the star radius
  beamTilt = 0.32, // magnetic-axis offset from the spin axis (radians)
}) {
  const core = useRef();
  const beamSpin = useRef();
  const pulse = useRef(1);
  const surfUniforms = useMemo(
    () => ({ uTime: { value: 0 }, uCold: { value: new THREE.Color(cold) }, uHot: { value: new THREE.Color(hot) } }),
    [cold, hot]
  );
  useFrame((state, dt) => {
    const t = state.clock.elapsedTime;
    surfUniforms.uTime.value += dt;
    // Sharp, periodic pulse — the beam swings past + the whole star breathes.
    pulse.current = 0.72 + 0.28 * Math.pow(0.5 + 0.5 * Math.sin(t * 2.4), 6.0) + 0.08 * Math.sin(t * 5.0);
    if (core.current) core.current.rotation.y += dt * spin;
    if (beamSpin.current) beamSpin.current.rotation.y += dt * spin * 1.6;
  });

  return (
    <group>
      <mesh ref={core}>
        <sphereGeometry args={[radius, 128, 128]} />
        <shaderMaterial vertexShader={surfVert} fragmentShader={surfFrag} uniforms={surfUniforms} />
      </mesh>

      {beams && (
        <group ref={beamSpin}>
          <group rotation={[0, 0, beamTilt]}>
            <Beam length={radius * beamLength} baseR={radius * 0.42} color={beamColor} pulseRef={pulse} dir={1} />
            <Beam length={radius * beamLength} baseR={radius * 0.42} color={beamColor} pulseRef={pulse} dir={-1} />
          </group>
        </group>
      )}

      {/* Additive halos stand in for screen-space bloom; they breathe with the pulse. */}
      <Glow scale={radius * 1.35} color={glow} pow={2.4} strength={2.4} pulseRef={pulse} />
      <Glow scale={radius * 2.6} color={cold} pow={1.4} strength={1.1} pulseRef={pulse} />
    </group>
  );
}
