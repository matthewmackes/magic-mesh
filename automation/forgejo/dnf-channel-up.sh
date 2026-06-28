#!/usr/bin/env bash
# dnf-channel-up.sh — DAR-23: a SOVEREIGN dnf channel served by the control VM,
# laid out like the gh-pages channel (fedora-N-x86_64/repodata/repomd.xml + the
# RPM-GPG-KEY), so a freshly-provisioned peer can dnf-install from the mesh when
# GitHub is unreachable (air-gapped provisioning).
#
# Layout (matches the gh-pages baseurl shape so do-lighthouse-cloudinit.sh works
# UNCHANGED with REPO_BASEURL pointed here):
#   <root>/fedora-<N>-x86_64/repodata/repomd.xml   ← createrepo_c metadata
#   <root>/fedora-<N>-x86_64/HOLD/                  ← DAR-24: CI stages UNSIGNED here
#   <root>/RPM-GPG-KEY-magic-mesh                   ← the published public key
#
# Signing stays OPERATOR-GATED (sign-release.sh / the /release step) — this channel
# serves UNSIGNED CI artifacts from the HOLD area until an operator signs + promotes
# them. The channel is served over the control VM OVERLAY IP (podman + a static
# httpd), never 0.0.0.0 / a hardcoded LAN IP.
#
# Usage: dnf-channel-up.sh [--host <overlay-ip>] [--fedora <N,...>]
# Env: MCNF_HOST_IP, MCNF_DNF_ROOT (/var/lib/mcnf-dnf-channel),
#      MCNF_DNF_PORT (8480), MCNF_FEDORA_VERSIONS (43 44).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SRC_REPO="$(cd "$HERE/../.." && pwd)"
GPG_KEY="$SRC_REPO/packaging/repo/RPM-GPG-KEY-magic-mesh"

HOST_IP="${MCNF_HOST_IP:-}"
ROOT="${MCNF_DNF_ROOT:-/var/lib/mcnf-dnf-channel}"
PORT="${MCNF_DNF_PORT:-8480}"
FEDORAS="${MCNF_FEDORA_VERSIONS:-43 44}"

while [ $# -gt 0 ]; do
  case "$1" in
    --host)    HOST_IP="$2"; shift 2 ;;
    --fedora)  FEDORAS="${2//,/ }"; shift 2 ;;
    -h|--help) sed -n '2,24p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) shift ;;
  esac
done

detect_overlay() { ip -o -4 addr show 2>/dev/null | awk '$2 ~ /nebula|mde-neb/ {split($4,a,"/"); print a[1]; exit}'; }
[ -n "$HOST_IP" ] || HOST_IP="$(detect_overlay)"
[ -n "$HOST_IP" ] || { echo "dnf-channel-up: no overlay IP (pass --host)" >&2; exit 1; }

command -v podman >/dev/null || { echo "podman required" >&2; exit 1; }
command -v createrepo_c >/dev/null || { echo "createrepo_c required (dnf install createrepo_c)" >&2; exit 1; }

echo "==> lay out the channel under $ROOT (gh-pages shape) for fedora: $FEDORAS"
mkdir -p "$ROOT"
# Publish the public GPG key at the channel root (gpgcheck=1 path).
[ -f "$GPG_KEY" ] && cp -f "$GPG_KEY" "$ROOT/RPM-GPG-KEY-magic-mesh"
for n in $FEDORAS; do
  arch_dir="$ROOT/fedora-${n}-x86_64"
  mkdir -p "$arch_dir/HOLD"
  # createrepo_c over the arch dir (the HOLD subdir's RPMs are indexed too, so a
  # CI-staged unsigned RPM is dnf-visible immediately for testing; an operator
  # signs + promotes it out of HOLD for production).
  echo "   createrepo_c fedora-${n}-x86_64"
  createrepo_c --update "$arch_dir" >/dev/null
done

# A ready-to-drop client repo file (mirrors gh-pages magic-mesh.repo but pointed at
# this sovereign channel). do-lighthouse-cloudinit.sh renders its own from
# REPO_BASEURL; this is for hand-install + verification.
cat > "$ROOT/magic-mesh.repo" <<EOF
# Sovereign mesh dnf channel (DAR-23) — served by the control VM over the overlay.
[magic-mesh]
name=Magic Mesh (sovereign mesh channel)
baseurl=http://${HOST_IP}:${PORT}/fedora-\$releasever-\$basearch/
type=rpm-md
skip_if_unavailable=True
gpgcheck=1
gpgkey=http://${HOST_IP}:${PORT}/RPM-GPG-KEY-magic-mesh
repo_gpgcheck=0
enabled=1
EOF

echo "==> serve $ROOT over overlay ${HOST_IP}:${PORT} (static httpd, overlay-only bind)"
if podman container exists mcnf-dnf-channel 2>/dev/null; then
  echo "   mcnf-dnf-channel already present — content refreshed in place (idempotent)"
else
  podman run -d --name mcnf-dnf-channel --restart=always \
    -p "${HOST_IP}:${PORT}:80" \
    -v "$ROOT:/usr/share/nginx/html:ro,Z" \
    docker.io/library/nginx:alpine >/dev/null
fi

echo "==> wait for the channel"
for _ in $(seq 1 15); do curl -s -o /dev/null -w '%{http_code}' "http://${HOST_IP}:${PORT}/RPM-GPG-KEY-magic-mesh" 2>/dev/null | grep -q 200 && break; sleep 1; done

cat <<EOF
Sovereign dnf channel → http://${HOST_IP}:${PORT}
  repomd:  http://${HOST_IP}:${PORT}/fedora-<N>-x86_64/repodata/repomd.xml
  gpg key: http://${HOST_IP}:${PORT}/RPM-GPG-KEY-magic-mesh
  HOLD:    <root>/fedora-<N>-x86_64/HOLD/ (DAR-24 stages UNSIGNED CI RPMs here)
Point do-lighthouse-cloudinit.sh REPO_BASEURL=http://${HOST_IP}:${PORT} for air-gap provisioning.
Signing stays operator-gated (sign-release.sh) — promotion out of HOLD is NOT automated.
EOF
