#!/usr/bin/env bash
# WL-CRIT-007/S2 — event-driven peer recovery after resume/network return.
# This never enrolls, mutates identity, or writes mesh authority state. Recovery
# state is bounded to the existing systemd service status and journal.
set -u

SYSTEMCTL="${MCNF_RECOVERY_SYSTEMCTL:-/usr/bin/systemctl}"
IP="${MCNF_RECOVERY_IP:-/usr/sbin/ip}"
NOTIFY="${MCNF_RECOVERY_NOTIFY:-/usr/bin/systemd-notify}"
SLEEP="${MCNF_RECOVERY_SLEEP:-/usr/bin/sleep}"
NM_ONLINE="${MCNF_RECOVERY_NM_ONLINE:-/usr/bin/nm-online}"
NETWORKCTL="${MCNF_RECOVERY_NETWORKCTL:-/usr/bin/networkctl}"
FLOCK="${MCNF_RECOVERY_FLOCK:-/usr/bin/flock}"
LOCK="${MCNF_RECOVERY_LOCK:-/run/mcnf-peer-recovery/recovery.lock}"
NEBULA_DIR="${MCNF_NEBULA_DIR:-/etc/nebula}"
ROLE_FILE="${MCNF_ROLE_FILE:-/var/lib/mde/role.toml}"
ETCD_MEMBER_FILE="${MCNF_ETCD_MEMBER_FILE:-/etc/etcd/etcd.env}"
SYNCTHING_CONFIG="${MCNF_SYNCTHING_CONFIG:-/etc/systemd/system/syncthing.service.d/10-home.conf}"
MAX_ATTEMPTS="${MCNF_RECOVERY_MAX_ATTEMPTS:-4}"
COMMAND_TIMEOUT="${MCNF_RECOVERY_COMMAND_TIMEOUT:-10}"
GROUP_WAIT_CHECKS="${MCNF_RECOVERY_GROUP_WAIT_CHECKS:-30}"

log() { printf 'mcnf-peer-recovery: %s\n' "$*"; }
publish() {
    local state="${1:0:160}"
    log "state=$state"
    if [ -n "${NOTIFY_SOCKET:-}" ] && [ -x "$NOTIFY" ]; then
        "$NOTIFY" --ready --status="$state" >/dev/null 2>&1 || true
    fi
}
valid_uint() { case "$1" in ''|*[!0-9]*) return 1;; *) [ "$1" -gt 0 ];; esac; }
bounded_systemctl() { /usr/bin/timeout "$COMMAND_TIMEOUT" "$SYSTEMCTL" "$@"; }

admitted_role() {
    [ "$(/usr/bin/grep -Ec '^[[:space:]]*role[[:space:]]*=' "$ROLE_FILE")" -eq 1 ] \
        || return 1
    if /usr/bin/grep -Eq \
        '^[[:space:]]*role[[:space:]]*=[[:space:]]*("workstation"|workstation)[[:space:]]*$' \
        "$ROLE_FILE"; then
        printf '%s\n' workstation
    elif /usr/bin/grep -Eq \
        '^[[:space:]]*role[[:space:]]*=[[:space:]]*("lighthouse"|lighthouse)[[:space:]]*$' \
        "$ROLE_FILE"; then
        printf '%s\n' lighthouse
    else
        return 1
    fi
}

physical_network_online() {
    if [ -x "$NM_ONLINE" ] \
        && bounded_systemctl is-active --quiet NetworkManager.service >/dev/null 2>&1; then
        /usr/bin/timeout 3 "$NM_ONLINE" -x -q --timeout=1 >/dev/null 2>&1
    elif [ -x "$NETWORKCTL" ] \
        && bounded_systemctl is-active --quiet systemd-networkd.service >/dev/null 2>&1; then
        /usr/bin/timeout 3 "$NETWORKCTL" is-online --quiet >/dev/null 2>&1
    else
        # No approved network manager can attest readiness. A default route can
        # remain cached across link loss, so fail closed instead of mutating.
        return 1
    fi
}

nebula_ready() {
    bounded_systemctl is-active --quiet nebula.service >/dev/null 2>&1 \
        && /usr/bin/timeout 3 "$IP" -4 -o address show dev nebula1 scope global 2>/dev/null \
            | /usr/bin/grep -q ' inet '
}

wait_nebula_ready() {
    local checks=0
    while [ "$checks" -lt 5 ]; do
        nebula_ready && return 0
        checks=$((checks + 1))
        [ "$checks" -lt 5 ] && "$SLEEP" 1
    done
    return 1
}

