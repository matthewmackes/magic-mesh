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
readonly EX_DATAERR=65
readonly EX_NOINPUT=66
readonly EX_TEMPFAIL=75
readonly EX_CONFIG=78

validate_key() {
  local value bytes
  if LC_ALL=C grep -q '[[:cntrl:]]' "$1"; then
    return 1
  fi
  value="$(<"$1")"
  bytes="$(LC_ALL=C wc -c <"$1")"
  [ -n "$value" ] && [ "$bytes" -gt 0 ] && [ "$bytes" -le 4096 ]
}

unit_has_one_line() {
  [ "$(grep -Fxc -- "$2" "$1")" -eq 1 ]
}

validate_retry_unit() {
  local unit="$1"
  unit_has_one_line "$unit" \
    'ExecStart=/usr/bin/timeout --signal=TERM --kill-after=5s 30s /usr/libexec/mackesd/provision-resource-publisher-credential' \
    && unit_has_one_line "$unit" 'StartLimitIntervalSec=5min' \
    && unit_has_one_line "$unit" 'StartLimitBurst=6' \
    && unit_has_one_line "$unit" 'Restart=on-failure' \
    && unit_has_one_line "$unit" 'RestartSec=30s' \
    && unit_has_one_line "$unit" 'RestartPreventExitStatus=65 66 78' \
    && [ "$(grep -Ec '^StartLimitIntervalSec=' "$unit")" -eq 1 ] \
    && [ "$(grep -Ec '^StartLimitBurst=' "$unit")" -eq 1 ] \
    && [ "$(grep -Ec '^Restart=' "$unit")" -eq 1 ] \
    && [ "$(grep -Ec '^RestartSec=' "$unit")" -eq 1 ] \
    && [ "$(grep -Ec '^RestartPreventExitStatus=' "$unit")" -eq 1 ] \
    && ! grep -Eq '^(ExecStart=-|SuccessExitStatus=|RestartForceExitStatus=)' "$unit"
}

assert_hostile_unit_rejected() {
  local unit="$1" mutation="$2"
  if validate_retry_unit "$unit"; then
    echo "provision-resource-publisher-credential: unsafe unit mutation accepted: $mutation" >&2
    return 1
  fi
}

self_test() {
  local test_dir good bad template unit hostile
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
  validate_retry_unit "$unit"

  # Hostile structural mutations prove the verifier rejects restart storms,
  # failure masking, unbounded retries, and loss of terminal credential states.
  hostile="$test_dir/hostile.service"
  cp -- "$unit" "$hostile"
  sed -i 's/^RestartSec=30s$/RestartSec=1ms/' "$hostile"
  assert_hostile_unit_rejected "$hostile" 'restart storm pacing'
  cp -- "$unit" "$hostile"
  printf '\nRestart=always\n' >>"$hostile"
  assert_hostile_unit_rejected "$hostile" 'overriding restart policy'
  cp -- "$unit" "$hostile"
  sed -i 's/^StartLimitBurst=6$/StartLimitBurst=60/' "$hostile"
  assert_hostile_unit_rejected "$hostile" 'unbounded retry burst'
  cp -- "$unit" "$hostile"
  sed -i '/^RestartPreventExitStatus=/d' "$hostile"
  assert_hostile_unit_rejected "$hostile" 'retrying permanent credential failures'
  cp -- "$unit" "$hostile"
  printf '\nSuccessExitStatus=75\n' >>"$hostile"
  assert_hostile_unit_rejected "$hostile" 'masking transient failure as success'
  cp -- "$unit" "$hostile"
  sed -i 's|^ExecStart=|ExecStart=-|' "$hostile"
  assert_hostile_unit_rejected "$hostile" 'masking ExecStart failure'
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
    exit "$EX_CONFIG"
  }
done
[ -x "$SECRET_BIN" ] || {
  echo "provision-resource-publisher-credential: approved secret-store API unavailable: $SECRET_BIN" >&2
  exit "$EX_CONFIG"
}
[ -r "$DROPIN_SOURCE" ] || {
  echo "provision-resource-publisher-credential: credential drop-in template unavailable" >&2
  exit "$EX_CONFIG"
}

tmp_dir="$(mktemp -d /run/mcnf-resource-publisher.XXXXXX)"
trap 'rm -rf -- "$tmp_dir"' EXIT
plain="$tmp_dir/plain"
encrypted="$tmp_dir/encrypted"
decrypted="$tmp_dir/decrypted"

secret_status=0
"$SECRET_BIN" secret get "$SECRET_NAME" >"$plain" 2>/dev/null || secret_status=$?
if [ "$secret_status" -eq 3 ]; then
  echo "provision-resource-publisher-credential: approved secret '$SECRET_NAME' is not in the store" >&2
  exit "$EX_NOINPUT"
elif [ "$secret_status" -ne 0 ]; then
  echo "provision-resource-publisher-credential: approved secret store is temporarily unavailable (status $secret_status)" >&2
  exit "$EX_TEMPFAIL"
fi
validate_key "$plain" || {
  echo "provision-resource-publisher-credential: approved publisher key is empty, multiline, or oversized" >&2
  exit "$EX_DATAERR"
}

for path in "$CREDENTIAL_PATH" "$DROPIN_PATH"; do
  [ ! -L "$path" ] || {
    echo "provision-resource-publisher-credential: refusing symlinked output: $path" >&2
    exit "$EX_CONFIG"
  }
  [ ! -e "$path" ] || [ -f "$path" ] || {
    echo "provision-resource-publisher-credential: refusing non-regular output: $path" >&2
    exit "$EX_CONFIG"
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
