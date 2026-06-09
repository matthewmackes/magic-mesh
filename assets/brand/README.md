# MDE Brand Asset Pack

This directory holds every piece of artwork the Mackes Desktop
Environment shell loads at runtime. Every file in this pack is
**designed to be replaced** — drop in your own SVG/PNG with the
same basename and the running MDE picks it up.

> **Current state (2026-05-21):** 7 PNGs imported from
> ChatGPT-generated source art. The original AI outputs are
> archived in `raw/`; the previous placeholder SVGs are archived
> in `baked/` (and still serve as the `include_bytes!` ultimate
> fallback). Replace any `<basename>.png` here with a hand-traced
> `<basename>.svg` to upgrade that slot to vector + tintable.

---

## Resolution order (how MDE finds your art)

For each [slot](#slot-table), the loader probes each candidate
file extension at each layer in order; **first hit wins**:

1. **`$MDE_BRAND_DIR/<basename>.<ext>`** — set this env var for
   dev workflows and per-user overrides without touching the
   system.
2. **`/usr/share/mde/brand/<basename>.<ext>`** — the system install
   (populated by the RPM from this directory).
3. **Baked fallback** — every shipped binary embeds the contents
   of `assets/brand/baked/*.svg` via `include_bytes!`, so if both
   override layers are missing the UI still renders.

**SVG wins over PNG when both are present in the same layer** —
SVG scales, supports `currentColor` tinting, and generally
renders better in UI contexts. The one exception is
`greeter-hero`, which is intrinsically raster and declares only
`png`.

A missing file at layers 1 and 2 silently falls through to layer
3. This means you can replace one slot (say, just `monogram.png`)
and leave the rest as-is.

## Slot table

| Slot | Basename | Probe order | Current art | Aspect | Used by |
|---|---|---|---|---|---|
| `Wordmark` | `wordmark` | svg → png | `wordmark.png` (2508×627) | 4:1 | Sidebar header, About panel header |
| `WordmarkHero` | `wordmark-hero` | svg → png | `wordmark-hero.png` (2508×627) | 4:1 | About panel hero, greeter |
| `Monogram` | `monogram` | svg → png | `monogram.png` (1254×1254) | 1:1 | Empty states, favicon-scale uses |
| `AppIcon` | `app-icon` | svg → png | `app-icon.png` (1254×1254) | 1:1 | Window manager icon, taskbar |
| `GreeterHero` | `greeter-hero` | png only | `greeter-hero.png` (1672×941) | 16:9-ish | sway greeter background |
| `GreeterWordmark` | `greeter-wordmark` | svg → png | `greeter-wordmark.png` (2508×627) | 4:1 | Greeter foreground over greeter-hero |
| `LogoLockup` | `logo-lockup` | svg → png | `logo-lockup.png` (1254×1254) | 1:1 | About hero, splash surfaces |

### Tintability

`currentColor` fills (when the slot's art is SVG) let the consumer
tint the mark at render time — same SVG can render indigo on
hover, charcoal in print, white on the greeter. The current
shipped PNGs **are not tintable** (PNGs are raster, colors are
baked into pixels). Hand-tracing a PNG to an `.svg` that uses
`currentColor` upgrades that slot.

`is_tintable()` on `BrandSlot` reports the *baked* fallback's
tintability. The fixed-palette slots — `AppIcon`, `GreeterHero`,
`GreeterWordmark`, `LogoLockup` — report `false` because they
must look right outside any host theme.

## Layout

```
assets/brand/
├── README.md               ← this file
├── wordmark.png            ← BR-1 sidebar header
├── wordmark-hero.png       ← BR-4 about panel hero
├── monogram.png            ← BR-3 empty states
├── app-icon.png            ← shipped app icon
├── greeter-hero.png        ← BR-5 greeter background
├── greeter-wordmark.png    ← BR-5 greeter foreground
├── logo-lockup.png         ← BR-4 about hero / splash (1:1 lockup)
├── raw/                    ← original AI outputs (audit trail)
├── baked/                  ← placeholder SVGs, embedded via include_bytes!
├── cursor/                 ← BR-5 cursor theme (not yet populated)
└── sounds/                 ← BR-5 audio assets (not yet populated)
```

## Upgrading a PNG slot to a tintable SVG

The shipped PNGs work, but vectorizing them unlocks:

- Crisp scaling at every size (sidebar 32 px, hero 600 px+, app
  icon 16 px).
- `currentColor` tinting — one file renders correctly in every
  theme and over every background.
- Smaller binary size for slots whose shape is geometric.

The vectorization flow (PNG → SVG):

```bash
# 1. Install potrace (one-time):
sudo dnf install potrace

# 2. Convert the source PNG to a high-contrast PGM:
magick monogram.png -alpha remove -background white \
    -threshold 50% monogram.pgm

# 3. Trace to SVG:
potrace -s -o monogram.svg monogram.pgm

# 4. Hand-tidy monogram.svg:
#    - Set <svg viewBox="0 0 1254 1254"> and strip width/height
#    - Replace fill="#000000" with fill="currentColor"
#    - Set <title> and <desc> for accessibility
#    - Remove the embedded raster <image> potrace sometimes
#      includes
```

Once `monogram.svg` exists in this directory, the loader will
prefer it over `monogram.png` automatically (SVG wins per the
probe order). You can leave the PNG in place as a fallback or
delete it.

## Producing replacement art with an AI image generator

Generators output raster. The flow for net-new art:

### 1. Prompt the AI

**Wordmark (4:1):**

> Minimalist flat vector logo for **'MDE'** (Mackes Desktop
> Environment). Geometric sans-serif letterforms with subtle
> architectural feel. Single solid color (#FFFFFF) on transparent
> background, no gradients, no shadows, no outline strokes, no
> 3D, no photorealism. Hard edges suitable for tracing to SVG.
> Aspect ratio 4:1. Output at 2048×512 PNG. Reference: Berkeley
> Mono, Inter, Geist Mono, Söhne — confident, modern, software-tool
> aesthetic, not corporate-tech.

**Monogram (1:1):** same as above but `'M'` (single letter, not
'MDE'), aspect ratio 1:1, output at 2048×2048 PNG.

**Logo lockup (1:1):** same as wordmark but lockup of the parent
brand name "Mackes" stacked above the "MDE" monogram, aspect
ratio 1:1, output at 2048×2048 PNG.

**Greeter hero (raster, no vectorization step):**

> Abstract architectural background, deep charcoal (#1d1d1f) base
> with indigo (#5b6af5) accent strokes — geometric, minimal, sparse.
> Slight grain texture. 3840×2160 PNG opaque. Reference: Linear app
> login screen, Vercel marketing pages, Apple Pro Display Marketing —
> high-end software brand, not stock-photo-y.

### 2. Drop and probe

```bash
# Drop the AI output into ./raw/ (audit trail), then:
cp raw/my-new-monogram.png monogram.png

# Restart the running shell — it reads on render, no cache:
pkill -USR1 mde-workbench  # (or just relaunch)
```

## Verifying which layer is active

The About panel (BR-4 task) renders each slot's resolved
`BrandSource` — Override / System / Baked — alongside the
preview, so you can confirm at a glance which layer your runtime
is pulling from. From code:

```rust
use mde_theme::{Brand, BrandSlot, BrandSource};
let asset = Brand::new().resolve(BrandSlot::Monogram);
match asset.source {
    BrandSource::Override(p) => println!("override: {p:?}"),
    BrandSource::System(p)   => println!("system:   {p:?}"),
    BrandSource::Baked       => println!("baked fallback"),
}
println!("format: {:?}, {} bytes", asset.format, asset.bytes.len());
```
