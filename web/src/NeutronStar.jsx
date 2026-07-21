import { useRef, useMemo, useEffect } from "react";
import { useFrame } from "@react-three/fiber";
import * as THREE from "three";

/*
 * A magnetar / pulsar — the Citadel body. (The Core is a main-sequence blue
 * star, `BlueStar.jsx`, which imports `Corona` from here.) Layers:
 *   1. core      — animated blue plasma, differential rotation, hot magnetic
 *                  pole caps, filaments. Emissive-bright.
 *   2. jets      — collimated beams along a tilted magnetic axis plus a GPU
 *                  particle wind, spinning → the classic lighthouse sweep.
 *   3. field     — a dipole (r = L·sin²θ) cage with a pulse running pole to pole.
 *   4. corona    — a raymarched VOLUME (see its own note below), not a shell.
 *   5. disk      — a raymarched accretion disk with real disk physics.
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
    // Keep the mix blue-dominant: this used to bias so far toward uHot that
    // the surface saturated to white, and only the (buggy) solid-blue corona
    // disc in front of it was making the star look blue at all.
    float t = 0.10 + 0.44*plasma + 0.14*gran;
    // Hot magnetic caps + bright filaments in the plasma.
    float pole = smoothstep(0.55, 1.0, abs(lat));
    float fil  = pow(smoothstep(0.62, 0.95, plasma), 2.0);
    vec3 col = mix(uCold, uHot, clamp(t,0.0,1.0));
    col *= 1.0 + 1.3*pole + 0.85*fil;
    // Limb brightening (emissive plasma glows hotter at the edge).
    vec3 V = normalize(cameraPosition - vP);
    float rim = pow(1.0 - max(dot(vN, V), 0.0), 2.0);
    col += mix(uCold, uHot, 0.45) * rim * 0.8;
    // Pulse flicker.
    col *= 0.92 + 0.08*sin(uTime*4.0 + plasma*7.0);
    gl_FragColor = vec4(col, 1.0);
  }
`;

/* ── Corona: a raymarched VOLUME, not a shell ──────────────────────────────
   Every surface-based approach reads as a bubble no matter how it's textured:
   a shell has one fixed silhouette, no internal depth, and no parallax when
   you orbit it. Real coronae are gas -- brightness is emission accumulated
   along the view ray through a 3D density field. So the bounding sphere here
   is just an entry/exit hull: the fragment shader intersects the view ray
   with it and marches through turbulent noise, integrating emission. That
   gives genuine depth (thick where the line of sight crosses more gas),
   a ragged self-occluding silhouette, and correct parallax. */
