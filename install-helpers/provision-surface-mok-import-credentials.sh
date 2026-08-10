#!/usr/bin/bash
# Activate one request-bound Surface MOK import credential pair for mackesd.
#
# The caller must put two *systemd-encrypted, host-bound* blobs at the fixed
# incoming paths below. This helper never accepts a caller-selected path and
# never decrypts either blob into argv, an environment variable, or a regular
# file. It stops mackesd-actions before installing both blobs and starts the
# service only after both and the fixed drop-in are present. That service
# activation boundary is required because systemd credentials cannot be added
# to an already-running process.
#
# This helper intentionally does not reboot. After mokutil has positively
# staged the fixed certificate, reboot authority remains a separate exact-body,
# single-use host-state `propose` then `confirm` pair. surface_enable cannot
# safely mint or retain either capability on behalf of the operator.
set -euo pipefail
umask 077
readonly PATH="/usr/sbin:/usr/bin"
export PATH

readonly UNIT="mackesd-actions.service"
readonly RUNTIME_ROOT="${MCNF_SURFACE_MOK_RUNTIME_ROOT:-/run}"
readonly INCOMING_DIR="$RUNTIME_ROOT/mcnf-surface-mok-import/incoming"
readonly LOCK_PATH="$RUNTIME_ROOT/mcnf-surface-mok-import/mint.lock"
readonly GENERATION_PATH="$INCOMING_DIR/.generation"
readonly TRANSACTION_ROOT="$RUNTIME_ROOT/mcnf-surface-mok-import/transactions"
readonly CREDSTORE_DIR="$RUNTIME_ROOT/credstore.encrypted"
readonly ENVELOPE_NAME="surface-mok-import.sealed"
readonly PASSPHRASE_NAME="surface-mok-import-passphrase"
readonly ENVELOPE_IN="$INCOMING_DIR/$ENVELOPE_NAME"
readonly PASSPHRASE_IN="$INCOMING_DIR/$PASSPHRASE_NAME"
readonly ENVELOPE_OUT="$CREDSTORE_DIR/$ENVELOPE_NAME"
readonly PASSPHRASE_OUT="$CREDSTORE_DIR/$PASSPHRASE_NAME"
readonly DROPIN_SOURCE="${MCNF_SURFACE_MOK_DROPIN_SOURCE:-/usr/libexec/mackesd/surface-mok-import-credential.conf}"
readonly DROPIN_DIR="${MCNF_SURFACE_MOK_SYSTEMD_ROOT:-/etc/systemd/system}/$UNIT.d"
readonly DROPIN_PATH="$DROPIN_DIR/70-surface-mok-import-credential.conf"
readonly COMMAND_TIMEOUT_SECS=3

fail() {
  printf 'provision-surface-mok-import-credentials: %s\n' "$1" >&2
  exit 1
}

bounded() {
  /usr/bin/timeout --signal=KILL --kill-after=2s "${COMMAND_TIMEOUT_SECS}s" "$@"
}

validate_generation() {
  validate_private_input "$GENERATION_PATH"
  validate_generation_value
}

validate_generation_value() {
  local value
  [ "$(/usr/bin/stat -c '%s' "$GENERATION_PATH")" -eq 36 ] \
    || fail "incoming credential generation identifier has the wrong byte length"
  value=$(/usr/bin/head -c 65 "$GENERATION_PATH")
  [[ "$value" =~ ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$ ]] \
    || fail "incoming credential generation identifier is malformed"
}

validate_private_input() {
  local path=$1 mode size
  [ -e "$path" ] || fail "required encrypted input is unavailable: $path"
  [ ! -L "$path" ] || fail "refusing symlinked encrypted input: $path"
  [ -f "$path" ] || fail "encrypted input is not a regular file: $path"
  [ "$(/usr/bin/stat -c '%u' "$path")" -eq 0 ] || fail "encrypted input is not root-owned: $path"
  [ "$(/usr/bin/stat -c '%h' "$path")" -eq 1 ] || fail "encrypted input has multiple links: $path"
  mode=$(/usr/bin/stat -c '%a' "$path")
  [ "$mode" = 400 ] || [ "$mode" = 600 ] \
    || fail "encrypted input permissions must be 0400 or 0600: $path"
  size=$(/usr/bin/stat -c '%s' "$path")
  if [ "$size" -le 0 ] || [ "$size" -gt 131072 ]; then
    fail "encrypted input has an invalid bounded size: $path"
  fi
}

