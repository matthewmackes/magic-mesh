#!/usr/bin/env bash
# drain-coordinator.sh — DRAIN-6 (the coordinator pattern, operator-locked
# 2026-06-24). The runnable half of the AI drain loop's per-tick mechanics, so the
# loop keeps N farm agents in flight and never idles a slot while buildable units
# remain. The /ship skill's "Coordinator pattern (DRAIN-6)" section drives it.
#
# Each tick the coordinator:
#   1. preflight  — guarantee dev-host disk headroom (disk-watchdog.sh) before any spawn
#   2. slots      — compute FREE heavy build slots per farm node (REAL topology +
#                   per-node caps), so it spreads instead of piling onto one node
#   3. next [N]   — list the next N open, UNBLOCKED worklist units to dispatch
#   4. plan [N]   — all of the above = the tick's dispatch plan
#
# Farm topology is DERIVED from the single tofu source (farm-inventory.sh) — never
# hardcoded, so it can't drift (this was a stale 3-node/"7 slots" table that missed
# xen-194). The live 4-dom0 reality (docs/BUILD-ENVIRONMENT.md §3):
#   .50  XEN-HOME-SERVICES / mcnf-build-50   3 vCPU  cap 2
#   .90  KVM-XCP1          / mcnf-build-51   3 vCPU  cap 2
#   .130 XEN-BIGBOY        / mcnf-build-52  10 vCPU  cap 3
#   .170 XEN-194           / mcnf-build-53   3 vCPU  cap 2   => 9 heavy slots total
#
# A node that is unreachable is reported down (0 free) and the tick continues — the
# coordinator never stalls on one node (park-don't-stall ethos, DRAIN-5).
set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
WORKLIST="${MCNF_WORKLIST:-$ROOT/docs/WORKLIST.md}"
KEY="${MCNF_FARM_KEY:-/root/.ssh/mackes_mesh_ed25519}"
SSH_USER="${MCNF_FARM_USER:-mm}"

# parallel arrays: node octet / dom0 label+build-VM / heavy-build cap — DERIVED from
# the single tofu source (farm-inventory.sh fleet: host|label|buildvm_ip|vcpus|cap).
NODES=(); NAMES=(); CAPS=()
if [ -x "$HERE/farm-inventory.sh" ]; then
  while IFS='|' read -r _hostip _label _vmip _vcpus _cap; do
    [ -n "$_vmip" ] || continue
    NODES+=("${_vmip##*.}"); NAMES+=("$_label/$_vmip"); CAPS+=("${_cap:-2}")
  done < <("$HERE/farm-inventory.sh" fleet)
fi
# Fallback if the tool is missing: the correct 4-dom0 reality (.50/.90/.130/.170).
if [ "${#NODES[@]}" -eq 0 ]; then
  NODES=(50 90 130 170)
  NAMES=("XEN-HOME-SERVICES/.50" "KVM-XCP1/.90" "XEN-BIGBOY/.130" "XEN-194/.170")
  CAPS=(2 2 3 2)
fi

farm_ssh() { timeout 14 ssh -i "$KEY" -o BatchMode=yes -o ConnectTimeout=8 \
  -o StrictHostKeyChecking=accept-new "$SSH_USER@172.20.0.$1" "$2" 2>/dev/null; }

cmd_preflight() {
  if [ -x "$HERE/disk-watchdog.sh" ]; then "$HERE/disk-watchdog.sh" "${1:-8}";
  else echo "drain-coordinator: disk-watchdog.sh missing — skipping preflight" >&2; fi
}

# Print one "node= cap= active= free=" line per node + a TOTAL_FREE= summary line.
cmd_slots() {
  local total=0 i node cap active free state
  for i in "${!NODES[@]}"; do
    node="${NODES[$i]}"; cap="${CAPS[$i]}"
    active="$(farm_ssh "$node" 'pgrep -c cargo')"
    if [ -z "$active" ]; then
      state="DOWN"; free=0
      printf '.%-4s %-28s cap=%s active=?  free=0  (%s)\n' "$node" "${NAMES[$i]}" "$cap" "$state"
      continue
    fi
    [ "$active" -gt "$cap" ] && active="$cap"   # don't report negative headroom
    free=$(( cap - active ))
    total=$(( total + free ))
    printf '.%-4s %-28s cap=%s active=%s free=%s\n' "$node" "${NAMES[$i]}" "$cap" "$active" "$free"
  done
  echo "TOTAL_FREE=$total"
}

# List up to N open, UNBLOCKED worklist unit ids (open = [ ] or [>]; not [!]/[~]).
# Honest: these are candidates — the coordinator still picks file-disjoint ones.
cmd_next() {
  local n="${1:-7}"
  [ -f "$WORKLIST" ] || { echo "drain-coordinator: no worklist at $WORKLIST" >&2; return 2; }
  grep -E '^- \[[ >]\] \*\*[A-Za-z0-9][A-Za-z0-9-]*[: ]' "$WORKLIST" \
    | sed -E 's/^- \[[ >]\] \*\*([A-Za-z0-9][A-Za-z0-9-]*).*/\1/' \
    | head -n "$n"
}

cmd_plan() {
  local n="${1:-7}"
  echo "### drain tick plan ($(date -u +%H:%M:%SZ))"
  echo "--- 1. preflight (disk headroom) ---"; cmd_preflight 8
  echo "--- 2. free farm slots ---"; cmd_slots
  echo "--- 3. next $n candidate units ---"; cmd_next "$n"
  echo "### rearm on each completion; park (park-blocker.sh) don't stall; no-flinch."
}

case "${1:-plan}" in
  preflight) shift; cmd_preflight "$@";;
  slots)     cmd_slots;;
  next)      shift; cmd_next "$@";;
  plan)      shift; cmd_plan "$@";;
  *) echo "usage: $0 {preflight|slots|next [N]|plan [N]}" >&2; exit 2;;
esac
