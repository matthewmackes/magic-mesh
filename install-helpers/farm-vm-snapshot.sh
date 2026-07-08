#!/usr/bin/env bash
# farm-vm-snapshot.sh — FARM-AUTOSCALE inter-job VM reset (FA-2 / design L3).
#
# Each toolchained build VM carries a `clean` snapshot — the post-toolchain,
# sccache-primed golden baseline with NO job state. Between build jobs the
# autoscaler does a fast `xe snapshot-revert` to that snapshot instead of a full
# re-clone (clean state without the clone/boot cost). This script is the two verbs:
#
#   snapshot  <vm>  create/REFRESH the VM's `clean` snapshot (replaces any prior
#                   one, so re-toolchaining re-bases the baseline). Idempotent.
#   reset     <vm>  revert the VM to its LATEST `clean` snapshot (inter-job reset).
#   has-clean <vm>  exit 0 iff the VM HAS a `clean` snapshot (the build-ready probe
#                   the reconciler gates toolchain-on-first-provision on); quiet —
#                   the EXIT CODE is the answer. Absent VM / no snapshot → nonzero.
#
# <vm> is a name-label OR a uuid. The dom0 hosting it is reached over SSH with the
# farm key (passwordless `xe`, matching farm.sh); pass --xcp-host or let it default
# to the dom0 in $MCNF_XCP_HOST. Safe + clear when the VM/snapshot is absent.
#
# Usage:
#   farm-vm-snapshot.sh snapshot mcnf-build-52 --xcp-host 172.20.145.165
#   farm-vm-snapshot.sh reset    mcnf-build-52 --xcp-host 172.20.145.165
#   MCNF_XCP_HOST=172.20.0.9 farm-vm-snapshot.sh reset mcnf-build-home-services
#
# Env: MCNF_FARM_KEY (default ~/.ssh/mackes_mesh_ed25519), MCNF_XCP_HOST,
#      MCNF_XCP_USER (default root).
set -euo pipefail

SNAP_NAME="clean" # the post-toolchain baseline snapshot name-label
KEY="${MCNF_FARM_KEY:-$HOME/.ssh/mackes_mesh_ed25519}"
XCP_HOST="${MCNF_XCP_HOST:-}"
XCP_USER="${MCNF_XCP_USER:-root}"

usage() { sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'; }

ACTION=""; VM=""
while [ $# -gt 0 ]; do case "$1" in
  snapshot | reset | has-clean | start) ACTION="$1"; shift;;
  --xcp-host) XCP_HOST="$2"; shift 2;;
  --xcp-user) XCP_USER="$2"; shift 2;;
  --key) KEY="$2"; shift 2;;
  -h | --help | help) usage; exit 0;;
  -*) echo "farm-vm-snapshot: unknown flag: $1" >&2; usage; exit 2;;
  *)
    if [ -z "$VM" ]; then VM="$1"; else echo "farm-vm-snapshot: unexpected arg: $1" >&2; exit 2; fi
    shift;;
esac; done

[ -n "$ACTION" ] || { echo "farm-vm-snapshot: need a verb (snapshot|reset|has-clean|start)" >&2; usage; exit 2; }
[ -n "$VM" ]     || { echo "farm-vm-snapshot: need a <vm-name-or-uuid>" >&2; usage; exit 2; }
[ -n "$XCP_HOST" ] || { echo "farm-vm-snapshot: no dom0 — pass --xcp-host or set MCNF_XCP_HOST" >&2; exit 2; }
[ -s "$KEY" ]      || { echo "farm-vm-snapshot: farm key not found: $KEY" >&2; exit 2; }

log()  { echo "==> farm-vm-snapshot: $*"; }
warn() { echo "==> farm-vm-snapshot: $*" >&2; }

# xe() — run `xe` on the dom0 over SSH (key auth). %q-quote each arg so a value
# with spaces (e.g. a name-label) survives ssh's remote re-split, matching the
# pattern in setup-xcp-build-vm.sh. -n keeps ssh off the loop's stdin.
SSHOPTS=(-i "$KEY" -o StrictHostKeyChecking=accept-new -o BatchMode=yes -o ConnectTimeout=15)
xe() {
  local _c="xe" _a
  for _a in "$@"; do _c="$_c $(printf '%q' "$_a")"; done
  ssh -n "${SSHOPTS[@]}" "$XCP_USER@$XCP_HOST" "$_c"
}

# clean1 <field> — strip the CR + any whitespace xe --minimal can leave.
clean1() { tr -d '\r' | tr -d '[:space:]'; }

# resolve_vm <name-or-uuid> → echo the VM uuid (empty if absent). A real VM, never
# a snapshot (is-control-domain=false is-a-snapshot=false). Try uuid= first, then
# name-label=, so either form works.
resolve_vm() {
  local q="$1" u
  u="$(xe vm-list uuid="$q" is-a-snapshot=false params=uuid --minimal 2>/dev/null | clean1 || true)"
  [ -n "$u" ] && { echo "$u"; return 0; }
  xe vm-list name-label="$q" is-a-snapshot=false params=uuid --minimal 2>/dev/null | clean1 || true
}

