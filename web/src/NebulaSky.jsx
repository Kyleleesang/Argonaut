import { useMemo } from "react";
import { useFrame } from "@react-three/fiber";
import * as THREE from "three";

/*
 * Stable deep-space backdrop: a large inverted sphere with a painterly nebula +
 * starfield shader (the "Sins-of-a-Solar-Empire" palette). Standard mesh, depth
 * write off, drawn behind everything — no fullscreen/renderOrder tricks, so it
 * composites cleanly and does not strobe.
 */

const vert = /* glsl */ `
  varying vec3 vP;
  void main(){
    vec4 wp = modelMatrix * vec4(position, 1.0);
    vP = wp.xyz;
    gl_Position = projectionMatrix * viewMatrix * wp;
  }
`;
const frag = /* glsl */ `
  uniform float uTime;
  varying vec3 vP;
  // 3D value noise sampled directly on the view-direction vector -- no
  // lon/lat unwrap, so there's no seam at +-180deg and no pole pinch.
  float hash(vec3 p){ p=fract(p*0.3183099+0.1); p*=17.0; return fract(p.x*p.y*p.z*(p.x+p.y+p.z)); }
  float vn(vec3 p){
    vec3 i=floor(p),f=fract(p); f=f*f*(3.0-2.0*f);
    float n=mix(mix(mix(hash(i+vec3(0,0,0)),hash(i+vec3(1,0,0)),f.x),
                    mix(hash(i+vec3(0,1,0)),hash(i+vec3(1,1,0)),f.x),f.y),
                mix(mix(hash(i+vec3(0,0,1)),hash(i+vec3(1,0,1)),f.x),
                    mix(hash(i+vec3(0,1,1)),hash(i+vec3(1,1,1)),f.x),f.y),f.z);
    return n;
  }
  float fbm(vec3 p){ float s=0.0,a=0.5; for(int i=0;i<5;i++){ s+=a*vn(p); p=p*2.02+11.3; a*=0.5; } return s; }
  void main(){
    vec3 d = normalize(vP - cameraPosition);
    float n1 = fbm(d * 2.6 + vec3(8.0, 3.0, 1.0));
    float n2 = fbm(d * 5.5 + vec3(n1 * 2.2, n1 * 1.3, n1 * 0.7));
    float n3 = fbm(d * 1.4 - vec3(4.0, 7.0, 2.0));
    vec3 col = mix(vec3(0.10, 0.05, 0.28), vec3(0.02, 0.20, 0.34), smoothstep(0.2, 0.85, n1));
    col = mix(col, vec3(0.34, 0.06, 0.24), smoothstep(0.45, 0.95, n2) * 0.5);
    col *= 0.10 + 1.25 * n2 * (0.4 + 0.6 * n3);
    col *= mix(0.25, 1.0, smoothstep(0.15, 0.6, n3));          // dust lanes
    col += vec3(0.3, 0.5, 0.7) * pow(smoothstep(0.78, 1.0, n2), 2.0) * 0.45;
    for (float l = 0.0; l < 3.0; l++) {
      float sc = 180.0 + l * 260.0; vec3 g = d * sc; vec3 id = floor(g);
      float h = hash(id + l * 37.0); float thr = 0.987;
      if (h > thr) {
        vec3 fp = fract(g) - 0.5; float d2 = dot(fp, fp);
        float b = smoothstep(0.03, 0.0, d2) * ((h - thr) / (1.0 - thr));
        float tw = 0.65 + 0.35 * sin(uTime * 1.5 + h * 80.0);
        col += b * tw * mix(vec3(0.72, 0.82, 1.0), vec3(1.0, 0.9, 0.75), hash(id + 7.0)) * 1.4;
      }
    }
    gl_FragColor = vec4(col, 1.0);
  }
`;

export function NebulaSky() {
  const uniforms = useMemo(() => ({ uTime: { value: 0 } }), []);
  useFrame((_, dt) => { uniforms.uTime.value += dt; });
  return (
    <mesh renderOrder={-1}>
      <sphereGeometry args={[2000, 48, 48]} />
      <shaderMaterial
        vertexShader={vert}
        fragmentShader={frag}
        uniforms={uniforms}
        side={THREE.BackSide}
        depthWrite={false}
      />
    </mesh>
  );
}
