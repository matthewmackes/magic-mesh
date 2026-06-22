#!/usr/bin/env bash
# farm-testbed.sh — BUILD-PLATFORM-3: the snapshot-reset VM-pool test harness.
#
# Spins N CLEAN VMs from the MDE-VM-golden template (UEFI, generalized), each on a
# fresh static IP, for the internal install/feature/stability tests (L1/L2/L3).
# Every run is hermetic: `up` clones fresh, `down` destroys VM + disks. Isolated
# from the build VMs (.50/.51/.52) and the live fleet by name (mcnf-test-*) and IP
# range (172.20.0.60+). Defaults to XEN-BIGBOY (most headroom: 12c/32G).
#
# Usage:
#   farm-testbed.sh up <N>        clone+boot N test VMs; prints "name ip" per line
#   farm-testbed.sh ips           list running test VMs + IPs
#   farm-testbed.sh down          destroy ALL mcnf-test-* VMs (+ their disks)
#   farm-testbed.sh ssh <ip> ...  run a command on a test VM (as mm)
set -uo pipefail

DOM0="${MCNF_TESTBED_DOM0:-172.20.145.165}"        # XEN-BIGBOY
GOLDEN="${MCNF_TESTBED_GOLDEN:-MDE-VM-golden}"
BASE3="${MCNF_TESTBED_BASE:-172.20.0.6}"           # IPs 172.20.0.60 .. .69
GW="${MCNF_TESTBED_GW:-172.20.0.1}"
KEY="${MCNF_FARM_KEY:-$HOME/.ssh/mackes_mesh_ed25519}"
PUB="$KEY.pub"
SSHO="-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR -o BatchMode=yes -o ConnectTimeout=15"
xe() { local c="xe" a; for a in "$@"; do c="$c $(printf '%q' "$a")"; done; ssh -i "$KEY" $SSHO "root@$DOM0" "$c"; }
log() { echo "==> testbed: $*" >&2; }

seed_iso() { # <name> <ip> -> local path to seed.iso
  local name="$1" ip="$2" w; w="$(mktemp -d)"
  local pubkey; pubkey="$(cat "$PUB")"
  cat > "$w/user-data" <<UD
#cloud-config
hostname: $name
users:
  - name: mm
    sudo: ALL=(ALL) NOPASSWD:ALL
    shell: /bin/bash
    groups: [wheel]
    ssh_authorized_keys: [ "$pubkey" ]
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
      address1=$ip/16,$GW
      dns=8.8.8.8;1.1.1.1;
      [ipv6]
      method=ignore
  - path: /etc/cloud/cloud.cfg.d/99-disable-network-config.cfg
    permissions: '0644'
    content: |
      network: {config: disabled}
runcmd:
  - [ nmcli, connection, reload ]
  - [ sh, -c, "nmcli connection up static-primary || systemctl restart NetworkManager" ]
  - [ hostnamectl, set-hostname, "$name" ]
UD
  printf 'instance-id: %s-%s\nlocal-hostname: %s\n' "$name" "$RANDOM" "$name" > "$w/meta-data"
  ( cd "$w" && genisoimage -quiet -output seed.iso -volid cidata -joliet -rock user-data meta-data )
  local sz; sz=$(stat -c%s "$w/seed.iso"); truncate -s $(( (sz+1048575)/1048576*1048576 )) "$w/seed.iso"
  echo "$w/seed.iso"
}

cmd_up() {
  local n="${1:?usage: up <N>}"
  command -v genisoimage >/dev/null || { echo "genisoimage required" >&2; exit 1; }
  local gt; gt="$(xe template-list name-label="$GOLDEN" --minimal | tr -d '\r')"
  [ -n "$gt" ] || { echo "no template $GOLDEN on $DOM0" >&2; exit 1; }
  local sr; sr="$(xe sr-list name-label='Local storage' params=uuid --minimal | tr -d '\r')"
  for ((i=0; i<n; i++)); do
    local name="mcnf-test-$i" ip="${BASE3}$i"
    xe vm-list name-label="$name" --minimal 2>/dev/null | grep -q . && { log "$name exists — skip"; continue; }
    log "clone $GOLDEN -> $name @ $ip"
    local iso; iso="$(seed_iso "$name" "$ip")"
    ssh -i "$KEY" $SSHO "root@$DOM0" "cat > /tmp/$name-seed.iso" < "$iso"; rm -rf "$(dirname "$iso")"
    local vm; vm="$(xe vm-clone uuid="$gt" new-name-label="$name" | tr -d '\r')"
    xe vm-param-set uuid="$vm" is-a-template=false
    local seedsz; seedsz="$(ssh -i "$KEY" $SSHO root@$DOM0 "stat -c%s /tmp/$name-seed.iso" | tr -d '\r')"
    local svdi; svdi="$(xe vdi-create sr-uuid="$sr" name-label="$name-seed" type=user virtual-size="$seedsz" | tr -d '\r')"
    xe vdi-import uuid="$svdi" filename=/tmp/$name-seed.iso format=raw
    xe vbd-create vm-uuid="$vm" vdi-uuid="$svdi" device=1 bootable=false type=Disk mode=RO >/dev/null
    xe vm-start uuid="$vm"
    ssh -i "$KEY" $SSHO root@$DOM0 "rm -f /tmp/$name-seed.iso" || true
  done
  log "waiting for test VMs to answer SSH"
  for ((i=0; i<n; i++)); do
    local ip="${BASE3}$i"
    for t in $(seq 1 48); do timeout 3 bash -c "cat </dev/null >/dev/tcp/$ip/22" 2>/dev/null && break; sleep 5; done
    echo "mcnf-test-$i $ip"
  done
}

cmd_down() {
  for vm in $(xe vm-list params=uuid --minimal 2>/dev/null | tr ',' ' '); do
    local nm; nm="$(xe vm-param-get uuid="$vm" param-name=name-label 2>/dev/null | tr -d '\r')"
    case "$nm" in mcnf-test-*)
      log "destroy $nm"
      xe vm-shutdown uuid="$vm" force=true 2>/dev/null; sleep 1
      local vdis; vdis="$(xe vbd-list vm-uuid="$vm" type=Disk params=vdi-uuid --minimal | tr ',' ' ')"
      xe vm-destroy uuid="$vm"
      for d in $vdis; do [ -n "$d" ] && xe vdi-destroy uuid="$d" 2>/dev/null || true; done
    ;; esac
  done
  log "testbed clean"
}

cmd_ips() {
  for vm in $(xe vm-list power-state=running params=uuid --minimal 2>/dev/null | tr ',' ' '); do
    local nm; nm="$(xe vm-param-get uuid="$vm" param-name=name-label 2>/dev/null | tr -d '\r')"
    case "$nm" in mcnf-test-*) echo "$nm";; esac
  done
}

case "${1:-}" in
  up)   shift; cmd_up "$@" ;;
  down) cmd_down ;;
  ips)  cmd_ips ;;
  ssh)  shift; ip="$1"; shift; ssh -i "$KEY" $SSHO "mm@$ip" "$@" ;;
  *) echo "usage: farm-testbed.sh up <N> | ips | down | ssh <ip> <cmd>" >&2; exit 1 ;;
esac
