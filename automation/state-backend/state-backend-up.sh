#!/usr/bin/env bash
# DATACENTER-2 — run the OpenTofu http state backend (etcd-backed) as a container.
# Same pattern as the other fleet services (xo-up.sh / sccache-backend-up.sh). The
# script has no deps beyond the Python stdlib, so a stock python image runs it.
#   MCNF_ETCD (default http://172.20.145.192:2379), port 8390 (host network).
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ETCD="${MCNF_ETCD:-http://172.20.145.192:2379}"
IMG="${STATE_BACKEND_IMG:-docker.io/library/python:3-alpine}"

podman rm -f tofu-state-etcd >/dev/null 2>&1 || true
podman run -d --name tofu-state-etcd --restart=always --network host \
  -e MCNF_ETCD="$ETCD" -e STATE_BACKEND_PORT=8390 \
  -v "$HERE/tofu-state-etcd.py:/app.py:ro,Z" \
  "$IMG" python /app.py
echo "tofu-state-etcd up on :8390 → etcd $ETCD"
