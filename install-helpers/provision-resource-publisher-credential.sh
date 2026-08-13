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
readonly RECEIPT_DIR="${MCNF_RESOURCE_PUBLISHER_RECEIPT_DIR:-/etc/mcnf/release-inputs/resource-publisher}"
readonly RECEIPT_PATH="$RECEIPT_DIR/resource-publisher-receipt.json"
readonly RECEIPT_SIGNATURE="$RECEIPT_DIR/resource-publisher-receipt.json.asc"
readonly RELEASE_PUBLIC_KEY="${MCNF_RELEASE_PUBLIC_KEY:-/etc/pki/rpm-gpg/RPM-GPG-KEY-magic-mesh}"
readonly ROLE_FILE="${MCNF_ROLE_FILE:-/var/lib/mde/role.toml}"
readonly EX_DATAERR=65
readonly EX_NOINPUT=66
readonly EX_TEMPFAIL=75
readonly EX_CONFIG=78

validate_key() {
  local value bytes lines
  if LC_ALL=C grep -q '[[:cntrl:]]' "$1"; then
    return 1
  fi
  value="$(<"$1")"
  bytes="$(LC_ALL=C wc -c <"$1")"
  lines="$(LC_ALL=C wc -l <"$1")"
  [ -n "$value" ] && [ "$bytes" -gt 0 ] && [ "$bytes" -le 4096 ] && [ "$lines" -eq 0 ]
}

validate_receipt() {
  local receipt="$1" signature="$2" public_key="$3" credential="$4" role_file="$5" node="$6"
  local keyring="$7" status="$8" expected_hash actual_hash role signer path identity after target
  for path in "$receipt" "$signature" "$public_key" "$role_file"; do
    [ -f "$path" ] && [ ! -L "$path" ] && [ "$(stat -c %h "$path")" -eq 1 ] || return 1
    [ "$(( 8#$(stat -c %a "$path") & 8#022 ))" -eq 0 ] || return 1
    identity="$(stat -Lc '%d:%i:%s:%y:%z' "$path")" || return 1
    case "$path" in
      "$receipt") target="$status.receipt" ;;
      "$signature") target="$status.signature" ;;
      "$public_key") target="$status.public-key" ;;
      "$role_file") target="$status.role" ;;
      *) return 1 ;;
    esac
    cp --no-preserve=all -- "$path" "$target" || return 1
    after="$(stat -Lc '%d:%i:%s:%y:%z' "$path")" || return 1
    [ "$identity" = "$after" ] || return 1
  done
  receipt="$status.receipt"
  signature="$status.signature"
  public_key="$status.public-key"
  role_file="$status.role"
  gpg --batch --yes --dearmor --output "$keyring" "$public_key" >/dev/null 2>&1 || return 1
  gpgv --status-fd 1 --keyring "$keyring" "$signature" "$receipt" >"$status" 2>/dev/null || return 1
  signer="$(sed -n 's/^\[GNUPG:\] VALIDSIG \([0-9A-F]\{40,64\}\) .*/\1/p' "$status")"
  [ -n "$signer" ] && [ "$(printf '%s\n' "$signer" | wc -l)" -eq 1 ] || return 1
  python3 - "$receipt" "$node" "$signer" >"$status.json" <<'PY'
import json, re, sys
path, node, signer = sys.argv[1:]
try:
    value = json.load(open(path, encoding="ascii"))
except (OSError, UnicodeError, json.JSONDecodeError):
    raise SystemExit(1)
expected = {"schema_version", "kind", "publisher_identity", "credential_sha256", "source_revision", "target_node", "target_role"}
if set(value) != expected or value["schema_version"] != 1 or value["kind"] != "mcnf-resource-publisher-credential":
    raise SystemExit(1)
if value["publisher_identity"] != f"openpgp-primary:{signer}" or value["target_node"] != node:
    raise SystemExit(1)
if value["target_role"] not in {"lighthouse", "workstation"}:
    raise SystemExit(1)
if not re.fullmatch(r"[0-9a-f]{64}", value["credential_sha256"]) or not re.fullmatch(r"[0-9a-f]{40}", value["source_revision"]):
    raise SystemExit(1)
