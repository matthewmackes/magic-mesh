#!/bin/bash
# setup-workstation-passthrough.sh — DATACENTER-22: the Enhanced Workstation
# profile's PCI-passthrough + Primary-Desktop-VM auto-launch host configuration.
#
# The Enhanced Workstation stack (docs/design/datacenter-control.md §7):
#
#   Hardware → XCP-ng → dom0 (hidden, management-only)
#            → Primary Desktop VM (owns monitor/keyboard/mouse/audio via PCI
#              passthrough; auto-launches at boot) → the user experiences the VM
#              as the local desktop. A small management VM mediates the console
#              and can reclaim the display for recovery if the desktop VM fails.
#
# This script configures that on an XCP-ng dom0: it hides the desktop's GPU / USB
# / audio PCI functions from dom0 (binds them to `xen-pciback` at boot), assigns
# them to the Primary Desktop VM, and arms pool+VM auto-poweron so the VM owns the
# console from boot. Run it ON the dom0 (it uses the local `xe` + `xen-cmdline`).
#
# SAFE BY DEFAULT: with no --apply this is a DRY RUN — it detects the candidate
# devices and PRINTS the exact plan (the xen-pciback hide list, the VM `pci=`
# assignment, the auto-poweron flips) WITHOUT changing anything. --apply performs
# the config; the xen-pciback hide takes effect only after a dom0 REBOOT (the
# script reminds you, never reboots for you).
#
# ⚠️ LIVE-HARDWARE VERIFICATION IS BLOCKED: actually confirming the GPU drives a
# physical monitor from inside the VM needs a passthrough-capable GPU + IOMMU on a
# real desktop host (the build farm is headless server VMs). This script encodes
# the config; the end-to-end "display owned by the VM" check is operator-gated on
# such hardware (WORKLIST DATACENTER-22).
#
# Usage (on the dom0):
#   setup-workstation-passthrough.sh --vm <name|uuid>            # auto-detect GPU/USB/audio, dry run
#   setup-workstation-passthrough.sh --vm <name> --pci 0000:01:00.0,0000:01:00.1
#   setup-workstation-passthrough.sh --vm <name> --apply         # perform it (reboot to take effect)
#   setup-workstation-passthrough.sh --vm <name> --no-usb --no-audio   # GPU only
set -euo pipefail

VM=""; PCI=""; APPLY=0; WANT_GPU=1; WANT_USB=1; WANT_AUDIO=1
while [ $# -gt 0 ]; do case "$1" in
  --vm) VM="$2"; shift 2;;
  --pci) PCI="$2"; shift 2;;
  --apply) APPLY=1; shift;;
  --no-gpu) WANT_GPU=0; shift;;
  --no-usb) WANT_USB=0; shift;;
  --no-audio) WANT_AUDIO=0; shift;;
  -h|--help) sed -n '2,33p' "$0" | sed 's/^# \{0,1\}//'; exit 0;;
  *) echo "unknown arg: $1" >&2; exit 1;;
esac; done
[ -n "$VM" ] || { echo "--vm <name|uuid> required (the Primary Desktop VM)" >&2; exit 2; }

log()  { echo "==> passthrough: $*" >&2; }
have() { command -v "$1" >/dev/null 2>&1; }

# Detect candidate passthrough PCI functions from lspci -Dnn by class, unless an
# explicit --pci list was given. PURE-ish: just reads lspci, prints BDFs.
detect_pci() {
  have lspci || { echo "lspci not found — pass --pci explicitly" >&2; exit 3; }
  local bdfs=()
  if [ "$WANT_GPU" -eq 1 ]; then
    # VGA (0300) + 3D (0302) + Display (0380) controllers.
    while read -r b; do [ -n "$b" ] && bdfs+=("$b"); done < <(lspci -Dnn -d ::0300 -d ::0302 -d ::0380 2>/dev/null | awk '{print $1}')
  fi
  if [ "$WANT_AUDIO" -eq 1 ]; then
    # Audio devices (0403 HDA) — typically the GPU's companion HDMI-audio function.
    while read -r b; do [ -n "$b" ] && bdfs+=("$b"); done < <(lspci -Dnn -d ::0403 2>/dev/null | awk '{print $1}')
  fi
  if [ "$WANT_USB" -eq 1 ]; then
    # USB controllers (0c03) so the VM owns keyboard/mouse.
    while read -r b; do [ -n "$b" ] && bdfs+=("$b"); done < <(lspci -Dnn -d ::0c03 2>/dev/null | awk '{print $1}')
  fi
  printf '%s\n' "${bdfs[@]}" | awk 'NF' | sort -u
}

