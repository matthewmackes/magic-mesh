#!/usr/bin/env bash
# xcp-slots.sh — lifecycle for XCP build *slots* (the isolated build VMs that
# xcp-build.sh drives in parallel). One slot = one VM with its own warm cargo
# cache. This manages them on the XCP-ng dom0s and writes the .xcp-slots.conf
# registry xcp-build.sh reads.
#
# Operator directive 2026-06-20: heavy compute runs on XCP, the AI host stays
# local. Destructive ops (provision/destroy/rekey) change shared infra, so they
# require --yes. dom0 access is root over ssh; the password is read from the
# environment (MCNF_XCP_PASS) or --pass and is NEVER baked into this file.
#
# Usage:
#   xcp-slots.sh list    <dom0>                              read-only VM inventory
#   xcp-slots.sh provision <dom0> <name> <ip/cidr> --yes \
#                  [--gw 172.20.0.1] [--vcpus 4] [--mem 16GiB] [--disk 80GiB] \
#                  [--from-raw /tmp/fedora-build.raw] [--pubkey ~/.ssh/..pub] [--register a]
#                                                            fresh Fedora build VM w/ my key
#   xcp-slots.sh bootstrap <slot>                            install the rust/mold/deps toolchain (non-destructive)
#   xcp-slots.sh rekey   <dom0> <vm> --yes [--user mm] [--pubkey ..pub]
#                                                            offline-inject my key into an EXISTING VM (keeps warm cache)
#   xcp-slots.sh destroy <dom0> <vm> --yes                   shut down + delete a VM and its disks
#   xcp-slots.sh register <name> <host> <user> <key> <dir>   add a slot to .xcp-slots.conf
#   xcp-slots.sh slots                                       show the registry
#
# Env: MCNF_XCP_PASS (dom0 root pw) · MCNF_XCP_KEY (dom0 ssh key, if key-auth)
set -uo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
SLOTS_CONF="${MCNF_SLOTS_CONF:-$REPO/.xcp-slots.conf}"
PUBKEY_DEFAULT="$HOME/.ssh/mackes_mesh_ed25519.pub"
SSHO=(-o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR -o ConnectTimeout=15)
log() { echo "==> xcp-slots: $*" >&2; }
die() { echo "!! xcp-slots: $*" >&2; exit 1; }

# ---- dom0 transport (password via env/flag, or key) -----------------------
XCP_PASS="${MCNF_XCP_PASS:-}"; XCP_KEY="${MCNF_XCP_KEY:-}"; XCP_HOST=""
dom0() {
  if [ -n "$XCP_PASS" ]; then sshpass -p "$XCP_PASS" ssh "${SSHO[@]}" "root@$XCP_HOST" "$@"
  elif [ -n "$XCP_KEY" ]; then ssh -i "$XCP_KEY" "${SSHO[@]}" "root@$XCP_HOST" "$@"
  else ssh "${SSHO[@]}" "root@$XCP_HOST" "$@"; fi
}
xe() { dom0 xe "$@"; }
scp_dom0() {
  if [ -n "$XCP_PASS" ]; then sshpass -p "$XCP_PASS" scp "${SSHO[@]}" "$1" "root@$XCP_HOST:$2"
  else scp ${XCP_KEY:+-i "$XCP_KEY"} "${SSHO[@]}" "$1" "root@$XCP_HOST:$2"; fi
}
need_pass_or_key() { [ -n "$XCP_PASS" ] || [ -n "$XCP_KEY" ] || die "set MCNF_XCP_PASS (dom0 root pw) or MCNF_XCP_KEY"; }

register_slot() { # name host user key dir
  touch "$SLOTS_CONF"
  grep -vE "^$1[[:space:]]" "$SLOTS_CONF" > "$SLOTS_CONF.tmp" 2>/dev/null || true
  mv "$SLOTS_CONF.tmp" "$SLOTS_CONF" 2>/dev/null || true
  printf '%s %s %s %s %s\n' "$1" "$2" "$3" "$4" "$5" >> "$SLOTS_CONF"
  log "registered slot '$1' → $3@$2:$5"
}