# clean_snapshots <vm-uuid> → newest-first list of `clean` snapshot uuids (one/line).
# Sort by snapshot_time desc so reset always picks the latest, and refresh removes
# the stale ones.
clean_snapshots() {
  # `xe … params=…` prints blank-line-SEPARATED records of `key ( RO): value`
  # lines, and the field ORDER within a record is NOT guaranteed (xe emits them
  # alphabetically, so `snapshot-time` lands BEFORE `uuid`). Parse per-record —
  # collect uuid + snapshot-time wherever they appear, emit `time<TAB>uuid` only
  # at the record boundary — so the pairing is order-independent (mirrors the Rust
  # parse_param_records in crates/mesh/mackes-xcp). snapshot-time is fixed-width
  # ISO (YYYYMMDDThh:mm:ssZ), so `sort -r` on it is newest-first.
  xe snapshot-list snapshot-of="$1" name-label="$SNAP_NAME" \
    params=uuid,snapshot-time 2>/dev/null \
    | tr -d '\r' \
    | awk '
        function flush() { if (u != "" && t != "") print t "\t" u; u=""; t="" }
        # split "key ( RO): value" on the FIRST colon; strip the ( RO) parenthetical.
        {
          line=$0; sub(/[[:space:]]+$/, "", line)
          if (line == "") { flush(); next }
          ci=index(line, ":"); if (ci==0) next
          key=substr(line, 1, ci-1); val=substr(line, ci+1)
          sub(/[[:space:]]*\(.*$/, "", key); gsub(/^[[:space:]]+|[[:space:]]+$/, "", key)
          gsub(/^[[:space:]]+|[[:space:]]+$/, "", val)
          if (key == "uuid")          u=val
          else if (key == "snapshot-time") t=val
        }
        END { flush() }
      ' \
    | sort -r \
    | cut -f2 \
    | sed '/^$/d'
}

VM_UUID="$(resolve_vm "$VM")"
if [ -z "$VM_UUID" ]; then
  # has-clean is a QUIET probe (exit code is the answer): an absent VM is simply
  # "not build-ready" → nonzero, no warn. The mutating verbs report loudly.
  [ "$ACTION" = "has-clean" ] && exit 1
  warn "no VM named/uuid '$VM' on $XCP_HOST — nothing to do"; exit 1
fi
[ "$ACTION" = "has-clean" ] || log "VM '$VM' → $VM_UUID on $XCP_HOST"

case "$ACTION" in
  snapshot)
    # REFRESH: snapshot first, then drop the older `clean` snapshots so exactly one
    # baseline remains. Snapshot-before-delete means a failed snapshot leaves the
    # previous baseline intact (never strand the VM without a clean point).
    log "creating fresh '$SNAP_NAME' snapshot"
    NEW="$(xe vm-snapshot uuid="$VM_UUID" new-name-label="$SNAP_NAME" | clean1)"
    [ -n "$NEW" ] || { warn "vm-snapshot returned no uuid — aborting"; exit 1; }
    log "new baseline snapshot $NEW"
    pruned=0
    while IFS= read -r snap; do
      if [ -z "$snap" ] || [ "$snap" = "$NEW" ]; then continue; fi
      # snapshot-uninstall removes the snapshot AND its VDIs (free the SR).
      if xe snapshot-uninstall uuid="$snap" force=true >/dev/null 2>&1; then
        pruned=$((pruned + 1))
      else
        warn "could not prune old snapshot $snap (left in place)"
      fi
    done < <(clean_snapshots "$VM_UUID")
    log "done — baseline=$NEW, pruned $pruned stale snapshot(s)"
    ;;
  reset)
    LATEST="$(clean_snapshots "$VM_UUID" | head -1)"
    [ -n "$LATEST" ] || { warn "no '$SNAP_NAME' snapshot on '$VM' — run 'snapshot' first"; exit 1; }
    log "reverting to latest '$SNAP_NAME' snapshot $LATEST"
    # The baseline is a DISK-ONLY snapshot (vm-snapshot above, not a memory checkpoint),
    # so snapshot-revert returns the disks to the clean baseline but leaves the VM
    # HALTED. Boot it back so 'reset' yields a VM that is clean AND running
    # (build-ready) — a kept build VM left powered off is the bug this fixes.
    xe snapshot-revert snapshot-uuid="$LATEST"
    if xe vm-start uuid="$VM_UUID" >/dev/null 2>&1; then
      log "reverted + started — '$VM' is clean and running"
    else
      log "reverted — '$VM' clean (was already running, or vm-start declined)"
    fi
    ;;
  start)
    # Ensure the VM is running. Clean-baseline VMs are Halted after a disk-only
    # snapshot-revert; the reconciler calls this to bring a kept VM back up
    # build-ready WITHOUT reverting it (it is already clean). Idempotent.
    if [ "$(xe vm-list uuid="$VM_UUID" params=power-state --minimal 2>/dev/null)" = "running" ]; then
      log "'$VM' already running — nothing to do"
    elif xe vm-start uuid="$VM_UUID" >/dev/null 2>&1; then
      log "'$VM' started"
    else
      warn "vm-start of '$VM' failed"
    fi
    ;;
  has-clean)
    # Quiet build-ready probe: exit 0 iff a `clean` snapshot exists (reuses the same
    # resolve_vm + clean_snapshots the reset path trusts, so the reconciler's
    # readiness check can NEVER drift from what reset actually reverts to).
    [ -n "$(clean_snapshots "$VM_UUID" | head -1)" ] || exit 1
    ;;
esac
