#!/usr/bin/env bash
# migrate-cred-files.sh — DAR-5: fold the remaining host-local PLAINTEXT credential
# files into the mesh secret store (age + etcd), so a reconstituted control VM needs
# NO `/root/.mcnf-*` plaintext on disk.
#
# WHAT this folds (each `<name>` -> /mcnf/secret/<name>):
#
#   secret name          plaintext source (today)                consumer switched to the store
#   ─────────────────    ──────────────────────────────────────  ──────────────────────────────
#   xo-token             /root/.mcnf-xo-token (xo-mint-token.sh)  infra/tofu/env.sh.example
#   edgeos-cred          /root/.mcnf-ubnt-cred                    infra/tofu/edgeos (via tofu-env.sh)
#   dns-token            $MCNF_DNS_TOKEN_FILE (no live consumer    reserved (design §2.3 cred set);
#                        yet — design-set cred, optional source)  resolved by future DNS-01 IaC
#   sccache-access-key   $MCNF_MINIO_ACCESS_KEY / minio root user automation/cache/sccache-backend-up.sh
#   sccache-secret-key   $MCNF_MINIO_SECRET_KEY / minio root pass  + infra/ansible/sccache.yml (-e from get)
#
# (do-token + xapi-password were folded already — DATACENTER-3; dr-spaces-key is
# folded by the DR scripts — DAR-39/41. This script covers ONLY the remaining set.)
#
# SECURITY (§6, §7): a secret VALUE is NEVER printed, logged, or passed on argv. Each
# value is read from its 0600 source file (or env) and piped on STDIN into
# `mcnf-secret.sh put`, so it never appears in `ps` / /proc/<pid>/cmdline / this log.
# We report presence + byte-length only.
#
# SAFETY: default is --dry-run (PLAN only — touches NO live store). A real fold needs
# explicit --apply AND a reachable store (MCNF_ETCD / /etc/mackesd/etcd-endpoints).
# The agent never runs --apply; the operator does.
#
# Usage:
#   migrate-cred-files.sh [--dry-run]     plan: show which sources are present + the
#                                         exact put that WOULD run (no store writes)
#   migrate-cred-files.sh --apply         OPERATOR: read each present source + put it
#                                         into the store (idempotent; re-put is fine)
#   migrate-cred-files.sh --apply --only <name>[,<name>...]   fold only the named set
#   migrate-cred-files.sh selftest        offline test (stub put; NO live store)
#
# Env (override the default plaintext sources for a non-standard layout):
#   MCNF_XO_TOKEN_FILE        (default /root/.mcnf-xo-token)
#   MCNF_EDGEOS_CRED_FILE     (default /root/.mcnf-ubnt-cred)
#   MCNF_DNS_TOKEN_FILE       (default unset — dns-token has no canonical file yet)
#   MCNF_MINIO_ACCESS_KEY / MCNF_MINIO_SECRET_KEY  (sccache; from the minio root creds)
#   MCNF_SECRET_BIN           (default ./mcnf-secret.sh next to this script)
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" && pwd)"
SECRET="${MCNF_SECRET_BIN:-$HERE/mcnf-secret.sh}"

XO_TOKEN_FILE="${MCNF_XO_TOKEN_FILE:-/root/.mcnf-xo-token}"
EDGEOS_CRED_FILE="${MCNF_EDGEOS_CRED_FILE:-/root/.mcnf-ubnt-cred}"
DNS_TOKEN_FILE="${MCNF_DNS_TOKEN_FILE:-}"

# The canonical fold set: "secret-name". Each has a SOURCE resolver below.
FOLD_SET=(xo-token edgeos-cred dns-token sccache-access-key sccache-secret-key)

# Resolve a secret's plaintext value onto STDOUT for a given name, WITHOUT logging
# it. Returns 0 + emits the value if the source is present; returns 1 (no output) if
# the source is absent. NEVER echoes the value anywhere but the returned stream.
_source_value() { # <name>
  case "$1" in
    xo-token)
      [ -r "$XO_TOKEN_FILE" ] || return 1
      cat "$XO_TOKEN_FILE" ;;
    edgeos-cred)
      [ -r "$EDGEOS_CRED_FILE" ] || return 1
      cat "$EDGEOS_CRED_FILE" ;;
    dns-token)
      # No canonical host file — only fold if the operator points one out.
      [ -n "$DNS_TOKEN_FILE" ] && [ -r "$DNS_TOKEN_FILE" ] || return 1
      cat "$DNS_TOKEN_FILE" ;;
    sccache-access-key)
      [ -n "${MCNF_MINIO_ACCESS_KEY:-}" ] || return 1
      printf %s "$MCNF_MINIO_ACCESS_KEY" ;;
    sccache-secret-key)
      [ -n "${MCNF_MINIO_SECRET_KEY:-}" ] || return 1
      printf %s "$MCNF_MINIO_SECRET_KEY" ;;
    *) return 1 ;;
  esac
}

