#!/usr/bin/env bash
# Deterministic, no-daemon regression tests for Syncthing device scoping.
# All service, registry, and Syncthing commands are mocked under a temporary
# root; this script never touches a live node or its Syncthing configuration.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SETUP="$REPO_ROOT/install-helpers/setup-syncthing.sh"
RECONCILE="$REPO_ROOT/install-helpers/syncthing-reconcile.sh"
HEALTH="$REPO_ROOT/install-helpers/mesh-health-check.sh"
TEST_ROOT="$(mktemp -d /tmp/mcnf-syncthing-device-scope.XXXXXX)"

cleanup() {
    case "$TEST_ROOT" in
        /tmp/mcnf-syncthing-device-scope.*) rm -rf -- "$TEST_ROOT" ;;
        *) printf 'refusing to remove unexpected test path: %s\n' "$TEST_ROOT" >&2 ;;
    esac
}
trap cleanup EXIT

SELF_ID='AAAAAAA-AAAAAAA-AAAAAAA-AAAAAAA-AAAAAAA-AAAAAAA-AAAAAAA-AAAAAAA'
PEER_ONE='BBBBBBB-BBBBBBB-BBBBBBB-BBBBBBB-BBBBBBB-BBBBBBB-BBBBBBB-BBBBBBB'
PEER_TWO='CCCCCCC-CCCCCCC-CCCCCCC-CCCCCCC-CCCCCCC-CCCCCCC-CCCCCCC-CCCCCCC'
STALE_GLOBAL='DDDDDDD-DDDDDDD-DDDDDDD-DDDDDDD-DDDDDDD-DDDDDDD-DDDDDDD-DDDDDDD'
STALE_MANAGED='EEEEEEE-EEEEEEE-EEEEEEE-EEEEEEE-EEEEEEE-EEEEEEE-EEEEEEE-EEEEEEE'
OTHER_FOLDER='FFFFFFF-FFFFFFF-FFFFFFF-FFFFFFF-FFFFFFF-FFFFFFF-FFFFFFF-FFFFFFF'

export TEST_SELF_ID="$SELF_ID"
export TEST_FOLDER_ID=mcnf-mesh
export TEST_HOSTNAME=seat15

MOCK_BIN="$TEST_ROOT/bin"
mkdir -p "$MOCK_BIN"

cat > "$MOCK_BIN/hostname" <<'SH'
#!/usr/bin/env bash
printf '%s\n' "${TEST_HOSTNAME:-seat15}"
SH

cat > "$MOCK_BIN/systemctl" <<'SH'
#!/usr/bin/env bash
printf 'systemctl %s\n' "$*" >> "${TEST_CALL_LOG:?}"
exit 0
SH

cat > "$MOCK_BIN/etcdctl" <<'SH'
#!/usr/bin/env bash
case " $* " in
    *" endpoint health "*)
        case "${TEST_ETCD_HEALTH:-healthy}" in
            first-down)
                case "$*" in
                    *"--endpoints=https://10.42.0.1:2379"*) exit 1 ;;
                esac
                ;;
            all-down) exit 1 ;;
        esac
        exit 0
        ;;
    *" put "*)
        [ "${TEST_ETCD_MODE:-authoritative}" != offline ]
        ;;
    *" get "*)
        case "${TEST_ETCD_MODE:-authoritative}" in
            authoritative) sed -n '1,$p' "${TEST_REGISTRY_FILE:?}" ;;
            amplified)
                for _ in $(seq 1 400); do
                    sed -n '1,$p' "${TEST_REGISTRY_FILE:?}"
                done
                ;;
            empty) exit 0 ;;
            offline) exit 1 ;;
            *) exit 64 ;;
        esac
        ;;
    *) exit 64 ;;
esac
SH

