#!/usr/bin/env bash
# Bounded, read-only post-install CPU proof for WL-FUNC-021.
#
# The proof binds its observation to the declared magic-mesh RPM identity, then
# samples the mackesd process over a real interval using /proc tick deltas. It
# never restarts a service, changes a seat, or treats an old package as proof of
# the current source mitigation.
set -Eeuo pipefail

PROGRAM_NAME='verify-music-cpu-proof'
HOST="${MUSIC_LIVE_HOST:-172.20.0.15}"
HOSTS_RAW="${MUSIC_CPU_PROOF_HOSTS:-$HOST}"
USER_="${MUSIC_LIVE_USER:-mm}"
SSH_KEY="${MUSIC_LIVE_SSH_KEY:-${MCNF_FARM_KEY:-$HOME/.ssh/mackes_mesh_ed25519}}"
SSH_TIMEOUT="${MUSIC_CPU_PROOF_SSH_TIMEOUT_SECONDS:-150}"
OBSERVE_SECONDS="${MUSIC_CPU_PROOF_OBSERVE_SECONDS:-30}"
SAMPLE_INTERVAL="${MUSIC_CPU_PROOF_SAMPLE_INTERVAL_SECONDS:-2}"
MAX_PERMILLE="${MUSIC_CPU_PROOF_MAX_PERMILLE:-850}"
MEAN_PERMILLE="${MUSIC_CPU_PROOF_MEAN_PERMILLE:-500}"
EXPECTED_ARCH='x86_64'
EXPECTED_EXECUTABLE='/usr/bin/mackesd'
declare -a PROOF_HOSTS=()

usage() {
    cat >&2 <<'EOF'
usage: verify-music-cpu-proof.sh [--self-test]

Runs bounded, read-only CPU observations on one or more approved Music seats.
The installed magic-mesh RPM must match the checked-out version/release. Each
seat also reports executable/package provenance and refuses a daemon process
that predates the installed RPM. CPU is reported as permille of one host CPU,
derived from /proc tick deltas.

Environment: MUSIC_LIVE_HOST, MUSIC_LIVE_USER, MUSIC_LIVE_SSH_KEY,
MUSIC_CPU_PROOF_HOSTS (comma-separated hosts; defaults to MUSIC_LIVE_HOST),
MUSIC_CPU_PROOF_SSH_TIMEOUT_SECONDS (20..300),
MUSIC_CPU_PROOF_OBSERVE_SECONDS (10..180),
MUSIC_CPU_PROOF_SAMPLE_INTERVAL_SECONDS (1..10),
MUSIC_CPU_PROOF_MAX_PERMILLE (100..1000), and
MUSIC_CPU_PROOF_MEAN_PERMILLE (100..1000).
EOF
}

fail() {
    printf '[FAIL] %s: %s\n' "$PROGRAM_NAME" "$1" >&2
    exit 2
}

refuse() {
    printf '[REFUSAL] %s: %s\n' "$PROGRAM_NAME" "$1" >&2
    exit 3
}

bounded_integer() {
    local value=$1 minimum=$2 maximum=$3
    [[ "$value" =~ ^[0-9]+$ ]] && (( value >= minimum && value <= maximum ))
}

cpu_permille() {
    local process_delta=$1 total_delta=$2 cores=$3
    if (( process_delta < 0 || total_delta <= 0 || cores <= 0 )); then
        printf '%s\n' '-1'
    else
        printf '%s\n' "$(( process_delta * cores * 1000 / total_delta ))"
    fi
}

