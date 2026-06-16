#!/bin/bash
# MESHSHELL SHELL-2 — fetch + verify the pinned starship binary.
# starship isn't in the Fedora repos; install the upstream static musl release,
# sha256-verified, to /usr/bin/starship. Idempotent + one-way. Soft-fails on
# network errors (the first-boot unit retries next boot); HARD-fails on a
# checksum mismatch (never install an unverified binary).
set -u
VER="v1.25.1"
SHA256="c6ddd3ecb9c0071a2ad38d98cee748160066b7c4f197421268058f4a5d6f8504"
ASSET="starship-x86_64-unknown-linux-musl.tar.gz"
URL="https://github.com/starship/starship/releases/download/${VER}/${ASSET}"
DEST=/usr/bin/starship
log(){ echo "mesh-install-starship: $*"; }

if command -v starship >/dev/null 2>&1 && starship --version 2>/dev/null | grep -q "${VER#v}"; then
  log "starship ${VER} already present"; exit 0
fi
command -v curl >/dev/null || { log "curl missing — skipping"; exit 0; }
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
log "fetching starship ${VER}"
curl -fsSL "$URL" -o "$TMP/$ASSET" || { log "download failed (will retry next boot)"; exit 0; }
echo "${SHA256}  $TMP/$ASSET" | sha256sum -c - >/dev/null 2>&1 \
  || { log "SHA256 MISMATCH — refusing to install"; exit 1; }
tar -xzf "$TMP/$ASSET" -C "$TMP" starship || { log "extract failed"; exit 0; }
install -m755 "$TMP/starship" "$DEST" && log "installed starship ${VER} -> $DEST"
