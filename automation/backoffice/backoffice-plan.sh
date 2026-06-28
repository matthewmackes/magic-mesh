#!/usr/bin/env bash
# backoffice-plan.sh — DAR-15: the declarative, READ-ONLY backoffice planner.
#
# Reads the tier manifest (manifest.<tier>.toml) and prints the EXACT ordered unit
# list the orchestrator (backoffice-up.sh) would enable, as JSON, with ZERO
# mutations. This is the single source of truth for "what would the backoffice
# bring up" — the genesis-wizard `action/dc/backoffice-plan` RPC (DAR-45) shells
# out to THIS script so the wizard renders the REAL rendered plan, never a canned
# list (the acceptance asserts the RPC output matches this script's output).
#
# Output shape (stable; the wizard + tests parse it):
#   {
#     "ok": true,
#     "tier": "full",
#     "secrets_ready": true,                  # do-token present in the store (bool only)
#     "units": [
#       {"id":"precheck","phase":0,"ready":false,"live_gated":false,
#        "via_script":"automation/state-backend/state-backend-bootstrap.sh"},
#       ...
#     ]
#   }
#
# Every `via_script` is asserted to resolve to an existing repo file (a dangling
# reference makes that unit `"resolves":false`, and the script exits non-zero so a
# broken manifest is caught). `ready` is a best-effort liveness probe (read-only:
# a curl HEAD / systemctl is-active) — never a mutation. Re-running leaves
# etcd/services unchanged.
#
# Usage:
#   backoffice-plan.sh --tier {minimal|full} [--control-ip <ip>] [--manifest-dir <d>]
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
# shellcheck source=../lib/control-host.sh
. "$REPO/automation/lib/control-host.sh"   # DAR-17: portable control-HOST resolver

TIER=""
CONTROL_IP="${MCNF_CONTROL_IP:-}"
MANIFEST_DIR="$HERE"

while [ $# -gt 0 ]; do case "$1" in
  --tier)         TIER="$2"; shift 2;;
  --control-ip)   CONTROL_IP="$2"; shift 2;;
  --manifest-dir) MANIFEST_DIR="$2"; shift 2;;
  -h|--help)      sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'; exit 0;;
  *) echo "backoffice-plan: unknown arg: $1" >&2; exit 2;;
esac; done

# err prints a JSON error object on stdout (the RPC caller decodes {"error":..}).
err() { printf '{"error":"%s"}\n' "$1"; exit "${2:-2}"; }

case "$TIER" in
  minimal|full) ;;
  "") err "missing --tier {minimal|full}";;
  *)  err "invalid tier '$TIER' (expected minimal|full)";;
esac
MANIFEST="$MANIFEST_DIR/manifest.$TIER.toml"
[ -r "$MANIFEST" ] || err "missing manifest $MANIFEST"

# DAR-17: resolve the control HOST for the readiness probes via the shared chain
# (explicit env > the per-mesh /mcnf/site doc > the peer directory > this node's
# overlay), NEVER the dead .192. An explicit =172.20.145.192 is the reconstitute arm.
CONTROL_IP="$(MCNF_CONTROL_IP="$CONTROL_IP" mcnf_resolve_control_host)"

# Best-effort READ-ONLY liveness probe per unit id. Mutates nothing.
_unit_ready() { # <id> -> echoes true|false
  case "$1" in
    state-backend)
      if [ -n "$CONTROL_IP" ] && curl -s -o /dev/null --max-time 3 \
           "http://$CONTROL_IP:8390/state/__readiness__" 2>/dev/null; then echo true; else echo false; fi;;
    forgejo)
      if [ -n "$CONTROL_IP" ] && curl -fsS --max-time 3 \
           "http://$CONTROL_IP:3000/api/healthz" >/dev/null 2>&1; then echo true; else echo false; fi;;
    forgejo-runner)
      if systemctl is-active --quiet mcnf-forgejo-runner 2>/dev/null; then echo true; else echo false; fi;;
    secrets-init-self)
      # The VM's own age identity exists?
      if [ -f "${MCNF_AGE_KEY:-/root/.mcnf-age-key}" ]; then echo true; else echo false; fi;;
    *) echo false;;
  esac
}

# do-token presence (boolean only — the credential is NEVER read). Mirrors the
# genesis_plan secret-presence probe. Best-effort: a tooling failure → false.
_secrets_ready() {
  if "$REPO/automation/secrets/mcnf-secret.sh" list 2>/dev/null | grep -qx 'do-token'; then
    echo true
  else
    echo false
  fi
}

# Parse the manifest → TAB lines (phase, id, live_gated, via_script), in order.
_parse_manifest() { # <manifest>
  python3 - "$1" <<'PY'
import sys
path = sys.argv[1]
try:
    import tomllib
    with open(path, "rb") as f:
        doc = tomllib.load(f)
    units = doc.get("unit", [])
    for u in units:
        print("%s\t%s\t%s\t%s" % (
            u.get("phase", ""), u.get("id", ""),
            "true" if u.get("live_gated") else "false",
            u.get("via_script", "")))
    sys.exit(0)
except ModuleNotFoundError:
    pass
cur, units = {}, []
def flush():
    if cur: units.append(dict(cur))
    cur.clear()
with open(path) as f:
    for raw in f:
        line = raw.strip()
        if line.startswith("#") or not line: continue
        if line == "[[unit]]": flush(); continue
        if "=" in line and not line.startswith("["):
            k, _, v = line.partition("=")
            k, v = k.strip(), v.strip()
            if v and v[0] in "\"'" and v[-1:] == v[0]: v = v[1:-1]
            cur[k] = v
flush()
for u in units:
    print("%s\t%s\t%s\t%s" % (
        u.get("phase",""), u.get("id",""),
        u.get("live_gated","false"), u.get("via_script","")))
PY
}

SECRETS_READY="$(_secrets_ready)"

# Build the JSON. We assemble the units array with printf, sorting by phase. A
# dangling via_script flips a flag so we can exit non-zero AFTER emitting the JSON
# (so the caller still gets a parseable body that names the broken unit).
dangling=0
units_json=""
first=1
while IFS=$'\t' read -r phase id gated via; do
  [ -n "$id" ] || continue
  resolves=true
  if [ ! -e "$REPO/$via" ]; then resolves=false; dangling=1; fi
  ready="$(_unit_ready "$id")"
  row="$(printf '{"id":"%s","phase":%s,"ready":%s,"live_gated":%s,"resolves":%s,"via_script":"%s"}' \
    "$id" "$phase" "$ready" "$gated" "$resolves" "$via")"
  if [ "$first" -eq 1 ]; then units_json="$row"; first=0; else units_json="$units_json,$row"; fi
done < <(_parse_manifest "$MANIFEST" | sort -s -t$'\t' -k1,1n)

printf '{"ok":true,"tier":"%s","secrets_ready":%s,"units":[%s]}\n' \
  "$TIER" "$SECRETS_READY" "$units_json"

# A broken manifest (dangling via_script) is a hard error so CI/the wizard catches it.
if [ "$dangling" -eq 1 ]; then
  echo "backoffice-plan: a via_script does not resolve to an existing file (see resolves:false)" >&2
  exit 1
fi
exit 0
