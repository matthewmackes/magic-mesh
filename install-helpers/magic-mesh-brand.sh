#!/bin/bash
# magic-mesh-brand.sh — BRAND epic: apply the Carbon system-wide branding.
#
# One-way (no revert by design, operator lock 2026-06-16). Auto-run once on
# first boot of a WORKSTATION-role node by magic-mesh-brand.service; also
# runnable by hand: `magic-mesh-brand apply`.
#
# Carbon tokens (single-sourced from crates/shared/mde-theme/src/carbon.rs):
#   Gray 100 #161616 (background) · Gray 90 #262626 (layer) · Gray 10 #f4f4f4
#   (text) · Blue 60 #0f62fe (accent).
#
# Idempotent: every step is safe to re-run. Steps degrade to a logged warning
# (never a hard fail) when an optional tool/greeter is absent, so one missing
# piece never blocks the rest of the branding.
set -u

# ── asset + token config ────────────────────────────────────────────────────
BRAND_ASSETS="${BRAND_ASSETS:-/usr/share/magic-mesh/brand}"   # RPM install path
ICONS_ZIP="${BRAND_ASSETS}/mackes-carbon-icons.tar.xz"
LAYOUT_SRC="${BRAND_ASSETS}/cosmic-layout"
ICON_THEME="Mackes-Carbon"            # the dark variant is the system default
GTK_DARK="Adwaita-dark"               # Carbon-aligned dark base for GTK widgets
CARBON_BG="#161616"; CARBON_LAYER="#262626"; CARBON_TEXT="#f4f4f4"; CARBON_ACCENT="#0f62fe"
STAMP="/var/lib/mde/branded"

log() { echo "magic-mesh-brand: $*"; }
warn() { echo "magic-mesh-brand: WARN $*" >&2; }
have() { command -v "$1" >/dev/null 2>&1; }

# Resolve the desktop (uid-1000) user whose session gets the branding. The
# system service runs as root; per-user gsettings/config must target this user.
desktop_user() {
    # Prefer an active graphical session owner, else the lowest regular uid.
    local u
    u=$(loginctl list-users --no-legend 2>/dev/null | awk '$1>=1000 && $1<60000 {print $2; exit}')
    [ -z "$u" ] && u=$(awk -F: '$3>=1000 && $3<60000 {print $1; exit}' /etc/passwd)
    printf '%s' "$u"
}
USER_NAME="$(desktop_user)"
USER_HOME="$(getent passwd "$USER_NAME" 2>/dev/null | cut -d: -f6)"

# Run a command as the desktop user with a usable D-Bus session (for gsettings).
as_user() {
    local uid; uid="$(id -u "$USER_NAME" 2>/dev/null)"
    sudo -u "$USER_NAME" \
        DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/${uid}/bus" \
        XDG_RUNTIME_DIR="/run/user/${uid}" "$@"
}

# ── 1. icons (BRAND-2) ──────────────────────────────────────────────────────
brand_icons() {
    [ -f "$ICONS_ZIP" ] || { warn "icon set $ICONS_ZIP missing — skipping icons"; return; }
    have tar || { warn "tar missing — skipping icons"; return; }
    log "installing Carbon icon themes → /usr/share/icons"
    mkdir -p /usr/share/icons/
    tar -xJf "$ICONS_ZIP" -C /usr/share/icons/
    for t in Mackes-Carbon Mackes-Carbon-Light; do
        [ -d "/usr/share/icons/$t" ] && have gtk-update-icon-cache && \
            gtk-update-icon-cache -qtf "/usr/share/icons/$t" 2>/dev/null || true
    done
}

# ── 2. GTK 3/4 (BRAND-3) ────────────────────────────────────────────────────
brand_gtk() {
    log "applying GTK Carbon (dark + ${ICON_THEME} icons + Blue 60 accent)"
    for d in /etc/gtk-3.0 /etc/gtk-4.0; do
        mkdir -p "$d"
        cat > "$d/settings.ini" <<EOF
[Settings]
gtk-application-prefer-dark-theme=1
gtk-theme-name=${GTK_DARK}
gtk-icon-theme-name=${ICON_THEME}
EOF
    done
    # System-wide dconf default — applies on the user's FIRST login (the
    # first-boot service runs before anyone is logged in, so a live `gsettings`
    # set has no session to write to). A per-user override still wins later.
    if have dconf; then
        mkdir -p /etc/dconf/db/local.d /etc/dconf/profile
        [ -f /etc/dconf/profile/user ] || printf 'user-db:user\nsystem-db:local\n' > /etc/dconf/profile/user
        cat > /etc/dconf/db/local.d/10-mde-carbon <<EOF
[org/gnome/desktop/interface]
icon-theme='${ICON_THEME}'
gtk-theme='${GTK_DARK}'
color-scheme='prefer-dark'
EOF
        dconf update 2>/dev/null || true
    fi
    # Also apply live to an already-logged-in session (best-effort).
    if [ -n "$USER_NAME" ] && have gsettings; then
        as_user gsettings set org.gnome.desktop.interface color-scheme 'prefer-dark' 2>/dev/null || true
        as_user gsettings set org.gnome.desktop.interface gtk-theme "$GTK_DARK" 2>/dev/null || true
        as_user gsettings set org.gnome.desktop.interface icon-theme "$ICON_THEME" 2>/dev/null || true
    fi
}

# ── 3. Qt 5/6 (BRAND-4) — follow the GTK theme + Carbon icons ────────────────
brand_qt() {
    log "routing Qt through the GTK theme (QT_QPA_PLATFORMTHEME=gtk3)"
    mkdir -p /etc/environment.d
    cat > /etc/environment.d/95-mde-qt.conf <<EOF
# BRAND — Qt apps inherit the Carbon GTK theme + icon set.
QT_QPA_PLATFORMTHEME=gtk3
EOF
}

