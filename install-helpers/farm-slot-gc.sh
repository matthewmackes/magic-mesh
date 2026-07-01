#!/usr/bin/env bash
# farm-slot-gc.sh — reclaim stale build-slot dirs on the MCNF build farm.
#
# Why: the dev-host disk-watchdog (DRAIN-2) does NOT cover the farm VMs. Each farm
# build runs in an isolated `~/magic-mesh-farm-<slot>/` with its own multi-GB
# `target/`; finished agents leave their slot dirs behind, which accumulate until a
# node hits 100% /home — then rustc SIGSEGVs / fails "No space left on device" and
# every build on that node wedges (observed live 2026-06-25: .130 → 100%, 30G+18G+17G
# of stale slots). This GCs the stale ones SAFELY (an active build is never touched).
#
# A slot dir is STALE when (a) no process has its CWD inside it AND (b) nothing under
# it was modified in the last IDLE_MIN minutes. Either condition true → keep it.
#
# Modes:
#   farm-slot-gc.sh                 GC the LOCAL node (what the systemd timer runs ON a farm VM)
#   farm-slot-gc.sh --remote        GC every farm node from the coordinator (ssh each → run local GC)
#   farm-slot-gc.sh --install       install the GC + a 20-min systemd timer ON the local node
#   farm-slot-gc.sh --deploy        push + --install the GC timer to every farm node (from coordinator)
#
# Env: MCNF_GC_IDLE_MIN (30) · MCNF_GC_KEEP ("slotA slotB" never-touch) ·
#      MCNF_FARM_NODES ("172.20.0.50 172.20.0.90 172.20.0.130 172.20.0.170") · MCNF_FARM_KEY · MCNF_FARM_USER
set -uo pipefail
IDLE_MIN="${MCNF_GC_IDLE_MIN:-30}"
KEEP=" ${MCNF_GC_KEEP:-} "
# 4 build VMs (incl. XEN-194/.170, the 4th dom0); canonical roster: install-helpers/farm-topology.sh.
NODES="${MCNF_FARM_NODES:-172.20.0.50 172.20.0.90 172.20.0.130 172.20.0.170}"
KEY="${MCNF_FARM_KEY:-/root/.ssh/mackes_mesh_ed25519}"
USER="${MCNF_FARM_USER:-mm}"
SELF="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"

# --- LOCAL GC: runs on a farm VM, reclaims that node's stale slot dirs ----------
gc_local() {
  local home freed=0 d slot sz active
  home="$(eval echo ~)"
  shopt -s nullglob
  for d in "$home"/magic-mesh-farm-* "$home"/magic-mesh-[0-9]* ; do
    [ -d "$d" ] || continue
    slot="${d##*magic-mesh-}"; slot="${slot#farm-}"
    case "$KEEP" in *" $slot "*) echo "  keep $slot (protected)"; continue;; esac
    # (a) any process CWD inside this slot dir? → active build, never touch.
    active=""
    for p in /proc/[0-9]*/cwd; do
      t="$(readlink -f "$p" 2>/dev/null)" || continue
      case "$t" in "$d"|"$d"/*) active=1; break;; esac
    done
    [ -n "$active" ] && { echo "  skip $slot (a build's CWD is inside it)"; continue; }
    # (b) modified within IDLE_MIN? → likely active, keep.
    if [ -n "$(find "$d" -maxdepth 4 -newermt "-${IDLE_MIN} min" -print -quit 2>/dev/null)" ]; then
      echo "  skip $slot (touched < ${IDLE_MIN}m ago)"; continue
    fi
    sz="$(du -sk "$d" 2>/dev/null | cut -f1)"; sz="${sz:-0}"
    if rm -rf "$d"; then freed=$((freed + sz)); printf '  removed %s (%.1fG)\n' "$slot" "$(awk "BEGIN{print $sz/1048576}")"; fi
  done
  printf '  GC done: freed ~%.1fG; /home now %s\n' "$(awk "BEGIN{print $freed/1048576}")" "$(df -h "$home" | awk 'NR==2{print $5" used, "$4" free"}')"
}

# --- REMOTE GC: drive the local GC on every farm node ---------------------------
gc_remote() {
  for n in $NODES; do
    echo "### farm-slot-gc on $n"
    ssh -i "$KEY" -o BatchMode=yes -o ConnectTimeout=10 "$USER@$n" \
      "MCNF_GC_IDLE_MIN=$IDLE_MIN MCNF_GC_KEEP='${MCNF_GC_KEEP:-}' bash -s" < "$SELF" 2>&1 \
      || echo "  ($n unreachable / GC failed)"
  done
}

# --- INSTALL: a systemd (or cron) timer on the local node -----------------------
install_local() {
  local bin="$HOME/.local/bin/farm-slot-gc.sh"
  mkdir -p "$HOME/.local/bin"; install -m 0755 "$SELF" "$bin"
  if command -v systemctl >/dev/null 2>&1 && systemctl --user show-environment >/dev/null 2>&1; then
    mkdir -p "$HOME/.config/systemd/user"
    cat >"$HOME/.config/systemd/user/farm-slot-gc.service" <<EOF
[Unit]
Description=MCNF farm build-slot GC (reclaim stale ~/magic-mesh-farm-* dirs)
[Service]
Type=oneshot
ExecStart=$bin
EOF
    cat >"$HOME/.config/systemd/user/farm-slot-gc.timer" <<EOF
[Unit]
Description=Run the MCNF farm-slot GC every 20 minutes
[Timer]
OnBootSec=5min
OnUnitActiveSec=20min
[Install]
WantedBy=timers.target
EOF
    systemctl --user daemon-reload && systemctl --user enable --now farm-slot-gc.timer \
      && echo "  installed farm-slot-gc.timer (user, every 20m) on $(hostname)"
  else
    # fallback: a cron entry
    ( crontab -l 2>/dev/null | grep -v farm-slot-gc; echo "*/20 * * * * $bin >/dev/null 2>&1" ) | crontab -
    echo "  installed farm-slot-gc cron (every 20m) on $(hostname)"
  fi
}

deploy_remote() {
  for n in $NODES; do
    echo "### deploy farm-slot-gc to $n"
    ssh -i "$KEY" -o BatchMode=yes -o ConnectTimeout=10 "$USER@$n" 'cat >/tmp/farm-slot-gc.sh && chmod +x /tmp/farm-slot-gc.sh && /tmp/farm-slot-gc.sh --install' < "$SELF" 2>&1 \
      || echo "  ($n deploy failed)"
  done
}

case "${1:-}" in
  ""|--local)  gc_local ;;
  --remote)    gc_remote ;;
  --install)   install_local ;;
  --deploy)    deploy_remote ;;
  *) echo "usage: $0 {--local|--remote|--install|--deploy}" >&2; exit 2 ;;
esac
