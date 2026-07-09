# Argonaut Frontend — Solar-System Experience Spec

**Status: plan / documentation (2026-07-09), no code.** A build plan for the
public-facing Argonaut website as an explorable 3D solar system: the network
is the star, each technology pillar is a planet, each dApp is a moon, and
selecting a section flies the camera to that world and drops into an orbiting
focus view. The goal is a genuinely high-fidelity, "screenshot-worthy"
experience that still loads fast, ranks in search, and works for keyboard and
reduced-motion users.

This is a frontend deliverable and lives **outside the Rust workspace** (its
own `web/` directory or a separate repo). It touches no chain code.

Guiding principle up front: **the 3D is an enhancement layer over a real,
server-rendered content site — never the other way around.** Everything
readable must exist in the DOM so it is crawlable, accessible, and survives
WebGL being unavailable. That single decision shapes the whole stack.

---

## 1. Concept → information architecture

The solar system is not decoration bolted onto a nav bar; it *is* the nav.
The mapping is deliberate and mirrors the real architecture (see
`ARGONAUT_ARCHITECTURE_GUIDE.md`, `DAPP_CATALOG_SPEC.md`), which is what makes
it feel earned rather than themed-for-the-sake-of-it.

| Body | Site section | Real thing it represents | Signature visual |
|---|---|---|---|
| **The Star — "Argonaut"** | Home / landing | The network itself: the source of security and energy everything orbits | Emissive core, animated turbulence, corona, god-rays, lens flare |
| **Prism** (inner planet) | Consensus / how it works | Prism parallel-chain ordering (no reorgs) | Crystalline planet refracting the star's light into separate orbital light-bands = *m* voter chains + proposer + tx chain |
| **Falcon** (inner planet) | Security / post-quantum | Falcon-512 signatures on every account | Deep-blue world sheathed in a shimmering magnetosphere/aurora — cryptographic "shield" |
| **Aegis** (ringed gas giant) | Storage platform | Implementation D storage/retrieval | Saturn-class rings whose banding = erasure-coded shards; small moons = storage providers |
| **Ecosystem** (large planet) | Apps / dApp catalog | The application layer | A planet whose **moons are the dApps** — Anchor, Keystone, Press, Bazaar, Vault, Relic, Meter — orbiting the platform exactly as they do architecturally |
| **Forge** (planet or station) | Developers / docs | SDKs, specs, running a node | Structured, grid-lit world or orbital station; "under construction" honesty is on-theme |
| **Assembly** (ringed planet) | Governance / treasury | OpenGov council + treasury (E spec) | Ringed world; ring segments light up = active referenda |

Design rules for the IA:
- **Top-level tabs = planets.** Never more than ~7 primary worlds (cognitive
  load + orbital legibility). The table above is exactly 6 planets + star.
- **Second-level items = moons.** The dApp catalog's tier structure becomes
  moon distance: Tier-1 apps (Anchor, Keystone, Press) closest and brightest;
  Tier-2 further out; Tier-3 as faint distant satellites. The IA teaches the
  roadmap without a word of copy.
- **Deep content = surface panels.** Selecting a moon or "landing" on a planet
  opens an HTML content panel; it does not spawn a third nested 3D level.
  Two levels of 3D (planet → moon), then flat content. Resist infinite zoom.

## 2. Recommended stack (and why)

| Layer | Choice | Rationale |
|---|---|---|
| Site shell / SSR | **Astro** (islands) | Ships zero JS for content by default; renders all copy to static HTML (SEO + a11y + fast FCP); mounts the 3D as a single hydrated island. Next.js App Router is the alternative if the team wants React everywhere. |
| 3D runtime | **React Three Fiber (R3F)** + **@react-three/drei** | Declarative scene graph, huge helper library (`Stars`, `Environment`, `useTexture`, `Html`, `AdaptiveDpr`), maintainable by web devs. Vanilla Three.js is the alternative for smallest bundle / maximum control — pick it only if the team is Three-native and wants no React overhead. |
| Postprocessing | **@react-three/postprocessing** (pmndrs) | Bloom, god-rays, DoF, SMAA, tone mapping as composable effects; the single biggest source of "wow" per line of code. |
| Camera / tweening | **GSAP** (or a critically-damped spring) | Smooth, interruptible camera choreography with real easing curves. Avoid naive `lerp` — it can't do overshoot/anticipation. |
| State | **Zustand** | One tiny store for `mode`, `focusedBody`, `cameraTarget`; readable from both DOM UI and the R3F scene without prop-drilling. |
| Content source | **Markdown/MDX** or a headless CMS | Copy lives as data, not hardcoded in JSX — so non-devs can edit and content stays crawlable. |
| Tuning | **Leva** (dev-only) | Live-tweak shader/bloom/camera params; strip from production build. |

