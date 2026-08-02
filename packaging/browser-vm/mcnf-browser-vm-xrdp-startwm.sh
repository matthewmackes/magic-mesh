#!/bin/sh
# xrdp's user-session entrypoint. xrdp supplies DISPLAY and XAUTHORITY; the
# guest-owned runtime still owns Sway, Chromium, PipeWire, and all browser UI.
set -eu

export WLR_BACKENDS=x11
if [ -z "${WLR_RENDERER:-}" ]; then
    render_node=
    for node in /dev/dri/renderD*; do
        if [ -e "$node" ]; then
            render_node=$node
            break
        fi
    done
    if [ -n "$render_node" ]; then
        export WLR_RENDERER=gles2
    else
        export WLR_RENDERER=pixman
    fi
fi
export WLR_NO_HARDWARE_CURSORS=1

exec /usr/local/libexec/mcnf-browser-vm-runtime
