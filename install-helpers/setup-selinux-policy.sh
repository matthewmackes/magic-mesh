#!/bin/bash
# setup-selinux-policy.sh - QC-22 SELinux tightening bootstrap.
#
# Construct-cloud restored the Red Hat-conventions target: SELinux should be
# Enforcing on shipped nodes, with MCNF/OpenStack policy loaded explicitly. This
# helper is intentionally safe in RPM flows: it is run by a bounded systemd
# oneshot, never synchronously from dnf %post.
#
# It does three things:
#   1. Persist SELINUX=enforcing for the next boot.
#   2. Load the shipped MCNF CIL policy modules with bounded semodule calls.
#   3. If the current boot is Permissive, try to switch to Enforcing after the
#      policy is loaded. A Disabled kernel cannot be changed until reboot.
#
# A persistent per-module fingerprint avoids repeating the expensive semodule
# transaction on every boot. The fingerprint is used only when the source hash,
# that module's installed inventory record, runtime/configured mode, and
# active/configured policy type all agree. Any uncertainty falls back to
# semodule -i. Markers are written atomically only after a successful load or a
# verified unchanged skip.
#
# Optional modules are best-effort because their referenced policy types only
# exist when the matching subsystem is installed (for example container-selinux
# for Podman, libvirt SELinux policy for virtqemud).
set -uo pipefail

CONFIG=${MCNF_SELINUX_CONFIG:-/etc/selinux/config}
POLICY_DIR=${MCNF_SELINUX_POLICY_DIR:-/usr/share/magic-mesh/selinux}
STATE_DIR=${MCNF_SELINUX_STATE_DIR:-${MCNF_SELINUX_MARKER_DIR:-/var/lib/mackesd/selinux-policy}}
SEMODULE_TIMEOUT=${MCNF_SEMODULE_TIMEOUT:-90}
ENFORCE_NOW=${MCNF_SELINUX_ENFORCE_NOW:-1}

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
SCRIPT_PATH="$SCRIPT_DIR/$(basename -- "${BASH_SOURCE[0]}")"

cur=Disabled
rc=0
HASH_TOOL=
MODULE_INVENTORY=
MODULE_INVENTORY_OK=0
CONFIGURED_MODE=missing
CONFIGURED_POLICY_TYPE=missing
POLICY_TYPE=unknown
BASE_LOAD_STATE=not-run
PODMAN_LOAD_STATE=not-run
LIBVIRT_LOAD_STATE=not-run
BASE_SOURCE_SHA256=
PODMAN_SOURCE_SHA256=
LIBVIRT_SOURCE_SHA256=

usage() {
  cat <<'EOF'
Usage:
  setup-selinux-policy
  setup-selinux-policy --self-test

The normal invocation persists SELINUX=enforcing and loads the shipped MCNF
SELinux CIL modules. --self-test uses only temporary fake command fixtures and
performs no live SELinux operation.
EOF
}

canonical_mode() {
  local value=${1:-}
  value="$(printf '%s' "$value" | tr '[:upper:]' '[:lower:]')"
  case "$value" in
    enforcing) printf 'enforcing\n' ;;
    permissive) printf 'permissive\n' ;;
    disabled) printf 'disabled\n' ;;
    *) printf '%s\n' "$value" ;;
  esac
}

read_config_value() {
  local key=$1
  [ -r "$CONFIG" ] || return 1
  awk -F= -v wanted="$key" '
    $1 ~ "^[[:space:]]*" wanted "[[:space:]]*$" {
      value = $2
      sub(/[[:space:]]*#.*/, "", value)
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", value)
      print value
      exit
    }
  ' "$CONFIG"
}

configured_mode_fingerprint() {
  local value
  value="$(read_config_value SELINUX 2>/dev/null || true)"
  [ -n "$value" ] || {
    printf 'missing\n'
    return 0
  }
  canonical_mode "$value"
}

configured_policy_type_fingerprint() {
  local value
  value="$(read_config_value SELINUXTYPE 2>/dev/null || true)"
  [ -n "$value" ] || {
    printf 'missing\n'
    return 0
  }
  printf '%s\n' "$value"
}

active_policy_type_fingerprint() {
  local value configured

  if command -v sestatus >/dev/null 2>&1; then
    value="$(
      sestatus 2>/dev/null | awk -F: '
        {
          key = $1
          gsub(/^[[:space:]]+|[[:space:]]+$/, "", key)
          if (tolower(key) == "loaded policy name") {
            value = $2
            gsub(/^[[:space:]]+|[[:space:]]+$/, "", value)
            print value
            exit
          }
        }
      '
    )" || value=
    if [ -n "$value" ]; then
      printf 'active:%s\n' "$value"
      return 0
    fi
  fi

  # A configured type is weaker than sestatus's active type, but still catches
  # a policy-store target change. If neither can be observed, the marker logic
  # refuses to skip and safely reloads on the next invocation.
  configured="$(configured_policy_type_fingerprint)"
  if [ "$configured" != "missing" ]; then
    printf 'configured:%s\n' "$configured"
  else
    printf 'unknown\n'
  fi
}