# ---- subcommands ----------------------------------------------------------
cmd_list() {
  XCP_HOST="$1"; need_pass_or_key
  log "VM inventory on $XCP_HOST (read-only)"
  xe vm-list is-control-domain=false params=name-label,power-state,uuid,VCPUs-number,memory-actual 2>/dev/null
}

cmd_destroy() {
  XCP_HOST="$1"; local vm="$2"; need_pass_or_key
  local uuid; uuid="$(xe vm-list name-label="$vm" --minimal 2>/dev/null | tr -d '\r')"
  [ -n "$uuid" ] || die "no VM named '$vm' on $XCP_HOST"
  log "DESTROY $vm ($uuid) on $XCP_HOST — shutting down + deleting disks"
  xe vm-shutdown uuid="$uuid" 2>/dev/null || xe vm-shutdown uuid="$uuid" force=true 2>/dev/null || true
  for i in $(seq 1 12); do [ "$(xe vm-param-get uuid="$uuid" param-name=power-state 2>/dev/null|tr -d '\r')" = halted ] && break; sleep 3; done
  xe vm-uninstall uuid="$uuid" force=true 2>/dev/null \
    || die "vm-uninstall failed (try manually: xe vm-uninstall uuid=$uuid force=true)"
  log "destroyed $vm"
}

# Build a NoCloud cidata seed.iso locally (genisoimage; no cloud-localds needed).
build_seed() { # outfile name ipcidr gw pubkey
  local out="$1" name="$2" ipcidr="$3" gw="$4" pub; pub="$(cat "$5")"
  local dns="8.8.8.8, 1.1.1.1" work; work="$(mktemp -d)"
  cat > "$work/user-data" <<UD
#cloud-config
hostname: $name
users:
  - name: mm
    sudo: ALL=(ALL) NOPASSWD:ALL
    shell: /bin/bash
    groups: [wheel]
    ssh_authorized_keys: [ $pub ]
ssh_pwauth: false
growpart: { mode: auto, devices: ['/'], ignore_growroot_disabled: false }
UD
  printf 'instance-id: %s-%s\nlocal-hostname: %s\n' "$name" "$(date +%s)" "$name" > "$work/meta-data"
  cat > "$work/network-config" <<NC
version: 2
ethernets:
  primary:
    match: { name: "e*" }
    dhcp4: false
    addresses: [$ipcidr]
    routes: [ { to: default, via: $gw } ]
    nameservers: { addresses: [$dns] }
NC
  genisoimage -quiet -output "$out" -volid cidata -joliet -rock \
    "$work/user-data" "$work/meta-data" "$work/network-config"
  rm -rf "$work"
}

