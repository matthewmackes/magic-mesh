#!/usr/bin/env bash
# dr-push-offfleet.sh — DAR-41: OPERATOR-RUN off-fleet push of the newest DR
# artifact to the DO Spaces bucket (mcnf-dr-4533), so an off-fleet copy survives
# total-fleet loss. The safety classifier hard-BLOCKS an automated agent from
# running this (spend/egress past the trust boundary) — it is operator-only.
#
# DEFAULT IS --dry-run: prints the exact rclone command + the resolved source path
# WITHOUT contacting DO and WITHOUT printing the Spaces secret. Pass --push to run.
#
# The Spaces key is fetched via mcnf-secret.sh get dr-spaces-key (NO plaintext in
# the script or any log). The off-fleet path is s3://<bucket>/age/.
#
# Usage:
#   dr-push-offfleet.sh                 # dry-run (default): show the command, push nothing
#   dr-push-offfleet.sh --push          # OPERATOR: actually push (agent is classifier-blocked)
#   dr-push-offfleet.sh --file <dr.age> # push a specific artifact (default: newest in the mesh dir)
# Env (via dr-env.sh): MCNF_MESHFS_DIR, MCNF_DR_BUCKET, MCNF_DR_DIR.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SRC_REPO="$(cd "$HERE/../.." && pwd)"
SECRET="$SRC_REPO/automation/secrets/mcnf-secret.sh"
# shellcheck source=./dr-env.sh
. "$HERE/dr-env.sh"

PUSH=0
FILE=""
while [ $# -gt 0 ]; do
  case "$1" in
    --push)    PUSH=1; shift ;;
    --dry-run) PUSH=0; shift ;;
    --file)    FILE="$2"; shift 2 ;;
    -h|--help) sed -n '2,22p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "dr-push-offfleet: unknown arg '$1'" >&2; exit 2 ;;
  esac
done

BUCKET="$MCNF_DR_BUCKET"
REMOTE_PATH="s3://${BUCKET}/age/"

# Resolve the source: explicit --file, else the newest dr-*.age in the mesh dir,
# else the newest in the local DR working dir.
if [ -z "$FILE" ]; then
  FILE="$(ls -1t "$MCNF_MESHFS_DIR"/dr/dr-*.age 2>/dev/null | head -1 || true)"
  [ -n "$FILE" ] || FILE="$(ls -1t "$MCNF_DR_DIR"/dr-*.age 2>/dev/null | head -1 || true)"
fi
[ -n "$FILE" ] && [ -f "$FILE" ] || { echo "dr-push-offfleet: no DR artifact found (run dr-snapshot-onmesh.sh first, or pass --file)" >&2; exit 2; }

SHA="$(sha256sum "$FILE" | awk '{print $1}')"

# The rclone command shape (the remote name `mcnf-spaces` is the operator's rclone
# config; creds are NOT inlined — rclone reads its own config, or the env vars below
# which we set from the store ONLY on a real --push, never echoing the value).
RCLONE_CMD="rclone copyto \"$FILE\" \"${REMOTE_PATH}$(basename "$FILE")\""

if [ "$PUSH" -eq 0 ]; then
  cat <<EOF
dr-push-offfleet --dry-run (NOTHING pushed; DO not contacted):
  source:   $FILE
  sha256:   $SHA
  remote:   ${REMOTE_PATH}$(basename "$FILE")
  command:  $RCLONE_CMD
  (the Spaces key is fetched from /mcnf/secret/dr-spaces-key on a real --push; never printed)

To push (OPERATOR ONLY — the agent is classifier-blocked):
  automation/dr/dr-push-offfleet.sh --push
EOF
  exit 0
fi

# --- real push (operator) ---
command -v rclone >/dev/null 2>&1 || { echo "dr-push-offfleet: rclone not installed" >&2; exit 1; }

# Fetch the Spaces key from the store into process-scoped env — NEVER logged, NEVER
# written to a file. The store value is expected as "ACCESS_KEY:SECRET_KEY".
KEYPAIR="$(bash "$SECRET" get dr-spaces-key 2>/dev/null || true)"
[ -n "$KEYPAIR" ] || { echo "dr-push-offfleet: /mcnf/secret/dr-spaces-key is absent — operator must seal it first" >&2; exit 1; }
RCLONE_S3_ACCESS_KEY_ID="${KEYPAIR%%:*}"
RCLONE_S3_SECRET_ACCESS_KEY="${KEYPAIR#*:}"
export RCLONE_CONFIG_MCNFSPACES_TYPE=s3
export RCLONE_CONFIG_MCNFSPACES_PROVIDER=DigitalOcean
export RCLONE_CONFIG_MCNFSPACES_ACCESS_KEY_ID="$RCLONE_S3_ACCESS_KEY_ID"
export RCLONE_CONFIG_MCNFSPACES_SECRET_ACCESS_KEY="$RCLONE_S3_SECRET_ACCESS_KEY"
export RCLONE_CONFIG_MCNFSPACES_ENDPOINT="${MCNF_DR_SPACES_ENDPOINT:-nyc3.digitaloceanspaces.com}"
unset KEYPAIR

echo "==> pushing $(basename "$FILE") → mcnfspaces:${BUCKET}/age/ (sha256 ${SHA:0:12}…)"
if rclone copyto "$FILE" "mcnfspaces:${BUCKET}/age/$(basename "$FILE")"; then
  echo "dr-push-offfleet: pushed OK (verify the remote sha256 matches $SHA)"
else
  echo "dr-push-offfleet: rclone push FAILED" >&2; exit 1
fi
