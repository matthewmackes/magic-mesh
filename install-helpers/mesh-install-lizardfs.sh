#!/bin/bash
# mesh-install-lizardfs.sh — BIRTHRIGHT-1: install the LizardFS binaries for a
# role, so a fresh `found`/`join` can stand up QNM-Shared (the §1 shared-state
# plane) with no manual step.
#
# LizardFS is NOT in the Fedora base repos on F44 (it IS on F43), so it can't be
# a hard RPM Requires. This mirrors the BIRTHRIGHT-2 provisioning ladder:
#   1. already installed?           -> done
#   2. `dnf install`                -> covers F43 lighthouses/servers
#   3. bundled fc43 RPMs (offline)  -> /usr/share/magic-mesh/vendor/lizardfs/*.rpm
#   4. fetch from a pinned manifest -> /usr/share/mde/lizardfs/rpms.manifest
#   5. none of the above            -> LOUD warning, exit 0 (non-fatal)
#
# Non-fatal by design: a node that can't get LizardFS should still join the
# overlay; the daemon's startup mount-assert + the mesh-health watchdog surface
# the degraded shared-state plane loudly so it's never a silent no-op.
#
# Usage: mesh-install-lizardfs.sh <lighthouse|server|workstation>
set -u

ROLE="${1:-workstation}"
log()  { echo "mesh-install-lizardfs: $*"; }
warn() { echo "mesh-install-lizardfs: WARN $*" >&2; }
have() { command -v "$1" >/dev/null 2>&1; }

# Package set + the binaries that prove each is installed, by role.
# Lighthouse: master + chunkserver + client + admin. Server: chunkserver +
# client + admin. Workstation: client + admin.
case "$ROLE" in
  lighthouse) PKGS="lizardfs-master lizardfs-chunkserver lizardfs-client lizardfs-adm"
              NEEDED="mfsmaster mfschunkserver mfsmount lizardfs-admin" ;;
  server)     PKGS="lizardfs-chunkserver lizardfs-client lizardfs-adm"
              NEEDED="mfschunkserver mfsmount lizardfs-admin" ;;
  *)          PKGS="lizardfs-client lizardfs-adm"
              NEEDED="mfsmount lizardfs-admin" ;;
esac

have_all() { for b in $NEEDED; do have "$b" || return 1; done; return 0; }

# 1. Already present?
if have_all; then log "LizardFS binaries already present for role $ROLE"; exit 0; fi

# 2. dnf (F43 lighthouses/servers; a no-op miss on F44).
if have dnf; then
  log "installing via dnf: $PKGS"
  dnf install -y --setopt=install_weak_deps=False $PKGS >/dev/null 2>&1 || true
  if have_all; then log "installed via dnf"; exit 0; fi
fi

# 3. Bundled fc43 RPMs (air-gapped/offline — staged into the magic-mesh RPM by
#    install-helpers/vendor-lizardfs-rpms.sh at build time). The fc43 binaries
#    run on F44 unchanged; --nodeps because the F44 dep graph differs.
VENDOR=/usr/share/magic-mesh/vendor/lizardfs
if [ -d "$VENDOR" ] && ls "$VENDOR"/*.rpm >/dev/null 2>&1; then
  log "installing bundled fc43 RPMs from $VENDOR"
  rpm -Uvh --replacepkgs --nodeps "$VENDOR"/*.rpm >/dev/null 2>&1 || true
  if have_all; then log "installed from the bundled RPMs (offline)"; exit 0; fi
fi

# 4. Fetch from a pinned manifest (URL<TAB>SHA256 per line), checksum-verified.
MANIFEST=/usr/share/mde/lizardfs/rpms.manifest
if [ -f "$MANIFEST" ] && have curl; then
  TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
  log "fetching LizardFS RPMs from the pinned manifest"
  ok=1
  while IFS=$'\t' read -r url sha; do
    [ -z "$url" ] && continue
    case "$url" in \#*) continue;; esac
    f="$TMP/$(basename "$url")"
    curl -fsSL "$url" -o "$f" || { warn "download failed: $url"; ok=0; break; }
    echo "${sha}  $f" | sha256sum -c - >/dev/null 2>&1 \
      || { warn "SHA256 MISMATCH: $url — refusing"; ok=0; break; }
  done < "$MANIFEST"
  if [ "$ok" = 1 ] && ls "$TMP"/*.rpm >/dev/null 2>&1; then
    rpm -Uvh --replacepkgs --nodeps "$TMP"/*.rpm >/dev/null 2>&1 || true
    if have_all; then log "installed from the pinned manifest"; exit 0; fi
  fi
fi

# 5. Could not provision — loud, but non-fatal (overlay join still succeeds).
warn "could NOT install LizardFS for role $ROLE — the shared-state plane (QNM-Shared)"
warn "will be DOWN until LizardFS is installed. On F44, stage the fc43 RPMs at"
warn "$VENDOR (rebuild the RPM with vendor-lizardfs-rpms.sh) or add the F43 repo."
warn "Missing: $(for b in $NEEDED; do have "$b" || printf '%s ' "$b"; done)"
exit 0
