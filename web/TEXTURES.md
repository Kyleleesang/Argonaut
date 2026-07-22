# Texture variants (8K vs 2K)

The planet/star/station maps exist in two on-disk variants. Both are
**gitignored** (see `.gitignore`); this file plus `ASSETS.md §0` are the only
record of what's where.

| directory | size | what it is |
|---|---|---|
| `public/textures/`    | (whichever is live) | the set the app actually loads |
| `public/textures-8k/` | ~51 MB | the full-resolution originals |
| `public/textures-2k/` | ~3.5 MB | the downscaled web set |

`public/textures/` is a **copy** of one of the two variant directories. Swap it
with a plain copy — nothing else references the variant dirs:

```bash
cp -f public/textures-2k/* public/textures/    # downscaled (fast load)
cp -f public/textures-8k/* public/textures/    # full-resolution originals
```

Same filenames in both, so **hard-refresh the browser after swapping**
(`Ctrl+Shift+R`) — a normal reload serves the old files from HTTP cache and the
swap looks like it did nothing.

## The downscale

Generated from the originals with Pillow (Lanczos, JPEG q88 progressive):
equirectangular planet maps → 2048×1024, station hull maps → 1024², the ring
strip left untouched (its width *is* the radial detail). Result: **52.6 MB →
3.5 MB, ~93% smaller.**

Biggest savings were the least-visible bodies: `prism_albedo.jpg` (Luna's map)
was 15 MB — the largest file in the project — rendering a moon a few pixels
across; the four station hull maps were 11.6 MB combined for a small object.
Earth carries four of the eleven resized maps (albedo/clouds/night/normal), so
it's where any softening concentrates — the body to scrutinise if you compare.

## What the downscale does and doesn't do

- **Load time:** transformed — every visitor downloads 3.5 MB instead of 51 MB.
  This is the reason it exists (GitHub Pages / any real deploy).
- **VRAM:** an 8.3× cut (~1.1 GB → ~134 MB once decoded to RGBA + mipmaps). A
  JPEG's file size is not its GPU cost; it's decoded to raw pixels. This can
  matter for smoothness on low-memory GPUs, where 1.1 GB oversubscribes VRAM and
  the driver hitches evicting/re-uploading. On a desktop GPU with headroom, no
  visible difference.
- **Sustained frame rate:** essentially unchanged, and it does **not** touch the
  Citadel stutter — that's fragment-shader raymarching with no textures
  involved. See `PERFORMANCE.md`.

## Publishing

For a public GitHub Pages / static deploy the textures must be **committed**
(they're gitignored today, so a deploy would ship a broken scene — `BlueStar.jsx`
`useTexture` has no error boundary and would throw). Ship the 2K set. Before
going public also: (1) add a `CREDITS.md` — the Solar System Scope maps are
CC BY 4.0 and require attribution when published; (2) set Vite `base` and route
the ~15 absolute `/textures/…` paths through `import.meta.env.BASE_URL`, since a
project page serves from `/<repo>/`, not the domain root.
