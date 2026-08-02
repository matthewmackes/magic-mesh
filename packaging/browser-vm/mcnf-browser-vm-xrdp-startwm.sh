#!/bin/sh
# xrdp's user-session entrypoint. xrdp supplies DISPLAY and XAUTHORITY; the
# guest-owned runtime still owns Sway, Chromium, PipeWire, and all browser UI.
set -eu

export WLR_BACKENDS=x11
export WLR_RENDERER=pixman
export WLR_NO_HARDWARE_CURSORS=1

exec /usr/local/libexec/mcnf-browser-vm-runtime
