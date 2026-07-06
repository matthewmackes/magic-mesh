#!/usr/bin/env bash
# DATACENTER-23 — safe verification of the newest off-fleet DR backup.
#
# Downloads the newest dr-*.age artifact + .sha256 sidecar from the configured
# rclone target, verifies the checksum, restores under a throwaway temp prefix,
# then deletes the temp etcd prefix. Production keys and CA paths are never
# touched.
#
# Env:
#   MCNF_DR_REMOTE  rclone target (default mcnf-spaces:mcnf-mesh-media/mcnf-dr-backups)
#   MCNF_ETCD       etcd v3 gateway (http://172.20.145.192:2379)
set -euo pipefail

REMOTE="${MCNF_DR_REMOTE:-mcnf-spaces:mcnf-mesh-media/mcnf-dr-backups}"
ETCD="${MCNF_ETCD:-http://172.20.145.192:2379}"
HERE="$(cd "$(dirname "$0")" && pwd)"

need() { command -v "$1" >/dev/null 2>&1 || { echo "$1 is required" >&2; exit 2; }; }
need rclone
need sha256sum
need python3

b64() { base64 -w0; }

delete_prefix() {
  local prefix="$1" payload
  payload="$(python3 - "$prefix" <<'PY'
import base64, json, sys

prefix = sys.argv[1]
if not prefix:
    raise SystemExit("empty prefix")
last = prefix[-1]
range_end = prefix[:-1] + chr(ord(last) + 1)
print(json.dumps({
    "key": base64.b64encode(prefix.encode()).decode(),
    "range_end": base64.b64encode(range_end.encode()).decode(),
}))
PY
)"
  curl -s -X POST "$ETCD/v3/kv/deleterange" -d "$payload" >/dev/null
}

latest="$(
  rclone lsf "$REMOTE" 2>/dev/null \
    | grep -E '^dr-[0-9]{8}T[0-9]{6}Z\.age$' \
    | sort \
    | tail -1
)"
[ -n "$latest" ] || { echo "no dr-*.age artifacts found at $REMOTE" >&2; exit 1; }

tmp="$(mktemp -d /tmp/mcnf-dr-verify.XXXXXX)"
cleanup() {
  if [ -n "${prefix:-}" ]; then
    delete_prefix "$prefix" || true
  fi
  rm -rf "$tmp"
}
trap cleanup EXIT

artifact="$tmp/$latest"
sidecar="$artifact.sha256"
rclone copyto "$REMOTE/$latest" "$artifact" >/dev/null
rclone copyto "$REMOTE/$latest.sha256" "$sidecar" >/dev/null

(cd "$tmp" && sha256sum -c "$(basename "$sidecar")")

prefix="$tmp/restore/"
"$HERE/dr-restore.sh" "$artifact" "$prefix"

summary="$(
  age -d -i "${MCNF_AGE_KEY:-/root/.mcnf-age-key}" <"$artifact" | python3 -c '
import json
import sys

m = json.load(sys.stdin)
version = m.get("dr_backup_version")
kv_count = m.get("kv_count")
file_count = m.get("file_count", 0)
missing_count = len(m.get("missing_files") or [])
print(f"version={version} kv={kv_count} files={file_count} missing_files={missing_count}")
'
)"

echo "verified off-fleet DR artifact: $REMOTE/$latest"
echo "$summary"
