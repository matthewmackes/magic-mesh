#!/usr/bin/env bash
# testvm-up.sh — bring up the throwaway VDI test-endpoint VM(s) on an XCP-ng dom0
# (TESTVM-1/2, design: docs/design/vdi-test-endpoints.md).
#
# Creates `testvm-lin`: a minimal Alpine guest on the dom0 LAN bridge (DHCP,
# 172.20.x) serving an in-guest VNC endpoint (:5900, x11vnc) and an in-guest
# Spice endpoint (:5930, Xspice — falls back to a tiny qemu -spice server if
# Xspice can't start; on Alpine 3.24 Xspice's Xorg module segfaults [bit-rotted
# upstream], so the qemu spice channel IS the live path — the Spice viewer sees
# the nested qemu's SeaBIOS framebuffer, a real spice-protocol endpoint).
# No/trivial auth BY DESIGN — these are throwaway
# "does the viewer connect" targets for the shell's Desktop/VDI surface,
# never fleet infra. Root + `alpine` passwords are `testvm`; the farm key
# (~/.ssh/mackes_mesh_ed25519) is authorized for root SSH debug.
#
# Pipeline (runs from the build host; needs qemu-img + genisoimage locally):
#   1. probe both farm dom0s' free memory, pick the one with headroom
#   2. mirror the Alpine "generic cloudinit" cloud image into /root/mcnf-images
#      (sha512-verified; the dom0s + build host have LAN egress, so this is the
#      airgap-safe mirror step — nothing in the guest path needs a proxy)
#   3. qemu-img convert -> raw, grown to $TESTVM_DISK_GIB (cloud-init resizefs
#      expands the partitionless ext4 root to fill it at first boot)
#   4. build a NoCloud seed ISO (user-data installs x11vnc/Xspice + boot service)
#   5. stream both to the dom0, xe vdi-create + vdi-import, assemble the VM
#      ("Other install media" HVM template, 1 vCPU / 1 GiB, VIF on xenbr0 with
#      a fixed locally-administered MAC), vm-start
#   6. discover the DHCP address (xe-guest-utilities networks param, falling
#      back to the dom0 neighbor table — the guest pings the dom0 on boot),
#      then poll the VNC/Spice ports and check the RFB banner
#
# Usage:
#   install-helpers/testvm-up.sh              # auto-pick dom0, bring up testvm-lin
#   install-helpers/testvm-up.sh --host 172.20.0.9
#   install-helpers/testvm-up.sh --probe      # capacity + image/ISO recon only
#
# TEARDOWN: install-helpers/testvm-down.sh  (xe vm-shutdown --force +
#   vm-uninstall force=true — destroys the VM *and* its VDIs; see that script).
#   The VMs are named testvm-* and description-tagged THROWAWAY on purpose.
set -euo pipefail

# ---------------------------------------------------------------- config ----
DOM0S=(${TESTVM_DOM0S:-172.20.0.9 172.20.145.193})   # farm dom0s to consider
SSH_KEY="${TESTVM_SSH_KEY:-$HOME/.ssh/mackes_mesh_ed25519}"
VM_NAME="testvm-lin"
VM_MAC="5a:54:00:e5:70:01"          # fixed locally-administered MAC -> findable lease
DISK_GIB="${TESTVM_DISK_GIB:-2}"
MEM_MIB=1024
CACHE="${TESTVM_IMAGE_CACHE:-/root/mcnf-images}"
ALPINE_BRANCH="${TESTVM_ALPINE_BRANCH:-latest-stable}"
ALPINE_MIRROR="https://dl-cdn.alpinelinux.org/alpine"
VNC_PORT=5900
SPICE_PORT=5930

HOST=""; PROBE_ONLY=0
while [ $# -gt 0 ]; do case "$1" in
  --host)  HOST="$2"; shift 2;;
  --probe) PROBE_ONLY=1; shift;;
  -h|--help) sed -n '2,40p' "$0" | sed 's/^# \{0,1\}//'; exit 0;;
  *) echo "unknown arg: $1" >&2; exit 1;;
esac; done

SSH_OPTS=(-o BatchMode=yes -o ConnectTimeout=10 -o StrictHostKeyChecking=accept-new -i "$SSH_KEY")
d0() { ssh "${SSH_OPTS[@]}" "root@$1" "${@:2}"; }   # run on a dom0
log() { echo "==> testvm-up: $*" >&2; }   # stderr: safe inside $() capture
die() { echo "==> testvm-up: FATAL — $*" >&2; exit 1; }

