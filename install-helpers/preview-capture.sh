#!/usr/bin/env bash
# preview-capture.sh — headless /preview capture for the iced Workbench.
#
# Renders mde-workbench under a HEADLESS wlroots compositor (sway with
# WLR_BACKENDS=headless) and screenshots the output with grim — so the
# Carbon-look /preview verify (AI_GOVERNANCE §4) runs on a tty box with no
# physical display. grim needs the wlr-screencopy protocol, which sway/
# cosmic-comp provide (weston does not), hence sway-headless here.
#
# Usage:  preview-capture.sh [deep-link-slug] [out.png]
#   e.g.  preview-capture.sh fleet.hardware /tmp/hardware.png
#         preview-capture.sh maintain.audit /tmp/audit.png
#         preview-capture.sh '' /tmp/home.png      # default view
#
# Exits 0 + writes the PNG on success; non-zero if render/capture failed.
set -u

SLUG="${1:-}"
OUT="${2:-/tmp/mde-preview.png}"
BIN="${MDE_WORKBENCH_BIN:-$PWD/target/debug/mde-workbench}"
RES="${MDE_PREVIEW_RES:-1400x900}"

if [ ! -x "$BIN" ]; then
  echo "preview-capture: $BIN not found — build it (cargo build -p mde-workbench)" >&2
  exit 2
fi
for t in sway grim; do
  command -v "$t" >/dev/null 2>&1 || { echo "preview-capture: missing $t" >&2; exit 2; }
done

RT="$(mktemp -d)"
chmod 700 "$RT"
export XDG_RUNTIME_DIR="$RT"
export WLR_BACKENDS=headless
export WLR_LIBINPUT_NO_DEVICES=1
export WLR_RENDERER=pixman          # software render — no GPU on a headless box
export LIBGL_ALWAYS_SOFTWARE=1

FOCUS=""
[ -n "$SLUG" ] && FOCUS="--focus $SLUG"

# A capture helper sway execs: wait for the app to draw, grab the headless
# output, then tell sway to exit so the script returns.
cat > "$RT/capture.sh" <<CAP
#!/usr/bin/env bash
sleep "${MDE_PREVIEW_DELAY:-5}"
grim "$OUT" 2>>"$RT/grim.log" || grim -o HEADLESS-1 "$OUT" 2>>"$RT/grim.log"
swaymsg exit >/dev/null 2>&1
CAP
chmod +x "$RT/capture.sh"

cat > "$RT/sway.cfg" <<CFG
output HEADLESS-1 mode $RES
default_border none
exec $BIN $FOCUS
exec $RT/capture.sh
CFG

timeout 40 sway -c "$RT/sway.cfg" >>"$RT/sway.log" 2>&1
rc=$?

if [ -s "$OUT" ]; then
  echo "preview-capture: wrote $OUT ($(stat -c%s "$OUT") bytes, slug='${SLUG:-<home>}')"
  exit 0
fi
echo "preview-capture: no image captured (rc=$rc). logs in $RT:" >&2
tail -n 8 "$RT/sway.log" "$RT/grim.log" 2>/dev/null >&2
exit 1