validate_private_directory() {
  local path=$1 mode
  [ ! -L "$path" ] || fail "refusing symlinked encrypted-input directory: $path"
  [ -d "$path" ] || fail "encrypted-input directory is unavailable: $path"
  [ "$(/usr/bin/stat -c '%u' "$path")" -eq 0 ] || fail "encrypted-input directory is not root-owned: $path"
  mode=$(/usr/bin/stat -c '%a' "$path")
  [ "$mode" = 700 ] || fail "encrypted-input directory permissions must be 0700: $path"
}

validate_envelope_stream() {
  /usr/bin/python3 -I -S -c '
import sys
data = sys.stdin.buffer.read(65537)
if not (62 <= len(data) <= 65536):
    raise SystemExit("sealed envelope has an invalid bounded size")
if data[:5] != b"MNCA\x01":
    raise SystemExit("sealed envelope is not the supported mde-seal format")
' || fail "decrypted sealed envelope failed structural validation"
}

validate_passphrase_stream() {
  /usr/bin/python3 -I -S -c '
import sys
data = sys.stdin.buffer.read(4097)
data = data.rstrip(b"\r\n")
if not (1 <= len(data) <= 4096) or b"\x00" in data:
    raise SystemExit("sealing passphrase has an invalid bounded shape")
try:
    data.decode("utf-8")
except UnicodeDecodeError:
    raise SystemExit("sealing passphrase is not UTF-8")
' || fail "decrypted sealing passphrase failed structural validation"
}

validate_dropin() {
  [ -r "$DROPIN_SOURCE" ] || fail "credential drop-in template unavailable: $DROPIN_SOURCE"
  /usr/bin/grep -Fxq \
    'LoadCredentialEncrypted=surface-mok-import.sealed:/run/credstore.encrypted/surface-mok-import.sealed' \
    "$DROPIN_SOURCE" || fail "drop-in has the wrong sealed-envelope binding"
  /usr/bin/grep -Fxq \
    'LoadCredentialEncrypted=surface-mok-import-passphrase:/run/credstore.encrypted/surface-mok-import-passphrase' \
    "$DROPIN_SOURCE" || fail "drop-in has the wrong passphrase binding"
}

self_test() {
  local repo handoff_literal
  repo=$(cd "$(dirname "$0")/.." && pwd)
  fixture=$(/usr/bin/mktemp -d)
  trap '/usr/bin/find "$fixture" -depth -delete' EXIT

  printf 'MNCA\001' >"$fixture/good-envelope"
  /usr/bin/dd if=/dev/zero bs=1 count=57 status=none >>"$fixture/good-envelope"
  validate_envelope_stream <"$fixture/good-envelope"
  if printf 'hostile' | validate_envelope_stream 2>/dev/null; then
    fail "self-test accepted a malformed mde-seal envelope"
  fi
  printf 'bounded-passphrase' | validate_passphrase_stream
  if printf '\0hostile' | validate_passphrase_stream 2>/dev/null; then
    fail "self-test accepted a NUL-bearing passphrase"
  fi

  /usr/bin/install -d -m 0700 "$fixture/runtime/mcnf-surface-mok-import/incoming"
  printf '%s' '01234567-89ab-cdef-0123-456789abcdef' \
    >"$fixture/runtime/mcnf-surface-mok-import/incoming/.generation"
  /usr/bin/chmod 0600 "$fixture/runtime/mcnf-surface-mok-import/incoming/.generation"
  MCNF_SURFACE_MOK_RUNTIME_ROOT="$fixture/runtime" "$0" --test-generation >/dev/null
  printf '%s\n' '01234567-89ab-cdef-0123-456789abcdef' \
    >"$fixture/runtime/mcnf-surface-mok-import/incoming/.generation"
  if MCNF_SURFACE_MOK_RUNTIME_ROOT="$fixture/runtime" "$0" --test-generation 2>/dev/null; then
    fail "self-test accepted a newline-suffixed generation identifier"
  fi
  printf '%s' 'hostile-generation' \
    >"$fixture/runtime/mcnf-surface-mok-import/incoming/.generation"
  if MCNF_SURFACE_MOK_RUNTIME_ROOT="$fixture/runtime" "$0" --test-generation 2>/dev/null; then
    fail "self-test accepted a malformed generation identifier"
  fi

  MCNF_SURFACE_MOK_DROPIN_SOURCE="$repo/packaging/systemd/surface-mok-import-credential.conf" \
    "$0" --verify-contract >/dev/null
  /usr/bin/grep -Fq 'readonly PATH="/usr/sbin:/usr/bin"' "$0"
  /usr/bin/grep -Fxq '#!/usr/bin/bash' "$0"
  /usr/bin/grep -Fq '/usr/bin/python3 -I -S -c' "$0"
  /usr/bin/grep -Fq "/usr/bin/env -i PATH=\"\$PATH\" /usr/bin/systemd-creds" "$0"
  /usr/bin/grep -Fq "/usr/bin/env -i PATH=\"\$PATH\" /usr/bin/systemctl --system" "$0"
  /usr/bin/grep -Fq 'systemd credentials cannot be added' "$0"
  /usr/bin/grep -Fq '/usr/bin/flock -x 9' "$0"
  /usr/bin/grep -Fq '/usr/bin/timeout --signal=KILL --kill-after=2s' "$0"
  /usr/bin/grep -Fq 'credential-bearing action service did not reach active state' "$0"
  /usr/bin/grep -Fq 'rollback()' "$0"
  /usr/bin/grep -Fq -- '--property=RuntimeMaxSec=60s' \
    "$repo/install-helpers/mint-surface-mok-import.sh"
  handoff_literal="host-state \`propose\` then \`confirm\` pair"
  /usr/bin/grep -Fq "$handoff_literal" "$0"
  /usr/bin/grep -Fq 'surface_enable never mints or retains either exact-body capability' \
    "$repo/crates/mesh/mackesd/src/surface/enable.rs"
  /usr/bin/grep -Fq 'Phase::Propose' "$repo/crates/mesh/mackesd/src/workers/host_state.rs"
  /usr/bin/grep -Fq 'Phase::Confirm' "$repo/crates/mesh/mackesd/src/workers/host_state.rs"
  /usr/bin/find "$fixture" -depth -delete
  trap - EXIT
  printf 'provision-surface-mok-import-credentials: self-test passed\n'
}

