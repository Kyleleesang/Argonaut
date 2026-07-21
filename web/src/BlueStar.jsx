import { useRef, useMemo, useEffect } from "react";
import { useFrame } from "@react-three/fiber";
import { useTexture } from "@react-three/drei";
import * as THREE from "three";
import { Corona } from "./NeutronStar";

/*
 * A hot blue star for the Core (the network's light source) -- a churning
 * photosphere, not a pulsar: no relativistic jets, no hard lighthouse pulse.
 * Reuses NeutronStar's raymarched volumetric `Corona` for the halo.
 *
 * SURFACE DETAIL COMES FROM REAL DATA. There is no photographic texture of a
 * blue star's surface anywhere, and there cannot be: no telescope resolves any
 * star's disc except the Sun (plus a couple of red supergiants as a few
 * pixels), so every "blue star texture" online is an artist's painting. The
 * Sun, however, IS a resolved star -- so we sample a real solar map
 * (`/textures/star_surface.jpg`) for genuine granulation, plage and sunspot
 * structure, and remap its LUMINANCE through a blue-hot palette. Real
 * structure, correct colour temperature.
 *
 * Sampled via the sphere's own `uv` attribute, NOT an atan2 unwrap -- the
 * latter wraps discontinuously at +-180deg and puts a hard seam down the
 * body (the exact bug that once split the nebula sky in two).
 */

const photoVert = /* glsl */ `
  varying vec3 vObj;
  varying vec3 vN;
  varying vec3 vP;
  varying vec2 vUv;
  void main(){
    vObj = position;
    vUv = uv;
    vN = normalize(mat3(modelMatrix) * normal);
    vec4 wp = modelMatrix * vec4(position, 1.0);
    vP = wp.xyz;
    gl_Position = projectionMatrix * viewMatrix * wp;
  }
`;
const photoFrag = /* glsl */ `
  uniform float uTime;
  uniform vec3  uCool;   // deep blue base
  uniform vec3  uHot;    // pale cyan bright patches
  uniform vec3  uSpot;   // near-white hotspots
  uniform sampler2D uMap;
  varying vec3 vObj;
  varying vec3 vN;
  varying vec3 vP;
  varying vec2 vUv;
  float hash(vec3 p){ p=fract(p*0.3183099+0.1); p*=17.0; return fract(p.x*p.y*p.z*(p.x+p.y+p.z)); }
  float vn(vec3 p){
    vec3 i=floor(p),f=fract(p); f=f*f*(3.0-2.0*f);
    float n=mix(mix(mix(hash(i+vec3(0,0,0)),hash(i+vec3(1,0,0)),f.x),
                    mix(hash(i+vec3(0,1,0)),hash(i+vec3(1,1,0)),f.x),f.y),
                mix(mix(hash(i+vec3(0,0,1)),hash(i+vec3(1,0,1)),f.x),
                    mix(hash(i+vec3(0,1,1)),hash(i+vec3(1,1,1)),f.x),f.y),f.z);
    return n;
  }
  float fbm(vec3 p){ float s=0.0,a=0.5; for(int i=0;i<5;i++){ s+=a*vn(p); p=p*2.05+3.3; a*=0.5; } return s; }

  void main(){
    vec3 n = normalize(vObj);
    // Differential rotation: the equator laps the poles, shearing the pattern.
    float lat = n.y;
    float shear = uTime * 0.04 * (1.0 - 0.35 * abs(lat));
    float cs = cos(shear), sn = sin(shear);
    vec3 ns = vec3(n.x * cs - n.z * sn, n.y, n.x * sn + n.z * cs);

    // REAL solar surface structure. Luminance only -- the source map is a
    // yellow G-type star, and we want its STRUCTURE, not its colour.
    float sunLum = dot(texture2D(uMap, vUv).rgb, vec3(0.2126, 0.7152, 0.0722));
    float detail = smoothstep(0.22, 0.95, sunLum);

    // Slow procedural mottling layered on top, purely so the surface breathes
    // (the texture itself is a static snapshot and would otherwise be frozen).
    vec3 warp = vec3(fbm(ns * 1.6 + 11.0), fbm(ns * 1.6 + 27.0), fbm(ns * 1.6 + 43.0));
    float big = fbm(ns * 2.2 + warp * 1.7 + vec3(0.0, uTime * 0.02, 0.0));
    float mid = fbm(ns * 5.2 + warp * 1.1 - vec3(uTime * 0.03, 0.0, 0.0));
    float mottle = big * 0.6 + mid * 0.4;

    // Real detail carries the look; the noise only nudges it.
    float t = 0.30 + 0.86 * detail + 0.16 * (mottle - 0.5);
    vec3 col = mix(uCool, uHot, clamp(t, 0.0, 1.0));

    // The map's own brightest regions (plage / faculae) burn out to near-white.
    float hs = smoothstep(0.86, 1.0, sunLum);
    col = mix(col, uSpot, hs * 0.65);

    vec3 V = normalize(cameraPosition - vP);
    float mu = clamp(dot(vN, V), 0.0, 1.0);
    // Eddington limb darkening I(mu)/I(1) = (2+3mu)/5, applied at partial
    // strength: the reference has a bright, diffuse edge, so full darkening
    // would fight the halo instead of blending into it.
    col *= mix(1.0, (2.0 + 3.0 * mu) / 5.0, 0.35);
    float limb = 1.0 - mu;
    col += uHot * pow(limb, 2.2) * 0.7;   // glowing rim into the corona
    // Gentle shimmer -- convective turnover, not a pulsar pulse.
    col *= 0.985 + 0.015 * sin(uTime * 1.3 + mottle * 5.0);
    // Overall emissive gain. A star is a LIGHT SOURCE: without pushing well
    // past 1.0 it tonemaps down to a dull lit sphere and reads as a planet.
    // ACES rolls the highlights off, so the bright cores clip gracefully.
    col *= 1.9;
    gl_FragColor = vec4(col, 1.0);
  }
`;

