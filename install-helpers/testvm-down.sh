#!/usr/bin/env bash
# testvm-down.sh — tear down the throwaway VDI test-endpoint VMs (TESTVM-1,
# design: docs/design/vdi-test-endpoints.md).
#
# For each name (default: testvm-lin testvm-win) on each farm dom0:
#   1. xe vm-shutdown --force   (falls back to vm-reset-powerstate --force)
#   2. xe vm-uninstall force=true          — destroys the VM AND its VDIs
#   3. destroys any leftover ${name}-root / ${name}-seed VDIs (orphan sweep)
#   4. removes /var/tmp/${name}-* staging files on the dom0
#
# Idempotent: names that don't exist are skipped quietly. This is the
# documented teardown path for the TESTVM bed — the VMs are throwaway by
# design; nothing else references them.
#
# Usage:
#   install-helpers/testvm-down.sh                 # both default names, all dom0s
#   install-helpers/testvm-down.sh testvm-lin      # just one name
#   TESTVM_DOM0S=172.20.0.9 install-helpers/testvm-down.sh
set -euo pipefail

DOM0S=(${TESTVM_DOM0S:-172.20.0.9 172.20.145.193})
SSH_KEY="${TESTVM_SSH_KEY:-$HOME/.ssh/mackes_mesh_ed25519}"
if [ $# -gt 0 ]; then NAMES=("$@"); else NAMES=(testvm-lin testvm-win); fi

SSH_OPTS=(-o BatchMode=yes -o ConnectTimeout=10 -o StrictHostKeyChecking=accept-new -i "$SSH_KEY")
d0() { ssh "${SSH_OPTS[@]}" "root@$1" "${@:2}"; }
log() { echo "==> testvm-down: $*"; }

for h in "${DOM0S[@]}"; do
  if ! d0 "$h" 'true' 2>/dev/null; then log "dom0 $h unreachable — skipping"; continue; fi
  for name in "${NAMES[@]}"; do
    case "$name" in testvm-*) ;; *) log "refusing non-testvm name '$name'"; continue;; esac
    VM=$(d0 "$h" "xe vm-list name-label=$name --minimal" || true)
    if [ -n "$VM" ]; then
      log "$h: destroying $name ($VM)"
      d0 "$h" "xe vm-shutdown uuid=$VM --force 2>/dev/null || xe vm-reset-powerstate uuid=$VM --force 2>/dev/null || true"
      d0 "$h" "xe vm-uninstall uuid=$VM force=true"
    else
      log "$h: no VM named $name"
    fi
    # orphan VDI sweep (covers a bringup that died between vdi-import and vbd-create)
    for suffix in root seed; do
      for vdi in $(d0 "$h" "xe vdi-list name-label=${name}-${suffix} --minimal" | tr , ' '); do
        [ -n "$vdi" ] || continue
        log "$h: destroying orphan VDI ${name}-${suffix} ($vdi)"
        d0 "$h" "xe vdi-destroy uuid=$vdi" || true
      done
    done
    d0 "$h" "rm -f /var/tmp/${name}-*.raw /var/tmp/${name}-*.iso" || true
  done
done
log "done"
