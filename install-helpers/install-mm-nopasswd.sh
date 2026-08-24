#!/usr/bin/env bash
# Install the Dell/Seat 15 mm passwordless sudo drop-in so seat agents can
# `sudo -n` without a promotion sidecar. Idempotent; does not overwrite an
# existing drop-in. visudo -cf is the gate.
set -euo pipefail

readonly DROPIN=/etc/sudoers.d/90-mm-nopasswd
readonly BODY='mm ALL=(ALL) NOPASSWD:ALL'

if [[ "${1:-}" == "--self-test" ]]; then
  [[ "$DROPIN" == /etc/sudoers.d/90-mm-nopasswd ]]
  [[ "$BODY" == 'mm ALL=(ALL) NOPASSWD:ALL' ]]
  grep -Fq 'visudo -cf' "$0"
  grep -Fq 'install -m 0440' "$0"
  echo 'install-mm-nopasswd: self-test passed'
  exit 0
fi

if (($#)); then
  echo "usage: $0 [--self-test]" >&2
  exit 2
fi

if [[ "$(id -u)" -ne 0 ]]; then
  echo 'install-mm-nopasswd: must run as root' >&2
  exit 1
fi

if ! id mm >/dev/null 2>&1; then
  echo 'install-mm-nopasswd: user mm is absent; nothing to do'
  exit 0
fi

command -v visudo >/dev/null 2>&1 || {
  echo 'install-mm-nopasswd: visudo is required' >&2
  exit 1
}

if [[ -e "$DROPIN" ]]; then
  echo "install-mm-nopasswd: $DROPIN already present"
  exit 0
fi

tmp="$(mktemp)"
cleanup() { rm -f "$tmp"; }
trap cleanup EXIT
printf '%s\n' "$BODY" >"$tmp"
chmod 0440 "$tmp"
visudo -cf "$tmp" >/dev/null
install -m 0440 -o root -g root "$tmp" "$DROPIN"
echo "install-mm-nopasswd: wrote $DROPIN"