select_hash_tool() {
  HASH_TOOL="$(command -v sha256sum 2>/dev/null || true)"
}

is_sha256() {
  [[ "${1:-}" =~ ^[[:xdigit:]]{64}$ ]]
}

sha256_file() {
  local digest
  [ -n "$HASH_TOOL" ] || return 1
  digest="$("$HASH_TOOL" "$1" 2>/dev/null | awk 'NR == 1 { print $1; exit }')" || return 1
  is_sha256 "$digest" || return 1
  printf '%s\n' "$digest"
}

sha256_text() {
  local digest
  [ -n "$HASH_TOOL" ] || return 1
  digest="$(printf '%s\n' "$1" | "$HASH_TOOL" 2>/dev/null | awk 'NR == 1 { print $1; exit }')" || return 1
  is_sha256 "$digest" || return 1
  printf '%s\n' "$digest"
}

refresh_module_inventory() {
  local output

  MODULE_INVENTORY=
  MODULE_INVENTORY_OK=0
  if ! output="$(semodule -l 2>/dev/null)"; then
    return 1
  fi

  MODULE_INVENTORY="$output"
  MODULE_INVENTORY_OK=1
  return 0
}

module_is_installed() {
  local name=$1
  [ "$MODULE_INVENTORY_OK" -eq 1 ] || return 1
  printf '%s\n' "$MODULE_INVENTORY" |
    awk -v wanted="$name" '$1 == wanted { found = 1 } END { exit found ? 0 : 1 }'
}

module_inventory_fingerprint() {
  local name=$1
  local records

  [ "$MODULE_INVENTORY_OK" -eq 1 ] || return 1
  records="$(printf '%s\n' "$MODULE_INVENTORY" |
    awk -v wanted="$name" '$1 == wanted { print }' |
    LC_ALL=C sort)"
  [ -n "$records" ] || return 1
  sha256_text "$records"
}

marker_value() {
  local key=$1
  local marker=$2
  [ -r "$marker" ] || return 1
  awk -F= -v wanted="$key" '
    $1 == wanted {
      print substr($0, length(wanted) + 2)
      found = 1
      exit
    }
    END { if (!found) exit 1 }
  ' "$marker"
}

marker_path() {
  printf '%s/%s.fingerprint\n' "$STATE_DIR" "$1"
}

marker_matches() {
  local name=$1
  local source_sha256=$2
  local marker module_inventory_sha256

  [ "$MODULE_INVENTORY_OK" -eq 1 ] || return 1
  module_is_installed "$name" || return 1
  module_inventory_sha256="$(module_inventory_fingerprint "$name" 2>/dev/null || true)"
  [ -n "$module_inventory_sha256" ] || return 1
  marker="$(marker_path "$name")"
  [ "$(marker_value marker_version "$marker" 2>/dev/null || true)" = "1" ] || return 1
  [ "$(marker_value module "$marker" 2>/dev/null || true)" = "$name" ] || return 1
  [ "$(marker_value source_sha256 "$marker" 2>/dev/null || true)" = "$source_sha256" ] || return 1
  [ "$(marker_value mode "$marker" 2>/dev/null || true)" = "$cur" ] || return 1
  [ "$(marker_value configured_mode "$marker" 2>/dev/null || true)" = "$CONFIGURED_MODE" ] || return 1
  [ "$(marker_value policy_type "$marker" 2>/dev/null || true)" = "$POLICY_TYPE" ] || return 1
  [ "$(marker_value configured_policy_type "$marker" 2>/dev/null || true)" = "$CONFIGURED_POLICY_TYPE" ] || return 1
  [ "$(marker_value module_inventory_sha256 "$marker" 2>/dev/null || true)" = "$module_inventory_sha256" ] || return 1
  return 0
}

