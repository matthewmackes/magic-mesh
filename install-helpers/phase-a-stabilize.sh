#!/usr/bin/env bash
# phase-a-stabilize.sh — operator-run Phase-A stabilization of the wedged SUBSTRATE-V2 fleet.
#
# State (discovered 2026-06-24): etcd is ALREADY up + a healthy 3-member quorum;
# LizardFS/FUSE is already DOWN. But mackesd crash-loops on the lighthouses because
# (a) /run tmpfs is 100% full from the mde-bus spool, and (b) the legacy LizardFS
# plane (qnm-shared retry loop / mfschunkserver) keeps churning + re-filling /run.
# This is incident remediation, NOT the etcd-init cutover (which already happened).
#
# Per node, reversible (LizardFS stays as the one-release backout):
#   1. abort any residual wedged FUSE + lazy-unmount mfs#   (no-op if already down)
#   2. mask the wedged qnm-shared retry loop + drop the dead LizardFS dnf mirror
#   3. stop mackesd, reclaim the EPHEMERAL /run/mde-bus tmpfs IPC spool
#   4. (lh2 only) rewrite the malformed/duplicated etcd-endpoints
#   5. restart mackesd clean + verify (/run, mackesd state, ABRT rate, etcd leader)
#
# Run from THIS control host as root (it has the keys + Eagle's sudo cred):
#   sudo bash install-helpers/phase-a-stabilize.sh
# Then SOAK 30-60 min and re-check. The dangerous etcd member-topology change
# (Eagle) and Phase B (retire LizardFS -> Syncthing) are intentionally NOT here.
set -uo pipefail
KEY=/root/.ssh/id_ed25519
CRED=/root/.mcnf-xapi-cred            # mm's sudo password on Eagle
ANCHOR=http://10.42.0.1:2379
SSH_OPTS=(-o StrictHostKeyChecking=accept-new -o ConnectTimeout=15)

BODY=$(mktemp); trap 'rm -f "$BODY"' EXIT
cat > "$BODY" <<NODE
set +e
# 1. residual FUSE (idempotent; already-down nodes find nothing)
for c in /sys/fs/fuse/connections/*/abort; do [ -e "\$c" ] && echo 1 > "\$c" 2>/dev/null; done
mount | awk '/mfs#/{print \$3}' | sort -r | while read -r m; do fusermount -uz "\$m" 2>/dev/null; umount -l "\$m" 2>/dev/null; done
# 2. mask the wedged qnm-shared loop + drop the dead LizardFS dnf mirror
if systemctl list-unit-files 2>/dev/null | grep -q '^qnm-shared.service'; then ln -sf /dev/null /etc/systemd/system/qnm-shared.service; echo "  masked qnm-shared.service"; fi
rm -f /etc/yum.repos.d/mackes-mirror-magic-mesh.repo 2>/dev/null
# 2b. SUBSTRATE-V2 retirement: disable the dead LizardFS binaries so the in-mackesd
#     meshfs_worker hits its documented no-op path. The LizardFS master is gone
#     (2026-06-20), but mfschunkserver/mfssetgoal are still ON DISK, so the worker
#     re-spawns them every ~5s; each fails (no master / no mfshdd.cfg) and that churn
#     starves the 60s systemd watchdog -> mackesd SIGABRT crash-loop (diagnosed live
#     2026-06-25). Reversible: renamed, not removed. (Durable fix = the meshfs_worker
#     !on_etcd code gate; this script unwedges the live fleet now.)
pkill -9 mfschunkserver mfsmaster 2>/dev/null
for b in mfschunkserver mfsmaster mfssetgoal; do p=\$(command -v \$b 2>/dev/null); [ -n "\$p" ] && mv -f "\$p" "\$p.cutover-disabled" 2>/dev/null && echo "  disabled \$p"; done
rm -f /var/lib/mfs/.mfschunkserver.lock 2>/dev/null
# 3. stop mackesd + reclaim the ephemeral /run/mde-bus spool (tmpfs IPC, regenerated on next publish)
systemctl stop mackesd 2>/dev/null
rm -rf /run/mde-bus/* 2>/dev/null   # full ephemeral spool wipe — rm -f missed the audit/<node>/ + state/boot-readiness/ SUBDIRS that filled the 190M tmpfs
# 5. restart clean + verify across a FULL watchdog cycle (so the read is real, not a 10s snapshot)
systemctl daemon-reload 2>/dev/null
systemctl reset-failed mackesd 2>/dev/null
systemctl start mackesd
sleep 72
printf "  RESULT  /run=%s  mackesd=%s  NRestarts=%s  ABRT(70s)=%s  churn(70s)=%s  leader=%s\n" \
  "\$(df -h /run | awk 'NR==2{print \$5}')" \
  "\$(systemctl is-active mackesd)" \
  "\$(systemctl show -p NRestarts --value mackesd)" \
  "\$(journalctl -u mackesd --since '70 sec ago' 2>/dev/null | grep -c 'status=6/ABRT')" \
  "\$(journalctl -u mackesd --since '70 sec ago' 2>/dev/null | grep -ciE 'mfschunkserver|converging replication|exited non-zero')" \
  "\$(ETCDCTL_API=3 etcdctl --endpoints=$ANCHOR get /mesh/leader --print-value-only 2>/dev/null | head -c40)"
NODE

run_root() { echo "########## $2 ($1) ##########"; ssh -i "$KEY" "${SSH_OPTS[@]}" "root@$1" 'bash -s' < "$BODY"; }

run_root 167.71.247.150 "lh1 — founding anchor"

echo "########## lh2 (68.183.55.253) — fix malformed endpoints first ##########"
ssh -i "$KEY" "${SSH_OPTS[@]}" root@68.183.55.253 \
  "printf 'http://10.42.0.3:2379,http://10.42.0.1:2379,http://10.42.0.2:2379\n' > /etc/mackesd/etcd-endpoints && chmod 0644 /etc/mackesd/etcd-endpoints && echo '  rewrote lh2 etcd-endpoints (de-duplicated 3-entry csv)'"
ssh -i "$KEY" "${SSH_OPTS[@]}" root@68.183.55.253 'bash -s' < "$BODY"

echo "########## Eagle (172.20.146.13) — via sudo ##########"
{ cat "$CRED"; printf '\n'; cat "$BODY"; } | sshpass -f "$CRED" \
  ssh -o PreferredAuthentications=password -o PubkeyAuthentication=no "${SSH_OPTS[@]}" mm@172.20.146.13 "sudo -S -p '' bash -s"

echo
echo "########## DONE. SOAK 30-60 min, then re-check (read-only):"
echo "  ssh -i $KEY root@167.71.247.150 'ETCDCTL_API=3 etcdctl --endpoints=$ANCHOR endpoint health --cluster; etcdctl --endpoints=$ANCHOR get /mesh/leader --print-value-only; df -h /run; journalctl -u mackesd --since \"5 min ago\" | grep -c ABRT'"
echo "Acceptance: endpoint health all OK, exactly one /mesh/leader, /run < 80%, ZERO new ABRT over the soak."