#!/usr/bin/env bash
# setup-xcp-build-vm.sh — provision the MCNF build VM on an idle XCP-ng host so
# heavy compute (Rust/RPM/container builds) runs off the local AI/dev host
# (operator directive 2026-06-20). Idempotent-ish: re-run with --name to make a
# fresh one; it refuses if a VM by that name already exists.
#
# Method (no Fedora template needed on the host): convert a Fedora-Cloud qcow2
# to raw locally, `xe vdi-import` it into a fresh VDI on the host's Local
# storage, `vdi-resize` to the build disk size (cloud-init growpart expands the
# rootfs), attach a NoCloud cloud-init seed (static IP + the dev SSH key), and
# boot from a generic "Other install media" HVM template (BIOS). The build VM
# gets a deterministic static LAN IP so `xcp-build.sh` can always reach it.
#
# Prereqs (dev host): qemu-img, cloud-localds, sshpass, a Fedora-Cloud qcow2.
# Prereqs (XCP host): xe, `xe vdi-import` (present on XCP-ng 8.x), ~6 GB free on
# dom0 / for the staged raw, Local storage SR with room for the build disk.
#
# Usage:
#   setup-xcp-build-vm.sh --xcp-host 172.20.0.9 --xcp-pass <pw> \
#       [--name mcnf-build] [--ip 172.20.0.50/16] [--gw 172.20.0.1] \
#       [--vcpus 4] [--mem 16GiB] [--disk 80GiB] \
#       [--qcow2 ~/mesh-vms/fedora-cloud.qcow2] [--pubkey ~/.ssh/mackes_mesh_ed25519.pub]
set -euo pipefail

XCP_HOST=""; XCP_USER="root"; XCP_PASS=""
NAME="mcnf-build"; IPCIDR="172.20.0.50/16"; GW="172.20.0.1"; DNS="8.8.8.8 1.1.1.1"
VCPUS=4; MEM="16GiB"; DISK="80GiB"
QCOW2="$HOME/mesh-vms/fedora-cloud.qcow2"
PUBKEY="$HOME/.ssh/mackes_mesh_ed25519.pub"

while [ $# -gt 0 ]; do case "$1" in
  --xcp-host) XCP_HOST="$2"; shift 2;;
  --xcp-user) XCP_USER="$2"; shift 2;;
  --xcp-pass) XCP_PASS="$2"; shift 2;;
  --name) NAME="$2"; shift 2;;
  --ip) IPCIDR="$2"; shift 2;;
  --gw) GW="$2"; shift 2;;
  --vcpus) VCPUS="$2"; shift 2;;
  --mem) MEM="$2"; shift 2;;
  --disk) DISK="$2"; shift 2;;
  --qcow2) QCOW2="$2"; shift 2;;
  --pubkey) PUBKEY="$2"; shift 2;;
  *) echo "unknown arg: $1" >&2; exit 1;;
esac; done
[ -n "$XCP_HOST" ] && [ -n "$XCP_PASS" ] || { sed -n '20,30p' "$0" | sed 's/^# \{0,1\}//'; exit 1; }
for t in qemu-img genisoimage sshpass; do command -v "$t" >/dev/null || { echo "missing $t" >&2; exit 1; }; done
[ -s "$QCOW2" ] || { echo "missing qcow2: $QCOW2" >&2; exit 1; }
[ -s "$PUBKEY" ] || { echo "missing pubkey: $PUBKEY" >&2; exit 1; }

export SSHPASS="$XCP_PASS"
SSHOPTS="-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR -o ConnectTimeout=15"
# NOTE: ssh re-splits the remote command on spaces, so a value with spaces
# (e.g. template='Other install media', name-label='Local storage') would arrive
# at `xe` as separate words and never match. Shell-quote each arg with %q so the
# remote shell reassembles them intact.
xe() {
  local _c="xe" _a
  for _a in "$@"; do _c="$_c $(printf '%q' "$_a")"; done
  sshpass -e ssh $SSHOPTS "$XCP_USER@$XCP_HOST" "$_c"
}
log() { echo "==> build-vm: $*"; }
IP="${IPCIDR%%/*}"