cmd_provision() {
  XCP_HOST="$1"; local name="$2" ipcidr="$3"; shift 3
  local gw="172.20.0.1" vcpus=4 mem="16GiB" disk="80GiB" raw="/tmp/fedora-build.raw"
  local pubkey="$PUBKEY_DEFAULT" reg=""
  while [ $# -gt 0 ]; do case "$1" in
    --gw) gw="$2"; shift 2;; --vcpus) vcpus="$2"; shift 2;; --mem) mem="$2"; shift 2;;
    --disk) disk="$2"; shift 2;; --from-raw) raw="$2"; shift 2;; --pubkey) pubkey="$2"; shift 2;;
    --register) reg="$2"; shift 2;; --yes) shift;; *) die "provision: unknown arg $1";; esac; done
  need_pass_or_key
  command -v genisoimage >/dev/null || die "genisoimage missing (dnf install -y genisoimage)"
  [ -s "$pubkey" ] || die "missing pubkey $pubkey"
  xe vm-list name-label="$name" --minimal 2>/dev/null | grep -q . && die "VM '$name' already exists on $XCP_HOST"
  dom0 "test -s $raw" || die "staged raw $raw not found on $XCP_HOST (stage a Fedora-Cloud raw there first)"
  local ip="${ipcidr%%/*}"
  log "provision '$name' on $XCP_HOST ($vcpus vCPU/$mem/$disk, static $ipcidr) from $raw"

  local seed; seed="$(mktemp --suffix=.iso)"; build_seed "$seed" "$name" "$ipcidr" "$gw" "$pubkey"
  scp_dom0 "$seed" "/tmp/$name-seed.iso"; rm -f "$seed"

  local SR NET RAWB SEEDB
  SR="$(xe sr-list name-label='Local storage' params=uuid --minimal | tr -d '\r')"
  NET="$(xe pif-list management=true params=network-uuid --minimal | tr -d '\r')"
  RAWB="$(dom0 "stat -c%s $raw" | tr -d '\r')"
  SEEDB="$(dom0 "stat -c%s /tmp/$name-seed.iso" | tr -d '\r')"
  log "SR=$SR NET=$NET"

  local FVDI SVDI VM
  FVDI="$(xe vdi-create sr-uuid="$SR" name-label="$name-root" type=user virtual-size="$RAWB" | tr -d '\r')"
  xe vdi-import uuid="$FVDI" filename="$raw" format=raw
  xe vdi-resize uuid="$FVDI" disk-size="$disk"
  SVDI="$(xe vdi-create sr-uuid="$SR" name-label="$name-seed" type=user virtual-size="$SEEDB" | tr -d '\r')"
  xe vdi-import uuid="$SVDI" filename="/tmp/$name-seed.iso" format=raw

  VM="$(xe vm-install template='Other install media' new-name-label="$name" sr-uuid="$SR" | tr -d '\r')"
  for vbd in $(xe vbd-list vm-uuid="$VM" type=Disk params=uuid --minimal | tr ',' ' '); do
    local vdi; vdi="$(xe vbd-param-get uuid="$vbd" param-name=vdi-uuid 2>/dev/null | tr -d '\r' || echo)"
    xe vbd-destroy uuid="$vbd" || true
    [ -n "$vdi" ] && [ "$vdi" != "<not in database>" ] && xe vdi-destroy uuid="$vdi" 2>/dev/null || true
  done
  xe vm-param-remove uuid="$VM" param-name=other-config param-key=disks 2>/dev/null || true
  xe vm-memory-limits-set uuid="$VM" static-min=2GiB static-max="$mem" dynamic-min="$mem" dynamic-max="$mem"
  xe vm-param-set uuid="$VM" VCPUs-max="$vcpus"; xe vm-param-set uuid="$VM" VCPUs-at-startup="$vcpus"
  xe vbd-create vm-uuid="$VM" vdi-uuid="$FVDI" device=0 bootable=true type=Disk mode=RW >/dev/null
  xe vbd-create vm-uuid="$VM" vdi-uuid="$SVDI" device=1 bootable=false type=Disk mode=RO >/dev/null
  xe vif-create vm-uuid="$VM" network-uuid="$NET" device=0 >/dev/null
  xe vm-param-set uuid="$VM" HVM-boot-policy="BIOS order"; xe vm-param-set uuid="$VM" HVM-boot-params:order=c
  xe vm-start uuid="$VM"; dom0 "rm -f /tmp/$name-seed.iso" || true

  log "started; waiting for $ip…"
  for i in $(seq 1 40); do ping -c1 -W1 "$ip" >/dev/null 2>&1 && break; sleep 5; done
  ping -c1 -W2 "$ip" >/dev/null 2>&1 && log "VM up at $ip (VM=$VM)" || log "VM started but $ip not pingable yet — check console"
  [ -n "$reg" ] && register_slot "$reg" "$ip" mm "${PUBKEY_DEFAULT%.pub}" magic-mesh
  log "next: xcp-slots.sh bootstrap ${reg:-$name}   (install the rust/mold/deps toolchain)"
}

