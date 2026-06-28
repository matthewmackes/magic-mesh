#!/usr/bin/env bash
# tofu-env.sh — DAR-10: unseal the Tofu provider creds from the MESH SECRET STORE
# into process-scoped env at apply time, so a fresh control VM needs NO hand-placed
# /root/.mcnf-* token files. SOURCE it (not exec) from a root's env.sh:
#
#   . "$(git rev-parse --show-toplevel)/automation/lib/tofu-env.sh"
#   tofu_env_load <root>            # root ∈ {zone1-do, xen-xapi, edgeos, control-vm}
#   tofu plan
#
# WHY a shared lib (vs inlining `mcnf-secret.sh get` in each env.sh.example):
#   - the EdgeOS provider takes its password from a FILE (`sshpass -f`), not env, so
#     it needs a tmpfs 0600 materialization + an exit-trap shred that a one-line
#     `export X=$(get …)` cannot express. One lib gets that exactly right once.
#   - every cred is unsealed with THIS node's OWN age key (DAR-3 secret-zero): the
#     value lands ONLY in a process env var (or a tmpfs file), NEVER in the repo, a
#     dotfile, tofu state, or a log. We print presence/length only — never a value.
#   - falls back to a pre-existing root `env.sh` if present (so an operator who still
#     keeps a hand-rolled env.sh is not broken by the indirection).
#
# Resolution per cred: `mcnf-secret.sh get <name>` (which resolves the VM's key +
# the etcd quorum via DAR-1b). A missing secret FAILS LOUD with the exact `put` line.
#
# Acceptance (WORKLIST DAR-10):
#   - `source tofu-env.sh` populates XOA_TOKEN / TF_VAR_xapi_password /
#     DIGITALOCEAN_TOKEN in-process (via the VM's own key) and tofu authenticates.
#   - the EdgeOS cred file is written to a TMPFS path 0600 and removed by the exit
#     trap; no unsealed secret hits the repo / a dotfile / a log.
#   - falls back to an existing env.sh only if present.

# Guard against double-source (the trap + helpers are idempotent, but re-defining is
# wasteful and could re-arm the trap stack).
if [ -n "${_TOFU_ENV_SH_LOADED:-}" ]; then return 0 2>/dev/null || exit 0; fi
_TOFU_ENV_SH_LOADED=1

# Resolve the secret-store CLI relative to this lib (works in any worktree/checkout;
# no git invocation needed, so it survives a detached or bare working copy).
_TOFU_ENV_HERE="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" && pwd)"
TOFU_ENV_SECRET="${MCNF_SECRET_BIN:-$_TOFU_ENV_HERE/../secrets/mcnf-secret.sh}"

# A tmpfs dir for any cred that a provider reads from a FILE (EdgeOS). Prefer real
# tmpfs (/dev/shm, /run) so the plaintext never touches a disk; fall back to a
# 0700 mktemp dir under TMPDIR if neither is writable. The whole dir is shredded by
# the exit trap.
_tofu_env_tmpfs_dir() {
  local base d
  for base in /dev/shm /run "${XDG_RUNTIME_DIR:-}"; do
    [ -n "$base" ] && [ -d "$base" ] && [ -w "$base" ] || continue
    d="$(mktemp -d "$base/mcnf-tofu-cred.XXXXXX" 2>/dev/null)" || continue
    printf '%s\n' "$d"; return 0
  done
  # Last resort: a private dir under the default TMPDIR (still 0700).
  mktemp -d "${TMPDIR:-/tmp}/mcnf-tofu-cred.XXXXXX"
}

# Files we materialize (EdgeOS cred). Cleaned by the exit trap; NEVER committed.
_TOFU_ENV_CRED_FILES=()
TOFU_ENV_EDGEOS_CRED_FILE=""
# Out-param for _tofu_env_get_to_file (avoids a trap-arming command substitution).
_TOFU_ENV_LAST_FILE=""

# Exit trap: shred + remove every materialized cred file. Best-effort shred (the
# tmpfs is RAM-backed anyway) then unlink. Installed once, on the FIRST load.
_tofu_env_cleanup() {
  local f
  for f in "${_TOFU_ENV_CRED_FILES[@]:-}"; do
    [ -n "$f" ] || continue
    shred -u "$f" 2>/dev/null || rm -f "$f" 2>/dev/null || true
    # Drop the (now empty) tmpfs dir too, if we own it.
    rmdir "$(dirname "$f")" 2>/dev/null || true
  done
}
# Append (don't clobber) any pre-existing EXIT trap.
_tofu_env_arm_trap() {
  local existing
  existing="$(trap -p EXIT | sed -E "s/^trap -- '(.*)' EXIT$/\1/")"
  if [ -n "$existing" ] && [ "$existing" != "_tofu_env_cleanup" ]; then
    # shellcheck disable=SC2064
    trap "_tofu_env_cleanup; $existing" EXIT
  else
    trap _tofu_env_cleanup EXIT
  fi
}