# Normalize each BDF to the full 0000:BB:DD.F form xen-pciback/xe expect.
normalize_bdf() { # <bdf>
  case "$1" in
    *:*:*.*) printf '%s\n' "$1" ;;   # already domain:bus:dev.func
    *:*.*)   printf '0000:%s\n' "$1" ;; # bus:dev.func → prepend domain
    *)       printf '%s\n' "$1" ;;
  esac
}

if [ -n "$PCI" ]; then
  IFS=',' read -r -a RAW <<< "$PCI"
else
  mapfile -t RAW < <(detect_pci)
fi
BDFS=()
for b in "${RAW[@]}"; do b="$(echo "$b" | tr -d '[:space:]')"; [ -n "$b" ] && BDFS+=("$(normalize_bdf "$b")"); done
[ "${#BDFS[@]}" -gt 0 ] || { echo "no passthrough PCI devices detected — pass --pci, or check --no-* flags" >&2; exit 3; }

# Build the two derived strings:
#   HIDE  — the xen-pciback hide list:  (0000:01:00.0)(0000:01:00.1)...
#   ASSIGN— the VM other-config:pci value:  0/0000:01:00.0,0/0000:01:00.1,...
HIDE=""; ASSIGN=""; i=0
for b in "${BDFS[@]}"; do
  HIDE="$HIDE($b)"
  [ -n "$ASSIGN" ] && ASSIGN="$ASSIGN,"
  ASSIGN="$ASSIGN$i/$b"
  i=$((i+1))
done

log "Primary Desktop VM : $VM"
log "passthrough devices: ${BDFS[*]}"
log "xen-pciback hide   : xen-pciback.hide=$HIDE"
log "VM pci assignment  : other-config:pci=$ASSIGN"

if [ "$APPLY" -eq 0 ]; then
  cat >&2 <<PLAN
[dry run] would, with --apply:
  1. /opt/xensource/libexec/xen-cmdline --set-dom0 "xen-pciback.hide=$HIDE"
     (hides the devices from dom0 at the next boot)
  2. xe vm-param-set uuid=<$VM> other-config:pci=$ASSIGN
     (assigns the hidden functions to the Primary Desktop VM)
  3. xe pool-param-set uuid=<pool> other-config:auto_poweron=true
     xe vm-param-set  uuid=<$VM> other-config:auto_poweron=true
     (the VM owns the console from boot)
Re-run with --apply, then REBOOT dom0 for the xen-pciback hide to take effect.
PLAN
  exit 0
fi

# ----- APPLY -----
have xe || { echo "xe not found — run this ON an XCP-ng dom0" >&2; exit 3; }

# Resolve the VM uuid (accept a name-label or a uuid).
VM_UUID="$VM"
if ! echo "$VM" | grep -Eq '^[0-9a-fA-F-]{36}$'; then
  VM_UUID="$(xe vm-list name-label="$VM" --minimal | tr -d '\r' | cut -d, -f1)"
fi
[ -n "$VM_UUID" ] || { echo "no VM matching '$VM'" >&2; exit 4; }
POOL_UUID="$(xe pool-list --minimal | tr -d '\r' | cut -d, -f1)"

log "1/3 hiding devices from dom0 (xen-pciback)"
if [ -x /opt/xensource/libexec/xen-cmdline ]; then
  /opt/xensource/libexec/xen-cmdline --set-dom0 "xen-pciback.hide=$HIDE"
else
  echo "passthrough: /opt/xensource/libexec/xen-cmdline missing — set xen-pciback.hide=$HIDE on the dom0 kernel cmdline by hand" >&2
fi

log "2/3 assigning the devices to $VM_UUID"
xe vm-param-set uuid="$VM_UUID" other-config:pci="$ASSIGN"

log "3/3 arming pool + VM auto-poweron"
[ -n "$POOL_UUID" ] && xe pool-param-set uuid="$POOL_UUID" other-config:auto_poweron=true
xe vm-param-set uuid="$VM_UUID" other-config:auto_poweron=true

log "DONE — configured. REBOOT the dom0 for the xen-pciback hide to take effect,"
log "then the Primary Desktop VM auto-starts owning the passed-through hardware."
log "NOTE: keep a small management VM (no passthrough) on the pool so the console"
log "      can be reclaimed for recovery if the desktop VM fails to start."