wait_active() {
    local unit="$1" checks=0
    while [ "$checks" -lt 5 ]; do
        if bounded_systemctl is-active --quiet "$unit" >/dev/null 2>&1; then
            return 0
        fi
        checks=$((checks + 1))
        "$SLEEP" 1
    done
    return 1
}

grouped_mackesd_ready() {
    local unit
    for unit in control observation actions data compute integrations; do
        bounded_systemctl is-active --quiet "mackesd-$unit.service" >/dev/null 2>&1 || return 1
    done
}

wait_grouped_mackesd_ready() {
    local checks=0
    while [ "$checks" -lt "$GROUP_WAIT_CHECKS" ]; do
        grouped_mackesd_ready && return 0
        checks=$((checks + 1))
        [ "$checks" -lt "$GROUP_WAIT_CHECKS" ] && "$SLEEP" 1
    done
    return 1
}

start_grouped_mackesd_without_disruption() {
    local unit
    # Recovery is additive. `restart mackesd.target` begins by stopping every
    # PartOf child and, when queued with --no-block, can make a transiently
    # active child set look recovered while that stop transaction is still
    # draining. Start the target, then explicitly start only missing groups.
    bounded_systemctl --no-block start mackesd.target >/dev/null 2>&1 || return 1
    for unit in control observation actions data compute integrations; do
        if ! bounded_systemctl is-active --quiet "mackesd-$unit.service" >/dev/null 2>&1; then
            bounded_systemctl --no-block start "mackesd-$unit.service" >/dev/null 2>&1 \
                || return 1
        fi
    done
}

grouped_mackesd_target_starting() {
    local state
    state="$(bounded_systemctl show mackesd.target -p ActiveState --value 2>/dev/null)" \
        || return 1
    [ "$state" = activating ]
}

restore_xdg_binds() {
    bounded_systemctl start mcnf-xdg-bind-recovery.service >/dev/null 2>&1
}

restore_role_desktop_state() {
    local role="$1"
    if [ "$role" = lighthouse ]; then
        publish "skipped-workstation-xdg-lighthouse"
        return 0
    fi
    publish "restoring-workstation-xdg-binds"
    if ! restore_xdg_binds; then
        publish "failed-workstation-xdg-binds"
        return 1
    fi
}

configured_substrate_ready() {
    if [ -s "$ETCD_MEMBER_FILE" ]; then
        bounded_systemctl is-active --quiet etcd.service >/dev/null 2>&1 || return 1
    fi
    if [ -f "$SYNCTHING_CONFIG" ]; then
        bounded_systemctl is-active --quiet syncthing.service >/dev/null 2>&1 || return 1
    fi
}

restore_configured_service() {
    local unit="$1" label="$2"
    # A boot-time network-return event can race the unit's ordinary boot
    # activation.  Restarting an already-active Syncthing here asks systemd to
    # stop a process that may still be finishing its first hash scan; the
    # bounded restart can then time out with the unit stranded in
    # `deactivating`.  Active is the same readiness contract used by
    # configured_substrate_ready(), so preserve that healthy process.  An
    # inactive configured substrate needs a bounded start, not a stop/start.
    if bounded_systemctl is-active --quiet "$unit" >/dev/null 2>&1; then
        publish "$label-already-ready"
        return 0
    fi
    publish "restoring-$label"
    if ! bounded_systemctl start "$unit" >/dev/null 2>&1 || ! wait_active "$unit"; then
        log "WARN: $unit did not recover"
        return 1
    fi
}

