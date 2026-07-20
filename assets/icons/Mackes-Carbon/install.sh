#!/bin/sh
# Mackes-Carbon icon theme installer (freedesktop.org Icon Theme Specification).
# Installs for GNOME / XFCE (and any GTK / Qt desktop honouring XDG icon paths).
#
#   ./install.sh            install for the current user (~/.local/share/icons)
#   sudo ./install.sh --system   install system-wide (/usr/share/icons)
#   ./install.sh --uninstall     remove a previous install (honours --system)
#
set -eu

THEME="Mackes-Carbon"
SELF_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

mode="user"
action="install"
for arg in "$@"; do
    case "$arg" in
        --system)    mode="system" ;;
        --user)      mode="user" ;;
        --uninstall) action="uninstall" ;;
        -h|--help)
            sed -n '2,9p' "$0" | sed 's/^# \{0,1\}//'
            exit 0 ;;
        *) echo "unknown option: $arg" >&2; exit 2 ;;
    esac
done

if [ "$mode" = "system" ]; then
    dest="/usr/share/icons"
else
    dest="${XDG_DATA_HOME:-$HOME/.local/share}/icons"
fi
target="$dest/$THEME"

if [ "$action" = "uninstall" ]; then
    if [ -d "$target" ]; then
        rm -rf "$target"
        echo "removed $target"
    else
        echo "nothing to remove at $target"
    fi
    exit 0
fi

mkdir -p "$dest"
# Copy the theme payload (everything except this installer + docs).
rm -rf "$target"
mkdir -p "$target"
cp "$SELF_DIR/index.theme" "$SELF_DIR/LICENSE" "$SELF_DIR/NOTICE" "$target/"
cp -r "$SELF_DIR/scalable" "$target/"
echo "installed theme to $target"

# Build the GTK icon cache so the theme is picked up without a logout.
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f -t "$target" >/dev/null 2>&1 \
        && echo "icon cache rebuilt" \
        || echo "icon cache rebuild skipped (non-fatal)"
fi

cat <<EOF

Done. Select the theme:
  GNOME : gnome-tweaks  ->  Appearance  ->  Icons  ->  $THEME
          (or)  gsettings set org.gnome.desktop.interface icon-theme '$THEME'
  XFCE  : Settings  ->  Appearance  ->  Icons  ->  $THEME
          (or)  xfconf-query -c xsettings -p /Net/IconThemeName -s '$THEME'
EOF
