#!/usr/bin/env bash
# DS-5 — build the mcnf-golden template from a Fedora Cloud image on an XCP-ng pool.
# Pipeline: qcow2 → VHD → import VDI into the pool SR (via the dom0) → create a VM with a
# cloud-init seed → (caller runs Ansible to install MCNF) → generalize → mark template.
# Run from the control host. Idempotent-ish: re-running re-imports; clean up stale first.
#
# Usage: build-golden-fedora.sh <dom0-ip> <sr-name-label> [qcow2-path]
# Secrets: DOM0_PW must be exported (from the DS-8 store); never hardcoded here.
set -euo pipefail

DOM0="${1:?dom0 ip}"
SR_LABEL="${2:?SR name-label}"
QCOW="${3:-/var/tmp/golden-build/fedora44-cloud.qcow2}"
WORK="/var/tmp/golden-build"
VHD="$WORK/fedora44-cloud.vhd"
: "${DOM0_PW:?export DOM0_PW from the secret store}"
SSHP="sshpass -p $DOM0_PW ssh -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null root@$DOM0"

echo "==> 1. convert qcow2 → dynamic VHD (XCP-ng native disk format)"
[ -f "$QCOW" ] || { echo "missing $QCOW"; exit 1; }
qemu-img convert -p -O vpc -o subformat=dynamic "$QCOW" "$VHD"
VHD_BYTES=$(qemu-img info --output=json "$VHD" | jq -r '.["virtual-size"]')
echo "    VHD virtual-size: $VHD_BYTES bytes"

echo "==> 2. resolve SR uuid on $DOM0"
SR_UUID=$($SSHP "xe sr-list name-label='$SR_LABEL' --minimal" | tr -d '\r')
[ -n "$SR_UUID" ] || { echo "SR '$SR_LABEL' not found"; exit 1; }
echo "    SR=$SR_UUID"

echo "==> 3. create VDI + import the VHD (streamed over ssh)"
VDI_UUID=$($SSHP "xe vdi-create sr-uuid=$SR_UUID name-label=mcnf-golden-disk type=user virtual-size=$VHD_BYTES" | tr -d '\r')
echo "    VDI=$VDI_UUID"
$SSHP "xe vdi-import uuid=$VDI_UUID format=vhd filename=/dev/stdin" < "$VHD"

echo "==> 4. create the VM shell, attach the disk + a NIC"
TEMPLATE=$($SSHP "xe template-list name-label='Other install media' --minimal" | tr -d '\r')
VM_UUID=$($SSHP "xe vm-install template=$TEMPLATE new-name-label=mcnf-golden sr-uuid=$SR_UUID" | tr -d '\r')
# swap the empty install disk for our imported VDI
$SSHP "xe vm-disk-list uuid=$VM_UUID" >/dev/null 2>&1 || true
NET_UUID=$($SSHP "xe network-list bridge=xenbr0 --minimal" | tr -d '\r')
$SSHP "xe vif-create vm-uuid=$VM_UUID network-uuid=$NET_UUID device=0" >/dev/null
$SSHP "xe vbd-create vm-uuid=$VM_UUID vdi-uuid=$VDI_UUID device=0 bootable=true type=Disk mode=RW" >/dev/null
$SSHP "xe vm-param-set uuid=$VM_UUID VCPUs-max=2 VCPUs-at-startup=2"
$SSHP "xe vm-memory-limits-set uuid=$VM_UUID static-min=1GiB static-max=2GiB dynamic-min=2GiB dynamic-max=2GiB"

echo "==> golden base VM created: $VM_UUID"
echo "    NEXT (caller): seed cloud-init (control-host key), boot, run Ansible to install MCNF,"
echo "    then: install-helpers/build-mde-vm-golden.sh root@<ip>  &&"
echo "    xe vm-shutdown uuid=$VM_UUID --force && xe vm-param-set uuid=$VM_UUID is-a-template=true"
