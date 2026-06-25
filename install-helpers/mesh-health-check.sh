#!/bin/bash
# mesh-health-check.sh — the MCNF node health watchdog + recovery.
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

# 0. Shared-state plane health. SUBSTRATE-V2: the plane is etcd (coordination)
#    + Syncthing (files). When this node is on the etcd coordination plane
#    (setup-etcd wrote the endpoints file), assert etcd quorum health + the
#    Syncthing daemon.
ETCD_ENDPOINTS_FILE=/etc/mackesd/etcd-endpoints
QNM="${MDE_WORKGROUP_ROOT:-${QNM_PATH:-/mnt/mesh-storage}}"
if [ -s "$ETCD_ENDPOINTS_FILE" ]; then
    # etcd coordination plane: quorum health (any reachable client endpoint).
    EPS="$(tr '\n' ',' < "$ETCD_ENDPOINTS_FILE" | sed 's/,$//')"
    if command -v etcdctl >/dev/null 2>&1; then
        if ! ETCDCTL_API=3 etcdctl --endpoints="$EPS" endpoint health >/dev/null 2>&1; then
            restart etcd.service "etcd unreachable (coordination plane down)"
        fi
    fi
    # Syncthing file plane (non-critical to liveness, but recover + note it).
    if systemctl list-unit-files syncthing.service >/dev/null 2>&1 \
       && ! systemctl is-active --quiet syncthing.service 2>/dev/null; then
        restart syncthing.service "Syncthing down (Mesh Sync file plane out of sync)"
    fi
    # SUBSTRATE-10: a syncthing that is UP but not actually CONNECTED to its
    # configured peers is silently OUT OF SYNC — service-active isn't enough.
    # This is the exact failure the reconciler addresses (a peer device-id not
    # yet wired → "unknown device" rejection, syncthing up but no connection) and
    # also catches an overlay partition. Compare configured peer devices (minus
    # self) to live connections and alert if short, so a stuck file plane is
    # visible instead of silently diverging.
    if systemctl is-active --quiet syncthing.service 2>/dev/null && command -v syncthing >/dev/null 2>&1; then
        ST_HOME="${MCNF_SYNCTHING_HOME:-/var/lib/mcnf-syncthing}"
        st_peers=$(( $(HOME="$ST_HOME" syncthing cli --home="$ST_HOME" config devices list 2>/dev/null | grep -c .) - 1 ))
        st_conn=$(HOME="$ST_HOME" syncthing cli --home="$ST_HOME" show connections 2>/dev/null | grep -c '"connected": true')
        if [ "${st_peers:-0}" -gt 0 ] && [ "${st_conn:-0}" -lt "$st_peers" ]; then
            alert "syncthing-out-of-sync" "Mesh Sync OUT OF SYNC on $(hostname): ${st_conn}/${st_peers} peer device(s) connected (reconcile pending or overlay partition)"
        fi
    fi
fi

# 0b. BUS-RETENTION-2 — /run headroom guard. The message bus spool lives on /run
#     (tmpfs); a full /run breaks runtime locks — dnf AND the bus index WAL (so
#     the bus's own GC can no longer delete rows). This is the failure class that
#     blocked the v10.0.18 fleet roll. mackesd's in-process GC also raises a Hub
#     alert, but flag it here too since the watchdog runs even if mackesd is wedged.
RUN_AVAIL=$(df -B1 --output=avail /run 2>/dev/null | tail -1 | tr -d ' ')
RUN_TOTAL=$(df -B1 --output=size  /run 2>/dev/null | tail -1 | tr -d ' ')
if [ -n "${RUN_AVAIL:-}" ] && [ -n "${RUN_TOTAL:-}" ] && [ "$RUN_TOTAL" -gt 0 ]; then
    RUN_PCT=$(( RUN_AVAIL * 100 / RUN_TOTAL ))
    if [ "$RUN_PCT" -lt 15 ]; then
        log "WARN: /run low — ${RUN_PCT}% free ($(( RUN_AVAIL/1024/1024 ))MB of $(( RUN_TOTAL/1024/1024 ))MB); bus/dnf locks at risk"
        alert "run-low" "/run at ${RUN_PCT}% free on $(hostname) — bus + dnf runtime locks at risk"
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
