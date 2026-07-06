#!/usr/bin/env bash
# DATACENTER-23 — disaster-recovery RESTORE of a dr-*.age manifest into etcd.
#
# Decrypts a manifest produced by dr-backup.sh with the mesh age identity and
# re-puts every key into etcd and restores any manifest file payloads, REWRITING
# the key/file prefixes. By default it restores under a TEMP prefix
# (/dr-restore-test/) so a round-trip can be verified without ever touching the
# live /tofu/, /mcnf/, or CA paths. Pass --prod to restore to the ORIGINAL keys
# and files (clobbers production) — deliberately opt-in.
#
# Prefix rewrite: each original key is mapped by replacing its leading "/" with
# the target prefix. File paths use the same rule. e.g. with target
# "/dr-restore-test/":
#   /tofu/state/xen-xapi  ->  /dr-restore-test/tofu/state/xen-xapi
#   /var/lib/mackesd/nebula-ca/ca.key -> /dr-restore-test/var/lib/mackesd/nebula-ca/ca.key
# With --prod the keys are restored verbatim (no rewrite).
#
# Usage:
#   dr-restore.sh <dr-file.age> [target-prefix]   # default /dr-restore-test/
#   dr-restore.sh <dr-file.age> --prod            # restore to original keys (DANGER)
#
# Env (via dr-env.sh, DAR-37): MCNF_ETCD (resolved from /etc/mackesd/etcd-endpoints,
# NO dead .192:2379 default), MCNF_AGE_KEY. Reads a v1 OR v2 manifest — both carry
# the top-level "entries" (tofu+secret+recipient kvs) this script re-puts.
set -euo pipefail

_DRR_HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=./dr-env.sh
. "$_DRR_HERE/dr-env.sh"
dr_require_etcd || exit 1
ETCD="$MCNF_ETCD"
# A single endpoint for the per-key re-puts (any member answers).
ETCD="${ETCD%%,*}"
KEY="${MCNF_AGE_KEY:-/root/.mcnf-age-key}"

FILE="${1:-}"
ARG2="${2:-/dr-restore-test/}"
[ -n "$FILE" ] || { echo "usage: $0 <dr-file.age> [target-prefix|--prod]" >&2; exit 2; }
[ -f "$FILE" ] || { echo "no such file: $FILE" >&2; exit 2; }

PROD=0
PREFIX="$ARG2"
if [ "$ARG2" = "--prod" ]; then
  PROD=1
  PREFIX=""
fi

b64() { base64 -w0; }

# Decrypt the manifest once. age -d -i is the inverse of the backup's age -r.
PLAIN="$(age -d -i "$KEY" <"$FILE")"

# Emit rewritten "key<TAB>value" lines (both base64) for each entry, ready to
# re-put. The prefix rewrite happens in python on the DECODED key, then re-b64.
# The manifest JSON is passed on stdin; PROD/PREFIX via the environment.
mapfile -t LINES < <(
  printf %s "$PLAIN" | PROD="$PROD" PREFIX="$PREFIX" python3 -c '
import sys, json, base64, os

m = json.load(sys.stdin)
prod = os.environ["PROD"] == "1"
prefix = os.environ["PREFIX"]

for e in m.get("entries", []):
    key = base64.b64decode(e["key"]).decode()
    if not prod:
        # map leading "/" -> target prefix; original key keeps the rest
        key = prefix + key.lstrip("/")
    new_key_b64 = base64.b64encode(key.encode()).decode()
    # value is already base64 as etcd returned it; re-put verbatim
    print(new_key_b64 + "\t" + e.get("value", ""))
'
)

COUNT=0
for line in "${LINES[@]}"; do
  [ -n "$line" ] || continue
  k="${line%%$'\t'*}"
  v="${line#*$'\t'}"
  curl -s -X POST "$ETCD/v3/kv/put" -d "{\"key\":\"$k\",\"value\":\"$v\"}" >/dev/null
  COUNT=$((COUNT + 1))
done

FILE_COUNT="$(
  printf %s "$PLAIN" | PROD="$PROD" PREFIX="$PREFIX" python3 -c '
import sys, json, base64, os, pathlib

m = json.load(sys.stdin)
prod = os.environ["PROD"] == "1"
prefix = os.environ["PREFIX"]
count = 0

for e in m.get("file_entries", []) or []:
    path = e.get("path") or ""
    value = e.get("value") or ""
    mode = e.get("mode") or "0600"
    if not path:
        continue
    target = path if prod else prefix + path.lstrip("/")
    data = base64.b64decode(value)
    p = pathlib.Path(target)
    p.parent.mkdir(parents=True, exist_ok=True)
    with open(p, "wb") as fh:
        fh.write(data)
    try:
        os.chmod(p, int(mode, 8))
    except Exception:
        os.chmod(p, 0o600)
    count += 1

print(count)
'
)"

if [ "$PROD" -eq 1 ]; then
  echo "restored $COUNT keys and $FILE_COUNT files to PRODUCTION (original keys/paths) from $FILE"
else
  echo "restored $COUNT keys and $FILE_COUNT files under prefix '$PREFIX' from $FILE"
fi
