# Performance notes

The scene runs smoothly at system-overview framing. The one place frame rate
drops is **flying in close to Citadel** (the magnetar). This document explains
why, what has already been done about it, and the fix options in priority
order — so the next person doesn't have to re-derive it.

## Why Citadel stutters

Citadel's corona and accretion disk are **raymarched in a fragment shader**
(`src/NeutronStar.jsx`, the `Corona` and `AccretionDisk` components). That is
the whole story: the cost is paid **per screen pixel the volumes cover**, not
per object, and for every such pixel the GPU walks a ray through the volume.

Per covered pixel:

| volume | march steps | fbm calls / step | octaves | hash lookups / octave | **hash evals / pixel** |
|---|---|---|---|---|---|
| corona | 40 | 2 | 4 | 8 | **2,560** |
| disk   | 36 | 2 | 4 | 8 | **2,304** |
| where both hulls overlap | | | | | **4,864** |

Because screen area grows as `1/distance²`, halving the camera distance
quadruples the pixel count and therefore the work:

| camera distance | hash evals / frame (1080p) |
|---|---|
| 18 units (the current fly-to framing) | ~1.4 billion |
| 12 units | ~3.0 billion |
| 8 units  | ~4.1 billion |
| 5 units (hulls fill the screen) | ~4.5 billion |

At 60 fps, 1.4 B/frame is ~84 billion hash evaluations per second. The stutter
is not a bug — it is simply more arithmetic than the GPU can do comfortably
once the volumes fill much of the screen.

Three things make it worse than it needs to be:

- **Empty space costs full price.** The march steps uniformly through the hull;
  in sparse regions the noise is evaluated, found near zero, and discarded —
  the `if (d > 0.001)` skips only the *accumulation*, after the expensive part
  already ran.
- **`discard` defeats early-Z**, so the GPU can't cheaply reject these
  fragments before shading them.
- **The `alpha > 0.99` early break rarely fires**, because these volumes are
  deliberately wispy.

## What's already been done

**Citadel's fly-to framing hangs back at `framing: 18`** (double the other
bodies — see the comment in `config.js`). Pulling the camera back is a real fix,
not a workaround: it directly reduces the screen area the volumes cover. Do not
"tidy" that value back down without a cheaper shader.

## What does NOT help (and a common misconception)

**Downscaling the textures does not fix this.** Texture resolution drives
*load time* and *VRAM*, not raymarch cost — sampling a texture costs the same
whether it came from an 8K or 2K source (the GPU reads a filtered mip either
way). The Citadel stutter is pure fragment-shader math with no textures
involved. See `TEXTURES.md` for what the downscale *does* do (load time, and
VRAM, which can matter for smoothness on low-memory GPUs — a different axis).

## Fix options, in priority order

Each value below is a single number in `src/NeutronStar.jsx` unless noted, so
everything here is easy to try and easy to revert.

### 1. Fewer march steps / octaves — cheapest, do this first

`STEPS` (40 corona / 36 disk) and the fbm octave count (`for (int i=0; i<4)`).

- **Steps 40/36 → 24** is a ~37% cut with **no re-tuning and no visual risk
  beyond a little grain.** The Riemann sum already scales each sample by `dt`
  (`a = d * uDensity * dt`), so fewer, fatter steps keep the same brightness and
  opacity — you only lose sampling fidelity, and the per-pixel dither already in
  the shader turns the resulting banding into fine noise rather than rings.
- **Octaves 4 → 3** is the worst cost/benefit term in the shader: the 4th
  octave is 6.7% of the amplitude for 25% of the cost. **But it needs a small
  re-tune** — dropping it shifts fbm's mean by ~0.03, which moves where the
  `smoothstep(0.40, 0.70, …)` thresholds bite (the corona thins). Nudge the
  threshold or density to compensate.
- **Steps 24 + octaves 3 together ≈ 53% cut** (1.4 → ~0.66 B/frame), roughly
  one "distance halving" of headroom — enough to lower `framing` back toward 12.

### 2. Analytic early-out — free, no visual change

Before sampling the noise, compute a cheap upper bound on the radial envelope
(a couple of `smoothstep`s + a `pow`) and `continue` when even the most generous
bound is negligible. Being a conservative bound, it **cannot change the image.**
Expect **15–30%**, limited by (a) how much of the march is genuinely empty — the
disk is already tightly bounded by its slab∩cylinder intersection, so it gains
less — and (b) GPU branch divergence within a warp.

### 3. 3D noise texture instead of procedural fbm — larger win, needs a re-tune

One `vn()` call is ~180 ALU ops (8 corner `hash()` + trilinear blend).
Precompute the noise into a small 3D texture (e.g. 128³) and replace each call
with a single `texture(noiseTex, p)` — the GPU's texture-filtering hardware does
the fetch+interpolation that the silicon exists for. Standard technique in
real-time volumetrics.

Caveats: not the 180× the op count implies — 40 hops per ray thrash the texture
cache, so you become memory-latency-bound instead of ALU-bound; realistically
**3–6×**. Watch for tiling (offset/rotate each octave), precision (R8 bands in
gradients; R16F fixes it), and — the real cost — the streamer/lobe look is tuned
against the current field, so swapping the source **shifts the look and needs
re-tuning.** Needs WebGL2 `sampler3D` (available) + a one-time fill pass.

### 4. Half-resolution offscreen render — biggest win, highest risk, do last

Render the volumes into a half-size buffer and upscale: ¼ the pixels, ~4× less
raymarching, **proportional regardless of shader cost**, and composes with #3
for ~16×.

But it reintroduces the **multi-pass / depth-stencil territory that already
burned this project** — `App.jsx` documents removing `EffectComposer` because
its depth handling threw `GL_INVALID_OPERATION` and strobed on this
three/postprocessing pairing. It also makes depth correctness manual (the
low-res buffer vs full-res geometry), needs depth-aware ("nearest-depth")
upsampling to avoid edge halos where the corona meets the star/station, and
softens exactly the high-contrast detail (streamers, the disk's Doppler-bright
limb) we spent effort building. Worth it only if #1–#3 aren't enough.

## Suggested sequence

Do **#1 (steps → 24) then #2**, measure. That's plausibly ~50% combined with no
visual risk, likely enough to let `framing` come back toward 9–12 for closer
flybys. Reach for **#3** only if you want more (accepting a corona re-tune), and
**#4** last, if ever.
