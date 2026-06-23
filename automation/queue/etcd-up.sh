#!/usr/bin/env bash
# etcd-up.sh — bring up the single-node etcd that backs the FARM-AUTO-3 work
# queue (the SUBSTRATE-V2 coordination store, here used for build orchestration).
# Idempotent; publishes the v3 client API + HTTP gateway on host :2379.
#   farm-enqueue / farm-agent / farm-pool-manager all talk to it over plain HTTP.
set -euo pipefail
IMG="${MCNF_ETCD_IMAGE:-quay.io/coreos/etcd:v3.5.16}"
PORT="${MCNF_ETCD_PORT:-2379}"
command -v podman >/dev/null || { echo "podman required" >&2; exit 1; }
if podman container exists mcnf-etcd 2>/dev/null; then
  echo "mcnf-etcd already present ($(podman inspect mcnf-etcd --format '{{.State.Status}}'))"
else
  podman run -d --name mcnf-etcd -p "${PORT}:2379" -p 2380:2380 "$IMG" \
    /usr/local/bin/etcd --name s1 \
    --advertise-client-urls "http://0.0.0.0:2379" --listen-client-urls "http://0.0.0.0:2379" \
    --initial-advertise-peer-urls "http://0.0.0.0:2380" --listen-peer-urls "http://0.0.0.0:2380" \
    --initial-cluster s1=http://0.0.0.0:2380 >/dev/null
  echo "started mcnf-etcd"
fi
for i in $(seq 1 15); do curl -s "http://127.0.0.1:${PORT}/version" >/dev/null 2>&1 && break; sleep 2; done
echo "etcd → http://<host>:${PORT}  $(curl -s http://127.0.0.1:${PORT}/version)"