export function BlueStar({
  radius = 2.2,
  spin = 0.12,
  cool = "#1550c8",  // deep saturated blue
  hot = "#b8e4ff",   // pale cyan highlights
  spot = "#eaf6ff",  // near-white hotspots (cool white: a warm tint reads as
                     // yellow blotches against the blue, not as hot plasma)
  glow = "#6fa8ff",
}) {
  const core = useRef();
  const pulse = useRef(1); // constant -- Corona expects a ref, but this star doesn't pulse
  const map = useTexture("/textures/star_surface.jpg");
  const uniforms = useMemo(
    () => ({
      uTime: { value: 0 },
      uCool: { value: new THREE.Color(cool) },
      uHot: { value: new THREE.Color(hot) },
      uSpot: { value: new THREE.Color(spot) },
      uMap: { value: map },
    }),
    [cool, hot, spot, map]
  );
  useEffect(() => {
    map.colorSpace = THREE.SRGBColorSpace;
    map.anisotropy = 16;
  }, [map]);
  useFrame((_, dt) => {
    uniforms.uTime.value += dt;
    if (core.current) core.current.rotation.y += dt * spin;
  });

  return (
    <group>
      <mesh ref={core}>
        <sphereGeometry args={[radius, 128, 128]} />
        <shaderMaterial vertexShader={photoVert} fragmentShader={photoFrag} uniforms={uniforms} />
      </mesh>
      {/* A star's halo is a smooth glow hugging the disc (low wisp), not the
          filamentary lobed nebula the pulsar gets.

          KEEP DENSITY LOW, and remember it trades against `wisp`: the corona
          is integrated in FRONT of the star too, so if the accumulated alpha
          there approaches 1 the halo stops being a glow and becomes an opaque
          veil over the photosphere. `wisp` gates how many samples survive the
          noise mask (~30% at wisp 1, ~83% at wisp 0.25), so lowering wisp
          without lowering density in step multiplies the opacity several-fold
          -- which is exactly how this turned into a featureless gas cloud. */}
      <Corona
        coreRadius={radius}
        outerRadius={radius * 2.6}
        hot={hot}
        cool={glow}
        // Structure has to be visible for streamers to read, so wisp is high --
        // and since wisp gates how many samples survive the noise mask, density
        // rises with it to keep the same overall brightness.
        density={3.0}
        flatten={1.02}
        wisp={0.8}
        streamer={1.0}   // radial streamers, not isotropic cloud
        falloff={2.8}    // steep thinning, as a real corona does
        polarThin={1.0}  // coronal holes over the poles
        seed={4.2}
        pulseRef={pulse}
      />
    </group>
  );
}