ensure_state_dir() {
  if [ -d "$STATE_DIR" ]; then
    [ -w "$STATE_DIR" ] || return 1
    return 0
  fi
  mkdir -p -- "$STATE_DIR" 2>/dev/null || return 1
  chmod 0755 -- "$STATE_DIR" 2>/dev/null || true
  [ -w "$STATE_DIR" ]
}

write_marker() {
  local name=$1
  local source_sha256=$2
  local module_inventory_sha256=$3
  local marker tmp

  ensure_state_dir || {
    echo "WARN: cannot write SELinux fingerprint state in $STATE_DIR; policy will be checked again next boot" >&2
    return 0
  }
  marker="$(marker_path "$name")"
  tmp="$(mktemp "$STATE_DIR/.${name}.fingerprint.XXXXXX" 2>/dev/null)" || {
    echo "WARN: cannot create SELinux fingerprint state for $name; policy will be checked again next boot" >&2
    return 0
  }
  if ! {
    printf 'marker_version=1\n'
    printf 'module=%s\n' "$name"
    printf 'source_sha256=%s\n' "$source_sha256"
    printf 'mode=%s\n' "$cur"
    printf 'configured_mode=%s\n' "$CONFIGURED_MODE"
    printf 'policy_type=%s\n' "$POLICY_TYPE"
    printf 'configured_policy_type=%s\n' "$CONFIGURED_POLICY_TYPE"
    printf 'module_inventory_sha256=%s\n' "$module_inventory_sha256"
  } >"$tmp"; then
    rm -f -- "$tmp"
    echo "WARN: cannot write SELinux fingerprint state for $name; policy will be checked again next boot" >&2
    return 0
  fi
  chmod 0644 -- "$tmp" 2>/dev/null || true
  if ! mv -f -- "$tmp" "$marker"; then
    rm -f -- "$tmp"
    echo "WARN: cannot install SELinux fingerprint state for $name; policy will be checked again next boot" >&2
    return 0
  fi
}

set_module_state() {
  case "$1" in
    magicmesh-base) BASE_LOAD_STATE=$2 ;;
    magicmesh-podman) PODMAN_LOAD_STATE=$2 ;;
    magicmesh-libvirt) LIBVIRT_LOAD_STATE=$2 ;;
  esac
}

set_module_source_hash() {
  case "$1" in
    magicmesh-base) BASE_SOURCE_SHA256=$2 ;;
    magicmesh-podman) PODMAN_SOURCE_SHA256=$2 ;;
    magicmesh-libvirt) LIBVIRT_SOURCE_SHA256=$2 ;;
  esac
}

module_source_hash() {
  case "$1" in
    magicmesh-base) printf '%s\n' "$BASE_SOURCE_SHA256" ;;
    magicmesh-podman) printf '%s\n' "$PODMAN_SOURCE_SHA256" ;;
    magicmesh-libvirt) printf '%s\n' "$LIBVIRT_SOURCE_SHA256" ;;
    *) printf '\n' ;;
  esac
}

module_state() {
  case "$1" in
    magicmesh-base) printf '%s\n' "$BASE_LOAD_STATE" ;;
    magicmesh-podman) printf '%s\n' "$PODMAN_LOAD_STATE" ;;
    magicmesh-libvirt) printf '%s\n' "$LIBVIRT_LOAD_STATE" ;;
    *) printf 'unknown\n' ;;
  esac
}

load_cil() {
  local name=$1
  local file=$2
  local required=$3
  local source_sha256=

  if [ ! -f "$file" ]; then
    echo "WARN: SELinux module $name missing at $file"
    [ "$required" = "required" ] && rc=1
    set_module_source_hash "$name" ""
    set_module_state "$name" missing
    return 0
  fi

  source_sha256="$(sha256_file "$file" 2>/dev/null || true)"
  set_module_source_hash "$name" "$source_sha256"
  if [ -n "$source_sha256" ] && marker_matches "$name" "$source_sha256"; then
    echo "==> SELinux module $name unchanged; fingerprint and policy state match, skipped"
    set_module_state "$name" skipped
    return 0
  fi

  if [ -z "$source_sha256" ]; then
    echo "WARN: cannot fingerprint SELinux module $name; loading without a skip marker"
  fi
  if timeout "$SEMODULE_TIMEOUT" semodule -i "$file" >/dev/null 2>&1; then
    echo "==> loaded SELinux module $name"
    set_module_state "$name" loaded
    return 0
  fi

  set_module_state "$name" failed
  if [ "$required" = "required" ] && [ "$cur" != "disabled" ]; then
    echo "ERROR: failed to load required SELinux module $name from $file" >&2
    rc=1
  else
    echo "WARN: skipped SELinux module $name; optional type may be absent or SELinux disabled"
  fi
}

