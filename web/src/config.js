// The Argonaut system. Positions are fixed for now (clean cinematic camera
// transitions); orbital motion is a documented follow-up in README.md.
//
// Textures live in public/textures/ (see ASSETS.md). Any body with `textures`
// set renders photoreal PBR maps; `textures: null` falls back to a clean sphere.

export const CORE = {
  name: "Core",
  role: "The Network",
  cls: "Neutron star",
  accent: "#8fb8ff",
  framing: 22,
};

export const BODIES = [
  {
    name: "Prism",
    role: "Consensus",
    cls: "Crystalline world",
    position: [16, 1.5, 4],
    radius: 1.6,
    // Real lunar surface, tinted cool + given crater relief → a crystalline world.
    color: "#bcd8ec",
    roughness: 0.5,
    metalness: 0.18,
    bumpScale: 0.03,
    framing: 6,
    accent: "#7fe3ff",
    atmosphere: { day: "#9fe6ff", night: "#0a2833", falloff: 3.2, strength: 0.75 },
    textures: {
      map: "/textures/prism_albedo.jpg",
      bumpMap: "/textures/prism_albedo.jpg",
    },
  },
  {
    name: "Aegis",
    role: "Storage",
    cls: "Ringed gas giant",
    position: [-15, 0, 12],
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
    position: [8, -1, -18],
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
    position: [2, 5, 24],
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
    framing: 9,
    accent: "#9fc4ff",
    textures: null,
    // A futuristic station in orbit around the neutron star.
    station: { distance: 3.4, period: 46, inclination: 0.42, scale: 0.85 },
  },
];
