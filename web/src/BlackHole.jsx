import { useMemo, useRef } from "react";
import { useFrame, useThree } from "@react-three/fiber";
import * as THREE from "three";

/*
 * Black-hole lensing as a full-screen BACKGROUND pass (a "skybox that bends
 * light"). A fullscreen triangle draws first (renderOrder -1000, depth off);
 * every pixel reconstructs its world-space ray from the live camera, then
 * raymarches a photon geodesic past the event horizon and accretion disk.
 * Planets (real meshes) draw on top. Output is LINEAR HDR — the EffectComposer
 * tone-maps + blooms it alongside the rest of the scene.
 *
 * Known v1 limitation (same as the standalone artifact): a planet positioned
 * *behind* the hole is not lensed/occluded — the background can't read planet
 * depth. Rare given the orbit radii; a depth-aware pass is a later upgrade.
 */

const VERT = /* glsl */ `#version 300 es
in vec3 position;
out vec2 vNdc;
void main(){ vNdc = position.xy; gl_Position = vec4(position.xy, 1.0, 1.0); }`;

const FRAG = /* glsl */ `#version 300 es
precision highp float;
uniform mat4 uInvProjView;
uniform vec3 uCamPos;
uniform float uTime, uDiskIntensity, uSpin, uLensing, uInnerR, uOuterR, uSteps;
in vec2 vNdc;
out vec4 fragColor;

float hash21(vec2 p){ p=fract(p*vec2(123.34,456.21)); p+=dot(p,p+45.32); return fract(p.x*p.y); }
float vnoise(vec2 p){
  vec2 i=floor(p),f=fract(p); f=f*f*(3.0-2.0*f);
  float a=hash21(i),b=hash21(i+vec2(1.0,0.0)),c=hash21(i+vec2(0.0,1.0)),d=hash21(i+vec2(1.0,1.0));
  return mix(mix(a,b,f.x),mix(c,d,f.x),f.y);
}
float fbm(vec2 p){ float s=0.0,a=0.5; for(int i=0;i<5;i++){ s+=a*vnoise(p); p=p*2.02+11.3; a*=0.5; } return s; }
vec3 blackbody(float t){
  t=clamp(t,0.0,1.0);
  vec3 c=mix(vec3(0.55,0.02,0.0),vec3(1.0,0.32,0.05),smoothstep(0.0,0.3,t));
  c=mix(c,vec3(1.0,0.76,0.32),smoothstep(0.3,0.55,t));
  c=mix(c,vec3(1.0,0.98,0.92),smoothstep(0.55,0.78,t));
  c=mix(c,vec3(0.72,0.83,1.0),smoothstep(0.78,1.0,t));
  return c;
}
vec3 nebula(vec3 d){
  vec2 uv=vec2(atan(d.z,d.x)*0.159154,asin(clamp(d.y,-1.0,1.0))*0.318309);
  float n1=fbm(uv*2.6+vec2(8.0,3.0));
  float n2=fbm(uv*5.5+vec2(n1*2.2,n1*1.3));
  float n3=fbm(uv*1.4-vec2(4.0,7.0));
  vec3 col=mix(vec3(0.13,0.05,0.32),vec3(0.02,0.25,0.36),smoothstep(0.2,0.85,n1));
  col=mix(col,vec3(0.40,0.07,0.27),smoothstep(0.45,0.95,n2)*0.55);
  col*=0.12+1.35*n2*(0.4+0.6*n3);
  col*=mix(0.28,1.0,smoothstep(0.15,0.6,n3));
  col+=vec3(0.35,0.55,0.7)*pow(smoothstep(0.75,1.0,n2),2.0)*0.5;
  for(float l=0.0;l<3.0;l++){
    float sc=180.0+l*260.0; vec2 g=uv*sc; vec2 id=floor(g);
    float h=hash21(id+l*37.0); float thr=0.986;
    if(h>thr){ vec2 fp=fract(g)-0.5; float d2=dot(fp,fp);
      float b=smoothstep(0.03,0.0,d2)*((h-thr)/(1.0-thr));
      float tw=0.65+0.35*sin(uTime*1.5+h*80.0);
      vec3 tint=mix(vec3(0.72,0.82,1.0),vec3(1.0,0.88,0.72),hash21(id+7.0));
      col+=b*tw*tint*1.5;
    }
  }
  return col;
}
vec4 sampleDisk(vec3 hit,float rr){
  float ang=atan(hit.z,hit.x);
  float krot=uSpin*uTime/pow(rr,1.5);
  float turb=fbm(vec2(ang*3.0+krot,rr*1.4-uTime*0.15*uSpin));
  float turb2=fbm(vec2((ang*3.0+krot)*2.3-krot*0.5,rr*3.0+uTime*0.25));
  float density=smoothstep(0.15,1.15,turb*1.1+0.4*turb2);
  float temp=pow(clamp(uInnerR/rr,0.0,1.0),0.85);
  vec3 base=blackbody(temp*0.9+0.08);
  float bright=temp*(0.3+1.15*density);
  vec3 tang=normalize(vec3(-hit.z,0.0,hit.x));
  vec3 toCam=normalize(uCamPos-hit);
  float beta=clamp(1.7/sqrt(rr),0.0,0.72);
  float dopp=1.0/max(1.0-beta*dot(tang,toCam),0.15);
  bright*=pow(dopp,3.0);
  base=mix(base,base.zyx,clamp((dopp-1.0)*0.5,0.0,0.5));
  bright*=smoothstep(uOuterR,uOuterR*0.55,rr);
  bright*=smoothstep(uInnerR*0.97,uInnerR*1.18,rr);
  bright*=uDiskIntensity;
  return vec4(base*bright,clamp(density*0.92,0.0,1.0));
}
void main(){
  vec4 world=uInvProjView*vec4(vNdc,1.0,1.0);
  vec3 rd=normalize(world.xyz/world.w-uCamPos);
  vec3 pos=uCamPos, vel=rd, col=vec3(0.0); float trans=1.0; bool captured=false;
  float Rs=1.0;
  for(int i=0;i<300;i++){
    if(float(i)>=uSteps) break;
    float r=length(pos);
    if(r<Rs){ captured=true; break; }
    float dtStep=min(0.02+0.03*r,0.6);
    vec3 h=cross(pos,vel); float h2=dot(h,h);
    vec3 accel=-1.5*uLensing*h2*pos/pow(dot(pos,pos),2.5);
    vec3 prev=pos; vel+=accel*dtStep; pos+=vel*dtStep;
    if(prev.y*pos.y<0.0){
      float tc=prev.y/(prev.y-pos.y); vec3 hit=mix(prev,pos,tc);
      float rr=length(hit.xz);
      if(rr>uInnerR*0.97 && rr<uOuterR){ vec4 dc=sampleDisk(hit,rr); col+=trans*dc.rgb; trans*=(1.0-dc.a*0.82); }
    }
    if(r>120.0) break;
    if(trans<0.02) break;
  }
  if(!captured) col+=trans*nebula(normalize(vel));
  fragColor=vec4(col,1.0);
}`;