const coronaVert = /* glsl */ `
  varying vec3 vWorld;
  void main(){
    vec4 wp = modelMatrix * vec4(position, 1.0);
    vWorld = wp.xyz;
    gl_Position = projectionMatrix * viewMatrix * wp;
  }
`;
const coronaFrag = /* glsl */ `
  uniform vec3  uCenter;
  uniform float uCoreR;    // opaque star radius -- gas behind it must not show through
  uniform float uOuterR;   // volume hull radius
  uniform vec3  uHot;      // inner (near-star) emission colour
  uniform vec3  uCool;     // outer (cooled) emission colour
  uniform float uDensity;
  uniform float uFlatten;  // >1 squashes toward the equatorial plane
  uniform float uWisp;      // 1 = fully structured, 0 = smooth featureless halo
  uniform float uStreamer;  // 0 = isotropic blobs, 1 = radial streamers
  uniform float uFalloff;   // radial thinning exponent (higher = steeper)
  uniform float uPolarThin; // 1 = coronal holes over the poles
  uniform float uTime;
  uniform float uSeed;
  uniform float uPulse;
  varying vec3 vWorld;
  float hash(vec3 p){ p=fract(p*0.3183099+0.1); p*=17.0; return fract(p.x*p.y*p.z*(p.x+p.y+p.z)); }
  float vn(vec3 p){
    vec3 i=floor(p),f=fract(p); f=f*f*(3.0-2.0*f);
    float n=mix(mix(mix(hash(i+vec3(0,0,0)),hash(i+vec3(1,0,0)),f.x),
                    mix(hash(i+vec3(0,1,0)),hash(i+vec3(1,1,0)),f.x),f.y),
                mix(mix(hash(i+vec3(0,0,1)),hash(i+vec3(1,0,1)),f.x),
                    mix(hash(i+vec3(0,1,1)),hash(i+vec3(1,1,1)),f.x),f.y),f.z);
    return n;
  }
  float fbm(vec3 p){ float s=0.0,a=0.5; for(int i=0;i<4;i++){ s+=a*vn(p); p=p*2.15+4.7; a*=0.5; } return s; }
  void main(){
    vec3 ro = cameraPosition;
    vec3 rd = normalize(vWorld - ro);
    vec3 oc = ro - uCenter;
    float b = dot(oc, rd);

    // Ray vs. the volume hull -> the segment we integrate over.
    float hOut = b*b - (dot(oc,oc) - uOuterR*uOuterR);
    if (hOut <= 0.0) discard;
    hOut = sqrt(hOut);
    float t0 = max(-b - hOut, 0.0);
    float t1 = -b + hOut;

    // Soft star occlusion. A hard clip (t1 = min(t1, tc)) removes ALL glow
    // behind the star along each ray; because the cut is binary at the star's
    // silhouette it carves a hard-edged dark "shadow ball" out of the
    // surrounding glow when viewed off-axis (the displaced-shadow artifact).
    // Instead, keep the full march and fade each sample that lies behind the
    // star's near surface, softly by how squarely the ray meets the star -- so
    // gas directly behind the star is still hidden, but the edge is a gradient
    // rather than a disc. tcNear = near hit; block = 1 through the star centre,
    // easing to 0 at the limb.
    float hCore = b*b - (dot(oc,oc) - uCoreR*uCoreR);
    float tcNear = (hCore > 0.0) ? (-b - sqrt(hCore)) : 1e9;
    float impact = sqrt(max(dot(oc,oc) - b*b, 0.0));
    float block = 1.0 - smoothstep(uCoreR * 0.5, uCoreR * 1.05, impact);
    if (t1 <= t0) discard;

    const int STEPS = 40;
    float dt = (t1 - t0) / float(STEPS);
    // Dither the start offset: trades banding for a little noise.
    float jit = fract(sin(dot(gl_FragCoord.xy, vec2(12.9898, 78.233))) * 43758.5453);
    float t = t0 + dt * jit;
    float coreN = uCoreR / uOuterR;

    vec3  acc = vec3(0.0);
    float alpha = 0.0;

    for (int i = 0; i < STEPS; i++) {
      vec3 rel = (ro + rd * t - uCenter) / uOuterR;
      rel.y *= uFlatten;              // equatorial flattening (wind-nebula torus)
      float r = length(rel);
      if (r < 1.0) {
        vec3 dir = rel / max(r, 1e-4);

        // uStreamer morphs the SAMPLING SPACE. At 0 the field is sampled in
        // plain 3D (isotropic blobs -- clouds). At 1 the sample only creeps
        // slowly outward along the SAME ray as r grows, so features stay
        // correlated radially and pull into streamers: the structure a magnetic
        // field actually imposes on a corona, and what separates one from fog.
        vec3 seed3 = vec3(uSeed * 4.1, uSeed * 2.7, uSeed * 6.3);
        vec3 sp = mix(rel * 2.1, dir * (3.6 + r * 0.5), uStreamer) + seed3;
        float lobes = fbm(sp + vec3(uTime * 0.015, -uTime * 0.010, uTime * 0.012));
        vec3 fp = mix(rel * 6.5, dir * (10.0 + r * 0.8), uStreamer);
        float fine  = fbm(fp + vec3(-uTime * 0.05, uTime * 0.04, uTime * 0.03));
        float shaped = smoothstep(0.40, 0.70, lobes * 0.7 + fine * 0.3);

        // No gas inside the star; then thin outward. uFalloff sets how fast --
        // a real corona drops off far more steeply than a linear fade.
        float inner = smoothstep(coreN * 0.72, coreN * 1.4, r);
        float rr = clamp((r - coreN) / max(1.0 - coreN, 1e-4), 0.0, 1.0);
        // Density feeds back into the falloff: dense columns thin SLOWLY and so
        // reach far out, while the gaps between them die off fast. Without this
        // the streamers only tint a radially symmetric blob instead of standing
        // out past it. Gated by uStreamer so plain coronae keep a uniform fade.
        float fexp = uFalloff * mix(1.0, mix(1.9, 0.5, shaped), uStreamer);
        float outer = pow(1.0 - rr, fexp);
        // Coronal holes: real coronae are thin over the magnetic poles and
        // reach furthest at the equator (helmet streamers).
        float polar = mix(1.0, mix(1.0, 0.42, smoothstep(0.30, 0.95, abs(dir.y))), uPolarThin);
        float radial = inner * outer * polar;

        // uWisp 1 = fully structured; 0 = a smooth featureless halo.
        float d = mix(1.0, shaped, uWisp) * radial;
        // Soft occlusion: fade samples sitting behind the star's near surface.
        d *= (t > tcNear) ? (1.0 - block) : 1.0;
        if (d > 0.001) {
          vec3 col = mix(uHot, uCool, clamp(r * 1.25, 0.0, 1.0));
          float a = clamp(d * uDensity * dt, 0.0, 1.0);
          acc   += (1.0 - alpha) * a * col * uPulse;  // front-to-back emission
          alpha += (1.0 - alpha) * a;
        }
      }
      if (alpha > 0.99) break;
      t += dt;
    }
    if (alpha < 0.003) discard;
    gl_FragColor = vec4(acc, alpha); // premultiplied; blended with One/One below
  }
`;

