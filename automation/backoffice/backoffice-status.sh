#!/usr/bin/env bash
# backoffice-status.sh — DAR-44: a single command + a mesh-published health record
# for the backoffice ITSELF. Per the critique (resolves STUB 4): the reconcile
# health is read from etcd /reconciler/* — NOT a host-local automation/.state/
# farm-status.txt that won't exist on a fresh control VM. Every other component is
# probed live; a partially-down backoffice reports that component unhealthy rather
# than erroring out.
#
# Emits per-component health as JSON:
#   state-backend  :8390 reachable over the overlay (HTTP, any 2xx/404 = up)
#   secret-store   mcnf-secret.sh list succeeds (store readable with this node's key)
#   units          each tier systemd unit's is-active state
#   reconcile      last rev+outcome+ts from etcd /reconciler/last-reconcile
#                  (unknown/never on a brand-new VM with empty /reconciler/* — NOT an error)
#   forgejo        :3000/api/healthz
#   dr             newest artifact age from $MCNF_MESHFS_DIR/dr/INDEX.json
#
# Usage: backoffice-status.sh [--json] [--host <overlay-ip>]
#   --json  emit JSON (default also prints a human summary to stderr)
# Env (via dr-env.sh + reconciler-state.sh): MCNF_ETCD, MCNF_MESHFS_DIR, MCNF_HOST_IP.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SRC_REPO="$(cd "$HERE/../.." && pwd)"
RECON_STATE="$SRC_REPO/automation/reconciler/reconciler-state.sh"
SECRET="$SRC_REPO/automation/secrets/mcnf-secret.sh"

JSON=0
HOST_IP="${MCNF_HOST_IP:-}"
while [ $# -gt 0 ]; do
  case "$1" in
    --json)    JSON=1; shift ;;
    --host)    HOST_IP="$2"; shift 2 ;;
    -h|--help) sed -n '2,24p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) shift ;;
  esac
done

detect_overlay() { ip -o -4 addr show 2>/dev/null | awk '$2 ~ /nebula|mde-neb/ {split($4,a,"/"); print a[1]; exit}'; }
[ -n "$HOST_IP" ] || HOST_IP="$(detect_overlay)"
[ -n "$HOST_IP" ] || HOST_IP="127.0.0.1"

MESHFS_DIR="${MCNF_MESHFS_DIR:-/mnt/mesh-storage}"

# ── component probes (each best-effort; never aborts — a down component reports
#    its own unhealthy state, never crashes the status run) ──

# state-backend :8390 — any HTTP response (200/404) means it's up; refused = down.
probe_state_backend() {
  local code
  code="$(curl -s -o /dev/null -w '%{http_code}' --max-time 4 "http://${HOST_IP}:8390/state/_status" 2>/dev/null || echo 000)"
  case "$code" in 2*|404) echo up ;; *) echo down ;; esac
}

# secret store — list succeeds (the store is readable with THIS node's age key).
probe_secret_store() {
  if bash "$SECRET" list >/dev/null 2>&1; then echo ok; else echo unreadable; fi
}

# a systemd unit's active state (active/inactive/failed/unknown); never errors.
unit_state() { systemctl is-active "$1" 2>/dev/null || echo unknown; }

# reconcile health — from etcd /reconciler/last-reconcile (DAR-44: NOT a host txt).
# On a brand-new VM with an empty /reconciler/* the key is absent → report
# outcome=never / rev=unknown rather than erroring.
probe_reconcile() {
  local raw
  raw="$(bash "$RECON_STATE" get last-reconcile 2>/dev/null || true)"
  if [ -z "$raw" ]; then
    printf '{"rev":"unknown","outcome":"never","ts":null}'
    return 0
  fi
  # Validate it's JSON; if garbage, report unknown rather than emitting bad JSON.
  printf '%s' "$raw" | python3 -c 'import sys,json
try:
    d=json.load(sys.stdin)
    print(json.dumps({"rev":d.get("rev","unknown"),"outcome":d.get("outcome","unknown"),"ts":d.get("ts")}))
except Exception:
    print(json.dumps({"rev":"unknown","outcome":"unknown","ts":None}))'
}

# forgejo health — :3000/api/healthz.
probe_forgejo() {
  if curl -s --max-time 4 "http://${HOST_IP}:3000/api/healthz" 2>/dev/null | grep -q pass; then echo pass; else echo down; fi
}