cat > "$MOCK_BIN/syncthing" <<'SH'
#!/usr/bin/env bash
case " $* " in
    *" generate "*)
        printf 'Device ID: %s\n' "${TEST_SELF_ID:?}"
        ;;
    *" config devices list "*)
        sed -n '1,$p' "${TEST_GLOBAL_DEVICES_FILE:?}"
        ;;
    *" config folders ${TEST_FOLDER_ID:-mcnf-mesh} devices list "*)
        sed -n '1,$p' "${TEST_FOLDER_DEVICES_FILE:?}"
        ;;
    *" show system "*)
        sed -n '1,$p' "${TEST_SYSTEM_FILE:?}"
        ;;
    *" show connections "*)
        sed -n '1,$p' "${TEST_CONNECTIONS_FILE:?}"
        ;;
    *" config devices add "*|*" config folders ${TEST_FOLDER_ID:-mcnf-mesh} devices add "*)
        printf 'syncthing %s\n' "$*" >> "${TEST_CALL_LOG:?}"
        ;;
    *)
        printf 'unexpected syncthing invocation: %s\n' "$*" >&2
        exit 64
        ;;
esac
SH

cat > "$MOCK_BIN/logger" <<'SH'
#!/usr/bin/env bash
printf 'logger %s\n' "$*" >> "${TEST_CALL_LOG:?}"
SH

cat > "$MOCK_BIN/ip" <<'SH'
#!/usr/bin/env bash
case " $* " in
    *" link show nebula1 "*) printf '9: nebula1: <UP> mtu 1300\n' ;;
    *) exit 0 ;;
esac
SH

cat > "$MOCK_BIN/ping" <<'SH'
#!/usr/bin/env bash
exit 0
SH

cat > "$MOCK_BIN/df" <<'SH'
#!/usr/bin/env bash
case " $* " in
    *" --output=avail /run "*) printf 'Avail\n900000000\n' ;;
    *" --output=size /run "*) printf 'Size\n1000000000\n' ;;
    *) exit 64 ;;
esac
SH

cat > "$MOCK_BIN/mesh-alert" <<'SH'
#!/usr/bin/env bash
printf 'mesh-alert %s\n' "$*" >> "${TEST_CALL_LOG:?}"
SH

cat > "$MOCK_BIN/dnf" <<'SH'
#!/usr/bin/env bash
printf 'dnf must not be called by this test\n' >&2
exit 64
SH

