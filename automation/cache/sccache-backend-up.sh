#!/usr/bin/env bash
# sccache-backend-up.sh — BUILD-PLATFORM-1: the shared sccache backend (minio/S3)
# on the control host. Every build VM's sccache reads/writes this bucket, so a
# crate compiled on any node is reused on all of them (kills cold-target latency).
# Idempotent.
#
# DAR-5: the minio root creds come from the MESH SECRET STORE (age-encrypted in
# etcd), NOT a committed default. The old in-repo literals (mcnfcache/mcnfcache2026)
# are GONE — seal real keys once with:
#   printf %s '<access-key>' | automation/secrets/mcnf-secret.sh put sccache-access-key
#   printf %s '<secret-key>' | automation/secrets/mcnf-secret.sh put sccache-secret-key
# Explicit MCNF_MINIO_ACCESS_KEY / MCNF_MINIO_SECRET_KEY env still win (for a first
# bootstrap before the store exists); otherwise we resolve from the store and fail
# loud if neither is available. The value is never printed.
#
# After this: ansible-playbook infra/ansible/sccache.yml (creds resolved from the store)
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" && pwd)"
SECRET="${MCNF_SECRET_BIN:-$HERE/../secrets/mcnf-secret.sh}"
DATA="${MCNF_MINIO_DATA:-/var/lib/mcnf-minio}"
PORT="${MCNF_MINIO_PORT:-9000}"
CONSOLE="${MCNF_MINIO_CONSOLE:-9001}"
# Resolve creds: explicit env wins (bootstrap); else unseal from the store; else fail.
AK="${MCNF_MINIO_ACCESS_KEY:-}"
SK="${MCNF_MINIO_SECRET_KEY:-}"
[ -n "$AK" ] || AK="$(bash "$SECRET" get sccache-access-key 2>/dev/null || true)"
[ -n "$SK" ] || SK="$(bash "$SECRET" get sccache-secret-key 2>/dev/null || true)"
if [ -z "$AK" ] || [ -z "$SK" ]; then
  echo "sccache-backend-up: no minio creds — set MCNF_MINIO_ACCESS_KEY/SECRET_KEY, or seal them:" >&2
  echo "  printf %s '<key>' | $SECRET put sccache-access-key   (and sccache-secret-key)" >&2
  exit 1
fi
HOST_IP="${MCNF_HOST_IP:-172.20.145.192}"
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
# DAR-5: never print the resolved creds — the play unseals them from the store too.
echo "next: ansible-playbook infra/ansible/sccache.yml  (minio creds unsealed from /mcnf/secret/sccache-*)"
