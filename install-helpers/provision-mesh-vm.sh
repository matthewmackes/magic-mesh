#!/usr/bin/env bash
# OBS-1/test-bed — provision a Fedora cloud VM for live mesh testing
# (Nebula data-plane + Cosmic-visual acceptances). Idempotent-ish;
# re-run with a fresh NAME. Needs: libvirt + virt-install + cloud-utils
# + passwordless sudo + /dev/kvm. The Fedora base image is fetched once.
#
#   ./provision-mesh-vm.sh <name> [overlay-ip]
#
# Then: ssh -i ~/mesh-vms/id_mesh mm@<vm-ip>
set -euo pipefail
NAME="${1:?usage: provision-mesh-vm.sh <name> [overlay-ip]}"
VMDIR="$HOME/mesh-vms"; IMG="$VMDIR/fedora-cloud.qcow2"
mkdir -p "$VMDIR"
URL="https://download.fedoraproject.org/pub/fedora/linux/releases/42/Cloud/x86_64/images/Fedora-Cloud-Base-Generic-42-1.1.x86_64.qcow2"
[ -s "$IMG" ] || curl -sL -o "$IMG" "$URL"
[ -f "$VMDIR/id_mesh" ] || ssh-keygen -t ed25519 -N "" -f "$VMDIR/id_mesh" -q
PUB=$(cat "$VMDIR/id_mesh.pub")
SEED="$VMDIR/seed-$NAME"; mkdir -p "$SEED"
cat > "$SEED/user-data" <<UD
#cloud-config
hostname: $NAME
users:
  - name: mm
    sudo: ALL=(ALL) NOPASSWD:ALL
    shell: /bin/bash
    ssh_authorized_keys:
      - "$PUB"
ssh_pwauth: false
packages: [ nebula ]
UD
echo "instance-id: $NAME; local-hostname: $NAME" > "$SEED/meta-data"
# NoCloud seed via genisoimage (cloud-localds isn't packaged on EL9) — a
# `cidata`-labelled ISO of user-data + meta-data, what NoCloud reads.
( cd "$SEED" && genisoimage -quiet -output "$VMDIR/seed-$NAME.iso" -volid cidata \
    -joliet -rock user-data meta-data )
sudo cp "$IMG" "/var/lib/libvirt/images/$NAME.qcow2"
sudo qemu-img resize "/var/lib/libvirt/images/$NAME.qcow2" 12G
sudo cp "$VMDIR/seed-$NAME.iso" "/var/lib/libvirt/images/seed-$NAME.iso"
sudo virt-install --name "$NAME" --memory 2048 --vcpus 2 \
  --disk "/var/lib/libvirt/images/$NAME.qcow2",device=disk,bus=virtio \
  --disk "/var/lib/libvirt/images/seed-$NAME.iso",device=cdrom \
  --os-variant fedora-unknown --import --network network=default,model=virtio \
  --graphics none --noautoconsole
echo "provisioned $NAME — find its IP with: sudo virsh domifaddr $NAME"
