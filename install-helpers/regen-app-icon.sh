#!/usr/bin/env bash
# Construct brand - regenerate the single-source Construct product icon set from the
# generated brand artwork. One mark everywhere: the RPM installs the
# `assets/brand/construct/app-icon-<N>.png` crops into the standard hicolor
# dirs (`/usr/share/icons/hicolor/<N>x<N>/apps/magic-mesh.png` — see the
# `[package.metadata.generate-rpm]` assets in crates/mesh/mackesd/Cargo.toml),
# every .desktop in packaging/ uses `Icon=magic-mesh` resolved from that set,
# and the favicon is cut from the same crops. Regenerating here re-brands ALL.
#
# SINGLE SOURCE: `assets/brand/CONSTRUCT-MAIN.png`, cropped to the central
# Construct mark. Pass an explicit source image to override (non-square sources
# are center-cropped).
#
# Usage:  ./install-helpers/regen-app-icon.sh [source-icon.png]
#
# Regenerates (PIL LANCZOS — the same method that cut the official rasters):
#   * assets/brand/construct/app-icon-<N>.png   N = 16 22 24 32 48 64 128 256 512
#   * assets/brand/app-icon.png              (512² brand master)
#   * assets/brand/app-launcher.png          (512² shell/taskbar launcher master)
#   * assets/brand/favicon.ico               (real multi-res .ico: 16+32+48)
# Requires python3 + Pillow (PIL).
set -euo pipefail

SRC="${1:-}"
if [ -n "$SRC" ] && [ ! -f "$SRC" ]; then
  echo "usage: $0 [source-icon.png]  (default: the Construct source-art mark crop)" >&2
  exit 2
fi

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

SRC="$SRC" python3 - "$ROOT" <<'PY'
import os
import sys

from PIL import Image

root = sys.argv[1]
brand = os.path.join(root, "assets", "brand")
construct = os.path.join(brand, "construct")
os.makedirs(construct, exist_ok=True)

# The one source: the central Construct mark cropped from CONSTRUCT-MAIN.png.
CROP_BOX = (434, 0, 974, 540)

src = os.environ.get("SRC") or ""
if src:
    img = Image.open(src).convert("RGBA")
    if img.width != img.height:  # center-crop a non-square override to square
        side = min(img.size)
        left = (img.width - side) // 2
        top = (img.height - side) // 2
        img = img.crop((left, top, left + side, top + side))
    print(f"source: {src} ({img.width}x{img.height})")
else:
    main = os.path.join(brand, "CONSTRUCT-MAIN.png")
    img = Image.open(main).convert("RGBA").crop(CROP_BOX)
    print(f"source: {main} crop {CROP_BOX} ({img.width}x{img.height})")

sizes = (16, 22, 24, 32, 48, 64, 128, 256, 512)
crops = {n: img.resize((n, n), Image.LANCZOS) for n in sizes}

for n in sizes:
    dst = os.path.join(construct, f"app-icon-{n}.png")
    crops[n].save(dst, "PNG", optimize=True)
    print(f"wrote {dst}")

dst = os.path.join(brand, "app-icon.png")
crops[512].save(dst, "PNG", optimize=True)
print(f"wrote {dst}")

dst = os.path.join(brand, "app-launcher.png")
crops[512].save(dst, "PNG", optimize=True)
print(f"wrote {dst}")

# Multi-res favicon from the 16/32/48 crops (not one image auto-resized).
# The base frame must be the LARGEST — Pillow's ICO writer drops any listed
# size bigger than the base image, even when append_images provides it.
dst = os.path.join(brand, "favicon.ico")
crops[48].save(
    dst,
    format="ICO",
    append_images=[crops[16], crops[32]],
    sizes=[(16, 16), (32, 32), (48, 48)],
)
ico_sizes = sorted(Image.open(dst).ico.sizes())
assert ico_sizes == [(16, 16), (32, 32), (48, 48)], f"favicon not multi-res: {ico_sizes}"
print(f"wrote {dst} (sizes: {ico_sizes})")
PY

echo "done — re-brand the fleet by cutting a release (the Construct hicolor set + brand masters ship in the RPM; %post runs gtk-update-icon-cache)."
