#!/usr/bin/env bash
# Materialize the mesh resource-publisher HMAC secret as a host-bound systemd
# credential for the root shell. The source of truth remains the approved
# mackesd SecretStore; plaintext exists only in a root-only temporary file and
# the short-lived provisioning process.
set -euo pipefail
set +x
umask 077
ulimit -S -c 0
ulimit -H -c 0

readonly SECRET_NAME="resource/publisher-hmac"
readonly CREDENTIAL_NAME="resource-publisher-hmac"
readonly CREDENTIAL_PATH="/etc/credstore.encrypted/resource-publisher-hmac"
readonly DROPIN_SOURCE="/usr/libexec/mackesd/resource-publisher-hmac.conf"
readonly DROPIN_PATH="/etc/systemd/system/mde-shell-egui.service.d/60-resource-publisher-hmac.conf"
readonly SECRET_BIN="/usr/bin/mackesd"
readonly UNIT_SOURCE="/usr/lib/systemd/system/mcnf-resource-publisher-credential.service"

validate_key() {
  local value bytes
  if LC_ALL=C grep -q '[[:cntrl:]]' "$1"; then
    return 1
  fi
  value="$(<"$1")"
  bytes="$(LC_ALL=C wc -c <"$1")"
  [ -n "$value" ] && [ "$bytes" -gt 0 ] && [ "$bytes" -le 4096 ]
}

self_test() {
  local test_dir good bad template unit
  test_dir="$(mktemp -d)"
  good="$test_dir/good"
  bad="$test_dir/bad"
  printf 'publisher-key-from-secret-store' >"$good"
  printf 'publisher-key\n' >"$bad"
  validate_key "$good"
  ! validate_key "$bad"
  template="$DROPIN_SOURCE"
  if [ ! -r "$template" ]; then
    template="$(cd "$(dirname "$0")/.." && pwd)/packaging/systemd/resource-publisher-hmac.conf"
  fi
  grep -Fxq \
    'LoadCredentialEncrypted=resource-publisher-hmac:/etc/credstore.encrypted/resource-publisher-hmac' \
    "$template"
  unit="$UNIT_SOURCE"
  if [ ! -r "$unit" ]; then
    unit="$(cd "$(dirname "$0")/.." && pwd)/packaging/systemd/mcnf-resource-publisher-credential.service"
  fi
  grep -Fxq \
    'ExecStart=/usr/bin/timeout --signal=TERM --kill-after=5s 30s /usr/libexec/mackesd/provision-resource-publisher-credential' \
    "$unit"
  ! grep -Eq '^ExecStart=-' "$unit"
  rm -rf -- "$test_dir"
  echo "provision-resource-publisher-credential: self-test passed"
}

if [ "${1:-}" = "--self-test" ]; then
  [ "$#" -eq 1 ] || {
    echo "usage: $0 [--self-test]" >&2
    exit 2
  }
  self_test
  exit 0
fi

[ "$#" -eq 0 ] || {
  echo "usage: $0 [--self-test]" >&2
  exit 2
}
[ "$(id -u)" -eq 0 ] || {
  echo "provision-resource-publisher-credential: must run as root" >&2
  exit 1
}
for command_name in systemd-creds install grep wc cmp chmod chown; do
  command -v "$command_name" >/dev/null 2>&1 || {
    echo "provision-resource-publisher-credential: required command unavailable: $command_name" >&2
    exit 1
  }
done
[ -x "$SECRET_BIN" ] || {
  echo "provision-resource-publisher-credential: approved secret-store API unavailable: $SECRET_BIN" >&2
  exit 1
}
[ -r "$DROPIN_SOURCE" ] || {
  echo "provision-resource-publisher-credential: credential drop-in template unavailable" >&2
  exit 1
}

tmp_dir="$(mktemp -d /run/mcnf-resource-publisher.XXXXXX)"
trap 'rm -rf -- "$tmp_dir"' EXIT
plain="$tmp_dir/plain"
encrypted="$tmp_dir/encrypted"
decrypted="$tmp_dir/decrypted"

if ! "$SECRET_BIN" secret get "$SECRET_NAME" >"$plain" 2>/dev/null; then
  echo "provision-resource-publisher-credential: approved secret '$SECRET_NAME' is unavailable" >&2
  exit 1
fi
validate_key "$plain" || {
  echo "provision-resource-publisher-credential: approved publisher key is empty, multiline, or oversized" >&2
  exit 1
}

for path in "$CREDENTIAL_PATH" "$DROPIN_PATH"; do
  [ ! -L "$path" ] || {
    echo "provision-resource-publisher-credential: refusing symlinked output: $path" >&2
    exit 1
  }
  [ ! -e "$path" ] || [ -f "$path" ] || {
    echo "provision-resource-publisher-credential: refusing non-regular output: $path" >&2
    exit 1
  }
done

changed=0
if [ -f "$CREDENTIAL_PATH" ] \
  && systemd-creds decrypt "$CREDENTIAL_PATH" "$decrypted" >/dev/null 2>&1 \
  && cmp -s "$plain" "$decrypted"; then
  chmod 0600 "$CREDENTIAL_PATH"
  chown root:root "$CREDENTIAL_PATH"
else
  systemd-creds encrypt --name="$CREDENTIAL_NAME" "$plain" "$encrypted" >/dev/null
  install -D -m 0600 -o root -g root "$encrypted" "$CREDENTIAL_PATH"
  changed=1
fi

if [ -f "$DROPIN_PATH" ] && cmp -s "$DROPIN_SOURCE" "$DROPIN_PATH"; then
  chmod 0644 "$DROPIN_PATH"
  chown root:root "$DROPIN_PATH"
else
  install -D -m 0644 -o root -g root "$DROPIN_SOURCE" "$DROPIN_PATH"
  changed=1
fi

if [ "$changed" -eq 1 ]; then
  systemctl daemon-reload
  echo "provision-resource-publisher-credential: host-bound publisher credential staged for the next controlled shell restart"
else
  echo "provision-resource-publisher-credential: host-bound publisher credential already current"
fi
