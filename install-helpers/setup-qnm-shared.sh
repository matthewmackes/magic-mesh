#!/bin/bash
# setup-qnm-shared.sh — ONBOARD-6: stand up the QNM-Shared LizardFS volume
# replicated over the Nebula overlay, boot-durable.
#
# QNM-Shared is the platform's shared-state plane (AI_GOVERNANCE §1): the leader
# lease, the peer directory, fleet rollups, and CA all live on it. The code
# always assumed it was a mounted replicated volume; this provisions that.
#
# Roles (compose freely on one node):
#   --master        lizardfs-master (run on the founding lighthouse only, §8)
#   --chunkserver   lizardfs-chunkserver (storage; >=2 for replication goal 2)
#   --client        mfsmount the volume at the QNM path (every node)
#
# Options:
#   --master-ip <ip>   master's OVERLAY ip (default 10.42.0.1)
#   --listen <ip>      this node's overlay ip for chunkserver/master binds
#   --qnm-path <dir>   mount point (default /root/QNM-Shared — where root mackesd reads)
#   --goal <n>         replication goal for the client to set (default 2)
#
# Idempotent: safe to re-run. Installs boot-durable systemd units so the volume
# comes up after nebula on every reboot. On F44 (no lizardfs in repo) pass the
# F43 lizardfs RPMs on PATH/installed first (the F43 client binary runs on F44).
set -euo pipefail

# Neutral, world-traversable mount (parent /mnt is 0755) so the root mackesd
# daemon AND the uid-1000 desktop GUIs share one volume — /root/QNM-Shared is
# unreadable to the GUI because /root is 0750. Pin MDE_WORKGROUP_ROOT to this.
MASTER_IP=10.42.0.1; LISTEN=""; QNM_PATH=/mnt/mesh-storage; GOAL=2
DO_MASTER=0; DO_CHUNK=0; DO_CLIENT=0; DO_SHADOW=0
while [ $# -gt 0 ]; do case "$1" in
  --master) DO_MASTER=1; shift;;
  --shadow) DO_SHADOW=1; shift;;
  --chunkserver) DO_CHUNK=1; shift;;
  --client) DO_CLIENT=1; shift;;
  --master-ip) MASTER_IP="$2"; shift 2;;
  --listen) LISTEN="$2"; shift 2;;
  --qnm-path) QNM_PATH="$2"; shift 2;;
  --goal) GOAL="$2"; shift 2;;
  *) echo "unknown arg: $1" >&2; exit 1;;
