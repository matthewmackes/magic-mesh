#!/bin/bash
# mesh-health-check.sh — the Magic Mesh node health watchdog + recovery.
#
# Driven by mesh-health.timer (~every 60s). systemd's own `Restart=on-failure`
# already recovers a CRASHED mackesd/nebula; this catches the cases systemd
# can't see on its own:
#   * a unit that exhausted its StartLimit and gave up → kick it back to life;
#   * nebula running but the `nebula1` overlay interface is gone;
#   * a peer whose tunnel has wedged (iface up, but the lighthouse is
#     unreachable over the overlay) → bounce nebula so it re-handshakes.
#
# Fail-safe: it only acts on an ENROLLED node (one with /etc/nebula/host.crt).
# An un-enrolled or role-less box is left alone (mackesd fails closed on
# purpose there — see the unit's ENT-2 note). All actions are logged to the
# journal so `journalctl -u mesh-health` shows what recovered and why.
set -u

ETC_NEBULA="/etc/nebula"
log() { echo "mesh-health: $*"; }       # journal via the unit's StandardOutput

# Only manage a node that has actually been enrolled.
[ -f "$ETC_NEBULA/host.crt" ] || { log "node not enrolled (no host.crt); nothing to manage"; exit 0; }
# A role must be pinned, else mackesd fails closed by design — don't fight it.
[ -f /var/lib/mde/role.toml ] || { log "no role pinned; leaving services alone"; exit 0; }

MESH_ALERT_BIN="${MESH_ALERT_BIN:-/usr/libexec/mackesd/mesh-alert}"

# Notify (throttled to once / 10 min per unit so a persistent fault doesn't
# spam) that the watchdog had to act. systemd's OnFailure= covers clean
# crashes; this covers the wedged-but-not-failed cases the watchdog catches.
alert() {
    local stamp="/run/mesh-health/$(printf '%s' "$1" | tr -c 'a-zA-Z0-9' '_').alerted"
    mkdir -p /run/mesh-health 2>/dev/null
    if [ -z "$(find "$stamp" -newermt '-10 minutes' 2>/dev/null)" ]; then
        [ -x "$MESH_ALERT_BIN" ] && "$MESH_ALERT_BIN" "$1" crit "watchdog recovering $1 on $(hostname): $2" || true
        : > "$stamp"
    fi
}

restart() {
    log "RECOVER: restarting $1 ($2)"
    alert "$1" "$2"
    systemctl restart "$1" >/dev/null 2>&1 || log "  restart $1 failed"
}

# 0. QNM-Shared must be a REAL mount, not a silently-local directory (ONBOARD-6
#    process fix #3 — the exact failure that hid for the whole project: the
#    shared-state code works identically against a local dir, so a missing mount
#    no-ops silently → NO LEADER / empty directory). If qnm-shared.service exists
#    but the volume isn't a fuse mount, recover it + alert loudly.
# AUDIT-MESH-1 — assert the SAME root the daemon uses. mackesd runs with
# MDE_WORKGROUP_ROOT=/mnt/mesh-storage (systemd unit + env.d drop-in), and
# setup-qnm-shared.sh mounts there; the old /root/QNM-Shared default checked the
# wrong path, so a failed /mnt/mesh-storage mount slipped past the watchdog.
QNM="${MDE_WORKGROUP_ROOT:-${QNM_PATH:-/mnt/mesh-storage}}"
if systemctl list-unit-files qnm-shared.service >/dev/null 2>&1 && [ -d "$QNM" ]; then
    if ! mount 2>/dev/null | grep -q " $QNM type fuse"; then
        restart qnm-shared.service "QNM-Shared not mounted (shared-state plane down)"
    fi
fi

# 1. The worker daemon must be active. If it stopped (incl. StartLimit
#    exhaustion → 'failed'), restart resets the counter and revives it.
if ! systemctl is-active --quiet mackesd.service; then
    restart mackesd.service "not active"
fi

# 2. nebula must be active AND own the overlay interface.
if ! systemctl is-active --quiet nebula.service; then
    restart nebula.service "not active"
elif ! ip -o link show nebula1 >/dev/null 2>&1; then
    restart nebula.service "nebula1 interface missing"
else
    # 3. Overlay liveness — a peer must be able to reach a lighthouse over the
    #    overlay. Skip on the lighthouse itself (am_lighthouse: true). Ping the
    #    configured lighthouse overlay IP(s); restart nebula only on TOTAL loss
    #    (transient drops don't count) to re-establish a wedged tunnel.
    if grep -q "am_lighthouse: false" "$ETC_NEBULA/config.yaml" 2>/dev/null; then
        mapfile -t LH < <(sed -n '/^lighthouse:/,/^[^[:space:]]/p' "$ETC_NEBULA/config.yaml" 2>/dev/null \
            | grep -oE '"10\.[0-9.]+"' | tr -d '"')
        reachable=0
        for ip in "${LH[@]}"; do
            if ping -c 3 -W 2 "$ip" >/dev/null 2>&1; then reachable=1; break; fi
        done
        if [ "${#LH[@]}" -gt 0 ] && [ "$reachable" -eq 0 ]; then
            restart nebula.service "overlay unreachable: no lighthouse answered"
        fi
    fi
fi

log "ok"
exit 0