xe vm-list name-label="$NAME" --minimal 2>/dev/null | grep -q . && {
  echo "a VM named '$NAME' already exists on $XCP_HOST — refusing" >&2; exit 1; }

WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
log "convert qcow2 → raw"
qemu-img convert -f qcow2 -O raw "$QCOW2" "$WORK/disk.raw"
RAW_BYTES="$(stat -c%s "$WORK/disk.raw")"

log "build NoCloud seed (static $IPCIDR, dev SSH key)"
PUB="$(cat "$PUBKEY")"
DNS_SEMI="$(echo "$DNS" | tr ' ' ';');"   # NM keyfile wants `8.8.8.8;1.1.1.1;`
# NETWORK — write the NetworkManager keyfile DIRECTLY, do not rely on cloud-init's
# netplan-v2 → NM rendering. On Fedora-Cloud + Xen HVM, cloud-init *parses* a v2
# network-config but renders NO keyfile (confirmed from a VM's own cloud-init.log:
# "applying net config ... 172.20.0.50/16" yet /etc/NetworkManager/system-connections
# stays empty), so the NIC falls back to DHCP — and a static-only LAN with no DHCP
# server leaves the VM permanently unreachable. This dark-VMed the whole farm. The
# keyfile has no interface-name so it binds the single ethernet NIC regardless of
# its name (Xen calls it enX0, not eth0/ens3). We also disable cloud-init's net
# management so nothing competes, and bring the connection up on first boot via
# runcmd (the keyfile alone is picked up automatically on every later boot).
cat > "$WORK/user-data" <<UD
#cloud-config
hostname: $NAME
users:
  - name: mm
    sudo: ALL=(ALL) NOPASSWD:ALL
    shell: /bin/bash
    groups: [wheel]
    ssh_authorized_keys:
      - "$PUB"
ssh_pwauth: false
growpart: { mode: auto, devices: ['/'], ignore_growroot_disabled: false }
write_files:
  - path: /etc/NetworkManager/system-connections/static-primary.nmconnection
    permissions: '0600'
    owner: root:root
    content: |
      [connection]
      id=static-primary
      type=ethernet
      autoconnect=true
      autoconnect-priority=999

      [ipv4]
      method=manual
      address1=$IPCIDR,$GW
      dns=$DNS_SEMI

      [ipv6]
      method=ignore
  - path: /etc/cloud/cloud.cfg.d/99-disable-network-config.cfg
    permissions: '0644'
    content: |
      network: {config: disabled}
runcmd:
  - [ nmcli, connection, reload ]
  - [ sh, -c, "nmcli connection up static-primary || systemctl restart NetworkManager" ]
UD
echo -e "instance-id: $NAME-001\nlocal-hostname: $NAME" > "$WORK/meta-data"
# Build the NoCloud seed ISO directly with genisoimage (cloud-localds isn't
# packaged on EL9; the seed is just a `cidata`-labelled ISO carrying
# user-data + meta-data at the root — what NoCloud reads).
( cd "$WORK" && genisoimage -quiet -output seed.iso -volid cidata -joliet -rock \
    user-data meta-data )
sz="$(stat -c%s "$WORK/seed.iso")"; pad=$(( (sz + 1048575) / 1048576 * 1048576 )); truncate -s "$pad" "$WORK/seed.iso"
SEED_BYTES="$(stat -c%s "$WORK/seed.iso")"

log "stage raw + seed onto dom0 /tmp"
sshpass -e scp $SSHOPTS "$WORK/disk.raw" "$WORK/seed.iso" "$XCP_USER@$XCP_HOST:/tmp/"

