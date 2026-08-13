#!/usr/bin/env bash
# Operational fixtures for WL-CRIT-007/S2: offline refusal, bounded online
# recovery order/backoff, trigger filtering, and single-flight coalescing.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HELPER="$REPO/install-helpers/mesh-peer-recovery.sh"
SLEEP_HOOK="$REPO/packaging/systemd/mcnf-peer-recovery-sleep"
NETWORK_HOOK="$REPO/packaging/systemd/90-mcnf-peer-recovery"
[ "$(id -u)" -eq 0 ] || { echo 'run fixture as root' >&2; exit 1; }

ROOT="$(mktemp -d /tmp/mcnf-peer-recovery-test.XXXXXX)"
trap 'rm -rf -- "$ROOT"' EXIT
STATE="$ROOT/state"
BIN="$ROOT/bin"
mkdir -p "$STATE" "$BIN" "$ROOT/nebula"
printf '%s\n' cert >"$ROOT/nebula/host.crt"
printf '%s\n' 'role = "workstation"' >"$ROOT/role.toml"
printf '%s\n' member >"$ROOT/etcd.env"
printf '%s\n' configured >"$ROOT/syncthing.conf"
: >"$STATE/mutations"
: >"$STATE/restarts"
: >"$STATE/notifies"
: >"$STATE/sleeps"
: >"$STATE/active-mde-shell-egui.service"

cat >"$BIN/systemctl" <<'SH'
#!/bin/sh
set -eu
state=${MCNF_TEST_STATE:?}
if [ "${1:-}" = --no-block ]; then
    shift
