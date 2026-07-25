#!/usr/bin/env bash
# verify-vehicle-credential-hygiene.sh — read-only MG90 credential hygiene check.
#
# The vehicle worker's preferred contract is:
#   MDE_VEHICLE_GATEWAY=<mg90-ip>
#   MDE_VEHICLE_ROOT_PW_FILE=/etc/mackesd/mg90-root-password
#
# The legacy MDE_VEHICLE_ROOT_PW environment fallback is intentionally rejected
# here because `systemctl cat/show` and crash/debug surfaces can expose it.  This
# helper prints only paths and modes; it never reads or echoes the secret.
set -euo pipefail

SERVICE="${MDE_VEHICLE_SERVICE:-mackesd}"
DEFAULT_PASSWORD_FILE="${MDE_VEHICLE_ROOT_PW_FILE_DEFAULT:-/etc/mackesd/mg90-root-password}"
DEFAULT_KNOWN_HOSTS="${MDE_VEHICLE_KNOWN_HOSTS_DEFAULT:-/etc/mackesd/mg90_known_hosts}"
PACKAGED_KNOWN_HOSTS="${MDE_VEHICLE_KNOWN_HOSTS_PACKAGED:-/usr/share/magic-mesh/mg90-known-hosts}"
REQUIRED_OWNER_UID="${MDE_VEHICLE_REQUIRED_OWNER_UID:-0}"

die() {
    echo "vehicle-credential-hygiene: $*" >&2
    exit 2
}

usage() {
    cat <<'USAGE'
Usage:
  verify-vehicle-credential-hygiene.sh
  verify-vehicle-credential-hygiene.sh --self-test

Checks the live mackesd systemd environment and MG90 credential files without
reading or printing the credential. Fails if the legacy MDE_VEHICLE_ROOT_PW env
value is present.
USAGE
}

environment_lines() {
    if [[ -n "${MCNF_TEST_SYSTEMD_ENV+x}" ]]; then
        tr ' ' '\n' <<<"$MCNF_TEST_SYSTEMD_ENV"
        return
    fi
    command -v systemctl >/dev/null 2>&1 || die "systemctl is required"
    systemctl show -p Environment --value "$SERVICE" | tr ' ' '\n'
}

env_value() {
    local key="$1"
    environment_lines | awk -F= -v key="$key" '$1 == key { print substr($0, length(key) + 2); exit }'
}

root_only_secret_file() {
    local file="$1" mode owner type
    [[ -n "$file" ]] || die "password file path is empty"
    [[ -e "$file" ]] || die "password file is missing: $file"
    [[ ! -L "$file" ]] || die "password file must not be a symlink: $file"
    type="$(stat -c '%F' -- "$file")" || die "cannot stat password file: $file"
    [[ "$type" == regular*file ]] || die "password file is not regular: $file"
    owner="$(stat -c '%u' -- "$file")" || die "cannot stat password owner: $file"
    mode="$(stat -c '%a' -- "$file")" || die "cannot stat password mode: $file"
    [[ "$owner" == "$REQUIRED_OWNER_UID" ]] || die "password file must be owned by uid $REQUIRED_OWNER_UID: $file"
    case "$mode" in
        400|600) ;;
        *) die "password file must be mode 0400 or 0600, got $mode: $file" ;;
    esac
}

root_owned_pin_file() {
    local file="$1" mode owner type
    [[ -n "$file" ]] || die "known-hosts path is empty"
    [[ -e "$file" ]] || die "known-hosts pin is missing: $file"
    [[ ! -L "$file" ]] || die "known-hosts pin must not be a symlink: $file"
    type="$(stat -c '%F' -- "$file")" || die "cannot stat known-hosts pin: $file"
    [[ "$type" == regular*file ]] || die "known-hosts pin is not regular: $file"
    owner="$(stat -c '%u' -- "$file")" || die "cannot stat known-hosts owner: $file"
    mode="$(stat -c '%a' -- "$file")" || die "cannot stat known-hosts mode: $file"
    [[ "$owner" == "$REQUIRED_OWNER_UID" ]] || die "known-hosts pin must be owned by uid $REQUIRED_OWNER_UID: $file"
    case "$mode" in
        *2|*3|*6|*7) die "known-hosts pin must not be group/world writable, got $mode: $file" ;;
    esac
}

run_check() {
    local gateway legacy password_file known_hosts
    gateway="$(env_value MDE_VEHICLE_GATEWAY)"
    legacy="$(env_value MDE_VEHICLE_ROOT_PW)"
    password_file="$(env_value MDE_VEHICLE_ROOT_PW_FILE)"
    [[ -z "$legacy" ]] || die "legacy MDE_VEHICLE_ROOT_PW is set; move the secret to a root-only file"

    if [[ -z "$gateway" ]]; then
        echo "vehicle-credential-hygiene: ok — no MDE_VEHICLE_GATEWAY configured"
        return 0
    fi

    [[ -n "$password_file" ]] || password_file="$DEFAULT_PASSWORD_FILE"
    root_only_secret_file "$password_file"

    if [[ -f "$DEFAULT_KNOWN_HOSTS" ]]; then
        known_hosts="$DEFAULT_KNOWN_HOSTS"
    else
        known_hosts="$PACKAGED_KNOWN_HOSTS"
    fi
    root_owned_pin_file "$known_hosts"

    echo "vehicle-credential-hygiene: ok — gateway configured; password_file=$password_file; known_hosts=$known_hosts"
}

self_test() {
    local test_dir good_pw bad_pw pin output rc
    test_dir="$(mktemp -d)"
    trap 'rm -rf -- "$test_dir"' RETURN
    good_pw="$test_dir/mg90-root-password"
    bad_pw="$test_dir/world-readable-password"
    pin="$test_dir/mg90_known_hosts"
    install -m 0600 /dev/null "$good_pw"
    install -m 0644 /dev/null "$bad_pw"
    install -m 0644 /dev/null "$pin"

    DEFAULT_KNOWN_HOSTS="$pin"
    PACKAGED_KNOWN_HOSTS="$pin"
    REQUIRED_OWNER_UID="$(id -u)"

    MCNF_TEST_SYSTEMD_ENV="MDE_VEHICLE_GATEWAY=172.20.0.25 MDE_VEHICLE_ROOT_PW_FILE=$good_pw" \
        run_check >/dev/null

    set +e
    output="$(
        MCNF_TEST_SYSTEMD_ENV="MDE_VEHICLE_GATEWAY=172.20.0.25 MDE_VEHICLE_ROOT_PW=do-not-print-me" \
            run_check 2>&1
    )"
    rc=$?
    set -e
    [[ "$rc" -ne 0 ]] || die "self-test accepted legacy password env"
    [[ "$output" != *"do-not-print-me"* ]] || die "self-test leaked the legacy secret"

    set +e
    (
        MCNF_TEST_SYSTEMD_ENV="MDE_VEHICLE_GATEWAY=172.20.0.25 MDE_VEHICLE_ROOT_PW_FILE=$bad_pw" \
            run_check >/dev/null 2>&1
    )
    rc=$?
    set -e
    [[ "$rc" -ne 0 ]] || die "self-test accepted world-readable password file"

    echo "vehicle-credential-hygiene: self-test passed"
}

case "${1:-}" in
    --self-test) self_test ;;
    -h|--help) usage ;;
    "") run_check ;;
    *) usage >&2; exit 2 ;;
esac