SR="$(xe sr-list name-label='Local storage' params=uuid --minimal | tr -d '\r')"
# Portability: the local SR isn't always named "Local storage" (it's ext on some
# hosts, lvm on others). Fall back to the first local user SR by type.
[ -n "$SR" ] || SR="$(xe sr-list type=ext params=uuid --minimal | tr -d '\r' | tr ',' '\n' | head -1)"
[ -n "$SR" ] || SR="$(xe sr-list type=lvm params=uuid --minimal | tr -d '\r' | tr ',' '\n' | head -1)"
[ -n "$SR" ] || { echo "no local SR (Local storage / ext / lvm) found on $XCP_HOST" >&2; exit 1; }
NET="$(xe pif-list management=true params=network-uuid --minimal | tr -d '\r')"
log "SR=$SR NET=$NET"

log "import root + seed VDIs; resize root → $DISK"
FVDI="$(xe vdi-create sr-uuid="$SR" name-label="$NAME-root" type=user virtual-size="$RAW_BYTES" | tr -d '\r')"
xe vdi-import uuid="$FVDI" filename=/tmp/disk.raw format=raw
xe vdi-resize uuid="$FVDI" disk-size="$DISK"
SVDI="$(xe vdi-create sr-uuid="$SR" name-label="$NAME-seed" type=user virtual-size="$SEED_BYTES" | tr -d '\r')"
xe vdi-import uuid="$SVDI" filename=/tmp/seed.iso format=raw

log "create VM ($VCPUS vCPU / $MEM) + attach disks + VIF + BIOS boot"
VM="$(xe vm-install template='Other install media' new-name-label="$NAME" sr-uuid="$SR" | tr -d '\r')"
for vbd in $(xe vbd-list vm-uuid="$VM" type=Disk params=uuid --minimal | tr ',' ' '); do
  vdi="$(xe vbd-param-get uuid="$vbd" param-name=vdi-uuid 2>/dev/null | tr -d '\r' || echo)"
  xe vbd-destroy uuid="$vbd" || true
  [ -n "$vdi" ] && [ "$vdi" != "<not in database>" ] && xe vdi-destroy uuid="$vdi" 2>/dev/null || true
done
xe vm-param-remove uuid="$VM" param-name=other-config param-key=disks 2>/dev/null || true
xe vm-memory-limits-set uuid="$VM" static-min=2GiB static-max="$MEM" dynamic-min="$MEM" dynamic-max="$MEM"
xe vm-param-set uuid="$VM" VCPUs-max="$VCPUS"
xe vm-param-set uuid="$VM" VCPUs-at-startup="$VCPUS"
xe vbd-create vm-uuid="$VM" vdi-uuid="$FVDI" device=0 bootable=true type=Disk mode=RW >/dev/null
xe vbd-create vm-uuid="$VM" vdi-uuid="$SVDI" device=1 bootable=false type=Disk mode=RO >/dev/null
xe vif-create vm-uuid="$VM" network-uuid="$NET" device=0 >/dev/null
xe vm-param-set uuid="$VM" HVM-boot-policy="BIOS order"
xe vm-param-set uuid="$VM" HVM-boot-params:order=c
# BUILD-FARM — survive a host reboot. Without auto_poweron the build VM stays
# halted after any dom0 reboot/shutdown (the live "build VM down" outage that
# blocked every GUI/farm build). XCP-ng gates per-VM auto-start on BOTH the VM
# flag AND the pool flag, so set both (idempotent).
xe vm-param-set uuid="$VM" other-config:auto_poweron=true
POOL="$(xe pool-list params=uuid --minimal | tr -d '\r')"
[ -n "$POOL" ] && xe pool-param-set uuid="$POOL" other-config:auto_poweron=true
xe vm-start uuid="$VM"
sshpass -e ssh $SSHOPTS "$XCP_USER@$XCP_HOST" "rm -f /tmp/disk.raw /tmp/seed.iso" || true

log "started; waiting for $IP"
for i in $(seq 1 40); do ping -c1 -W1 "$IP" >/dev/null 2>&1 && break; sleep 5; done
ping -c1 -W2 "$IP" >/dev/null 2>&1 && log "build VM up at $IP (VM=$VM)" \
  || log "VM started (VM=$VM) but $IP not yet pingable — check the XCP console"
echo "Next: install the toolchain, then drive builds with install-helpers/xcp-build.sh"
