#!/usr/bin/env bash
# preview-gallery.sh — OBS-4 screenshot-artifact gallery.
#
# Captures a curated set of Workbench views as PNGs into an output directory
# (wrapping the single-view preview-capture.sh over a slug list) and writes an
# index.html contact sheet, so CI can post the gallery as a build artifact for
# human visual review (AI_GOVERNANCE §4 Carbon look — the on-session visual
# gate is lifted, but a reviewer can still eyeball the rendered surfaces here).
#
# Usage:  preview-gallery.sh [out-dir]
#   e.g.  preview-gallery.sh /tmp/mde-gallery
#
# Each capture is best-effort: a slug that fails to render is logged + skipped
# (it still emits a placeholder note in the index). Exit 0 if at least one
# view was captured; non-zero only if every capture failed (a real breakage).
set -u

OUT_DIR="${1:-${MDE_GALLERY_DIR:-/tmp/mde-gallery}}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CAPTURE="$HERE/preview-capture.sh"

if [ ! -x "$CAPTURE" ]; then
  echo "preview-gallery: $CAPTURE not found/executable" >&2
  exit 2
fi

mkdir -p "$OUT_DIR"

# The curated surfaces. Format: "slug|filename|Human label". An empty slug is
# the default home view. Slugs are <group>[.<panel>] per view_from_focus_slug
# (model.rs); group slugs: peers/node/controller/network/fleet/provisioning/
# dashboard/apps/devices/compute/look_and_feel/maintain/system/help.
VIEWS=(
  "peers.peers|peers-front-door|Peers — the Front Door (PD-3)"
  "look_and_feel.themes|themes|Look & Feel — Themes (Carbon)"
  "look_and_feel.wallpaper|wallpaper|Look & Feel — Wallpaper"
  "fleet.fleet_rollup|fleet-rollup|Fleet — Mesh Health rollup (PD-20)"
  "compute|compute-instances|Compute — Instances"
  "devices.connect|connected-devices|Devices — Connected Devices (KDC hub)"
  "maintain.audit|audit|Maintain — Audit"
  "network.remote_desktop|remote-access|Network — Remote Access"
  "dashboard|dashboard|Dashboard — home"
)

INDEX="$OUT_DIR/index.html"
{
  echo "<!doctype html><meta charset=utf-8>"
  echo "<title>Magic Mesh — Workbench preview gallery</title>"
  echo "<body style='background:#161616;color:#f4f4f4;font-family:sans-serif;margin:24px'>"
  echo "<h1>Magic Mesh — Workbench preview gallery</h1>"
  echo "<p style='color:#8d8d8d'>OBS-4 visual-regression contact sheet. Captured headlessly (sway + grim, software render). Each tile is one Workbench surface; eyeball against the IBM Carbon reference (Gray 100 dark).</p>"
} > "$INDEX"

ok=0
fail=0
for entry in "${VIEWS[@]}"; do
  IFS='|' read -r slug fname label <<< "$entry"
  png="$OUT_DIR/$fname.png"
  echo "preview-gallery: capturing '$label' (slug='${slug:-<home>}')…" >&2
  if "$CAPTURE" "$slug" "$png" >&2; then
    ok=$((ok + 1))
    {
      echo "<figure style='display:inline-block;margin:12px;vertical-align:top'>"
      echo "<img src='$fname.png' width='560' style='border:1px solid #393939;display:block'>"
      echo "<figcaption style='color:#c6c6c6;margin-top:6px'>$label<br><code style='color:#6f6f6f'>${slug:-&lt;home&gt;}</code></figcaption>"
      echo "</figure>"
    } >> "$INDEX"
  else
    fail=$((fail + 1))
    {
      echo "<figure style='display:inline-block;margin:12px;vertical-align:top;width:560px;height:360px;border:1px dashed #fa4d56'>"
      echo "<figcaption style='color:#fa4d56;padding:12px'>capture FAILED — $label<br><code>${slug:-&lt;home&gt;}</code></figcaption>"
      echo "</figure>"
    } >> "$INDEX"
  fi
done

echo "</body>" >> "$INDEX"
echo "preview-gallery: $ok captured, $fail failed → $OUT_DIR (index: $INDEX)"

# Succeed if anything rendered; total failure means the render path is broken.
[ "$ok" -gt 0 ]
