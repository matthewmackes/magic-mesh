#!/usr/bin/env bash
# DATACENTER-2 / DAR-7 — run the OpenTofu http state backend (etcd-backed) as a
# container. Same pattern as the other fleet services (xo-up.sh /
# sccache-backend-up.sh). No deps beyond the Python stdlib, so a stock python
# image runs it.
#
# DAR-7: endpoints are SOURCED from /etc/mackesd/etcd-endpoints via the shared
# resolver (automation/lib/etcd-endpoints.sh) — NO hardcoded .192:2379 default.
# Fails loud if neither MCNF_ETCD nor the endpoints file is present. The backend
# binds the OVERLAY IP (lock 7), passed in as STATE_BACKEND_BIND, not 0.0.0.0.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"

# shellcheck source=../lib/etcd-endpoints.sh
. "$REPO/automation/lib/etcd-endpoints.sh"

ETCD="$(mcnf_resolve_etcd)" || exit 1   # explicit env → endpoints file → fail loud
IMG="${STATE_BACKEND_IMG:-docker.io/library/python:3-alpine}"
PORT="${STATE_BACKEND_PORT:-8390}"

# Overlay bind: detect the nebula/mde-neb IPv4 unless the operator overrides it.
# (NEVER 0.0.0.0 — the overlay-only bind is the only thing fronting plain-HTTP
# unauthenticated etcd state; see design §6.)
detect_overlay() {
  ip -o -4 addr show 2>/dev/null \
    | awk '$2 ~ /nebula|mde-neb/ {split($4,a,"/"); print a[1]; exit}'
}
BIND="${STATE_BACKEND_BIND:-$(detect_overlay)}"
if [ -z "$BIND" ]; then
  echo "state-backend-up: no overlay (nebula/mde-neb) interface found and " \
       "STATE_BACKEND_BIND unset — refusing to bind 0.0.0.0. Join the mesh " \
       "(mackesd join) or export STATE_BACKEND_BIND=<overlay-ip>." >&2
  exit 1
fi

podman rm -f tofu-state-etcd >/dev/null 2>&1 || true
podman run -d --name tofu-state-etcd --restart=always --network host \
  -e MCNF_ETCD="$ETCD" -e STATE_BACKEND_PORT="$PORT" -e STATE_BACKEND_BIND="$BIND" \
  -v "$HERE/tofu-state-etcd.py:/app.py:ro,Z" \
  "$IMG" python /app.py
echo "tofu-state-etcd up on $BIND:$PORT → etcd $ETCD"