export function BlackHole({ intensity = 1.0, spin = 1.0, lensing = 1.0, inner = 2.6, outer = 9.0, steps = 170 }) {
  const { camera } = useThree();
  const uniforms = useMemo(
    () => ({
      uInvProjView: { value: new THREE.Matrix4() },
      uCamPos: { value: new THREE.Vector3() },
      uTime: { value: 0 },
      uDiskIntensity: { value: intensity },
      uSpin: { value: spin },
      uLensing: { value: lensing },
      uInnerR: { value: inner },
      uOuterR: { value: outer },
      uSteps: { value: steps },
    }),
    [] // eslint-disable-line react-hooks/exhaustive-deps
  );

  const verts = useMemo(() => new Float32Array([-1, -1, 0, 3, -1, 0, -1, 3, 0]), []);

  useFrame((_, dt) => {
    uniforms.uTime.value += dt;
    uniforms.uCamPos.value.copy(camera.position);
    // invViewProj = matrixWorld * projectionMatrixInverse  → world ray from NDC
    uniforms.uInvProjView.value.multiplyMatrices(camera.matrixWorld, camera.projectionMatrixInverse);
  });

  return (
    <mesh frustumCulled={false} renderOrder={-1000}>
      <bufferGeometry>
        <bufferAttribute attach="attributes-position" args={[verts, 3]} />
      </bufferGeometry>
      <rawShaderMaterial
        vertexShader={VERT}
        fragmentShader={FRAG}
        uniforms={uniforms}
        glslVersion={THREE.GLSL3}
        depthTest={false}
        depthWrite={false}
      />
    </mesh>
  );
}
