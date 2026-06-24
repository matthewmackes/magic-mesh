#!/bin/sh
# qnm-mount.sh — BOOT-XPA8-4: the robust QNM-Shared (LizardFS) mount loop, as a
# SHIPPED libexec script the qnm-shared.service unit just calls.
#
# WHY a separate script (not the inline ExecStart= heredoc loop): the loop body
# used to live inline in the unit that setup-qnm-shared.sh generates. That made
# every fix to the loop (BOOT-REC-2 retry, LH-JOIN-QNM-1 wedge-proofing,
# BOOT-XPA8-3 race-fix) reachable on an existing node ONLY by re-running
# setup-qnm-shared.sh OR sed-rewriting the unit in post_install — fragile, and
# easy to drift. Shipped here, the logic updates with every RPM upgrade and the
# unit's ExecStart is a stable one-liner that never needs sed-patching again.
#
# Usage (called by qnm-shared.service):
#   qnm-mount mount     # the boot-race-proof retry mount loop (ExecStart)
#   qnm-mount unmount   # lazy-detach a wedged/clean mount (ExecStop)
#
# Config (env, set by the unit; falls back to the on-disk defaults so a manual
# invocation still works):
#   QNM_PATH    mount point            (default /mnt/mesh-storage)
#   MASTER_IP   overlay master ip      (default: /etc/mackesd-qnm-master, else 10.42.0.1)
#
# POSIX /bin/sh ONLY — runs under dash. No bashisms (the old inline loop hit
# exactly this: /dev/tcp is unavailable under dash and spun the full timeout).
set -u

QNM_PATH="${QNM_PATH:-/mnt/mesh-storage}"
if [ -z "${MASTER_IP:-}" ]; then
  if [ -r /etc/mackesd-qnm-master ]; then
    MASTER_IP="$(cat /etc/mackesd-qnm-master 2>/dev/null)"
  fi
  MASTER_IP="${MASTER_IP:-10.42.0.1}"
fi

# LH-JOIN-QNM-1 — lazy-detach so a half-formed/stale FUSE mount in uninterruptible
# D-state (mfsmount daemon gone, kernel entry lingering) actually releases. Plain
# `umount -u` cannot detach a wedged mount; `fusermount -uz` + `umount -l` can.
qnm_detach() {
  fusermount -uz "$QNM_PATH" 2>/dev/null || true
  umount -l "$QNM_PATH" 2>/dev/null || true
  pkill -f "mfsmount $QNM_PATH" 2>/dev/null || true
}

# BOOT-REC-2 — a cold boot brings nebula + the LizardFS master up AFTER this unit
# is first scheduled, so RETRY the actual mount until it succeeds (the master
# becomes reachable a few seconds into boot). Bounded ~60s; on a genuinely-down
# master it exits non-zero so systemd's Restart=on-failure + RestartSec=30 + the
# mesh-health watchdog keep retrying.
# Every check that touches the mount is `timeout`-guarded (LH-JOIN-QNM-1) so a
# wedged mount in D-state can NEVER hang the loop — it always makes progress.
qnm_mount() {
  command -v mfsmount >/dev/null 2>&1 || {
    echo "qnm-mount: mfsmount missing (install lizardfs-client; F44: the F43 client RPM)" >&2
    exit 1
  }
  i=0
  while [ "$i" -lt 15 ]; do
    timeout 6 mountpoint -q "$QNM_PATH" && exit 0
    # BOOT-XPA8-3 — clear any stale/half-mount BEFORE each attempt so the loop
    # never races itself (concurrent mfsmounts colliding / a stale session
    # displacing each new one).
    qnm_detach
    sleep 1
    # A non-empty unmounted mountpoint makes mfsmount need -o nonempty AND can
    # mask stray writes; stash anything found so the mount lands on a clean dir.
    if [ -n "$(timeout 6 ls -A "$QNM_PATH" 2>/dev/null)" ]; then
      d="/var/lib/mde/qnm-stray-$(date +%s 2>/dev/null || echo bk)"
      mkdir -p "$d"
      mv "$QNM_PATH"/* "$QNM_PATH"/.[!.]* "$d"/ 2>/dev/null || true
    fi
    mfsmount "$QNM_PATH" -H "$MASTER_IP" -o allow_other,nonempty 2>/dev/null || true
    # BOOT-XPA8-3 — verify the mount actually HELD before declaring success
    # (a concurrent attempt could displace it a beat after mfsmount returns 0).
    sleep 3
    timeout 6 mountpoint -q "$QNM_PATH" && exit 0
    i=$((i + 1))
    sleep 2
  done
  exit 1
}

case "${1:-mount}" in
  mount)   qnm_mount ;;
  unmount) qnm_detach; exit 0 ;;
  *) echo "usage: qnm-mount {mount|unmount}" >&2; exit 2 ;;
esac
