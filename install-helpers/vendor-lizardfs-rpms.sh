#!/bin/bash
# vendor-lizardfs-rpms.sh — BIRTHRIGHT-1: stage the fc43 LizardFS RPM set so the
# magic-mesh RPM can bundle it for air-gapped / F44 first-boot provisioning.
#
# LizardFS is in the F43 repos but NOT F44; the fc43 binaries run on F44
# unchanged (installed --nodeps). This resolves + downloads the full RPM set
# (incl. deps) inside a fedora:43 container into vendor/birthright/lizardfs/,
# which the generate-rpm `assets` array ships to /usr/share/magic-mesh/vendor/
# lizardfs/ (mesh-install-lizardfs.sh installs them when dnf can't, step 3).
#
# Blobs are NOT committed to git (third-party RPMs) — produced at build time,
# like the BIRTHRIGHT-2 ntfy/starship blobs. Idempotent: skips when the dir
# already holds RPMs. build-rpm-fedora43.sh runs this before generate-rpm.
#
# Requires podman + network (build machine only; the install target is offline).
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$REPO/vendor/birthright/lizardfs"
FEDORA="${1:-43}"
IMAGE="registry.fedoraproject.org/fedora:${FEDORA}"
PKGS="lizardfs-master lizardfs-chunkserver lizardfs-client lizardfs-adm"
log() { echo "vendor-lizardfs: $*"; }

mkdir -p "$OUT"
if ls "$OUT"/*.rpm >/dev/null 2>&1; then
  log "RPMs already staged in $OUT — skipping (rm them to re-fetch)"
  ls -la "$OUT"; exit 0
fi
command -v podman >/dev/null || { echo "vendor-lizardfs: podman required" >&2; exit 1; }

# Download ONLY the lizardfs* family — NOT the base-OS dependency closure
# (F44 already has glibc/systemd/fuse-libs/…). They install with `rpm --nodeps`
# on F44, so the fc43 base deps are neither wanted nor needed; pulling them
# would bloat the RPM by ~70 MB and risk downgrading F44 base packages.
log "downloading the fc${FEDORA} LizardFS family (lizardfs*) — no base-OS deps"
podman run --rm --security-opt label=disable -v "$OUT:/out" "$IMAGE" bash -c "
  set -e
  dnf install -y 'dnf-command(download)' >/dev/null 2>&1 || dnf install -y dnf-plugins-core >/dev/null 2>&1 || true
  dnf download --destdir=/out 'lizardfs*'
"
# Defensive: drop anything that isn't a lizardfs* RPM (a future dnf that pulls
# extra deps despite no --resolve).
find "$OUT" -maxdepth 1 -name '*.rpm' ! -name 'lizardfs*' -delete 2>/dev/null || true
log "staged $(ls "$OUT"/*.rpm 2>/dev/null | wc -l) LizardFS RPM(s) in $OUT"
ls -la "$OUT"