verify_contract() {
  validate_dropin
  printf '%s\n' \
    'surface MOK credential contract: ready' \
    'surface reboot handoff: separate host-state propose/confirm authority required'
}

activate() {
  local lock_mode=${1:-standalone} transaction active_deadline was_active=0 mutation_started=0 committed=0
  local had_envelope=0 had_passphrase=0 had_dropin=0
  [ "$(id -u)" -eq 0 ] || fail "must run as root"
  for override in \
    MCNF_SURFACE_MOK_RUNTIME_ROOT \
    MCNF_SURFACE_MOK_DROPIN_SOURCE \
    MCNF_SURFACE_MOK_SYSTEMD_ROOT; do
    [ -z "${!override+x}" ] \
      || fail "$override is test-only and cannot override the production activation contract"
  done
  for command_name in systemd-creds systemctl install stat python3 grep timeout flock head; do
    command -v "$command_name" >/dev/null || fail "required command unavailable: $command_name"
  done
  /usr/bin/install -d -m 0700 -o root -g root "$(dirname "$LOCK_PATH")" "$TRANSACTION_ROOT"
  if [ "$lock_mode" = standalone ]; then
    exec 9>"$LOCK_PATH"
    /usr/bin/flock -x 9
  elif [ "$lock_mode" != parent-locked ]; then
    fail "invalid activation lock mode"
  fi
  validate_dropin
  validate_private_directory "$INCOMING_DIR"
  validate_generation
  validate_private_input "$ENVELOPE_IN"
  validate_private_input "$PASSPHRASE_IN"
  [ ! -L "$ENVELOPE_OUT" ] || fail "refusing symlinked installed envelope credential"
  [ ! -L "$PASSPHRASE_OUT" ] || fail "refusing symlinked installed passphrase credential"
  [ ! -L "$DROPIN_PATH" ] || fail "refusing symlinked credential drop-in"

  # --name binds each ciphertext to the only credential leaf mackesd accepts.
  # Plaintext flows directly through a pipe into a bounded validator.
  bounded /usr/bin/env -i PATH="$PATH" /usr/bin/systemd-creds \
    decrypt --name="$ENVELOPE_NAME" "$ENVELOPE_IN" - 2>/dev/null \
    | validate_envelope_stream
  bounded /usr/bin/env -i PATH="$PATH" /usr/bin/systemd-creds \
    decrypt --name="$PASSPHRASE_NAME" "$PASSPHRASE_IN" - 2>/dev/null \
    | validate_passphrase_stream

  transaction=$(/usr/bin/mktemp -d "$TRANSACTION_ROOT/txn.XXXXXXXX")
  if bounded /usr/bin/env -i PATH="$PATH" /usr/bin/systemctl --system is-active --quiet "$UNIT"; then
    was_active=1
  fi
  if [ -f "$ENVELOPE_OUT" ] && [ ! -L "$ENVELOPE_OUT" ]; then
    /usr/bin/install -m 0600 -o root -g root "$ENVELOPE_OUT" "$transaction/$ENVELOPE_NAME"
    had_envelope=1
  fi
  if [ -f "$PASSPHRASE_OUT" ] && [ ! -L "$PASSPHRASE_OUT" ]; then
    /usr/bin/install -m 0600 -o root -g root "$PASSPHRASE_OUT" "$transaction/$PASSPHRASE_NAME"
    had_passphrase=1
  fi
  if [ -f "$DROPIN_PATH" ] && [ ! -L "$DROPIN_PATH" ]; then
    /usr/bin/install -m 0644 -o root -g root "$DROPIN_PATH" "$transaction/dropin.conf"
    had_dropin=1
  fi

  rollback() {
    [ "$committed" -eq 0 ] || return 0
    [ "$mutation_started" -eq 1 ] || return 0
    bounded /usr/bin/env -i PATH="$PATH" /usr/bin/systemctl --system stop "$UNIT" >/dev/null 2>&1 || :
    if [ "$had_envelope" -eq 1 ]; then
      /usr/bin/install -m 0600 -o root -g root "$transaction/$ENVELOPE_NAME" "$ENVELOPE_OUT"
    else
      /usr/bin/rm -f -- "$ENVELOPE_OUT"
    fi
    if [ "$had_passphrase" -eq 1 ]; then
      /usr/bin/install -m 0600 -o root -g root "$transaction/$PASSPHRASE_NAME" "$PASSPHRASE_OUT"
    else
      /usr/bin/rm -f -- "$PASSPHRASE_OUT"
    fi
    if [ "$had_dropin" -eq 1 ]; then
      /usr/bin/install -m 0644 -o root -g root "$transaction/dropin.conf" "$DROPIN_PATH"
    else
      /usr/bin/rm -f -- "$DROPIN_PATH"
    fi
    bounded /usr/bin/env -i PATH="$PATH" /usr/bin/systemctl --system daemon-reload >/dev/null 2>&1 || :
    if [ "$was_active" -eq 1 ]; then
      bounded /usr/bin/env -i PATH="$PATH" /usr/bin/systemctl --system start --no-block "$UNIT" >/dev/null 2>&1 || :
    fi
  }
  cleanup_transaction() {
    rollback
    [ -z "${transaction:-}" ] || /usr/bin/find "$transaction" -depth -delete 2>/dev/null || :
  }
  trap cleanup_transaction EXIT HUP INT TERM

  # Credentials are immutable to a running service. Stop first, install the
  # complete pair, then admit the queued start before consuming staging.
  mutation_started=1
  bounded /usr/bin/env -i PATH="$PATH" /usr/bin/systemctl --system stop "$UNIT"
  /usr/bin/install -d -m 0700 -o root -g root "$CREDSTORE_DIR" "$DROPIN_DIR"
  /usr/bin/install -m 0600 -o root -g root "$ENVELOPE_IN" "$ENVELOPE_OUT"
  /usr/bin/install -m 0600 -o root -g root "$PASSPHRASE_IN" "$PASSPHRASE_OUT"
  /usr/bin/install -m 0644 -o root -g root "$DROPIN_SOURCE" "$DROPIN_PATH"
  bounded /usr/bin/env -i PATH="$PATH" /usr/bin/systemctl --system daemon-reload
  bounded /usr/bin/env -i PATH="$PATH" /usr/bin/systemctl --system start --no-block "$UNIT"
  active_deadline=$((SECONDS + 5))
  until bounded /usr/bin/env -i PATH="$PATH" /usr/bin/systemctl --system is-active --quiet "$UNIT"; do
    [ "$SECONDS" -lt "$active_deadline" ] || fail "credential-bearing action service did not reach active state"
    /usr/bin/sleep 0.25
  done

  committed=1
  /usr/bin/find "$INCOMING_DIR" -depth -delete
  /usr/bin/find "$transaction" -depth -delete
  transaction=
  trap - EXIT HUP INT TERM
  printf '%s\n' \
    'provision-surface-mok-import-credentials: admitted one active request-bound credential generation' \
    'provision-surface-mok-import-credentials: publish the bound Surface enable request before its 30-second permit expires' \
    'provision-surface-mok-import-credentials: reboot remains a separate host-state propose/confirm operation'
}

case "${1:-}" in
  --activate) activate standalone ;;
  --activate-under-lock) activate parent-locked ;;
  --verify-contract) verify_contract ;;
  --self-test) self_test ;;
  --test-generation) validate_generation_value ;;
  *)
    printf 'usage: %s --activate | --verify-contract | --self-test\n' "$0" >&2
    exit 2
    ;;
esac
