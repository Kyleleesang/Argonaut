# Argonaut — realistic solar-system frontend

A standalone React Three Fiber app: a navigable star system (fly between the
star, three worlds, and a pulsar) built for **near-photoreal real-time**
rendering. It lives outside the Rust workspace on purpose
(see `../docs/FRONTEND_SOLARSYSTEM_SPEC.md`); Astro can wrap this scene as an
island later for SSR content + SEO.

## Run it

```bash
cd web
npm install
npm run dev        # → http://localhost:5173
```

It boots immediately with clean PBR spheres and the full postprocessing stack.
**It is not yet photoreal until you add assets** — that's the point of Path B.

> Heads-up: this scaffold was written without a browser to test in. If `npm run
> dev` throws, paste the error and it's almost certainly a version-pin or import
> tweak — the pinned stack (three 0.160 · @react-three/fiber 8 · drei 9 ·
> postprocessing 2 · React 18) is a known-compatible combination.

## Go photoreal (in order of impact)

1. **HDRI lighting** — drop `public/hdr/space.hdr` (ASSETS.md §1), set
   `USE_HDRI = true` in `src/App.jsx`. Real reflections + ambient. Biggest jump
   for the least work.
2. **Real planet textures** — add the maps in ASSETS.md §2 to
   `public/textures/`, uncomment the `textures: { … }` blocks in `src/config.js`.
   The Earth "Blue Marble + Black Marble + clouds" set on **Ecosystem** is the
   most convincing single addition.
3. **Compress to KTX2** before shipping (ASSETS.md §3).

## What's already doing the realism work (the code levers)

- **Linear colour + ACES Filmic tonemapping** (`gl.toneMapping`) — the biggest
  "cinematic vs flat-WebGL" difference, on by default.
- **PBR materials** (`meshStandardMaterial`) with albedo/normal/roughness/
  emissive slots + correct sRGB-vs-linear colour spaces per map.
- **Atmospheric scattering** — a back-side fresnel shell brighter on the lit
  limb, with sunset colour near the terminator (`Atmosphere` in `Planet.jsx`).
- **Postprocessing** — bloom (mipmap, thresholded so only genuinely bright
  things glow), vignette, subtle film grain, SMAA anti-aliasing.
- **Cinematic camera** — `CameraControls.setLookAt(..., true)` flies smoothly to
  each body on tab click; drag to orbit, scroll to zoom.

## Roadmap (next, in rough priority)

- **Orbital motion** — animate `config.js` positions on ellipses and have the
  camera rig follow the focused body each frame. Fixed positions now keep the
  fly-to transitions clean.
- **Black-hole lensing background** — the raymarched lensing shader is ported in
  `src/BlackHole.jsx` (fullscreen pass), but it is **not wired in**: as a
  fullscreen depth-off pass it fought the EffectComposer's buffer clear and
  strobed. The center is currently a rotating blue **neutron star**
  (`src/NeutronStar.jsx`), which lights the planets. To make lensing robust it
  needs a depth-aware render into the composer (or its own render target) rather
  than a renderOrder/depthTest hack — a proper follow-up.
- **Depth of field** — add `@react-three/postprocessing` `DepthOfField` focused
  on the active body (left out of the default to avoid a mushy look until tuned).
- **Content panels** — slide in each pillar's copy on focus (per the frontend
  spec), so tabs navigate meaning as well as space.
- **Progressive textures + KTX2**, `AdaptiveDpr`, reduced-motion, WebGL
  fallback — the production-hardening pass from the spec.

## Layout

```
src/
  main.jsx        entry
  App.jsx         Canvas · lighting · postprocessing · camera fly-to · nav HUD
  NeutronStar.jsx rotating blue neutron star at the origin (the light source)
  NebulaSky.jsx   stable painterly nebula + starfield backdrop
  Planet.jsx      PBR body (textured or plain) + atmosphere + rings + pulsar
  BlackHole.jsx   OPTIONAL lensing background (not wired in — see roadmap)
  config.js       the system definition (positions, radii, texture slots)
  index.css       HUD styling
public/
  textures/     ← your planet maps (gitignored; see ASSETS.md)
  hdr/          ← your environment HDRI (gitignored)
```