parse_hosts() {
    local raw=$1 host
    PROOF_HOSTS=()
    IFS=',' read -r -a PROOF_HOSTS <<<"$raw"
    (( ${#PROOF_HOSTS[@]} > 0 )) || return 1
    for host in "${PROOF_HOSTS[@]}"; do
        [[ "$host" =~ ^[A-Za-z0-9._:-]+$ ]] || return 1
    done
}

declared_release_version() {
    local cargo_toml="${REPO_ROOT}/Cargo.toml"
    [[ -f "$cargo_toml" && ! -L "$cargo_toml" ]] || return 1
    awk '
        $0 ~ /^\[workspace\.package\][[:space:]]*$/ { in_workspace = 1; next }
        in_workspace && $0 ~ /^\[/ { exit }
        in_workspace && $0 ~ /^[[:space:]]*version[[:space:]]*=/ {
            value = $0
            sub(/^[^"]*"/, "", value)
            sub(/".*$/, "", value)
            if (value != "") { print value; found = 1; exit }
        }
        END { if (!found) exit 1 }
    ' "$cargo_toml"
}

declared_rpm_release() {
    local cargo_toml="${REPO_ROOT}/crates/mesh/mackesd/Cargo.toml"
    [[ -f "$cargo_toml" && ! -L "$cargo_toml" ]] || return 1
    awk '
        $0 ~ /^\[package\.metadata\.generate-rpm\][[:space:]]*$/ { in_rpm = 1; next }
        in_rpm && $0 ~ /^\[/ { exit }
        in_rpm && $0 ~ /^[[:space:]]*release[[:space:]]*=/ {
            value = $0
            sub(/^[^"]*"/, "", value)
            sub(/".*$/, "", value)
            if (value != "") { print value; found = 1; exit }
        }
        END { if (!found) exit 1 }
    ' "$cargo_toml"
}

validate_config() {
    parse_hosts "$HOSTS_RAW" || fail 'host list contains an empty or unsupported host'
    [[ "$USER_" =~ ^[A-Za-z0-9._-]+$ ]] || fail 'user contains unsupported characters'
    [[ -n "$SSH_KEY" ]] || fail 'SSH key path must be non-empty'
    bounded_integer "$SSH_TIMEOUT" 20 300 || fail 'SSH timeout must be 20..300 seconds'
    bounded_integer "$OBSERVE_SECONDS" 10 180 || fail 'observation window must be 10..180 seconds'
    bounded_integer "$SAMPLE_INTERVAL" 1 10 || fail 'sample interval must be 1..10 seconds'
    bounded_integer "$MAX_PERMILLE" 100 1000 || fail 'max CPU threshold must be 100..1000 permille'
    bounded_integer "$MEAN_PERMILLE" 100 1000 || fail 'mean CPU threshold must be 100..1000 permille'
    (( SSH_TIMEOUT > OBSERVE_SECONDS )) || fail 'SSH timeout must exceed observation window'
}

self_test() {
    [[ "$(cpu_permille 100 800 8)" == '1000' ]] || fail 'one full CPU was not normalized to 1000 permille'
    [[ "$(cpu_permille 0 800 8)" == '0' ]] || fail 'zero process ticks were not accepted'
    [[ "$(cpu_permille 1 0 8)" == '-1' ]] || fail 'zero total ticks were accepted'
    bounded_integer 20 20 300
    bounded_integer 19 20 300 && fail 'out-of-range lower bound was accepted'
    bounded_integer 1000 100 1000
    bounded_integer 1001 100 1000 && fail 'out-of-range upper bound was accepted'
    parse_hosts 'seat-a,172.20.146.225' || fail 'valid multi-seat host list was rejected'
    [[ "${#PROOF_HOSTS[@]}" == 2 && "${PROOF_HOSTS[1]}" == '172.20.146.225' ]] ||
        fail 'multi-seat host list was not preserved'
    parse_hosts 'seat-a,,seat-b' && fail 'empty host entry was accepted'
    parse_hosts 'seat-a/unsafe' && fail 'unsafe host entry was accepted'
    printf '%s: self-test passed (no SSH attempted)\n' "$PROGRAM_NAME"
}

if [[ "${1:-}" == '--self-test' ]]; then
    [[ "$#" == 1 ]] || fail '--self-test takes no additional arguments'
    self_test
    exit 0
fi
if [[ "${1:-}" == '--help' || "${1:-}" == '-h' ]]; then
    usage
    exit 0
fi
[[ "$#" == 0 ]] || { usage; fail "unknown argument: $1"; }

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd -P)"
validate_config
RELEASE_VERSION="$(declared_release_version)" || fail 'could not read declared platform release'
RPM_RELEASE="$(declared_rpm_release)" || fail 'could not read declared RPM release'
command -v ssh >/dev/null 2>&1 || fail 'ssh is required'
command -v timeout >/dev/null 2>&1 || fail 'timeout is required'
[[ -r "$SSH_KEY" ]] || refuse 'configured SSH key is unavailable; no CPU proof was attempted'

printf '[INFO] expected package: magic-mesh-%s-%s.%s\n' "$RELEASE_VERSION" "$RPM_RELEASE" "$EXPECTED_ARCH"
printf '[INFO] window=%ss interval=%ss max=%s‰ mean=%s‰ of one CPU\n' \
    "$OBSERVE_SECONDS" "$SAMPLE_INTERVAL" "$MAX_PERMILLE" "$MEAN_PERMILLE"

run_host() {
    local host=$1
    local remote_output='' remote_rc=0
    printf '== Music post-install CPU proof (%s@%s) ==\n' "$USER_" "$host"
    remote_output="$(
        timeout --signal=TERM --kill-after=3s "${SSH_TIMEOUT}s" \
            ssh -i "$SSH_KEY" -o BatchMode=yes -o ConnectTimeout=10 \
            -o ServerAliveInterval=5 -o ServerAliveCountMax=1 \
            -o StrictHostKeyChecking=accept-new -- "$USER_@$host" bash -s -- \
            "$RELEASE_VERSION" "$RPM_RELEASE" "$EXPECTED_ARCH" "$EXPECTED_EXECUTABLE" \
            "$OBSERVE_SECONDS" "$SAMPLE_INTERVAL" "$MAX_PERMILLE" "$MEAN_PERMILLE" \
            2>/dev/null <<'REMOTE_SCRIPT'
set -Eeuo pipefail
expected_version=$1
expected_release=$2
expected_arch=$3
expected_executable=$4
observe_seconds=$5
sample_interval=$6
max_threshold=$7
mean_threshold=$8

bounded_integer() {
    local value=$1 minimum=$2 maximum=$3
    [[ "$value" =~ ^[0-9]+$ ]] && (( value >= minimum && value <= maximum ))
}

for required in awk getconf rpm sha256sum sleep systemctl; do
    command -v "$required" >/dev/null 2>&1 || {
        printf '[REFUSAL] remote prerequisite unavailable: %s\n' "$required"
        exit 3
    }
done

identity="$(rpm -q --qf '%{NAME}\t%{VERSION}\t%{RELEASE}\t%{ARCH}\t%{INSTALLTIME}' magic-mesh 2>/dev/null || true)"
IFS=$'\t' read -r package_name package_version package_release package_arch package_install_epoch <<<"$identity"
if [[ "$package_name" != magic-mesh || "$package_version" != "$expected_version" ||
    "$package_release" != "$expected_release" || "$package_arch" != "$expected_arch" ]]; then
    printf '[REFUSAL] installed package is %s-%s-%s.%s; expected magic-mesh-%s-%s.%s\n' \
        "${package_name:-<unavailable>}" "${package_version:-<unavailable>}" \
        "${package_release:-<unavailable>}" "${package_arch:-<unavailable>}" \
        "$expected_version" "$expected_release" "$expected_arch"
    exit 3
fi
printf '[OK] installed package identity matches magic-mesh-%s-%s.%s\n' \
    "$expected_version" "$expected_release" "$expected_arch"
[[ "$package_install_epoch" =~ ^[1-9][0-9]*$ ]] || {
    printf '[REFUSAL] installed package time is unavailable; process provenance cannot be bounded\n'
    exit 3
}

mackesd_units=(
    mackesd-control.service mackesd-observation.service
    mackesd-actions.service mackesd-data.service
    mackesd-compute.service mackesd-integrations.service
)
main_pids=()
restarts_before=()
for mackesd_unit in "${mackesd_units[@]}"; do
    systemctl is-active --quiet "$mackesd_unit" || {
        printf '[REFUSAL] %s is not active\n' "$mackesd_unit"
        exit 3
    }
    unit_pid="$(systemctl show -p MainPID --value "$mackesd_unit" 2>/dev/null || true)"
    [[ "$unit_pid" =~ ^[1-9][0-9]*$ && -r "/proc/$unit_pid/stat" ]] || {
        printf '[REFUSAL] %s MainPID is unavailable\n' "$mackesd_unit"
        exit 3
    }
    main_pids+=("$unit_pid")
    restarts_before+=("$(systemctl show -p NRestarts --value "$mackesd_unit" 2>/dev/null || true)")
done
main_pid="${main_pids[0]}"

process_exe=''
process_exe_source=''
if command -v readlink >/dev/null 2>&1; then
    process_exe="$(readlink "/proc/$main_pid/exe" 2>/dev/null || true)"
fi
if [[ -n "$process_exe" ]]; then
    [[ "$process_exe" == "$expected_executable" ]] || {
        printf '[REFUSAL] mackesd executable is %s; expected %s (deleted or alternate binaries are not accepted)\n' \
            "$process_exe" "$expected_executable"
        exit 3
    }
    process_exe_source='procfs'
else
    exec_start="$(systemctl show -p ExecStart --value mackesd-control.service 2>/dev/null || true)"
    case "$exec_start" in
        *"path=$expected_executable ;"*)
            process_exe="$expected_executable"
            process_exe_source='systemd-execstart (procfs restricted)'
            ;;
        *)
            printf '[REFUSAL] mackesd executable path is unavailable from procfs and systemd\n'
            exit 3
            ;;
    esac
fi
owner="$(rpm -q --qf '%{NAME}\t%{VERSION}\t%{RELEASE}\t%{ARCH}' -f "$process_exe" 2>/dev/null || true)"
IFS=$'\t' read -r owner_name owner_version owner_release owner_arch <<<"$owner"
if [[ "$owner_name" != magic-mesh || "$owner_version" != "$expected_version" ||
    "$owner_release" != "$expected_release" || "$owner_arch" != "$expected_arch" ]]; then
    printf '[REFUSAL] mackesd executable is owned by %s-%s-%s.%s, not the expected magic-mesh package\n' \
        "${owner_name:-<unavailable>}" "${owner_version:-<unavailable>}" \
        "${owner_release:-<unavailable>}" "${owner_arch:-<unavailable>}"
    exit 3
fi
rpm_digest="$(rpm -q --qf '[%{FILENAMES}\t%{FILEDIGESTS}\n]' magic-mesh 2>/dev/null |
    awk -F '\t' -v path="$expected_executable" '$1 == path { print $2; found = 1; exit } END { if (!found) exit 1 }' || true)"
actual_digest="$(sha256sum "$process_exe" 2>/dev/null | awk '{print $1}' || true)"
if [[ ! "$rpm_digest" =~ ^[0-9a-fA-F]{64}$ || "$actual_digest" != "$rpm_digest" ]]; then
    printf '[REFUSAL] mackesd executable digest does not match the installed RPM file digest\n'
    exit 3
fi
process_start_ticks="$(awk '{ print $22 }' "/proc/$main_pid/stat" 2>/dev/null || true)"
boot_epoch="$(awk '$1 == "btime" { print $2; exit }' /proc/stat 2>/dev/null || true)"
clock_ticks="$(getconf CLK_TCK 2>/dev/null || true)"
if [[ ! "$process_start_ticks" =~ ^[0-9]+$ || ! "$boot_epoch" =~ ^[1-9][0-9]*$ ||
    ! "$clock_ticks" =~ ^[1-9][0-9]*$ ]]; then
    printf '[REFUSAL] mackesd process start provenance is unavailable\n'
    exit 3
fi
process_start_epoch=$((boot_epoch + process_start_ticks / clock_ticks))
if (( process_start_epoch < package_install_epoch )); then
    printf '[REFUSAL] mackesd MainPID %s predates package installation (%s < %s); restart it before claiming this RPM\n' \
        "$main_pid" "$process_start_epoch" "$package_install_epoch"
    exit 3
fi
printf '[OK] mackesd provenance executable=%s source=%s package=magic-mesh-%s-%s.%s install_epoch=%s process_start_epoch=%s\n' \
    "$process_exe" "$process_exe_source" "$expected_version" "$expected_release" \
    "$expected_arch" "$package_install_epoch" "$process_start_epoch"

proc_ticks() {
    awk '{ print $14 + $15 }' "/proc/$1/stat" 2>/dev/null || printf '0\n'
}
group_proc_ticks() {
    local pid total=0 ticks
    for pid in "${main_pids[@]}"; do
        ticks="$(proc_ticks "$pid")"
        total=$((total + ticks))
    done
    printf '%s\n' "$total"
}
total_ticks() {
    awk '$1 == "cpu" { total = 0; for (i = 2; i <= NF; i++) total += $i; print total; exit }' /proc/stat
}
cpu_permille() {
    local process_delta=$1 total_delta=$2 cores=$3
    if (( process_delta < 0 || total_delta <= 0 || cores <= 0 )); then
        printf '%s\n' '-1'
    else
        printf '%s\n' "$(( process_delta * cores * 1000 / total_delta ))"
    fi
}

cores="$(getconf _NPROCESSORS_ONLN 2>/dev/null || true)"
[[ "$cores" =~ ^[1-9][0-9]*$ ]] || {
    printf '[REFUSAL] CPU count unavailable\n'
    exit 3
}
sample_count=$((observe_seconds / sample_interval))
(( sample_count >= 1 )) || sample_count=1
max_seen=0
sum_seen=0
valid=0
for ((sample = 1; sample <= sample_count; sample++)); do
    process_before="$(group_proc_ticks)"
    total_before="$(total_ticks)"
    sleep "$sample_interval"
    process_after="$(group_proc_ticks)"
    total_after="$(total_ticks)"
    process_delta=$((process_after - process_before))
    total_delta=$((total_after - total_before))
    permille="$(cpu_permille "$process_delta" "$total_delta" "$cores")"
    if (( permille < 0 )); then
        printf '[REFUSAL] invalid /proc CPU sample %s\n' "$sample"
        exit 3
    fi
    (( permille > max_seen )) && max_seen=$permille
    sum_seen=$((sum_seen + permille))
    valid=$((valid + 1))
    printf '[sample %d/%d] mackesd_cpu_permille_one_core=%s\n' "$sample" "$sample_count" "$permille"
done

restarts_after=()
main_pids_after=()
for mackesd_unit in "${mackesd_units[@]}"; do
    restarts_after+=("$(systemctl show -p NRestarts --value "$mackesd_unit" 2>/dev/null || true)")
    main_pids_after+=("$(systemctl show -p MainPID --value "$mackesd_unit" 2>/dev/null || true)")
done
mean_seen=$((sum_seen / valid))
printf '[RESULT] pids=%s samples=%s max_permille_one_core=%s mean_permille_one_core=%s restarts=%s->%s\n' \
    "${main_pids[*]}" "$valid" "$max_seen" "$mean_seen" "${restarts_before[*]}" "${restarts_after[*]}"
if [[ "${restarts_before[*]}" != "${restarts_after[*]}" ||
    "${main_pids[*]}" != "${main_pids_after[*]}" ]]; then
    printf '[FAIL] grouped mackesd process identity changed during CPU proof (pids=%s->%s)\n' \
        "${main_pids[*]}" "${main_pids_after[*]}"
    exit 1
fi
if (( max_seen > max_threshold || mean_seen > mean_threshold )); then
    printf '[FAIL] mackesd CPU exceeded threshold (max<=%s‰, mean<=%s‰)\n' \
        "$max_threshold" "$mean_threshold"
    exit 1
fi
printf '[PASS] mackesd CPU stayed within the declared post-install thresholds\n'
REMOTE_SCRIPT
    )" || remote_rc=$?

    printf '%s\n' "$remote_output"
    if (( remote_rc == 0 )); then
        printf '%s@%s: PASS\n' "$USER_" "$host"
        return 0
    elif (( remote_rc == 3 )); then
        printf '%s@%s: REFUSED (read-only proof unavailable or provenance/package identity did not match)\n' \
            "$USER_" "$host" >&2
        return 3
    else
        printf '%s@%s: FAIL (bounded SSH rc=%s)\n' "$USER_" "$host" "$remote_rc" >&2
        return 2
    fi
}

overall_rc=0
for host in "${PROOF_HOSTS[@]}"; do
    host_rc=0
    run_host "$host" || host_rc=$?
    case "$host_rc" in
        2) overall_rc=2 ;;
        1) (( overall_rc == 0 || overall_rc == 3 )) && overall_rc=1 ;;
        3) (( overall_rc == 0 )) && overall_rc=3 ;;
    esac
done

case "$overall_rc" in
    0) printf '%s: PASS (%s seat(s) verified)\n' "$PROGRAM_NAME" "${#PROOF_HOSTS[@]}" ;;
    3) refuse 'one or more post-install CPU proofs were unavailable or failed provenance/package identity checks' ;;
    *) fail "one or more post-install CPU threshold proofs failed (bounded SSH rc=$overall_rc)" ;;
esac
