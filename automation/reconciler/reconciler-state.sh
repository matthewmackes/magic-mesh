#!/usr/bin/env bash
# reconciler-state.sh — DAR-28: durable reconciler state in the mesh etcd under
# /reconciler/*, OFF the host-local /var/lib so a control-VM rebuild or fail-over
# resumes reconciliation without losing hysteresis (busy-state, dwell, last-good
# shapes, last reconcile rev+outcome).
#
# WHY etcd: the old reconciler kept its busy-state in a host-local file
# (/var/lib/mcnf-farm/farm-busy-state). On a fresh control VM that file does not
# exist, so the loop forgot which VMs were mid-build and which tasks had a fresh
# result — losing the FA-4 hysteresis that prevents thrash. Moving it into the
# replicated etcd store (the same quorum that carries /tofu/state + /mcnf/secret)
# means any control VM that comes up reads the live reconcile memory.
#
# The etcd v3 HTTP KV layer is the SAME pattern mcnf-secret.sh uses (base64
# keys/values over /v3/kv/{put,range,txn}), so there is no new transport. Values
# under /reconciler/* are NON-secret (VM names, timestamps, shape strings, revs) —
# this helper NEVER stores a credential (those stay in /mcnf/secret/* via
# mcnf-secret.sh). It is therefore plain put/get; no age layer.
#
# Endpoints resolve via the shared DAR-1b resolver (automation/lib/etcd-endpoints.sh)
# → explicit MCNF_ETCD → /etc/mackesd/etcd-endpoints → FAIL LOUD. No dead .192:2379.
#
# Usage:
#   reconciler-state.sh get <key>                 print value (exit 3 if absent)
#   reconciler-state.sh put <key> [<value>]       value from arg or stdin
#   reconciler-state.sh del <key>
#   reconciler-state.sh cas <key> <expected> <new> compare-and-swap (exit 0 won, 1 lost)
#   reconciler-state.sh ensure-prefix             touch /reconciler/.init (idempotent)
#   reconciler-state.sh list                      list keys under /reconciler/
#   reconciler-state.sh selftest                  offline self-test (mock etcd dir)
#
# Keys this helper carries (the reconciler writes these; see farm-reconciler.sh):
#   /reconciler/farm-busy-state   newline-joined VM names building last tick
#   /reconciler/last-reconcile    JSON {rev,outcome,ts} — read by backoffice-status (DAR-44)
#   /reconciler/.init             ensure-prefix sentinel
#
# Env: MCNF_ETCD (resolved by DAR-1b), RECONCILER_PREFIX (default /reconciler),
#      MCNF_RECONCILER_SELFTEST (set by selftest → routes I/O to a local mock dir).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../lib/etcd-endpoints.sh
. "$HERE/../lib/etcd-endpoints.sh"

PREFIX="${RECONCILER_PREFIX:-/reconciler}"

b64() { base64 -w0; }

# Resolve the etcd endpoint (first of the quorum; CAS/put are re-tried by callers).
# Skipped in selftest (the mock backs etcd with a local dir).
_resolve() {
  if [ -n "${MCNF_RECONCILER_SELFTEST:-}" ]; then ETCD="mock://selftest"; return 0; fi
  ETCD="$(mcnf_resolve_etcd_first)" || exit 1
}

# ── etcd v3 HTTP KV layer (mockable for selftest) ──
_mock_path() { printf '%s/%s' "$MOCK_DIR" "$(printf %s "$1" | b64)"; }

_kv_put() { # <full-key> <raw-value>
  if [ "$ETCD" = "mock://selftest" ]; then
    mkdir -p "$MOCK_DIR"; printf %s "$2" >"$(_mock_path "$1")"; return 0
  fi
  local k v
  k="$(printf %s "$1" | b64)"; v="$(printf %s "$2" | b64)"
  curl -s -X POST "$ETCD/v3/kv/put" -d "{\"key\":\"$k\",\"value\":\"$v\"}" >/dev/null
}

_kv_get() { # <full-key> -> raw value on stdout (exit 3 if absent)
  if [ "$ETCD" = "mock://selftest" ]; then
    local p; p="$(_mock_path "$1")"
    [ -f "$p" ] || return 3
    cat "$p"; return 0
  fi
  local k; k="$(printf %s "$1" | b64)"
  curl -s -X POST "$ETCD/v3/kv/range" -d "{\"key\":\"$k\"}" | python3 -c '
import sys,json,base64
d=json.load(sys.stdin); kvs=d.get("kvs")
if not kvs: sys.exit(3)
sys.stdout.buffer.write(base64.b64decode(kvs[0]["value"]))'
}