# ------------------------------------------------- 1. capacity + recon ------
pick_dom0() {
  local best="" best_free=0 h free
  for h in "${DOM0S[@]}"; do
    free=$(d0 "$h" 'xe host-list params=memory-free --minimal' 2>/dev/null || echo 0)
    log "dom0 $h memory-free: $((free / 1024 / 1024)) MiB"
    if [ "${free:-0}" -gt "$best_free" ]; then best="$h"; best_free="$free"; fi
  done
  [ -n "$best" ] || die "no dom0 reachable (tried: ${DOM0S[*]}; key: $SSH_KEY)"
  [ "$best_free" -gt $((2 * 1024 * 1024 * 1024)) ] \
    || die "no dom0 has >2GiB free (best: $best at $best_free B)"
  echo "$best"
}

probe_images() {   # TESTVM-1 recon record: Alpine mirrored? Windows ISO/template?
  local h="$1"
  log "image recon on $h:"
  echo "    - alpine cache ($CACHE): $(ls "$CACHE"/generic_alpine-*cloudinit*.qcow2 2>/dev/null || echo ABSENT)"
  echo "    - dom0 ISO VDIs: $(d0 "$h" 'xe vdi-list params=name-label --minimal' | tr , '\n' | grep -i 'iso' | paste -sd' ' - || echo none)"
  echo "    - windows ISO on dom0: $(d0 "$h" 'xe vdi-list params=name-label --minimal' | tr , '\n' | grep -i -E 'win.*iso|iso.*win' || echo ABSENT)"
  echo "    - windows templates: $(d0 "$h" 'xe template-list params=name-label --minimal' | tr , '\n' | grep -ic windows || true) present (templates need install media — no local Windows ISO => TESTVM-3 falls back to Alpine+xrdp)"
}

if [ -z "$HOST" ]; then HOST=$(pick_dom0); fi
log "chosen dom0: $HOST"
probe_images "$HOST"
[ "$PROBE_ONLY" = 1 ] && { log "probe-only — done"; exit 0; }

d0 "$HOST" "xe vm-list name-label=$VM_NAME --minimal" | grep -q . \
  && die "$VM_NAME already exists on $HOST — run install-helpers/testvm-down.sh first"

# ------------------------------------------------- 2. mirror the image ------
mkdir -p "$CACHE"
IMG=$(curl -sf "$ALPINE_MIRROR/$ALPINE_BRANCH/releases/cloud/" \
      | grep -o 'generic_alpine-[0-9.]*-x86_64-bios-cloudinit-r[0-9]*\.qcow2' | sort -uV | tail -1)
[ -n "$IMG" ] || die "could not resolve the Alpine cloudinit image name from the mirror"
if [ ! -f "$CACHE/$IMG" ]; then
  log "mirroring $IMG"
  curl -sf -o "$CACHE/$IMG" "$ALPINE_MIRROR/$ALPINE_BRANCH/releases/cloud/$IMG"
fi
curl -sf -o "$CACHE/$IMG.sha512" "$ALPINE_MIRROR/$ALPINE_BRANCH/releases/cloud/$IMG.sha512" || true
if [ -s "$CACHE/$IMG.sha512" ]; then
  want=$(awk '{print $1}' "$CACHE/$IMG.sha512"); got=$(sha512sum "$CACHE/$IMG" | awk '{print $1}')
  [ "$want" = "$got" ] || die "sha512 mismatch on $IMG"
  log "image sha512 OK"
else
  log "WARNING: no sha512 published for $IMG — continuing unverified"
fi

RAW="$CACHE/${VM_NAME}.raw"
log "converting to raw + growing to ${DISK_GIB}GiB"
qemu-img convert -O raw "$CACHE/$IMG" "$RAW"
truncate -s "${DISK_GIB}G" "$RAW"
RAW_BYTES=$(stat -c%s "$RAW")

# ------------------------------------------------- 3. NoCloud seed ISO ------
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
PUBKEY=$(cat "${SSH_KEY}.pub")

cat > "$WORK/meta-data" <<EOF
instance-id: ${VM_NAME}-001
local-hostname: ${VM_NAME}
EOF

cat > "$WORK/user-data" <<EOF
#cloud-config
hostname: ${VM_NAME}
disable_root: false
ssh_pwauth: true
chpasswd:
  expire: false
  users:
    - {name: root, password: testvm, type: text}
    - {name: alpine, password: testvm, type: text}
