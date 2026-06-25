#!/usr/bin/env bash
# fix-broker-ntfy.sh — install the mde-bus broker (ntfy) on the SUBSTRATE-V2 fleet.
#
# Root cause (diagnosed live 2026-06-25): the mde-bus broker is an `ntfy` daemon the
# mde-bus process spawns on `<overlay_ip>:8443`. `ntfy` is ABSENT on the lighthouses
# (not in the dnf repos, never installed), so `evaluate_prereqs` returns
# `NtfyMissing` → the broker is skipped → mackesd cannot reach its own :8443 broker →
# every publish is persisted to the /run mde-bus spool (which grows: boot-readiness
# 25M + audit 19M + compute 13M) AND the broker-unreachable path eventually starves
# the 60s systemd watchdog → SIGABRT, ~every 3 minutes (NRestarts climbing on lh1+lh2).
#
# This is the LIVE fix: fetch the ntfy static binary (it is not packaged), install it
# on PATH, wipe the bloated ephemeral spool, and restart mackesd so the broker spawns.
# The DURABLE fixes (ntfy bundled into the node setup + mackesd made resilient to a
# missing broker so it never starves the watchdog) land in the branch / next release.
#
# Run from this control host as root:  sudo bash install-helpers/fix-broker-ntfy.sh
set -uo pipefail
KEY=/root/.ssh/id_ed25519
CRED=/root/.mcnf-xapi-cred                 # mm's sudo password on Eagle
NTFY_VER=2.11.0
NTFY_URL="https://github.com/binwiederhier/ntfy/releases/download/v${NTFY_VER}/ntfy_${NTFY_VER}_linux_amd64.tar.gz"

BODY=$(mktemp); trap 'rm -f "$BODY"' EXIT
cat > "$BODY" <<NODE
set +e
# 1. install the ntfy broker binary if absent (idempotent)
if command -v ntfy >/dev/null 2>&1; then
  echo "  ntfy already present: \$(command -v ntfy)"
else
  echo "  fetching ntfy ${NTFY_VER}..."
  if curl -fsSL "${NTFY_URL}" -o /tmp/ntfy.tgz && tar -xzf /tmp/ntfy.tgz -C /tmp 2>/dev/null; then
    install -m 0755 "/tmp/ntfy_${NTFY_VER}_linux_amd64/ntfy" /usr/local/bin/ntfy 2>/dev/null \
      && echo "  installed: \$(/usr/local/bin/ntfy --version 2>/dev/null | head -1)" \
      || echo "  NTFY INSTALL FAILED (extract/place)"
  else
    echo "  NTFY FETCH FAILED (no internet / bad url) — broker stays skipped"
  fi
  rm -rf /tmp/ntfy.tgz "/tmp/ntfy_${NTFY_VER}_linux_amd64" 2>/dev/null
fi
# 2. wipe the bloated ephemeral /run/mde-bus spool (tmpfs IPC, regenerated) + clean restart
systemctl stop mackesd 2>/dev/null
rm -rf /run/mde-bus/* 2>/dev/null
systemctl reset-failed mackesd 2>/dev/null
systemctl start mackesd
# 3. soak ~70s (one full watchdog cycle) so the broker-up / no-abort result is real
sleep 72
printf "  RESULT  ntfy=%s  broker:8443=%s  mackesd=%s  NRestarts=%s  ABRT(70s)=%s  /run=%s\n" \
  "\$(command -v ntfy >/dev/null 2>&1 && echo present || echo ABSENT)" \
  "\$(ss -tlnp 2>/dev/null | grep -c ':8443')" \
  "\$(systemctl is-active mackesd)" \
  "\$(systemctl show -p NRestarts --value mackesd)" \
  "\$(journalctl -u mackesd --since '70 sec ago' 2>/dev/null | grep -c 'status=6/ABRT')" \
  "\$(df -h /run | awk 'NR==2{print \$5}')"
NODE

run_root() { echo "########## $2 ($1) ##########"; ssh -i "$KEY" -o StrictHostKeyChecking=accept-new -o ConnectTimeout=15 "root@$1" 'bash -s' < "$BODY"; }

echo "### Broker (ntfy) install + clean restart across the fleet. Each node soaks ~72s. ###"
run_root 167.71.247.150 "lh1 — founding anchor (10.42.0.1)"
run_root 68.183.55.253  "lh2 — lighthouse (10.42.0.3)"
echo "########## Eagle (172.20.146.13, overlay 10.42.0.2) — via sudo ##########"
{ cat "$CRED"; printf '\n'; cat "$BODY"; } | sshpass -f "$CRED" \
  ssh -o PreferredAuthentications=password -o PubkeyAuthentication=no -o StrictHostKeyChecking=accept-new -o ConnectTimeout=15 mm@172.20.146.13 "sudo -S -p '' bash -s"
echo
echo "### DONE. Acceptance: broker:8443=1 (listening), NRestarts stops climbing, ABRT(70s)=0, /run < 80% on all three."
echo "### Durable follow-up (in the branch, next release): bundle ntfy in the node setup + make mackesd resilient to a"
echo "### missing broker (bounded retry + spool cap + watchdog never starved) so a skipped broker is truly non-fatal."