fi
case "$1" in
    restart)
        unit=$2
        printf '%s\n' "$unit" >>"$state/mutations"
        printf '%s\n' "$unit" >>"$state/restarts"
        if [ "$unit" = nebula.service ]; then
            count=$(cat "$state/nebula-attempts" 2>/dev/null || printf 0)
            count=$((count + 1))
            printf '%s\n' "$count" >"$state/nebula-attempts"
        elif [ "$unit" = mackesd.target ]; then
            if [ -f "$state/delay-groups" ]; then
                : >"$state/pending-groups"
            else
                for group in control observation actions data compute integrations; do
                    : >"$state/active-mackesd-$group.service"
                done
            fi
        else
            : >"$state/active-$unit"
        fi
        ;;
    is-active)
        [ "$2" = --quiet ] && unit=$3 || unit=$2
        if [ "$unit" = NetworkManager.service ]; then
            test -f "$state/online"
            exit $?
        fi
        if [ "$unit" = nebula.service ] && [ -f "$state/force-nebula-inactive" ]; then
            exit 3
        fi
        if [ "$unit" = etcd.service ] && [ -f "$state/drop-etcd-after-etcd-start-armed" ]; then
            checks=$(cat "$state/etcd-post-start-checks" 2>/dev/null || printf 0)
            checks=$((checks + 1))
            printf '%s\n' "$checks" >"$state/etcd-post-start-checks"
            if [ "$checks" -ge 2 ]; then
                rm -f "$state/drop-etcd-after-etcd-start-armed" \
                    "$state/active-etcd.service"
            fi
        fi
        if [ "$unit" = nebula.service ] && [ -s "$state/nebula-attempts" ]; then
            checks=$(cat "$state/nebula-ready-checks" 2>/dev/null || printf 0)
            checks=$((checks + 1))
            printf '%s\n' "$checks" >"$state/nebula-ready-checks"
            [ "$checks" -ge 3 ] && : >"$state/active-nebula.service"
        fi
        case "$unit" in
            mackesd-*.service)
                if [ -f "$state/pending-groups" ]; then
                    checks=$(cat "$state/group-ready-checks" 2>/dev/null || printf 0)
                    checks=$((checks + 1))
                    printf '%s\n' "$checks" >"$state/group-ready-checks"
                    if [ "$checks" -ge 3 ]; then
                        for group in control observation actions data compute integrations; do
                            : >"$state/active-mackesd-$group.service"
                        done
                        rm -f "$state/pending-groups"
                        if [ -f "$state/drop-after-groups-ready" ]; then
                            rm -f "$state/drop-after-groups-ready" "$state/online"
                        fi
                        if [ -f "$state/drop-substrate-after-groups-ready" ]; then
                            rm -f "$state/drop-substrate-after-groups-ready" \
                                "$state/active-etcd.service"
                        fi
                    fi
                fi
                ;;
        esac
        test -f "$state/active-$unit"
        ;;
    show)
        if [ "${2:-}" = mackesd.target ]; then
            if [ -f "$state/target-activating" ]; then
                printf '%s\n' activating
            elif [ -f "$state/active-mackesd-control.service" ] \
                && [ -f "$state/active-mackesd-observation.service" ] \
                && [ -f "$state/active-mackesd-actions.service" ] \
                && [ -f "$state/active-mackesd-data.service" ] \
                && [ -f "$state/active-mackesd-compute.service" ] \
                && [ -f "$state/active-mackesd-integrations.service" ]; then
                printf '%s\n' active
            else
                printf '%s\n' inactive
            fi
        else
            exit 64
        fi
        ;;
    start)
        if [ "${2:-}" = mcnf-xdg-bind-recovery.service ]; then
            printf '%s\n' xdg-binds >>"$state/mutations"
            if [ -f "$state/drop-after-xdg" ]; then
                rm -f "$state/drop-after-xdg" "$state/online"
            elif [ -f "$state/drop-substrate-after-xdg" ]; then
                rm -f "$state/drop-substrate-after-xdg" \
                    "$state/active-etcd.service"
            fi
        elif [ "${2:-}" = etcd.service ] || [ "${2:-}" = syncthing.service ]; then
            unit=$2
            printf '%s\n' "$unit" >>"$state/mutations"
            [ ! -f "$state/fail-start-$unit" ] || exit 1
            : >"$state/active-$unit"
            if [ "$unit" = etcd.service ] && [ -f "$state/drop-after-etcd-start" ]; then
                rm -f "$state/drop-after-etcd-start" "$state/online"
            elif [ "$unit" = etcd.service ] && [ -f "$state/drop-overlay-after-etcd-start" ]; then
                rm -f "$state/drop-overlay-after-etcd-start" \
                    "$state/active-nebula.service"
                : >"$state/force-nebula-inactive"
            elif [ "$unit" = etcd.service ] && [ -f "$state/drop-etcd-after-etcd-start" ]; then
                rm -f "$state/drop-etcd-after-etcd-start" \
                    "$state/etcd-post-start-checks"
                : >"$state/drop-etcd-after-etcd-start-armed"
            elif [ "$unit" = syncthing.service ] && [ -f "$state/drop-after-syncthing-start" ]; then
                rm -f "$state/drop-after-syncthing-start" "$state/online"
            elif [ "$unit" = syncthing.service ] && [ -f "$state/drop-etcd-after-syncthing-start" ]; then
                rm -f "$state/drop-etcd-after-syncthing-start" \
                    "$state/active-etcd.service"
            elif [ "$unit" = syncthing.service ] && [ -f "$state/drop-overlay-after-syncthing-start" ]; then
                rm -f "$state/drop-overlay-after-syncthing-start" \
                    "$state/active-nebula.service"
                : >"$state/force-nebula-inactive"
            fi
        elif [ "${2:-}" = mde-shell-egui.service ]; then
            printf '%s\n' "$2" >>"$state/mutations"
            [ ! -f "$state/fail-start-mde-shell-egui.service" ] || exit 1
            : >"$state/active-mde-shell-egui.service"
        elif [ "${2:-}" = mackesd.target ]; then
            printf '%s\n' mackesd.target >>"$state/mutations"
            if [ ! -f "$state/target-active" ]; then
                if [ -f "$state/delay-groups" ]; then
                    : >"$state/pending-groups"
                else
                    for group in control observation actions data compute integrations; do
                        : >"$state/active-mackesd-$group.service"
                    done
                fi
            fi
        elif printf '%s\n' "${2:-}" | grep -Eq '^mackesd-(control|observation|actions|data|compute|integrations)\.service$'; then
            unit=$2
            printf '%s\n' "$unit" >>"$state/mutations"
            : >"$state/active-$unit"
        else
            printf 'trigger:%s\n' "$*" >>"$state/triggers"
        fi
        ;;
    *) exit 64 ;;
