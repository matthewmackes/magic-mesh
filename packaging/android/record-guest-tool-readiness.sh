#!/usr/bin/env bash
# WL-FUNC-020 — publish tooling readiness from inside the nested Cuttlefish host.
#
# This receipt is deliberately narrower than Android guest readiness: it proves
# only that the fixed cvd/adb tools answered and that the nested host can open
# /dev/kvm. It does not start Cuttlefish, inspect packages, or claim a live
# Android display/session. The placement verifier consumes this receipt and
# still reports live Android proof as unavailable.
set -euo pipefail
umask 077

readonly RECEIPT_KIND="cuttlefish_guest_tool_readiness"
readonly SCHEMA_VERSION=1
readonly MAX_ID_BYTES=128
readonly MAX_VERSION_BYTES=128
readonly MAX_PATH_BYTES=512
readonly VERSION_RE='^[A-Za-z0-9][A-Za-z0-9._+:/ -]{0,127}$'
readonly DIGEST_RE='^sha256:[0-9a-f]{64}$'
readonly ID_RE='^[A-Za-z0-9][A-Za-z0-9_.:-]{0,127}$'

usage() {
    cat <<'EOF'
Usage:
  record-guest-tool-readiness.sh --workload-id ID --image-digest DIGEST --output PATH
  record-guest-tool-readiness.sh --self-test

Run inside the nested Debian Cuttlefish host after its fixed tools are installed.
Exit 0 writes a tooling-readiness receipt; exit 3 prints unavailable when cvd,
adb, timeout, or /dev/kvm is absent. This command never starts or claims a live
Android guest.
EOF
}

unavailable() {
    local reason=$1
    printf '{"schema_version":%d,"kind":"%s","status":"unavailable","reason":"%s","live_android_guest_proof":"unavailable"}\n' \
        "$SCHEMA_VERSION" "$RECEIPT_KIND" "$reason"
    exit 3
}

valid_identity() {
    [[ "$1" =~ $ID_RE && "${#1}" -le "$MAX_ID_BYTES" ]]
}

valid_digest() {
    [[ "$1" =~ $DIGEST_RE && "$1" != "sha256:$(printf '%064d' 0)" ]]
}