# Unseal one secret from the store into the NAMED env var. NEVER echoes the value:
# we report only presence (and byte-length, which is non-sensitive). FAILS LOUD with
# the exact `put` remediation if the secret is absent.
#   _tofu_env_get_into <ENV_VAR_NAME> <secret-name>
_tofu_env_get_into() {
  local var="$1" name="$2" val
  if ! val="$(bash "$TOFU_ENV_SECRET" get "$name" 2>/dev/null)"; then
    cat >&2 <<EOF
tofu-env: /mcnf/secret/$name is absent (or the store is unreachable).
  Seal it first:  printf %s '<value>' | $TOFU_ENV_SECRET put $name
  (a fresh control VM also needs an operator-run: mcnf-secret.sh reseal-to <vm-recipient>)
EOF
    return 1
  fi
  [ -n "$val" ] || { echo "tofu-env: /mcnf/secret/$name decrypted to EMPTY — reseal/rotate it" >&2; return 1; }
  # Assign without ever printing the value. printf -v keeps it out of argv/the log.
  printf -v "$var" '%s' "$val"
  export "${var?}"
  echo "tofu-env: $var <- /mcnf/secret/$name (${#val} bytes, value not logged)" >&2
}

# Materialize a secret into a tmpfs 0600 FILE (for providers that read a cred file,
# e.g. EdgeOS sshpass -f). Records the path for the exit-trap shred. Returns the
# path in the GLOBAL `_TOFU_ENV_LAST_FILE` (NOT via stdout/`$(...)`): a command
# substitution runs in a subshell, so arming the exit trap there would fire — and
# shred the file — the instant the substitution closed, before the provider could
# read it. Setting a global keeps the trap armed in the CALLER's process. The path
# is non-sensitive; the file CONTENTS are never printed.
#   _tofu_env_get_to_file <secret-name>  -> sets _TOFU_ENV_LAST_FILE
_tofu_env_get_to_file() {
  local name="$1" dir f val
  _TOFU_ENV_LAST_FILE=""
  if ! val="$(bash "$TOFU_ENV_SECRET" get "$name" 2>/dev/null)"; then
    cat >&2 <<EOF
tofu-env: /mcnf/secret/$name is absent (or the store is unreachable).
  Seal it first:  printf %s '<value>' | $TOFU_ENV_SECRET put $name
EOF
    return 1
  fi
  [ -n "$val" ] || { echo "tofu-env: /mcnf/secret/$name decrypted to EMPTY — reseal/rotate it" >&2; return 1; }
  dir="$(_tofu_env_tmpfs_dir)" || { echo "tofu-env: could not create a tmpfs dir for the cred file" >&2; return 1; }
  f="$dir/$name"
  ( umask 077; printf %s "$val" >"$f" )
  chmod 600 "$f" 2>/dev/null || true
  _TOFU_ENV_CRED_FILES+=("$f")
  _tofu_env_arm_trap
  echo "tofu-env: /mcnf/secret/$name materialized to a tmpfs 0600 file (${#val} bytes, removed on exit)" >&2
  _TOFU_ENV_LAST_FILE="$f"
}

# If a hand-rolled env.sh exists ALONGSIDE the caller, source it and skip the store.
# Lets an operator keep a legacy env.sh without the indirection forcing the store.
#   _tofu_env_fallback_envsh <root-dir>  -> returns 0 if it sourced one
_tofu_env_fallback_envsh() {
  local rootdir="$1"
  if [ -f "$rootdir/env.sh" ]; then
    echo "tofu-env: sourcing existing $rootdir/env.sh (fallback; store indirection skipped)" >&2
    # shellcheck disable=SC1090,SC1091
    . "$rootdir/env.sh"
    return 0
  fi
  return 1
}

