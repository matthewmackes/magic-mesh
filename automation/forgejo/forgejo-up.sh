#!/usr/bin/env bash
# forgejo-up.sh — FARM-AUTO-2 / DAR-20 + DAR-25: self-hosted Forgejo on the
# control VM (the platform's chosen CI; GitHub stays canonical, Forgejo
# pull-mirrors). Forgejo is the trigger/UX (push/schedule, logs, matrix, retries);
# the work runs on the farm via the shared substrate (automation/lib + reconciler).
#
# DAR-20 — control-VM-targeted + secret-store-backed:
#   - binds the control VM's OVERLAY IP (nebula/mde-neb), not 0.0.0.0 / a hardcoded
#     LAN IP, so the CI host is reachable only over the mesh (same trust boundary
#     as the state backend). ROOT_URL is the overlay IP.
#   - the three durable secrets (SECRET_KEY, admin password, runner registration
#     token) come from / are persisted to the mesh secret store /mcnf/secret/
#     forgejo-* (DAR-25) — NO host-local plaintext .secret / .runner-token. A
#     control-VM rebuild reconstitutes CI by `get`ting them back.
#
# Idempotent: an existing container is detected and secrets are NOT regenerated.
#
# Usage: forgejo-up.sh [--host <overlay-ip>] [--admin-pass <pw>]
#   --host       control VM overlay IP (default: detected nebula/mde-neb IPv4).
#   --admin-pass override the admin password (default: from the store, else minted).
#
# Env: MCNF_HOST_IP (overlay IP override), MCNF_FORGEJO_DATA (/var/lib/mcnf-forgejo),
#      MCNF_FORGEJO_ADMIN (mcnfadmin), MCNF_ETCD (resolved by DAR-1b for the store).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
SECRET="$REPO/automation/secrets/mcnf-secret.sh"

NET="forgejo"
DATA="${MCNF_FORGEJO_DATA:-/var/lib/mcnf-forgejo}"
IMG="${MCNF_FORGEJO_IMAGE:-codeberg.org/forgejo/forgejo:9}"
# The runner is HOST-NATIVE (DAR-21, forgejo-runner-up.sh) — no runner image here.
ADMIN_USER="${MCNF_FORGEJO_ADMIN:-mcnfadmin}"
HOST_IP="${MCNF_HOST_IP:-}"
ADMIN_PASS=""

while [ $# -gt 0 ]; do
  case "$1" in
    --host)       HOST_IP="$2"; shift 2 ;;
    --admin-pass) ADMIN_PASS="$2"; shift 2 ;;
    -h|--help)    sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) shift ;;
  esac
done

# ── overlay bind (DAR-20): never 0.0.0.0, never a hardcoded LAN IP ──
detect_overlay() {
  ip -o -4 addr show 2>/dev/null \
    | awk '$2 ~ /nebula|mde-neb/ {split($4,a,"/"); print a[1]; exit}'
}
[ -n "$HOST_IP" ] || HOST_IP="$(detect_overlay)"
if [ -z "$HOST_IP" ]; then
  echo "forgejo-up: no overlay (nebula/mde-neb) interface found and --host/MCNF_HOST_IP unset" >&2
  echo "  — refusing to bind a CI host off the mesh. Join the mesh (mackesd join) or pass --host <overlay-ip>." >&2
  exit 1
fi

command -v podman >/dev/null || { echo "podman required" >&2; exit 1; }

# ── secret-store helpers (DAR-25): the three durable Forgejo secrets ──
# get-or-mint: return the stored value, or mint+store a fresh one (idempotent).
store_get() { bash "$SECRET" get "$1" 2>/dev/null; }
store_put() { printf '%s' "$2" | bash "$SECRET" put "$1" >/dev/null; }
get_or_mint() { # <secret-name> <mint-cmd>
  local v
  if v="$(store_get "$1")" && [ -n "$v" ]; then printf '%s' "$v"; return 0; fi
  v="$(eval "$2")"
  store_put "$1" "$v"
  printf '%s' "$v"
}

podman network exists "$NET" 2>/dev/null || podman network create "$NET" >/dev/null
mkdir -p "$DATA"

if podman container exists mcnf-forgejo 2>/dev/null; then
  echo "==> mcnf-forgejo already present (idempotent — secrets not regenerated)"
else
  echo "==> start Forgejo (web :3000, ssh :2222) on overlay $HOST_IP — headless auto-install"
  # DAR-25: SECRET_KEY from the store (mint+persist on first stand-up) — never a
  # host-local $DATA/.secret plaintext. Stable across restarts AND rebuilds.
  SECRET_KEY="$(get_or_mint forgejo-secret-key 'openssl rand -hex 32')"
  podman run -d --name mcnf-forgejo --network "$NET" \
    -p "${HOST_IP}:3000:3000" -p "${HOST_IP}:2222:22" \
    -e FORGEJO__server__ROOT_URL="http://${HOST_IP}:3000/" \
    -e FORGEJO__server__SSH_PORT=2222 \
    -e FORGEJO__actions__ENABLED=true \
    -e FORGEJO__database__DB_TYPE=sqlite3 \
    -e FORGEJO__database__PATH=/data/gitea/forgejo.db \
    -e FORGEJO__security__INSTALL_LOCK=true \
    -e FORGEJO__security__SECRET_KEY="$SECRET_KEY" \
    -e FORGEJO__service__DISABLE_REGISTRATION=true \
    -v "$DATA:/data:Z" "$IMG" >/dev/null   # :Z — SELinux relabel (Rocky/EL9 enforce)
fi

echo "==> wait for Forgejo"
for _ in $(seq 1 30); do curl -s "http://${HOST_IP}:3000/api/healthz" 2>/dev/null | grep -q pass && break; sleep 2; done

# ── admin (DAR-25): password from the store (mint+persist if absent) ──
[ -n "$ADMIN_PASS" ] || ADMIN_PASS="$(get_or_mint forgejo-admin-pass 'openssl rand -base64 18')"
echo "==> ensure admin $ADMIN_USER (idempotent — ignore already-exists)"
podman exec -u git mcnf-forgejo forgejo admin user create \
  --admin --username "$ADMIN_USER" --password "$ADMIN_PASS" \
  --email "$ADMIN_USER@mcnf.local" --must-change-password=false 2>&1 | tail -1 || true

# ── runner registration token (DAR-25): persist to the store ──
echo "==> mint an Actions runner registration token + persist to the store"
TOKEN="$(podman exec -u git mcnf-forgejo forgejo actions generate-runner-token 2>/dev/null | tr -d '\r\n')"
if [ -n "$TOKEN" ]; then
  store_put forgejo-runner-token "$TOKEN"
  echo "   runner token: ${TOKEN:0:8}… (stored /mcnf/secret/forgejo-runner-token — NO host-local plaintext)"
else
  echo "   WARN: could not mint a runner token (Forgejo not ready?) — re-run forgejo-up.sh" >&2
fi

cat <<EOF
Forgejo → http://${HOST_IP}:3000  (admin: $ADMIN_USER; password in /mcnf/secret/forgejo-admin-pass).
Next: forgejo-runner-up.sh (host-native act_runner, label farm), then forgejo-seed.sh (repo).
Durable secrets in the store: forgejo-secret-key, forgejo-admin-pass, forgejo-runner-token.
EOF