# DR — newest artifact + its age (hours) from the on-mesh INDEX.json.
probe_dr() {
  local idx="$MESHFS_DIR/dr/INDEX.json"
  if [ ! -f "$idx" ]; then printf '{"last_artifact":null,"age_hours":null}'; return 0; fi
  python3 - "$idx" <<'PY'
import sys, json, datetime, os
try:
    d = json.load(open(sys.argv[1]))
    arts = sorted(d.get("artifacts", []), key=lambda a: a.get("ts",""))
    if not arts:
        print(json.dumps({"last_artifact": None, "age_hours": None})); sys.exit(0)
    last = arts[-1]
    ts = last.get("ts","")  # YYYYMMDDTHHMMSSZ
    age_h = None
    try:
        t = datetime.datetime.strptime(ts, "%Y%m%dT%H%M%SZ").replace(tzinfo=datetime.timezone.utc)
        age_h = round((datetime.datetime.now(datetime.timezone.utc) - t).total_seconds()/3600, 1)
    except Exception:
        pass
    print(json.dumps({"last_artifact": last.get("file"), "age_hours": age_h}))
except Exception:
    print(json.dumps({"last_artifact": None, "age_hours": None}))
PY
}

SB="$(probe_state_backend)"
SS="$(probe_secret_store)"
RECON_JSON="$(probe_reconcile)"
FJ="$(probe_forgejo)"
DR_JSON="$(probe_dr)"
U_STATE_BACKEND="$(unit_state mcnf-state-backend.service)"
U_FORGEJO="$(unit_state mcnf-forgejo-runner.service)"
U_AUTOSCALE="$(unit_state mcnf-farm-autoscale-reconcile.timer)"
U_BUILD="$(unit_state mcnf-farm-reconcile.timer)"
U_DR="$(unit_state mcnf-dr-backup.timer)"

# Assemble the record. The overall "healthy" flag is a conservative AND of the
# probes that are EXPECTED up (state-backend + secret store); reconcile=never and a
# down optional unit do not flip healthy=false (a Minimal tier / fresh VM is fine).
RECORD="$(
  HOST_IP="$HOST_IP" SB="$SB" SS="$SS" FJ="$FJ" \
  U_STATE_BACKEND="$U_STATE_BACKEND" U_FORGEJO="$U_FORGEJO" \
  U_AUTOSCALE="$U_AUTOSCALE" U_BUILD="$U_BUILD" U_DR="$U_DR" \
  RECON_JSON="$RECON_JSON" DR_JSON="$DR_JSON" \
  python3 -c '
import os, json, datetime
rec = {
  "host": os.environ["HOST_IP"],
  "checked_utc": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
  "state_backend": os.environ["SB"],
  "secret_store": os.environ["SS"],
  "reconcile": json.loads(os.environ["RECON_JSON"]),
  "forgejo": os.environ["FJ"],
  "dr": json.loads(os.environ["DR_JSON"]),
  "units": {
    "mcnf-state-backend.service": os.environ["U_STATE_BACKEND"],
    "mcnf-forgejo-runner.service": os.environ["U_FORGEJO"],
    "mcnf-farm-autoscale-reconcile.timer": os.environ["U_AUTOSCALE"],
    "mcnf-farm-reconcile.timer": os.environ["U_BUILD"],
    "mcnf-dr-backup.timer": os.environ["U_DR"],
  },
}
rec["healthy"] = (rec["state_backend"] == "up" and rec["secret_store"] == "ok")
print(json.dumps(rec, indent=2))
'
)"

if [ "$JSON" -eq 1 ]; then
  printf '%s\n' "$RECORD"
else
  printf '%s\n' "$RECORD"
  {
    echo "backoffice-status @ $HOST_IP:"
    echo "  state-backend=$SB  secret-store=$SS  forgejo=$FJ"
    echo "  reconcile=$(printf '%s' "$RECON_JSON" | python3 -c 'import sys,json;d=json.load(sys.stdin);print(d["outcome"]+" @ "+str(d["rev"]))')"
    echo "  dr=$(printf '%s' "$DR_JSON" | python3 -c 'import sys,json;d=json.load(sys.stdin);print(str(d["last_artifact"])+" ("+str(d["age_hours"])+"h)")')"
  } >&2
fi