# Load the provider creds for a Tofu root from the store (or an existing env.sh).
# Idempotent + side-effect-scoped to the current process.
#   tofu_env_load <root> [<root-dir>]
# <root-dir> defaults to infra/tofu/<root> under the repo (resolved from this lib).
tofu_env_load() {
  local root="${1:?usage: tofu_env_load <root>}"
  local rootdir="${2:-$_TOFU_ENV_HERE/../../infra/tofu/$root}"

  # An existing env.sh in the root wins (legacy escape hatch).
  if _tofu_env_fallback_envsh "$rootdir"; then return 0; fi

  case "$root" in
    zone1-do)
      _tofu_env_get_into DIGITALOCEAN_TOKEN do-token || return 1
      ;;
    xen-xapi)
      _tofu_env_get_into TF_VAR_xapi_password xapi-password || return 1
      # The dead XO path: XOA_TOKEN is unsealed too IF the root still references it,
      # so a revived XO workspace authenticates from the store, never a dotfile.
      if [ "${TOFU_ENV_WITH_XO:-0}" = "1" ]; then
        _tofu_env_get_into XOA_TOKEN xo-token || return 1
      fi
      ;;
    control-vm)
      _tofu_env_get_into TF_VAR_xapi_password xapi-password || return 1
      _tofu_env_get_into TF_VAR_join_token join-token || return 1
      ;;
    edgeos)
      # EdgeOS scripts read the password from a FILE via `sshpass -f` (never argv),
      # so we materialize it to a tmpfs 0600 file and point TF_VAR_edgeos_cred_file
      # at it. The exit trap shreds it; nothing persists on disk or in the repo.
      # (No `$(...)` here — that subshell would arm+fire the shred trap immediately;
      # _tofu_env_get_to_file returns the path in _TOFU_ENV_LAST_FILE instead.)
      _tofu_env_get_to_file edgeos-cred || return 1
      TOFU_ENV_EDGEOS_CRED_FILE="$_TOFU_ENV_LAST_FILE"
      export TF_VAR_edgeos_cred_file="$TOFU_ENV_EDGEOS_CRED_FILE"
      ;;
    *)
      echo "tofu-env: unknown root '$root' (expected zone1-do|xen-xapi|edgeos|control-vm)" >&2
      return 2
      ;;
  esac
  return 0
}

# ── offline self-test (mocked secret store; touches NO live store) ──
# Drives tofu_env_load against a stub `mcnf-secret.sh get` so the REAL unseal +
# tmpfs-file + exit-trap-shred paths are exercised honestly. Asserts:
#   (1) zone1-do populates DIGITALOCEAN_TOKEN from the store,
#   (2) xen-xapi populates TF_VAR_xapi_password,
#   (3) edgeos writes a 0600 tmpfs file whose CONTENTS are the unsealed value,
#   (4) the cred file is gone after the trap fires (subshell exit),
#   (5) an existing env.sh short-circuits the store,
#   (6) a missing secret fails loud,
#   (7) NO secret value appears in any captured output.
if [ "${1:-}" = "selftest" ]; then
  set -uo pipefail
  work="$(mktemp -d)"; trap 'rm -rf "$work"' EXIT
  fail=0
  pass() { printf '  PASS %s\n' "$1"; }
  bad()  { printf '  FAIL %s\n' "$1"; fail=1; }

  SECRET_VALUE="do-token-SELFTEST-$RANDOM-$$"
  XAPI_VALUE="xapi-pw-SELFTEST-$RANDOM"
  EDGE_VALUE="edgeos-pw-SELFTEST-$RANDOM"

  # Stub mcnf-secret.sh: `get <name>` echoes a per-name canned value; unknown → exit 1.
  stub="$work/mcnf-secret.sh"
  cat >"$stub" <<STUB
#!/usr/bin/env bash
[ "\$1" = get ] || { echo "stub: only get" >&2; exit 2; }
case "\$2" in
  do-token)      printf %s "$SECRET_VALUE" ;;
  xapi-password) printf %s "$XAPI_VALUE" ;;
  edgeos-cred)   printf %s "$EDGE_VALUE" ;;
  *) exit 1 ;;
