#!/usr/bin/env bash
# Fedora-modular glue for infra/ansible/node-virt.yml: enable virtqemud /
# virtnetworkd / virtstoraged / podman sockets, grant mm the libvirt group,
# and ensure a dir storage pool (mde-vms, default, or images).
set -euo pipefail

readonly SOCKETS=(
  virtqemud.socket
  virtnetworkd.socket
  virtstoraged.socket
  podman.socket
)
readonly POOLS=(mde-vms default images)
readonly POOL_TARGET=/var/lib/libvirt/images

if [[ "${1:-}" == "--self-test" ]]; then
  grep -Fq 'virtqemud.socket' "$0"
  grep -Fq 'podman.socket' "$0"
  grep -Fq 'mde-vms' "$0"
  grep -Fq 'pool-define-as default dir' "$0"
  grep -Fq 'usermod -aG libvirt' "$0"
  echo 'prepare-node-virt: self-test passed'
  exit 0
fi

if (($#)); then
  echo "usage: $0 [--self-test]" >&2
  exit 2
fi

if [[ "$(id -u)" -ne 0 ]]; then
  echo 'prepare-node-virt: must run as root' >&2
  exit 1
fi

enable_now() {
  local unit="$1"
  systemctl list-unit-files --no-legend --plain "$unit" 2>/dev/null | grep -q "^${unit}" || return 0
  systemctl enable --now "$unit" >/dev/null 2>&1 || return 0
}

for unit in "${SOCKETS[@]}"; do
  enable_now "$unit"
done

if getent group libvirt >/dev/null 2>&1; then
  for user in mm mde; do
    if id "$user" >/dev/null 2>&1; then
      usermod -aG libvirt "$user" >/dev/null 2>&1 || :
    fi
  done
fi

if ! command -v virsh >/dev/null 2>&1; then
  echo 'prepare-node-virt: virsh absent; sockets/groups only'
  exit 0
fi

install -d -m 0711 "$POOL_TARGET"
have_pool=0
for name in "${POOLS[@]}"; do
  if virsh --connect qemu:///system pool-info "$name" >/dev/null 2>&1; then
    virsh --connect qemu:///system pool-autostart "$name" >/dev/null 2>&1 || :
    virsh --connect qemu:///system pool-start "$name" >/dev/null 2>&1 || :
    have_pool=1
    break
  fi
done
if [[ "$have_pool" -eq 0 ]]; then
  virsh --connect qemu:///system pool-define-as default dir --target "$POOL_TARGET"
  virsh --connect qemu:///system pool-autostart default
  virsh --connect qemu:///system pool-start default
fi
echo 'prepare-node-virt: node virt stack prepared'