export function Corona({
  coreRadius,
  outerRadius,
  hot,
  cool,
  density = 5.0,
  flatten = 1.5,
  wisp = 1.0,
  streamer = 0,
  falloff = 1.0,
  polarThin = 0,
  seed = 0,
  pulseRef,
}) {
  const mesh = useRef();
  const uniforms = useMemo(
    () => ({
      uCenter: { value: new THREE.Vector3() },
      uCoreR: { value: coreRadius },
      uOuterR: { value: outerRadius },
      uHot: { value: new THREE.Color(hot) },
      uCool: { value: new THREE.Color(cool) },
      uDensity: { value: density },
      uFlatten: { value: flatten },
      uWisp: { value: wisp },
      uStreamer: { value: streamer },
      uFalloff: { value: falloff },
      uPolarThin: { value: polarThin },
      uSeed: { value: seed },
      uTime: { value: 0 },
      uPulse: { value: 1 },
    }),
    [coreRadius, outerRadius, hot, cool, density, flatten, wisp, streamer, falloff, polarThin, seed]
  );
  useFrame((_, dt) => {
    uniforms.uTime.value += dt;
    if (pulseRef) uniforms.uPulse.value = pulseRef.current;
    if (mesh.current) mesh.current.getWorldPosition(uniforms.uCenter.value);
  });
  return (
    <mesh ref={mesh}>
      <sphereGeometry args={[outerRadius, 32, 32]} />
      <shaderMaterial
        vertexShader={coronaVert}
        fragmentShader={coronaFrag}
        uniforms={uniforms}
        transparent
        // Premultiplied additive: the march already weighted colour by alpha,
        // so src must NOT be scaled by alpha again (what AdditiveBlending does).
        blending={THREE.CustomBlending}
        blendSrc={THREE.OneFactor}
        blendDst={THREE.OneFactor}
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
  uniform vec3 uColor; uniform float uPulse; uniform float uTime;
  varying vec2 vUv;
  void main(){
    // v: 0 at base (near star) → 1 at tip. Bright, tight near the star; fades out.
    float along = pow(1.0 - vUv.y, 1.6);
    // Electromagnetic wave packets riding outward along the jet -- the
    // rotating dipole's Poynting flux, not a static light shaft.
    float wave = 0.5 + 0.5*sin(vUv.y * 26.0 - uTime * 6.0);
    float packet = pow(wave, 3.0);
    // Soft, hollow-cone edges read as a light shaft.
    float a = along * (0.35 + 0.65*uPulse) * (0.6 + 0.4*packet);
    gl_FragColor = vec4(uColor * (0.6 + 1.4*along) * (0.7 + 0.6*packet) * uPulse, a);
  }
`;

function Beam({ length, baseR, color, pulseRef, dir }) {
  const uniforms = useMemo(
    () => ({ uColor: { value: new THREE.Color(color) }, uPulse: { value: 1 }, uTime: { value: 0 } }),
    [color]
  );
  useFrame((_, dt) => {
    if (pulseRef) uniforms.uPulse.value = pulseRef.current;
    uniforms.uTime.value += dt;
  });
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

/* ── Particle wind: individual relativistic particles streaming along the jet ── */
const particleVert = /* glsl */ `
  uniform float uTime;
  uniform float uLength;
  uniform float uSpread;
  uniform float uTwist;
  uniform float uSpeed;
  uniform float uSize;
  attribute vec3 aSeed; // x: phase, y: radial weight (0 = axis, 1 = edge), z: start angle
  varying float vProgress;
  varying float vRadial;
  void main(){
    float progress = fract(aSeed.x + uTime * uSpeed);
    vProgress = progress;
    vRadial = aSeed.y;
    // Helical drift (magnetic corkscrew) that widens with distance -- turbulent
    // divergence of the wind as it leaves the light cylinder.
    float ang = aSeed.z + progress * uTwist;
    float spread = uSpread * progress * (0.25 + 0.75 * aSeed.y);
    vec3 pos = vec3(cos(ang) * spread, progress * uLength, sin(ang) * spread);
    vec4 mv = modelViewMatrix * vec4(pos, 1.0);
    gl_Position = projectionMatrix * mv;
    gl_PointSize = uSize * (300.0 / max(-mv.z, 0.001)) * (0.35 + 0.65 * (1.0 - progress));
  }
`;
const particleFrag = /* glsl */ `
  uniform vec3 uColorHot;
  uniform vec3 uColorCool;
  uniform float uPulse;
  varying float vProgress;
  varying float vRadial;
  void main(){
    vec2 c = gl_PointCoord - 0.5;
    float d = dot(c, c);
    if (d > 0.25) discard;
    float soft = smoothstep(0.25, 0.0, d);
    // Synchrotron cooling: white-hot right off the pole, cooling to the jet's
    // characteristic colour as it streams outward.
    vec3 col = mix(uColorHot, uColorCool, clamp(vProgress * 1.4, 0.0, 1.0));
    float fade = (1.0 - vProgress) * (1.0 - 0.55 * vRadial);
    gl_FragColor = vec4(col, soft * fade * (0.5 + 0.5 * uPulse));
  }
`;

function BeamParticles({ length, spread, color, pulseRef, dir, count = 260, twist = 7.0 }) {
  const geo = useMemo(() => {
    const g = new THREE.BufferGeometry();
    const pos = new Float32Array(count * 3); // required by three; overridden in the vertex shader
    const seed = new Float32Array(count * 3);
    for (let i = 0; i < count; i++) {
      seed[i * 3 + 0] = Math.random();
      seed[i * 3 + 1] = Math.pow(Math.random(), 1.6); // denser toward the axis
      seed[i * 3 + 2] = Math.random() * Math.PI * 2;
    }
    g.setAttribute("position", new THREE.BufferAttribute(pos, 3));
    g.setAttribute("aSeed", new THREE.BufferAttribute(seed, 3));
    return g;
  }, [count]);
  useEffect(() => () => geo.dispose(), [geo]);

  const uniforms = useMemo(
    () => ({
      uTime: { value: 0 },
      uLength: { value: length },
      uSpread: { value: spread },
      uTwist: { value: twist },
      uSpeed: { value: 0.55 },
      uSize: { value: 1.5 },
      uColorHot: { value: new THREE.Color("#ffffff") },
      uColorCool: { value: new THREE.Color(color) },
      uPulse: { value: 1 },
    }),
    [length, spread, twist, color]
  );
  useFrame((_, dt) => {
    uniforms.uTime.value += dt;
    if (pulseRef) uniforms.uPulse.value = pulseRef.current;
  });

  return (
    <points geometry={geo} rotation={[dir > 0 ? 0 : Math.PI, 0, 0]}>
      <shaderMaterial
        vertexShader={particleVert}
        fragmentShader={particleFrag}
        uniforms={uniforms}
        transparent
        blending={THREE.AdditiveBlending}
        depthWrite={false}
      />
    </points>
  );
}

/* ── Magnetic field lines: a dipole "cage" with a travelling EM-wave pulse ─── */
// r(theta) = L * sin^2(theta) is the classic dipole field-line shape, traced
// in the meridional plane at azimuth phi and swept around the axis to form
// the cage of loops threading both poles (a real pulsar magnetosphere).
function fieldLineCurve(L, phi, segments = 48) {
  const pts = [];
  for (let s = 0; s <= segments; s++) {
    const theta = THREE.MathUtils.lerp(0.02, Math.PI - 0.02, s / segments);
    const r = L * Math.sin(theta) * Math.sin(theta);
    const x = r * Math.sin(theta) * Math.cos(phi);
    const z = r * Math.sin(theta) * Math.sin(phi);
    const y = r * Math.cos(theta);
    pts.push(new THREE.Vector3(x, y, z));
  }
  return new THREE.CatmullRomCurve3(pts);
}

const fieldVert = /* glsl */ `
  varying float vT;
  void main(){
    vT = uv.x; // TubeGeometry: u runs along the path, pole (0) -> equator (0.5) -> pole (1)
    gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0);
  }
`;
const fieldFrag = /* glsl */ `
  uniform vec3 uColor; uniform float uTime; uniform float uSpeed; uniform float uStrength;
  varying float vT;
  void main(){
    // A wave packet racing pole-to-pole along the field line -- the rotating
    // magnetic dipole's radiation, made visible.
    float wave = 0.5 + 0.5*sin(vT * 16.0 - uTime * uSpeed);
    float pulse = pow(wave, 3.0);
    float fade = smoothstep(0.0, 0.1, vT) * smoothstep(1.0, 0.9, vT); // dim right at the poles
    float a = (0.10 + 0.6*pulse) * fade * uStrength;
    gl_FragColor = vec4(uColor * (0.7 + 1.8*pulse), a);
  }
`;

function MagneticFieldLines({ radius, color = "#8fd0ff", loops = 14 }) {
  // Field strength falls off with distance from the star -- larger (more
  // distant) loops read fainter, matching a real dipole's 1/r^3 falloff.
  const uniformsList = useMemo(
    () =>
      Array.from({ length: loops }, (_, i) => ({
        uColor: { value: new THREE.Color(color) },
        uTime: { value: 0 },
        uSpeed: { value: 3.4 },
        uStrength: { value: THREE.MathUtils.lerp(1.4, 0.45, (((i * 5) % loops) / loops)) },
      })),
    [loops, color]
  );
  const geometries = useMemo(() => {
    const geos = [];
    for (let i = 0; i < loops; i++) {
      const phi = (i / loops) * Math.PI * 2;
      const L = radius * (1.25 + 1.15 * (((i * 5) % loops) / loops)); // nested loop sizes
      geos.push(new THREE.TubeGeometry(fieldLineCurve(L, phi), 64, radius * 0.014, 6, false));
    }
    return geos;
  }, [loops, radius]);
  useEffect(() => () => geometries.forEach((g) => g.dispose()), [geometries]);
  useFrame((_, dt) => { uniformsList.forEach((u) => { u.uTime.value += dt; }); });

  return (
    <>
      {geometries.map((geo, i) => (
        <mesh key={i} geometry={geo}>
          <shaderMaterial
            vertexShader={fieldVert}
            fragmentShader={fieldFrag}
            uniforms={uniformsList[i]}
            transparent
            blending={THREE.AdditiveBlending}
            depthWrite={false}
            side={THREE.DoubleSide}
          />
        </mesh>
      ))}
    </>
  );
}

/* ── Accretion disk: a raymarched volume with real disk physics ─────────────
   Replaces the old flat striped current sheet (which was a plane with sine
   stripes -- no thickness, uniform rotation, one flat colour). This one is a
   flared slab of gas integrated along the view ray, with the four things that
   actually make an accretion disk look like one:
     * Keplerian shear -- orbital rate goes as r^-3/2, so the inner disk laps
       the outer one and turbulence is sheared into trailing spiral arms.
       (The noise is sampled in a frame rotated back by omega(r)*t, so the
       shear emerges from the flow rather than being drawn as fixed stripes.)
     * Blackbody temperature gradient -- T goes as r^-3/4: X-ray-hot blue-white
       at the inner edge, cooling through white and amber to dull red outside.
     * Relativistic Doppler beaming -- the limb rotating toward the camera is
       boosted by ~(1 + beta*mu)^3, so one side is markedly brighter. This is
       the asymmetry that reads as "real" in every actual disk image.
     * Magnetospheric truncation -- a magnetar's field clears the disk inside
       the Alfven radius, which is why there is a hard inner hole. */
const diskVert = /* glsl */ `
  varying vec3 vWorld;
  void main(){
    vec4 wp = modelMatrix * vec4(position, 1.0);
    vWorld = wp.xyz;
    gl_Position = projectionMatrix * viewMatrix * wp;
  }
`;
const diskFrag = /* glsl */ `
  uniform vec3  uCenter;
  uniform float uCoreR;
  uniform float uInner;
  uniform float uOuter;
  uniform float uH;        // slab half-thickness at the outer edge (flared inward)
  uniform float uSpeed;    // angular rate scale at the inner edge
  uniform float uBeta;     // orbital speed / c at the inner edge (beaming strength)
  uniform float uDensity;
  uniform vec3  uHot;      // inner-edge emission
  uniform vec3  uMid;
  uniform vec3  uCoolC;    // outer-edge emission
  uniform float uTime;
  varying vec3 vWorld;
  float hash(vec3 p){ p=fract(p*0.3183099+0.1); p*=17.0; return fract(p.x*p.y*p.z*(p.x+p.y+p.z)); }
  float vn(vec3 p){
    vec3 i=floor(p),f=fract(p); f=f*f*(3.0-2.0*f);
    float n=mix(mix(mix(hash(i+vec3(0,0,0)),hash(i+vec3(1,0,0)),f.x),
                    mix(hash(i+vec3(0,1,0)),hash(i+vec3(1,1,0)),f.x),f.y),
                mix(mix(hash(i+vec3(0,0,1)),hash(i+vec3(1,0,1)),f.x),
                    mix(hash(i+vec3(0,1,1)),hash(i+vec3(1,1,1)),f.x),f.y),f.z);
    return n;
  }
  float fbm(vec3 p){ float s=0.0,a=0.5; for(int i=0;i<4;i++){ s+=a*vn(p); p=p*2.17+3.9; a*=0.5; } return s; }

  void main(){
    vec3 ro = cameraPosition;
    vec3 rd = normalize(vWorld - ro);
    vec3 oc = ro - uCenter;

    // Ray vs. the flared slab |y| <= uH.
    float ty0, ty1;
    if (abs(rd.y) < 1e-5) {
      if (abs(oc.y) > uH) discard;
      ty0 = -1e9; ty1 = 1e9;
    } else {
      float a0 = (-uH - oc.y) / rd.y;
      float a1 = ( uH - oc.y) / rd.y;
      ty0 = min(a0, a1); ty1 = max(a0, a1);
    }

    // Ray vs. the bounding cylinder r <= uOuter.
    vec2 o2 = oc.xz, d2 = rd.xz;
    float A = dot(d2, d2), B = dot(o2, d2), C = dot(o2, o2) - uOuter * uOuter;
    float tc0, tc1;
    if (A < 1e-7) {
      if (C > 0.0) discard;
      tc0 = -1e9; tc1 = 1e9;
    } else {
      float h = B * B - A * C;
      if (h <= 0.0) discard;
      h = sqrt(h);
      tc0 = (-B - h) / A; tc1 = (-B + h) / A;
    }

    float t0 = max(max(ty0, tc0), 0.0);
    float t1 = min(ty1, tc1);

    // Soft star occlusion (see the corona for the full rationale): fade disk
    // emission behind the star's near surface with a limb-softened edge, rather
    // than hard-clipping the march, which carved a displaced dark ball.
    float b = dot(oc, rd);
    float hCore = b * b - (dot(oc, oc) - uCoreR * uCoreR);
    float tcNear = (hCore > 0.0) ? (-b - sqrt(hCore)) : 1e9;
    float impact = sqrt(max(dot(oc, oc) - b * b, 0.0));
    float block = 1.0 - smoothstep(uCoreR * 0.5, uCoreR * 1.05, impact);
    if (t1 <= t0) discard;

    const int STEPS = 36;
    float dt = (t1 - t0) / float(STEPS);
    float jit = fract(sin(dot(gl_FragCoord.xy, vec2(12.9898, 78.233))) * 43758.5453);
    float t = t0 + dt * jit;

    vec3  acc = vec3(0.0);
    float alpha = 0.0;

    for (int i = 0; i < STEPS; i++) {
      vec3 rel = ro + rd * t - uCenter;
      float rc = length(rel.xz);
      if (rc > uInner && rc < uOuter) {
        float rn = (rc - uInner) / (uOuter - uInner);   // 0 inner .. 1 outer
        // Flared: the disk is thin at the truncation radius, puffier outside.
        float hgt = uH * (0.28 + 0.72 * pow(rn, 0.9));
        float vert = exp(-pow(rel.y / hgt, 2.0) * 2.2); // gaussian scale height
        if (vert > 0.004) {
          // Keplerian shear: sample the turbulence in a frame counter-rotated
          // by omega(r)*t, so inner annuli genuinely lap outer ones.
          float omega = uSpeed * pow(uInner / rc, 1.5);
          float ang = -omega * uTime;
          float ca = cos(ang), sa = sin(ang);
          vec2 rot = mat2(ca, -sa, sa, ca) * rel.xz;
          vec3 q = vec3(rot.x, rel.y * 2.5, rot.y) * (2.6 / uInner);
          float turb = fbm(q) * 0.68 + fbm(q * 3.3 + 11.0) * 0.32;
          // Fade in at the truncation edge, out at the rim.
          float edge = smoothstep(0.0, 0.10, rn) * smoothstep(1.0, 0.62, rn);
          float d = smoothstep(0.34, 0.66, turb) * vert * edge;
          // Soft occlusion: fade samples behind the star's near surface.
          d *= (t > tcNear) ? (1.0 - block) : 1.0;
          if (d > 0.001) {
            // T ~ r^-3/4 -> blue-white in, amber, then dull red out.
            float temp = pow(uInner / rc, 0.75);
            vec3 col = mix(uCoolC, uMid, smoothstep(0.30, 0.62, temp));
            col = mix(col, uHot, smoothstep(0.62, 0.95, temp));
            // Doppler beaming: material orbiting toward the camera is boosted.
            vec3 vhat = normalize(cross(vec3(0.0, 1.0, 0.0), vec3(rel.x, 0.0, rel.z)));
            float beta = uBeta * sqrt(uInner / rc);
            float boost = pow(clamp(1.0 + beta * dot(vhat, -rd), 0.05, 3.0), 3.0);
            float a = clamp(d * uDensity * dt, 0.0, 1.0);
            acc   += (1.0 - alpha) * a * col * boost * (0.55 + 1.3 * temp);
            alpha += (1.0 - alpha) * a;
          }
        }
      }
      if (alpha > 0.99) break;
      t += dt;
    }
    if (alpha < 0.003) discard;
    gl_FragColor = vec4(acc, alpha);
  }
`;

function AccretionDisk({ radius, coreRadius, hot, mid, cool }) {
  // Same footprint the current sheet had: inner 1.7r, outer 5.5r.
  const inner = radius * 1.7;
  const outer = radius * 5.5;
  const mesh = useRef();
  const uniforms = useMemo(
    () => ({
      uCenter: { value: new THREE.Vector3() },
      uCoreR: { value: coreRadius },
      uInner: { value: inner },
      uOuter: { value: outer },
      uH: { value: radius * 0.42 },
      uSpeed: { value: 0.85 },
      uBeta: { value: 0.42 },
      uDensity: { value: 7.0 },
      uHot: { value: new THREE.Color(hot) },
      uMid: { value: new THREE.Color(mid) },
      uCoolC: { value: new THREE.Color(cool) },
      uTime: { value: 0 },
    }),
    [inner, outer, radius, coreRadius, hot, mid, cool]
  );
  useFrame((_, dt) => {
    uniforms.uTime.value += dt;
    if (mesh.current) mesh.current.getWorldPosition(uniforms.uCenter.value);
  });
  return (
    <mesh ref={mesh}>
      {/* Hull only has to cover the disk on screen; the march segment is
          computed analytically from the slab/cylinder intersection above.
          It MUST be capped (openEnded = false): with an open cylinder a ray
          arriving from above hits no geometry at all, so nothing marches and
          only the lateral wall -- a bare ring -- ever renders. BackSide gives
          exactly one fragment per ray, so the volume is integrated once
          rather than double-added by front and back faces. */}
      <cylinderGeometry args={[outer, outer, radius * 0.9, 64, 1, false]} />
      <shaderMaterial
        vertexShader={diskVert}
        fragmentShader={diskFrag}
        uniforms={uniforms}
        transparent
        blending={THREE.CustomBlending}
        blendSrc={THREE.OneFactor}
        blendDst={THREE.OneFactor}
        side={THREE.BackSide}
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
        <>
          <group ref={beamSpin}>
            <group rotation={[0, 0, beamTilt]}>
              {/* Slim collimated core -- real jets stay narrow, they don't funnel out. */}
              <Beam length={radius * beamLength} baseR={radius * 0.055} color={beamColor} pulseRef={pulse} dir={1} />
              <Beam length={radius * beamLength} baseR={radius * 0.055} color={beamColor} pulseRef={pulse} dir={-1} />
              {/* Individual particles streaming outward around that core. */}
              <BeamParticles length={radius * beamLength} spread={radius * 0.14} color={beamColor} pulseRef={pulse} dir={1} />
              <BeamParticles length={radius * beamLength} spread={radius * 0.14} color={beamColor} pulseRef={pulse} dir={-1} />
              <MagneticFieldLines radius={radius} color={glow} />
            </group>
          </group>
          <AccretionDisk radius={radius} coreRadius={radius} hot="#eaf2ff" mid="#ffc98a" cool="#8a2f1c" />
        </>
      )}

      {/* Volumetric wind nebula, equatorially flattened, breathing with the pulse. */}
      <Corona
        coreRadius={radius}
        outerRadius={radius * 3.4}
        hot={glow}
        cool={cold}
        density={6.5}
        flatten={1.6}
        seed={1.0}
        pulseRef={pulse}
      />
    </group>
  );
}