chmod +x "$MOCK_BIN"/*
export PATH="$MOCK_BIN:$PATH"

REGISTRY_FILE="$TEST_ROOT/registry.txt"
cat > "$REGISTRY_FILE" <<EOF
/mesh/syncthing/seat15
$SELF_ID@10.42.0.5
/mesh/syncthing/dell
$PEER_ONE@10.42.0.4
/mesh/syncthing/surface
$PEER_TWO@10.42.0.7
EOF
export TEST_REGISTRY_FILE="$REGISTRY_FILE"

write_stale_config() {
    local config="$1"
    mkdir -p "$(dirname "$config")"
    cat > "$config" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<configuration version="37">
  <folder id="mcnf-mesh" label="Mesh Sync" path="/old/mesh" type="sendreceive">
    <device id="$SELF_ID" />
    <device id="$STALE_MANAGED" />
  </folder>
  <folder id="unrelated" label="Unrelated" path="/unrelated" type="sendreceive">
    <device id="$SELF_ID" />
    <device id="$OTHER_FOLDER" />
  </folder>
  <device id="$SELF_ID" name="seat15"><address>dynamic</address></device>
  <device id="$STALE_GLOBAL" name="old-lighthouse"><address>dynamic</address></device>
  <device id="$STALE_MANAGED" name="old-managed"><address>dynamic</address></device>
  <device id="$OTHER_FOLDER" name="other-folder-peer"><address>dynamic</address></device>
  <gui><address>127.0.0.1:8384</address></gui>
  <options><listenAddress>default</listenAddress></options>
</configuration>
EOF
}

assert_setup_result() {
    local mode="$1" config="$2"
    python3 - "$mode" "$config" "$SELF_ID" "$PEER_ONE" "$PEER_TWO" \
        "$STALE_GLOBAL" "$STALE_MANAGED" "$OTHER_FOLDER" <<'PY'
import sys
import xml.etree.ElementTree as ET

mode, config, self_id, peer_one, peer_two, stale_global, stale_managed, other = sys.argv[1:]
root = ET.parse(config).getroot()
global_devices = {device.get('id') for device in root.findall('device')}
folders = {
    folder.get('id'): {device.get('id') for device in folder.findall('device')}
    for folder in root.findall('folder')
}

if mode == 'authoritative':
    assert global_devices == {self_id, peer_one, peer_two, other}, global_devices
    assert folders['mcnf-mesh'] == {self_id, peer_one, peer_two}, folders['mcnf-mesh']
    assert stale_global not in global_devices
    assert stale_managed not in global_devices
    assert folders['unrelated'] == {self_id, other}
else:
    original = {self_id, stale_global, stale_managed, other}
    assert global_devices == original, global_devices
    assert folders['mcnf-mesh'] == {self_id, stale_managed}, folders['mcnf-mesh']
    assert folders['unrelated'] == {self_id, other}
PY
}

run_setup_case() {
    local mode="$1"
    local case_root="$TEST_ROOT/setup-$mode"
    local home="$case_root/home"
    local systemd="$case_root/systemd"
    local calls="$case_root/calls.log"
    mkdir -p "$home" "$systemd"
    : > "$calls"
    write_stale_config "$home/config.xml"
    TEST_CALL_LOG="$calls" TEST_ETCD_MODE="$mode" \
    TEST_GLOBAL_DEVICES_FILE="$case_root/unused-global" \
    TEST_FOLDER_DEVICES_FILE="$case_root/unused-folder" \
    TEST_SYSTEM_FILE="$case_root/unused-system" \
    TEST_CONNECTIONS_FILE="$case_root/unused-connections" \
    MCNF_ETCD_ENDPOINTS_FILE="$TEST_ROOT/endpoints" \
    MCNF_SYSTEMD_DIR="$systemd" \
        "$SETUP" --listen 10.42.0.5 --home "$home" \
        --folder "$case_root/mesh" --folder-id mcnf-mesh \
        >"$case_root/stdout" 2>"$case_root/stderr"
    assert_setup_result "$mode" "$home/config.xml"
}

printf 'https://10.42.0.1:2379\n' > "$TEST_ROOT/endpoints"
run_setup_case authoritative
run_setup_case offline
run_setup_case empty
printf 'ok: full setup prunes only stale unshared globals from an authoritative registry\n'
printf 'ok: failed and empty registry reads preserve folder shares and global devices\n'

GLOBAL_DEVICES_FILE="$TEST_ROOT/global-devices.txt"
FOLDER_DEVICES_FILE="$TEST_ROOT/folder-devices.txt"
SYSTEM_FILE="$TEST_ROOT/system.json"
cat > "$GLOBAL_DEVICES_FILE" <<EOF
$SELF_ID
$PEER_ONE
$PEER_TWO
$STALE_GLOBAL
$STALE_MANAGED
$OTHER_FOLDER
EOF
cat > "$FOLDER_DEVICES_FILE" <<EOF
$SELF_ID
$PEER_ONE
$PEER_TWO
EOF
printf '{"myID":"%s"}\n' "$SELF_ID" > "$SYSTEM_FILE"

run_health_case() {
    local scenario="$1" expected="$2" connections="$3"
    local case_root="$TEST_ROOT/health-$scenario"
    local calls="$TEST_ROOT/health-$scenario.calls"
    mkdir -p "$case_root/nebula" "$case_root/run" "$case_root/home"
    : > "$case_root/nebula/host.crt"
    printf 'lighthouse:\n  am_lighthouse: true\n' > "$case_root/nebula/config.yaml"
    printf 'role = "Workstation"\n' > "$case_root/role.toml"
    printf 'ok\n' > "$case_root/run/peer-publication.ok"
    : > "$calls"
    TEST_CALL_LOG="$calls" TEST_ETCD_MODE=authoritative \
    TEST_GLOBAL_DEVICES_FILE="$GLOBAL_DEVICES_FILE" \
    TEST_FOLDER_DEVICES_FILE="$FOLDER_DEVICES_FILE" \
    TEST_SYSTEM_FILE="$SYSTEM_FILE" TEST_CONNECTIONS_FILE="$connections" \
    MCNF_NEBULA_DIR="$case_root/nebula" MCNF_ROLE_FILE="$case_root/role.toml" \
    MCNF_HEALTH_RUN_DIR="$case_root/run" MCNF_ETCD_ENDPOINTS_FILE="$TEST_ROOT/endpoints" \
    MCNF_SYNCTHING_HOME="$case_root/home" MESH_ALERT_BIN="$MOCK_BIN/mesh-alert" \
        "$HEALTH" >"$case_root/stdout" 2>"$case_root/stderr"

    if [ "$expected" = no-alert ]; then
        if grep -q '^mesh-alert .*syncthing-out-of-sync' "$calls"; then
            printf 'unexpected Syncthing alert in healthy stale-global scenario\n' >&2
            sed -n '1,$p' "$calls" >&2
            exit 1
        fi
    else
        grep -q '^mesh-alert .*syncthing-out-of-sync' "$calls"
        grep -q '1/2 managed-folder peer device(s) connected' "$calls"
    fi
}

HEALTHY_CONNECTIONS="$TEST_ROOT/connections-healthy.json"
DISCONNECTED_CONNECTIONS="$TEST_ROOT/connections-disconnected.json"
cat > "$HEALTHY_CONNECTIONS" <<EOF
{"connections":{"$PEER_ONE":{"connected":true},"$PEER_TWO":{"connected":true},"$STALE_GLOBAL":{"connected":false},"$STALE_MANAGED":{"connected":false},"$OTHER_FOLDER":{"connected":true}}}
EOF
cat > "$DISCONNECTED_CONNECTIONS" <<EOF
{"connections":{"$PEER_ONE":{"connected":true},"$PEER_TWO":{"connected":false},"$STALE_GLOBAL":{"connected":true},"$STALE_MANAGED":{"connected":true},"$OTHER_FOLDER":{"connected":true}}}
EOF
run_health_case stale-unshared no-alert "$HEALTHY_CONNECTIONS"
run_health_case disconnected-folder-peer alert "$DISCONNECTED_CONNECTIONS"
printf 'ok: health ignores stale/unshared globals and accepts all connected folder peers\n'
printf 'ok: health alerts 1/2 when a real managed-folder peer is disconnected\n'

# A single slow/unreachable member must not make the watchdog restart a healthy
# local etcd while another coordination endpoint answers.
ENDPOINTS_MULTI="$TEST_ROOT/endpoints-multi"
printf 'https://10.42.0.1:2379\nhttps://10.42.0.2:2379\n' > "$ENDPOINTS_MULTI"
HEALTH_ENDPOINT_CASE="$TEST_ROOT/health-endpoint-any"
mkdir -p "$HEALTH_ENDPOINT_CASE/nebula" "$HEALTH_ENDPOINT_CASE/run" "$HEALTH_ENDPOINT_CASE/home"
: > "$HEALTH_ENDPOINT_CASE/nebula/host.crt"
printf 'lighthouse:\n  am_lighthouse: true\n' > "$HEALTH_ENDPOINT_CASE/nebula/config.yaml"
printf 'role = "Workstation"\n' > "$HEALTH_ENDPOINT_CASE/role.toml"
printf 'ok\n' > "$HEALTH_ENDPOINT_CASE/run/peer-publication.ok"
ENDPOINT_CALLS="$HEALTH_ENDPOINT_CASE/calls.log"
: > "$ENDPOINT_CALLS"
TEST_CALL_LOG="$ENDPOINT_CALLS" TEST_ETCD_MODE=authoritative TEST_ETCD_HEALTH=first-down \
TEST_GLOBAL_DEVICES_FILE="$GLOBAL_DEVICES_FILE" TEST_FOLDER_DEVICES_FILE="$FOLDER_DEVICES_FILE" \
TEST_SYSTEM_FILE="$SYSTEM_FILE" TEST_CONNECTIONS_FILE="$HEALTHY_CONNECTIONS" \
MCNF_NEBULA_DIR="$HEALTH_ENDPOINT_CASE/nebula" MCNF_ROLE_FILE="$HEALTH_ENDPOINT_CASE/role.toml" \
MCNF_HEALTH_RUN_DIR="$HEALTH_ENDPOINT_CASE/run" MCNF_ETCD_ENDPOINTS_FILE="$ENDPOINTS_MULTI" \
MCNF_SYNCTHING_HOME="$HEALTH_ENDPOINT_CASE/home" MESH_ALERT_BIN="$MOCK_BIN/mesh-alert" \
    "$HEALTH" >"$HEALTH_ENDPOINT_CASE/stdout" 2>"$HEALTH_ENDPOINT_CASE/stderr"
if grep -q 'systemctl restart etcd.service' "$ENDPOINT_CALLS"; then
    printf 'watchdog restarted etcd despite a healthy coordination endpoint\n' >&2
    exit 1
fi
printf 'ok: health leaves etcd alone when any coordination endpoint is healthy\n'

# A running observation group without a fresh successful own-row transaction is
# degraded even when 2/3 coordination members can answer. This is the Dell
# failure mode: service-active and quorum-ready must not mask stale presence.
touch -d '5 minutes ago' "$HEALTH_ENDPOINT_CASE/run/peer-publication.ok"
: > "$ENDPOINT_CALLS"
if TEST_CALL_LOG="$ENDPOINT_CALLS" TEST_ETCD_MODE=authoritative TEST_ETCD_HEALTH=first-down \
TEST_GLOBAL_DEVICES_FILE="$GLOBAL_DEVICES_FILE" TEST_FOLDER_DEVICES_FILE="$FOLDER_DEVICES_FILE" \
TEST_SYSTEM_FILE="$SYSTEM_FILE" TEST_CONNECTIONS_FILE="$HEALTHY_CONNECTIONS" \
MCNF_NEBULA_DIR="$HEALTH_ENDPOINT_CASE/nebula" MCNF_ROLE_FILE="$HEALTH_ENDPOINT_CASE/role.toml" \
MCNF_HEALTH_RUN_DIR="$HEALTH_ENDPOINT_CASE/run" MCNF_ETCD_ENDPOINTS_FILE="$ENDPOINTS_MULTI" \
MCNF_SYNCTHING_HOME="$HEALTH_ENDPOINT_CASE/home" MESH_ALERT_BIN="$MOCK_BIN/mesh-alert" \
    "$HEALTH" >"$HEALTH_ENDPOINT_CASE/stale.stdout" 2>"$HEALTH_ENDPOINT_CASE/stale.stderr"; then
    printf 'watchdog reported success with a stale own-peer publication\n' >&2
    exit 1
fi
grep -q 'systemctl restart mackesd-observation.service' "$ENDPOINT_CALLS"
grep -q 'DEGRADED: own peer publication is stale' "$HEALTH_ENDPOINT_CASE/stale.stdout"
printf 'ok: stale own-peer publication fails health and requests bounded observation recovery\n'

RECONCILE_GLOBAL="$TEST_ROOT/reconcile-global.txt"
RECONCILE_FOLDER="$TEST_ROOT/reconcile-folder.txt"
cat > "$RECONCILE_GLOBAL" <<EOF
$SELF_ID
$PEER_ONE
$STALE_GLOBAL
EOF
printf '%s\n' "$SELF_ID" > "$RECONCILE_FOLDER"

run_reconcile_case() {
    local mode="$1"
    local calls="$TEST_ROOT/reconcile-$mode.calls"
    : > "$calls"
    TEST_CALL_LOG="$calls" TEST_ETCD_MODE="$mode" \
    TEST_GLOBAL_DEVICES_FILE="$RECONCILE_GLOBAL" \
    TEST_FOLDER_DEVICES_FILE="$RECONCILE_FOLDER" \
    TEST_SYSTEM_FILE="$SYSTEM_FILE" TEST_CONNECTIONS_FILE="$HEALTHY_CONNECTIONS" \
    MCNF_ETCD_ENDPOINTS_FILE="$TEST_ROOT/endpoints" MCNF_SYNCTHING_HOME="$TEST_ROOT/reconcile-home" \
    MCNF_HOSTNAME=seat15 "$RECONCILE"

    if [ "$mode" = authoritative ]; then
        grep -q "config folders mcnf-mesh devices add --device-id $PEER_ONE" "$calls"
        grep -q "config devices add --device-id $PEER_TWO" "$calls"
        grep -q "config folders mcnf-mesh devices add --device-id $PEER_TWO" "$calls"
        if grep -q "config devices add --device-id $PEER_ONE" "$calls"; then
            printf 'reconciler re-added an existing global device\n' >&2
            exit 1
        fi
    elif grep -qE 'config (devices|folders).* (add|delete|remove)' "$calls"; then
        printf 'offline reconciler mutated Syncthing state\n' >&2
        exit 1
    fi
    if grep -qE 'systemctl .*restart|config (devices|folders).* (delete|remove)' "$calls"; then
        printf 'additive reconciler attempted restart or deletion\n' >&2
        exit 1
    fi
}

run_reconcile_case authoritative
run_reconcile_case offline
printf 'ok: timer reconciler adds missing global/folder membership without restart or deletion\n'
printf 'ok: offline timer reconciler performs no Syncthing mutation\n'

# A duplicated/hostile registry response must not amplify one timer tick into
# unbounded repeated CLI mutations. The cap is intentionally exercised below
# the normal default so this stays a deterministic bounded-recovery regression.
AMPLIFIED_CALLS="$TEST_ROOT/reconcile-amplified.calls"
AMPLIFIED_GLOBAL="$TEST_ROOT/reconcile-amplified-global.txt"
AMPLIFIED_FOLDER="$TEST_ROOT/reconcile-amplified-folder.txt"
: > "$AMPLIFIED_CALLS"
printf '%s\n' "$SELF_ID" > "$AMPLIFIED_GLOBAL"
printf '%s\n' "$SELF_ID" > "$AMPLIFIED_FOLDER"
TEST_CALL_LOG="$AMPLIFIED_CALLS" TEST_ETCD_MODE=amplified \
TEST_GLOBAL_DEVICES_FILE="$AMPLIFIED_GLOBAL" \
TEST_FOLDER_DEVICES_FILE="$AMPLIFIED_FOLDER" \
TEST_SYSTEM_FILE="$SYSTEM_FILE" TEST_CONNECTIONS_FILE="$HEALTHY_CONNECTIONS" \
MCNF_ETCD_ENDPOINTS_FILE="$TEST_ROOT/endpoints" MCNF_SYNCTHING_HOME="$TEST_ROOT/reconcile-amplified-home" \
MCNF_HOSTNAME=seat15 MCNF_SYNCTHING_RECONCILE_MAX_ENTRIES=2 "$RECONCILE"
if grep -q "config devices add --device-id $PEER_TWO" "$AMPLIFIED_CALLS" || \
   [ "$(grep -c "config devices add --device-id $PEER_ONE" "$AMPLIFIED_CALLS")" -ne 1 ] || \
   [ "$(grep -c "config folders mcnf-mesh devices add --device-id $PEER_ONE" "$AMPLIFIED_CALLS")" -ne 1 ]; then
    printf 'reconciler exceeded its per-run registry entry cap under amplified input\n' >&2
    sed -n '1,20p' "$AMPLIFIED_CALLS" >&2
    exit 1
fi
printf 'ok: amplified registry input is capped before it can amplify Syncthing CLI mutations\n'
printf 'PASS: Syncthing managed-folder device-scope self-test\n'