record_marker_if_ready() {
  local name=$1
  local file=$2
  local state source_sha256 expected_source_sha256

  state="$(module_state "$name")"
  case "$state" in
    loaded|skipped) ;;
    *) return 0 ;;
  esac
  [ -f "$file" ] || return 0
  [ "$MODULE_INVENTORY_OK" -eq 1 ] || return 0
  module_is_installed "$name" || return 0
  local module_inventory_sha256
  module_inventory_sha256="$(module_inventory_fingerprint "$name" 2>/dev/null || true)"
  [ -n "$module_inventory_sha256" ] || return 0
  expected_source_sha256="$(module_source_hash "$name")"
  [ -n "$expected_source_sha256" ] || return 0
  source_sha256="$(sha256_file "$file" 2>/dev/null || true)"
  if [ -z "$source_sha256" ] || [ "$source_sha256" != "$expected_source_sha256" ]; then
    echo "WARN: SELinux module $name source changed during policy setup; no skip marker recorded" >&2
    return 0
  fi
  write_marker "$name" "$source_sha256" "$module_inventory_sha256"
}

persist_enforcing() {
  if [ ! -f "$CONFIG" ]; then
    echo "WARN: $CONFIG absent; cannot persist SELINUX=enforcing"
    return 0
  fi

  if grep -q '^SELINUX=' "$CONFIG"; then
    sed -i 's/^SELINUX=.*/SELINUX=enforcing/' "$CONFIG"
  else
    printf '\nSELINUX=enforcing\n' >>"$CONFIG"
  fi
  echo "==> persisted SELINUX=enforcing in $CONFIG"
}

main() {
  cur="$(getenforce 2>/dev/null || echo Disabled)"
  cur="$(canonical_mode "$cur")"
  rc=0
  select_hash_tool

  persist_enforcing
  CONFIGURED_MODE="$(configured_mode_fingerprint)"
  CONFIGURED_POLICY_TYPE="$(configured_policy_type_fingerprint)"

  if ! command -v semodule >/dev/null 2>&1; then
    echo "WARN: semodule not installed; policy load deferred until selinux-policy tools exist"
    return 0
  fi

  refresh_module_inventory || {
    echo "WARN: cannot inspect installed SELinux modules; policy loads will not be skipped"
  }
  POLICY_TYPE="$(active_policy_type_fingerprint)"

  load_cil magicmesh-base "$POLICY_DIR/magicmesh-base.cil" required
  load_cil magicmesh-podman "$POLICY_DIR/magicmesh-podman.cil" optional
  load_cil magicmesh-libvirt "$POLICY_DIR/magicmesh-libvirt.cil" optional

  if [ "$cur" = "permissive" ] && [ "$ENFORCE_NOW" = "1" ] && command -v setenforce >/dev/null 2>&1; then
    if setenforce 1 >/dev/null 2>&1; then
      cur="enforcing"
      echo "==> setenforce 1 after loading MCNF policy"
    else
      echo "ERROR: setenforce 1 failed after loading MCNF policy" >&2
      rc=1
    fi
  elif [ "$cur" = "disabled" ]; then
    echo "==> SELinux kernel state is Disabled; reboot required for Enforcing"
  fi

  # Capture the post-load state. This prevents a successful Permissive ->
  # Enforcing transition from forcing another full semodule transaction next
  # boot, while still refusing to mark a failed or incomplete policy load.
  CONFIGURED_MODE="$(configured_mode_fingerprint)"
  CONFIGURED_POLICY_TYPE="$(configured_policy_type_fingerprint)"
  POLICY_TYPE="$(active_policy_type_fingerprint)"
  if ! refresh_module_inventory; then
    echo "WARN: cannot verify the post-load SELinux module inventory; no skip markers recorded"
  else
    record_marker_if_ready magicmesh-base "$POLICY_DIR/magicmesh-base.cil"
    record_marker_if_ready magicmesh-podman "$POLICY_DIR/magicmesh-podman.cil"
    record_marker_if_ready magicmesh-libvirt "$POLICY_DIR/magicmesh-libvirt.cil"
  fi

  echo "==> SELinux mode now: $(getenforce 2>/dev/null || echo Disabled); target = Enforcing"
  return "$rc"
}

