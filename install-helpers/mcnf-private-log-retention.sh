#!/usr/bin/env bash
# PRIVACY-RETENTION-1 — enforce the fleet-wide six-hour runtime-history ceiling.
set -euo pipefail

MODE="${1:---sweep}"
RETENTION_MINUTES=360

require_root() {
  if [ "$(id -u)" -ne 0 ]; then
    printf 'mcnf-private-log-retention: root is required\n' >&2
    exit 77
  fi
}

stop_if_active() {
  local unit="$1"
  if systemctl is-active --quiet "$unit" 2>/dev/null; then
    systemctl stop "$unit"
    STOPPED_UNITS+=("$unit")
  fi
}

restart_stopped() {
  local unit
  if [ "${#STOPPED_UNITS[@]}" -eq 0 ]; then
    return
  fi
  for unit in "${STOPPED_UNITS[@]}"; do
    systemctl start "$unit"
  done
}

purge_flat_logs() {
  # Rotate the live journal first so vacuuming covers every pre-purge record.
  journalctl --rotate >/dev/null 2>&1 || true
  journalctl --vacuum-time=1s >/dev/null 2>&1 || true

  # Stop traditional writers where the platform permits it, remove rotated
  # files, and truncate current files without deleting their ownership/mode.
  stop_if_active rsyslog.service
  stop_if_active netdata.service
  find /var/log -xdev -type f -exec truncate -s 0 -- {} + 2>/dev/null || true
  find /var/cache/netdata -xdev -mindepth 1 -delete 2>/dev/null || true
}

sweep_flat_logs() {
  journalctl --rotate >/dev/null 2>&1 || true
  journalctl --vacuum-time=6h >/dev/null 2>&1 || true
  if command -v logrotate >/dev/null 2>&1 && [ -f /etc/logrotate.conf ]; then
    logrotate --force /etc/logrotate.conf >/dev/null 2>&1 || true
  fi
  # Active files are force-rotated above. This removes only old generations;
  # journal storage is managed by journalctl and is deliberately excluded.
  find /var/log -xdev -path /var/log/journal -prune -o \
    -path '/var/log/journal/*' -prune -o -type f -mmin "+${RETENTION_MINUTES}" \
    -delete 2>/dev/null || true
}

purge_bus_history() {
  local root
  for root in /run/mde-bus /run/user/*/mde-bus /root/.local/share/mde/bus /home/*/.local/share/mde/bus; do
    [ -d "$root" ] || continue
    find "$root" -maxdepth 1 -type f -name 'index.sqlite*' -delete
    find "$root" -mindepth 1 -maxdepth 1 -type d \
      ! -name local ! -name .mackesd-worker-owners -exec rm -rf -- {} +
  done
}

purge_application_history() {
  # Exact history authorities only: preserve settings, identities, databases,
  # media, VM disks, transfer payloads, and current materialized snapshots.
  local root
  for root in /mnt/mesh-storage /var/lib/mde /var/lib/mackesd \
    /root/.local/share/mde /home/*/.local/share/mde; do
    [ -d "$root" ] || continue
    find "$root" -xdev -type f \( \
      -name '*.jsonl' -o -name '*.log' -o -name '*.log.*' -o \
      -name 'history.json' -o -name '*history*.jsonl' -o \
      -name '*.sync-conflict-*' \) -delete 2>/dev/null || true
  done
  find /mnt/mesh-storage/.stversions -mindepth 1 -delete 2>/dev/null || true
  for root in /var/lib/mde/transfers/ledger /var/lib/mde/transfers/ledger-v2 \
    /var/lib/mde/transfers/notification-outbox; do
    [ -d "$root" ] && find "$root" -mindepth 1 -delete
  done
}

application_epoch() {
  # These writers keep descriptors open, so quiesce them before deleting their
  # histories. Units absent on farm/dom0 nodes are harmlessly ignored.
  # `mackesd.service` covers pre-grouped lighthouse releases; the six explicit
  # groups close races even when stopping a target does not propagate.
  stop_if_active mackesd.service
  stop_if_active mackesd-control.service
  stop_if_active mackesd-observation.service
  stop_if_active mackesd-actions.service
  stop_if_active mackesd-data.service
  stop_if_active mackesd-compute.service
  stop_if_active mackesd-integrations.service
  stop_if_active mackesd.target
  stop_if_active syncthing.service
  stop_if_active syncthing@mm.service
  stop_if_active syncthing@root.service
  purge_bus_history
  purge_application_history
}

require_root
declare -a STOPPED_UNITS=()
trap restart_stopped EXIT

case "$MODE" in
  --purge)
    application_epoch
    purge_flat_logs
    ;;
  --application-epoch)
    application_epoch
    sweep_flat_logs
    ;;
  --sweep)
    sweep_flat_logs
    ;;
  *)
    printf 'usage: %s [--purge|--application-epoch|--sweep]\n' "$0" >&2
    exit 64
    ;;
esac

restart_stopped
STOPPED_UNITS=()
printf 'mcnf-private-log-retention: %s complete (ceiling=6h)\n' "$MODE"
