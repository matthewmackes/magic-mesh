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
# Fail-safe: it only acts on an ENROLLED node.  Newer enrollment stores the
# active identity under identity/current; retain the flat path for older nodes.
# The old flat-only check made the watchdog silently exit on migrated laptops,
# exactly when suspend/resume most needs the overlay recovery path.
# An un-enrolled or role-less box is left alone (mackesd fails closed on
# purpose there — see the unit's ENT-2 note). All actions are logged to the
# journal so `journalctl -u mesh-health` shows what recovered and why.
set -u

ETC_NEBULA="${MCNF_NEBULA_DIR:-/etc/nebula}"
ROLE_FILE="${MCNF_ROLE_FILE:-/var/lib/mde/role.toml}"
HEALTH_RUN_DIR="${MCNF_HEALTH_RUN_DIR:-/run/mesh-health}"
NEBULA_UNREACHABLE_RESTART_STAMP="${MCNF_NEBULA_UNREACHABLE_RESTART_STAMP:-$HEALTH_RUN_DIR/nebula-unreachable.restarted}"
NEBULA_UNREACHABLE_RESTART_COOLDOWN_S="${MCNF_NEBULA_UNREACHABLE_RESTART_COOLDOWN_S:-600}"
log() { echo "mesh-health: $*"; }       # journal via the unit's StandardOutput

case "$NEBULA_UNREACHABLE_RESTART_COOLDOWN_S" in
    ''|*[!0-9]*) NEBULA_UNREACHABLE_RESTART_COOLDOWN_S=600 ;;
esac
if [ "$NEBULA_UNREACHABLE_RESTART_COOLDOWN_S" -lt 60 ]; then
    NEBULA_UNREACHABLE_RESTART_COOLDOWN_S=600
fi

# Only manage a node that has actually been enrolled.
if [ ! -f "$ETC_NEBULA/host.crt" ] &&
   [ ! -f "$ETC_NEBULA/identity/current/host.crt" ]; then
    log "node not enrolled (no active host certificate); nothing to manage"
    exit 0
fi
# A role must be pinned, else mackesd fails closed by design — don't fight it.
[ -f "$ROLE_FILE" ] || { log "no role pinned; leaving services alone"; exit 0; }

MESH_ALERT_BIN="${MESH_ALERT_BIN:-/usr/libexec/mackesd/mesh-alert}"

# Notify (throttled to once / 10 min per unit so a persistent fault doesn't
# spam) that the watchdog had to act. systemd's OnFailure= covers clean
# crashes; this covers the wedged-but-not-failed cases the watchdog catches.
alert() {
    local stamp
    stamp="$HEALTH_RUN_DIR/$(printf '%s' "$1" | tr -c 'a-zA-Z0-9' '_').alerted"
    mkdir -p "$HEALTH_RUN_DIR" 2>/dev/null
    if [ -z "$(find "$stamp" -newermt '-10 minutes' 2>/dev/null)" ]; then
        if [ -x "$MESH_ALERT_BIN" ]; then
            "$MESH_ALERT_BIN" "$1" crit "watchdog recovering $1 on $(hostname): $2" || true
        fi
        : > "$stamp"
    fi
}

restart() {
    log "RECOVER: restarting $1 ($2)"
    alert "$1" "$2"
    systemctl restart "$1" >/dev/null 2>&1 || log "  restart $1 failed"
}

MACKESD_GROUP_UNITS=(
    mackesd-control.service
    mackesd-observation.service
    mackesd-actions.service
    mackesd-data.service
    mackesd-compute.service
    mackesd-integrations.service
)

restore_grouped_mackesd_after_nebula_restart() {
    local unit
    # mackesd.target and mackesd-control.service Require=nebula.service. A
    # watchdog restart therefore tears down the grouped daemon even though the
    # fault was in the overlay. Queue an additive target start and only the
    # missing children; never restart a group that survived the transaction.
    log "RECOVER: restoring grouped mackesd after nebula restart"
    systemctl --no-block start mackesd.target >/dev/null 2>&1 \
        || log "  start mackesd.target failed"
    for unit in "${MACKESD_GROUP_UNITS[@]}"; do
        if ! systemctl is-active --quiet "$unit" >/dev/null 2>&1; then
            systemctl --no-block start "$unit" >/dev/null 2>&1 \
                || log "  start $unit failed"
        fi
    done
}

