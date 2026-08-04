#!/usr/bin/env bash
# Register this node's host-held age identity and, on a Lighthouse, reseal mesh
# secrets when the public recipient set changes. Private identity material stays
# at /root/.mcnf-age-key; only age public recipients are published to etcd.
set -euo pipefail

ROLE_FILE="${MCNF_ROLE_FILE:-/var/lib/mde/role.toml}"
SECRET_HELPER="${MCNF_SECRET_HELPER:-${MCNF_REPO:-/opt/mcnf}/automation/secrets/mcnf-secret.sh}"
STATE_DIR="${MCNF_RECIPIENT_STATE_DIR:-/var/lib/mackesd}"
STATE_FILE="$STATE_DIR/mesh-secret-recipients.sha256"

if [ ! -r "$ROLE_FILE" ]; then
  echo "mesh-secret-recipient: role is not pinned yet; retrying later" >&2
  exit 0
fi
if [ ! -x "$SECRET_HELPER" ]; then
  echo "mesh-secret-recipient: helper unavailable at $SECRET_HELPER" >&2
  exit 1
fi

role="$(awk -F= '
  /^[[:space:]]*role[[:space:]]*=/ {
    value=$2
    gsub(/[[:space:]"]/, "", value)
    print tolower(value)
    exit
  }
' "$ROLE_FILE")"
case "$role" in
  lighthouse|workstation) ;;
  *) echo "mesh-secret-recipient: unsupported role '$role'" >&2; exit 1 ;;
esac

# Idempotent: creates the 0600 local identity only when absent, then publishes
# only its public recipient and role tag. Suppress the helper's public-key hint
# so normal service logs stay compact.
MCNF_NODE_ROLE="$role" "$SECRET_HELPER" init-self >/dev/null

# A Workstation cannot and must not reseal the store merely because it has just
# joined: the existing Lighthouse holders authorize that recipient expansion.
if [ "$role" != lighthouse ]; then
  exit 0
fi

recipient_hash="$($SECRET_HELPER recipients | sha256sum | awk '{print $1}')"
previous_hash=""
if [ -r "$STATE_FILE" ]; then
  previous_hash="$(sed -n '1p' "$STATE_FILE")"
fi
if [ "$recipient_hash" = "$previous_hash" ]; then
  exit 0
fi

# mcnf-secret.sh serializes competing lighthouses with its lease-backed CAS
# lock and preserves every secret's recorded scope while resealing.
"$SECRET_HELPER" reseal-all >/dev/null
install -d -m 0700 "$STATE_DIR"
temporary="$(mktemp "$STATE_DIR/.mesh-secret-recipients.XXXXXX")"
trap 'rm -f -- "$temporary"' EXIT
chmod 0600 "$temporary"
printf '%s\n' "$recipient_hash" >"$temporary"
mv -f -- "$temporary" "$STATE_FILE"
trap - EXIT
