#!/usr/bin/env bash
# lint-style-leaks.sh — the /polish mechanical gate (E12 egui era).
#
# The shared `mde_egui::Style`/`Motion`/`Fonts` are the ONLY source of look
# (AI_GOVERNANCE §4). A surface crate that mints a raw `Color32::from_*(...)`
# or a literal animation duration is a style-leak regression, not an
# improvement. Raw egui hover text is also forbidden; hover labels must route
# through themed tooltip/menu primitives so dark/light text remains readable.
# This script is the mechanical gate: ZERO hits required in crates/desktop for
# look leaks, and ZERO raw hover-text hits in crates/desktop or crates/shared.
#
# DATA-not-look exclusions (these draw pixel/ANSI *data*, never UI chrome):
#   - the VDI protocol decoders: mde-vdi-{rdp,spice,vnc}
#   - the shared VDI pixel core:  mde-vdi-core/src/pixel.rs  (framebuffer bytes)
#   - the terminal colour tables: mde-term-egui/src/{palette,presets}.rs
#   - CEF verifier pixel fixtures: mde-web-preview-client/src/bin/cef-verify.rs
# Browser workspace exception (AI_GOVERNANCE §4): Browser chrome owns a local
# Chrome/Material palette in mde-shell-egui/src/web/chrome_ui/mod.rs.
# Map-content palettes can be marked line-by-line with
# `style-leak-ok: map-content-color`; unmarked UI chrome literals still fail.
#
# Usage: install-helpers/lint-style-leaks.sh   (run from repo root; exit != 0 on any leak)
set -euo pipefail

cd "$(dirname "$0")/.."

DATA_EXCLUDE='mde-vdi-(rdp|spice|vnc)/|mde-vdi-core/src/pixel\.rs|mde-term-egui/src/(palette|presets)\.rs'
COLOUR_EXCLUDE="${DATA_EXCLUDE}|mde-web-preview-client/src/bin/cef-verify\.rs|mde-shell-egui/src/web/chrome_ui/mod\.rs"

# 1) hardcoded colours minted outside mde-egui
colour_hits="$(grep -rnE 'Color32::from_(rgb|rgba|gray|black_alpha|white_alpha)' \
  crates/desktop --include='*.rs' | grep -vE "$COLOUR_EXCLUDE" | grep -v 'style-leak-ok: map-content-color' || true)"

# 2) bespoke animation durations (a literal float second in animate_bool_with_time)
motion_hits="$(grep -rnE 'animate_bool_with_time\([^)]*[0-9]\.[0-9]' \
  crates/desktop --include='*.rs' | grep -vE "$DATA_EXCLUDE" || true)"

# 3) raw egui hover text outside themed helpers
hover_hits="$(grep -rnE '\.on_(disabled_)?hover_text\(' \
  crates/desktop crates/shared --include='*.rs' | grep -vE "$DATA_EXCLUDE" || true)"

n_colour="$(printf '%s' "$colour_hits" | grep -c . || true)"
n_motion="$(printf '%s' "$motion_hits" | grep -c . || true)"
n_hover="$(printf '%s' "$hover_hits" | grep -c . || true)"

if [ "$n_colour" -eq 0 ] && [ "$n_motion" -eq 0 ] && [ "$n_hover" -eq 0 ]; then
  echo "[OK] style-leak gate: 0 leaks in crates/desktop (look reads only from mde-egui)."
  exit 0
fi

echo "[FAIL] style-leak gate: ${n_colour} colour leak(s) + ${n_motion} duration leak(s) + ${n_hover} raw hover-text leak(s)." >&2
echo "       Move the value into crates/shared/mde-egui (Style/Motion) with a backing test, then consume it." >&2
[ "$n_colour" -gt 0 ] && { echo "--- colour leaks ---" >&2; printf '%s\n' "$colour_hits" >&2; }
[ "$n_motion" -gt 0 ] && { echo "--- duration leaks ---" >&2; printf '%s\n' "$motion_hits" >&2; }
[ "$n_hover" -gt 0 ] && { echo "--- raw hover-text leaks ---" >&2; printf '%s\n' "$hover_hits" >&2; }
exit 1
