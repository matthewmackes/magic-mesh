#!/bin/sh
# Image-owned Browser VM runtime. Host input is identity-only and is validated
# before any compositor or Chromium process starts.
set -eu

/usr/local/libexec/mcnf-browser-vm-validate
runtime_dir=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}
install -d -o "$(id -u)" -g "$(id -g)" -m 0700 "$runtime_dir"
export XDG_RUNTIME_DIR="$runtime_dir"
# QXL/SPICE is the compatibility display path on nodes without an
# OpenGL-enabled QEMU display backend. Pixman keeps the guest compositor
# usable there instead of failing during EGL renderer setup.
export WLR_RENDERER=pixman
export WLR_NO_HARDWARE_CURSORS=1
chromium_bin=$(command -v chromium || command -v chromium-browser)

mkdir -p "$HOME/.config/sway"
cat > "$HOME/.config/sway/config" <<'EOF'
default_border none
default_floating_border none
output * resolution 1920x1080
exec @CHROMIUM_BIN@ --ozone-platform=wayland --enable-features=UseOzonePlatform --start-maximized --no-first-run --disable-session-crashed-bubble --user-data-dir=/var/lib/mcnf-browser/chromium
EOF

# Chromium and the compositor are guest-owned. No URL, command, path, or
# browser state is accepted from the host declaration.
sed -i "s#@CHROMIUM_BIN@#$chromium_bin#" "$HOME/.config/sway/config"
exec /usr/bin/dbus-run-session -- /usr/bin/sway --unsupported-gpu --config "$HOME/.config/sway/config"
