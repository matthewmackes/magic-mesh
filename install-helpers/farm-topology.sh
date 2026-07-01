#!/usr/bin/env bash
# farm-topology.sh — THE SINGLE CANONICAL SOURCE OF TRUTH for the MCNF build farm.
#
# 4 dom0s, each hosting ONE Fedora build VM (user `mm`, key
# `mackes_mesh_ed25519`, shared sccache). Every farm-aware tool + doc + skill must
# READ this file (source it) or CITE it as the authority — never hardcode the node
# list anywhere else, or it drifts. It DID drift: the 4th dom0 (XEN-194 → build VM
# mcnf-build-53 / .170) sat IDLE for a whole session under a stale 3-node topology
# until the operator caught it (2026-07-01). This file + its `table`/`check`
# commands are the fix: one roster, verified live each run.
#
# Usage:
#   source install-helpers/farm-topology.sh   # → FARM_OCTETS/FARM_NAMES/FARM_CAPS/
#                                              #   FARM_DOM0_IPS/FARM_TOTAL_CAP arrays
#   ./farm-topology.sh table    # probe EVERY node → print the VERIFIED utilization
#                               # table; EXIT NON-ZERO if any canonical node is
#                               # unreachable (the never-fall-out-of-sync guard)
#   ./farm-topology.sh check    # like `table` but quiet on full success (loop tick)
#   ./farm-topology.sh octets   # print the 4 build-VM octets, space-separated
#
# Canonical roster (build-VM octet / dom0 / dom0 IP / vCPU / heavy-build cap):
#   .50   XEN-HOME-SERVICES  172.20.0.9      4c   cap 2
#   .90   KVM-XCP1           172.20.145.193  4c   cap 2
#   .130  XEN-BIGBOY         172.20.145.165  12c  cap 3   (BigBoy — the long-pole node)
#   .170  XEN-194            172.20.145.194  4c   cap 2   (the 4th dom0)
#   => 9 heavy build slots total (2 + 2 + 3 + 2)
#
# Build-VM names are legacy and do NOT equal the IP octet: mcnf-build-51 = .90,
# mcnf-build-52 = .130, mcnf-build-53 = .170 (per-dom0 lane, docs/BUILD-ENVIRONMENT.md §3).

# --- the canonical arrays (parallel; index 0..3) ---
FARM_OCTETS=(50 90 130 170)
FARM_NAMES=(
  "XEN-HOME-SERVICES/mcnf-build-50"
  "KVM-XCP1/mcnf-build-51"
  "XEN-BIGBOY/mcnf-build-52"
  "XEN-194/mcnf-build-53"
)
FARM_DOM0_IPS=(172.20.0.9 172.20.145.193 172.20.145.165 172.20.145.194)
FARM_CAPS=(2 2 3 2)
FARM_TOTAL_CAP=9   # 2 + 2 + 3 + 2 — keep in sync with FARM_CAPS

FARM_KEY="${MCNF_MESH_KEY:-/root/.ssh/mackes_mesh_ed25519}"
FARM_SSH_USER="${MCNF_BUILD_USER:-mm}"

# One-liner probe of a build VM (over its .0.<octet> address). Empty on unreachable.
_farm_probe() {
  timeout 15 ssh -i "$FARM_KEY" -o BatchMode=yes -o ConnectTimeout=8 \
    -o StrictHostKeyChecking=accept-new "$FARM_SSH_USER@172.20.0.$1" \
    'echo "$(cut -d" " -f1 /proc/loadavg)|$(pgrep -c cargo)|$(pgrep -c rustc)|$(df -h --output=avail /home|tail -1|tr -d " ")"' 2>/dev/null
}

# Print the VERIFIED farm utilization table. Returns non-zero if ANY canonical
# node is unreachable (a missing member must FAIL, not be silently dropped — the
# whole point of the never-drift guard). `check` mode is quiet on full success.
farm_table() {
  local mode="${1:-table}" i octet cap name probe load cargo rustc free
  local total_free=0 down=0 active_sum=0
  local rows=""
  for i in "${!FARM_OCTETS[@]}"; do
    octet="${FARM_OCTETS[$i]}"; cap="${FARM_CAPS[$i]}"; name="${FARM_NAMES[$i]}"
    probe="$(_farm_probe "$octet")"
    if [ -z "$probe" ]; then
      down=$(( down + 1 ))
      rows+=$(printf '| .%-4s | %-26s | %-3s | %-6s | %-4s | %-5s | %s |\n' \
        "$octet" "$name" "$cap" "DOWN" "?" "?" "⚠️ UNREACHABLE")
      rows+=$'\n'
      continue
    fi
    IFS='|' read -r load cargo rustc free <<<"$probe"
    [ "$cargo" -gt "$cap" ] 2>/dev/null && cargo="$cap"
    local nfree=$(( cap - cargo ))
    total_free=$(( total_free + nfree )); active_sum=$(( active_sum + cargo ))
    rows+=$(printf '| .%-4s | %-26s | %-3s | %-6s | %-4s | %-5s | load %s |\n' \
      "$octet" "$name" "$cap" "$cargo" "$nfree" "$free" "$load")
    rows+=$'\n'
  done

  if [ "$mode" = "check" ] && [ "$down" -eq 0 ]; then
    echo "farm: 4/4 dom0s up; ${active_sum}/${FARM_TOTAL_CAP} heavy slots active, ${total_free} free."
  else
    echo "### Xen Host Utilization — Farm Wide (verified $(date -u +%H:%M:%SZ))"
    echo "| Node | dom0 / build VM | cap | active | free | /home | note |"
    echo "|---|---|---|---|---|---|---|"
    printf '%s' "$rows"
    echo "**${active_sum}/${FARM_TOTAL_CAP} heavy slots active · ${total_free} free · $(( ${#FARM_OCTETS[@]} - down ))/${#FARM_OCTETS[@]} nodes up**"
  fi

  if [ "$down" -gt 0 ]; then
    echo "farm-topology: $down canonical node(s) UNREACHABLE — roster out of sync or a node is down." >&2
    return 1
  fi
  return 0
}

# Run the CLI only when executed directly (not when sourced for the arrays).
if [ "${BASH_SOURCE[0]}" = "${0}" ]; then
  case "${1:-table}" in
    table) farm_table table;;
    check) farm_table check;;
    octets) echo "${FARM_OCTETS[*]}";;
    *) echo "usage: $0 {table|check|octets}" >&2; exit 2;;
  esac
fi
