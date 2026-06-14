#!/bin/bash
# mesh-alert.sh <source> [severity] [summary] — emit a Magic Mesh alert when a
# core service goes offline (or any node-level event worth surfacing).
#
# Rides the platform's existing alert path: drops an AlertEvent JSON into the
# alert_relay spool (`$XDG_DATA_HOME/mde/alerts/`), which mackesd's alert_relay
# worker relays to the desktop (FDO notify-send via the Bus / cosmic-applet) and
# any configured [[alert_hooks]] (webhook/email). Also logs to the journal at
# the matching priority (always works — remotely greppable on headless nodes)
# and broadcasts to logged-in terminals.
#
# Invoked by:
#   * systemd `OnFailure=mesh-alert@%n.service` on mackesd/nebula (once per
#     failure transition);
#   * the mesh-health watchdog when it recovers a wedged service.
set -u

SRC="${1:-mesh}"
SEV="${2:-crit}"                 # crit | warn | info
SUMMARY="${3:-$SRC is offline}"
HOST="$(hostname 2>/dev/null || echo node)"

DIR="${MDE_ALERTS_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/mde/alerts}"
if ! mkdir -p "$DIR" 2>/dev/null; then DIR=/tmp/mde-alerts; mkdir -p "$DIR"; fi

SAFE="$(printf '%s' "$SRC" | tr -c 'a-zA-Z0-9' '_')"
ID="mesh-${SAFE}-$(date +%s)-$$"
TMP="$DIR/.${ID}.json.tmp"
# Atomic publish (alert_relay skips *.json.tmp, consumes *.json).
printf '{"id":"%s","severity":"%s","alert":"service.%s","host":"%s","summary":"%s","chart_url":""}\n' \
    "$ID" "$SEV" "$SAFE" "$HOST" "$SUMMARY" > "$TMP"
mv "$TMP" "$DIR/${ID}.json"

case "$SEV" in
    crit) PRI=crit;;
    warn) PRI=warning;;
    *)    PRI=info;;
esac
logger -t mesh-alert -p "daemon.$PRI" "$SUMMARY" 2>/dev/null || true
if command -v wall >/dev/null 2>&1; then printf '[mesh-alert] %s\n' "$SUMMARY" | wall 2>/dev/null || true; fi
exit 0