restart_nebula_and_restore_groups() {
    local reason="$1"
    log "RECOVER: restarting nebula.service ($reason)"
    alert nebula.service "$reason"
    if systemctl restart nebula.service >/dev/null 2>&1; then
        restore_grouped_mackesd_after_nebula_restart
    else
        log "  restart nebula.service failed"
    fi
}

nebula_unreachable_restart_due() {
    local now stamp_mtime
    now="$(date +%s 2>/dev/null || true)"
    stamp_mtime="$(stat -c %Y "$NEBULA_UNREACHABLE_RESTART_STAMP" 2>/dev/null || true)"
    [ -z "$now" ] || [ -z "$stamp_mtime" ] \
        || [ "$((now - stamp_mtime))" -ge "$NEBULA_UNREACHABLE_RESTART_COOLDOWN_S" ]
}

record_nebula_unreachable_restart() {
    mkdir -p "$HEALTH_RUN_DIR" 2>/dev/null || true
    : > "$NEBULA_UNREACHABLE_RESTART_STAMP"
}

# 0. Shared-state plane health. SUBSTRATE-V2: the plane is etcd (coordination)
#    + Syncthing (files). When this node is on the etcd coordination plane
#    (setup-etcd wrote the endpoints file), assert etcd quorum health + the
#    Syncthing daemon.
ETCD_ENDPOINTS_FILE="${MCNF_ETCD_ENDPOINTS_FILE:-/etc/mackesd/etcd-endpoints}"
ETCD_MEMBER_FILE="${MCNF_ETCD_MEMBER_FILE:-/etc/etcd/etcd.env}"
SYNCTHING_FOLDER_ID="${MCNF_SYNCTHING_FOLDER_ID:-mcnf-mesh}"
PEER_PUBLICATION_STAMP="${MCNF_PEER_PUBLICATION_STAMP:-$HEALTH_RUN_DIR/peer-publication.ok}"
PEER_PUBLICATION_MAX_AGE_S="${MCNF_PEER_PUBLICATION_MAX_AGE_S:-120}"
publication_failed=0
coordination_failed=0
if [ -s "$ETCD_ENDPOINTS_FILE" ]; then
    # etcd coordination plane: quorum health (any reachable client endpoint).
    EPS="$(tr '\n' ',' < "$ETCD_ENDPOINTS_FILE" | sed 's/,$//')"
    if command -v etcdctl >/dev/null 2>&1; then
        # Do not ask etcdctl to check the whole comma-separated set here.  That
        # command fails while a member is still joining (or during a transient
        # overlay flap), even when this node can already reach a healthy member;
        # restarting the local daemon in that window prevents the quorum from
        # ever converging.  The watchdog only needs one coordination endpoint
        # to be alive before it can safely leave the local etcd process alone.
        healthy_endpoint=0
        IFS=',' read -r -a etcd_endpoints <<<"$EPS"
        for endpoint in "${etcd_endpoints[@]}"; do
            endpoint="$(printf '%s' "$endpoint" | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')"
            [ -n "$endpoint" ] || continue
            if ETCDCTL_API=3 etcdctl --command-timeout=5s --endpoints="$endpoint" endpoint health >/dev/null 2>&1; then
                healthy_endpoint=1
                break
            fi
        done
        if [ "$healthy_endpoint" -eq 0 ]; then
            coordination_failed=1
            if [ -s "$ETCD_MEMBER_FILE" ]; then
                restart etcd.service "etcd unreachable (coordination plane down)"
            else
                # Workstations are coordination clients, not local members.
                # Restarting their condition-skipped etcd.service cannot heal a
                # remote quorum and creates false recovery churn every minute.
                log "DEGRADED: coordination endpoints unreachable; client-only node has no local etcd member to restart"
                alert etcd-client "coordination endpoints unreachable; no local member to restart"
            fi
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
    # also catches an overlay partition. Compare ONLY devices shared on the
    # managed folder (minus self) with the connected-device map. Unrelated global
    # devices are valid Syncthing state and must not inflate either count.
    if systemctl is-active --quiet syncthing.service 2>/dev/null && command -v syncthing >/dev/null 2>&1; then
        ST_HOME="${MCNF_SYNCTHING_HOME:-/var/lib/mcnf-syncthing}"
        st_folder_devices="$(HOME="$ST_HOME" syncthing cli --home="$ST_HOME" config folders "$SYNCTHING_FOLDER_ID" devices list 2>/dev/null || true)"
        st_system="$(HOME="$ST_HOME" syncthing cli --home="$ST_HOME" show system 2>/dev/null || true)"
        st_connections="$(HOME="$ST_HOME" syncthing cli --home="$ST_HOME" show connections 2>/dev/null || true)"
        if st_counts="$(ST_FOLDER_DEVICES="$st_folder_devices" ST_SYSTEM="$st_system" ST_CONNECTIONS="$st_connections" python3 - <<'PY'
