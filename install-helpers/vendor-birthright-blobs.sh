#!/bin/bash
# vendor-birthright-blobs.sh — BIRTHRIGHT-2: stage the bundled, air-gapped
# first-boot birthright blobs for the RPM.
#
# The out-of-repo birthrights (ntfy, starship) are provisioned at first boot by
# the mesh-install-* helpers, which prefer a BUNDLED tarball under
# /usr/share/magic-mesh/vendor/ so an offline/air-gapped install still works.
# Those tarballs are NOT committed to git (third-party binaries, ~18 MB) — they
# are fetched + sha256-verified here at BUILD time into vendor/birthright/, which
# the generate-rpm `assets` array ships into the RPM.
#
# Pins (VER / SHA256 / ASSET / URL) are the SINGLE SOURCE OF TRUTH in the
# install scripts; this script parses them out so the bundle can never drift
# from what the installer verifies. Run before `cargo generate-rpm`
# (build-rpm-fedora43.sh calls it automatically). Idempotent: a present,
# checksum-valid blob is left untouched.
#
# lizardfs / lizardfs-adm are deliberately NOT vendored here — they are the
# BIRTHRIGHT-1 substrate-provisioning epic (held), documented in
# docs/design/birthrights.md.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$REPO/vendor/birthright"
mkdir -p "$OUT"
log() { echo "vendor-birthright: $*"; }

# Parse a shell var assignment (VER="…") out of an install script.
pin() { sed -n "s/^$2=\"\?\([^\"]*\)\"\?.*/\1/p" "$REPO/install-helpers/$1" | head -1; }

stage() {
  local script="$1" asset sha url dest
  asset="$(pin "$script" ASSET)"
  sha="$(pin "$script" SHA256)"
  url="$(pin "$script" URL)"
  # Expand ${VER}/${ASSET} that the URL/ASSET pins interpolate in the script.
  local ver; ver="$(pin "$script" VER)"
  asset="${asset//\$\{VER\}/$ver}"; asset="${asset//\$VER/$ver}"
  url="${url//\$\{VER\}/$ver}"; url="${url//\$\{ASSET\}/$asset}"
  dest="$OUT/$asset"
  if [ -f "$dest" ] && echo "${sha}  $dest" | sha256sum -c - >/dev/null 2>&1; then
    log "$asset already staged + verified"; return
  fi
  log "fetching $asset"
  curl -fsSL "$url" -o "$dest.tmp"
  echo "${sha}  $dest.tmp" | sha256sum -c - >/dev/null 2>&1 \
    || { echo "vendor-birthright: SHA256 MISMATCH for $asset — refusing" >&2; rm -f "$dest.tmp"; exit 1; }
  mv "$dest.tmp" "$dest"
  log "staged $dest"
}

stage mesh-install-ntfy.sh
stage mesh-install-starship.sh
log "done — bundled birthright blobs in $OUT"
ls -la "$OUT"
