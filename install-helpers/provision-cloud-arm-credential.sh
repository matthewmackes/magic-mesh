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
  validate_key "$bad" && exit 1
  template="$DROPIN_SOURCE"
  if [ ! -r "$template" ]; then
    template="$(cd "$(dirname "$0")/.." && pwd)/packaging/systemd/cloud-arm-credential.conf"
  fi
  grep -Fxq \
    'LoadCredentialEncrypted=cloud-arm-key:/etc/credstore.encrypted/cloud-arm-key' \
    "$template"
  grep -Fq "case \"\$argument\"" "$0"
  grep -Fq -- '--refresh' "$0"
  grep -Fq 'systemctl try-restart mackesd.service' "$0"
  grep -Fq 'systemctl try-restart mde-shell-egui.service' "$0"
  grep -Fq "cmp -s \"\$plain\" \"\$decrypted\"" "$0"
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
refresh=0
force_restart=0
for argument in "$@"; do
  case "$argument" in
    --init) initialize=1 ;;
    --refresh) refresh=1 ;;
    # Keep the operator-facing flag backwards compatible. The boot unit uses
    # --refresh so a repeated boot is idempotent and never tears down a live
    # seat; --restart remains an explicit request to apply the credential to
    # active services even when the material is unchanged.
    --restart) refresh=1; force_restart=1 ;;
    *)
      echo "usage: $0 [--init] [--refresh|--restart] | --self-test" >&2
      exit 2
      ;;
  esac
done

tmp_dir="$(mktemp -d /run/mcnf-cloud-arm.XXXXXX)"
trap 'rm -rf -- "$tmp_dir"' EXIT
plain="$tmp_dir/plain"
encrypted="$tmp_dir/encrypted"
decrypted="$tmp_dir/decrypted"

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

credential_changed=1
if [ -f "$CREDENTIAL_PATH" ] && [ ! -L "$CREDENTIAL_PATH" ] \
  && systemd-creds decrypt "$CREDENTIAL_PATH" "$decrypted" >/dev/null 2>&1 \
  && cmp -s "$plain" "$decrypted"; then
  credential_changed=0
fi

if [ "$credential_changed" -eq 1 ]; then
  systemd-creds encrypt --name="$CREDENTIAL_NAME" "$plain" "$encrypted" >/dev/null
  install -D -m 0600 -o root -g root "$encrypted" "$CREDENTIAL_PATH"
else
  # Keep the existing host-bound ciphertext, but repair its ownership/mode if
  # an operator or an older package left the metadata too permissive.
  chmod 0600 "$CREDENTIAL_PATH"
  chown root:root "$CREDENTIAL_PATH"
fi

dropin_changed=0
for unit in mackesd.service mde-shell-egui.service; do
  dropin_path="/etc/systemd/system/$unit.d/50-cloud-arm-credential.conf"
  if [ -L "$dropin_path" ]; then
    echo "provision-cloud-arm-credential: refusing symlinked drop-in: $dropin_path" >&2
    exit 1
  fi
  if [ -f "$dropin_path" ] && cmp -s "$DROPIN_SOURCE" "$dropin_path"; then
    chmod 0644 "$dropin_path"
    chown root:root "$dropin_path"
  else
    install -D -m 0644 -o root -g root "$DROPIN_SOURCE" "$dropin_path"
    dropin_changed=1
  fi
done

materialized_changed=$((credential_changed || dropin_changed))
if [ "$materialized_changed" -eq 1 ]; then
  echo "provision-cloud-arm-credential: installed host-bound credential at $CREDENTIAL_PATH"
else
  echo "provision-cloud-arm-credential: host-bound credential already current"
fi

if [ "$materialized_changed" -eq 1 ] || [ "$force_restart" -eq 1 ]; then
  systemctl daemon-reload
fi
if [ "$refresh" -eq 1 ] && [ "$force_restart" -eq 1 ] \
  && { [ "$materialized_changed" -eq 1 ] || [ "$force_restart" -eq 1 ]; }; then
  # --restart is intentionally the only path that can interrupt an active
  # seat. try-restart never starts an inactive unit.
  systemctl try-restart mackesd.service
  systemctl try-restart mde-shell-egui.service
elif [ "$refresh" -eq 1 ] && [ "$materialized_changed" -eq 1 ]; then
  echo "provision-cloud-arm-credential: staged for next controlled restart"
fi
