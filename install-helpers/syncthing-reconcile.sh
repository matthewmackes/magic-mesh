#!/bin/bash
# syncthing-reconcile.sh — SUBSTRATE-5 self-heal for the Syncthing device list.
#
# setup-syncthing.sh wires the peers present in the etcd `/mesh/syncthing/<host>`
# registry *at provision time* and is one-shot — a node that provisions BEFORE its
# peers register never learns them, and existing nodes never pick up a late-joiner
# (live symptom, SUBSTRATE-14 rehearsal: A logged `Connection from <B> … rejected:
# unknown device` in a loop). This reconciler closes that gap: timer-driven, it
# reads the registry and adds any MISSING peer device to the RUNNING syncthing via
# `syncthing cli config` — LIVE, no service restart. It is strictly additive and
# idempotent: a device already in the config is skipped, so a steady-state run is a
# no-op and never disrupts an in-flight sync (re-running the full setup-syncthing.sh
# would `systemctl restart syncthing` every cycle — exactly what we avoid here).
set -uo pipefail

H="${MCNF_SYNCTHING_HOME:-/var/lib/mcnf-syncthing}"
ENDPOINTS_FILE=/etc/mackesd/etcd-endpoints
FOLDER_ID="${MCNF_SYNCTHING_FOLDER_ID:-mcnf-mesh}"
HOST="$(hostname)"
DEV_RE='^[A-Z2-7]{7}(-[A-Z2-7]{7}){7}$'

[ -s "$ENDPOINTS_FILE" ] || exit 0                       # no etcd substrate → nothing to do
command -v etcdctl   >/dev/null 2>&1 || exit 0
command -v syncthing >/dev/null 2>&1 || exit 0
systemctl is-active --quiet syncthing || exit 0          # daemon down → nothing to reconcile live

EPS="$(tr '\n' ',' < "$ENDPOINTS_FILE" | sed 's/,$//')"
cli() { HOME="$H" syncthing cli --home="$H" "$@"; }

# Devices already in the running config (one base32 id per line, incl. self).
CURRENT="$(cli config devices list 2>/dev/null || true)"

# Registry → "host<TAB>device-id@overlay-ip" pairs (clean alternating key/value
# lines from etcdctl, paired by awk — matching setup-syncthing.sh's parser).
ETCDCTL_API=3 etcdctl --endpoints="$EPS" get --prefix /mesh/syncthing/ 2>/dev/null \
  | awk 'NR%2==1{sub(/.*\/mesh\/syncthing\//,"",$0); k=$0; next} {print k"\t"$0}' \
  | while IFS=$'\t' read -r rhost val; do
      [ "$rhost" = "$HOST" ] && continue                 # never re-add ourselves
      dev="${val%@*}"; ip="${val#*@}"
      [ -z "$dev" ] && continue
      printf '%s' "$dev" | grep -qE "$DEV_RE" || continue   # skip a corrupt registry id
      printf '%s\n' "$CURRENT" | grep -qF "$dev" && continue # already present → no-op, no restart
      addr="dynamic"; [ -n "$ip" ] && [ "$ip" != "$dev" ] && addr="tcp://${ip}:22000"
      logger -t syncthing-reconcile "adding mesh peer device $rhost ($dev) addr=$addr"
      cli config devices add --device-id "$dev" --name "$rhost" --addresses "$addr" --compression metadata 2>/dev/null || true
      cli config folders "$FOLDER_ID" devices add --device-id "$dev" 2>/dev/null || true
    done
exit 0
