#!/usr/bin/env bash
# WL-ARCH-001 Phase B (item 3) — the OpenTofu external-data-source bridge to the
# mesh secret store (mde-seal / mcnf-secret.sh). NO Ansible Vault, NO second
# secret system (decided-stack #8): this + the Ansible lookup plugin
# (automation/ansible/plugins/lookup/mde_seal.py) both resolve from the SAME
# age-sealed etcd store.
#
# OpenTofu `external` data-source protocol: a JSON query object arrives on stdin
# ({"helper":"<path>","name":"<secret>"}); we must print a flat JSON object of
# string→string on stdout ({"value":"<unsealed-secret>"}). A failure exits
# non-zero with a message on stderr (tofu surfaces it verbatim). The value is
# marked `sensitive` in secrets.tf so it never prints in a plan/apply log.
#
#   echo '{"helper":"…/mcnf-secret.sh","name":"nebula-join-token"}' | mde-seal-external.sh
#   mde-seal-external.sh --selftest      # offline self-check (stub helper), no live store
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" && pwd)"
# The tofu MODULE dir (infra/tofu/cloud) — the parent of this scripts/ dir. A
# relative `helper` default is written relative to the module (matching how the
# var default reads), so it resolves against here, not scripts/.
MODULE_DIR="$(cd "$HERE/.." && pwd)"

die() { echo "mde-seal-external: $*" >&2; exit 1; }

# Resolve the helper to an absolute path. A relative `helper` (the default
# ../../../automation/secrets/mcnf-secret.sh) is resolved against the tofu module
# dir so tofu can run from anywhere; a bare name is taken from $PATH.
resolve_helper() {
  local helper="$1"
  case "$helper" in
    /*) printf '%s' "$helper" ;;
    */*) (cd "$MODULE_DIR" && cd "$(dirname "$helper")" 2>/dev/null && printf '%s/%s' "$(pwd)" "$(basename "$helper")") || printf '%s/%s' "$MODULE_DIR" "$helper" ;;
    *) command -v "$helper" 2>/dev/null || printf '%s' "$helper" ;;
  esac
}

# Emit a flat JSON object {"value": "<val>"} with proper escaping, binary-safe
# via python3 (the same interpreter mcnf-secret.sh already relies on).
emit_value() {
  MDE_SEAL_VAL="$1" python3 -c '
import json, os, sys
sys.stdout.write(json.dumps({"value": os.environ["MDE_SEAL_VAL"]}))
'
}

# Read one string field from the stdin JSON query.
query_field() {
  local field="$1" body="$2"
  MDE_SEAL_QUERY="$body" MDE_SEAL_FIELD="$field" python3 -c '
import json, os, sys
q = json.loads(os.environ["MDE_SEAL_QUERY"] or "{}")
v = q.get(os.environ["MDE_SEAL_FIELD"], "")
sys.stdout.write(v if isinstance(v, str) else "")
'
}

resolve() {
  local body helper name val
  body="$(cat)"
  helper="$(query_field helper "$body")"
  name="$(query_field name "$body")"
  [ -n "$name" ] || die "the query is missing a non-empty \`name\` (the sealed secret's store key)"
  [ -n "$helper" ] || helper="mcnf-secret.sh"
  helper="$(resolve_helper "$helper")"
  [ -x "$helper" ] || [ -f "$helper" ] || die "secret-store helper not found/executable: $helper"

  # Unseal with the node's OWN age key. A missing secret / unreachable store makes
  # the helper exit non-zero — surface it, never emit a fabricated value.
  if ! val="$(bash "$helper" get "$name" 2>/dev/null)"; then
    die "\`$helper get $name\` failed — the secret is absent or the store is unreachable (seal it: printf %s '<value>' | $helper put $name)"
  fi
  [ -n "$val" ] || die "/mcnf/secret/$name decrypted to EMPTY — reseal/rotate it"
  emit_value "$val"
}

# ── offline self-test (stubbed helper — touches NO live store) ──
selftest() {
  local work stub fixture out
  work="$(mktemp -d)"
  fixture="join-token-SELFTEST-$RANDOM-$$"
  stub="$work/mcnf-secret.sh"
  cat >"$stub" <<STUB
#!/usr/bin/env bash
[ "\$1" = get ] || { echo "stub: only get" >&2; exit 2; }
case "\$2" in
  nebula-join-token) printf %s "$fixture" ;;
  *) exit 3 ;;
esac
STUB
  chmod +x "$stub"

  echo "mde-seal-external --selftest (stubbed store — NO live store touched)"
  local fail=0
  pass() { printf '  PASS %s\n' "$1"; }
  bad() { printf '  FAIL %s\n' "$1"; fail=1; }

  # (1) a present fixture secret resolves to {"value":"<fixture>"}.
  out="$(printf '{"helper":"%s","name":"nebula-join-token"}' "$stub" | resolve)"
  local got
  got="$(MDE_SEAL_OUT="$out" python3 -c 'import json,os;print(json.loads(os.environ["MDE_SEAL_OUT"])["value"])')"
  [ "$got" = "$fixture" ] && pass "a fixture secret resolves to its value" || bad "resolved '$got' != '$fixture'"

  # (2) the output is a flat JSON object of strings (the external-data contract).
  if MDE_SEAL_OUT="$out" python3 -c '
import json,os,sys
d=json.loads(os.environ["MDE_SEAL_OUT"])
sys.exit(0 if isinstance(d,dict) and all(isinstance(v,str) for v in d.values()) else 1)'; then
    pass "output is a flat JSON object of strings"
  else
    bad "output is not a flat string map: $out"
  fi

  # (3) an ABSENT secret fails loud (non-zero), never emits a fabricated value.
  local rc=0
  printf '{"helper":"%s","name":"no-such-secret"}' "$stub" | resolve >/dev/null 2>&1 || rc=$?
  [ "$rc" -ne 0 ] && pass "an absent secret fails loud (non-zero)" || bad "an absent secret did NOT fail"

  # (4) the fixture value never leaks onto stderr.
  local err
  err="$(printf '{"helper":"%s","name":"nebula-join-token"}' "$stub" | resolve 2>&1 >/dev/null || true)"
  case "$err" in *"$fixture"*) bad "the secret value leaked onto stderr" ;; *) pass "no secret value on stderr" ;; esac

  # (5) the DEFAULT relative helper resolves to the real in-repo mcnf-secret.sh
  #     (path resolution only — no `get`, so the live store is never touched).
  local default_helper resolved
  default_helper="../../../automation/secrets/mcnf-secret.sh"
  resolved="$(resolve_helper "$default_helper")"
  if [ -f "$resolved" ]; then
    pass "default helper resolves to the in-repo mcnf-secret.sh ($resolved)"
  else
    bad "default helper resolved to a non-existent path: $resolved"
  fi

  rm -rf "$work"
  [ "$fail" -eq 0 ] && echo "selftest: ALL PASS" || { echo "selftest: FAILURES" >&2; return 1; }
}

case "${1:-}" in
  --selftest) selftest ;;
  *) resolve ;;
esac
