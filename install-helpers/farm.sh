#!/usr/bin/env bash
# farm.sh — MCNF Build-Farm Automation Manager.
#
# ONE entry point for the whole build farm: see its state, bring it fully online,
# (re)provision build VMs, install the toolchain, key the hypervisors, diagnose a
# stuck VM, and drive builds. It orchestrates the single-purpose helpers in this
# directory over a fleet inventory so farm ops are repeatable, not ad-hoc:
#
#   setup-xcp-build-vm.sh      create + boot a build VM (qcow2 → VDI + cloud-init)
#   setup-build-vm-toolchain.sh   install the Rust build toolchain on a build VM
#   xcp-authorize-farm-key.sh  install the farm SSH key on a dom0 (passwordless xe)
#   xcp-build.sh               rsync the tree to a build VM + run cargo there
#
# Architecture + the recovery playbook: docs/farm.md.
#
# Fleet (override via env or ~/.config/mcnf-farm.conf — `dom0_ip|label|buildvm_ip`):
#   172.20.145.192  the dev/orchestration host (this box; local builds + podman)
#   172.20.0.9      XEN-HOME-SERVICES  → build VM 172.20.0.50
#   172.20.145.193  KVM-XCP1           → build VM 172.20.0.51
#
# Usage:
#   farm.sh status                  fleet state (dom0 reachable · VMs · build VM up + toolchained)
#   farm.sh up                      bring the WHOLE farm online (key + provision + toolchain; idempotent)
#   farm.sh key       <dom0>        install the farm key on a dom0 (needs XCP_PASS the first time)
#   farm.sh provision <dom0> [ip]   create+boot a fresh build VM on <dom0> + toolchain (needs XCP_PASS)
#   farm.sh toolchain <vm-ip>       (re)install the Rust build toolchain on a build VM
#   farm.sh doctor    <dom0> <vm>   diagnose a stuck VM (power · network/ARP · console · disk)
#   farm.sh build     <cargo args>  rsync + build on the primary build VM (xcp-build.sh)
#   farm.sh ssh       <dom0|vm-ip>  interactive shell (key auth)
#
# Auth: management is SSH-key (mackes_mesh_ed25519, key-on-dom0 via `farm.sh key`).
# Provisioning the FIRST time needs the dom0 root password in $XCP_PASS (sshpass).
set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
KEY="${MCNF_FARM_KEY:-$HOME/.ssh/mackes_mesh_ed25519}"
CONF="${MCNF_FARM_CONF:-$HOME/.config/mcnf-farm.conf}"

# Default fleet; a conf file (one `dom0_ip|label|buildvm_ip` per line) overrides.
FLEET_DEFAULT=$'172.20.0.9|XEN-HOME-SERVICES|172.20.0.50\n172.20.145.193|KVM-XCP1|172.20.0.51\n172.20.145.165|XEN-BIGBOY|172.20.0.52'
fleet() { [ -f "$CONF" ] && grep -vE '^\s*(#|$)' "$CONF" || printf '%s\n' "$FLEET_DEFAULT"; }

SSHK=(ssh -i "$KEY" -o StrictHostKeyChecking=accept-new -o BatchMode=yes -o ConnectTimeout=12)
log()  { echo "==> farm: $*"; }
warn() { echo "==> farm: $*" >&2; }

# reachable <ip> <port> — TCP probe.
reachable() { timeout 4 bash -c "cat </dev/null >/dev/tcp/$1/${2:-22}" 2>/dev/null; }
# toolchained <vm-ip> — does the build VM have rustc + a C++ compiler?
# NOTE: -n (stdin from /dev/null) is REQUIRED — without it ssh slurps the rest of
# the `while read … < <(fleet)` loop's stdin and only the first host is processed.
toolchained() { "${SSHK[@]}" -n "mm@$1" '. "$HOME/.cargo/env" 2>/dev/null; command -v rustc >/dev/null && command -v g++ >/dev/null' 2>/dev/null; }

cmd_status() {
  log "fleet status (key=$KEY)"
  printf '  %-16s %-20s %-8s %-16s %-7s %s\n' HOST LABEL DOM0 'BUILD-VM' 'VM:22' TOOLCHAIN
  while IFS='|' read -r ip label vmip; do
    [ -n "$ip" ] || continue
    local d0="down" vm="down" tc="-"
    reachable "$ip" 22 && d0="up"
    if reachable "$vmip" 22; then vm="up"; toolchained "$vmip" && tc="ready" || tc="bare"; fi
    printf '  %-16s %-20s %-8s %-16s %-7s %s\n' "$ip" "$label" "$d0" "$vmip" "$vm" "$tc"
  done < <(fleet)
  echo "  dev host (local): $(command -v cargo >/dev/null && echo 'cargo ok' || echo 'no cargo'); podman $(command -v podman >/dev/null && echo present || echo absent)"
}