Hosting: static/edge — Cloudflare Pages, Vercel, or Netlify; assets on a CDN.
No server needed beyond SSR/prerender at build time.

## 3. Project structure

```
web/
  src/
    pages/               # Astro routes: /, /prism, /falcon, /storage, /apps, ...
                         #   each is real SSR content — the source of truth
    content/             # MDX per section + a bodies.ts config (orbit params, copy keys)
    scene/               # the R3F island
      Scene.tsx          # <Canvas>, camera rig, postprocessing stack
      bodies/            # Star.tsx, Planet.tsx, Moon.tsx, Rings.tsx, AsteroidBelt.tsx
      shaders/           # atmosphere.glsl, sun.glsl, dayNight.glsl, rings.glsl
      camera/            # CameraRig.tsx + state machine (§5)
      systems/           # orbits, LOD, lazy-texture loader
    ui/                  # DOM overlay: nav, content panels, loader, controls
    store.ts             # zustand
  public/
    textures/            # KTX2/Basis compressed; 2K base + 4K focus variants
    hdr/                 # Milky-Way environment map(s)
    audio/               # optional ambient + UI sfx
```

`web/` is decoupled from the cargo workspace and has its own toolchain (Vite
via Astro). Node/pnpm, not cargo.

## 4. Scene architecture (layers)

Rendered back-to-front, each on its own concern:

1. **Skybox / deep space** — high-res equirectangular Milky-Way HDR as both
   background *and* image-based lighting (IBL) source, so planets pick up
   subtle galactic ambient. One hero HDR (~4–8K, compressed).
2. **Starfield** — additive `Points` BufferGeometry, size-attenuated, gentle
   per-star twinkle in a shader (`sin(time + seed)`). ~3–5k points; cheap.
3. **The star (Sun)** — emissive core + corona + light source. It is the
   scene's single `PointLight`/`DirectionalLight` origin so every planet's
   day/night terminator is physically consistent (§6.3).
4. **Orbital system** — planets on elliptical paths, each with optional rings,
   moons, and (for Aegis) an asteroid/shard belt.
5. **Atmospheres** — per-planet additive back-sphere shells (§6.2), drawn
   after opaque geometry.
6. **Postprocessing** — the full-frame effect chain (§6.6).
7. **DOM overlay** (outside WebGL) — nav, content panels, loader, controls,
   the accessible parallel navigation.

Scene units: work in a scaled system (1 unit = 10⁶ km-ish, *artistic* not
literal — real solar-system distances are unwatchable). Log-depth or a
generous `camera.far` with a `near` kept high enough to avoid z-fighting on
rings.

## 5. Navigation & camera choreography (the core interaction)

A three-state machine in Zustand drives everything:

```
        select planet / moon                 back / Esc
OVERVIEW ─────────────────────► TRANSIT ─────────────► FOCUS
   ▲                                                     │
   └──────────────── back / Esc ◄────────────────────────┘
```

- **OVERVIEW** — heliocentric. Slow auto-orbit of the camera around the star;
  damped `OrbitControls` let the user drag. All planets visible on their
  orbits. This is the "map".
- **TRANSIT** — on select, GSAP tweens the camera along a curved path
  (quadratic Bézier, not a straight line — arcs read as "flying") from its
  current pose to a framing pose near the target planet. 1.2–1.8 s, ease
  `power2.inOut`. Interruptible: selecting a different planet mid-transit
  re-targets the tween from the live pose. A subtle motion-blur/streak
  postprocess pulses during transit for a "warp" feel (skip under reduced
  motion).
- **FOCUS** — camera parks in an orbit *around the planet* and slowly circles
  it (this is the "rotating orbit around it" the brief asks for): the planet
  rotates on its axis, its moons/rings revolve, and the camera describes a
  gentle arc so the lighting terminator sweeps across the surface —
  maximally flattering. A content panel slides in from the side (DOM). In
  FOCUS, drag orbits the planet with clamped polar angle and zoom.

Entry points must stay in sync both directions:
- Clicking a 3D body → sets `focusedBody`, drives the machine, **and** pushes
  a route (`/prism`) so URLs are shareable and back-button works.
- Navigating to `/prism` directly (deep link, SSR) → scene reads the route on
  hydrate and starts in FOCUS on Prism (no forced fly-through from home).
- The DOM nav `<a href>` links are the accessible path and trigger the same
  transitions via the router — the 3D just visualizes the route change.

