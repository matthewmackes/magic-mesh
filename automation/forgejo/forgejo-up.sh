#!/usr/bin/env bash
# forgejo-up.sh — FARM-AUTO-2: self-hosted Forgejo (the platform's chosen CI; GitHub
# stays canonical, Forgejo pull-mirrors) + an Actions runner that drives the build
# farm. Forgejo is the trigger/UX (push/schedule, logs, matrix, retries); the work
# runs on the farm via the shared substrate (automation/lib + reconciler).
#
# Brings up Forgejo in podman on host :3000 (SSH :2222), creates an admin, mints a
# runner registration token, and starts an act_runner (label `farm`) that executes
# workflows on the control host — which dispatch to the fleet.
#
# Idempotent. Usage: forgejo-up.sh [--admin-pass <pw>]
set -euo pipefail
NET="forgejo"; DATA="${MCNF_FORGEJO_DATA:-/var/lib/mcnf-forgejo}"
IMG="${MCNF_FORGEJO_IMAGE:-codeberg.org/forgejo/forgejo:9}"
RUNNER_IMG="${MCNF_RUNNER_IMAGE:-code.forgejo.org/forgejo/runner:6}"
HOST_IP="${MCNF_HOST_IP:-172.20.145.192}"
ADMIN_USER="${MCNF_FORGEJO_ADMIN:-mcnfadmin}"
ADMIN_PASS=""
while [ $# -gt 0 ]; do case "$1" in --admin-pass) ADMIN_PASS="$2"; shift 2;; *) shift;; esac; done

command -v podman >/dev/null || { echo "podman required" >&2; exit 1; }
podman network exists "$NET" 2>/dev/null || podman network create "$NET" >/dev/null
mkdir -p "$DATA"

if podman container exists mcnf-forgejo 2>/dev/null; then
  echo "==> mcnf-forgejo already present"
else
  echo "==> start Forgejo (web :3000, ssh :2222) — headless auto-install (sqlite, INSTALL_LOCK)"
  # Persist a secret so restarts don't invalidate sessions/tokens.
  [ -f "$DATA/.secret" ] || { mkdir -p "$DATA"; openssl rand -hex 32 > "$DATA/.secret"; chmod 600 "$DATA/.secret"; }
  SECRET="$(cat "$DATA/.secret")"
  podman run -d --name mcnf-forgejo --network "$NET" \
    -p 3000:3000 -p 2222:22 \
    -e FORGEJO__server__ROOT_URL="http://${HOST_IP}:3000/" \
    -e FORGEJO__server__SSH_PORT=2222 \
    -e FORGEJO__actions__ENABLED=true \
    -e FORGEJO__database__DB_TYPE=sqlite3 \
    -e FORGEJO__database__PATH=/data/gitea/forgejo.db \
    -e FORGEJO__security__INSTALL_LOCK=true \
    -e FORGEJO__security__SECRET_KEY="$SECRET" \
    -e FORGEJO__service__DISABLE_REGISTRATION=true \
    -v "$DATA:/data:Z" "$IMG" >/dev/null   # :Z — SELinux relabel (Rocky/EL9 enforce)
fi
echo "==> wait for Forgejo"
for i in $(seq 1 30); do curl -s "http://127.0.0.1:3000/api/healthz" 2>/dev/null | grep -q pass && break; sleep 2; done

# Admin (idempotent — ignore "already exists").
if [ -n "$ADMIN_PASS" ]; then
  echo "==> ensure admin $ADMIN_USER"
  podman exec -u git mcnf-forgejo forgejo admin user create \
    --admin --username "$ADMIN_USER" --password "$ADMIN_PASS" \
    --email "$ADMIN_USER@mcnf.local" --must-change-password=false 2>&1 | tail -1 || true
fi

echo "==> mint an Actions runner registration token"
TOKEN="$(podman exec -u git mcnf-forgejo forgejo actions generate-runner-token 2>/dev/null | tr -d '\r\n')"
[ -n "$TOKEN" ] && echo "   runner token: ${TOKEN:0:8}… (saved /var/lib/mcnf-forgejo/.runner-token)" && printf '%s' "$TOKEN" > "$DATA/.runner-token" && chmod 600 "$DATA/.runner-token"

if [ -n "$TOKEN" ]; then
  podman container exists mcnf-forgejo-runner 2>/dev/null && { echo "==> runner already present"; } || {
    echo "==> register + start act_runner (label: farm)"
    podman run -d --name mcnf-forgejo-runner --network "$NET" \
      -e FORGEJO_INSTANCE_URL="http://mcnf-forgejo:3000" \
      -e FORGEJO_RUNNER_REGISTRATION_TOKEN="$TOKEN" \
      -e FORGEJO_RUNNER_LABELS="farm:host" \
      -v "$DATA/runner:/data" "$RUNNER_IMG" >/dev/null 2>&1 || echo "   (runner start needs review — see forgejo-up notes)"
  }
fi
echo "Forgejo → http://${HOST_IP}:3000  (admin: $ADMIN_USER). Push a repo with .forgejo/workflows/ to run the farm gate."