write_files:
  - path: /etc/local.d/testvm-endpoints.start
    permissions: '0755'
    content: |
      #!/bin/sh
      # testvm-lin throwaway endpoints: VNC :${VNC_PORT} (x11vnc) + Spice :${SPICE_PORT}
      # (Xspice preferred; tiny qemu -spice fallback). Restart-safe via local.d.
      exec >>/var/log/testvm-endpoints.log 2>&1
      echo "=== \$(date) endpoints start"
      pgrep -f x11vnc >/dev/null 2>&1 && { echo already-running; exit 0; }
      port_up() { netstat -tln 2>/dev/null | grep -q ":\$1 "; }
      SPICE=none
      if command -v Xspice >/dev/null 2>&1; then
        Xspice --port ${SPICE_PORT} --disable-ticketing :1 >/var/log/xspice.log 2>&1 &
        for i in \$(seq 1 15); do port_up ${SPICE_PORT} && SPICE=Xspice && break; sleep 1; done
      fi
      if [ "\$SPICE" = none ]; then
        pkill -f Xspice 2>/dev/null || true
        Xvfb :1 -screen 0 1024x768x16 >/var/log/xvfb.log 2>&1 &
        if command -v qemu-system-x86_64 >/dev/null 2>&1; then
          qemu-system-x86_64 -m 32 -nodefaults -vga std \\
            -spice port=${SPICE_PORT},addr=0.0.0.0,disable-ticketing=on -daemonize \\
            >/var/log/qemu-spice.log 2>&1 && SPICE=qemu
        fi
      fi
      sleep 3
      DISPLAY=:1 xsetroot -solid "#224466" 2>/dev/null || true
      DISPLAY=:1 xterm -geometry 110x32+40+40 \\
        -e /bin/sh -lc "hostname; ip addr; exec /bin/sh" 2>/dev/null &
      x11vnc -display :1 -rfbport ${VNC_PORT} -nopw -forever -shared -bg
      echo "spice=\$SPICE vnc-port=\$(port_up ${VNC_PORT} && echo up || echo DOWN) spice-port=\$(port_up ${SPICE_PORT} && echo up || echo DOWN)"
runcmd:
  - mkdir -p /root/.ssh && chmod 700 /root/.ssh
  - echo "${PUBKEY}" >> /root/.ssh/authorized_keys
  - ping -c 3 ${HOST} || true
  - grep -q '/community\$' /etc/apk/repositories || sed -n 's|/main\$|/community|p' /etc/apk/repositories >> /etc/apk/repositories
  - apk update
  - apk add --no-progress xvfb x11vnc xterm xsetroot font-misc-misc xe-guest-utilities xspice xorg-server xf86-video-qxl || true
  - apk add --no-progress qemu-system-x86_64 qemu-ui-spice-core qemu-hw-display-qxl || true
  - rc-update add local default || true
  - rc-service xe-guest-utilities start 2>/dev/null || rc-update add xe-guest-utilities default || true
  - /etc/local.d/testvm-endpoints.start
  - ping -c 3 ${HOST} || true
EOF

genisoimage -quiet -output "$WORK/seed.iso" -volid cidata -joliet -rock \
  "$WORK/user-data" "$WORK/meta-data"
truncate -s 4M "$WORK/seed.iso"   # pad to a clean VDI size
SEED_BYTES=$(stat -c%s "$WORK/seed.iso")

# ------------------------------------------- 4. ship + import on the dom0 ---
SR=$(d0 "$HOST" 'xe pool-list params=default-SR --minimal')
[ -n "$SR" ] && d0 "$HOST" "xe sr-list uuid=$SR --minimal" | grep -q . \
  || SR=$(d0 "$HOST" 'xe sr-list name-label="Local storage" --minimal')
[ -n "$SR" ] || die "no usable SR on $HOST"
log "SR: $SR"

log "streaming root disk ($((RAW_BYTES / 1024 / 1024)) MiB raw, gzip over ssh)"
gzip -c "$RAW" | d0 "$HOST" "gunzip -c > /var/tmp/${VM_NAME}-root.raw"
gzip -c "$WORK/seed.iso" | d0 "$HOST" "gunzip -c > /var/tmp/${VM_NAME}-seed.iso"

ROOT_VDI=$(d0 "$HOST" "xe vdi-create sr-uuid=$SR name-label=${VM_NAME}-root type=user virtual-size=$RAW_BYTES")
SEED_VDI=$(d0 "$HOST" "xe vdi-create sr-uuid=$SR name-label=${VM_NAME}-seed type=user virtual-size=$SEED_BYTES")
log "importing VDIs (root=$ROOT_VDI seed=$SEED_VDI)"
d0 "$HOST" "xe vdi-import uuid=$ROOT_VDI filename=/var/tmp/${VM_NAME}-root.raw format=raw"
d0 "$HOST" "xe vdi-import uuid=$SEED_VDI filename=/var/tmp/${VM_NAME}-seed.iso format=raw"
d0 "$HOST" "rm -f /var/tmp/${VM_NAME}-root.raw /var/tmp/${VM_NAME}-seed.iso"