_kv_del() { # <full-key>
  if [ "$ETCD" = "mock://selftest" ]; then rm -f "$(_mock_path "$1")"; return 0; fi
  local k; k="$(printf %s "$1" | b64)"
  curl -s -X POST "$ETCD/v3/kv/deleterange" -d "{\"key\":\"$k\"}" >/dev/null
}

_kv_keys() { # <prefix> -> decoded keys, one per line
  if [ "$ETCD" = "mock://selftest" ]; then
    [ -d "$MOCK_DIR" ] || return 0
    local f name
    for f in "$MOCK_DIR"/*; do
      [ -e "$f" ] || continue
      name="$(basename "$f" | base64 -d 2>/dev/null || true)"
      case "$name" in "$1"*) printf '%s\n' "$name" ;; esac
    done
    return 0
  fi
  local s e
  s="$(printf %s "$1" | b64)"
  e="$(python3 -c "import sys,base64;b=sys.argv[1].encode();print(base64.b64encode(b[:-1]+bytes([b[-1]+1])).decode())" "$1")"
  curl -s -X POST "$ETCD/v3/kv/range" -d "{\"key\":\"$s\",\"range_end\":\"$e\",\"keys_only\":true}" | python3 -c '
import sys,json,base64
for kv in (json.load(sys.stdin).get("kvs") or []):
    print(base64.b64decode(kv["key"]).decode())'
}

# Compare-and-swap a key from <expected> to <new>. <expected> empty means
# create-if-absent. Returns 0 iff WE won (the value matched), 1 otherwise.
_kv_cas() { # <full-key> <expected> <new>
  local key="$1" expected="$2" new="$3"
  if [ "$ETCD" = "mock://selftest" ]; then
    local cur p; p="$(_mock_path "$key")"
    cur="$( [ -f "$p" ] && cat "$p" || true )"
    if [ "$cur" = "$expected" ]; then mkdir -p "$MOCK_DIR"; printf %s "$new" >"$p"; return 0; fi
    return 1
  fi
  local kb vb nb body resp
  kb="$(printf %s "$key" | b64)"; nb="$(printf %s "$new" | b64)"
  if [ -z "$expected" ]; then
    # create-if-absent: compare CREATE revision == 0
    body="$(python3 -c "
import json,sys
print(json.dumps({'compare':[{'key':sys.argv[1],'result':'EQUAL','target':'CREATE','create_revision':'0'}],
 'success':[{'requestPut':{'key':sys.argv[1],'value':sys.argv[2]}}],'failure':[]}))" "$kb" "$nb")"
  else
    vb="$(printf %s "$expected" | b64)"
    body="$(python3 -c "
import json,sys
print(json.dumps({'compare':[{'key':sys.argv[1],'result':'EQUAL','target':'VALUE','value':sys.argv[2]}],
 'success':[{'requestPut':{'key':sys.argv[1],'value':sys.argv[3]}}],'failure':[]}))" "$kb" "$vb" "$nb")"
  fi
  resp="$(curl -s -X POST "$ETCD/v3/kv/txn" -d "$body")"
  printf '%s' "$resp" | python3 -c "import json,sys;sys.exit(0 if json.load(sys.stdin).get('succeeded') else 1)" 2>/dev/null
}

# Normalize a relative key into the /reconciler/ prefix (a leading-/ key is taken
# verbatim so a caller can pass the full path; a bare name is prefixed).
_full() { case "$1" in /*) printf '%s' "$1" ;; *) printf '%s/%s' "$PREFIX" "$1" ;; esac; }

cmd="${1:-}"
case "$cmd" in
  get)
    _resolve
    [ -n "${2:-}" ] || { echo "usage: get <key>" >&2; exit 2; }
    _kv_get "$(_full "$2")"
    ;;

  put)
    _resolve
    [ -n "${2:-}" ] || { echo "usage: put <key> [<value>]" >&2; exit 2; }
    if [ $# -ge 3 ]; then val="$3"; else val="$(cat)"; fi
    _kv_put "$(_full "$2")" "$val"
    ;;

  del)
    _resolve
    [ -n "${2:-}" ] || { echo "usage: del <key>" >&2; exit 2; }
    _kv_del "$(_full "$2")"
    ;;

  cas)
    _resolve
    [ -n "${2:-}" ] && [ $# -ge 4 ] || { echo "usage: cas <key> <expected> <new>" >&2; exit 2; }
    if _kv_cas "$(_full "$2")" "$3" "$4"; then echo "cas: won"; exit 0; else echo "cas: lost (value changed)"; exit 1; fi
    ;;

  ensure-prefix)
    _resolve
    # Idempotent: create-if-absent the .init sentinel so the prefix always exists
    # (so a fresh control VM's first list/get doesn't look like a fault).
    _kv_cas "$PREFIX/.init" "" "$(date -u +%FT%TZ 2>/dev/null || echo init)" || true
    echo "ensure-prefix: $PREFIX ready"
    ;;

  list)
    _resolve
    _kv_keys "$PREFIX/"
    ;;

  selftest)
    # Offline: mock etcd with a local dir; touches NO live store. Round-trips
    # get/put/del/cas/ensure-prefix through the REAL code paths (re-invokes $0).
    MOCK_DIR="$(mktemp -d)"; export MOCK_DIR
    ETCD="mock://selftest"
    fail=0
    pass() { printf '  PASS %s\n' "$1"; }
    bad()  { printf '  FAIL %s\n' "$1"; fail=1; }
    run() { env MCNF_RECONCILER_SELFTEST=1 MOCK_DIR="$MOCK_DIR" bash "$0" "$@"; }
    echo "reconciler-state selftest (mocked etcd at $MOCK_DIR — NO live store touched)"

    # ensure-prefix → .init present, list shows it.
    run ensure-prefix >/dev/null
    case "$(run list)" in *"$PREFIX/.init"*) pass "ensure-prefix creates the .init sentinel" ;; *) bad "ensure-prefix did not create .init" ;; esac

    # put/get round-trips a value.
    printf '%s' 'xen-bigboy-50\nxen-home-51' | run put farm-busy-state
    got="$(run get farm-busy-state)"
    [ "$got" = 'xen-bigboy-50\nxen-home-51' ] && pass "put/get round-trips busy-state" || bad "busy-state round-trip got '$got'"

    # get of an absent key exits 3.
    rc=0; run get no-such-key >/dev/null 2>&1 || rc=$?
    [ "$rc" -eq 3 ] && pass "get of absent key exits 3" || bad "get of absent key exited $rc (want 3)"

    # cas create-if-absent wins once, loses on the second (value now set).
    run del last-reconcile 2>/dev/null || true
    if run cas last-reconcile "" 'rev1' >/dev/null; then pass "cas create-if-absent wins on a fresh key"; else bad "cas create-if-absent should have won"; fi
    if run cas last-reconcile "" 'rev2' >/dev/null 2>&1; then bad "cas create-if-absent should LOSE once set"; else pass "cas create-if-absent loses once the key exists"; fi
    # cas expected→new wins, then the stale-expected loses.
    if run cas last-reconcile 'rev1' 'rev3' >/dev/null; then pass "cas matched-expected wins"; else bad "cas matched-expected should have won"; fi
    if run cas last-reconcile 'rev1' 'rev4' >/dev/null 2>&1; then bad "cas stale-expected should LOSE"; else pass "cas stale-expected loses"; fi
    [ "$(run get last-reconcile)" = 'rev3' ] && pass "value after cas chain is rev3" || bad "value after cas chain is '$(run get last-reconcile)'"

    # del removes the key.
    run del last-reconcile
    rc=0; run get last-reconcile >/dev/null 2>&1 || rc=$?
    [ "$rc" -eq 3 ] && pass "del removes the key" || bad "del did not remove the key (rc=$rc)"

    rm -rf "$MOCK_DIR"
    if [ "$fail" -eq 0 ]; then echo "selftest: ALL PASS"; else echo "selftest: FAILURES" >&2; fi
    exit "$fail"
    ;;

  -h|--help)
    sed -n '2,40p' "$0" | sed 's/^# \{0,1\}//'
    ;;

  *)
    echo "usage: $0 {get <key>|put <key> [val]|del <key>|cas <key> <exp> <new>|ensure-prefix|list|selftest}" >&2
    exit 2
    ;;
esac
