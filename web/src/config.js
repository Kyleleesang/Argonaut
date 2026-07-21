// The Argonaut system. Bodies ORBIT the star: `position` is each body's
// LOCAL offset [radius, y, 0] inside its own orbit group, and `orbit`
// carries the motion. Periods follow Kepler's third law (T ~ r^1.5,
// anchored at Citadel r=25 -> 90s), so outer bodies really do move slower.
//
// ORBITAL RADII ARE CLEARANCE-CONSTRAINED, not chosen by eye. Each body's
// footprint is much bigger than its sphere -- Aegis's rings reach 7.5 and
// Citadel's station orbits at 6.8 -- so neighbouring orbits must be separated
// by more than the SUM of the two footprints or they clip when the phases
// line up. Verified by scratchpad/clearance.mjs, which samples every orbit
// over thousands of periods and reports the true closest approach. Re-run it
// after changing any radius, ring extent, moon distance or station distance.
//
// Keeping the offset LOCAL matters: the star sits at the centre of every
// orbit group, so the direction from a body to the star is constant in that
// body's own frame -- which is what lets Planet.jsx's atmosphere sun-direction
// stay correct as the body orbits, with no per-frame uniform update.
//
// Textures live in public/textures/ (see ASSETS.md). Any body with `textures`
// set renders photoreal PBR maps; `textures: null` falls back to a clean sphere.

// `summary` copy is grounded in docs/FRONTEND_SOLARSYSTEM_SPEC.md's mapping
// table (body -> real subsystem), not invented for flavour.
export const CORE = {
  name: "Core",
  role: "The Network",
  cls: "Blue star",
  accent: "#8fb8ff",
  // Single source of truth for the star's size — App.jsx also derives its pick
  // target and hologram offset from this, so they can't drift out of sync.
  radius: 5.5,
  framing: 22,
  summary:
    "The Argonaut network itself — the source of security and energy every other subsystem orbits. Consensus, storage, and applications all draw on the same chain state.",
  facts: ["Substrate runtime", "Falcon-512 accounts", "No-reorg finality"],
};

export const BODIES = [
  {
    name: "Prism",
    role: "Consensus",
    cls: "Crystalline world",
    summary:
      "Prism parallel-chain ordering: a proposer chain and m independent voter chains order transactions together. Blocks are never reorganised, so there is no best-vs-final split to reconcile.",
    facts: ["Parallel voter chains", "No reorgs", "Deterministic ordering"],
    position: [35, -3, 0],
    orbit: { period: 149, phase: 2.3, inclination: 0.04 },
    radius: 1.6,
    // Real Neptune imagery (Voyager 2 / SSS composite) → a true ice-giant look.
    color: "#ffffff",
    roughness: 0.85,
    metalness: 0.05,
    bumpScale: 0.012,
    framing: 6,
    accent: "#7fe3ff",
    atmosphere: { day: "#9fe6ff", night: "#0a2833", falloff: 3.2, strength: 0.75 },
    textures: {
      map: "/textures/prism_neptune_albedo.jpg",
      bumpMap: "/textures/prism_neptune_albedo.jpg",
    },
  },
  {
    name: "Aegis",
    role: "Storage",
    cls: "Ringed gas giant",
    summary:
      "The decentralised storage layer. Data is erasure-coded into shards held by registered providers, who must post recurring proofs of storage and are paid through streaming payment channels.",
    facts: ["Reed-Solomon shards", "Storage proofs", "Payment channels"],
    position: [62, 2, 0],
    orbit: { period: 351, phase: 3.9, inclination: 0.05 },
    radius: 3.2,
    color: "#ffffff",
    roughness: 1.0,
    metalness: 0.0,
    framing: 12,
    accent: "#ffcf7a",
    // Real Saturn banding + a real ring strip + a field of orbiting asteroid rocks.
    textures: { map: "/textures/aegis_albedo.jpg" },
    rings: {
      inner: 1.28,
      outer: 2.35,
      texture: "/textures/saturn_ring_alpha.png",
      asteroids: 5000,
      color: "#cbb58f",
      tiltX: 0.42,
      tiltZ: 0.16,
      spin: 0.03,
    },
  },
  {
    name: "Ecosystem",
    role: "Applications",
    cls: "Ocean world",
    summary:
      "The application layer built on top of the platform. The dApp catalog orbits here in tiers — Anchor, Keystone and Press closest, with the wider roadmap further out.",
    facts: ["dApp catalog", "Tier-1: Anchor · Keystone · Press", "App-layer pallets"],
    position: [47, -4, 0],
    orbit: { period: 232, phase: 5.5, inclination: 0.03 },
    radius: 2.1,
    color: "#2f5f96",
    roughness: 0.7,
    metalness: 0.0,
    framing: 7,
    accent: "#79ffce",
    atmosphere: { day: "#5aa2ff", night: "#08122e", falloff: 3.0, strength: 1.1 },
    textures: {
      map:         "/textures/earth_albedo.jpg",
      normalMap:   "/textures/earth_normal.jpg",
      emissiveMap: "/textures/earth_night.jpg",   // glows on the dark side
      cloudsMap:   "/textures/earth_clouds.jpg",
    },
    // A real Moon that revolves around Earth (natural grey lunar surface).
    moons: [
      {
        name: "Luna",
        radius: 0.55,
        distance: 4.6,
        period: 24,        // seconds per orbit
        spin: 0.04,
        inclination: 0.32, // tilt of the orbital plane (radians)
        color: "#c8c8c8",
        roughness: 0.95,
        bumpScale: 0.02,
        texture: { map: "/textures/prism_albedo.jpg", bumpMap: "/textures/prism_albedo.jpg" },
      },
    ],
  },
  {
    name: "Citadel",
    role: "Security",
    cls: "Orbital station · neutron star",
    summary:
      "Post-quantum security. Every account is a Falcon-512 keypair, its address the BLAKE3 hash of the public key, with signatures verified natively by the runtime rather than in wasm.",
    facts: ["Falcon-512 signatures", "AccountId = blake3(pubkey)", "Native verification"],
    position: [25, 4, 0],
    orbit: { period: 90, phase: 0.4, inclination: 0.05 },
    radius: 0.9,
    color: "#cfe0ff",
    emissive: "#bcd4ff",
    emissiveIntensity: 6.0,
    // Neutron-star render params (plasma palette + lighthouse beams).
    cold: "#3d63ff",
    hot: "#eef3ff",
    glow: "#9fc4ff",
    beamColor: "#dbe8ff",
    beamLength: 5, // kept short so beams don't wash over the orbiting station
    spin: 1.1,
    lightIntensity: 1.6,
    // Deliberately hangs back twice as far as the other bodies (was 9). The
    // corona and accretion disk are raymarched in the fragment shader, so
    // their cost scales with how much of the SCREEN they cover, not with
    // scene complexity -- filling the viewport means every pixel runs a
    // 36-40 step march and the frame rate collapses. Keeping the camera at
    // arm's length is the cheapest real fix. If you want to fly closer,
    // lower STEPS in NeutronStar.jsx's Corona/AccretionDisk first.
    framing: 18,
    accent: "#9fc4ff",
    textures: null,
    // A futuristic station in orbit around the neutron star -- must stay
    // clear of the accretion disk, whose outer edge is at 5.5 * radius.
    station: { distance: 6.8, period: 46, inclination: 0.42, scale: 0.85 },
  },
];
