#!/usr/bin/env bash
# Rotate the fixed Browser VM guest account and seal the matching shell login as
# a host-bound systemd credential. Plaintext exists only in /run and process
# memory for the duration of this transaction.
set -euo pipefail
set +x
umask 077

readonly DOMAIN="${MCNF_BROWSER_VM_DOMAIN:-browser-vm}"
readonly USERNAME="mcnf-browser"
readonly CREDENTIAL_NAME="browser-vm-rdp"
readonly CREDENTIAL_PATH="${MCNF_BROWSER_VM_RDP_CREDENTIAL_PATH:-/etc/credstore.encrypted/browser-vm-rdp}"
readonly DROPIN_SOURCE="${MCNF_BROWSER_VM_RDP_DROPIN_SOURCE:-/usr/libexec/mackesd/browser-vm-rdp-credential.conf}"
readonly DROPIN_PATH="/etc/systemd/system/mde-shell-egui.service.d/55-browser-vm-rdp-credential.conf"
readonly WARNING_HELPER="${MCNF_SEAT_UPDATE_WARNING_HELPER:-/usr/libexec/mackesd/seat-update-warning}"

fallback_dropin_source() {
  local source=$DROPIN_SOURCE
  if [[ ! -r "$source" ]]; then
    source="$(cd "$(dirname "$0")/.." && pwd)/packaging/systemd/browser-vm-rdp-credential.conf"
  fi
    printf '%s\n' "$source"
}

fallback_warning_source() {
  local source=$WARNING_HELPER
  if [[ ! -r "$source" ]]; then
    source="$(cd "$(dirname "$0")" && pwd)/seat-update-warning.sh"
  fi
  printf '%s\n' "$source"
}

validate_password() {
  [[ "$1" =~ ^[0-9a-f]{64}$ ]]
}

self_test() {
  local source warning_source
  validate_password "$(printf '%064d' 0)"
  ! validate_password "not-a-password"
  source=$(fallback_dropin_source)
  grep -Fxq \
    'LoadCredentialEncrypted=browser-vm-rdp:/etc/credstore.encrypted/browser-vm-rdp' \
    "$source"
  warning_source=$(fallback_warning_source)
  grep -Fq 'AI-GENERATED-ALERT' "$warning_source"
  echo "provision-browser-vm-rdp-credential: self-test passed"
}

if [[ "${1:-}" == "--self-test" ]]; then
  (($# == 1)) || { echo "usage: $0 [--restart] | --self-test" >&2; exit 2; }
  self_test
  exit 0
fi

restart=0
if [[ "${1:-}" == "--restart" ]]; then
  restart=1
  shift
fi
(($# == 0)) || { echo "usage: $0 [--restart] | --self-test" >&2; exit 2; }

[[ "$(id -u)" -eq 0 ]] || {
  echo "provision-browser-vm-rdp-credential: must run as root" >&2
  exit 1
}
for command_name in systemd-creds virsh od tr base64 install cmp; do
  command -v "$command_name" >/dev/null 2>&1 || {
    echo "provision-browser-vm-rdp-credential: required command unavailable: $command_name" >&2
    exit 1
  }
done
[[ -x "$WARNING_HELPER" ]] || {
  echo "provision-browser-vm-rdp-credential: mandatory seat warning helper unavailable: $WARNING_HELPER" >&2
  exit 1
}
dropin_source=$(fallback_dropin_source)
[[ -r "$dropin_source" ]] || {
  echo "provision-browser-vm-rdp-credential: drop-in template unavailable" >&2
  exit 1
}
[[ "$(virsh domstate "$DOMAIN" 2>/dev/null | tr -d '[:space:]')" == running ]] || {
  echo "provision-browser-vm-rdp-credential: Browser VM is not running: $DOMAIN" >&2
  exit 1
}
virsh qemu-agent-command "$DOMAIN" '{"execute":"guest-ping"}' >/dev/null || {
  echo "provision-browser-vm-rdp-credential: Browser VM guest agent is unavailable" >&2
  exit 1
}

# The warning helper both publishes the exact red alert flag and enforces the
# five-second interval. If publication fails, this transaction does not mutate.
"$WARNING_HELPER"

tmp_dir=$(mktemp -d /run/mcnf-browser-vm-rdp.XXXXXX)
trap 'unset password encoded agent_command; rm -rf -- "$tmp_dir"' EXIT
plain="$tmp_dir/plain.json"
encrypted="$tmp_dir/encrypted"

password=$(od -An -N32 -tx1 /dev/urandom | tr -d ' \n')
validate_password "$password" || {
  echo "provision-browser-vm-rdp-credential: CSPRNG output validation failed" >&2
  exit 1
}
printf '{"schema_version":1,"username":"%s","password":"%s"}\n' \
  "$USERNAME" "$password" >"$plain"

systemd-creds encrypt --name="$CREDENTIAL_NAME" "$plain" "$encrypted" >/dev/null
cmp -s "$plain" <(systemd-creds decrypt --name="$CREDENTIAL_NAME" "$encrypted" - 2>/dev/null) || {
  echo "provision-browser-vm-rdp-credential: encrypted credential verification failed" >&2
  exit 1
}

# Use virsh's stdin command stream so the guest password does not appear in the
# host process argument list. QGA requires the password bytes as base64.
encoded=$(printf '%s' "$password" | base64 -w0)
agent_command=$(printf \
  '{"execute":"guest-set-user-password","arguments":{"username":"%s","password":"%s","crypted":false}}' \
  "$USERNAME" "$encoded")
if ! printf "qemu-agent-command %s '%s'\nquit\n" "$DOMAIN" "$agent_command" | virsh --quiet >/dev/null; then
  echo "provision-browser-vm-rdp-credential: guest password rotation failed" >&2
  exit 1
fi
unset password encoded agent_command

if [[ -f "$CREDENTIAL_PATH" && ! -L "$CREDENTIAL_PATH" ]]; then
  install -D -m 0600 -o root -g root "$CREDENTIAL_PATH" "${CREDENTIAL_PATH}.previous"
fi
install -D -m 0600 -o root -g root "$encrypted" "$CREDENTIAL_PATH"
install -D -m 0644 -o root -g root "$dropin_source" "$DROPIN_PATH"
systemctl daemon-reload

echo "provision-browser-vm-rdp-credential: guest login rotated and host-bound credential installed"
if ((restart)); then
  systemctl restart mde-shell-egui.service
  systemctl is-active --quiet mde-shell-egui.service
  echo "provision-browser-vm-rdp-credential: shell restarted with the encrypted credential"
fi
