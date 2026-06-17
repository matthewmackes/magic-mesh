#!/bin/bash
# build-mde-vm-golden.sh — XCP-2 / XPA-2,3,4: turn a prepared Fedora-cloud VM
# into a GENERALIZED golden image so every clone regenerates its own identity
# on first boot (no manual hostname/machine-id/ssh-host-key reset).
#
# The bug this fixes (found bringing up MDE-VM-1..4 on XCP, XPA-3): cloning a
# *booted* cloud image does NOT re-run cloud-init — clones inherited the base's
# hostname, machine-id, and SSH host keys. Generalizing resets cloud-init +
# strips the per-instance identity so the next boot (a clone, with a fresh
# NoCloud seed carrying a new instance-id) re-applies hostname/keys and
# regenerates machine-id + host keys.
#
# Usage (run from the dev box; the golden VM must be reachable + sudo-capable):
#   build-mde-vm-golden.sh <user@golden-ip>
# Then on the XCP host, halt it and mark it a template so it is never booted
# directly (clones come from the template):
#   xe vm-shutdown uuid=<golden> ; xe vm-param-set uuid=<golden> is-a-template=true
#
# After this, the XCP provisioner (XCP-3) clones the template, attaches a fresh
# per-VM NoCloud seed (instance-id + hostname MDE-VM-<n> + the op key), and a
# NEW VIF (fresh MAC, XPA-4) — and the clone self-identifies on first boot.
set -euo pipefail

TARGET="${1:-}"
[ -n "$TARGET" ] || { sed -n '2,28p' "$0" | sed 's/^# \{0,1\}//'; exit 1; }
SSH="ssh -o StrictHostKeyChecking=accept-new -o BatchMode=yes"

echo "==> generalizing golden $TARGET"
$SSH "$TARGET" 'sudo bash -s' <<'REMOTE'
set -e
# 1. Reset cloud-init so the NEXT boot is treated as a fresh instance and the
#    clone's new-instance-id seed re-runs (hostname, ssh keys, growpart, etc.).
command -v cloud-init >/dev/null && cloud-init clean --logs --seed || true
rm -rf /var/lib/cloud/instances/* /var/lib/cloud/instance 2>/dev/null || true
# 2. Strip the machine-id (systemd regenerates a unique one on next boot).
: > /etc/machine-id
rm -f /var/lib/dbus/machine-id
# 3. Remove SSH host keys (cloud-init ssh_deletekeys / sshd regenerate per host).
rm -f /etc/ssh/ssh_host_*
# 4. Tidy: logs, shell history, leftover stray dirs from any prior run.
rm -rf /var/log/journal/* /var/lib/mde/stray /var/lib/mde/qnm-stray-* 2>/dev/null || true
truncate -s0 /var/log/lastlog 2>/dev/null || true
rm -f /root/.bash_history /home/*/.bash_history 2>/dev/null || true
echo "generalized: cloud-init reset, machine-id + host keys stripped"
REMOTE

echo "==> done. Now halt the VM and mark it a template on the XCP host:"
echo "    xe vm-shutdown uuid=<golden-uuid> --force"
echo "    xe vm-param-set uuid=<golden-uuid> is-a-template=true"