cmd_key()       { XCP_PW="${XCP_PASS:-}" "$HERE/xcp-authorize-farm-key.sh" --host "$1" --key "$KEY.pub"; }
cmd_toolchain() { "$HERE/setup-build-vm-toolchain.sh" --host "$1" --key "$KEY"; }

cmd_provision() {
  local dom0="$1" vmip="${2:-}"
  [ -n "$vmip" ] || vmip="$(fleet | awk -F'|' -v h="$dom0" '$1==h{print $3}')"
  [ -n "$vmip" ] || { warn "no build-VM IP for $dom0 (pass one)"; return 1; }
  [ -n "${XCP_PASS:-}" ] || { warn "provisioning needs the dom0 root pw in XCP_PASS"; return 1; }
  log "provision build VM on $dom0 @ $vmip"
  SSHPASS="$XCP_PASS" "$HERE/setup-xcp-build-vm.sh" --xcp-host "$dom0" --xcp-pass "$XCP_PASS" \
    --ip "$vmip/16" --qcow2 "${MCNF_FARM_QCOW2:-/var/tmp/fedora-cloud.qcow2}" --pubkey "$KEY.pub" || return 1
  log "install toolchain on $vmip"
  cmd_toolchain "$vmip"
}

cmd_doctor() {
  local dom0="$1" vm="$2"
  log "doctor $vm on $dom0 (see docs/farm.md 'Recovery')"
  "${SSHK[@]}" "root@$dom0" "
    U=\$(xe vm-list name-label=$vm --minimal); echo \"uuid=\$U\"
    echo \"power: \$(xe vm-param-get uuid=\$U param-name=power-state)\"
    D=\$(xe vm-param-get uuid=\$U param-name=dom-id 2>/dev/null); echo \"dom-id=\$D\"
    M=\$(xe vif-list vm-uuid=\$U params=MAC --minimal | head -c17)
    echo \"vif MAC=\$M ; ARP: \$(ip neigh 2>/dev/null | grep -i \"\$M\" || echo 'no traffic (not networking)')\"
    echo '--- console (8s) ---'; ( sleep 6 ) | timeout 8 xl console \"\$D\" 2>&1 | tail -20
  "
}

cmd_build() { "$HERE/xcp-build.sh" cargo "$@"; }
cmd_ssh()   { local t="$1"; "${SSHK[@]}" -tt "$( [[ $t == 172.20.0.5* ]] && echo mm || echo root )@$t"; }

cmd_up() {
  log "bringing the full farm online"
  while IFS='|' read -r ip label vmip; do
    [ -n "$ip" ] || continue
    log "--- $label ($ip) → $vmip ---"
    reachable "$ip" 22 || { warn "$ip dom0 unreachable — skipping"; continue; }
    toolchained "$ip" >/dev/null 2>&1 || true
    if reachable "$vmip" 22 && toolchained "$vmip"; then
      log "$vmip already up + toolchained ✓"; continue
    fi
    [ -n "${XCP_PASS:-}" ] && cmd_key "$ip" 2>/dev/null || true
    if reachable "$vmip" 22; then
      log "$vmip up but bare → toolchain only"; cmd_toolchain "$vmip"
    else
      log "$vmip absent/down → provision"; cmd_provision "$ip" "$vmip"
    fi
  done < <(fleet)
  echo; cmd_status
}

case "${1:-status}" in
  status)    cmd_status ;;
  up)        cmd_up ;;
  key)       shift; cmd_key "$@" ;;
  provision) shift; cmd_provision "$@" ;;
  toolchain) shift; cmd_toolchain "$@" ;;
  doctor)    shift; cmd_doctor "$@" ;;
  build)     shift; cmd_build "$@" ;;
  ssh)       shift; cmd_ssh "$@" ;;
  -h|--help|help) sed -n '2,40p' "$0" | sed 's/^# \{0,1\}//' ;;
  *) warn "unknown subcommand: $1 (try: status up key provision toolchain doctor build ssh)"; exit 1 ;;
esac
