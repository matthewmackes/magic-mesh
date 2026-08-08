#!/usr/bin/env bash
# Materialize the host-bound Music Ed25519 seed for the root DRM shell and its
# public verification key for the user-owned mde-musicd daemon.
set -euo pipefail
umask 077

SECRET_NAME="music/action-ed25519-seed"
CREDENTIAL_NAME="music-action-private-key"
SECRET_BIN="${MCNF_SECRET_BIN:-/opt/mcnf/automation/secrets/mcnf-secret.sh}"
CREDENTIAL_PATH="${MCNF_MUSIC_ACTION_CREDENTIAL_PATH:-/etc/credstore.encrypted/music-action-private-key}"
PUBLIC_KEY_PATH="${MCNF_MUSIC_ACTION_PUBLIC_KEY_PATH:-/etc/mde/music-action-public-key}"
DROPIN_SOURCE="${MCNF_MUSIC_ACTION_DROPIN_SOURCE:-/usr/libexec/mackesd/music-action-credential.conf}"

validate_seed() {
  local value
  [ -r "$1" ] || return 1
  value="$(tr -d '\r\n' <"$1")"
  [ "${#value}" -eq 64 ] && [[ "$value" =~ ^[0-9a-fA-F]{64}$ ]]
}

derive_public_key() {
  local seed_hex="$1" seed_der="$2" pub_der="$3" value pair offset
  value="$(tr -d '\r\n' <"$seed_hex")"
  printf '%b' '\x30\x2e\x02\x01\x00\x30\x05\x06\x03\x2b\x65\x70\x04\x22\x04\x20' >"$seed_der"
  for ((offset = 0; offset < 64; offset += 2)); do
    pair="${value:offset:2}"
    printf '%b' "\\x$pair" >>"$seed_der"
  done
  openssl pkey -in "$seed_der" -inform DER -pubout -outform DER -out "$pub_der" >/dev/null 2>&1
  tail -c 32 "$pub_der" | od -An -tx1 | tr -d ' \n'
}

self_test() {
  local test_dir seed public
  test_dir="$(mktemp -d)"
  seed="$test_dir/seed"
  public="$test_dir/public"
  printf '%064d\n' 0 >"$seed"
  validate_seed "$seed"
  validate_seed "$test_dir/missing" && exit 1
  derive_public_key "$seed" "$test_dir/seed.der" "$test_dir/pub.der" >"$public"
  [ "$(wc -c <"$public")" -eq 64 ]
  [[ "$(tr -d '\r\n' <"$public")" =~ ^[0-9a-f]{64}$ ]]
  grep -Fxq 'LoadCredentialEncrypted=music-action-private-key:/etc/credstore.encrypted/music-action-private-key' "$DROPIN_SOURCE" 2>/dev/null || \
    grep -Fxq 'LoadCredentialEncrypted=music-action-private-key:/etc/credstore.encrypted/music-action-private-key' "$(cd "$(dirname "$0")/.." && pwd)/packaging/systemd/music-action-credential.conf"
  grep -Fq 'case "$argument"' "$0"
  grep -Fq -- '--refresh' "$0"
  grep -Fq 'music/action-ed25519-seed' "$0"
  rm -rf -- "$test_dir"
  echo "provision-music-action-credential: self-test passed"
}

if [ "${1:-}" = "--self-test" ]; then
  command -v openssl >/dev/null || { echo "provision-music-action-credential: openssl is required" >&2; exit 1; }
  self_test
  exit 0
fi

[ "$(id -u)" -eq 0 ] || { echo "provision-music-action-credential: must run as root" >&2; exit 1; }
command -v systemd-creds >/dev/null || { echo "provision-music-action-credential: systemd-creds is required" >&2; exit 1; }
command -v openssl >/dev/null || { echo "provision-music-action-credential: openssl is required" >&2; exit 1; }
[ -x "$SECRET_BIN" ] || { echo "provision-music-action-credential: secret helper not executable: $SECRET_BIN" >&2; exit 1; }
[ -r "$DROPIN_SOURCE" ] || { echo "provision-music-action-credential: credential drop-in unavailable: $DROPIN_SOURCE" >&2; exit 1; }

