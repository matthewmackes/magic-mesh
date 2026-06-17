#!/usr/bin/env bash
# BRAND — regenerate the single default app icon (`magic-mesh`) across every size
# from one source image. Every .desktop in packaging/ uses `Icon=magic-mesh`,
# resolved from the hicolor theme, so regenerating this set re-brands ALL apps.
#
# Usage:  ./install-helpers/regen-app-icon.sh <source-icon.png>
#
# Regenerates:
#   * assets/icons/hicolor/<size>/apps/magic-mesh.png  (16…512 — the app icon)
#   * assets/brand/app-icon.png                         (580² brand master)
#   * assets/brand/monogram.png                         (256² monogram)
# Requires ImageMagick (`magick`/`convert`).
set -euo pipefail

SRC="${1:-}"
[ -n "$SRC" ] && [ -f "$SRC" ] || { echo "usage: $0 <source-icon.png>" >&2; exit 2; }

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
HICOLOR="$ROOT/assets/icons/hicolor"
BRAND="$ROOT/assets/brand"

# Prefer `magick` (IM7); fall back to `convert` (IM6).
if command -v magick >/dev/null 2>&1; then IM="magick"; else IM="convert"; fi

SIZES="16 22 24 32 48 64 128 256 512"
for s in $SIZES; do
  dst="$HICOLOR/${s}x${s}/apps/magic-mesh.png"
  mkdir -p "$(dirname "$dst")"
  # -strip metadata; fill the square (^) + center-crop (-extent) so a near-square
  # source gets no transparent bars or distortion; high-quality downscale, alpha kept.
  $IM "$SRC" -strip -resize "${s}x${s}^" -background none -gravity center -extent "${s}x${s}" "$dst"
  echo "wrote $dst"
done

$IM "$SRC" -strip -resize 580x580^ -background none -gravity center -extent 580x580 "$BRAND/app-icon.png"
echo "wrote $BRAND/app-icon.png"
$IM "$SRC" -strip -resize 256x256^ -background none -gravity center -extent 256x256 "$BRAND/monogram.png"
echo "wrote $BRAND/monogram.png"

echo "done — re-brand the fleet by cutting a release (the magic-mesh hicolor set + brand masters ship in the RPM; %post runs gtk-update-icon-cache)."