# ── 4. Cosmic (BRAND-5) + 8. layout seed (BRAND-9) ──────────────────────────
brand_cosmic() {
    [ -n "$USER_HOME" ] || { warn "no desktop-user home — skipping Cosmic"; return; }
    local cfg="$USER_HOME/.config/cosmic"
    # Seed the baked .13 layout once (one-way; don't clobber a later user tweak).
    if [ -d "$LAYOUT_SRC" ] && [ ! -e "$cfg/.mde-branded" ]; then
        log "seeding Cosmic layout from the .13 template"
        mkdir -p "$cfg"
        cp -an "$LAYOUT_SRC/." "$cfg/" 2>/dev/null || cp -rn "$LAYOUT_SRC/." "$cfg/" 2>/dev/null || true
        : > "$cfg/.mde-branded"
    fi
    # Force dark mode (Carbon is the dark Gray-100 palette).
    mkdir -p "$cfg/com.system76.CosmicTheme.Mode/v1"
    printf 'true' > "$cfg/com.system76.CosmicTheme.Mode/v1/is_dark"
    chown -R "$USER_NAME":"$USER_NAME" "$cfg" 2>/dev/null || true
}

# ── 5. Plymouth boot splash (BRAND-6) ───────────────────────────────────────
brand_plymouth() {
    have plymouth-set-default-theme || { warn "plymouth absent — skipping splash"; return; }
    local dir=/usr/share/plymouth/themes/mackes-carbon
    log "installing Carbon Plymouth theme"
    mkdir -p "$dir"
    # Spinner-based theme: Gray-100 background + the Mackes monogram watermark.
    [ -f "$BRAND_ASSETS/monogram.png" ] && cp -f "$BRAND_ASSETS/monogram.png" "$dir/watermark.png"
    cat > "$dir/mackes-carbon.plymouth" <<EOF
[Plymouth Theme]
Name=Mackes Carbon
Description=MCNF — IBM Carbon boot splash
ModuleName=two-step

[two-step]
Font=Sans 12
TitleFont=Sans 24
ImageDir=$dir
DialogHorizontalAlignment=.5
DialogVerticalAlignment=.5
HorizontalAlignment=.5
VerticalAlignment=.5
WatermarkHorizontalAlignment=.5
WatermarkVerticalAlignment=.5
Transition=none
BackgroundStartColor=0x161616
BackgroundEndColor=0x161616
EOF
    # Fall back to spinner assets for the animation frames if present.
    if [ -d /usr/share/plymouth/themes/spinner ]; then
        cp -n /usr/share/plymouth/themes/spinner/*.png "$dir/" 2>/dev/null || true
    fi
    plymouth-set-default-theme -R mackes-carbon 2>/dev/null \
        || warn "plymouth-set-default-theme failed (initramfs rebuild needs root)"
}

# ── 6. LightDM greeter (BRAND-7) ────────────────────────────────────────────
brand_lightdm() {
    [ -d /etc/lightdm ] || { warn "lightdm absent — skipping greeter"; return; }
    local hero="$BRAND_ASSETS/greeter-hero.png"
    [ -f "$hero" ] && cp -f "$hero" /usr/share/backgrounds/mde-greeter.png 2>/dev/null
    local bg="/usr/share/backgrounds/mde-greeter.png"
    [ -f "$bg" ] || bg="$CARBON_BG"
    if rpm -q slick-greeter >/dev/null 2>&1 || [ -f /usr/share/xgreeters/slick-greeter.desktop ]; then
        log "theming LightDM (slick-greeter)"
        cat > /etc/lightdm/slick-greeter.conf <<EOF
[Greeter]
background=${bg}
background-color=${CARBON_BG}
theme-name=${GTK_DARK}
icon-theme-name=${ICON_THEME}
draw-user-backgrounds=false
EOF
    else
        log "theming LightDM (gtk-greeter)"
        cat > /etc/lightdm/lightdm-gtk-greeter.conf <<EOF
[greeter]
background=${bg}
theme-name=${GTK_DARK}
icon-theme-name=${ICON_THEME}
EOF
    fi
}

# ── 7. default file manager + applet (BRAND-8) ──────────────────────────────
brand_default_apps() {
    log "setting mde-files as the default file manager"
    # System-wide MIME default for directories.
    if have xdg-mime; then
        as_user xdg-mime default org.magicmesh.Files.desktop inode/directory 2>/dev/null || true
    fi
    mkdir -p /etc/xdg
    # Belt-and-suspenders system default.
    grep -q "inode/directory=org.magicmesh.Files.desktop" /etc/xdg/mimeapps.list 2>/dev/null || {
        printf '[Default Applications]\ninode/directory=org.magicmesh.Files.desktop\n' >> /etc/xdg/mimeapps.list
    }
    # The notification-applet swap is baked into the seeded layout
    # (CosmicAppletNotifications removed; com.mackes.MagicMeshApplet kept).
}

apply() {
    [ "$(id -u)" -eq 0 ] || { echo "magic-mesh-brand apply must run as root" >&2; exit 1; }
    log "applying Carbon branding (desktop user: ${USER_NAME:-none})"
    brand_icons
    brand_gtk
    brand_qt
    brand_cosmic
    brand_plymouth
    brand_lightdm
    brand_default_apps
    mkdir -p "$(dirname "$STAMP")"; : > "$STAMP"
    log "branding complete (stamp: $STAMP)"
}

case "${1:-apply}" in
    apply) apply ;;
    *) echo "usage: magic-mesh-brand apply" >&2; exit 2 ;;
esac
