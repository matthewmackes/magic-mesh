#!/bin/sh
# BRAND-11 (operator 2026-06-19) — seed the MCNF 11 branded image as the default
# Cosmic desktop wallpaper. Runs at COSMIC session start (an /etc/xdg/autostart
# entry, Workstation-rank gated) as the logging-in user.
#
# We CANNOT ship the cosmic-bg system default (/usr/share/cosmic/.../all is owned
# by the cosmic-bg RPM — a file conflict), so we seed the per-user config instead.
# Seed-once-if-absent: if the user already has a CosmicBackground config we leave
# it alone (never stomp a wallpaper the user chose). The image itself ships to
# /usr/share/backgrounds/mcnf-11-winter.png.
set -u

WALL="${MDE_WALLPAPER:-/usr/share/backgrounds/mcnf-11-winter.png}"
[ -f "$WALL" ] || exit 0

CFG="${XDG_CONFIG_HOME:-$HOME/.config}/cosmic/com.system76.CosmicBackground/v1"

# Already configured by the user (or a prior seed) — do nothing.
[ -f "$CFG/all" ] && exit 0

mkdir -p "$CFG" || exit 0

# RON shape matches cosmic-bg's own system default (verified against
# /usr/share/cosmic/com.system76.CosmicBackground/v1/all). filter_by_theme=false
# + rotation_frequency=0 pin the single branded image.
cat > "$CFG/all" <<RON
(
    output: "all",
    source: Path("$WALL"),
    filter_by_theme: false,
    rotation_frequency: 0,
    filter_method: Lanczos,
    scaling_mode: Zoom,
    sampling_method: Alphanumeric,
)
RON
printf '[All]' > "$CFG/backgrounds"
printf 'true' > "$CFG/same-on-all"

# Nudge cosmic-bg to apply it this session (it live-reloads cosmic-config, but a
# respawn guarantees the seed shows on first login). cosmic-session restarts it.
pkill -x cosmic-bg 2>/dev/null || true
exit 0
