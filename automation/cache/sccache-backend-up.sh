#!/usr/bin/env bash
# sccache-backend-up.sh — BUILD-PLATFORM-1: the shared sccache backend (minio/S3)
# on the control host. Every build VM's sccache reads/writes this bucket, so a
# crate compiled on any node is reused on all of them (kills cold-target latency).
# Idempotent. Creds are NOT in the repo — pass them (or accept the dev defaults
# below, which the Ansible play also uses via -e).
#
# After this: ansible-playbook infra/ansible/sccache.yml -e minio_access_key=… -e minio_secret_key=…
set -euo pipefail
_SCCACHE_HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# DAR-17: the minio host is the control VM, resolved via the shared portable chain
# (explicit MCNF_HOST_IP/MCNF_CONTROL_IP > the per-mesh /mcnf/site doc > the peer
# directory > this node's overlay), NEVER the dead LAN node 172.20.145.192.
# shellcheck source=../lib/control-host.sh
. "$_SCCACHE_HERE/../lib/control-host.sh"
DATA="${MCNF_MINIO_DATA:-/var/lib/mcnf-minio}"
PORT="${MCNF_MINIO_PORT:-9000}"
CONSOLE="${MCNF_MINIO_CONSOLE:-9001}"
AK="${MCNF_MINIO_ACCESS_KEY:-mcnfcache}"
SK="${MCNF_MINIO_SECRET_KEY:-mcnfcache2026}"
HOST_IP="${MCNF_HOST_IP:-$(MCNF_CONTROL_IP="${MCNF_CONTROL_IP:-}" mcnf_resolve_control_host)}"
[ -n "$HOST_IP" ] || { echo "sccache-backend-up: cannot resolve the control host (set MCNF_HOST_IP or MCNF_CONTROL_IP, or join the overlay)" >&2; exit 1; }
command -v podman >/dev/null || { echo "podman required" >&2; exit 1; }
mkdir -p "$DATA"

if podman container exists mcnf-minio 2>/dev/null; then
  echo "==> mcnf-minio already present"
else
  echo "==> start minio (S3 :$PORT, console :$CONSOLE)"
  podman run -d --name mcnf-minio -p "${PORT}:9000" -p "${CONSOLE}:9001" \
    -e MINIO_ROOT_USER="$AK" -e MINIO_ROOT_PASSWORD="$SK" \
    -v "${DATA}:/data:Z" quay.io/minio/minio server /data --console-address ":9001" >/dev/null
fi
for _ in $(seq 1 15); do curl -s -o /dev/null "http://127.0.0.1:${PORT}/minio/health/live" 2>/dev/null && break; sleep 2; done
echo "==> create the sccache bucket (idempotent)"
podman run --rm --network host -e MC_HOST_local="http://${AK}:${SK}@${HOST_IP}:${PORT}" \
  quay.io/minio/mc mb -p local/sccache 2>&1 | tail -1
echo "minio → http://${HOST_IP}:${PORT}  bucket: sccache  (console :${CONSOLE})"
echo "next: ansible-playbook infra/ansible/sccache.yml -e minio_access_key=$AK -e minio_secret_key=<secret>"
