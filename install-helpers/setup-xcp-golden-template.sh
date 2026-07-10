#!/usr/bin/env bash
# setup-xcp-golden-template.sh — XCP-2: build the generalized MDE-VM-golden
# template from a Fedora-cloud image on an XCP-ng host (UEFI + cloud-init).
#
# A pristine Fedora *cloud* image is ALREADY generalized — no SSH host keys,
# empty machine-id; cloud-init regenerates both on first boot from the clone's
# fresh NoCloud seed. So we import the image straight into a VDI and mark the VM
# a template; no boot+generalize pass is needed. The farm-only
# `farm-generalize-xcp-template.sh` path is reserved for booted/customized build
# template clones after toolchain baking.
#
# The resulting template is what infra/tofu (and XCP-3's `provision spawn`) clone:
# the clone gets a fresh cloud-init seed (hostname/keys/network) at clone time.
#
# UEFI: XCP-ng 8.3 boots OVMF when HVM-boot-params:firmware=uefi (edk2 present);
# the Fedora cloud image is hybrid (BIOS-boot + EFI System Partition) so it boots
# either way — we pin UEFI to meet the XCP-2 acceptance.
#
# Usage:
#   setup-xcp-golden-template.sh --xcp-host 172.20.0.9 --xcp-pass <pw> \
#       [--name MDE-VM-golden] [--qcow2 /var/tmp/fedora-cloud.qcow2] \
#       [--mem 2GiB] [--vcpus 2] [--disk 20GiB]
# Auth: prefers the mesh key if it's already on the dom0; falls back to --xcp-pass
# (sshpass) for the first run.
set -euo pipefail

XCP_HOST=""; XCP_USER="root"; XCP_PASS=""
NAME="MDE-VM-golden"
QCOW2="/var/tmp/fedora-cloud.qcow2"
MEM="2GiB"; VCPUS=2; DISK="20GiB"
KEY="${MCNF_FARM_KEY:-$HOME/.ssh/mackes_mesh_ed25519}"
while [ $# -gt 0 ]; do case "$1" in
  --xcp-host) XCP_HOST="$2"; shift 2;;
  --xcp-pass) XCP_PASS="$2"; shift 2;;
  --name) NAME="$2"; shift 2;;
  --qcow2) QCOW2="$2"; shift 2;;
  --mem) MEM="$2"; shift 2;;
  --vcpus) VCPUS="$2"; shift 2;;
  --disk) DISK="$2"; shift 2;;
  -h|--help) sed -n '2,24p' "$0" | sed 's/^# \{0,1\}//'; exit 0;;
  *) echo "unknown arg: $1" >&2; exit 1;;
esac; done
[ -n "$XCP_HOST" ] || { echo "need --xcp-host" >&2; exit 1; }
for t in qemu-img; do command -v "$t" >/dev/null || { echo "missing $t" >&2; exit 1; }; done
[ -s "$QCOW2" ] || { echo "missing qcow2: $QCOW2" >&2; exit 1; }

# Transport: mesh key if it already works, else sshpass with --xcp-pass.
SSHBASE="-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR -o ConnectTimeout=15"
if ssh -i "$KEY" $SSHBASE -o BatchMode=yes "$XCP_USER@$XCP_HOST" true 2>/dev/null; then
  RUN() { ssh -i "$KEY" $SSHBASE "$XCP_USER@$XCP_HOST" "$@"; }
  SCP() { scp -i "$KEY" $SSHBASE "$@"; }
else
  [ -n "$XCP_PASS" ] || { echo "mesh key not authorized on $XCP_HOST and no --xcp-pass given" >&2; exit 1; }
  command -v sshpass >/dev/null || { echo "missing sshpass (needed for password auth)" >&2; exit 1; }
  export SSHPASS="$XCP_PASS"
  RUN() { sshpass -e ssh $SSHBASE "$XCP_USER@$XCP_HOST" "$@"; }
  SCP() { sshpass -e scp $SSHBASE "$@"; }