Camera comfort (anti-motion-sickness, normative): eased starts/stops, no roll,
capped angular velocity, no sudden FOV changes. `prefers-reduced-motion` ⇒
transitions become near-instant cross-fades and auto-orbit stops (§9).

## 6. Graphical-fidelity playbook

This is where "impress people" is won or lost. Budget effort by tier:
**hero elements** (the Sun + one hero planet, e.g. Falcon or Aegis) get the
expensive treatment; the rest get the "good default." Consistency of lighting
and tone-mapping across all bodies matters more than any single fancy shader.

### 6.1 Materials — physically based, IBL-lit
Planets use `MeshStandardMaterial`/`MeshPhysicalMaterial` with albedo +
normal + roughness (+ AO) maps, lit by the Sun's light plus the Milky-Way
HDR as environment. ACES Filmic tone mapping and correct color management
(`THREE.SRGBColorSpace`) are non-negotiable — they are 80% of why renders
look "cinematic" vs "flat WebGL demo."

### 6.2 Atmospheres (fresnel rim glow)
A second, slightly larger sphere per atmospheric planet, back-face rendered,
additive blend, with a fresnel term:
```glsl
float rim = pow(1.0 - dot(normalize(vNormal), normalize(vViewDir)), falloff);
gl_FragColor = vec4(atmosphereColor * rim * intensity, rim);
```
Tune `falloff`/`intensity`/`color` per planet (Falcon = teal aurora, Aegis =
pale gold). For the **one hero planet**, upgrade to real Rayleigh/Mie
scattering (view-ray sampled) — gorgeous but costly, so exactly one.

### 6.3 Day/night terminator + city lights
Mix a day albedo and a night texture by the sun-facing dot product:
```glsl
float day = clamp(dot(normal, sunDir), 0.0, 1.0);
vec3 col = mix(nightTex * emissiveCityLights, dayTex, smoothstep(-0.05, 0.15, day));
```
The soft terminator band + emissive night-side lights (great for a
"populated network" metaphor on Ecosystem) read as high production value.

### 6.4 Clouds
Separate transparent sphere just above the surface, own slow rotation,
alpha-mapped, subtly casting darkening onto the surface (cheap: sample cloud
alpha in the surface shader, or a second shadow pass for the hero planet).

### 6.5 The Star (single biggest "wow")
- Emissive core sphere with animated FBM/curl noise scrolling over the
  surface (granulation, flares).
- Corona: additive fresnel shell, larger, animated.
- **Bloom** makes it glow (§6.6) — the star must be far brighter than 1.0 in
  the HDR buffer so bloom blooms.
- **Volumetric god-rays** (radial light-scatter postprocess) from the star
  when it's on-screen — the money shot in OVERVIEW.
- Lens flare (drei `Lensflare` or custom) when the camera looks near it.

