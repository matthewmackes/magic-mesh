#!/bin/sh
# MESH-LAYOUT (operator 2026-06-19) — enforce the canonical Cosmic panel/desktop
# layout on every mesh desktop, every session. The operator locked .13's current
# panel layout (the MagicMeshApps launcher + AppList on the left; the bell +
# status applets on the right; top Panel, dark Carbon) as the stock default for
# ALL mesh desktops and wants it ENFORCED (drift-resistant), not just seeded once.
#
# Runs at COSMIC session start (an /etc/xdg/autostart entry) as the logging-in
# user: it copies the layout-defining config from the system default into
# ~/.config/cosmic and, if anything had drifted, restarts cosmic-panel so the
# enforced layout takes effect. A clean (already-canonical) login does nothing.
#
# Only the *structural layout* is enforced — NOT per-applet state (volume, time
# format) or the wallpaper (BRAND/mde-mesh-wallpaper own that).
set -u

SRC="${MDE_LAYOUT_SRC:-/usr/share/magic-mesh/brand/cosmic-layout}"
CFG="${XDG_CONFIG_HOME:-$HOME/.config}/cosmic"
[ -d "$SRC" ] || exit 0

# The layout-defining config dirs (panel/dock/applet placement + compositor).
# APPLAUNCH-9 — CosmicPanelButton (the Start button now launches the Front Door
# launcher in place of the retired mde-apps-applet) + CosmicSettings.Shortcuts
# (the Super key → the Front Door launcher) join the enforced set.
LAYOUT_DIRS="com.system76.CosmicPanel com.system76.CosmicPanel.Panel com.system76.CosmicPanel.Dock com.system76.CosmicPanelButton com.system76.CosmicSettings.Shortcuts com.system76.CosmicAppList com.system76.CosmicComp"

changed=0
for d in $LAYOUT_DIRS; do
    [ -d "$SRC/$d" ] || continue
    if ! diff -rq "$SRC/$d" "$CFG/$d" >/dev/null 2>&1; then
        changed=1
    fi
    mkdir -p "$CFG/$d"
    cp -a "$SRC/$d/." "$CFG/$d/" 2>/dev/null || true
done

# Carbon is the dark Gray-100 palette — keep dark mode enforced too.
mkdir -p "$CFG/com.system76.CosmicTheme.Mode/v1"
printf 'true' > "$CFG/com.system76.CosmicTheme.Mode/v1/is_dark" 2>/dev/null || true

# If the layout had drifted, restart the panel so cosmic-session respawns it
# with the enforced config. (No-op on a clean login — no flash.)
if [ "$changed" = 1 ]; then
    pkill -f cosmic-panel 2>/dev/null || true
fi
exit 0