# Human-readable source DESCRIPTION (for the plan output — no values).
_source_desc() { # <name>
  case "$1" in
    xo-token)            printf '%s' "$XO_TOKEN_FILE" ;;
    edgeos-cred)         printf '%s' "$EDGEOS_CRED_FILE" ;;
    dns-token)           printf '%s' "${DNS_TOKEN_FILE:-<MCNF_DNS_TOKEN_FILE unset — no canonical source>}" ;;
    sccache-access-key)  printf '%s' "\$MCNF_MINIO_ACCESS_KEY (minio root user)" ;;
    sccache-secret-key)  printf '%s' "\$MCNF_MINIO_SECRET_KEY (minio root password)" ;;
    *) printf '%s' "<unknown>" ;;
  esac
}

# Plan/apply one secret. In dry-run we ONLY report presence + the put that WOULD run;
# in apply we pipe the value on stdin into `put`. Length is the only value-derived
# number ever emitted.
_handle() { # <name> <apply:0|1>
  local name="$1" apply="$2" val len
  if val="$(_source_value "$name")"; then
    len="${#val}"
    if [ "$apply" = "1" ]; then
      # Pipe on STDIN — the value never touches argv or this log.
      printf %s "$val" | bash "$SECRET" put "$name" >/dev/null
      echo "  FOLDED  $name  <- $(_source_desc "$name")  (${len} bytes; value not logged)"
    else
      echo "  PLAN    $name  <- $(_source_desc "$name")  (${len} bytes present) :: would run: <src> | mcnf-secret.sh put $name"
    fi
    # Scrub the local copy from this shell's memory promptly.
    val=""; unset val
    return 0
  else
    echo "  SKIP    $name  (source absent: $(_source_desc "$name"))"
    return 1
  fi
}

# ── selftest: offline, stubbed `put`; touches NO live store ──
if [ "${1:-}" = "selftest" ]; then
  work="$(mktemp -d)"; trap 'rm -rf "$work"' EXIT
  fail=0
  pass() { printf '  PASS %s\n' "$1"; }
  bad()  { printf '  FAIL %s\n' "$1"; fail=1; }

  XO_VALUE="xo-tok-SELFTEST-$RANDOM-$$"
  EDGE_VALUE="edge-pw-SELFTEST-$RANDOM"
  AK_VALUE="ak-SELFTEST-$RANDOM"
  SK_VALUE="sk-SELFTEST-$RANDOM"

  # Stub mcnf-secret.sh: `put <name>` records {name -> stdin} under $work/store; it
  # echoes ONLY the name + byte count (never the value), mirroring the real CLI.
  stub="$work/mcnf-secret.sh"
  cat >"$stub" <<'STUB'