esac
STUB
  chmod +x "$stub"
  export MCNF_SECRET_BIN="$stub"
  # TOFU_ENV_SECRET was bound at load time (before this block set MCNF_SECRET_BIN),
  # so re-point it at the stub now — otherwise the unseal would hit the REAL store.
  TOFU_ENV_SECRET="$stub"

  echo "tofu-env selftest (stubbed secret store — NO live store touched)"

  # (1) zone1-do
  log="$work/zone1.log"
  ( unset DIGITALOCEAN_TOKEN; tofu_env_load zone1-do >"$log" 2>&1; printf '%s' "${DIGITALOCEAN_TOKEN:-}" >"$work/zone1.out" )
  [ "$(cat "$work/zone1.out")" = "$SECRET_VALUE" ] && pass "zone1-do unseals DIGITALOCEAN_TOKEN from the store" || bad "DIGITALOCEAN_TOKEN not set"

  # (2) xen-xapi
  ( unset TF_VAR_xapi_password; tofu_env_load xen-xapi >>"$log" 2>&1; printf '%s' "${TF_VAR_xapi_password:-}" >"$work/xapi.out" )
  [ "$(cat "$work/xapi.out")" = "$XAPI_VALUE" ] && pass "xen-xapi unseals TF_VAR_xapi_password" || bad "TF_VAR_xapi_password not set"

  # (3)+(4) edgeos materializes a 0600 tmpfs file with the right CONTENTS that the
  # exit trap then shreds. We drive this in a SEPARATE process ($inner) — running it
  # in an inline command-substitution would let edgeos's appended cleanup trap stack
  # onto THIS selftest's `rm -rf "$work"` trap and wipe $work mid-run. $inner sources
  # the lib fresh (with MCNF_SECRET_BIN set, so its store IS the stub), checks the
  # file's mode + contents WHILE live, then on its own exit the trap shreds it; it
  # reports whether the path is gone afterward by re-checking from the parent.
  inner="$work/inner.sh"
  cat >"$inner" <<INNER
#!/usr/bin/env bash
set -uo pipefail
export MCNF_SECRET_BIN="$stub"
. "$_TOFU_ENV_HERE/tofu-env.sh"
tofu_env_load edgeos >/dev/null 2>&1
f="\$TOFU_ENV_EDGEOS_CRED_FILE"
mode="\$(stat -c '%a' "\$f" 2>/dev/null || stat -f '%Lp' "\$f" 2>/dev/null)"
echo "PATH=\$f"
echo "MODE=\$mode"
echo "CONTENT_OK=\$([ "\$(cat "\$f")" = "$EDGE_VALUE" ] && echo yes || echo no)"
INNER
  chmod +x "$inner"
  innerout="$(bash "$inner" 2>/dev/null)"
  credpath="$(printf '%s\n' "$innerout" | sed -n 's/^PATH=//p')"
  case "$innerout" in *"MODE=600"*) pass "edgeos cred file is 0600 (in tmpfs)" ;; *) bad "edgeos cred file mode wrong: $innerout" ;; esac
  case "$innerout" in *"CONTENT_OK=yes"*) pass "edgeos cred file contents = the unsealed value" ;; *) bad "edgeos cred file contents wrong" ;; esac
  # The $inner process has now exited, so its exit trap fired: the file is GONE.
  if [ -n "$credpath" ] && [ ! -e "$credpath" ]; then
    pass "edgeos cred file removed by the exit trap"
  else
    bad "edgeos cred file still present after exit: $credpath"
  fi

  # (5) existing env.sh short-circuits the store.
  fbroot="$work/fbroot"; mkdir -p "$fbroot"
  printf 'export TOFU_ENV_FALLBACK_MARKER=hit\n' >"$fbroot/env.sh"
  ( tofu_env_load some-root "$fbroot" >>"$log" 2>&1; printf '%s' "${TOFU_ENV_FALLBACK_MARKER:-}" >"$work/fb.out" )
  [ "$(cat "$work/fb.out")" = "hit" ] && pass "existing env.sh short-circuits the store" || bad "env.sh fallback not honored"

  # (6) a missing secret fails loud.
  printf '#!/usr/bin/env bash\nexit 1\n' >"$work/empty-stub"; chmod +x "$work/empty-stub"
  rc=0; ( unset DIGITALOCEAN_TOKEN; TOFU_ENV_SECRET="$work/empty-stub"; tofu_env_load zone1-do >>"$log" 2>&1 ) || rc=$?
  [ "$rc" -ne 0 ] && pass "a missing secret fails loud (non-zero)" || bad "missing secret did NOT fail"

  # (7) NO secret VALUE in any LOGGED output. We grep the lib's own captured
  # stdout+stderr ($log) and the $inner process's output — NOT the *.out files,
  # which are deliberate value captures the assertions above compare against (the
  # whole point of the lib is that those vars HOLD the value, in-process only).
  if grep -q -- "$SECRET_VALUE" "$log" 2>/dev/null \
     || grep -q -- "$XAPI_VALUE" "$log" 2>/dev/null \
     || grep -q -- "$EDGE_VALUE" "$log" 2>/dev/null \
     || printf '%s' "$innerout" | grep -q -- "$EDGE_VALUE"; then
    bad "a secret VALUE leaked into logged output"
  else
    pass "no secret value appears in any logged output"
  fi

  if [ "$fail" -eq 0 ]; then echo "selftest: ALL PASS"; else echo "selftest: FAILURES" >&2; fi
  exit "$fail"
fi
