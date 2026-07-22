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

**Status (2026-07-21): full texture set is wired and on disk** (gitignored —
see ASSETS.md §0 for the re-download inventory). Build + dev server verified.
Current scene: **Core = a blue star** (`src/BlueStar.jsx`) — convective
photosphere, Eddington limb darkening — which lights the system; Prism = real
Neptune imagery; Ecosystem = 8K Earth (day/night/clouds/normal) with **Luna
orbiting it**; Aegis = real Saturn + textured ring band + 5,000-rock instanced
asteroid field (Cassini-style gap); Citadel = a magnetar
(`src/NeutronStar.jsx`) with collimated particle jets, a dipole magnetic-field
cage, a **volumetric accretion disk**, and a **PBR space station in orbit**
(spinning habitat ring, solar arrays, nav lights). **The bodies orbit the star**
— Citadel innermost, then Prism, Ecosystem and Aegis outward, with Keplerian
periods so outer bodies move slower. Selecting a body — via its tab or by
**clicking the body itself** — opens a **holographic readout**
(`src/Hologram.jsx`) beside it; nothing is annotated until you pick something.
Galactic ambient fill (ambient + hemisphere lights) keeps night sides readable.

### Orbits are clearance-constrained — run the checker

Bodies render far larger than their spheres: Aegis's rings reach 7.5 units,
Citadel's station orbits at 6.8, the star's corona hull at 2.6 × radius. So
neighbouring orbits clip when their phases line up unless separated by more
than the **sum of both footprints**. This cannot be eyeballed — a set of radii
that looked fine on screen had three genuinely overlapping pairs.

```bash
node tools/clearance.mjs     # samples every orbit, prints true closest approach
```

Run it after changing any orbital radius, ring extent, moon distance, station
distance **or corona hull size** — the last one bit once, silently cutting the
star-to-Citadel clearance from 3.2 to 0.5 when the corona grew.

### Volumetric rendering (`Corona`, `AccretionDisk`)

Both are **raymarched volumes, not textured shells**. This distinction is the
whole ballgame: a shell has one fixed silhouette, no internal depth, and zero
parallax when you orbit it, so it reads as a soap bubble no matter what you
paint on it. Here the hull mesh only exists to generate fragments — the
fragment shader analytically intersects the view ray (sphere for the corona,
slab ∩ cylinder for the disk, so the thin disk isn't undersampled) and marches
36–40 steps accumulating emission front-to-back. Both clip the march at the
star's near surface so gas behind the opaque core can't bleed through it.

Three constraints that are easy to break:

- The hull **must be closed** (`openEnded: false`). An open cylinder generates
  no fragments for rays arriving from above, so nothing marches and you get a
  bare ring.
- Use `side: BackSide` — exactly one fragment per ray, so the volume isn't
  integrated twice and double-added.
- Use `CustomBlending` with `One`/`One`, **not** `AdditiveBlending`. The latter
  is `srcAlpha`/`One`, which re-multiplies by an alpha the front-to-back
  accumulation has already applied.

The disk implements real disk physics, not decoration: Keplerian shear
(ω ∝ r^−3/2, sampled in a counter-rotated frame so spiral arms emerge rather
than being drawn as stripes), a T ∝ r^−3/4 blackbody ramp, relativistic
Doppler beaming `(1 + βμ)³`, and magnetospheric inner truncation.

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
- **Volumetric coronae + accretion disk** — raymarched in the fragment shader
  rather than faked with shells (see above).
- **Cinematic camera that tracks** — `setLookAt(..., true)` flies smoothly to a
  body, then hands off to per-frame `moveTo` so the camera stays glued to it as
  it orbits (a one-shot `setLookAt` goes stale the moment the body moves on).
  Drag to orbit, scroll to zoom, both still work while it travels.

## Performance & textures

- **`PERFORMANCE.md`** — why flying in close to Citadel drops frame rate (the
  corona + disk are per-pixel raymarched, so cost scales with screen area), what
  already mitigates it (`framing: 18`), and the fix options in priority order
  (fewer steps/octaves → analytic early-out → 3D noise texture → half-res
  offscreen). Also: why downscaling textures does **not** fix it.
- **`TEXTURES.md`** — the 8K originals vs the 3.5 MB 2K web set, how to swap
  which is live, and what must happen before a public deploy (commit the maps,
  add `CREDITS.md` for the CC BY assets, set the Vite `base`).

## Roadmap (next, in rough priority)

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
  NeutronStar.jsx  magnetar (Citadel): plasma shader, jets, field cage, volumetric corona + accretion disk
  SpaceStation.jsx procedural PBR station (hub, spinning habitat ring, solar arrays)
  NebulaSky.jsx    stable painterly nebula + starfield backdrop
  BlueStar.jsx     the Core: real solar surface map recoloured blue + volumetric corona
  Planet.jsx       PBR body + atmosphere + textured rings + asteroid field + orbiting moons
  Hologram.jsx     holographic readout pinned beside the selected body
  BlackHole.jsx    OPTIONAL lensing background (not wired in — see roadmap)
  config.js        the system definition (orbits, radii, texture slots, moons, station, copy)
  index.css        HUD + hologram styling
tools/
  clearance.mjs    orbit overlap checker (see above) — run after changing sizes
public/
  textures/     ← live map set (gitignored; 8K/2K variants + swap in TEXTURES.md)
  hdr/          ← environment HDRI (gitignored; not wired, USE_HDRI=false)
```

Docs: `ASSETS.md` (texture sourcing), `TEXTURES.md` (8K/2K variants),
`PERFORMANCE.md` (raymarch cost + fixes), `../docs/FRONTEND_SOLARSYSTEM_SPEC.md`
(the design this implements).
