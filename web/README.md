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

**Status (2026-07-13): full texture set is wired and on disk** (gitignored —
see ASSETS.md §0 for the re-download inventory). Build + dev server verified.
Current scene: Prism = 8K lunar surface tinted crystalline (bump relief);
Aegis = real Saturn + textured ring band + 5,000-rock instanced asteroid
field (Cassini-style gap); Ecosystem = 8K Earth (day/night/clouds/normal)
with **Luna orbiting it**; Citadel = neutron star (plasma shader + rotating
pulsar beams) with a **PBR space station in orbit** (spinning habitat ring,
solar arrays, nav lights). Galactic ambient fill (ambient + hemisphere
lights) keeps night sides readable.

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
  main.jsx         entry
  App.jsx          Canvas · lighting (star + galactic ambient/hemisphere) · camera fly-to · HUD
  NeutronStar.jsx  neutron star/pulsar: plasma shader, rotating beams, breathing corona
  SpaceStation.jsx procedural PBR station (hub, spinning habitat ring, solar arrays)
  NebulaSky.jsx    stable painterly nebula + starfield backdrop
  Planet.jsx       PBR body + atmosphere + textured rings + asteroid field + orbiting moons
  BlackHole.jsx    OPTIONAL lensing background (not wired in — see roadmap)
  config.js        the system definition (positions, radii, texture slots, moons, station)
  index.css        HUD styling
public/
  textures/     ← planet/station maps (gitignored; inventory in ASSETS.md §0)
  hdr/          ← environment HDRI (gitignored; not wired, USE_HDRI=false)
```