fi
# ssh re-splits the remote command on spaces → quote each xe arg with %q so a
# value with spaces (template='Other install media') arrives intact.
xe() { local _c="xe" _a; for _a in "$@"; do _c="$_c $(printf '%q' "$_a")"; done; RUN "$_c"; }
log() { echo "==> golden: $*"; }

xe vm-list name-label="$NAME" --minimal 2>/dev/null | grep -q . && {
  echo "a VM/template named '$NAME' already exists on $XCP_HOST — refusing (destroy it first)" >&2; exit 1; }

WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
log "convert qcow2 → raw"
qemu-img convert -f qcow2 -O raw "$QCOW2" "$WORK/disk.raw"
RAW_BYTES="$(stat -c%s "$WORK/disk.raw")"

log "stage raw onto dom0 /tmp"
SCP "$WORK/disk.raw" "$XCP_USER@$XCP_HOST:/tmp/golden.raw"

# Resolve a local SR by name, then by type (portability: not always "Local storage").
SR="$(xe sr-list name-label='Local storage' params=uuid --minimal | tr -d '\r')"
[ -n "$SR" ] || SR="$(xe sr-list type=ext params=uuid --minimal | tr -d '\r' | tr ',' '\n' | head -1)"
[ -n "$SR" ] || SR="$(xe sr-list type=lvm params=uuid --minimal | tr -d '\r' | tr ',' '\n' | head -1)"
[ -n "$SR" ] || { echo "no local SR on $XCP_HOST" >&2; exit 1; }
NET="$(xe pif-list management=true params=network-uuid --minimal | tr -d '\r')"
log "SR=$SR NET=$NET"

log "import root VDI; resize → $DISK"
FVDI="$(xe vdi-create sr-uuid="$SR" name-label="$NAME-root" type=user virtual-size="$RAW_BYTES" | tr -d '\r')"
xe vdi-import uuid="$FVDI" filename=/tmp/golden.raw format=raw
xe vdi-resize uuid="$FVDI" disk-size="$DISK"
RUN "rm -f /tmp/golden.raw" || true

log "create VM shell + attach disk + VIF"
VM="$(xe vm-install template='Other install media' new-name-label="$NAME" sr-uuid="$SR" | tr -d '\r')"
for vbd in $(xe vbd-list vm-uuid="$VM" type=Disk params=uuid --minimal | tr ',' ' '); do
  vdi="$(xe vbd-param-get uuid="$vbd" param-name=vdi-uuid 2>/dev/null | tr -d '\r' || echo)"
  xe vbd-destroy uuid="$vbd" || true
  [ -n "$vdi" ] && [ "$vdi" != "<not in database>" ] && xe vdi-destroy uuid="$vdi" 2>/dev/null || true
done
xe vm-param-remove uuid="$VM" param-name=other-config param-key=disks 2>/dev/null || true
xe vm-memory-limits-set uuid="$VM" static-min=1GiB static-max="$MEM" dynamic-min="$MEM" dynamic-max="$MEM"
xe vm-param-set uuid="$VM" VCPUs-max="$VCPUS"
xe vm-param-set uuid="$VM" VCPUs-at-startup="$VCPUS"
xe vbd-create vm-uuid="$VM" vdi-uuid="$FVDI" device=0 bootable=true type=Disk mode=RW >/dev/null
xe vif-create vm-uuid="$VM" network-uuid="$NET" device=0 >/dev/null

log "set UEFI firmware (OVMF) + boot from disk"
xe vm-param-set uuid="$VM" HVM-boot-policy="BIOS order"
xe vm-param-set uuid="$VM" HVM-boot-params:firmware=uefi
xe vm-param-set uuid="$VM" HVM-boot-params:order=c
xe vm-param-set uuid="$VM" platform:secureboot=false

log "mark as template"
xe vm-param-set uuid="$VM" is-a-template=true
xe template-param-set uuid="$VM" other-config:instant=true 2>/dev/null || true

log "DONE — $NAME built on $XCP_HOST (uuid=$VM, UEFI, $DISK base)"
echo "verify:  xe template-list name-label=$NAME"
echo "next:    set var.golden_template_name=\"$NAME\" in infra/tofu, then tofu apply"
