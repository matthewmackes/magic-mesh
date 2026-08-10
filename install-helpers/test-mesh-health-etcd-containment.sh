#!/usr/bin/env bash
# Hostile WL-CRIT-007 regression for lighthouse etcd restart containment.
set -euo pipefail

ROOT="$(mktemp -d)"
trap 'rm -rf -- "$ROOT"' EXIT
BIN="$ROOT/bin"
STATE="$ROOT/state"
NEBULA="$ROOT/nebula"
HEALTH_RUN="$ROOT/run"
mkdir -p "$BIN" "$STATE" "$NEBULA/identity/current" "$HEALTH_RUN"
: > "$NEBULA/identity/current/host.crt"
printf '%s\n' 'role = "lighthouse"' > "$ROOT/role.toml"
printf '%s\n' 'ETCD_NAME=lh-hostile' > "$ROOT/etcd.env"
cat > "$ROOT/etcd-endpoints" <<'EOF'
https://10.42.0.1:2379
https://10.42.0.2:2379
https://10.42.0.3:2379
EOF
cat > "$NEBULA/config.yaml" <<'EOF'
lighthouse:
  am_lighthouse: true
EOF
printf '%s\n' 'some avg10=0.00 avg60=0.00 avg300=0.00 total=0' > "$ROOT/cpu.pressure"
printf '%s\n' 'some avg10=0.00 avg60=0.00 avg300=0.00 total=0' > "$ROOT/memory.pressure"
: > "$HEALTH_RUN/peer-publication.ok"

for unit in control observation actions data compute integrations; do
    : > "$STATE/active-mackesd-$unit.service"
done
: > "$STATE/active-nebula.service"
printf '%s\n' active > "$STATE/etcd-active-state"
printf '%s\n' running > "$STATE/etcd-sub-state"
printf '%s\n' 2 > "$STATE/status-responders"

cat > "$BIN/systemctl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
state=${MCNF_TEST_STATE:?}
printf '%s\n' "$*" >> "$state/systemctl.log"
if [[ "$*" == "show etcd.service -p ActiveState --value" ]]; then
    cat "$state/etcd-active-state"
    exit 0
fi
if [[ "$*" == "show etcd.service -p SubState --value" ]]; then
    cat "$state/etcd-sub-state"
    exit 0
fi
if [[ " $* " == *" list-unit-files syncthing.service "* ]]; then
    exit 1
fi
if [[ " $* " == *" is-active "* ]]; then
    unit=${*: -1}
    test -f "$state/active-$unit"
    exit
fi
if [[ " $* " == *" restart etcd.service "* ]]; then
    count=0
    test ! -f "$state/etcd-restarts" || read -r count < "$state/etcd-restarts"
    printf '%s\n' "$((count + 1))" > "$state/etcd-restarts"
    exit 0
fi
exit 0
EOF

cat > "$BIN/etcdctl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
state=${MCNF_TEST_STATE:?}
printf '%s\n' "$*" >> "$state/etcdctl.log"
if [[ " $* " == *" endpoint health "* ]]; then
    test -f "$state/health-restored"
    exit
fi
if [[ " $* " == *" endpoint status "* ]]; then
    responders=$(cat "$state/status-responders")
    endpoint=""
    for arg in "$@"; do
        case "$arg" in --endpoints=*) endpoint=${arg#--endpoints=} ;; esac
    done
    case "$endpoint" in
        *10.42.0.1*) index=1; member=101 ;;
        *10.42.0.2*) index=2; member=102 ;;
        *10.42.0.3*) index=3; member=103 ;;
        *) exit 1 ;;
    esac
    test "$index" -le "$responders" || exit 1
    leader=102
    if test "$index" -eq 2 && test -f "$state/divergent-leader"; then
        leader=999
    fi
    printf '[{"Endpoint":"%s","Status":{"header":{"cluster_id":9001,"member_id":%s,"revision":73,"raft_term":7},"version":"3.5.21","dbSize":75000000,"leader":%s,"raftIndex":81,"raftTerm":7}}]\n' \
        "$endpoint" "$member" "$leader"
    exit 0
fi
exit 1
EOF

cat > "$BIN/ip" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF

cat > "$BIN/hostname" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' hostile-lighthouse
EOF