valid_path() {
    local path=$1 component
    [[ "$path" == /* && "${#path}" -le "$MAX_PATH_BYTES" ]] || return 1
    IFS='/' read -r -a components <<<"$path"
    for component in "${components[@]}"; do
        [[ "$component" != .. ]] || return 1
        [[ "$component" != *$'\n'* && "$component" != *$'\r'* ]] || return 1
    done
}

tool_version() {
    local tool=$1 output first_line
    command -v "$tool" >/dev/null 2>&1 || return 1
    output=$(timeout --signal=KILL 5s "$tool" version 2>/dev/null) || return 1
    first_line=$(printf '%s\n' "$output" | sed -n '1p')
    [[ -n "$first_line" && "${#first_line}" -le "$MAX_VERSION_BYTES" ]] || return 1
    [[ "$first_line" =~ $VERSION_RE ]] || return 1
    printf '%s' "$first_line"
}

write_receipt() {
    local workload_id=$1 image_digest=$2 cvd_version=$3 adb_version=$4 output=$5 now tmp
    now=$(date -u '+%Y-%m-%dT%H:%M:%SZ') || unavailable timestamp_unavailable
    tmp="${output}.tmp.$$"
    if [[ -e "$output" && -L "$output" ]]; then
        echo "record-guest-tool-readiness: refusing symlinked output: $output" >&2
        exit 2
    fi
    printf '{"schema_version":%d,"kind":"%s","workload_id":"%s","image_digest":"%s","cvd_version":"%s","adb_version":"%s","kvm_access":true,"recorded_at":"%s"}\n' \
        "$SCHEMA_VERSION" "$RECEIPT_KIND" "$workload_id" "$image_digest" \
        "$cvd_version" "$adb_version" "$now" >"$tmp" || exit 1
    chmod 0600 "$tmp"
    mv -f -- "$tmp" "$output"
    printf '%s\n' "$(<"$output")"
}

self_test() {
    local fixture bin output receipt workload digest
    fixture=$(mktemp -d)
    trap 'if [[ -n ${fixture:-} ]]; then rm -rf -- "$fixture"; fi' EXIT
    bin="$fixture/bin"
    mkdir -p "$bin"
    cat >"$bin/cvd" <<'EOF'
#!/usr/bin/env bash
printf 'Cuttlefish host 2026.08.1\n'
EOF
    cat >"$bin/adb" <<'EOF'
#!/usr/bin/env bash
printf 'Android Debug Bridge version 1.0.41\n'
EOF
    cat >"$bin/timeout" <<'EOF'
#!/usr/bin/env bash
shift 2
exec "$@"
EOF
    chmod 0755 "$bin/cvd" "$bin/adb" "$bin/timeout"
    output="$fixture/receipt.json"
    workload="android-seat-15"
    digest="sha256:$(printf 'a%.0s' {1..64})"
    PATH="$bin:/usr/bin:/bin" \
        MCNF_CUTTLEFISH_SELF_TEST=1 \
        MCNF_CUTTLEFISH_KVM_DEVICE=/dev/null \
        "$0" --workload-id "$workload" --image-digest "$digest" --output "$output" >/dev/null
    [[ -f "$output" && ! -L "$output" && "$(stat -c '%a' "$output")" == 600 ]]
    receipt=$(<"$output")
    [[ "$receipt" == *'"kind":"cuttlefish_guest_tool_readiness"'* ]]
    [[ "$receipt" == *'"kvm_access":true'* ]]
    if PATH="$bin:/usr/bin:/bin" MCNF_CUTTLEFISH_SELF_TEST=1 \
        MCNF_CUTTLEFISH_KVM_DEVICE=/dev/null \
        "$0" --workload-id "$workload" \
        --image-digest "sha256:$(printf '%064d' 0)" --output "$output" >/dev/null 2>&1; then
        echo "record-guest-tool-readiness: self-test accepted the null digest" >&2
        return 1
    fi
    valid_identity "$workload"
    if valid_identity 'android seat'; then
        return 1
    fi
    valid_digest "$digest"
    if valid_digest "sha256:$(printf '%064d' 0)"; then
        return 1
    fi
    rm -rf -- "$fixture"
    trap - EXIT
    echo "record-guest-tool-readiness: self-test passed"
}

if [[ "${1:-}" == "--self-test" ]]; then
    [[ "$#" -eq 1 ]] || { usage >&2; exit 2; }
    self_test
    exit 0
fi

workload_id=''
image_digest=''
output=''
while [[ "$#" -gt 0 ]]; do
    case "$1" in
        --workload-id) [[ "$#" -ge 2 ]] || { usage >&2; exit 2; }; workload_id=$2; shift 2 ;;
        --image-digest) [[ "$#" -ge 2 ]] || { usage >&2; exit 2; }; image_digest=$2; shift 2 ;;
        --output) [[ "$#" -ge 2 ]] || { usage >&2; exit 2; }; output=$2; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) usage >&2; exit 2 ;;
    esac
done

valid_identity "$workload_id" || { echo "record-guest-tool-readiness: invalid workload identity" >&2; exit 2; }
valid_digest "$image_digest" || { echo "record-guest-tool-readiness: invalid image digest" >&2; exit 2; }
valid_path "$output" || { echo "record-guest-tool-readiness: invalid output path" >&2; exit 2; }
[[ -d "$(dirname -- "$output")" ]] || { echo "record-guest-tool-readiness: output directory is missing" >&2; exit 2; }
kvm_device=/dev/kvm
if [[ "${MCNF_CUTTLEFISH_SELF_TEST:-0}" == 1 ]]; then
    kvm_device=${MCNF_CUTTLEFISH_KVM_DEVICE:-/dev/null}
fi
[[ -c "$kvm_device" ]] || unavailable kvm_missing
if ! { exec {kvm_fd}<>"$kvm_device"; } 2>/dev/null; then
    unavailable kvm_access_denied
fi
exec {kvm_fd}>&-
command -v timeout >/dev/null 2>&1 || unavailable timeout_missing

cvd_version=$(tool_version cvd) || unavailable cvd_unavailable
adb_version=$(tool_version adb) || unavailable adb_unavailable
write_receipt "$workload_id" "$image_digest" "$cvd_version" "$adb_version" "$output"
