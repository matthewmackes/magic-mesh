#!/usr/bin/env bash
# dr-snapshot-onmesh.sh — DAR-39: the ON-MESH FIRST LINE of DR. Produces a fresh
# dr-<ts>.age (dr-backup.sh) and copies it into the Syncthing-replicated Mesh-Sync
# root ($MCNF_MESHFS_DIR/dr/), so the first recovery line is present on every
# surviving peer with NO egress. Maintains dr/INDEX.json + N-deep retention.
#
# This is the leader-gated daily timer's payload (dr_scheduler worker / the Full-
# tier mcnf-dr-backup.timer points here). The OFF-FLEET push (DO Spaces) is a
# SEPARATE operator-run step (dr-push-offfleet.sh) — never fired here.
#
# Usage: dr-snapshot-onmesh.sh [--keep <n>] [--use <existing-dr.age>]
#   --use   copy an EXISTING dr-<ts>.age into the mesh dir instead of producing one
#           (so a single dr-backup can feed both off-fleet + on-mesh lines).
# Env (via dr-env.sh): MCNF_MESHFS_DIR (/mnt/mesh-storage), MCNF_DR_KEEP (14).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=./dr-env.sh
. "$HERE/dr-env.sh"

KEEP="${MCNF_DR_KEEP:-14}"
USE=""
while [ $# -gt 0 ]; do
  case "$1" in
    --keep) KEEP="$2"; shift 2 ;;
    --use)  USE="$2"; shift 2 ;;
    -h|--help) sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "dr-snapshot-onmesh: unknown arg '$1'" >&2; exit 2 ;;
  esac
done

MESH_DR="$MCNF_MESHFS_DIR/dr"

# Verify the path is writable + (best-effort) replicating BEFORE relying on it —
# a non-existent/full mesh-storage means the first recovery line silently isn't
# there. We mkdir + a write probe; a failure is loud + fatal (no false success).
mkdir -p "$MESH_DR" 2>/dev/null || { echo "dr-snapshot-onmesh: cannot create $MESH_DR (is $MCNF_MESHFS_DIR mounted + replicating?)" >&2; exit 1; }
probe="$MESH_DR/.write-probe.$$"
if ! ( : >"$probe" ) 2>/dev/null; then
  echo "dr-snapshot-onmesh: $MESH_DR is not writable — refusing (Mesh-Sync down?)" >&2; exit 1
fi
rm -f "$probe"

# Produce or reuse the artifact.
if [ -n "$USE" ]; then
  [ -f "$USE" ] || { echo "dr-snapshot-onmesh: --use '$USE' not found" >&2; exit 2; }
  SRC="$USE"
else
  echo "==> producing a fresh dr-<ts>.age (dr-backup.sh)"
  SRC="$(bash "$HERE/dr-backup.sh" | tail -1)"
  [ -f "$SRC" ] || { echo "dr-snapshot-onmesh: dr-backup.sh did not produce an artifact" >&2; exit 1; }
fi

BASE="$(basename "$SRC")"
DEST="$MESH_DR/$BASE"
cp -f "$SRC" "$DEST"
chmod 600 "$DEST"
SHA="$(sha256sum "$DEST" | awk '{print $1}')"
SIZE="$(stat -c%s "$DEST" 2>/dev/null || wc -c <"$DEST")"
echo "==> on-mesh first line: $DEST (sha256 ${SHA:0:12}…, $SIZE bytes)"

# Update INDEX.json: append/replace this artifact's row {ts,file,sha256,size,components}.
# components are read from the manifest's age header is not decryptable here (we
# don't hold the key in the worker path generically), so we record the file-level
# facts + the known v2 component names; a fuller listing is dr-reconstitute --verify.
INDEX="$MESH_DR/INDEX.json"
TS="$(echo "$BASE" | sed -E 's/^dr-(.*)\.age$/\1/')"
[ -n "$TS" ] || TS="$(date -u +%Y%m%dT%H%M%SZ)"
python3 - "$INDEX" "$BASE" "$TS" "$SHA" "$SIZE" <<'PY'
import sys, json, os
index_path, fname, ts, sha, size = sys.argv[1:6]
data = {"artifacts": []}
if os.path.exists(index_path):
    try:
        with open(index_path) as f: data = json.load(f)
    except Exception:
        data = {"artifacts": []}
arts = [a for a in data.get("artifacts", []) if a.get("file") != fname]
arts.append({
    "ts": ts, "file": fname, "sha256": sha, "size": int(size),
    "components": ["tofu-state", "secrets", "age-recipients", "forgejo-data", "etcd-snapshot"],
})
arts.sort(key=lambda a: a.get("ts", ""))
data["artifacts"] = arts
data["updated_utc"] = ts
with open(index_path, "w") as f: json.dump(data, f, indent=2)
PY
echo "==> updated $INDEX"

# Retention: keep the KEEP newest dr-*.age (+ keep INDEX.json in sync).
mapfile -t ALL < <(ls -1 "$MESH_DR"/dr-*.age 2>/dev/null | sort)
n="${#ALL[@]}"
if [ "$n" -gt "$KEEP" ]; then
  drop=$(( n - KEEP ))
  echo "==> retention: $n artifacts, keeping $KEEP newest — pruning $drop oldest"
  for f in "${ALL[@]:0:$drop}"; do
    rm -f "$f"
    fb="$(basename "$f")"
    python3 - "$INDEX" "$fb" <<'PY'
import sys, json, os
index_path, fname = sys.argv[1:3]
if not os.path.exists(index_path): sys.exit(0)
try:
    with open(index_path) as f: data = json.load(f)
except Exception: sys.exit(0)
data["artifacts"] = [a for a in data.get("artifacts", []) if a.get("file") != fname]
with open(index_path, "w") as f: json.dump(data, f, indent=2)
PY
  done
fi

echo "dr-snapshot-onmesh: done ($MESH_DR — $(ls -1 "$MESH_DR"/dr-*.age 2>/dev/null | wc -l) artifact(s))"
echo "NEXT (operator, off-fleet): automation/dr/dr-push-offfleet.sh   (--dry-run first)"
