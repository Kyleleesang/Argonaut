// Orbit clearance checker:  node tools/clearance.mjs
//
// Bodies render much larger than their sphere -- rings, asteroid belts,
// moons, accretion disks, orbiting stations -- so neighbouring orbits clip
// when phases line up unless separated by more than the SUM of the two
// footprints. This samples every orbit over thousands of periods and reports
// the true closest approach. Run it after changing any orbital radius, ring
// extent, moon distance or station distance in config.js.

import { BODIES, CORE } from "../src/config.js";

// Worst-case radial reach of each body from its own centre, including
// everything that renders with it: rings, asteroid belts, moons, the
// accretion disk, an orbiting station, atmosphere shells.
const DISK_OUTER = 5.5;   // AccretionDisk: radius * 5.5
const CORONA     = 2.6;   // BlueStar corona hull: radius * 2.6 (keep in sync with BlueStar.jsx)
const STATION_SZ = 0.8;   // rough half-extent of the station model

function extent(b) {
  let e = b.radius * 1.03;                                   // atmosphere shell
  if (b.rings)   e = Math.max(e, b.rings.outer * b.radius);  // rings + asteroids
  if (b.moons)   for (const m of b.moons) e = Math.max(e, m.distance + m.radius);
  if (b.station) e = Math.max(e, b.station.distance + STATION_SZ, b.radius * DISK_OUTER);
  return e;
}

// Position at time t, matching OrbitGroup: tilt about X, then spin about Y,
// with the body at local [r, y, 0].
function posAt(b, t) {
  const [r, y] = b.position;
  const a = (b.orbit.phase ?? 0) + (t * Math.PI * 2) / b.orbit.period;
  const x = r * Math.cos(a), z = r * Math.sin(a);
  const i = b.orbit.inclination ?? 0;
  return [x, y * Math.cos(i) - z * Math.sin(i), y * Math.sin(i) + z * Math.cos(i)];
}

const ext = BODIES.map(extent);
const starExt = CORE.radius * CORONA;
console.log("extents:", BODIES.map((b, i) => `${b.name}=${ext[i].toFixed(2)}`).join("  "),
            `| star=${starExt.toFixed(2)}`);
console.log("orbit r:", BODIES.map((b) => `${b.name}=${b.position[0]}`).join("  "));

const worst = {};
const STEP = 0.05, SPAN = 4000;   // dense sampling over many orbital periods
for (let t = 0; t < SPAN; t += STEP) {
  const p = BODIES.map((b) => posAt(b, t));
  for (let i = 0; i < BODIES.length; i++) {
    const d0 = Math.hypot(...p[i]) - starExt - ext[i];        // vs the star
    const k0 = `star–${BODIES[i].name}`;
    if (worst[k0] === undefined || d0 < worst[k0]) worst[k0] = d0;
    for (let j = i + 1; j < BODIES.length; j++) {
      const d = Math.hypot(p[i][0]-p[j][0], p[i][1]-p[j][1], p[i][2]-p[j][2]) - ext[i] - ext[j];
      const k = `${BODIES[i].name}–${BODIES[j].name}`;
      if (worst[k] === undefined || d < worst[k]) worst[k] = d;
    }
  }
}
console.log("\nclosest approach (surface-to-surface, negative = OVERLAP):");
let bad = 0;
for (const [k, v] of Object.entries(worst).sort((a, b) => a[1] - b[1])) {
  const ok = v > 0 ? (v < 2 ? "tight" : "ok") : "*** OVERLAP ***";
  if (v <= 0) bad++;
  console.log(`  ${k.padEnd(22)} ${v.toFixed(2).padStart(8)}   ${ok}`);
}
console.log(bad ? `\n${bad} overlapping pair(s)` : "\nno overlaps");
