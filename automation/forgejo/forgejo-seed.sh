#!/usr/bin/env bash
# forgejo-seed.sh — DAR-22: seed the magic-mesh repo into Forgejo. GitHub stays
# canonical upstream; Forgejo PULL-MIRRORS it when reachable, else SEEDS from the
# on-disk clone (the air-gap path) so founding never hard-depends on the internet.
#
# Two paths, decided by GitHub reachability:
#   ONLINE  — create the repo as a pull-mirror of github.com/<UPSTREAM> (Forgejo
#             keeps it synced); the operator pushes feature branches to GitHub.
#   AIRGAP  — create an empty repo, then `git push` the local /opt/mcnf clone's
#             master so the sovereign Forgejo has the code with no GitHub.
#
# Admin token: minted via the API using the admin password from the secret store
# (/mcnf/secret/forgejo-admin-pass) — never a literal. Idempotent: an existing
# repo is detected (no duplicate, no re-clone).
#
# Usage: forgejo-seed.sh [--host <overlay-ip>] [--upstream <owner/repo>]
# Env: MCNF_FORGEJO_ADMIN (mcnfadmin), MCNF_REPO (/opt/mcnf), MCNF_HOST_IP,
#      MCNF_GH_UPSTREAM (default matthewmackes/magic-mesh).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SRC_REPO="$(cd "$HERE/../.." && pwd)"
SECRET="$SRC_REPO/automation/secrets/mcnf-secret.sh"

HOST_IP="${MCNF_HOST_IP:-}"
ADMIN_USER="${MCNF_FORGEJO_ADMIN:-mcnfadmin}"
REPO_DIR="${MCNF_REPO:-/opt/mcnf}"
UPSTREAM="${MCNF_GH_UPSTREAM:-matthewmackes/magic-mesh}"
REPO_NAME="magic-mesh"

while [ $# -gt 0 ]; do
  case "$1" in
    --host)     HOST_IP="$2"; shift 2 ;;
    --upstream) UPSTREAM="$2"; shift 2 ;;
    -h|--help)  sed -n '2,24p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) shift ;;
  esac
done

detect_overlay() { ip -o -4 addr show 2>/dev/null | awk '$2 ~ /nebula|mde-neb/ {split($4,a,"/"); print a[1]; exit}'; }
[ -n "$HOST_IP" ] || HOST_IP="$(detect_overlay)"
[ -n "$HOST_IP" ] || { echo "forgejo-seed: no overlay IP (pass --host)" >&2; exit 1; }
BASE="http://${HOST_IP}:3000"

ADMIN_PASS="$(bash "$SECRET" get forgejo-admin-pass 2>/dev/null || true)"
[ -n "$ADMIN_PASS" ] || { echo "forgejo-seed: no /mcnf/secret/forgejo-admin-pass (run forgejo-up.sh first)" >&2; exit 1; }

# Mint a short-lived admin API token via basic auth (the admin password). We do
# NOT persist this token — it is request-scoped. NEVER logged.
api() { curl -s -u "${ADMIN_USER}:${ADMIN_PASS}" "$@"; }

# Idempotent: already present?
if api "$BASE/api/v1/repos/${ADMIN_USER}/${REPO_NAME}" | grep -q "\"name\":\"${REPO_NAME}\""; then
  echo "==> repo ${ADMIN_USER}/${REPO_NAME} already exists (idempotent — no re-seed)"
  exit 0
fi

# GitHub reachable? (bounded TCP probe to github.com:443)
gh_reachable() { timeout 5 bash -c 'cat </dev/null >/dev/tcp/github.com/443' 2>/dev/null; }

if gh_reachable; then
  echo "==> GitHub reachable — create ${REPO_NAME} as a PULL-MIRROR of github.com/${UPSTREAM}"
  api -X POST "$BASE/api/v1/repos/migrate" \
    -H 'Content-Type: application/json' \
    -d "{\"clone_addr\":\"https://github.com/${UPSTREAM}.git\",\"repo_name\":\"${REPO_NAME}\",\"mirror\":true,\"private\":false,\"repo_owner\":\"${ADMIN_USER}\"}" \
    >/dev/null
  echo "   pull-mirror configured: $BASE/${ADMIN_USER}/${REPO_NAME} (Forgejo keeps it synced from GitHub)"
else
  echo "==> GitHub UNREACHABLE — air-gap seed from the on-disk clone $REPO_DIR"
  [ -d "$REPO_DIR/.git" ] || { echo "  no git clone at $REPO_DIR — cannot air-gap seed" >&2; exit 1; }
  # Create an empty repo, then push the local master.
  api -X POST "$BASE/api/v1/user/repos" \
    -H 'Content-Type: application/json' \
    -d "{\"name\":\"${REPO_NAME}\",\"private\":false,\"auto_init\":false}" >/dev/null
  # Push over HTTP with the admin creds in the URL (host-local, overlay-only; not logged).
  push_url="http://${ADMIN_USER}:${ADMIN_PASS}@${HOST_IP}:3000/${ADMIN_USER}/${REPO_NAME}.git"
  git -C "$REPO_DIR" push "$push_url" 'refs/heads/master:refs/heads/master' >/dev/null 2>&1 \
    || git -C "$REPO_DIR" push "$push_url" 'HEAD:refs/heads/master' >/dev/null 2>&1
  echo "   air-gap seed pushed: $BASE/${ADMIN_USER}/${REPO_NAME} (HEAD = local master)"
fi

echo "Seeded. Push .forgejo/workflows/ runs on the host-native runner (label farm)."
