#!/bin/bash
# MESHSHELL/BUS — fetch + verify the pinned ntfy binary (the cross-node
# notification broker). ntfy isn't in the Fedora repos, so it can't be an RPM
# birthright; this installs the upstream static release, sha256-verified, to
# /usr/bin/ntfy. Once present (+ the template at /usr/share/mde/ntfy), the
# mde-bus daemon starts the per-peer broker on its next eval cycle, so mesh-wide
# notification distribution begins WITHOUT a restart. Idempotent + one-way.
set -u
VER="v2.24.0"
SHA256="4789b38c1c068ef849f95645df4dcb100a7a05f94b29b3cff85153ff4d3b29bb"
ASSET="ntfy_2.24.0_linux_amd64.tar.gz"
URL="https://github.com/binwiederhier/ntfy/releases/download/${VER}/${ASSET}"
DEST=/usr/bin/ntfy
log(){ echo "mesh-install-ntfy: $*"; }

if command -v ntfy >/dev/null 2>&1 && ntfy --version 2>/dev/null | grep -q "${VER#v}"; then
  log "ntfy ${VER} already present"; exit 0
fi
command -v curl >/dev/null || { log "curl missing — skipping"; exit 0; }
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
log "fetching ntfy ${VER}"
curl -fsSL "$URL" -o "$TMP/$ASSET" || { log "download failed (will retry next boot)"; exit 0; }
echo "${SHA256}  $TMP/$ASSET" | sha256sum -c - >/dev/null 2>&1 \
  || { log "SHA256 MISMATCH — refusing to install"; exit 1; }
tar -xzf "$TMP/$ASSET" -C "$TMP" || { log "extract failed"; exit 0; }
bin="$(find "$TMP" -type f -name ntfy | head -1)"
[ -n "$bin" ] || { log "ntfy binary not found in tarball"; exit 0; }
install -m755 "$bin" "$DEST" && log "installed ntfy ${VER} -> $DEST"
