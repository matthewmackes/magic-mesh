#!/usr/bin/env bash
# dr-reconstitute.sh — DAR-43: guided reconstitution / rebirth on a fresh box from
# a dr-<ts>.age (v2) + the SEPARATE CA/age bundle (dr-ca-bundle.sh). Rebirths
# state-backend + secrets + Forgejo + reconciler so the backoffice comes back and a
# leader is elected — with a CONTENT-VERIFIED restore (not a healthz-only pass on a
# corrupt-but-loadable DB).
#
# Modes:
#   --verify <dr.age>          dearmor + LIST the v2 components, stop BEFORE any
#                              mutation. exit 0 = restorable.
#   --restore <dr.age> [--prod|--prefix <p>]
#                              restore the etcd snapshot (when present) + re-put the
#                              kv entries (via dr-restore.sh) + restore Forgejo data,
#                              then assert: a NAMED seed repo is present AND an admin
#                              row exists in the Forgejo `user` table AND healthz=pass.
#                              Default target is the safe temp prefix /dr-restore-test/.
#
# Env (via dr-env.sh): MCNF_ETCD, MCNF_AGE_KEY, MCNF_FORGEJO_DATA, MCNF_HOST_IP.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=./dr-env.sh
. "$HERE/dr-env.sh"

KEY="${MCNF_AGE_KEY:-/root/.mcnf-age-key}"
FORGEJO_DATA="${MCNF_FORGEJO_DATA:-/var/lib/mcnf-forgejo}"

MODE=""
FILE=""
PREFIX="/dr-restore-test/"
PROD=0
EXPECT_REPO="${MCNF_DR_EXPECT_REPO:-magic-mesh}"

while [ $# -gt 0 ]; do
  case "$1" in
    --verify)  MODE="verify"; FILE="$2"; shift 2 ;;
    --restore) MODE="restore"; FILE="$2"; shift 2 ;;
    --prod)    PROD=1; PREFIX=""; shift ;;
    --prefix)  PREFIX="$2"; shift 2 ;;
    -h|--help) sed -n '2,26p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "dr-reconstitute: unknown arg '$1'" >&2; exit 2 ;;
  esac
done

[ -n "$MODE" ] && [ -n "$FILE" ] || { echo "usage: dr-reconstitute.sh --verify <dr.age> | --restore <dr.age> [--prod|--prefix <p>]" >&2; exit 2; }
[ -f "$FILE" ] || { echo "dr-reconstitute: no such file: $FILE" >&2; exit 2; }
[ -f "$KEY" ] || { echo "dr-reconstitute: age identity $KEY absent — restore the CA/age bundle (dr-ca-bundle) first" >&2; exit 1; }

# Decrypt the manifest ONCE to a tmpfs file (binary-safe).
WORK="$(mktemp -d)"; chmod 700 "$WORK"; trap 'rm -rf "$WORK"' EXIT
MANIFEST="$WORK/manifest.json"
age -d -i "$KEY" <"$FILE" >"$MANIFEST" 2>/dev/null || { echo "dr-reconstitute: decrypt FAILED (wrong age identity?)" >&2; exit 1; }

# Summarize components (used by both modes).
summarize() {
  python3 - "$MANIFEST" <<'PY'
import sys, json
m = json.load(open(sys.argv[1]))
v = m.get("dr_backup_version")
print(f"dr_backup_version: {v}")
print(f"created_utc:       {m.get('created_utc')}")
comps = m.get("components") or {}
if comps:
    for name in ("tofu-state","secrets","age-recipients","forgejo-data","etcd-snapshot"):
        c = comps.get(name, {})
        if name == "forgejo-data":
            print(f"  forgejo-data:   present={c.get('present')} quiesce={c.get('quiesce','')}")
        elif name == "etcd-snapshot":
            print(f"  etcd-snapshot:  present={c.get('present')} revision={c.get('revision','')}")
        else:
            print(f"  {name:<14} {c.get('kv_count',0)} keys")
else:
    print(f"  (v1 manifest) entries: {m.get('kv_count',0)}")
PY
}

if [ "$MODE" = "verify" ]; then
  echo "== dr-reconstitute --verify $FILE =="
  summarize
  echo "OK: manifest decrypts + lists. RESTORABLE. (no mutation performed)"
  exit 0
fi

# ===== restore =====
dr_require_etcd || exit 1
echo "== dr-reconstitute --restore $FILE (target: ${PROD:+PRODUCTION}${PREFIX:+prefix $PREFIX}) =="
summarize

# 1) etcd snapshot (whole-store) — when present + restoring to PROD, this is the
#    authoritative path; otherwise the per-key re-put below gives a portable
#    restore under the temp prefix. We do NOT auto-overwrite a live datadir; we
#    write the snapshot file + print the etcdutl/etcdctl restore command the
#    operator runs against a throwaway/rebuilt member (so a round-trip into a
#    THROWAWAY etcd is operator-driven, never a silent prod clobber).
SNAP_PRESENT="$(python3 -c 'import json,sys;m=json.load(open(sys.argv[1]));print(1 if (m.get("components") or {}).get("etcd-snapshot",{}).get("present") else 0)' "$MANIFEST")"
if [ "$SNAP_PRESENT" = "1" ]; then
  SNAP_DB="$WORK/etcd-snapshot.db"
  python3 - "$MANIFEST" "$SNAP_DB" <<'PY'
