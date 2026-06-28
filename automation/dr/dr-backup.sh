#!/usr/bin/env bash
# DATACENTER-23 / DAR-38 — disaster-recovery BACKUP of the no-fixed-center
# substrate, MANIFEST v2.
#
# The substrate has no single recoverable center: the OpenTofu IaC state and the
# mesh secrets live in the replicated etcd store, and (Full tier) the Forgejo CI
# DB lives on the control VM. This dumps the RECOVERABLE subset — enough to rebuild
# the world — into a single age-encrypted manifest, with FIVE v2 components:
#
#   tofu-state     /tofu/state/*  (incl. /tofu/state/edgeos — DAR-9b)
#   secrets        /mcnf/secret/* (ALREADY age-encrypted → age-in-age)
#   age-recipients /mcnf/age-recipient + /mcnf/age-recipients/* (so a restore re-seals)
#   forgejo-data   the Forgejo sqlite DB + repos, captured with a SQLITE QUIESCE so
#                  the restore is consistent (NOT corrupt-but-loadable)
#   etcd-snapshot  a point-in-time etcd v3 snapshot from the LEADER endpoint, for
#                  whole-store reconstitution (revision recorded)
#
# Read-only on the live store (the snapshot is a maintenance read). Endpoints +
# paths come from dr-env.sh (DAR-37) — NO dead http://172.20.145.192:2379 default.
#
# CAVEAT (printed): the mesh age IDENTITY (private key) + the Nebula CA can NOT be
# recovered from this file — the master key cannot live inside the thing it
# decrypts. Back those up SEPARATELY via dr-ca-bundle.sh (DAR-42, operator-run).
#
# Usage: dr-backup.sh
# Env (via dr-env.sh): MCNF_ETCD, MCNF_AGE_KEY, MCNF_DR_DIR, MCNF_FORGEJO_DATA.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=./dr-env.sh
. "$HERE/dr-env.sh"
dr_require_etcd || exit 1

ETCD="$MCNF_ETCD"
# A single endpoint for the per-key range pulls (any member answers); the snapshot
# uses the LEADER (resolved below).
ETCD1="${ETCD%%,*}"
KEY="${MCNF_AGE_KEY:-/root/.mcnf-age-key}"
DR_DIR="${MCNF_DR_DIR:-$HOME/mcnf-dr-backups}"
FORGEJO_DATA="${MCNF_FORGEJO_DATA:-/var/lib/mcnf-forgejo}"

b64() { base64 -w0; }

# Dump every key under a prefix via the v3 range API (read-only). range_end is the
# prefix with its last byte incremented (the "<prefix>0" trick) for the open-ended
# scan. Emits the raw {"kvs":[{key,value},...]} with base64 keys/values as-is.
range_prefix() { # <endpoint> <prefix> <range_end>
  local s e
  s=$(printf %s "$2" | b64); e=$(printf %s "$3" | b64)
  curl -s -X POST "$1/v3/kv/range" -d "{\"key\":\"$s\",\"range_end\":\"$e\"}"
}
range_key() { # <endpoint> <key>
  local k; k=$(printf %s "$2" | b64)
  curl -s -X POST "$1/v3/kv/range" -d "{\"key\":\"$k\"}"
}

# ── leader endpoint for the consistent snapshot ──
# Ask each endpoint /v3/maintenance/status for its leader memberID, then find the
# member whose ID matches. If we can't resolve a leader, fall back to ETCD1 (still
# a consistent point-in-time snapshot of that member). NEVER fails the backup.
resolve_leader_endpoint() {
  local ep status leader_id
  IFS=',' read -ra eps <<<"$ETCD"
  for ep in "${eps[@]}"; do
    status="$(curl -s -X POST "$ep/v3/maintenance/status" -d '{}' 2>/dev/null || true)"
    leader_id="$(printf '%s' "$status" | python3 -c 'import sys,json
try: print(json.load(sys.stdin).get("leader",""))
except Exception: pass' 2>/dev/null || true)"
    # The endpoint whose own header.member_id == leader is the leader endpoint.
    local self_id
    self_id="$(printf '%s' "$status" | python3 -c 'import sys,json
try: print(json.load(sys.stdin).get("header",{}).get("member_id",""))
except Exception: pass' 2>/dev/null || true)"
    if [ -n "$leader_id" ] && [ "$leader_id" = "$self_id" ]; then
      printf '%s' "$ep"; return 0
    fi
  done
  printf '%s' "$ETCD1"
}

