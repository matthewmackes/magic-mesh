#!/usr/bin/env bash
# phase-b-retire-lizardfs.sh — SUBSTRATE-6: retire LizardFS, files onto Syncthing.
#
# Run AFTER phase-a-stabilize.sh has un-wedged the fleet and it has soaked clean on
# etcd (zero new ABRT, one /mesh/leader). Per node, anchors first (lh1 -> lh2 -> Eagle):
# ensure /mnt/mesh-storage is a PLAIN dir replicated by Syncthing, retire the LizardFS
# mount unit, restart mackesd onto the Syncthing file plane, verify.
#
# Safe here because LizardFS is already DEAD (master gone 2026-06-20): there is no FUSE
# mount to drain and no LizardFS backout to preserve, so the runbook's "Phase B is the
# hard-rollback stage" caution is moot — this finishes the migration the dead master
# left half-done. The data is already on the plain dir + Syncthing-replicated.
#
# Run from this control host as root:  sudo bash install-helpers/phase-b-retire-lizardfs.sh
set -uo pipefail
KEY=/root/.ssh/id_ed25519
CRED=/root/.mcnf-xapi-cred                 # mm's sudo password on Eagle
ANCHOR=http://10.42.0.1:2379

# per-node body run AS ROOT on the target; reads $LISTEN (the node's overlay IP)
BODY=$(mktemp); trap 'rm -f "$BODY"' EXIT
cat > "$BODY" <<'NODE'
set +e
echo "  listen(overlay)=$LISTEN"
# DATA-SAFETY gate: never run on a node where /mnt/mesh-storage is STILL a live LizardFS mount
if mount | grep -q 'mfs#.*mesh-storage'; then echo "  ABORT: /mnt/mesh-storage is STILL a LizardFS FUSE mount — run phase-a-stabilize first"; exit 9; fi
echo "  /mnt/mesh-storage: $(df -hT /mnt/mesh-storage 2>/dev/null | awk 'NR==2{print $2,$3" used"}')  files=$(ls -A /mnt/mesh-storage 2>/dev/null | wc -l)"
# 1. SUBSTRATE-5/6: make /mnt/mesh-storage a plain Syncthing folder (idempotent — already is)
SETUP=/usr/libexec/mackesd/setup-syncthing; [ -x "$SETUP" ] || SETUP=/usr/libexec/mackesd/setup-syncthing.sh
if [ -x "$SETUP" ]; then "$SETUP" --listen "$LISTEN" 2>&1 | tail -3; else echo "  WARN: setup-syncthing helper not found at $SETUP"; fi
systemctl enable --now syncthing-reconcile.timer 2>/dev/null && echo "  reconcile timer armed"
# 2. retire the LizardFS mount unit (idempotent; phase-a-stabilize may have masked it)
[ "$(readlink /etc/systemd/system/qnm-shared.service 2>/dev/null)" = /dev/null ] || { ln -sf /dev/null /etc/systemd/system/qnm-shared.service; echo "  masked qnm-shared.service"; }
rm -f /etc/yum.repos.d/mackes-mirror-magic-mesh.repo 2>/dev/null
systemctl daemon-reload 2>/dev/null
# 3. restart mackesd onto the clean Syncthing file plane + verify
systemctl stop mackesd 2>/dev/null; rm -f /run/mde-bus/audit/* /run/mde-bus/state/* 2>/dev/null; systemctl start mackesd; sleep 10
printf "  RESULT  mackesd=%s  ABRT(20s)=%s  syncthing-connected=%s  meshfs-churn(20s)=%s\n" \
  "$(systemctl is-active mackesd)" \
  "$(journalctl -u mackesd --since '20 sec ago' 2>/dev/null | grep -c ABRT)" \
  "$(syncthing cli --home=/var/lib/mcnf-syncthing show connections 2>/dev/null | grep -c '\"connected\": true')" \
  "$(journalctl -u mackesd --since '20 sec ago' 2>/dev/null | grep -ciE 'mfschunkserver|mfssetgoal')"
command -v mesh-health-check.sh >/dev/null 2>&1 && mesh-health-check.sh 2>&1 | grep -iE 'out of sync|OK|healthy' | head -2
NODE

run_root() { echo "########## $3 ($1, overlay $2) ##########"; ssh -i "$KEY" -o StrictHostKeyChecking=accept-new -o ConnectTimeout=15 "root@$1" "LISTEN=$2 bash -s" < "$BODY"; }

echo "### Phase B — LizardFS retirement. Pre-check: Phase A must have soaked clean. ###"
run_root 167.71.247.150 10.42.0.1 "lh1 — founding anchor"
run_root 68.183.55.253  10.42.0.3 "lh2 — lighthouse"
echo "########## Eagle (172.20.146.13, overlay 10.42.0.2) — via sudo ##########"
{ cat "$CRED"; printf '\n'; cat "$BODY"; } | sshpass -f "$CRED" ssh -o PreferredAuthentications=password -o PubkeyAuthentication=no -o StrictHostKeyChecking=accept-new -o ConnectTimeout=15 mm@172.20.146.13 "sudo -S -p '' LISTEN=10.42.0.2 bash -s"
echo
echo "### DONE. Acceptance: write a file on one node's /mnt/mesh-storage -> appears on the others;"
echo "    mesh-health-check.sh quiet (no 'Mesh Sync OUT OF SYNC'); zero mackesd ABRT; meshfs-churn=0 on all nodes."
echo "### If meshfs-churn stays >0 (the LizardFS storage supervisor still loops independent of the mount),"
echo "    that's a mackesd code gap (the supervisor isn't Syncthing-plane-aware) — flag it and I'll land the gating fix."