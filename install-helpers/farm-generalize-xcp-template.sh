#!/bin/bash
# farm-generalize-xcp-template.sh — generalize a booted XCP build-template clone
# after farm toolchain baking.
#
# QUASAR-CLOUD standard images are built by the OpenStack DIB -> Glance pipeline;
# this helper is only for the XCP build farm's MDE-VM-golden template refresh.
# Use it on a throwaway clone after installing Rust/sccache/mold, then shut down
# that clone and mark it as the replacement template.
#
# Usage (run from the dev box; the clone must be reachable + sudo-capable):
#   farm-generalize-xcp-template.sh <user@clone-ip>
set -euo pipefail

TARGET="${1:-}"
[ -n "$TARGET" ] || { sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'; exit 1; }
SSH="ssh -o StrictHostKeyChecking=accept-new -o BatchMode=yes"

echo "==> generalizing farm XCP template clone $TARGET"
$SSH "$TARGET" 'sudo bash -s' <<'REMOTE'
set -e
# Make the next boot look like a fresh clone: cloud-init re-reads the new
# NoCloud seed, systemd regenerates machine-id, and sshd/cloud-init rebuilds
# host keys for the new instance.
command -v cloud-init >/dev/null && cloud-init clean --logs --seed || true
rm -rf /var/lib/cloud/instances/* /var/lib/cloud/instance 2>/dev/null || true
: > /etc/machine-id
rm -f /var/lib/dbus/machine-id
rm -f /etc/ssh/ssh_host_*
rm -rf /var/log/journal/* /var/lib/mde/stray /var/lib/mde/qnm-stray-* 2>/dev/null || true
truncate -s0 /var/log/lastlog 2>/dev/null || true
rm -f /root/.bash_history /home/*/.bash_history 2>/dev/null || true
echo "generalized: cloud-init reset, machine-id + host keys stripped"
REMOTE

echo "==> done. Now halt the clone and mark it as the farm template on the XCP host:"
echo "    xe vm-shutdown uuid=<clone-uuid> --force"
echo "    xe vm-param-set uuid=<clone-uuid> is-a-template=true"