print(value["credential_sha256"])
print(value["target_role"])
PY
  expected_hash="$(sed -n '1p' "$status.json")"
  role="$(sed -n '2p' "$status.json")"
  [ -n "$expected_hash" ] && [ -n "$role" ] || return 1
  [ "$(grep -Ec '^[[:space:]]*role[[:space:]]*=' "$role_file")" -eq 1 ] || return 1
  grep -Eq "^[[:space:]]*role[[:space:]]*=[[:space:]]*\"?${role}\"?[[:space:]]*$" "$role_file" || return 1
  actual_hash="$(sha256sum "$credential" | cut -d' ' -f1)"
  [ "$actual_hash" = "$expected_hash" ]
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
  local test_dir good bad template unit hostile fingerprint receipt signature public_key role_file keyring status
  test_dir="$(mktemp -d)"
  good="$test_dir/good"
  bad="$test_dir/bad"
  printf 'publisher-key-from-secret-store' >"$good"
  printf 'publisher-key\n' >"$bad"
  validate_key "$good"
  if validate_key "$bad"; then
    echo "provision-resource-publisher-credential: multiline key was accepted" >&2
    return 1
  fi
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

  # Exercise the exact installed admission boundary with an ephemeral governed
  # signer. The receipt is public metadata; only the HMAC input remains secret.
  mkdir -m 0700 "$test_dir/gnupg"
  GNUPGHOME="$test_dir/gnupg" gpg --batch --passphrase '' --quick-generate-key \
    'FUNC-019 materializer fixture <fixture.invalid>' ed25519 sign 0 >/dev/null 2>&1
  fingerprint="$(GNUPGHOME="$test_dir/gnupg" gpg --batch --with-colons --list-secret-keys \
    | sed -n 's/^fpr:::::::::\([0-9A-F]\{40,64\}\):$/\1/p' | head -n1)"
  [ -n "$fingerprint" ]
  public_key="$test_dir/release.asc"
  GNUPGHOME="$test_dir/gnupg" gpg --batch --armor --export "$fingerprint" >"$public_key"
  receipt="$test_dir/resource-publisher-receipt.json"
  signature="$receipt.asc"
  role_file="$test_dir/role.toml"
  python3 - "$receipt" "$fingerprint" "$(sha256sum "$good" | cut -d' ' -f1)" <<'PY'
import json, sys
path, fingerprint, digest = sys.argv[1:]
value = {"schema_version": 1, "kind": "mcnf-resource-publisher-credential",
         "publisher_identity": f"openpgp-primary:{fingerprint}",
         "credential_sha256": digest, "source_revision": "1" * 40,
         "target_node": "peer:test-node", "target_role": "workstation"}
open(path, "w", encoding="ascii").write(json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n")
PY
  GNUPGHOME="$test_dir/gnupg" gpg --batch --armor --detach-sign --local-user "$fingerprint" \
    --output "$signature" "$receipt"
  printf 'role = "workstation"\n' >"$role_file"
  chmod 0400 "$public_key" "$receipt" "$signature" "$role_file"
  keyring="$test_dir/verify.gpg"
  status="$test_dir/verify"
  validate_receipt "$receipt" "$signature" "$public_key" "$good" "$role_file" \
    'peer:test-node' "$keyring" "$status"
  chmod 0600 "$role_file"
  printf 'role = "lighthouse"\n' >"$role_file"
  chmod 0400 "$role_file"
  if validate_receipt "$receipt" "$signature" "$public_key" "$good" "$role_file" \
    'peer:test-node' "$keyring.bad-role" "$status.bad-role"; then
    echo "provision-resource-publisher-credential: wrong-role receipt was accepted" >&2
    return 1
  fi
  chmod 0600 "$role_file"
  printf 'role = "workstation"\n' >"$role_file"
  chmod 0400 "$role_file"
  if validate_receipt "$receipt" "$signature" "$public_key" "$bad" "$role_file" \
    'peer:test-node' "$keyring.bad-key" "$status.bad-key"; then
    echo "provision-resource-publisher-credential: wrong publisher key was accepted" >&2
    return 1
  fi
  chmod 0600 "$receipt"
  printf '{"replaced":true}\n' >"$receipt"
  chmod 0400 "$receipt"
  if validate_receipt "$receipt" "$signature" "$public_key" "$good" "$role_file" \
    'peer:test-node' "$keyring.replaced" "$status.replaced"; then
    echo "provision-resource-publisher-credential: replaced receipt was accepted" >&2
    return 1
  fi

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
for command_name in systemd-creds install grep wc cmp chmod chown stat cp gpg gpgv python3 sha256sum hostname sed cut; do
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
keyring="$tmp_dir/release.gpg"
verify_status="$tmp_dir/verify"

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
node="peer:$(hostname -s)"
validate_receipt "$RECEIPT_PATH" "$RECEIPT_SIGNATURE" "$RELEASE_PUBLIC_KEY" "$plain" "$ROLE_FILE" "$node" "$keyring" "$verify_status" || {
  echo "provision-resource-publisher-credential: signed release receipt is missing, ambiguous, replaced, out of scope, or does not bind the approved publisher key" >&2
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
