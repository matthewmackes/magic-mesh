#!/usr/bin/env bash
# DS-1 prerequisite — stand up Xen Orchestra Community Edition on the control host (podman).
# XO fronts both XCP-ng pools and is what the OpenTofu vatesfr/xenorchestra provider drives.
# See docs/ops/environment-rebuild.md Phase 2. Idempotent: re-run is safe.
set -euo pipefail

NET=xo
XO_PORT="${XO_PORT:-8080}"
XO_IMAGE="${XO_IMAGE:-docker.io/ezka77/xen-orchestra-ce:latest}"
REDIS_IMAGE="${REDIS_IMAGE:-docker.io/library/redis:7-alpine}"

podman network exists "$NET" || podman network create "$NET"

if ! podman container exists xo-redis; then
  # Alias 'redis' so the XO image's default redis://redis:6379 resolves.
  podman run -d --name xo-redis --network "$NET" --network-alias redis \
    --restart unless-stopped "$REDIS_IMAGE"
fi

if ! podman container exists xo-ce; then
  podman run -d --name xo-ce --network "$NET" \
    -p "${XO_PORT}:8000" \
    -e REDIS_URI=redis://redis:6379 \
    --restart unless-stopped \
    "$XO_IMAGE"
fi

echo "XO containers:"
podman ps --filter name=xo- --format '  {{.Names}}  {{.Status}}  {{.Ports}}'
echo "XO UI/API → http://$(hostname -I | awk '{print $1}'):${XO_PORT}"