# ------------------------------------------------- 5. assemble the VM -------
VM=$(d0 "$HOST" "xe vm-install template='Other install media' new-name-label=$VM_NAME")
log "VM: $VM"
d0 "$HOST" "xe vm-param-set uuid=$VM name-description='THROWAWAY VDI test endpoint (VNC :$VNC_PORT / Spice :$SPICE_PORT) — tear down with install-helpers/testvm-down.sh'"
d0 "$HOST" "xe vm-memory-limits-set uuid=$VM static-min=$((512*1024*1024)) dynamic-min=$((MEM_MIB*1024*1024)) dynamic-max=$((MEM_MIB*1024*1024)) static-max=$((MEM_MIB*1024*1024))"
d0 "$HOST" "xe vm-param-set uuid=$VM VCPUs-max=1 VCPUs-at-startup=1 HVM-boot-params:order=c other-config:testvm=throwaway"
d0 "$HOST" "xe vbd-create vm-uuid=$VM vdi-uuid=$ROOT_VDI device=0 bootable=true mode=RW type=Disk" >/dev/null
d0 "$HOST" "xe vbd-create vm-uuid=$VM vdi-uuid=$SEED_VDI device=3 bootable=false mode=RO type=Disk" >/dev/null
NET=$(d0 "$HOST" 'xe network-list bridge=xenbr0 --minimal')
[ -n "$NET" ] || die "no xenbr0 network on $HOST"
d0 "$HOST" "xe vif-create vm-uuid=$VM network-uuid=$NET device=0 mac=$VM_MAC" >/dev/null
log "starting $VM_NAME"
d0 "$HOST" "xe vm-start uuid=$VM"

# ---------------------------------------- 6. discover IP + verify ports -----
log "waiting for a DHCP lease (fixed MAC $VM_MAC)…"
IP=""
for _ in $(seq 1 60); do   # ~5 min
  IP=$(d0 "$HOST" "xe vm-param-get uuid=$VM param-name=networks 2>/dev/null" \
       | grep -o '0/ip: [0-9.]*' | awk '{print $2}' || true)
  [ -n "$IP" ] && break
  IP=$(d0 "$HOST" "ip neigh | grep -i $VM_MAC | awk '{print \$1}' | head -1" || true)
  [ -n "$IP" ] && break
  sleep 5
done
[ -n "$IP" ] || die "no IP discovered for $VM_NAME after 5 min — check 'xe console' on $HOST / the EdgeOS DHCP pool"
log "$VM_NAME is at $IP"

port_open() { timeout 4 bash -c "exec 3<>/dev/tcp/$1/$2" 2>/dev/null; }
log "waiting for the endpoints (apk installs run over the LAN — allow ~5-10 min)…"
VNC_OK=0; SPICE_OK=0
for _ in $(seq 1 120); do
  [ "$VNC_OK" = 0 ] && port_open "$IP" "$VNC_PORT" && VNC_OK=1 && log "VNC :$VNC_PORT open"
  [ "$SPICE_OK" = 0 ] && port_open "$IP" "$SPICE_PORT" && SPICE_OK=1 && log "Spice :$SPICE_PORT open"
  [ "$VNC_OK" = 1 ] && [ "$SPICE_OK" = 1 ] && break
  sleep 5
done
RFB=$(timeout 5 bash -c "exec 3<>/dev/tcp/$IP/$VNC_PORT; head -c 12 <&3" 2>/dev/null | tr -d '\n' || true)

echo
log "================ RESULT ================"
log "dom0:   $HOST"
log "vm:     $VM_NAME ($VM)"
log "ip:     $IP"
log "vnc:    $IP:$VNC_PORT  open=$VNC_OK  banner='${RFB:-none}'"
log "spice:  $IP:$SPICE_PORT open=$SPICE_OK"
log "ssh:    root@$IP (farm key or password 'testvm')"
log "teardown: install-helpers/testvm-down.sh"
[ "$VNC_OK" = 1 ] && [ "$SPICE_OK" = 1 ] || {
  log "WARNING: an endpoint is not up yet — ssh in and check /var/log/testvm-endpoints.log"
  exit 2
}
