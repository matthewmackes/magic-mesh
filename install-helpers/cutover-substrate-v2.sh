#!/bin/bash
# cutover-substrate-v2.sh — SUBSTRATE-14: the etcd+Syncthing cutover entry point.
#
# Stands up the SUBSTRATE-V2 substrate on ONE node by composing setup-etcd
# (coordination) + setup-syncthing (files). This is the operator-driven cutover
# step — run it FIRST on the VM bed to rehearse (a reboot/disconnect drill), then
# on each fleet node at the 11.0 roll. It is deliberately NOT auto-wired into
# enroll: writing the etcd endpoints file flips this node's coordination from the
# LizardFS QNM-Shared fs path onto etcd (the SUBSTRATE-1..10 bridges go etcd-only
# once /etc/mackesd/etcd-endpoints exists), so it must be a conscious, rehearsed
# action — not a side effect of a routine re-enroll.
#
# Roles (pick one):
#   --init                bootstrap the FIRST anchor (founding etcd member)
#   --join <anchor-ip>    additional server/lighthouse: join the etcd cluster
#   --client-only         workstation: etcd client + Syncthing peer (no member)
#
# Options:
#   --listen <ip>     this node's OVERLAY ip (default: auto-detect nebula)
#   --anchors <csv>   anchor overlay IPs (client-only/join endpoint list)
#   --folder <dir>    Syncthing shared folder (default /mnt/mesh-storage)
#
# Rollback: this is additive until SUBSTRATE-6 removes LizardFS — both planes can
# coexist during the rehearsal. To roll a node back, `rm /etc/mackesd/etcd-endpoints`
# (the bridges fall back to the LizardFS fs path) + `systemctl disable --now
# etcd.service syncthing.service`. The one-release rollback RPM is the prior NEVRA
# (still carries setup-qnm-shared + the LizardFS Requires until 6 lands).
set -euo pipefail

MODE=""; JOIN_ANCHOR=""; LISTEN=""; ANCHORS=""; FOLDER=/mnt/mesh-storage
NO_FLIP=0; NO_FILES=0
HELPERS="$(dirname "$0")"
# Resolve a sibling helper whether INSTALLED (RPM strips the .sh:
# /usr/libexec/mackesd/setup-etcd) or run from the SOURCE tree (setup-etcd.sh).
helper() { if [ -x "$HELPERS/$1" ]; then echo "$HELPERS/$1"; else echo "$HELPERS/$1.sh"; fi; }

while [ $# -gt 0 ]; do case "$1" in
  --init) MODE="init"; shift;;
  --join) MODE="join"; JOIN_ANCHOR="$2"; shift 2;;
  --client-only) MODE="client"; shift;;
  --listen) LISTEN="$2"; shift 2;;
  --anchors) ANCHORS="$2"; shift 2;;
  --folder) FOLDER="$2"; shift 2;;
  # Fleet-safe orchestration (an EXISTING LizardFS mesh, not greenfield):
  --no-flip)  NO_FLIP=1; shift;;   # stand up etcd but do NOT restart mackesd
  --no-files) NO_FILES=1; shift;;  # coordination only — skip Syncthing (Phase A)
  *) echo "unknown arg: $1" >&2; exit 1;;
esac; done
[ -z "$MODE" ] && { sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'; exit 1; }
log() { echo "==> cutover: $*"; }

# 1. Coordination plane (etcd) — leader/directory/health.
ETCD_ARGS=()
case "$MODE" in
  init)   ETCD_ARGS=(--init);;
  join)   ETCD_ARGS=(--join "$JOIN_ANCHOR");;
  client) ETCD_ARGS=(--client-only);;
esac
[ -n "$LISTEN" ]  && ETCD_ARGS+=(--listen "$LISTEN")
[ -n "$ANCHORS" ] && ETCD_ARGS+=(--anchors "$ANCHORS")
log "etcd: setup-etcd ${ETCD_ARGS[*]}"
"$(helper setup-etcd)" "${ETCD_ARGS[@]}"

# 2. File plane (Syncthing) — replicates /mnt/mesh-storage full-mesh (no FUSE).
#    Phase A (--no-files) skips this: on an EXISTING mesh /mnt/mesh-storage is a
#    live LizardFS FUSE mount, so running Syncthing on it = double-replication.
#    The file swap is Phase B (LizardFS unmount → plain dir → Syncthing), SUBSTRATE-6.
if [ "$NO_FILES" -eq 0 ]; then
  SYNC_ARGS=(--folder "$FOLDER")
  [ -n "$LISTEN" ]  && SYNC_ARGS+=(--listen "$LISTEN")
  [ -n "$ANCHORS" ] && SYNC_ARGS+=(--anchors "$ANCHORS")
  log "syncthing: setup-syncthing ${SYNC_ARGS[*]}"
  "$(helper setup-syncthing)" "${SYNC_ARGS[@]}"
else
  log "syncthing: SKIPPED (--no-files; files stay on LizardFS until Phase B)"
fi

# 3. mackesd now coordinates via etcd (the 20-etcd.conf drop-in setup-etcd wrote
#    orders mackesd After=etcd). The flip happens on the next mackesd START
#    (run_serve's startup probe reads /etc/mackesd/etcd-endpoints). --no-flip
#    leaves the running mackesd on its current (LizardFS) plane so the fleet can
#    be STAGED (etcd up everywhere) and then flipped together in one fast pass —
#    a node-by-node flip would split the directory (etcd nodes vs fs nodes).
if [ "$NO_FLIP" -eq 0 ]; then
  log "restarting mackesd onto the etcd substrate"
  systemctl restart mackesd.service 2>/dev/null || true
else
  log "mackesd flip DEFERRED (--no-flip; etcd staged, mackesd still on LizardFS)"
  log "  flip the whole fleet together with: systemctl restart mackesd"
fi

log "done — node is on etcd (coordination) + Syncthing (files). Verify:"
echo "    etcdctl --endpoints=http://${LISTEN:-<overlay-ip>}:2379 endpoint health"
echo "    etcdctl --endpoints=... get --prefix /mesh/peers/   # the directory"
echo "    systemctl status syncthing.service                  # the file plane"
echo "  Rehearse the reboot/disconnect drill on the VM bed BEFORE the fleet roll."