cmd_bootstrap() { # <slot-name> — install the BUILD-FARM-2 toolchain on a keyed slot
  local slot="$1"
  local line; line="$(grep -E "^$slot[[:space:]]" "$SLOTS_CONF" 2>/dev/null)"
  [ -n "$line" ] || die "slot '$slot' not in $SLOTS_CONF — provision/register it first"
  read -r _ host user key dir _ <<<"$line"; key="${key/#\~/$HOME}"
  log "bootstrapping toolchain on slot '$slot' ($user@$host)"
  ssh -i "$key" "${SSHO[@]}" "$user@$host" 'bash -s' <<'BOOT'
set -e
sudo dnf install -y mold binutils protobuf-compiler gtk3-devel alsa-lib-devel \
  opus-devel openssl-devel cmake gcc gcc-c++ make git rsync podman rpm-build createrepo_c cloud-utils-growpart 2>&1 | tail -3
# GUI render/test stack (APPS-FIT etc): headless sway + grim + software Mesa
# (no GPU needed — preview-capture.sh uses WLR_RENDERER=pixman) + fonts for
# cosmic-text, so this slot can both BUILD and RENDER/screenshot the GUIs.
sudo dnf install -y sway grim mesa-dri-drivers mesa-vulkan-drivers mesa-libGL \
  google-noto-sans-fonts dejavu-sans-fonts dejavu-sans-mono-fonts 2>&1 | tail -2 || true
if ! command -v rustup >/dev/null && [ ! -x "$HOME/.cargo/bin/rustup" ]; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain 1.94.0 --profile minimal
fi
source "$HOME/.cargo/env"
rustup toolchain install 1.94.0 --profile minimal 2>/dev/null || true
rustup default 1.94.0
rustup component add clippy rustfmt
cargo install cargo-generate-rpm --version 0.21.0 2>/dev/null || cargo install cargo-generate-rpm || true
echo "=== toolchain ==="; rustc --version; cargo --version; mold --version | head -1
BOOT
  log "bootstrap done for '$slot'"
}

