# Asset manifest — where the realism comes from

The app boots without any of these (clean PBR spheres). **Photorealism is
unlocked by dropping real texture maps + an HDRI into `public/`.** None of this
is code — it's data, and it's the single biggest fidelity lever (see the
"three levers" discussion: technique / assets / compute).

All sources below are free and licensed for this use. **Record attribution** in
a `CREDITS.md` before shipping publicly (CC BY requires it).

## 0. Current on-disk inventory (2026-07-13) — REPRODUCIBILITY RECORD

`public/textures/` and `public/hdr/` are **gitignored**; this table is the
only record of what's deployed locally and where to re-download it.

| File | Size/res | Used by (config.js slot) | Source (re-download) | License |
|---|---|---|---|---|
| `earth_albedo.jpg` | 8192×4096 | Ecosystem `map` | NASA Blue Marble / solarsystemscope.com | PD / CC BY 4.0 |
| `earth_normal.jpg` | 8192×4096 | Ecosystem `normalMap` | Solar System Scope | CC BY 4.0 |
| `earth_night.jpg` | 8192×4096 | Ecosystem `emissiveMap` | NASA Black Marble / SSS | PD / CC BY 4.0 |
| `earth_clouds.jpg` | 8192×4096 | Ecosystem `cloudsMap` | Solar System Scope | CC BY 4.0 |
| `prism_albedo.jpg` | 8192×4096 | Prism `map`+`bumpMap`; Luna moon `map`+`bumpMap` | solarsystemscope.com/textures/download/8k_moon.jpg | CC BY 4.0 |
| `aegis_albedo.jpg` | 4096×2048 | Aegis `map` | …/download/8k_saturn.jpg (served at 4K) | CC BY 4.0 |
| `saturn_ring_alpha.png` | 8192×500 RGBA | Aegis `rings.texture` (alpha = gaps) | …/download/8k_saturn_ring_alpha.png | CC BY 4.0 |
| `station_hull_diff.jpg` | 2048² | SpaceStation hull `map` | dl.polyhaven.org …/metal_plate/metal_plate_diff_2k.jpg | CC0 |
| `station_hull_nor.jpg` | 2048² | SpaceStation `normalMap` (nor_gl) | Poly Haven metal_plate | CC0 |
| `station_hull_rough.jpg` | 2048² | SpaceStation `roughnessMap` | Poly Haven metal_plate | CC0 |
| `station_hull_metal.jpg` | 2048² | SpaceStation `metalnessMap` | Poly Haven metal_plate | CC0 |
| `hdr/space.hdr` | 73 MB | not wired (`USE_HDRI = false` in App.jsx) | Poly Haven milky-way HDRI | CC0 |

Notes: the ring PNG's **own alpha channel** carries the gap mask — use `map`
only, NOT `alphaMap` (its green channel would flatten opacity to ~34%).
Earth roughness/specular map: not available from SSS (404 at 8K and 2K);
deliberately omitted.

## 1. HDRI environment (image-based lighting) — do this first

IBL gives real reflections + ambient and is the fastest jump toward "real."

| File | Put at | Source | License |
|---|---|---|---|
| Milky-Way / starfield equirectangular `.hdr` | `public/hdr/space.hdr` | Poly Haven "Milky Way" HDRIs — https://polyhaven.com/hdris/skies (search "milky way" / "night") | CC0 |
| (alt) ESO Milky Way panorama | `public/hdr/space.hdr` | https://www.eso.org/public/images/eso0932a/ (convert to .hdr/.exr) | CC BY 4.0 |

Then set `USE_HDRI = true` at the top of `src/App.jsx`.

## 2. Planet texture maps

Drop files in `public/textures/`, then uncomment the matching `textures: { … }`
block in `src/config.js`. Get **8K** where offered; 2K is fine to start.

### Ecosystem (ocean world — the Earth-like hero)
Real Earth data is the most convincing single asset in the scene.

| Map | Slot | Source | License |
|---|---|---|---|
| Day albedo | `map` | NASA Visible Earth "Blue Marble" — https://visibleearth.nasa.gov/collection/1484/blue-marble | Public domain |
| Normal (from topography/bump) | `normalMap` | NASA elevation → normal, or Solar System Scope earth normal | PD / CC BY 4.0 |
| Roughness (land rough, ocean smooth — invert specular mask) | `roughnessMap` | derive from land/ocean mask (Blue Marble bathymetry) | PD |
| Night lights | `emissiveMap` | NASA "Black Marble" — https://earthobservatory.nasa.gov/features/NightLights | Public domain |
| Clouds | `cloudsMap` | Solar System Scope earth clouds — https://www.solarsystemscope.com/textures/ | CC BY 4.0 |

### Aegis (gas giant)
| Map | Slot | Source | License |
|---|---|---|---|
| Banded albedo (Jupiter-class) | `map` | Solar System Scope "Jupiter" 8K — https://www.solarsystemscope.com/textures/ | CC BY 4.0 |

### Prism (crystalline / rocky world)
| Map | Slot | Source | License |
|---|---|---|---|
| Rock albedo | `map` | ambientCG "Rock"/"Ice" — https://ambientcg.com/ | CC0 |
| Rock normal | `normalMap` | ambientCG (same set) | CC0 |
| Rock roughness | `roughnessMap` | ambientCG (same set) | CC0 |

Prism reads best with a cool tint + high specular; the crystalline look also
comes partly from the low `roughness`/higher `metalness` already in config.

### Pulsar (neutron star)
Emissive — no texture needed. Its glow comes from `emissiveIntensity` + bloom.

## 3. Delivery format (before shipping)

For production, compress every texture to **KTX2 / Basis** (GPU memory +
bandwidth): `npx @gltf-transform/cli` or `toktx`. Keep an 8K "focus" variant
and a 2K "far" variant and swap on approach. Raw JP/PNG is fine for local dev.

## 4. Quick mapping recap

`config.js` → `textures: { map, normalMap, roughnessMap, metalnessMap,
emissiveMap, cloudsMap }`. Any subset works; unset slots fall back cleanly.
Albedo/night/clouds are treated as sRGB, normal/roughness/metalness as linear
(handled in `Planet.jsx`).