chmod +x "$BIN"/*

HEALTH=${1:-"$(cd "$(dirname "$0")" && pwd)/mesh-health-check.sh"}

run_health() {
    PATH="$BIN:/usr/bin:/usr/sbin" \
    MCNF_TEST_STATE="$STATE" \
    MCNF_NEBULA_DIR="$NEBULA" \
    MCNF_ROLE_FILE="$ROOT/role.toml" \
    MCNF_HEALTH_RUN_DIR="$HEALTH_RUN" \
    MCNF_ETCD_ENDPOINTS_FILE="$ROOT/etcd-endpoints" \
    MCNF_ETCD_MEMBER_FILE="$ROOT/etcd.env" \
    MCNF_CPU_PRESSURE_FILE="$ROOT/cpu.pressure" \
    MCNF_MEMORY_PRESSURE_FILE="$ROOT/memory.pressure" \
    MCNF_ETCD_PRESSURE_THRESHOLD=80 \
    MCNF_ETCD_RESTART_COOLDOWN_S=300 \
    MCNF_PEER_PUBLICATION_MAX_AGE_S=3600 \
    MESH_ALERT_BIN=/bin/false \
    "$HEALTH"
}

assert_degraded_without_restart() {
    local log_file="$1" expected="$2"
    if run_health > "$log_file" 2>&1; then
        echo "expected watchdog pass to report degraded" >&2
        exit 1
    fi
    if ! grep -Fq "$expected" "$log_file"; then
        echo "missing diagnostic: $expected" >&2
        sed -n '1,120p' "$log_file" >&2
        exit 1
    fi
    test ! -e "$STATE/etcd-restarts"
}

# Never interrupt an in-flight start or stop, even with visible remote quorum.
printf '%s\n' activating > "$STATE/etcd-active-state"
printf '%s\n' start > "$STATE/etcd-sub-state"
assert_degraded_without_restart "$ROOT/activating.log" \
    'unit is activating/start; allow the current transition to finish'
printf '%s\n' deactivating > "$STATE/etcd-active-state"
printf '%s\n' stop-sigabrt > "$STATE/etcd-sub-state"
assert_degraded_without_restart "$ROOT/deactivating.log" \
    'unit is deactivating/stop-sigabrt; allow the current transition to finish'

# Missing pressure telemetry cannot be interpreted as proof of spare capacity.
printf '%s\n' active > "$STATE/etcd-active-state"
printf '%s\n' running > "$STATE/etcd-sub-state"
: > "$ROOT/cpu.pressure"
assert_degraded_without_restart "$ROOT/pressure-missing.log" \
    'PSI pressure telemetry unavailable or invalid (cpu=unavailable, memory=0.00)'

# Pressure refusal occurs before extra read-only status probes add more load.
printf '%s\n' 'some avg10=99.00 avg60=96.00 avg300=91.00 total=1' > "$ROOT/cpu.pressure"
printf '%s\n' 'some avg10=92.00 avg60=90.00 avg300=88.00 total=1' > "$ROOT/memory.pressure"
: > "$STATE/etcdctl.log"
assert_degraded_without_restart "$ROOT/pressure.log" \
    'severe host pressure cpu.some.avg10=99.00% memory.some.avg10=92.00%'
if grep -Fq 'endpoint status' "$STATE/etcdctl.log"; then
    echo "pressure refusal issued unnecessary endpoint status probes" >&2
    exit 1
fi

# Low pressure is still fail-closed when a strict majority cannot be observed.
printf '%s\n' 'some avg10=1.00 avg60=1.00 avg300=1.00 total=1' > "$ROOT/cpu.pressure"
printf '%s\n' 'some avg10=2.00 avg60=2.00 avg300=2.00 total=1' > "$ROOT/memory.pressure"
printf '%s\n' 1 > "$STATE/status-responders"
assert_degraded_without_restart "$ROOT/quorum.log" \
    'read-only quorum visibility 1/3, require 2'

# Two sockets are still uncertain when their consensus identities disagree.
printf '%s\n' 2 > "$STATE/status-responders"
: > "$STATE/divergent-leader"
assert_degraded_without_restart "$ROOT/divergent-quorum.log" \
    'require 2 distinct members agreeing on cluster/leader/term'
rm -f "$STATE/divergent-leader"

# A stable unit with bounded pressure and visible majority receives one real
# recovery attempt, then the cooldown contains repeated minute-timer failures.
printf '%s\n' 2 > "$STATE/status-responders"

# A full/broken runtime filesystem cannot provide durable rate limiting, so it
# must also remove restart authority rather than allowing an unbounded retry.
mkdir "$HEALTH_RUN/etcd-unreachable.restarted"
touch -d '6 minutes ago' "$HEALTH_RUN/etcd-unreachable.restarted"
assert_degraded_without_restart "$ROOT/stamp-unwritable.log" \
    'unable to persist restart cooldown'
rmdir "$HEALTH_RUN/etcd-unreachable.restarted"

if run_health > "$ROOT/safe-first.log" 2>&1; then
    echo "proposal failure must remain a degraded health result" >&2
    exit 1
fi
test "$(cat "$STATE/etcd-restarts")" = 1
test -f "$HEALTH_RUN/etcd-unreachable.restarted"
grep -Fq 'read-only quorum is visible (2/3) and host pressure is bounded' \
    "$ROOT/safe-first.log"

if run_health > "$ROOT/cooldown.log" 2>&1; then
    echo "cooldown pass must remain degraded" >&2
    exit 1
fi
test "$(cat "$STATE/etcd-restarts")" = 1
grep -Fq 'etcd recovery suppressed by 300s cooldown' "$ROOT/cooldown.log"

touch -d '6 minutes ago' "$HEALTH_RUN/etcd-unreachable.restarted"
if run_health > "$ROOT/safe-second.log" 2>&1; then
    echo "expired-cooldown proposal failure must remain degraded" >&2
    exit 1
fi
test "$(cat "$STATE/etcd-restarts")" = 2

# A successful commit probe clears containment state and reports healthy.
: > "$STATE/health-restored"
run_health > "$ROOT/healthy.log" 2>&1
test ! -e "$HEALTH_RUN/etcd-unreachable.restarted"
grep -Fq 'mesh-health: ok' "$ROOT/healthy.log"

echo "mesh-health etcd containment hostile regression: passed"
