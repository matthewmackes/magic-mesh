#!/usr/bin/bash
# Fixed root frontend for one exact pending Surface MOK request cancellation.
# The request id is public correlation data; the cloud-arm key is admitted only
# through the fixed systemd encrypted-credential contract below.
set -euo pipefail
readonly PATH=/usr/sbin:/usr/bin
export PATH

die() {
  printf 'cancel-surface-mok-import: %s\n' "$1" >&2
  exit 1
}

valid_request_id() {
  [[ "$1" =~ ^surface-mok-[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$ ]]
}

self_test() {
  valid_request_id surface-mok-12345678-1234-4abc-8def-1234567890ab
  ! valid_request_id surface-mok-12345678-1234-4abc-8def-1234567890AB
  ! valid_request_id ../surface-mok-12345678-1234-4abc-8def-1234567890ab
  ! valid_request_id surface-mok-12345678-1234-4abc-7def-1234567890ab
  local credential_contract='LoadCredentialEncrypted=cloud-arm-key:'
  credential_contract+='/etc/credstore.encrypted/cloud-arm-key'
  local fixed_command='/usr/bin/mackesd surface-mok-'
  fixed_command+='cancel --'
  [[ "$(grep -c -- "$credential_contract" "$0")" -eq 1 ]]
  [[ "$(grep -c -- "$fixed_command" "$0")" -eq 1 ]]
  printf 'cancel-surface-mok-import: self-test passed\n'
}

if [[ "${1:-}" == --self-test ]]; then
  [[ $# -eq 1 ]] || die '--self-test accepts no operands'
  self_test
  exit 0
fi

[[ "$(id -u)" -eq 0 ]] || die 'must run as root'
[[ $# -eq 1 ]] || die 'usage: cancel-surface-mok-import <surface-mok-request-id>'
valid_request_id "$1" || die 'request id is not an exact lowercase Surface MOK UUID'

exec /usr/bin/systemd-run --system --quiet --wait --collect \
  --property=Type=exec \
  --property=RuntimeMaxSec=20s \
  --property=TimeoutStopSec=5s \
  --property=LoadCredentialEncrypted=cloud-arm-key:/etc/credstore.encrypted/cloud-arm-key \
  /usr/bin/mackesd surface-mok-cancel -- "$1"