self_test_fail() {
  echo "setup-selinux-policy: self-test failed: $*" >&2
  exit 1
}

self_test() {
  local test_dir fake_bin config policy_dir state_dir modules log failures mode policy
  local output rc_value count

  test_dir="$(mktemp -d "${TMPDIR:-/tmp}/mcnf-selinux-policy.XXXXXX")" || \
    self_test_fail "could not create temporary fixture"
  export MCNF_SELINUX_SELF_TEST_DIR="$test_dir"
  trap 'rm -rf -- "${MCNF_SELINUX_SELF_TEST_DIR:-}"' EXIT

  fake_bin="$test_dir/bin"
  config="$test_dir/selinux-config"
  policy_dir="$test_dir/policy"
  state_dir="$test_dir/state"
  modules="$test_dir/modules"
  log="$test_dir/semodule.log"
  failures="$test_dir/failures"
  mode="$test_dir/mode"
  policy="$test_dir/policy-type"
  mkdir -p -- "$fake_bin" "$policy_dir" "$state_dir"
  : >"$modules"
  : >"$log"
  : >"$failures"
  printf 'SELINUX=enforcing\nSELINUXTYPE=targeted\n' >"$config"
  printf 'Enforcing\n' >"$mode"
  printf 'targeted\n' >"$policy"
  printf 'base policy v1\n' >"$policy_dir/magicmesh-base.cil"
  printf 'podman policy v1\n' >"$policy_dir/magicmesh-podman.cil"
  printf 'libvirt policy v1\n' >"$policy_dir/magicmesh-libvirt.cil"

  # These single-quoted strings are intentionally emitted literally into the
  # fake command fixtures; they must expand when the fixture, not this helper,
  # runs.
  # shellcheck disable=SC2016
  printf '%s\n' '#!/bin/sh' 'cat "$MCNF_TEST_MODE_FILE"' >"$fake_bin/getenforce"
  # shellcheck disable=SC2016
  printf '%s\n' '#!/bin/sh' 'printf "Enforcing\\n" >"$MCNF_TEST_MODE_FILE"' >"$fake_bin/setenforce"
  # shellcheck disable=SC2016
  printf '%s\n' '#!/bin/sh' \
    'printf "Loaded policy name: %s\\n" "$(cat "$MCNF_TEST_POLICY_FILE")"' >"$fake_bin/sestatus"
  printf '%s\n' '#!/bin/sh' 'shift' 'exec "$@"' >"$fake_bin/timeout"
  # shellcheck disable=SC2016
  printf '%s\n' \
    '#!/bin/sh' \
    'set -eu' \
    'case "${1:-}" in' \
    '  -l) cat "$MCNF_TEST_MODULES" ;;' \
    '  -i)' \
    '    file="${2:?}"' \
    '    printf "%s\\n" "$file" >>"$MCNF_TEST_SEMODULE_LOG"' \
    '    if [ -s "$MCNF_TEST_FAILURES" ] && grep -Fxq -- "$file" "$MCNF_TEST_FAILURES"; then exit 1; fi' \
    '    module="$(basename "$file" .cil)"' \
    '    if ! grep -Eq "^${module}([[:space:]]|$)" "$MCNF_TEST_MODULES"; then printf "%s 1.0\\n" "$module" >>"$MCNF_TEST_MODULES"; fi' \
    '    ;;' \
    '  *) exit 2 ;;' \
    'esac' >"$fake_bin/semodule"
  chmod 0755 -- "$fake_bin"/*

  export PATH="$fake_bin:/usr/bin:/bin:/usr/sbin"
  export MCNF_SELINUX_CONFIG="$config"
  export MCNF_SELINUX_POLICY_DIR="$policy_dir"
  export MCNF_SELINUX_STATE_DIR="$state_dir"
  export MCNF_SELINUX_ENFORCE_NOW=0
  export MCNF_TEST_MODE_FILE="$mode"
  export MCNF_TEST_POLICY_FILE="$policy"
  export MCNF_TEST_MODULES="$modules"
  export MCNF_TEST_SEMODULE_LOG="$log"
  export MCNF_TEST_FAILURES="$failures"

  run_helper() {
    local label=$1 expected=$2 output
    output="$test_dir/$label.out"
    if "$SCRIPT_PATH" >"$output" 2>&1; then
      rc_value=0
    else
      rc_value=$?
    fi
    [ "$rc_value" -eq "$expected" ] || {
      sed -n '1,120p' "$output" >&2
      self_test_fail "$label returned $rc_value; expected $expected"
    }
  }

  install_count() {
    awk 'END { print NR + 0 }' "$log"
  }

  run_helper initial 0
  count="$(install_count)"
  [ "$count" -eq 3 ] || self_test_fail "initial run installed $count modules; expected 3"

  run_helper unchanged 0
  count="$(install_count)"
  [ "$count" -eq 3 ] || self_test_fail "unchanged run installed $count modules; expected 3"
  grep -Fq 'fingerprint and policy state match, skipped' "$test_dir/unchanged.out" || \
    self_test_fail "unchanged run did not report fingerprint skip"

  printf 'base policy v2\n' >"$policy_dir/magicmesh-base.cil"
  run_helper source-change 0
  count="$(install_count)"
  [ "$count" -eq 4 ] || self_test_fail "source change installed $count modules; expected 4"

  sed -i '/^magicmesh-podman[[:space:]]/d' "$modules"
  run_helper missing-installed-module 0
  count="$(install_count)"
  [ "$count" -eq 5 ] || self_test_fail "missing installed module installed $count modules; expected 5"

  printf 'mls\n' >"$policy"
  run_helper policy-change 0
  count="$(install_count)"
  [ "$count" -eq 8 ] || self_test_fail "policy change installed $count modules; expected 8"

  printf 'Permissive\n' >"$mode"
  run_helper mode-change 0
  count="$(install_count)"
  [ "$count" -eq 11 ] || self_test_fail "mode change installed $count modules; expected 11"
  export MCNF_SELINUX_ENFORCE_NOW=1
  run_helper enforce-after-skip 0
  count="$(install_count)"
  [ "$count" -eq 11 ] || self_test_fail "enforcement transition reloaded $count modules; expected 11"
  run_helper stable-after-enforce 0
  count="$(install_count)"
  [ "$count" -eq 11 ] || self_test_fail "stable enforcing run installed $count modules; expected 11"

  rm -f -- "$policy_dir/magicmesh-podman.cil"
  run_helper optional-source-missing 0
  count="$(install_count)"
  [ "$count" -eq 11 ] || self_test_fail "missing optional source installed $count modules; expected 11"

  rm -f -- "$policy_dir/magicmesh-base.cil"
  run_helper required-source-missing 1
  count="$(install_count)"
  [ "$count" -eq 11 ] || self_test_fail "missing required source installed $count modules; expected 11"

  printf 'base policy v2\n' >"$policy_dir/magicmesh-base.cil"
  printf 'podman policy v2\n' >"$policy_dir/magicmesh-podman.cil"
  printf '%s\n' "$policy_dir/magicmesh-podman.cil" >"$failures"
  run_helper optional-load-failure 0
  count="$(install_count)"
  [ "$count" -eq 12 ] || self_test_fail "optional failure installed $count modules; expected 12"

  printf '%s\n' "$policy_dir/magicmesh-base.cil" >"$failures"
  printf 'base policy v3\n' >"$policy_dir/magicmesh-base.cil"
  run_helper required-load-failure 1
  count="$(install_count)"
  [ "$count" -eq 14 ] || self_test_fail "required failure installed $count modules; expected 14"

  printf 'Disabled\n' >"$mode"
  printf 'base policy v4\n' >"$policy_dir/magicmesh-base.cil"
  run_helper disabled-required-failure 0
  count="$(install_count)"
  [ "$count" -eq 17 ] || self_test_fail "disabled required failure installed $count modules; expected 17"

  echo "setup-selinux-policy: self-test passed"
}

case "${1:-}" in
  --self-test)
    [ "$#" -eq 1 ] || { usage >&2; exit 2; }
    self_test
    ;;
  "")
    main
    exit "$?"
    ;;
  *)
    usage >&2
    exit 2
    ;;
esac
