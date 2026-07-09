# Asset manifest — where the realism comes from

The app boots without any of these (clean PBR spheres). **Photorealism is
unlocked by dropping real texture maps + an HDRI into `public/`.** None of this
is code — it's data, and it's the single biggest fidelity lever (see the
"three levers" discussion: technique / assets / compute).

All sources below are free and licensed for this use. **Record attribution** in
a `CREDITS.md` before shipping publicly (CC BY requires it).

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
