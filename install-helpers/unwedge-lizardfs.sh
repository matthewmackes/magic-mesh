#!/usr/bin/env bash
# unwedge-lizardfs.sh — rescue a node whose LizardFS QNM-Shared FUSE mounts have
# wedged (the master went away), pinning the box at a runaway load average with
# uninterruptible D-state processes that even `kill -9` can't touch.
#
# Found live 2026-06-20: destroying the old DO lighthouses removed the LizardFS
# master, and EVERY old-mesh node's QNM-Shared mounts (/mnt/mesh-storage,
# /root/QNM-Shared, and the XDG bind-mounts Documents/Downloads/Music/…) wedged
# at once — `mfs#<master>:9421` mounts whose master is gone. `fusermount -uz`
# alone does NOT release the blocked I/O; the decisive move is to ABORT the FUSE
# connection via /sys/fs/fuse/connections/<id>/abort, which forces the D-state
# readers to return EIO and the load to collapse.
#
# This is the recovery half of the SUBSTRATE-V2 motivation: with etcd+Syncthing
# there is no FUSE master to disappear, so this failure mode goes away entirely.
#
# Usage:  sudo ./unwedge-lizardfs.sh            # abort + unmount + mask qnm-shared
#         sudo ./unwedge-lizardfs.sh --mask-mackesd   # also mask mackesd (stop respawn)
#         sudo ./unwedge-lizardfs.sh --no-mask        # abort+unmount only, leave units
set -uo pipefail
[ "$(id -u)" -eq 0 ] || { echo "run as root (sudo)"; exit 1; }

MASK_QNM=1; MASK_MACKESD=0
for a in "$@"; do case "$a" in
  --mask-mackesd) MASK_MACKESD=1;;
  --no-mask) MASK_QNM=0;;
  *) echo "unknown arg: $a" >&2; exit 1;;
esac; done

echo "==> load before: $(cut -d' ' -f1-3 /proc/loadavg)"

# 1. Abort every wedged FUSE connection — this is what actually releases the
#    uninterruptible D-state I/O (fusermount -uz on its own does not).
n=0
for c in /sys/fs/fuse/connections/*/abort; do
  [ -e "$c" ] || continue
  echo 1 > "$c" 2>/dev/null && n=$((n+1))
done
echo "==> aborted $n FUSE connection(s)"

# 2. Lazy-unmount every LizardFS (mfs#) mount, deepest first so nested XDG
#    bind-mounts detach before their parents.
mount | awk '/mfs#/ {print $3}' | sort -r | while read -r m; do
  fusermount -uz "$m" 2>/dev/null
  umount -l "$m" 2>/dev/null
  echo "    unmounted $m"
done

# 3. Mask the units so a respawn can't immediately re-wedge the box. Direct
#    /dev/null symlinks work even when systemd/dbus is too loaded to answer
#    `systemctl mask` (observed at load 80+).
if [ "$MASK_QNM" -eq 1 ]; then
  ln -sf /dev/null /etc/systemd/system/qnm-shared.service
  echo "==> masked qnm-shared.service"
fi
if [ "$MASK_MACKESD" -eq 1 ]; then
  ln -sf /dev/null /etc/systemd/system/mackesd.service
  echo "==> masked mackesd.service (unmask with: rm /etc/systemd/system/mackesd.service && systemctl daemon-reload)"
fi
systemctl daemon-reload 2>/dev/null &

echo "==> load after (settles over ~30-60s as I/O drains): $(cut -d' ' -f1-3 /proc/loadavg)"
echo "==> remaining D-state mackesd: $(ps -eo stat,cmd | grep -c '^D.*[m]ackesd serve')"
echo "Stragglers that won't drain need a reboot (reboot -f — a graceful reboot"
echo "hangs on the dead FUSE). Then rejoin the SUBSTRATE-V2 mesh; LizardFS stays masked."