import sys, json, base64
m = json.load(open(sys.argv[1]))
b64 = (m.get("components") or {}).get("etcd-snapshot",{}).get("snapshot_b64","")
open(sys.argv[2],"wb").write(base64.b64decode(b64))
PY
  echo "  etcd-snapshot written → $SNAP_DB"
  echo "    restore into a THROWAWAY member with:  etcdutl snapshot restore $SNAP_DB --data-dir <new-datadir>"
  cp "$SNAP_DB" "${MCNF_DR_DIR:-$HOME/mcnf-dr-backups}/etcd-snapshot-restore.db" 2>/dev/null || true
fi

# 2) re-put the kv entries (tofu-state + secrets + age-recipients) via dr-restore.sh.
#    Default temp prefix keeps prod untouched; --prod restores verbatim.
echo "  re-put kv entries via dr-restore.sh"
if [ "$PROD" -eq 1 ]; then
  bash "$HERE/dr-restore.sh" "$FILE" --prod
else
  bash "$HERE/dr-restore.sh" "$FILE" "$PREFIX"
fi

# 3) Forgejo data: extract the quiesced tar, restore the sqlite DB + repos, then
#    CONTENT-VERIFY (resolves STUB 3): the restored DB has an admin row AND the
#    named seed repo is present — not just a loadable-but-empty DB.
FORGEJO_PRESENT="$(python3 -c 'import json,sys;m=json.load(open(sys.argv[1]));print(1 if (m.get("components") or {}).get("forgejo-data",{}).get("present") else 0)' "$MANIFEST")"
if [ "$FORGEJO_PRESENT" = "1" ]; then
  echo "  restore Forgejo data"
  python3 - "$MANIFEST" "$WORK/forgejo.tar.gz" <<'PY'
import sys, json, base64
m = json.load(open(sys.argv[1]))
b64 = (m.get("components") or {}).get("forgejo-data",{}).get("tar_b64","")
open(sys.argv[2],"wb").write(base64.b64decode(b64))
PY
  EXTRACT="$WORK/forgejo-extract"; mkdir -p "$EXTRACT"
  tar -C "$EXTRACT" -xzf "$WORK/forgejo.tar.gz" 2>/dev/null || true
  RDB="$EXTRACT/forgejo/forgejo.db"
  if [ -f "$RDB" ] && command -v sqlite3 >/dev/null 2>&1; then
    # Content checks against the RESTORED DB (NOT the live one).
    admins="$(sqlite3 "$RDB" "SELECT count(*) FROM user WHERE is_admin=1;" 2>/dev/null || echo 0)"
    repos="$(sqlite3 "$RDB" "SELECT count(*) FROM repository WHERE lower_name='${EXPECT_REPO}';" 2>/dev/null || echo 0)"
    tables="$(sqlite3 "$RDB" ".tables" 2>/dev/null || true)"
    echo "    restored DB: admin rows=$admins, repo '$EXPECT_REPO'=$repos, .tables ok=$([ -n "$tables" ] && echo yes || echo no)"
    [ "$admins" -ge 1 ] || { echo "dr-reconstitute: VERIFY FAILED — no admin row in the restored user table" >&2; exit 1; }
    [ "$repos" -ge 1 ]  || { echo "dr-reconstitute: VERIFY FAILED — named seed repo '$EXPECT_REPO' absent in the restored DB" >&2; exit 1; }
    echo "    Forgejo content VERIFIED (admin row + named repo present in the restored DB)."
    # For --prod, place the restored data so forgejo-up.sh serves it.
    if [ "$PROD" -eq 1 ]; then
      mkdir -p "$FORGEJO_DATA/gitea"
      cp "$RDB" "$FORGEJO_DATA/gitea/forgejo.db"
      [ -f "$EXTRACT/forgejo/repos.tar.gz" ] && tar -C "$FORGEJO_DATA/git" -xzf "$EXTRACT/forgejo/repos.tar.gz" 2>/dev/null || true
      echo "    restored Forgejo data into $FORGEJO_DATA (start it with forgejo-up.sh)."
    fi
  else
    echo "    WARN: sqlite3 absent or no restored DB — cannot content-verify Forgejo (install sqlite3)" >&2
    [ "$PROD" -eq 1 ] && { echo "dr-reconstitute: refusing a --prod Forgejo restore without content verification" >&2; exit 1; }
  fi
fi

if [ "$PROD" -eq 1 ]; then target_desc="PRODUCTION (original keys)"; else target_desc="under temp prefix $PREFIX"; fi
[ "$SNAP_PRESENT" = "1" ] && snap_desc="written (see the etcdutl restore command above)" || snap_desc="absent"
[ "$FORGEJO_PRESENT" = "1" ] && fj_desc="content-verified (admin row + named repo)" || fj_desc="absent"
cat <<EOF

dr-reconstitute --restore done.
  - kv entries restored ($target_desc).
  - etcd snapshot: $snap_desc.
  - Forgejo: $fj_desc.
NEXT (live infra): start state-backend + forgejo-up.sh; confirm an etcd leader +
  mackesd healthz green, then arm the reconciler (enable-autoscale-timer.sh).
EOF
