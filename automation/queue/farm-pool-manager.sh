#!/usr/bin/env bash
# farm-pool-manager.sh — FARM-AUTO-3 (control host): the pool half. Reports
# queue/agents/results, ensures the builder agent runs on every node, and sizes
# the pool to the backlog (notes when more nodes — or a `tofu apply` of extra VMs
# on XEN-BIGBOY — would help).
#
# Usage:  farm-pool-manager.sh status | ensure-agents | scale
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"; . "$HERE/etcd-lib.sh"
KEY="${MCNF_FARM_KEY:-$HOME/.ssh/mackes_mesh_ed25519}"
REPO="$(cd "$HERE/../.." && pwd)"
default_nodes() {
  # shellcheck source=../../install-helpers/farm-topology.sh
  . "$REPO/install-helpers/farm-topology.sh"
  local i
  for i in "${!FARM_OCTETS[@]}"; do
    printf '%s 172.20.0.%s\n' "${FARM_CAPS[$i]}" "${FARM_OCTETS[$i]}"
  done | sort -rn | awk '{print $2}' | paste -sd' ' -
}
NODES="${MCNF_BUILD_NODES:-$(default_nodes)}"
SSH=(ssh -i "$KEY" -o StrictHostKeyChecking=accept-new -o BatchMode=yes -o ConnectTimeout=12)

depth() { etcd_range_keys "/farm/queue/" | grep -c . ; }
locks() { etcd_range_keys "/farm/lock/" | grep -c . ; }
results(){ etcd_range_keys "/farm/result/" | grep -c . ; }

cmd_status() {
  echo "queue depth : $(depth)   in-flight(locked): $(locks)   results: $(results)"
  echo "--- per-node agent ---"
  for n in $NODES; do
    local up="down" ag="-"
    timeout 4 bash -c "cat </dev/null >/dev/tcp/$n/22" 2>/dev/null && {
      up="up"; ag=$("${SSH[@]}" -n "mm@$n" 'if systemctl is-active --quiet mcnf-farm-agent 2>/dev/null; then echo active; elif pgrep -f farm-agent.sh >/dev/null; then echo running; else echo stopped; fi' 2>/dev/null)
    }
    printf '  %-15s ssh=%-5s agent=%s\n' "$n" "$up" "$ag"
  done
  echo "--- results ---"
  for k in $(etcd_range_keys "/farm/result/"); do printf '  %-30s %s\n' "${k#/farm/result/}" "$(etcd_get "$k")"; done
}

# Install + start the agent service on each node (idempotent).
cmd_ensure_agents() {
  # Source of truth for the unit is THIS control host's repo ($REPO — the tree the
  # manager itself runs from, the same current source xcp-build.sh's do_sync and
  # farm-enqueue rsync to the nodes), NOT the node's ~/magic-mesh copy. That copy is
  # only current if farm-enqueue happened to run first (a temporal coupling), and the
  # build VMs additionally carry a stale Forgejo-mirror clone at ~/magic-mesh whose
  # origin/master sits at an old commit. Reading the unit from there can install a
  # STALE unit (or silently no-op on a missing clone). So ship the CURRENT unit
  # ourselves, then install from that fresh copy.
  local unit="$REPO/packaging/systemd/mcnf-farm-agent.service"
  [ -f "$unit" ] || { echo "unit not found: $unit" >&2; exit 1; }
  for n in $NODES; do
    timeout 4 bash -c "cat </dev/null >/dev/tcp/$n/22" 2>/dev/null || { echo "skip $n (down)"; continue; }
    echo "==> ensure agent on $n"
    # rsync the current unit to the node (same current-source pattern as
    # xcp-build.sh do_sync / farm-enqueue's tree rsync), then install from it.
    rsync -az -e "${SSH[*]}" "$unit" "mm@$n:/tmp/mcnf-farm-agent.service" \
      && "${SSH[@]}" "mm@$n" 'sudo cp /tmp/mcnf-farm-agent.service /etc/systemd/system/mcnf-farm-agent.service &&
      sudo systemctl daemon-reload && sudo systemctl enable --now mcnf-farm-agent 2>&1 | tail -1 &&
      echo "  $(hostname): $(systemctl is-active mcnf-farm-agent)"' 2>&1
  done
}

cmd_scale() {
  local d; d=$(depth); local n; n=$(echo "$NODES" | wc -w)
  echo "backlog=$d  nodes=$n"
  if [ "$d" -gt "$n" ]; then
    echo "backlog > nodes — XEN-BIGBOY (12c/32G) can host more build VMs; add nodes in"
    echo "infra/tofu (per-host VM count) + re-apply, then ensure-agents. Until then the"
    echo "$n agents drain the queue serially-per-node."
  else
    echo "pool sufficient for the current backlog."
  fi
}

case "${1:-status}" in
  status) cmd_status ;;
  ensure-agents) cmd_ensure_agents ;;
  scale) cmd_scale ;;
  *) echo "usage: farm-pool-manager.sh status|ensure-agents|scale" >&2; exit 1 ;;
esac