esac; done
[ "$DO_MASTER$DO_SHADOW$DO_CHUNK$DO_CLIENT" = "0000" ] && { sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'; exit 1; }
LISTEN="${LISTEN:-$MASTER_IP}"
log() { echo "==> $*"; }

# ---- master ---------------------------------------------------------------
if [ "$DO_MASTER" = 1 ]; then
  log "lizardfs-master on $MASTER_IP"
  cat > /etc/mfs/mfsmaster.cfg <<EOF
PERSONALITY = master
DATA_PATH = /var/lib/mfs
MATOML_LISTEN_HOST = $MASTER_IP
MATOCS_LISTEN_HOST = $MASTER_IP
MATOCL_LISTEN_HOST = $MASTER_IP
EOF
  echo "10.42.0.0/16    /    rw,alldirs,maproot=0" > /etc/mfs/mfsexports.cfg
  : > /etc/mfs/mfsgoals.cfg; : > /etc/mfs/mfstopology.cfg
  [ -s /var/lib/mfs/metadata.mfs ] || cp -a /var/lib/mfs/metadata.mfs.empty /var/lib/mfs/metadata.mfs
  chown -R mfs:mfs /var/lib/mfs /etc/mfs/mfs*.cfg 2>/dev/null || true
  # Boot-durable + self-healing: start after the overlay, and clear a stale
  # metadata/master lock left by an unclean shutdown (else mfsmaster refuses
  # to start and demands a manual mfsmetarestore).
  mkdir -p /etc/systemd/system/lizardfs-master.service.d
  cat > /etc/systemd/system/lizardfs-master.service.d/10-mesh.conf <<EOF
[Unit]
After=nebula.service
Wants=nebula.service
[Service]
ExecStartPre=-/bin/sh -c 'rm -f /var/lib/mfs/.mfsmaster.lock /var/lib/mfs/metadata.mfs.lock'
Restart=on-failure
RestartSec=5
EOF
  systemctl daemon-reload
  systemctl reset-failed lizardfs-master 2>/dev/null || true
  systemctl enable --now lizardfs-master
fi

# ---- shadow master (HA-1) -------------------------------------------------
# A live metadata standby: lizardfs-master with PERSONALITY=shadow streams the
# metadata from the real master + a metalogger keeps a cold backup. On master
# loss the HA worker (HA-3) promotes this node (personality→master + claim the
# VIP), fenced by the QNM leader-lease. Runs alongside the chunkserver (Q8). The
# QNM-Shared master is otherwise a SPOF (2026-06-17 outage) — this removes it.
if [ "$DO_SHADOW" = 1 ]; then
  log "lizardfs shadow master -> master $MASTER_IP, listen $LISTEN"
  cat > /etc/mfs/mfsmaster.cfg <<EOF
PERSONALITY = shadow
DATA_PATH = /var/lib/mfs
MASTER_HOST = $MASTER_IP
MATOML_LISTEN_HOST = $LISTEN
MATOCS_LISTEN_HOST = $LISTEN
MATOCL_LISTEN_HOST = $LISTEN
EOF
  echo "10.42.0.0/16    /    rw,alldirs,maproot=0" > /etc/mfs/mfsexports.cfg
  : > /etc/mfs/mfsgoals.cfg; : > /etc/mfs/mfstopology.cfg
  [ -s /var/lib/mfs/metadata.mfs ] || cp -a /var/lib/mfs/metadata.mfs.empty /var/lib/mfs/metadata.mfs
  # Metalogger — a cold metadata backup pulled from the master.
  cat > /etc/mfs/mfsmetalogger.cfg <<EOF
MASTER_HOST = $MASTER_IP
DATA_PATH = /var/lib/mfs
EOF
  chown -R mfs:mfs /var/lib/mfs /etc/mfs/mfs*.cfg 2>/dev/null || true
  mkdir -p /etc/systemd/system/lizardfs-master.service.d
  cat > /etc/systemd/system/lizardfs-master.service.d/10-mesh.conf <<EOF
[Unit]
After=nebula.service
Wants=nebula.service
[Service]
# A shadow refusing to start on a stale lock would leave the mesh without HA.
ExecStartPre=-/bin/sh -c 'rm -f /var/lib/mfs/.mfsmaster.lock /var/lib/mfs/metadata.mfs.lock'
Restart=on-failure
RestartSec=5
EOF
  systemctl daemon-reload
  systemctl reset-failed lizardfs-master 2>/dev/null || true
  systemctl enable --now lizardfs-master
  systemctl enable --now lizardfs-metalogger 2>/dev/null \
    || log "WARN: lizardfs-metalogger unit absent (install lizardfs-metalogger; F44: the F43 RPM)"
fi

# ---- chunkserver ----------------------------------------------------------
if [ "$DO_CHUNK" = 1 ]; then
  log "lizardfs-chunkserver -> master $MASTER_IP, listen $LISTEN"
  mkdir -p /var/lib/mfs/chunks; chown -R mfs:mfs /var/lib/mfs
  cat > /etc/mfs/mfschunkserver.cfg <<EOF
DATA_PATH = /var/lib/mfs
MASTER_HOST = $MASTER_IP
MASTER_PORT = 9420
CSSERV_LISTEN_HOST = $LISTEN
EOF
  echo "/var/lib/mfs/chunks" > /etc/mfs/mfshdd.cfg
  chown mfs:mfs /etc/mfs/mfschunkserver.cfg /etc/mfs/mfshdd.cfg
  mkdir -p /etc/systemd/system/lizardfs-chunkserver.service.d
  cat > /etc/systemd/system/lizardfs-chunkserver.service.d/10-mesh.conf <<EOF
[Unit]
After=nebula.service
Wants=nebula.service
[Service]
Restart=on-failure
RestartSec=5
EOF
  systemctl daemon-reload
  systemctl reset-failed lizardfs-chunkserver 2>/dev/null || true
  systemctl enable --now lizardfs-chunkserver
fi

# ---- client (boot-durable mount) ------------------------------------------
if [ "$DO_CLIENT" = 1 ]; then
  log "QNM-Shared client mount at $QNM_PATH -> $MASTER_IP"
  command -v mfsmount >/dev/null || { echo "mfsmount missing (install lizardfs-client; F44: the F43 client RPM)"; exit 1; }
  # AUDIT-MESH-12 — the Mesh Storage panel shells `lizardfs-admin
  # list-chunkservers <vip> 9421 --porcelain` (mackesd's mesh-fs-status, the
  # LizardFS admin CLI, NOT MooseFS mfsadmin). It ships in `lizardfs-adm`, which
  # like lizardfs-client is absent from the F44 base repos — install the F43
  # `lizardfs-adm` RPM out-of-band on F44 (the fc43 binary runs on F44). Warn,
  # don't fail: the mount itself works without it, only the storage panel needs it.
  command -v lizardfs-admin >/dev/null || \
    log "WARN: lizardfs-admin missing — Mesh Storage panel can't query the master (install lizardfs-adm; F44: the F43 lizardfs-adm RPM)"
  echo "$MASTER_IP" > /etc/mackesd-qnm-master 2>/dev/null || true
  # allow_other lets the uid-1000 desktop GUIs read the root-owned FUSE mount;
  # it requires user_allow_other in /etc/fuse.conf. Enable it idempotently.
  if [ -f /etc/fuse.conf ] && ! grep -qE '^[[:space:]]*user_allow_other' /etc/fuse.conf; then
    if grep -qE '^[[:space:]]*#[[:space:]]*user_allow_other' /etc/fuse.conf; then
      sed -i 's/^[[:space:]]*#[[:space:]]*user_allow_other/user_allow_other/' /etc/fuse.conf
    else
      echo user_allow_other >> /etc/fuse.conf
    fi
    log "enabled user_allow_other in /etc/fuse.conf"
  fi
  # A oneshot mount unit ordered after nebula; mackesd is made to wait on it so
  # it never reads a not-yet-mounted QNM-Shared. ExecStartPre waits for the
  # overlay master to answer so a boot race can't fail the mount.
  cat > /etc/systemd/system/qnm-shared.service <<EOF
[Unit]
Description=Mount the QNM-Shared LizardFS volume over the overlay
After=nebula.service network-online.target
Wants=nebula.service
# XPA-8 (2026-06-17): do NOT order before mackesd. This unit RETRIES the mount
# for up to ~2 min and Restart=on-failure loops it; a hard `Before=mackesd`
# (and the matching `After=qnm-shared` drop-in) made mackesd's start job QUEUE
# behind it and, on a node where the mount can't yet succeed (e.g. fuse-libs
# missing — XPA-9), mackesd NEVER started (silent "inactive", no journal).
# mackesd self-heals the mount via meshfs_worker + BOOT-REC, so it must start
# independently; this unit is a best-effort Wants= pull only (drop-in below).
# BOOT-REC-2: never let the start-limit burst give up + leave the mount failed
# (the stuck-"activating"/NO-LEADER state seen after a cold reboot). Retry until
# the overlay+master are reachable; the mesh-health watchdog covers later drops.
StartLimitIntervalSec=0
[Service]
Type=oneshot
RemainAfterExit=yes
# BOOT-REC-2: a cold boot brings nebula + the LizardFS master up AFTER this unit
# is first scheduled, so RETRY the actual mount until it succeeds (the master
# becomes reachable a few seconds into boot). A portable POSIX loop — no bashism
# (/dev/tcp is unavailable under /bin/sh=dash, which made the old probe spin the
# full timeout and block mackesd). Bounded ~2 min; on a genuinely-down master it
# exits non-zero → Restart + the mesh-health watchdog keep retrying.
# LH-JOIN-QNM-1 (2026-06-20): wedge-proof. Every check that touches the mount is
# `timeout`-guarded so a half-formed/stale FUSE mount in uninterruptible D-state
# (mfsmount daemon gone, kernel entry lingering) can NEVER hang the loop; the
# recovery uses `fusermount -uz` + `umount -l` (LAZY) so it actually detaches a
# wedged mount (plain `-u` can't). A fresh remote lighthouse join hit exactly this
# (mackesd, started Wants-only per XPA-8, writes stray into the unmounted path →
# mfsmount over non-empty half-succeeds-then-dies → wedge that survives reboot).
ExecStart=/bin/sh -c 'i=0; while [ \$i -lt 15 ]; do timeout 6 mountpoint -q $QNM_PATH && exit 0; fusermount -uz $QNM_PATH 2>/dev/null; umount -l $QNM_PATH 2>/dev/null; pkill -f "mfsmount $QNM_PATH" 2>/dev/null; sleep 1; if [ -n "\$(timeout 6 ls -A $QNM_PATH 2>/dev/null)" ]; then d=/var/lib/mde/qnm-stray-\$(date +%s 2>/dev/null || echo bk); mkdir -p \$d; mv $QNM_PATH/* $QNM_PATH/.[!.]* \$d/ 2>/dev/null; fi; mfsmount $QNM_PATH -H $MASTER_IP -o allow_other,nonempty 2>/dev/null; sleep 3; timeout 6 mountpoint -q $QNM_PATH && exit 0; i=\$((i+1)); sleep 2; done; exit 1'
ExecStop=/bin/sh -c 'fusermount -uz $QNM_PATH 2>/dev/null; umount -l $QNM_PATH 2>/dev/null; true'
# LH-JOIN-QNM-1: bound the start job ABOVE the loop's worst case. WITHOUT this,
# systemd's default 90s TimeoutStartSec fired mid-loop and SIGKILLed the mount
# script ("fatal signal delivered to the control process"), leaving a wedged
# half-mount. The normal path exits 0 within a few seconds once the master
# answers; the ceiling only matters on a repeatedly-wedging node (15 iters ×
# ≤~24s each), so set it well above that — the timeout-guarded checks guarantee
# the loop always makes progress, it never genuinely hangs.
TimeoutStartSec=600
Restart=on-failure
# BOOT-XPA8-2 — a disconnected node (laptop off-mesh) can never mount, so don't
# spin the boot-race loop (now ~60s, was ~120s) back-to-back every few seconds:
# back off 30s between failed cycles. mackesd is NOT gated on this (XPA-8) and
# self-heals the mount via meshfs_worker, so the gentler cadence costs nothing
# on a connected node (the loop exits 0 the moment the master is reachable).
RestartSec=30
[Install]
WantedBy=multi-user.target
EOF
  mkdir -p /etc/systemd/system/mackesd.service.d
  cat > /etc/systemd/system/mackesd.service.d/20-qnm.conf <<EOF
[Unit]
# XPA-8: Wants (best-effort pull) but NOT After — mackesd must not block on the
# mount-retry loop. mackesd self-heals the mount (meshfs_worker + BOOT-REC) and
# refuses to poison the unmounted mountpoint (shared_root_writable guard).
Wants=qnm-shared.service
EOF
  mkdir -p "$QNM_PATH"
  systemctl daemon-reload
  systemctl enable qnm-shared.service
  mountpoint -q "$QNM_PATH" || systemctl start qnm-shared.service || true
  if mountpoint -q "$QNM_PATH"; then
    log "mounted; setting replication goal $GOAL"
    mfssetgoal -r "$GOAL" "$QNM_PATH" >/dev/null 2>&1 || true
  fi
fi
log "done"