### 6.6 Postprocessing chain (order matters)
Render to an HDR float buffer, then:
1. **Selective bloom** — star + emissive city lights + ring highlights only
   (selective, so albedo doesn't wash out).
2. **God-rays** — from the star (OVERVIEW/TRANSIT).
3. **Depth of field** — subtle, focus distance = active planet; sells scale.
4. **Chromatic aberration + vignette** — very subtle, edges only.
5. **Film grain** — faint; kills banding in the dark gradients of space.
6. **Tone map (ACES Filmic) + SMAA** (or TAA if the jitter budget allows).
Every effect has a global intensity that reduced-motion / low-power scales
toward zero.

### 6.7 Rings & belts
- **Rings** (Aegis, Assembly): ring geometry with a radial gradient/banding
  texture, transparent, receives the planet's shadow and casts a soft band
  onto the planet. For Aegis, animate faint per-band brightness to imply
  "shards being served."
- **Asteroid/shard belt**: `InstancedMesh`, 2–5k low-poly rocks, per-instance
  orbital angle advanced each frame; one draw call. Doubles as Aegis's
  "distributed storage" motif.

### 6.8 The consistency checklist (what actually makes it look pro)
Single light origin (the star) · uniform tone mapping · correct sRGB ·
IBL on every material · no pure-black (space is very dark navy + grain, never
`#000`) · bloom reserved for genuinely-emissive things · restrained DoF/CA.
A scene that nails these with plain textures beats a scene of fancy shaders
with inconsistent lighting.

## 7. Content integration (focus panels)

When a body is focused, a DOM panel (not in-canvas) presents the real
Argonaut content for that section — headings, prose, links to docs, code
snippets. Because it's DOM: real text, selectable, translatable, crawlable,
styleable with normal CSS, and screen-reader friendly.

Per-planet content is authored in MDX under `src/content/`; the scene reads a
sibling `bodies.ts` for orbital parameters (radius, period, tilt, texture
set, atmosphere color, moon list). Editing copy never touches scene code.

Example mapping to real material:
- **Prism panel** → the "why no reorgs / throughput" story (guide §4).
- **Falcon panel** → post-quantum signatures, the harvest-now-forge-later
  framing (dApp catalog §1 / guide §2).
- **Ecosystem moons** → one card per dApp from `DAPP_CATALOG_SPEC.md`, each
  linking to its section; moon size/brightness ∝ tier.

## 8. Performance budget

| Target | Value |
|---|---|
| Desktop mid-GPU | 60 fps sustained |
| Mobile / integrated | 30–60 fps, reduced particle + effect counts |
| Initial JS (hydration island) | keep the R3F bundle lean; content ships as static HTML with zero JS |
| First textures | 2K KTX2/Basis per visible planet; **4K variants lazy-loaded on focus** |
| Draw calls | budgeted; belts/stars/moons instanced |

Techniques:
- **KTX2 + Basis** compressed, mipmapped textures (GPU memory + bandwidth).
- **LOD** spheres per planet (high poly only when focused/near).
- **Progressive load**: low-res base in immediately, swap high-res on
  approach; a `Suspense` boundary with a hyperspace-style loader.
- Cap `devicePixelRatio` (≤ 2 desktop, ≤ 1.5 mobile); drei `AdaptiveDpr` +
  `AdaptiveEvents` to shed resolution under load.
- Pause the render loop when the tab is hidden; consider `frameloop="demand"`
  only if idle states truly have no motion (orbits animate, so likely
  continuous with a hidden-tab guard).
- Frustum culling on; keep transparent draws (atmospheres, rings) minimal and
  sorted.
- Profile with spector.js / the R3F perf HUD each fidelity PR — regressions
  are easy to introduce and hard to notice.

## 9. Accessibility, fallback & responsive

Non-negotiable, and easiest if designed in from M0, not retrofitted.

- **SSR content is the substrate.** Every section renders full HTML server-
  side; the canvas is progressive enhancement. Disable JS → the site is a
  clean, navigable, readable document. This also *is* the SEO story.
- **WebGL / power detection.** No WebGL2, or `deviceMemory`/GPU heuristics
  flag low-end ⇒ skip the scene, show a lightweight fallback (static hero
  render or a CSS/2D-canvas star backdrop) with the identical DOM nav. Never
  a blank canvas or a spinner of death.
- **`prefers-reduced-motion`** ⇒ stop auto-orbit and planet spin, replace
  fly-throughs with cross-fades, drop bloom-pulse/warp/parallax, freeze
  belts. The map still works; it just stops moving on its own.
- **Keyboard**: Tab cycles bodies (focus ring rendered as an on-planet
  reticle), Enter focuses, Esc returns to OVERVIEW, arrows rotate the focused
  planet. All reachable without a mouse.
- **Screen readers**: the WebGL canvas is `aria-hidden`; the parallel
  semantic `<nav>` + panels carry the real interaction. Focus is moved into a
  panel when it opens and returned on close.
- **Responsive**: on small screens, simplify to a "planet carousel" — swipe
  between bodies, reduced effects, smaller textures. Don't try to cram the
  full orbital map onto a phone; degrade gracefully to the same content.
- **Motion sickness**: reduced-motion covers the switch; also cap camera
  speeds and avoid roll/FOV punches even in the full experience.

## 10. Audio (optional, high impact-per-effort)
Ambient space drone (low, evolving), a soft "whoosh" on TRANSIT, a gentle
chime on FOCUS. **Muted by default** (autoplay policy + respect), with a
persistent mute/unmute control and reduced-motion implying reduced/aud-off by
default. Keep total audio payload small and lazy-loaded.

## 11. Asset pipeline
- Textures authored/sourced at 4K, exported to 2K + 4K KTX2 (Basis UASTC for
  normals, ETC1S for albedo) via `toktx`/`gltf-transform`.
- Planet/moon meshes are just UV spheres (procedural in-engine) — no modeling
  needed except optional ring/rock meshes (glTF, Draco-compressed).
- HDR environment downsampled + compressed; one hero map reused as IBL.
- A `bodies.ts` manifest is the single source of orbital + asset params;
  adding a planet = one entry + one MDX file + a texture set.
- License hygiene: use CC0/owned textures (e.g. Solar System Scope, NASA) —
  record sources in `web/CREDITS.md`.

## 12. Build, test, deploy
- Astro build → static HTML + hashed assets + the hydrated scene island.
- CI: typecheck, lint, Lighthouse budget (SEO/a11y/perf ≥ 90), a visual
  smoke test (Playwright screenshot of OVERVIEW + one FOCUS), and a
  bundle-size gate on the island.
- Deploy to edge/CDN; textures/HDR/audio on the CDN with long cache + hashes.
- Preview deploys per PR so fidelity changes are reviewable visually.

## 13. Milestones (phased, each shippable/demoable)

- **M0 — Spike (proof the pipeline).** R3F `<Canvas>`, one IBL-lit textured
  planet, `OrbitControls`, ACES tone mapping, bloom. Confirms the look is
  achievable and the toolchain is sound. Timebox tight; it's a go/no-go.
- **M1 — Scene skeleton.** Star + all 6 planets on orbits, starfield,
  skybox/IBL, OVERVIEW camera + auto-orbit. No shaders yet. It should already
  read as "a solar system."
- **M2 — Navigation.** The OVERVIEW→TRANSIT→FOCUS state machine, GSAP camera
  choreography, route sync + deep links, placeholder DOM content panels,
  keyboard nav. The *experience* works even before it's pretty.
- **M3 — Fidelity pass.** Atmospheres, day/night, clouds, the hero Sun
  (corona + god-rays + flare), full postprocessing chain, rings, asteroid
  belt, DoF. This is the "wow" milestone.
- **M4 — Content.** Real Argonaut copy per planet (MDX), the dApp moons from
  the catalog, doc links, focus-panel design. The site becomes *informative*,
  not just pretty.
- **M5 — Performance + a11y + fallback.** LOD, KTX2 compression, progressive
  loading, AdaptiveDpr, reduced-motion, WebGL fallback, mobile carousel,
  Lighthouse ≥ 90. The "ships to real users" milestone.
- **M6 — Polish.** Audio, loader animation, micro-interactions (hover halos,
  reticle), cross-browser QA, final tuning pass with Leva then strip it.

Dependency spine: M0 gates everything; M2 needs M1; M3/M4 parallelize after
M2; M5 is continuous but formalized before launch; M6 last. A credible public
launch is **M0–M5**; M6 is the layer that turns "impressive" into "memorable."

## 14. Risks & mitigations

| Risk | Severity | Mitigation |
|---|---|---|
| Content trapped in canvas → invisible to Google & screen readers | High | SSR-content-first architecture (§2/§9) is the whole mitigation — design it in at M0 |
| Poor perf on mobile/low-end → jank or crash | High | Fallback + LOD + compression budgeted from M0; AdaptiveDpr; test on real mid-tier phones each milestone |
| Fidelity scope creep (every planet gets Rayleigh scattering) | Med | Hero-vs-default tiering (§6); one hero planet, one hero star, good defaults elsewhere |
| Asset weight blows the bandwidth budget | Med | 2K base + lazy 4K, KTX2, CDN, hard bundle gate in CI |
| Motion sickness / accessibility complaints | Med | reduced-motion + capped camera dynamics as acceptance criteria, not afterthoughts |
| Maintenance (copy hardcoded in JSX) | Low | MDX/CMS content + `bodies.ts` manifest; adding a planet is data, not code |
| WebGL context loss on long sessions | Low | Handle `webglcontextlost`, attempt restore, fall back to static hero on repeated failure |

## 15. Definition of done
60 fps on a mid-tier desktop GPU; graceful 30 fps+ with reduced effects on
mobile; Lighthouse SEO + a11y + perf ≥ 90; full experience under
`prefers-reduced-motion`; complete keyboard navigation; a real 2D/static
fallback when WebGL is unavailable; **all section copy present and crawlable
in server-rendered HTML**; every planet maps to a real Argonaut section with
accurate content; shareable deep-link URLs per body. Only then is the
"impressive" allowed to be the *first* thing people notice — because
everything underneath it is solid.

---

### Appendix A — why this theme fits Argonaut specifically
It isn't arbitrary sci-fi dressing. Argonaut's architecture is *literally* a
core with independent bodies in orbit: Prism's parallel chains, a platform
(storage) with providers around it, and a catalog of apps that orbit the
platform (the dApp catalog even describes apps as living in orbit around the
chain, with tiers by distance). The solar system is the truest possible
visual metaphor for "one gravitational core, many independent worlds held in
its ordering" — which is exactly what Prism-ordered, PQ-secured Argonaut is.
The name itself — Argonaut, a voyager among worlds — closes the loop.
