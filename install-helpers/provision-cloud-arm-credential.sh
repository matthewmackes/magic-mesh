#!/usr/bin/env bash
# Materialize the mesh-wide cloud arming secret as a host-bound systemd
# credential. Plaintext exists only in a root-only temporary directory.
set -euo pipefail
umask 077

SECRET_NAME="cloud-arm-key"
CREDENTIAL_NAME="cloud-arm-key"
SECRET_BIN="${MCNF_SECRET_BIN:-/opt/mcnf/automation/secrets/mcnf-secret.sh}"
CREDENTIAL_PATH="${MCNF_CLOUD_ARM_CREDENTIAL_PATH:-/etc/credstore.encrypted/cloud-arm-key}"
DROPIN_SOURCE="${MCNF_CLOUD_ARM_DROPIN_SOURCE:-/usr/libexec/mackesd/cloud-arm-credential.conf}"

validate_key() {
  local value
  value="$(tr -d '\r\n' <"$1")"
  [ "${#value}" -eq 64 ] && [[ "$value" =~ ^[0-9a-f]{64}$ ]]
}

self_test() {
  local test_dir good bad template
  test_dir="$(mktemp -d)"
  good="$test_dir/good"
  bad="$test_dir/bad"
  printf '%064d\n' 0 >"$good"
  printf 'not-a-key\n' >"$bad"
  validate_key "$good"
  ! validate_key "$bad"
  template="$DROPIN_SOURCE"
  if [ ! -r "$template" ]; then
    template="$(cd "$(dirname "$0")/.." && pwd)/packaging/systemd/cloud-arm-credential.conf"
  fi
  grep -Fxq \
    'LoadCredentialEncrypted=cloud-arm-key:/etc/credstore.encrypted/cloud-arm-key' \
    "$template"
  rm -rf -- "$test_dir"
  echo "provision-cloud-arm-credential: self-test passed"
}

if [ "${1:-}" = "--self-test" ]; then
  self_test
  exit 0
fi

[ "$(id -u)" -eq 0 ] || {
  echo "provision-cloud-arm-credential: must run as root" >&2
  exit 1
}
command -v systemd-creds >/dev/null || {
  echo "provision-cloud-arm-credential: systemd-creds is required" >&2
  exit 1
}
[ -x "$SECRET_BIN" ] || {
  echo "provision-cloud-arm-credential: secret helper not executable: $SECRET_BIN" >&2
  exit 1
}
[ -r "$DROPIN_SOURCE" ] || {
  echo "provision-cloud-arm-credential: credential drop-in template unavailable: $DROPIN_SOURCE" >&2
  exit 1
}

initialize=0
restart=0
for argument in "$@"; do
  case "$argument" in
    --init) initialize=1 ;;
    --restart) restart=1 ;;
    *)
      echo "usage: $0 [--init] [--restart] | --self-test" >&2
      exit 2
      ;;
  esac
done

tmp_dir="$(mktemp -d /run/mcnf-cloud-arm.XXXXXX)"
trap 'rm -rf -- "$tmp_dir"' EXIT
plain="$tmp_dir/plain"
encrypted="$tmp_dir/encrypted"

if ! "$SECRET_BIN" get "$SECRET_NAME" >"$plain" 2>/dev/null; then
  if [ "$initialize" -ne 1 ]; then
    echo "provision-cloud-arm-credential: sealed secret '$SECRET_NAME' is absent; initialize it once with --init" >&2
    exit 1
  fi
  od -An -N32 -tx1 /dev/urandom | tr -d ' \n' >"$plain"
  validate_key "$plain" || {
    echo "provision-cloud-arm-credential: CSPRNG output validation failed" >&2
    exit 1
  }
  "$SECRET_BIN" put "$SECRET_NAME" <"$plain"
fi

validate_key "$plain" || {
  echo "provision-cloud-arm-credential: sealed secret must be 64 lowercase hex characters" >&2
  exit 1
}

systemd-creds encrypt --name="$CREDENTIAL_NAME" "$plain" "$encrypted" >/dev/null
install -D -m 0600 -o root -g root "$encrypted" "$CREDENTIAL_PATH"
for unit in mackesd.service mde-shell-egui.service; do
  install -D -m 0644 -o root -g root "$DROPIN_SOURCE" \
    "/etc/systemd/system/$unit.d/50-cloud-arm-credential.conf"
done
echo "provision-cloud-arm-credential: installed host-bound credential at $CREDENTIAL_PATH"

systemctl daemon-reload
if [ "$restart" -eq 1 ]; then
  systemctl try-restart mackesd.service
  systemctl try-restart mde-shell-egui.service
fi