TS="$(date -u +%Y%m%dT%H%M%SZ)"
RECIP="$(age-keygen -y "$KEY" 2>/dev/null)"
[ -n "$RECIP" ] || { echo "dr-backup: cannot derive recipient from $KEY" >&2; exit 1; }

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# ── component 1-3: tofu state, secrets, age recipients ──
# DAR-1: the tofu-state prefix is the SAME canonical /tofu/state/ the state backend
# (tofu-state-etcd.py CANONICAL_STATE_PREFIX) writes under, and BOTH honor the same
# STATE_PREFIX override so they can never drift — a DR range over STATE_PREFIX reads
# exactly what the backend put there. The range_end is the prefix sans trailing
# slash + the next byte (the "<…>0" open-ended-scan trick) computed from STATE_PREFIX
# so a custom prefix scans correctly too.
STATE_PREFIX="${STATE_PREFIX:-/tofu/state/}"
case "$STATE_PREFIX" in */) ;; *) STATE_PREFIX="$STATE_PREFIX/" ;; esac
# range_end = prefix with the trailing slash bumped to the next byte (slash=0x2f → '0'=0x30).
STATE_RANGE_END="${STATE_PREFIX%/}0"
TOFU_JSON="$(range_prefix "$ETCD1" "$STATE_PREFIX" "$STATE_RANGE_END")"
SECRET_JSON="$(range_prefix "$ETCD1" "/mcnf/secret/" "/mcnf/secret0")"
RECIP_LEGACY_JSON="$(range_key "$ETCD1" "/mcnf/age-recipient")"
RECIP_SET_JSON="$(range_prefix "$ETCD1" "/mcnf/age-recipients/" "/mcnf/age-recipients0")"

# ── component 4: Forgejo data with a SQLITE QUIESCE (DAR-38) ──
# A naive tar of a live sqlite DB can capture a torn write (corrupt-but-loadable).
# We quiesce: prefer `sqlite3 .backup` (an online, consistent copy WITHOUT stopping
# Forgejo); if sqlite3 is unavailable, fall back to a WAL checkpoint + copy; the
# documented hard-consistent option is to stop Forgejo, tar, restart. We then tar
# the repos (excluding work-dirs / build caches). FORGEJO_B64 stays empty if there
# is no Forgejo data on this node (a non-CI node).
FORGEJO_B64=""
FORGEJO_NOTE="absent"
DB="$FORGEJO_DATA/gitea/forgejo.db"
if [ -f "$DB" ]; then
  mkdir -p "$WORK/forgejo"
  if command -v sqlite3 >/dev/null 2>&1; then
    # Online consistent snapshot — Forgejo keeps running, the .backup is atomic.
    if sqlite3 "$DB" ".backup '$WORK/forgejo/forgejo.db'" 2>/dev/null; then
      FORGEJO_NOTE="sqlite3 .backup (online consistent)"
    else
      # WAL checkpoint then plain copy (best-effort consistency).
      sqlite3 "$DB" "PRAGMA wal_checkpoint(TRUNCATE);" >/dev/null 2>&1 || true
      cp "$DB" "$WORK/forgejo/forgejo.db"; FORGEJO_NOTE="wal-checkpoint + copy"
    fi
  else
    # No sqlite3: WAL files may exist; copy the DB + any -wal/-shm so a restore can
    # replay. The documented hard-consistent path (stop Forgejo, tar) is in the README.
    cp "$DB" "$WORK/forgejo/forgejo.db"
    [ -f "$DB-wal" ] && cp "$DB-wal" "$WORK/forgejo/" || true
    [ -f "$DB-shm" ] && cp "$DB-shm" "$WORK/forgejo/" || true
    FORGEJO_NOTE="copy + WAL/SHM (sqlite3 absent — see README for the stop-tar consistent path)"
  fi
  # Repos (the git data), excluding transient work-dirs + build caches.
  if [ -d "$FORGEJO_DATA/git/repositories" ]; then
    tar -C "$FORGEJO_DATA/git" \
        --exclude='*/tmp' --exclude='*/.cache' \
        -czf "$WORK/forgejo/repos.tar.gz" repositories 2>/dev/null || true
  fi
  FORGEJO_B64="$(tar -C "$WORK" -czf - forgejo 2>/dev/null | b64)"
fi

# ── component 5: consistent etcd v3 snapshot from the LEADER (DAR-38) ──
# /v3/maintenance/snapshot streams a point-in-time bbolt snapshot. We capture it +
# its revision (from /v3/maintenance/status on the same endpoint). If the endpoint
# can't stream it (older etcd / no maintenance API), the snapshot stays empty and
# the per-key manifest above still gives a portable restore — the backup never fails.
LEADER_EP="$(resolve_leader_endpoint)"
SNAP_B64=""; SNAP_REV=""
SNAP_RAW="$WORK/etcd-snapshot.db"
if curl -s -X POST "$LEADER_EP/v3/maintenance/snapshot" -d '{}' -o "$SNAP_RAW" 2>/dev/null && [ -s "$SNAP_RAW" ]; then
  # The HTTP snapshot stream frames each chunk as a JSON line {"result":{"blob":"<b64>"}}.
  # Decode + concatenate the blobs into the raw bbolt file, then re-b64 the whole thing.
  SNAP_BLOB="$WORK/etcd-snapshot.bin"
  python3 - "$SNAP_RAW" "$SNAP_BLOB" <<'PY' 2>/dev/null || true