import json
import os
import re
import sys

device_re = re.compile(r'^[A-Z2-7]{7}(?:-[A-Z2-7]{7}){7}$')
folder_lines = [line.strip() for line in os.environ['ST_FOLDER_DEVICES'].splitlines() if line.strip()]
if not folder_lines or any(not device_re.fullmatch(device) for device in folder_lines):
    raise SystemExit(1)

try:
    system = json.loads(os.environ['ST_SYSTEM'])
    connection_document = json.loads(os.environ['ST_CONNECTIONS'])
except (KeyError, json.JSONDecodeError):
    raise SystemExit(1)

self_id = system.get('myID')
connections = connection_document.get('connections')
if not isinstance(self_id, str) or not device_re.fullmatch(self_id) or not isinstance(connections, dict):
    raise SystemExit(1)

folder_peers = set(folder_lines)
folder_peers.discard(self_id)
connected = {
    device_id
    for device_id, state in connections.items()
    if isinstance(state, dict) and state.get('connected') is True
}
print(len(folder_peers & connected), len(folder_peers))
PY
)"; then
            read -r st_conn st_peers <<<"$st_counts"
            if [ "${st_peers:-0}" -gt 0 ] && [ "${st_conn:-0}" -lt "$st_peers" ]; then
                alert "syncthing-out-of-sync" "Mesh Sync OUT OF SYNC on $(hostname): ${st_conn}/${st_peers} managed-folder peer device(s) connected (reconcile pending or overlay partition)"
            fi
        else
            log "WARN: unable to evaluate Syncthing folder-scoped connections for $SYNCTHING_FOLDER_ID"
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

# 1. Every independently supervised worker group must be active. Checking only
#    mackesd.target would miss a group that failed after the target started.
for mackesd_group_unit in "${MACKESD_GROUP_UNITS[@]}"; do
    if ! systemctl is-active --quiet "$mackesd_group_unit"; then
        restart "$mackesd_group_unit" "process group not active"
    fi
done

# A running heartbeat worker is not healthy unless its lease-backed own-row
# transaction is actually committing.  The previous watchdog accepted one
# reachable etcd endpoint and therefore reported `ok` while a non-committing
# first endpoint made every peer publication fail.  The heartbeat refreshes
# this stamp only after the peer row + overlay claim transaction succeeds.
publication_now="$(date +%s 2>/dev/null || true)"
publication_mtime="$(stat -c %Y "$PEER_PUBLICATION_STAMP" 2>/dev/null || true)"
if [ -s "$ETCD_ENDPOINTS_FILE" ] && {
    [ -z "$publication_now" ] || [ -z "$publication_mtime" ] \
        || [ "$((publication_now - publication_mtime))" -gt "$PEER_PUBLICATION_MAX_AGE_S" ];
}; then
    publication_failed=1
    restart mackesd-observation.service \
        "own peer publication missing or stale (lease-backed directory transaction not committing)"
fi

# 2. nebula must be active AND own the overlay interface.
overlay_failed=0
if ! systemctl is-active --quiet nebula.service; then
    restart_nebula_and_restore_groups "not active"
elif ! ip -o link show nebula1 >/dev/null 2>&1; then
    restart_nebula_and_restore_groups "nebula1 interface missing"
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
            overlay_failed=1
            if nebula_unreachable_restart_due; then
                # Record before the restart so a failed or timed-out restart
                # cannot be retried every minute and collapse Requires=
                # dependants indefinitely.
                record_nebula_unreachable_restart
                restart_nebula_and_restore_groups \
                    "overlay unreachable: no lighthouse answered"
            else
                log "DEGRADED: overlay unreachable; nebula restart suppressed by ${NEBULA_UNREACHABLE_RESTART_COOLDOWN_S}s cooldown"
                alert nebula.service \
                    "overlay unreachable; repeated restart suppressed"
            fi
        else
            rm -f -- "$NEBULA_UNREACHABLE_RESTART_STAMP"
        fi
    fi
fi

if [ "$publication_failed" -ne 0 ] || [ "$overlay_failed" -ne 0 ] \
   || [ "$coordination_failed" -ne 0 ]; then
    if [ "$publication_failed" -ne 0 ]; then
        log "DEGRADED: own peer publication is stale; recovery requested"
    fi
    exit 1
fi
log "ok"
exit 0