esac
SH
cat >"$BIN/ip" <<'SH'
#!/bin/sh
state=${MCNF_TEST_STATE:?}
if [ "${1:-}" = -4 ] && [ "${2:-}" = route ]; then
    test ! -f "$state/no-default-route" \
        && printf '%s\n' 'default via 192.0.2.1 dev eth0'
    exit 0
fi
if test -f "$state/active-nebula.service"; then
    printf '%s\n' '7: nebula1 inet 10.42.0.7/17 scope global'
    if test -f "$state/drop-after-nebula-ready"; then
        rm -f "$state/drop-after-nebula-ready" "$state/online"
    fi
fi
SH
cat >"$BIN/nm-online" <<'SH'
#!/bin/sh
state=${MCNF_TEST_STATE:?}
if [ -f "$state/drop-after-first-online-check" ]; then
    count=$(cat "$state/online-checks" 2>/dev/null || printf 0)
    count=$((count + 1))
    printf '%s\n' "$count" >"$state/online-checks"
    if [ "$count" -ge 2 ]; then
        rm -f "$state/online"
    fi
fi
test -f "$state/online"
SH
cat >"$BIN/notify" <<'SH'
#!/bin/sh
printf '%s\n' "$*" >>"${MCNF_TEST_STATE:?}/notifies"
SH
cat >"$BIN/pgrep" <<'SH'
#!/bin/sh
state=${MCNF_TEST_STATE:?}
[ "${1:-}" = -x ] && [ "${2:-}" = mde-shell-egui ] || exit 2
test -f "$state/stale-mde-shell-egui"
SH
cat >"$BIN/sleep" <<'SH'
#!/bin/sh
printf '%s\n' "$1" >>"${MCNF_TEST_STATE:?}/sleeps"
SH
chmod 0755 "$BIN"/*

run_helper() {
    NOTIFY_SOCKET="$ROOT/notify.sock" MCNF_TEST_STATE="$STATE" \
    MCNF_RECOVERY_SYSTEMCTL="$BIN/systemctl" MCNF_RECOVERY_IP="$BIN/ip" \
    MCNF_RECOVERY_NOTIFY="$BIN/notify" MCNF_RECOVERY_SLEEP="$BIN/sleep" \
    MCNF_RECOVERY_PGREP="$BIN/pgrep" \
    MCNF_RECOVERY_NM_ONLINE="$BIN/nm-online" MCNF_RECOVERY_NETWORKCTL="$ROOT/missing-networkctl" \
    MCNF_RECOVERY_LOCK="$ROOT/recovery.lock" MCNF_NEBULA_DIR="$ROOT/nebula" \
    MCNF_ROLE_FILE="$ROOT/role.toml" MCNF_ETCD_MEMBER_FILE="$ROOT/etcd.env" \
    MCNF_SYNCTHING_CONFIG="$ROOT/syncthing.conf" "$HELPER"
}

run_helper
[ ! -s "$STATE/mutations" ]
grep -Fq 'offline-no-mutation' "$STATE/notifies"
echo 'PASS offline fixture: no service mutation'

# NetworkManager can retain a positive global result after the usable
# substrate route disappears. A cached online signal must not authorize even
# the first recovery mutation.
: >"$STATE/online"
: >"$STATE/no-default-route"
: >"$STATE/mutations"
: >"$STATE/notifies"
run_helper
[ ! -s "$STATE/mutations" ]
grep -Fq 'offline-no-mutation' "$STATE/notifies"
rm -f "$STATE/no-default-route"
echo 'PASS default-route fixture: cached manager-online state fails closed'

# The event can be admitted while online and lose its link before the
# single-flight lease is acquired. The post-lock attestation must fail closed
# before even restarting Nebula or touching a configured substrate service.
: >"$STATE/online"
: >"$STATE/mutations"
: >"$STATE/notifies"
: >"$STATE/online-checks"
: >"$STATE/drop-after-first-online-check"
run_helper
if [ -s "$STATE/mutations" ]; then
    echo 'mid-recovery network loss caused a service mutation' >&2
    exit 1
fi
grep -Fq 'status=offline-during-recovery' "$STATE/notifies"
rm -f "$STATE/drop-after-first-online-check"
echo 'PASS stale-network fixture: post-lock attestation fails closed'

# The physical link can disappear after Nebula has materialized its TUN
# address. That overlay signal must not authorize etcd/Syncthing mutation.
: >"$STATE/online"
: >"$STATE/mutations"
: >"$STATE/notifies"
: >"$STATE/sleeps"
: >"$STATE/drop-after-nebula-ready"
run_helper
printf '%s\n' nebula.service >"$STATE/expected-mutations"
cmp "$STATE/expected-mutations" "$STATE/mutations"
grep -Fq 'status=offline-after-nebula' "$STATE/notifies"
rm -f "$STATE/drop-after-nebula-ready" "$STATE/active-nebula.service" \
    "$STATE/nebula-attempts" "$STATE/nebula-ready-checks"
: >"$STATE/sleeps"
echo 'PASS overlay-to-substrate fixture: link loss after Nebula readiness prevents downstream mutation'

: >"$STATE/online"
: >"$STATE/notifies"
: >"$STATE/mutations"
for invalid_role in 'role = "server"' 'role = "workstation' $'role = "workstation"\nrole = "lighthouse"'; do
    printf '%s\n' "$invalid_role" >"$ROOT/role.toml"
    if run_helper; then
        echo 'invalid recovery role unexpectedly reported success' >&2
        exit 1
    fi
done
[ ! -s "$STATE/mutations" ]
[ "$(grep -Fc 'status=refused-invalid-role' "$STATE/notifies")" -eq 3 ]
printf '%s\n' 'role = "workstation"' >"$ROOT/role.toml"
echo 'PASS role fixture: unsupported, malformed, and duplicate identity fail before service mutation'

: >"$STATE/notifies"
printf '%s\n' 'role = "lighthouse"' >"$ROOT/role.toml"
: >"$ROOT/etcd.env"
if run_helper; then
    echo 'unconfigured lighthouse unexpectedly reported recovery success' >&2
    exit 1
fi
[ ! -s "$STATE/mutations" ]
grep -Fq 'status=refused-lighthouse-etcd-unconfigured' "$STATE/notifies"
printf '%s\n' 'role = "workstation"' >"$ROOT/role.toml"
printf '%s\n' member >"$ROOT/etcd.env"
echo 'PASS lighthouse fixture: missing coordination membership fails before service mutation'

# A configured Lighthouse has no desktop seat or communal XDG binds. Even when
# every shared recovery dependency is healthy, a return event must not start a
# Workstation-only helper or make Lighthouse convergence depend on it.
printf '%s\n' 'role = "lighthouse"' >"$ROOT/role.toml"
: >"$STATE/active-nebula.service"
: >"$STATE/active-etcd.service"
: >"$STATE/active-syncthing.service"
for group in control observation actions data compute integrations; do
    : >"$STATE/active-mackesd-$group.service"
done
: >"$STATE/mutations"
: >"$STATE/notifies"
run_helper
[ ! -s "$STATE/mutations" ]
grep -Fq 'status=skipped-workstation-xdg-lighthouse' "$STATE/notifies"
grep -Fq 'status=already-recovered' "$STATE/notifies"
rm -f "$STATE"/active-nebula.service "$STATE"/active-etcd.service \
    "$STATE"/active-syncthing.service "$STATE"/active-mackesd-*.service
printf '%s\n' 'role = "workstation"' >"$ROOT/role.toml"
echo 'PASS lighthouse role fixture: peer return skips Workstation-only XDG mutation'

: >"$STATE/notifies"
run_helper
cat >"$STATE/expected-mutations" <<'EOF'
nebula.service
etcd.service
syncthing.service
mackesd.target
xdg-binds
EOF
cmp "$STATE/expected-mutations" "$STATE/mutations"
printf '1\n1\n' >"$STATE/expected-sleeps"
cmp "$STATE/expected-sleeps" "$STATE/sleeps"
grep -Fq 'status=recovered' "$STATE/notifies"
echo 'PASS online fixture: bounded TUN readiness wait and dependency order'

# A configured coordination member is an ordering dependency, not an optional
# best-effort service. If its start fails, recovery must not continue into
# Syncthing, XDG repair, or grouped workers that could act without coordination.
rm -f "$STATE"/active-etcd.service "$STATE"/active-syncthing.service \
    "$STATE"/active-mackesd-*.service
: >"$STATE/fail-start-etcd.service"
: >"$STATE/mutations"
: >"$STATE/notifies"
if run_helper; then
    echo 'configured etcd failure unexpectedly reported success' >&2
    exit 1
fi
printf '%s\n' etcd.service >"$STATE/expected-mutations"
cmp "$STATE/expected-mutations" "$STATE/mutations"
grep -Fq 'status=failed-configured-etcd' "$STATE/notifies"
rm -f "$STATE/fail-start-etcd.service"
echo 'PASS substrate failure fixture: no downstream mutation after etcd failure'

# The link can disappear after coordination starts but before the file
# substrate is touched. Recovery must stop at that boundary instead of
# mutating Syncthing from the stale pre-etcd admission.
rm -f "$STATE"/active-etcd.service "$STATE"/active-syncthing.service \
    "$STATE"/active-mackesd-*.service
: >"$STATE/online"
: >"$STATE/drop-after-etcd-start"
: >"$STATE/mutations"
: >"$STATE/notifies"
run_helper
printf '%s\n' etcd.service >"$STATE/expected-mutations"
cmp "$STATE/expected-mutations" "$STATE/mutations"
grep -Fq 'status=offline-after-etcd' "$STATE/notifies"
rm -f "$STATE/drop-after-etcd-start"
echo 'PASS substrate boundary fixture: link loss after etcd prevents Syncthing mutation'

# A healthy physical route does not preserve the completed overlay or
# coordination step.  If either disappears after etcd startup, Syncthing must
# not be mutated from the stale success result.
for lost_dependency in overlay etcd; do
    rm -f "$STATE"/active-etcd.service "$STATE"/active-syncthing.service \
        "$STATE"/active-mackesd-*.service
    : >"$STATE/active-nebula.service"
    : >"$STATE/online"
    : >"$STATE/drop-${lost_dependency}-after-etcd-start"
    : >"$STATE/mutations"
    : >"$STATE/notifies"
    if run_helper; then
        echo "lost $lost_dependency unexpectedly allowed Syncthing mutation" >&2
        exit 1
    fi
    printf '%s\n' etcd.service >"$STATE/expected-mutations"
    cmp "$STATE/expected-mutations" "$STATE/mutations"
    grep -Fq "status=${lost_dependency}-lost-after-etcd" "$STATE/notifies"
    rm -f "$STATE/drop-${lost_dependency}-after-etcd-start" \
        "$STATE/force-nebula-inactive" \
        "$STATE/drop-etcd-after-etcd-start-armed" \
        "$STATE/etcd-post-start-checks"
done
: >"$STATE/active-nebula.service"
: >"$STATE/active-etcd.service"
echo 'PASS post-etcd dependency fixture: lost overlay/coordination blocks Syncthing mutation'

# A boot-time event can arrive after Syncthing became active but before the
# grouped workers.  Recovery must preserve that process instead of racing its
# initial scan with a bounded restart.
: >"$STATE/online"
rm -f "$STATE"/active-etcd.service "$STATE"/active-mackesd-*.service
: >"$STATE/active-syncthing.service"
: >"$STATE/mutations"
: >"$STATE/notifies"
run_helper
cat >"$STATE/expected-mutations" <<'EOF'
etcd.service
mackesd.target
xdg-binds
EOF
cmp "$STATE/expected-mutations" "$STATE/mutations"
grep -Fq 'status=syncthing-already-ready' "$STATE/notifies"
grep -Fq 'status=recovered' "$STATE/notifies"
echo 'PASS boot race fixture: active Syncthing is preserved'

# The link can disappear after Syncthing starts but before grouped workers.
# Recovery must re-attest at that boundary rather than allowing stale network
# admission to mutate the daemon set.
rm -f "$STATE"/active-etcd.service "$STATE"/active-mackesd-*.service
rm -f "$STATE/active-syncthing.service"
: >"$STATE/online"
: >"$STATE/drop-after-syncthing-start"
: >"$STATE/mutations"
: >"$STATE/notifies"
run_helper
cat >"$STATE/expected-mutations" <<'EOF'
etcd.service
syncthing.service
EOF
cmp "$STATE/expected-mutations" "$STATE/mutations"
grep -Fq 'status=offline-after-syncthing' "$STATE/notifies"
echo 'PASS Syncthing boundary fixture: link loss prevents grouped mutation'
rm -f "$STATE/drop-after-syncthing-start"

# The physical route and both configured services can remain active after the
# overlay dies during Syncthing startup. Grouped workers must not be started
# from that stale overlay admission or publish against a partial mesh.
rm -f "$STATE"/active-etcd.service "$STATE"/active-syncthing.service \
    "$STATE"/active-mackesd-*.service
: >"$STATE/active-nebula.service"
: >"$STATE/online"
: >"$STATE/drop-overlay-after-syncthing-start"
: >"$STATE/mutations"
: >"$STATE/notifies"
if run_helper; then
    echo 'lost overlay unexpectedly allowed grouped recovery' >&2
    exit 1
fi
cat >"$STATE/expected-mutations" <<'EOF'
etcd.service
syncthing.service
EOF
cmp "$STATE/expected-mutations" "$STATE/mutations"
grep -Fq 'status=overlay-lost-before-grouped' "$STATE/notifies"
rm -f "$STATE/drop-overlay-after-syncthing-start" \
    "$STATE/force-nebula-inactive"
: >"$STATE/active-nebula.service"
echo 'PASS pre-grouped overlay fixture: lost overlay blocks grouped mutation'

# Coordination can fail after Syncthing starts while the physical link remains
# online.  The complete configured substrate must be re-attested before grouped
# workers are allowed to start against a partial mesh.
rm -f "$STATE"/active-etcd.service "$STATE"/active-syncthing.service \
    "$STATE"/active-mackesd-*.service
: >"$STATE/online"
: >"$STATE/drop-etcd-after-syncthing-start"
: >"$STATE/mutations"
: >"$STATE/notifies"
if run_helper; then
    echo 'lost coordination unexpectedly allowed grouped recovery' >&2
    exit 1
fi
cat >"$STATE/expected-mutations" <<'EOF'
etcd.service
syncthing.service
EOF
cmp "$STATE/expected-mutations" "$STATE/mutations"
grep -Fq 'status=substrate-lost-before-grouped' "$STATE/notifies"
rm -f "$STATE/drop-etcd-after-syncthing-start"
: >"$STATE/active-etcd.service"
echo 'PASS pre-grouped substrate fixture: lost coordination blocks grouped mutation'

rm -f "$STATE"/active-mackesd-*.service "$STATE/group-ready-checks"
: >"$STATE/online"
: >"$STATE/target-activating"
: >"$STATE/pending-groups"
: >"$STATE/mutations"
: >"$STATE/notifies"
: >"$STATE/sleeps"
run_helper
printf '%s\n' xdg-binds >"$STATE/expected-mutations"
cmp "$STATE/expected-mutations" "$STATE/mutations"
printf '1\n' >"$STATE/expected-sleeps"
cmp "$STATE/expected-sleeps" "$STATE/sleeps"
grep -Fq 'status=recovered' "$STATE/notifies"
rm -f "$STATE/target-activating"
echo 'PASS grouped boot fixture: delayed child readiness is awaited'

# The physical link can disappear while grouped workers settle. The final
# desktop mutation must remain behind a fresh physical-network attestation.
rm -f "$STATE"/active-mackesd-*.service "$STATE/group-ready-checks" \
    "$STATE/target-active"
: >"$STATE/delay-groups"
: >"$STATE/drop-after-groups-ready"
: >"$STATE/online"
: >"$STATE/mutations"
: >"$STATE/notifies"
run_helper
cat >"$STATE/expected-mutations" <<'EOF'
mackesd.target
mackesd-control.service
mackesd-observation.service
EOF
cmp "$STATE/expected-mutations" "$STATE/mutations"
if grep -Fqx xdg-binds "$STATE/mutations"; then
    echo 'late network loss allowed desktop mutation' >&2
    exit 1
fi
grep -Fq 'status=offline-before-desktop' "$STATE/notifies"
rm -f "$STATE/delay-groups" "$STATE/drop-after-groups-ready"
echo 'PASS late-network fixture: desktop mutation waits for final link attestation'

# Group startup can race a substrate crash even when the physical link remains
# online.  Desktop recovery and its success publication must remain behind a
# fresh complete-substrate attestation.
rm -f "$STATE"/active-mackesd-*.service "$STATE/group-ready-checks" \
    "$STATE/target-active"
: >"$STATE/active-etcd.service"
: >"$STATE/active-syncthing.service"
: >"$STATE/delay-groups"
: >"$STATE/drop-substrate-after-groups-ready"
: >"$STATE/online"
: >"$STATE/mutations"
: >"$STATE/notifies"
if run_helper; then
    echo 'lost substrate unexpectedly allowed desktop recovery' >&2
    exit 1
fi
if grep -Fqx xdg-binds "$STATE/mutations"; then
    echo 'lost substrate allowed desktop mutation' >&2
    exit 1
fi
grep -Fq 'status=substrate-lost-before-desktop' "$STATE/notifies"
rm -f "$STATE/delay-groups" "$STATE/drop-substrate-after-groups-ready"
: >"$STATE/active-etcd.service"
echo 'PASS pre-desktop substrate fixture: lost coordination blocks desktop mutation'

: >"$STATE/online"
: >"$STATE/mutations"
: >"$STATE/notifies"
: >"$STATE/sleeps"
run_helper
printf '%s\n' xdg-binds >"$STATE/expected-mutations"
cmp "$STATE/expected-mutations" "$STATE/mutations"
[ ! -s "$STATE/sleeps" ]
grep -Fq 'status=already-recovered' "$STATE/notifies"
echo 'PASS repeated healthy event: no overlay or service restart'

# Desktop restoration is an external systemd mutation and can race loss of a
# previously healthy coordination process. The final success publication must
# re-attest the whole peer/session chain rather than retaining the pre-XDG
# substrate result.
: >"$STATE/active-etcd.service"
: >"$STATE/drop-substrate-after-xdg"
: >"$STATE/mutations"
: >"$STATE/notifies"
if run_helper; then
    echo 'late coordination loss unexpectedly reported recovery success' >&2
    exit 1
fi
printf '%s\n' xdg-binds >"$STATE/expected-mutations"
cmp "$STATE/expected-mutations" "$STATE/mutations"
grep -Fq 'status=substrate-lost-after-desktop' "$STATE/notifies"
if grep -Fq 'status=already-recovered' "$STATE/notifies" \
    || grep -Fq 'status=recovered' "$STATE/notifies"; then
    echo 'late coordination loss retained a false convergence publication' >&2
    exit 1
fi
rm -f "$STATE/drop-substrate-after-xdg"
: >"$STATE/active-etcd.service"
echo 'PASS final convergence fixture: post-XDG coordination loss retracts success'

# A healthy mesh substrate does not prove that the Workstation session survived
# boot/resume. Recovery must start a missing shell without restarting healthy
# overlay, substrate, or grouped daemon processes.
rm -f "$STATE/active-mde-shell-egui.service"
: >"$STATE/mutations"
: >"$STATE/notifies"
run_helper
cat >"$STATE/expected-mutations" <<'EOF'
xdg-binds
mde-shell-egui.service
EOF
cmp "$STATE/expected-mutations" "$STATE/mutations"
grep -Fq 'status=restoring-workstation-session' "$STATE/notifies"
grep -Fq 'status=already-recovered' "$STATE/notifies"
: >"$STATE/active-mde-shell-egui.service"
echo 'PASS missing-session fixture: healthy substrate restores only the Workstation shell'

# The XDG repair can finish after the grouped-readiness network attestation.
# If the physical link disappears during that mutation, recovery must not
# start a fresh desktop shell against stale peer-return admission.
rm -f "$STATE/active-mde-shell-egui.service"
: >"$STATE/online"
: >"$STATE/drop-after-xdg"
: >"$STATE/mutations"
: >"$STATE/notifies"
run_helper
cat >"$STATE/expected-mutations" <<'EOF'
xdg-binds
EOF
cmp "$STATE/expected-mutations" "$STATE/mutations"
grep -Fq 'status=offline-after-workstation-xdg' "$STATE/notifies"
echo 'PASS desktop boundary fixture: link loss after XDG repair prevents shell mutation'
rm -f "$STATE/drop-after-xdg"
: >"$STATE/active-mde-shell-egui.service"

# An inactive unit can still leave an orphaned shell process behind after a
# crash or interrupted stop. Recovery must refuse to start a second session
# until that stale process is removed.
rm -f "$STATE/active-mde-shell-egui.service"
: >"$STATE/online"
: >"$STATE/stale-mde-shell-egui"
: >"$STATE/mutations"
: >"$STATE/notifies"
if run_helper; then
    echo 'stale shell process unexpectedly allowed a second session' >&2
    exit 1
fi
[ ! -s "$STATE/mutations" ]
grep -Fq 'status=refused-stale-workstation-session' "$STATE/notifies"
rm -f "$STATE/stale-mde-shell-egui"
echo 'PASS stale-session fixture: orphaned shell refuses duplicate recovery'
: >"$STATE/active-mde-shell-egui.service"

# A network-return event can find the target active while one grouped daemon is
# missing. Recovery must start only that group. Restarting the target creates a
# long stop transaction and can repeatedly strand Eagle with most groups down.
: >"$STATE/target-active"
rm -f "$STATE/active-mackesd-observation.service"
: >"$STATE/mutations"
: >"$STATE/restarts"
: >"$STATE/notifies"
run_helper
cat >"$STATE/expected-mutations" <<'EOF'
mackesd.target
mackesd-observation.service
xdg-binds
EOF
cmp "$STATE/expected-mutations" "$STATE/mutations"
if grep -Fqx mackesd.target "$STATE/restarts"; then
    echo 'partial grouped recovery restarted the target' >&2
    exit 1
fi
grep -Fq 'status=recovered' "$STATE/notifies"
rm -f "$STATE/target-active"
echo 'PASS partial grouped recovery: missing child starts without target restart'

: >"$STATE/mutations"
flock "$ROOT/recovery.lock" sh -c 'touch "$1"; /usr/bin/sleep 3' sh "$STATE/lock-held" &
holder=$!
for _ in 1 2 3 4 5; do
    [ -f "$STATE/lock-held" ] && break
    /usr/bin/sleep 1
done
run_helper
wait "$holder"
[ ! -s "$STATE/mutations" ]
grep -Fq 'coalesced-recovery-already-running' "$STATE/notifies"
echo 'PASS repeated-trigger fixture: concurrent recovery coalesced'

: >"$STATE/triggers"
MCNF_TEST_STATE="$STATE" MCNF_RECOVERY_SYSTEMCTL="$BIN/systemctl" "$SLEEP_HOOK" pre suspend
MCNF_TEST_STATE="$STATE" MCNF_RECOVERY_SYSTEMCTL="$BIN/systemctl" "$SLEEP_HOOK" post suspend
MCNF_TEST_STATE="$STATE" MCNF_RECOVERY_SYSTEMCTL="$BIN/systemctl" "$NETWORK_HOOK" eth0 down
MCNF_TEST_STATE="$STATE" MCNF_RECOVERY_SYSTEMCTL="$BIN/systemctl" "$NETWORK_HOOK" eth0 up
[ "$(wc -l <"$STATE/triggers")" -eq 2 ]
grep -Fxq 'trigger:start --no-block mcnf-peer-recovery.service' "$STATE/triggers"
echo 'PASS trigger fixtures: resume/online accepted; pre-sleep/offline refused'