import sys, json, base64
src, dst = sys.argv[1], sys.argv[2]
with open(src) as f, open(dst, "wb") as o:
    for line in f:
        line = line.strip()
        if not line:
            continue
        try:
            d = json.loads(line)
        except Exception:
            continue
        blob = (d.get("result") or {}).get("blob")
        if blob:
            o.write(base64.b64decode(blob))
PY
  if [ -s "$SNAP_BLOB" ]; then
    SNAP_B64="$(b64 < "$SNAP_BLOB")"
    SNAP_REV="$(curl -s -X POST "$LEADER_EP/v3/maintenance/status" -d '{}' 2>/dev/null \
      | python3 -c 'import sys,json
try: print(json.load(sys.stdin).get("header",{}).get("revision",""))
except Exception: pass' 2>/dev/null || true)"
  fi
fi

# ── assemble the v2 manifest ──
MANIFEST="$WORK/manifest.json"
TS="$TS" RECIP="$RECIP" FORGEJO_B64="$FORGEJO_B64" FORGEJO_NOTE="$FORGEJO_NOTE" \
SNAP_B64="$SNAP_B64" SNAP_REV="$SNAP_REV" LEADER_EP="$LEADER_EP" \
python3 - "$TOFU_JSON" "$SECRET_JSON" "$RECIP_LEGACY_JSON" "$RECIP_SET_JSON" >"$MANIFEST" <<'PY'
import sys, json, os

def kvs(raw):
    try: d = json.loads(raw)
    except Exception: return []
    return [{"key": kv["key"], "value": kv.get("value", "")} for kv in (d.get("kvs") or [])]

tofu, secret, recip_legacy, recip_set = (kvs(a) for a in sys.argv[1:5])

manifest = {
    "dr_backup_version": 2,
    "created_utc": os.environ["TS"],
    "age_recipient": os.environ["RECIP"],
    "components": {
        "tofu-state":     {"kv_count": len(tofu),         "entries": tofu},
        "secrets":        {"kv_count": len(secret),       "entries": secret},
        "age-recipients": {"kv_count": len(recip_legacy) + len(recip_set),
                            "entries": recip_legacy + recip_set},
        "forgejo-data":   {"present": bool(os.environ.get("FORGEJO_B64")),
                            "quiesce": os.environ.get("FORGEJO_NOTE", ""),
                            "tar_b64": os.environ.get("FORGEJO_B64", "")},
        "etcd-snapshot":  {"present": bool(os.environ.get("SNAP_B64")),
                            "revision": os.environ.get("SNAP_REV", ""),
                            "leader_endpoint": os.environ.get("LEADER_EP", ""),
                            "snapshot_b64": os.environ.get("SNAP_B64", "")},
    },
    # Back-compat: a flat kv_count + the tofu+secret+recipient entries so the v1
    # dr-restore.sh (which reads top-level "entries") still re-puts them.
    "kv_count": len(tofu) + len(secret) + len(recip_legacy) + len(recip_set),
    "entries": tofu + secret + recip_legacy + recip_set,
}
json.dump(manifest, sys.stdout)
PY

mkdir -p "$DR_DIR"
OUT="$DR_DIR/dr-$TS.age"
age -r "$RECIP" <"$MANIFEST" >"$OUT"
chmod 600 "$OUT"

echo "$OUT"
{
  echo "dr-backup v2 → $OUT"
  echo "  tofu-state:     $(python3 -c 'import json;print(json.load(open("'"$MANIFEST"'"))["components"]["tofu-state"]["kv_count"])') keys"
  echo "  secrets:        $(python3 -c 'import json;print(json.load(open("'"$MANIFEST"'"))["components"]["secrets"]["kv_count"])') keys"
  echo "  age-recipients: $(python3 -c 'import json;print(json.load(open("'"$MANIFEST"'"))["components"]["age-recipients"]["kv_count"])') keys"
  echo "  forgejo-data:   ${FORGEJO_NOTE}"
  if [ -n "$SNAP_B64" ]; then
    echo "  etcd-snapshot:  present rev=$SNAP_REV (leader $LEADER_EP)"
  else
    echo "  etcd-snapshot:  absent (per-key manifest still restorable)"
  fi
  echo "NOTE: the mesh age key ($KEY) + the Nebula CA are the master artifacts this"
  echo "      manifest can NOT carry (a key cannot live inside the thing it decrypts)."
  echo "      Back them up SEPARATELY via: automation/dr/dr-ca-bundle.sh (operator-run)."
} >&2
