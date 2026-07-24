#!/usr/bin/env bash
# preview-capture.sh — headless Construct-shell capture.
#
# Renders the live mde-shell-egui client under a HEADLESS wlroots compositor (sway with
# WLR_BACKENDS=headless) and screenshots the output with grim — so the
# Construct render proof (AI_GOVERNANCE §4) runs on a farm/dev host with no
# physical display. grim needs the wlr-screencopy protocol, which sway provides
# (weston does not), hence sway-headless here.
#
# Usage:  preview-capture.sh [out.png]
#   e.g.  preview-capture.sh /tmp/construct-home.png
#
# Exits 0 + writes the PNG on success; non-zero if render/capture failed.
set -u

PROFILE="${MDE_PREVIEW_PROFILE:-construct}"

write_profile_fixture() {
  case "$PROFILE" in
    construct|car) ;;
    *)
      echo "preview-capture: MDE_PREVIEW_PROFILE must be construct or car (got '$PROFILE')" >&2
      return 2
      ;;
  esac
  mkdir -p "$MDE_BUS_ROOT"
  # client_data_dir() prefers a live /run/mde-bus unless MDE_BUS_ROOT is set.
  # Pinning this fixture is what makes the capture independent of operator state.
  printf '{"layout_profile":"%s"}\n' "$PROFILE" >"$MDE_BUS_ROOT/settings-appearance.json"
}

if [ "${1:-}" = "--self-test" ]; then
  set -e
  SELF_RT="$(mktemp -d)"
  trap 'rm -rf -- "$SELF_RT"' EXIT
  export MDE_BUS_ROOT="$SELF_RT/bus"
  write_profile_fixture
  [ -s "$MDE_BUS_ROOT/settings-appearance.json" ]
  grep -Fq '"layout_profile":"construct"' "$MDE_BUS_ROOT/settings-appearance.json"
  PROFILE=car
  write_profile_fixture
  grep -Fq '"layout_profile":"car"' "$MDE_BUS_ROOT/settings-appearance.json"
  echo "preview-capture: self-test passed (Bus pin + Construct/Car fixtures)"
  exit 0
fi

OUT="${1:-/tmp/mde-preview.png}"
BIN="${MDE_SHELL_BIN:-$PWD/target/debug/mde-shell-egui}"
RES="${MDE_PREVIEW_RES:-1400x900}"

if [ ! -x "$BIN" ]; then
  echo "preview-capture: $BIN not found — build it (cargo build -p mde-shell-egui)" >&2
  exit 2
fi
for t in sway grim; do
  command -v "$t" >/dev/null 2>&1 || { echo "preview-capture: missing $t" >&2; exit 2; }
done

RT="$(mktemp -d)"
chmod 700 "$RT"
export XDG_RUNTIME_DIR="$RT"
# Isolate the mde-bus data dir (mde_bus::default_data_dir = dirs::data_dir()
# /mde/bus, which follows XDG_DATA_HOME). A fresh, empty bus dir per capture
# prevents retained operator state from changing the initial shell frame.
export XDG_DATA_HOME="$RT/data"
export XDG_CONFIG_HOME="$RT/config"
mkdir -p "$XDG_DATA_HOME" "$XDG_CONFIG_HOME"
# mde_bus::client_data_dir() prefers a live system Bus over XDG_DATA_HOME. Pin
# it explicitly so the render cannot consume retained operator state or live
# mirrors from the host running this advisory proof.
export MDE_BUS_ROOT="$RT/bus"
write_profile_fixture
# AUD-5 — pre-satisfy the DISCLAIMER accept gate so the headless capture renders
# the actual panels, not the (real, operator-facing) first-run accept screen.
export MDE_DISCLAIMER_ACCEPTED=1
export WLR_BACKENDS=headless
export WLR_LIBINPUT_NO_DEVICES=1
export WLR_RENDERER=pixman          # software render — no GPU on a headless box
export LIBGL_ALWAYS_SOFTWARE=1

# Capture into this run's private path first. A pre-existing OUT must never make
# a failed compositor/grim run look successful.
CAPTURE_OUT="$RT/capture.png"

# A capture helper sway execs: wait for the app to draw, grab the headless
# output, then tell sway to exit so the script returns.
cat > "$RT/capture.sh" <<CAP
#!/usr/bin/env bash
sleep "${MDE_PREVIEW_DELAY:-5}"
grim "$CAPTURE_OUT" 2>>"$RT/grim.log" || grim -o HEADLESS-1 "$CAPTURE_OUT" 2>>"$RT/grim.log"
swaymsg exit >/dev/null 2>&1
CAP
chmod +x "$RT/capture.sh"

cat > "$RT/sway.cfg" <<CFG
output HEADLESS-1 mode $RES
default_border none
exec $BIN
exec $RT/capture.sh
CFG

timeout 40 sway -c "$RT/sway.cfg" >>"$RT/sway.log" 2>&1
rc=$?

is_png() {
  [ -s "$1" ] || return 1
  [ "$(od -An -tx1 -N8 "$1" | tr -d '[:space:]')" = "89504e470d0a1a0a" ]
}

if is_png "$CAPTURE_OUT"; then
  mv -f -- "$CAPTURE_OUT" "$OUT"
  echo "preview-capture: wrote $OUT ($(stat -c%s "$OUT") bytes)"
  exit 0
fi
echo "preview-capture: no valid PNG captured (rc=$rc). logs in $RT:" >&2
tail -n 8 "$RT/sway.log" "$RT/grim.log" 2>/dev/null >&2
exit 1
