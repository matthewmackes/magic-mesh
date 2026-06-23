#!/usr/bin/env bash
# xo-up.sh — bring up Xen Orchestra CE in podman (the Farm Automation Manager's
# management / console / REST-API plane). Idempotent: creates the `xo` podman
# network, a redis, and the xo-ce app, publishing the UI + REST on host :8080.
#
# Reconstructed from the live deployment on the control host (172.20.145.192).
# NOTE: this runs XO with its data INSIDE the container (no volume) — matching the
# current setup. Recreating xo-ce therefore loses XO's config (pools, users,
# tokens); re-add the pools in the UI and re-mint the tofu token after. (Add a
# volume on `-v xo-data:/var/lib/xo-server` if you want that to persist.)
#
# After first start:
#   1. create the admin:  podman exec xo-ce xo-server-recover-account <email> <pw>
#   2. add the XCP pools in the UI (http://<host>:8080 → Settings → Servers)
#   3. mint the OpenTofu token:  install-helpers/xo-mint-token.sh
#
# Usage: xo-up.sh [--port 8080]
set -euo pipefail

NET="xo"
PORT="8080"
REDIS_IMG="docker.io/library/redis:7-alpine"
XO_IMG="docker.io/ezka77/xen-orchestra-ce:latest"
while [ $# -gt 0 ]; do case "$1" in
  --port) PORT="$2"; shift 2;;
  -h|--help) sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'; exit 0;;
  *) echo "unknown arg: $1" >&2; exit 1;;
esac; done

command -v podman >/dev/null || { echo "podman required" >&2; exit 1; }

podman network exists "$NET" 2>/dev/null || { echo "==> create network $NET"; podman network create "$NET" >/dev/null; }

if podman container exists xo-redis 2>/dev/null; then
  echo "==> xo-redis already present"
else
  echo "==> start xo-redis (alias 'redis' — xo-ce dials redis://redis:6379)"
  podman run -d --name xo-redis --network "$NET" --network-alias redis "$REDIS_IMG" >/dev/null
fi

if podman container exists xo-ce 2>/dev/null; then
  echo "==> xo-ce already present"
else
  echo "==> start xo-ce (UI+REST on host :$PORT)"
  podman run -d --name xo-ce --network "$NET" -p "${PORT}:8000" \
    -e REDIS_URI=redis://redis:6379 "$XO_IMG" >/dev/null
fi

echo "XO up → http://<this-host>:${PORT}  (REST under /rest/v0; ws:// for OpenTofu)"
echo "next: create admin (podman exec xo-ce xo-server-recover-account …), add pools, then xo-mint-token.sh"