cmd_rekey() { # <dom0> <vm> [--user mm] [--pubkey f] — offline key inject (keeps cache)
  XCP_HOST="$1"; local vm="$2"; shift 2
  local user="mm" pubkey="$PUBKEY_DEFAULT"
  while [ $# -gt 0 ]; do case "$1" in --user) user="$2"; shift 2;; --pubkey) pubkey="$2"; shift 2;; --yes) shift;; *) die "rekey: unknown $1";; esac; done
  need_pass_or_key; [ -s "$pubkey" ] || die "missing pubkey $pubkey"
  local pub; pub="$(cat "$pubkey")"
  local uuid; uuid="$(xe vm-list name-label="$vm" --minimal | tr -d '\r')"; [ -n "$uuid" ] || die "no VM '$vm'"
  local dom0uuid rootvdi vbd
  dom0uuid="$(xe vm-list is-control-domain=true params=uuid --minimal | tr -d '\r' | head -c36)"
  # root VDI = the device-0 disk
  for b in $(xe vbd-list vm-uuid="$uuid" type=Disk params=uuid --minimal | tr ',' ' '); do
    [ "$(xe vbd-param-get uuid="$b" param-name=userdevice 2>/dev/null|tr -d '\r')" = 0 ] && \
      rootvdi="$(xe vbd-param-get uuid="$b" param-name=vdi-uuid|tr -d '\r')"
  done
  [ -n "${rootvdi:-}" ] || die "could not find device-0 root VDI for $vm"
  log "shutting down $vm to inject key offline (warm cache preserved)"
  xe vm-shutdown uuid="$uuid" 2>/dev/null || xe vm-shutdown uuid="$uuid" force=true || true
  for i in $(seq 1 12); do [ "$(xe vm-param-get uuid="$uuid" param-name=power-state|tr -d '\r')" = halted ] && break; sleep 3; done
  vbd="$(xe vbd-create vm-uuid="$dom0uuid" vdi-uuid="$rootvdi" device=autodetect type=Disk mode=RW | tr -d '\r')"
  xe vbd-plug uuid="$vbd"
  # mount + inject + unmount, all on the dom0
  dom0 "bash -s '$user' '$pub'" <<'INJECT'
set -e
USER="$1"; PUB="$2"
# newest plugged xvd* device
DEV="$(ls -1t /dev/xvd? 2>/dev/null | head -1)"; [ -n "$DEV" ] || { echo "no /dev/xvd* after plug"; exit 1; }
partprobe "$DEV" 2>/dev/null || true; sleep 1
MNT="$(mktemp -d)"; ROOTPART=""
# pick the largest mountable partition
for p in $(lsblk -nrpo NAME,FSTYPE "$DEV" | awk '$2 ~ /ext4|xfs|btrfs/ {print $1}'); do ROOTPART="$p"; done
[ -n "$ROOTPART" ] || { echo "no ext4/xfs/btrfs partition on $DEV"; lsblk "$DEV"; exit 1; }
FSTYPE="$(lsblk -nro FSTYPE "$ROOTPART" | head -1)"
if [ "$FSTYPE" = btrfs ]; then mount -o subvol=root "$ROOTPART" "$MNT" 2>/dev/null || mount "$ROOTPART" "$MNT"; else mount "$ROOTPART" "$MNT"; fi
HOME_DIR="$MNT/home/$USER"; [ -d "$HOME_DIR" ] || HOME_DIR="$MNT/root"
mkdir -p "$HOME_DIR/.ssh"; touch "$HOME_DIR/.ssh/authorized_keys"
grep -qF "$PUB" "$HOME_DIR/.ssh/authorized_keys" || echo "$PUB" >> "$HOME_DIR/.ssh/authorized_keys"
chmod 700 "$HOME_DIR/.ssh"; chmod 600 "$HOME_DIR/.ssh/authorized_keys"
# best-effort owner fix (uid lookup from the image's passwd)
UID_N="$(awk -F: -v u="$USER" '$1==u{print $3}' "$MNT/etc/passwd" 2>/dev/null)"
[ -n "$UID_N" ] && chown -R "$UID_N:$UID_N" "$HOME_DIR/.ssh" 2>/dev/null || true
sync; umount "$MNT"; rmdir "$MNT"
echo "injected key into $HOME_DIR/.ssh/authorized_keys"
INJECT
  xe vbd-unplug uuid="$vbd" || true; xe vbd-destroy uuid="$vbd" || true
  xe vm-start uuid="$uuid"
  log "rekeyed $vm; restarted. Verify: ssh -i ${pubkey%.pub} $user@<vm-ip> echo ok"
}

# ---- dispatch -------------------------------------------------------------
CMD="${1:-}"; shift || true
require_yes() { case " $* " in *" --yes "*) :;; *) die "$CMD is destructive — re-run with --yes";; esac; }
case "$CMD" in
  list)      [ $# -ge 1 ] || die "list <dom0>"; cmd_list "$1";;
  provision) [ $# -ge 3 ] || die "provision <dom0> <name> <ip/cidr> --yes [opts]"; require_yes "$@"; cmd_provision "$@";;
  bootstrap) [ $# -ge 1 ] || die "bootstrap <slot>"; cmd_bootstrap "$1";;
  rekey)     [ $# -ge 2 ] || die "rekey <dom0> <vm> --yes [opts]"; require_yes "$@"; cmd_rekey "$@";;
  destroy)   [ $# -ge 2 ] || die "destroy <dom0> <vm> --yes"; require_yes "$@"; cmd_destroy "$1" "$2";;
  register)  [ $# -ge 5 ] || die "register <name> <host> <user> <key> <dir>"; register_slot "$@";;
  slots)     [ -f "$SLOTS_CONF" ] && cat "$SLOTS_CONF" || echo "(no $SLOTS_CONF yet)";;
  ""|-h|--help|help) sed -n '20,38p' "$0" | sed 's/^# \{0,1\}//';;
  *) die "unknown command '$CMD'";;
esac
