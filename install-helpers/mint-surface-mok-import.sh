#!/usr/bin/bash
# Private operator front-end for one local Surface MOK import. The password is
# read by mackesd directly from this process's stdin; it never becomes a shell
# variable, argument, environment value, Bus field, or log line.
set -euo pipefail
readonly PATH=/usr/sbin:/usr/bin
export PATH

if [ "$(id -u)" -ne 0 ]; then
  printf 'mint-surface-mok-import: must run as root\n' >&2
  exit 1
fi

restore_echo=0
restore() {
  if [ "$restore_echo" -eq 1 ]; then
    /usr/bin/stty echo < /dev/tty 2>/dev/null || :
    printf '\n' > /dev/tty 2>/dev/null || :
  fi
}
trap restore EXIT HUP INT TERM

if [ -t 0 ]; then
  printf 'Surface MOK password (8-16 ASCII letters/digits): ' > /dev/tty
  /usr/bin/stty -echo < /dev/tty
  restore_echo=1
fi

/usr/bin/systemd-run --system --quiet --pipe --wait --collect \
  --property=Type=exec \
  --property=RuntimeMaxSec=60s \
  --property=TimeoutStopSec=5s \
  --property=LoadCredentialEncrypted=cloud-arm-key:/etc/credstore.encrypted/cloud-arm-key \
  /usr/bin/mackesd surface-mok-mint
