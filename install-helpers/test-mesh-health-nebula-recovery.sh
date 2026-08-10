#!/usr/bin/env bash
# Hostile WL-CRIT-007 regression: an unreachable overlay restart must restore
# every Requires=nebula grouped daemon and must not repeat on every timer tick.
set -euo pipefail

ROOT="$(mktemp -d)"
trap 'rm -rf -- "$ROOT"' EXIT
BIN="$ROOT/bin"
STATE="$ROOT/state"
NEBULA="$ROOT/nebula"
HEALTH_RUN="$ROOT/run"
mkdir -p "$BIN" "$STATE" "$NEBULA/identity/current" "$HEALTH_RUN"
: > "$NEBULA/identity/current/host.crt"
printf '%s\n' 'role = "workstation"' > "$ROOT/role.toml"
cat > "$NEBULA/config.yaml" <<'EOF'
lighthouse:
  am_lighthouse: false
  hosts:
    - "10.42.0.1"
EOF

for unit in control observation actions data compute integrations; do
    : > "$STATE/active-mackesd-$unit.service"
done
: > "$STATE/active-nebula.service"

cat > "$BIN/systemctl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
state=${MCNF_TEST_STATE:?}
printf '%s\n' "$*" >> "$state/systemctl.log"
if [[ " $* " == *" list-unit-files syncthing.service "* ]]; then
    exit 1
fi
if [[ " $* " == *" is-active "* ]]; then
    unit=${*: -1}
    test -f "$state/active-$unit"
    exit
fi
if [[ " $* " == *" restart nebula.service "* ]]; then
    count=0
    test ! -f "$state/nebula-restarts" || read -r count < "$state/nebula-restarts"
    printf '%s\n' "$((count + 1))" > "$state/nebula-restarts"
    rm -f "$state"/active-mackesd-*.service
    : > "$state/active-nebula.service"
    exit
fi
if [[ " $* " == *" start mackesd.target "* ]]; then
    : > "$state/active-mackesd.target"
    exit
fi
if [[ " $* " == *" start mackesd-"*".service "* ]]; then
    unit=${*: -1}
    : > "$state/active-$unit"
    exit
fi
exit 0
EOF

cat > "$BIN/ip" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF

cat > "$BIN/ping" <<'EOF'
#!/usr/bin/env bash
test ! -e "${MCNF_TEST_STATE:?}/lighthouse-unreachable"
EOF

cat > "$BIN/hostname" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' hostile-seat
EOF

cat > "$BIN/etcdctl" <<'EOF'
#!/usr/bin/env bash
test ! -e "${MCNF_TEST_STATE:?}/etcd-unreachable"
EOF

chmod +x "$BIN"/*
: > "$STATE/lighthouse-unreachable"

run_health() {
    PATH="$BIN:/usr/bin:/usr/sbin" \
    MCNF_TEST_STATE="$STATE" \
    MCNF_NEBULA_DIR="$NEBULA" \
    MCNF_ROLE_FILE="$ROOT/role.toml" \
    MCNF_HEALTH_RUN_DIR="$HEALTH_RUN" \
    MCNF_ETCD_ENDPOINTS_FILE="$ROOT/etcd-endpoints" \
    MCNF_ETCD_MEMBER_FILE="$ROOT/no-etcd-member" \
    MCNF_NEBULA_UNREACHABLE_RESTART_COOLDOWN_S=60 \
    MCNF_PEER_PUBLICATION_RESTART_COOLDOWN_S=60 \
    MESH_ALERT_BIN=/bin/false \
    "$1"
}

HEALTH=${1:-"$(cd "$(dirname "$0")" && pwd)/mesh-health-check.sh"}

if run_health "$HEALTH" > "$ROOT/first.log" 2>&1; then
    echo "expected the first unreachable-overlay pass to report degraded" >&2
    exit 1
fi
test "$(cat "$STATE/nebula-restarts")" = 1
grep -Fq 'restoring grouped mackesd after nebula restart' "$ROOT/first.log"
test -f "$STATE/active-mackesd.target"
for unit in control observation actions data compute integrations; do
    test -f "$STATE/active-mackesd-$unit.service"
done

if run_health "$HEALTH" > "$ROOT/second.log" 2>&1; then
    echo "expected the cooldown pass to remain degraded" >&2
    exit 1
fi
test "$(cat "$STATE/nebula-restarts")" = 1
grep -Fq 'restart suppressed by 60s cooldown' "$ROOT/second.log"

touch -d '2 minutes ago' "$HEALTH_RUN/nebula-unreachable.restarted"
if run_health "$HEALTH" > "$ROOT/third.log" 2>&1; then
    echo "expected the expired-cooldown pass to report degraded" >&2
    exit 1
fi
test "$(cat "$STATE/nebula-restarts")" = 2
for unit in control observation actions data compute integrations; do
    test -f "$STATE/active-mackesd-$unit.service"
done

rm -f "$STATE/lighthouse-unreachable"
run_health "$HEALTH" > "$ROOT/healthy.log" 2>&1
test ! -e "$HEALTH_RUN/nebula-unreachable.restarted"
grep -Fq 'mesh-health: ok' "$ROOT/healthy.log"

# A client-only workstation must report total remote coordination loss without
# futilely restarting its condition-skipped local etcd.service.
printf '%s\n' 'http://10.42.0.1:2379' > "$ROOT/etcd-endpoints"
: > "$HEALTH_RUN/peer-publication.ok"
: > "$STATE/systemctl.log"
: > "$STATE/etcd-unreachable"
run_health "$HEALTH" > "$ROOT/client-only-etcd.log" 2>&1
grep -Fq 'client-only node has no local etcd member to restart' \
    "$ROOT/client-only-etcd.log"
grep -Fq 'mesh-health: ok' "$ROOT/client-only-etcd.log"
if grep -Fq 'restart etcd.service' "$STATE/systemctl.log"; then
    echo "client-only coordination loss attempted a futile local etcd restart" >&2
    exit 1
fi

# A persistent publication failure may restart observation once, but not on
# every minute timer tick while the remote quorum remains unable to commit.
rm -f "$STATE/etcd-unreachable"
touch -d '3 minutes ago' "$HEALTH_RUN/peer-publication.ok"
: > "$STATE/systemctl.log"
if run_health "$HEALTH" > "$ROOT/publication-first.log" 2>&1; then
    echo "expected stale publication to report degraded" >&2
    exit 1
fi
test "$(grep -Fc 'restart mackesd-observation.service' "$STATE/systemctl.log")" = 1
if run_health "$HEALTH" > "$ROOT/publication-second.log" 2>&1; then
    echo "expected cooldown-suppressed stale publication to remain degraded" >&2
    exit 1
fi
test "$(grep -Fc 'restart mackesd-observation.service' "$STATE/systemctl.log")" = 1
grep -Fq 'observation restart suppressed by 60s cooldown' \
    "$ROOT/publication-second.log"

echo "mesh-health nebula recovery hostile regression: passed"
