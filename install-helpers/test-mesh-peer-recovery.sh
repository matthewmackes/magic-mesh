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
        elif [ "${2:-}" = etcd.service ] || [ "${2:-}" = syncthing.service ]; then
            unit=$2
            printf '%s\n' "$unit" >>"$state/mutations"
            [ ! -f "$state/fail-start-$unit" ] || exit 1
            : >"$state/active-$unit"
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
test -f "${MCNF_TEST_STATE:?}/active-nebula.service" \
    && printf '%s\n' '7: nebula1 inet 10.42.0.7/17 scope global'
SH
cat >"$BIN/nm-online" <<'SH'
#!/bin/sh
test -f "${MCNF_TEST_STATE:?}/online"
SH
cat >"$BIN/notify" <<'SH'
#!/bin/sh
printf '%s\n' "$*" >>"${MCNF_TEST_STATE:?}/notifies"
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
    MCNF_RECOVERY_NM_ONLINE="$BIN/nm-online" MCNF_RECOVERY_NETWORKCTL="$ROOT/missing-networkctl" \
    MCNF_RECOVERY_LOCK="$ROOT/recovery.lock" MCNF_NEBULA_DIR="$ROOT/nebula" \
    MCNF_ROLE_FILE="$ROOT/role.toml" MCNF_ETCD_MEMBER_FILE="$ROOT/etcd.env" \
    MCNF_SYNCTHING_CONFIG="$ROOT/syncthing.conf" "$HELPER"
}

run_helper
[ ! -s "$STATE/mutations" ]
grep -Fq 'offline-no-mutation' "$STATE/notifies"
echo 'PASS offline fixture: no service mutation'

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
xdg-binds
mackesd.target
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

# A boot-time event can arrive after Syncthing became active but before the
# grouped workers.  Recovery must preserve that process instead of racing its
# initial scan with a bounded restart.
rm -f "$STATE"/active-etcd.service "$STATE"/active-mackesd-*.service
: >"$STATE/active-syncthing.service"
: >"$STATE/mutations"
: >"$STATE/notifies"
run_helper
cat >"$STATE/expected-mutations" <<'EOF'
etcd.service
xdg-binds
mackesd.target
EOF
cmp "$STATE/expected-mutations" "$STATE/mutations"
grep -Fq 'status=syncthing-already-ready' "$STATE/notifies"
grep -Fq 'status=recovered' "$STATE/notifies"
echo 'PASS boot race fixture: active Syncthing is preserved'

rm -f "$STATE"/active-mackesd-*.service "$STATE/group-ready-checks"
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

: >"$STATE/mutations"
: >"$STATE/notifies"
: >"$STATE/sleeps"
run_helper
printf '%s\n' xdg-binds >"$STATE/expected-mutations"
cmp "$STATE/expected-mutations" "$STATE/mutations"
[ ! -s "$STATE/sleeps" ]
grep -Fq 'status=already-recovered' "$STATE/notifies"
echo 'PASS repeated healthy event: no overlay or service restart'

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
xdg-binds
mackesd.target
mackesd-observation.service
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
