#!/usr/bin/env bash
# preview-gallery.sh — OBS-4 screenshot-artifact gallery.
#
# Captures deterministic Construct and Car home views as PNGs into an output
# directory and writes an index.html contact sheet, so CI can post the render
# artifact for human visual review (AI_GOVERNANCE §4; this is advisory and does
# not replace the `.15` DRM pixel gate).
#
# Usage:  preview-gallery.sh [out-dir]
#   e.g.  preview-gallery.sh /tmp/mde-gallery
#
# The capture is best-effort and emits a placeholder note on failure. Exit 0
# only when the live shell produced an image.
set -u

OUT_DIR="${1:-${MDE_GALLERY_DIR:-/tmp/mde-gallery}}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CAPTURE="$HERE/preview-capture.sh"

if [ ! -x "$CAPTURE" ]; then
  echo "preview-gallery: $CAPTURE not found/executable" >&2
  exit 2
fi

VIEWS=(
  "construct-home|construct|Construct — untitled all-icons Desktop"
  "car-home|car|Car — dashboard and driver instrument strip"
)

if [ "${1:-}" = "--self-test" ]; then
  set -e
  [ "${#VIEWS[@]}" -eq 2 ]
  [ "${VIEWS[0]}" = "construct-home|construct|Construct — untitled all-icons Desktop" ]
  [ "${VIEWS[1]}" = "car-home|car|Car — dashboard and driver instrument strip" ]
  echo "preview-gallery: self-test passed (Construct + Car views)"
  exit 0
fi

mkdir -p "$OUT_DIR"

INDEX="$OUT_DIR/index.html"
{
  echo "<!doctype html><meta charset=utf-8>"
  echo "<title>MCNF — Construct + Car preview gallery</title>"
  echo "<body style='background:#161616;color:#f4f4f4;font-family:sans-serif;margin:24px'>"
  echo "<h1>MCNF — Construct + Car preview gallery</h1>"
  echo "<p style='color:#8d8d8d'>Construct and Car render artifacts. Captured headlessly (sway + grim, software render); this is advisory and does not replace the live .15 DRM pixel gate.</p>"
} > "$INDEX"

ok=0
fail=0
for entry in "${VIEWS[@]}"; do
  IFS='|' read -r fname profile label <<< "$entry"
  png="$OUT_DIR/$fname.png"
  echo "preview-gallery: capturing '$label'…" >&2
  if MDE_PREVIEW_PROFILE="$profile" "$CAPTURE" "$png" >&2; then
    ok=$((ok + 1))
    {
      echo "<figure style='display:inline-block;margin:12px;vertical-align:top'>"
      echo "<img src='$fname.png' width='560' style='border:1px solid #393939;display:block'>"
      echo "<figcaption style='color:#c6c6c6;margin-top:6px'>$label</figcaption>"
      echo "</figure>"
    } >> "$INDEX"
  else
    fail=$((fail + 1))
    {
      echo "<figure style='display:inline-block;margin:12px;vertical-align:top;width:560px;height:360px;border:1px dashed #fa4d56'>"
      echo "<figcaption style='color:#fa4d56;padding:12px'>capture FAILED — $label</figcaption>"
      echo "</figure>"
    } >> "$INDEX"
  fi
done

echo "</body>" >> "$INDEX"
echo "preview-gallery: $ok captured, $fail failed → $OUT_DIR (index: $INDEX)"

# Succeed if anything rendered; total failure means the render path is broken.
[ "$ok" -gt 0 ]
