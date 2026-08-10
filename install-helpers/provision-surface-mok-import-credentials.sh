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

fail() {
  printf 'provision-surface-mok-import-credentials: %s\n' "$1" >&2
  exit 1
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

  MCNF_SURFACE_MOK_DROPIN_SOURCE="$repo/packaging/systemd/surface-mok-import-credential.conf" \
    "$0" --verify-contract >/dev/null
  /usr/bin/grep -Fq 'readonly PATH="/usr/sbin:/usr/bin"' "$0"
  /usr/bin/grep -Fxq '#!/usr/bin/bash' "$0"
  /usr/bin/grep -Fq '/usr/bin/python3 -I -S -c' "$0"
  /usr/bin/grep -Fq "/usr/bin/env -i PATH=\"\$PATH\" /usr/bin/systemd-creds" "$0"
  /usr/bin/grep -Fq "/usr/bin/env -i PATH=\"\$PATH\" /usr/bin/systemctl --system" "$0"
  /usr/bin/grep -Fq 'systemd credentials cannot be added' "$0"
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
  [ "$(id -u)" -eq 0 ] || fail "must run as root"
  for override in \
    MCNF_SURFACE_MOK_RUNTIME_ROOT \
    MCNF_SURFACE_MOK_DROPIN_SOURCE \
    MCNF_SURFACE_MOK_SYSTEMD_ROOT; do
    [ -z "${!override+x}" ] \
      || fail "$override is test-only and cannot override the production activation contract"
  done
  for command_name in systemd-creds systemctl install stat python3 grep; do
    command -v "$command_name" >/dev/null || fail "required command unavailable: $command_name"
  done
  validate_dropin
  validate_private_directory "$INCOMING_DIR"
  validate_private_input "$ENVELOPE_IN"
  validate_private_input "$PASSPHRASE_IN"

  # --name binds each ciphertext to the only credential leaf mackesd accepts.
  # Plaintext flows directly through a pipe into a bounded validator.
  /usr/bin/env -i PATH="$PATH" /usr/bin/systemd-creds \
    decrypt --name="$ENVELOPE_NAME" "$ENVELOPE_IN" - 2>/dev/null \
    | validate_envelope_stream
  /usr/bin/env -i PATH="$PATH" /usr/bin/systemd-creds \
    decrypt --name="$PASSPHRASE_NAME" "$PASSPHRASE_IN" - 2>/dev/null \
    | validate_passphrase_stream

  # Credentials are immutable to a running service. Stop first, install the
  # complete pair, then start; any intermediate failure leaves mutations off.
  /usr/bin/env -i PATH="$PATH" /usr/bin/systemctl --system stop "$UNIT"
  /usr/bin/install -d -m 0700 -o root -g root "$CREDSTORE_DIR" "$DROPIN_DIR"
  /usr/bin/install -m 0600 -o root -g root "$ENVELOPE_IN" "$ENVELOPE_OUT"
  /usr/bin/install -m 0600 -o root -g root "$PASSPHRASE_IN" "$PASSPHRASE_OUT"
  /usr/bin/install -m 0644 -o root -g root "$DROPIN_SOURCE" "$DROPIN_PATH"
  /usr/bin/env -i PATH="$PATH" /usr/bin/systemctl --system daemon-reload
  /usr/bin/env -i PATH="$PATH" /usr/bin/systemctl --system start "$UNIT"
  /usr/bin/find "$INCOMING_DIR" -maxdepth 1 -type f \
    \( -name "$ENVELOPE_NAME" -o -name "$PASSPHRASE_NAME" \) -delete
  printf '%s\n' \
    'provision-surface-mok-import-credentials: activated one request-bound credential generation' \
    'provision-surface-mok-import-credentials: publish the bound Surface enable request before its 30-second permit expires' \
    'provision-surface-mok-import-credentials: reboot remains a separate host-state propose/confirm operation'
}

case "${1:-}" in
  --activate) activate ;;
  --verify-contract) verify_contract ;;
  --self-test) self_test ;;
  *)
    printf 'usage: %s --activate | --verify-contract | --self-test\n' "$0" >&2
    exit 2
    ;;
esac