main() {
    local attempt=1 delay=1 role
    [ "$(id -u)" -eq 0 ] || { publish "refused-not-root"; return 1; }
    valid_uint "$MAX_ATTEMPTS" && [ "$MAX_ATTEMPTS" -le 6 ] \
        || { publish "refused-invalid-attempt-bound"; return 2; }
    valid_uint "$COMMAND_TIMEOUT" && [ "$COMMAND_TIMEOUT" -le 30 ] \
        || { publish "refused-invalid-timeout-bound"; return 2; }
    valid_uint "$GROUP_WAIT_CHECKS" && [ "$GROUP_WAIT_CHECKS" -le 30 ] \
        || { publish "refused-invalid-group-wait-bound"; return 2; }

    if [ ! -f "$NEBULA_DIR/host.crt" ] && [ ! -f "$NEBULA_DIR/identity/current/host.crt" ]; then
        publish "refused-not-enrolled"
        return 0
    fi
    [ -f "$ROLE_FILE" ] || { publish "refused-no-role"; return 0; }
    role="$(admitted_role)" \
        || { publish "refused-invalid-role"; return 2; }
    # Lighthouses are coordination members, never client-only peers. Missing
    # member configuration must stop recovery before network or service
    # mutation instead of taking the Workstation's intentional etcd-skip path.
    if [ "$role" = lighthouse ] && [ ! -s "$ETCD_MEMBER_FILE" ]; then
        publish "refused-lighthouse-etcd-unconfigured"
        return 2
    fi
    if ! physical_network_online; then
        publish "offline-no-mutation"
        return 0
    fi

    /usr/bin/install -d -m 0700 -- "${LOCK%/*}"
    exec 9>"$LOCK"
    if ! "$FLOCK" -n 9; then
        publish "coalesced-recovery-already-running"
        return 0
    fi

    # The first network check admits the event, but the link can disappear
    # while another recovery still holds the lock. Re-attest after acquiring
    # the single-flight lease so a stale positive event cannot trigger a
    # Nebula restart or downstream service mutation.
    if ! physical_network_online; then
        publish "offline-during-recovery"
        return 0
    fi

    # NetworkManager may emit several positive return events for one physical
    # recovery (link, DHCP, connectivity, reapply). Never restart an already
    # healthy overlay for each event: doing so can exhaust nebula.service's
    # bounded start limit and turn a healthy return into an outage. The XDG
    # helper remains an idempotent verification/repair step for Workstations;
    # Lighthouses must not start that role-inapplicable unit.
    if nebula_ready && configured_substrate_ready && grouped_mackesd_ready; then
        restore_role_desktop_state "$role" || return 1
        publish "already-recovered"
        return 0
    fi

    if nebula_ready; then
        publish "nebula-already-ready"
    else
        while [ "$attempt" -le "$MAX_ATTEMPTS" ]; do
            publish "restoring-nebula-attempt-$attempt-of-$MAX_ATTEMPTS"
            bounded_systemctl restart nebula.service >/dev/null 2>&1 || true
            # systemctl can report the process active before the TUN address is
            # materialized. Poll that exact readiness before another restart.
            if wait_nebula_ready; then
                break
            fi
            if [ "$attempt" -eq "$MAX_ATTEMPTS" ]; then
                publish "failed-nebula-unavailable"
                return 1
            fi
            "$SLEEP" "$delay"
            delay=$((delay * 2))
            attempt=$((attempt + 1))
        done
    fi

    # Nebula's TUN address is an overlay-readiness signal, not proof that the
    # physical link is still usable. A resume/network-return event can lose
    # the link while the bounded overlay wait is running; do not start or
    # otherwise mutate configured substrate services from that stale result.
    if ! physical_network_online; then
        publish "offline-after-nebula"
        return 0
    fi

    if [ -s "$ETCD_MEMBER_FILE" ]; then
        if ! restore_configured_service etcd.service etcd; then
            publish "failed-configured-etcd"
            return 1
        fi
    else
        publish "skipped-etcd-client-only"
    fi
    if [ -f "$SYNCTHING_CONFIG" ]; then
        if ! restore_configured_service syncthing.service syncthing; then
            publish "failed-configured-syncthing"
            return 1
        fi
    else
        publish "skipped-syncthing-unconfigured"
    fi

    publish "restoring-grouped-mackesd"
    # During boot the target can already be activating its six notify children;
    # let that bounded job settle. If it does not settle, perform additive
    # recovery only: start the target and any missing group without stopping a
    # healthy process. Poll the exact child readiness after those start jobs.
    if grouped_mackesd_target_starting && wait_grouped_mackesd_ready; then
        :
    elif ! start_grouped_mackesd_without_disruption \
        || ! wait_grouped_mackesd_ready; then
        publish "failed-grouped-mackesd"
        return 1
    fi
    # Group readiness can take long enough for the physical link to disappear
    # after the substrate gate above. Do not let that stale recovery continue
    # into the final desktop mutation or claim a complete peer return.
    if ! physical_network_online; then
        publish "offline-before-desktop"
        return 0
    fi
    # Desktop bind repair is the final local mutation.  Keep it behind the
    # grouped readiness gate so a failed daemon/session recovery cannot report
    # or partially apply a healthy desktop state.
    restore_role_desktop_state "$role" || return 1
    publish "recovered"
}

main "$@"
