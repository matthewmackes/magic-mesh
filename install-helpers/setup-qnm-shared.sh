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
DO_MASTER=0; DO_CHUNK=0; DO_CLIENT=0
while [ $# -gt 0 ]; do case "$1" in
  --master) DO_MASTER=1; shift;;
  --chunkserver) DO_CHUNK=1; shift;;
  --client) DO_CLIENT=1; shift;;
  --master-ip) MASTER_IP="$2"; shift 2;;
  --listen) LISTEN="$2"; shift 2;;
  --qnm-path) QNM_PATH="$2"; shift 2;;
  --goal) GOAL="$2"; shift 2;;
  *) echo "unknown arg: $1" >&2; exit 1;;
esac; done
[ "$DO_MASTER$DO_CHUNK$DO_CLIENT" = "000" ] && { sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'; exit 1; }
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
Before=mackesd.service
[Service]
Type=oneshot
RemainAfterExit=yes
ExecStartPre=/bin/sh -c 'for i in \$(seq 1 30); do mfsmount -H $MASTER_IP -t 2 /mnt 2>/dev/null && fusermount -u /mnt 2>/dev/null && exit 0; sleep 2; done; exit 0'
ExecStart=/bin/sh -c 'mountpoint -q $QNM_PATH || mfsmount $QNM_PATH -H $MASTER_IP -o allow_other'
ExecStop=/bin/sh -c 'fusermount -u $QNM_PATH 2>/dev/null || umount -l $QNM_PATH 2>/dev/null || true'
Restart=on-failure
RestartSec=5
[Install]
WantedBy=multi-user.target
EOF
  mkdir -p /etc/systemd/system/mackesd.service.d
  cat > /etc/systemd/system/mackesd.service.d/20-qnm.conf <<EOF
[Unit]
After=qnm-shared.service
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