#!/usr/bin/env bash
[ "$1" = put ] || { echo "stub: only put" >&2; exit 2; }
mkdir -p "$STORE_DIR"
n="$(cat | tee "$STORE_DIR/$2" | wc -c)"
echo "stored /mcnf/secret/$2 ($n bytes)"
STUB
  chmod +x "$stub"
  export STORE_DIR="$work/store"
  export MCNF_SECRET_BIN="$stub"
  SECRET="$stub"

  # Seed plaintext sources (0600), exactly like the real host files.
  ( umask 077; printf %s "$XO_VALUE"   >"$work/xo";   printf %s "$EDGE_VALUE" >"$work/edge" )
  export MCNF_XO_TOKEN_FILE="$work/xo"
  export MCNF_EDGEOS_CRED_FILE="$work/edge"
  export MCNF_MINIO_ACCESS_KEY="$AK_VALUE"
  export MCNF_MINIO_SECRET_KEY="$SK_VALUE"
  # dns-token intentionally has NO source -> must SKIP.

  echo "migrate-cred-files selftest (stubbed put — NO live store touched)"

  # (1) dry-run plans the present sources, writes NOTHING.
  dry="$work/dry.log"
  ( "$0" --dry-run ) >"$dry" 2>&1 || true
  if [ ! -d "$STORE_DIR" ] || [ -z "$(ls -A "$STORE_DIR" 2>/dev/null)" ]; then
    pass "dry-run writes nothing to the store"
  else
    bad "dry-run wrote to the store"
  fi
  grep -q "PLAN    xo-token" "$dry" && pass "dry-run plans xo-token" || bad "dry-run did not plan xo-token"
  grep -q "SKIP    dns-token" "$dry" && pass "dns-token with no source is SKIPPED" || bad "dns-token not skipped"

  # (2) apply folds every present source into the (stub) store with the right bytes.
  app="$work/apply.log"
  ( "$0" --apply ) >"$app" 2>&1 || true
  [ "$(cat "$STORE_DIR/xo-token" 2>/dev/null)" = "$XO_VALUE" ] && pass "apply folds xo-token (exact value)" || bad "xo-token value wrong in store"
  [ "$(cat "$STORE_DIR/edgeos-cred" 2>/dev/null)" = "$EDGE_VALUE" ] && pass "apply folds edgeos-cred" || bad "edgeos-cred value wrong"
  [ "$(cat "$STORE_DIR/sccache-access-key" 2>/dev/null)" = "$AK_VALUE" ] && pass "apply folds sccache-access-key" || bad "sccache-access-key wrong"
  [ "$(cat "$STORE_DIR/sccache-secret-key" 2>/dev/null)" = "$SK_VALUE" ] && pass "apply folds sccache-secret-key" || bad "sccache-secret-key wrong"
  [ ! -e "$STORE_DIR/dns-token" ] && pass "dns-token (no source) NOT folded" || bad "dns-token folded despite no source"

  # (3) --only restricts the fold set.
  rm -rf "$STORE_DIR"
  ( "$0" --apply --only xo-token ) >>"$app" 2>&1 || true
  [ -e "$STORE_DIR/xo-token" ] && [ ! -e "$STORE_DIR/edgeos-cred" ] && pass "--only folds just the named secret" || bad "--only did not restrict"

  # (4) NO secret VALUE appears in any logged output (dry OR apply).
  if grep -q -- "$XO_VALUE" "$dry" "$app" 2>/dev/null \
     || grep -q -- "$EDGE_VALUE" "$dry" "$app" 2>/dev/null \
     || grep -q -- "$AK_VALUE" "$dry" "$app" 2>/dev/null \
     || grep -q -- "$SK_VALUE" "$dry" "$app" 2>/dev/null; then
    bad "a secret VALUE leaked into logged output"
  else
    pass "no secret value appears in any logged output"
  fi

  if [ "$fail" -eq 0 ]; then echo "selftest: ALL PASS"; else echo "selftest: FAILURES" >&2; fi
  exit "$fail"
fi

# ── main ──
MODE="dry"          # default = dry-run (NO live store writes)
ONLY=""
while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run) MODE="dry"; shift ;;
    --apply)   MODE="apply"; shift ;;
    --only)    ONLY="${2:?--only needs a comma-separated name list}"; shift 2 ;;
    -h|--help) sed -n '2,50p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "migrate-cred-files: unknown arg '$1'" >&2; exit 2 ;;
  esac
done

# Restrict the fold set if --only was given.
if [ -n "$ONLY" ]; then
  IFS=',' read -r -a sel <<<"$ONLY"
  FOLD_SET=("${sel[@]}")
fi

if [ "$MODE" = "apply" ]; then
  echo "migrate-cred-files: APPLY — folding present plaintext sources into the mesh secret store"
  echo "  (store resolved by mcnf-secret.sh: MCNF_ETCD / /etc/mackesd/etcd-endpoints)"
else
  echo "migrate-cred-files: DRY-RUN — no store writes (use --apply to fold; OPERATOR-run)"
fi

any=0
for name in "${FOLD_SET[@]}"; do
  if _handle "$name" "$([ "$MODE" = apply ] && echo 1 || echo 0)"; then any=1; fi
done

if [ "$MODE" = "apply" ] && [ "$any" = "1" ]; then
  cat <<'NEXT'

next steps (OPERATOR):
  - a fresh control VM must be re-sealed in so its own key can read these:
      mcnf-secret.sh reseal-to <vm-recipient>
  - confirm the consumers now resolve from the store (no /root/.mcnf-* needed):
      ( cd infra/tofu/zone1-do && source <(...) )   # see infra/tofu/*/env.sh.example
  - once verified, the plaintext sources can be removed:
      shred -u /root/.mcnf-xo-token /root/.mcnf-ubnt-cred
NEXT
fi