initialize=0
refresh=0
force_restart=0
for argument in "$@"; do
  case "$argument" in
    --init) initialize=1 ;;
    --refresh) refresh=1 ;;
    --restart) refresh=1; force_restart=1 ;;
    *) echo "usage: $0 [--init] [--refresh|--restart] | --self-test" >&2; exit 2 ;;
  esac
done

tmp_dir="$(mktemp -d /run/mcnf-music-action.XXXXXX)"
trap 'rm -rf -- "$tmp_dir"' EXIT
plain="$tmp_dir/plain"
encrypted="$tmp_dir/encrypted"
decrypted="$tmp_dir/decrypted"
public="$tmp_dir/public"

if "$SECRET_BIN" get "$SECRET_NAME" >"$plain" 2>/dev/null; then
  :
else
  secret_rc=$?
  if [ "$secret_rc" -ne 3 ]; then
    echo "provision-music-action-credential: sealed secret '$SECRET_NAME' could not be retrieved (secret helper rc=$secret_rc); refusing initialization" >&2
    exit 1
  fi
  if [ "$initialize" -ne 1 ]; then
    echo "provision-music-action-credential: sealed secret '$SECRET_NAME' is absent; initialize it once with --init" >&2
    exit 1
  fi
  od -An -N32 -tx1 /dev/urandom | tr -d ' \n' >"$plain"
  validate_seed "$plain" || { echo "provision-music-action-credential: CSPRNG output validation failed" >&2; exit 1; }
  "$SECRET_BIN" put "$SECRET_NAME" <"$plain"
fi
validate_seed "$plain" || { echo "provision-music-action-credential: sealed seed must be 64 hexadecimal characters" >&2; exit 1; }

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
  chmod 0600 "$CREDENTIAL_PATH"
  chown root:root "$CREDENTIAL_PATH"
fi

derive_public_key "$plain" "$tmp_dir/seed.der" "$tmp_dir/pub.der" >"$public"
public_changed=1
if [ -f "$PUBLIC_KEY_PATH" ] && [ ! -L "$PUBLIC_KEY_PATH" ] && cmp -s "$public" "$PUBLIC_KEY_PATH"; then
  public_changed=0
fi
if [ "$public_changed" -eq 1 ]; then
  install -D -m 0644 -o root -g root "$public" "$PUBLIC_KEY_PATH"
else
  chmod 0644 "$PUBLIC_KEY_PATH"
  chown root:root "$PUBLIC_KEY_PATH"
fi

dropin_path="/etc/systemd/system/mde-shell-egui.service.d/50-music-action-credential.conf"
if [ -L "$dropin_path" ]; then
  echo "provision-music-action-credential: refusing symlinked drop-in: $dropin_path" >&2
  exit 1
fi
dropin_changed=0
if [ -f "$dropin_path" ] && cmp -s "$DROPIN_SOURCE" "$dropin_path"; then
  chmod 0644 "$dropin_path"
  chown root:root "$dropin_path"
else
  install -D -m 0644 -o root -g root "$DROPIN_SOURCE" "$dropin_path"
  dropin_changed=1
fi

materialized_changed=$((credential_changed || public_changed || dropin_changed))
if [ "$materialized_changed" -eq 1 ]; then
  echo "provision-music-action-credential: installed host-bound Music authorization material"
else
  echo "provision-music-action-credential: Music authorization material already current"
fi
if [ "$materialized_changed" -eq 1 ] || [ "$force_restart" -eq 1 ]; then
  systemctl daemon-reload
fi
if [ "$refresh" -eq 1 ] && [ "$force_restart" -eq 1 ]; then
  systemctl try-restart mde-shell-egui.service
elif [ "$refresh" -eq 1 ] && [ "$materialized_changed" -eq 1 ]; then
  echo "provision-music-action-credential: staged for next controlled shell restart"
fi
