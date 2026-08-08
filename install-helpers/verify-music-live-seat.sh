#!/usr/bin/env bash
# Bounded, read-only Music checks for the canonical non-production seat.
# Defaults: 172.20.0.15 / mm. Override MUSIC_LIVE_HOST, MUSIC_LIVE_USER, and
# MUSIC_LIVE_SSH_KEY for another approved seat. --play-probe is explicit because
# it starts playback; it is bounded and disabled by default.
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd -P)"
HOST="${MUSIC_LIVE_HOST:-172.20.0.15}"
USER_="${MUSIC_LIVE_USER:-mm}"
SSH_KEY="${MUSIC_LIVE_SSH_KEY:-${MCNF_FARM_KEY:-$HOME/.ssh/mackes_mesh_ed25519}}"
BUS_ROOT="${MUSIC_LIVE_BUS_ROOT:-/run/mde-bus}"
SSH_TIMEOUT="${MUSIC_LIVE_SSH_TIMEOUT_SECONDS:-45}"
COMMAND_TIMEOUT="${MUSIC_LIVE_COMMAND_TIMEOUT_SECONDS:-8}"
PLAY_TIMEOUT="${MUSIC_LIVE_PLAY_TIMEOUT_SECONDS:-15}"
PLAY_SONG_ID=""
readonly EXPECTED_PACKAGE_ARCH='x86_64'
readonly PLAY_DISABLED_SENTINEL='__music_play_probe_disabled__'

usage() {
    cat >&2 <<'EOF'
usage: verify-music-live-seat.sh [--play-probe SONG_ID] [--self-test]

Default checks: mde-musicd.service health, mde-musicd ping,
action/music/get-state, action/music/list-albums, and the installed
magic-mesh RPM payload. All default checks are bounded and read-only.
--play-probe SONG_ID explicitly starts a bounded playback probe.

Environment: MUSIC_LIVE_HOST, MUSIC_LIVE_USER, MUSIC_LIVE_SSH_KEY,
MUSIC_LIVE_BUS_ROOT, MUSIC_LIVE_SSH_TIMEOUT_SECONDS (1..120),
MUSIC_LIVE_COMMAND_TIMEOUT_SECONDS (1..30), and
MUSIC_LIVE_PLAY_TIMEOUT_SECONDS (1..30).
EOF
}

fail() { printf '[FAIL] %s\n' "$1" >&2; exit 2; }

declared_release_version() {
    local cargo_toml="$REPO_ROOT/Cargo.toml"
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
    local cargo_toml="$REPO_ROOT/crates/mesh/mackesd/Cargo.toml"
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

validate_declared_release_version() {
    [[ "$1" =~ ^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$ ]]
}

validate_declared_rpm_release() {
    [[ "$1" =~ ^[0-9][0-9A-Za-z._+~-]*$ ]]
}

bounded_integer() {
    local value=$1 minimum=$2 maximum=$3
    [[ "$value" =~ ^[0-9]+$ ]] && (( value >= minimum && value <= maximum ))
}

validate_reply() {
    local kind=$1
    python3 -c '
import json, sys
kind = sys.argv[1]
try:
    value = json.load(sys.stdin)
except (json.JSONDecodeError, TypeError) as exc:
    raise SystemExit(f"invalid JSON reply ({exc.__class__.__name__})")
if isinstance(value, dict) and isinstance(value.get("body"), str):
    try:
        value = json.loads(value["body"])
    except json.JSONDecodeError as exc:
        raise SystemExit(f"invalid JSON reply body ({exc.__class__.__name__})")
if not isinstance(value, dict) or value.get("ok") is False:
    raise SystemExit("Music reply is not successful JSON")
if kind == "state":
    if not any(k in value for k in ("active", "playing", "audio_available")):
        raise SystemExit("get-state has no state fields")
elif kind == "albums":
    if "albums" not in value and not (isinstance(value.get("result"), dict) and "albums" in value["result"]):
        raise SystemExit("list-albums has no albums field")
else:
    raise SystemExit("unknown reply kind")
' "$kind"
}

validate_package_payload() {
    local package_name=$1 payload=$2
    [[ "$package_name" == 'magic-mesh' ||
        "$package_name" =~ ^magic-mesh-[[:alnum:].:+_-]+$ ]] || return 1
    grep -Fqx '/usr/bin/mde-musicd' <<<"$payload" || return 1
    grep -Fqx '/usr/bin/mde-shell-egui' <<<"$payload"
}

validate_package_identity() {
    # The RPM query is deliberately fielded instead of matching a free-form
    # NEVRA string: VERSION is the platform release, while RELEASE remains a
    # packaging iteration.  This binds the installed Music payload to the
    # checked-out release authority without inventing another version source.
    local identity=$1 expected_version=$2 expected_rpm_release=$3 expected_arch=$4
    local -a fields=()
    [[ -n "$identity" && -n "$expected_version" && -n "$expected_rpm_release" &&
        -n "$expected_arch" ]] || return 1
    [[ "$identity" != *$'\n'* && "$identity" != *$'\r'* ]] || return 1
    IFS=$'\t' read -r -a fields <<<"$identity"
    (( ${#fields[@]} == 4 )) || return 1
    [[ "${fields[0]}" == 'magic-mesh' ]] || return 1
    [[ "${fields[1]}" == "$expected_version" ]] || return 1
    [[ "${fields[2]}" == "$expected_rpm_release" ]] || return 1
    [[ "${fields[3]}" == "$expected_arch" ]]
}

package_identity_summary() {
    local identity=$1
    local -a fields=()
    if [[ -n "$identity" && "$identity" != *$'\n'* && "$identity" != *$'\r'* ]]; then
        IFS=$'\t' read -r -a fields <<<"$identity"
        if (( ${#fields[@]} == 4 )) &&
            [[ "${fields[0]}" =~ ^[[:alnum:]_.+-]+$ &&
                "${fields[1]}" =~ ^[[:alnum:].+-]+$ &&
                "${fields[2]}" =~ ^[[:alnum:]_.+~-]+$ &&
                "${fields[3]}" =~ ^[[:alnum:]_.+-]+$ ]]; then
            printf '%s-%s-%s.%s' "${fields[0]}" "${fields[1]}" "${fields[2]}" "${fields[3]}"
            return 0
        fi
    fi
    printf '<unavailable>'
}

diagnose_package_identity() {
    local identity=$1 expected_version=$2 expected_rpm_release=$3 expected_arch=$4
    local -a fields=()
    local expected_artifact="magic-mesh-${expected_version}-${expected_rpm_release}.${expected_arch}"
    local actual
    actual="$(package_identity_summary "$identity")"
    if [[ "$actual" == '<unavailable>' ]]; then
        printf '[FAIL] installed RPM identity is unavailable or malformed; expected artifact %s\n' \
            "$expected_artifact"
        return 1
    fi
    IFS=$'\t' read -r -a fields <<<"$identity"
    if [[ "${fields[0]}" != 'magic-mesh' ]]; then
        printf '[FAIL] installed RPM name is %s; expected magic-mesh from artifact %s\n' \
            "$actual" "$expected_artifact"
        return 1
    fi
    if [[ "${fields[1]}" != "$expected_version" ]]; then
        printf '[FAIL] installed RPM version is %s; expected VERSION=%s from artifact %s\n' \
            "$actual" "$expected_version" "$expected_artifact"
        return 1
    fi
    if [[ "${fields[2]}" != "$expected_rpm_release" ]]; then
        printf '[FAIL] installed RPM release is %s; expected RELEASE=%s from artifact %s; install that release before rerunning\n' \
            "$actual" "$expected_rpm_release" "$expected_artifact"
        return 1
    fi
    printf '[FAIL] installed RPM architecture is %s; expected ARCH=%s from artifact %s\n' \
        "$actual" "$expected_arch" "$expected_artifact"
    return 1
}

diagnose_package_payload() {
    local payload=$1
    local -a missing=()
    grep -Fqx '/usr/bin/mde-musicd' <<<"$payload" || missing+=(/usr/bin/mde-musicd)
    grep -Fqx '/usr/bin/mde-shell-egui' <<<"$payload" || missing+=(/usr/bin/mde-shell-egui)
    if ((${#missing[@]} == 0)); then
        return 0
    fi
    printf '[FAIL] installed magic-mesh payload is missing required path(s): %s; expected Music and shell assets from the release artifact\n' \
        "${missing[*]}"
    return 1
}

diagnose_package_verification() {
    local verification_rc=$1 verification=$2 line unexpected=0
    if (( verification_rc == 0 )) && [[ -z "$verification" ]]; then
        return 0
    fi
    while IFS= read -r line; do
        case "$line" in
            ''|'S.5....T.    /opt/mcnf/automation/secrets/mcnf-secret.sh') ;;
            *) unexpected=$((unexpected + 1)) ;;
        esac
    done <<<"$verification"
    if (( verification_rc == 1 && unexpected == 0 )); then
        return 0
    fi
    printf '[FAIL] rpm -V magic-mesh reported %s unexpected installed-file difference(s) (rc=%s); inspect the seat before treating payload proof as valid\n' \
        "$unexpected" "$verification_rc"
    return 1
}

validate_package_verification() {
    # rpm -ql proves only that the package manifest names these paths.  Require
    # rpm -V to report no unexpected differences so missing or modified
    # installed files fail closed.  The secret helper is intentionally
    # provisioned/rewritten at runtime and is the sole package-owned mutable
    # path allowed here; its contents are never printed or inspected.
    local verification_rc=$1 verification=$2 line
    [[ "$verification_rc" =~ ^[0-9]+$ ]] || return 1
    if (( verification_rc == 0 )); then
        [[ -z "$verification" ]]
        return
    fi
    (( verification_rc == 1 )) && [[ -n "$verification" ]] || return 1
    while IFS= read -r line; do
        case "$line" in
            'S.5....T.    /opt/mcnf/automation/secrets/mcnf-secret.sh') ;;
            *) return 1 ;;
        esac
    done <<<"$verification"
}

validate_config() {
    [[ -n "$HOST" && -n "$USER_" && -n "$SSH_KEY" && -n "$BUS_ROOT" ]] ||
        fail 'host, user, SSH key, and Bus root must be non-empty'
    bounded_integer "$SSH_TIMEOUT" 1 120 || fail 'SSH timeout must be 1..120 seconds'
    bounded_integer "$COMMAND_TIMEOUT" 1 30 || fail 'command timeout must be 1..30 seconds'
    bounded_integer "$PLAY_TIMEOUT" 1 30 || fail 'play timeout must be 1..30 seconds'
    if [[ -n "$PLAY_SONG_ID" ]]; then
        [[ "$PLAY_SONG_ID" =~ ^[A-Za-z0-9._:-]{1,128}$ ]] ||
            fail 'play probe song id contains unsupported characters'
    fi
}

self_test() {
    local fixture expected_version expected_rpm_release tab
    tab=$'\t'
    expected_version="$(declared_release_version)" ||
        fail 'could not read [workspace.package].version from root Cargo.toml'
    validate_declared_release_version "$expected_version" ||
        fail 'root Cargo.toml declares an invalid release version'
    expected_rpm_release="$(declared_rpm_release)" ||
        fail 'could not read the base RPM release from mackesd Cargo.toml'
    validate_declared_rpm_release "$expected_rpm_release" ||
        fail 'mackesd Cargo.toml declares an invalid base RPM release'
    bounded_integer 1 1 120
    bounded_integer 30 1 30
    if bounded_integer 0 1 30; then fail 'self-test accepted a value below the timeout minimum'; fi
    if bounded_integer 31 1 30; then fail 'self-test accepted a value above the timeout maximum'; fi
    if bounded_integer nope 1 30; then fail 'self-test accepted a non-numeric timeout'; fi
    fixture='{"ok":true,"active":false,"playing":false,"audio_available":true}'
    printf '%s' "$fixture" | validate_reply state
    fixture='{"body":"{\"ok\":true,\"albums\":[]}"}'
    printf '%s' "$fixture" | validate_reply albums
    if printf '%s' '{"ok":false}' | validate_reply state >/dev/null 2>&1; then
        fail 'self-test accepted an unsuccessful reply'
    fi
    fixture=$'/usr/bin/mde-musicd\n/usr/bin/mde-shell-egui\n'
    validate_package_payload "magic-mesh-${expected_version}-${expected_rpm_release}.x86_64" "$fixture"
    if validate_package_payload 'other-package-1.0-1.x86_64' "$fixture"; then
        fail 'self-test accepted a non-magic-mesh package'
    fi
    if validate_package_payload "magic-mesh-${expected_version}-${expected_rpm_release}.x86_64" '/usr/bin/mde-musicd'; then
        fail 'self-test accepted a package missing the shell payload'
    fi
    validate_package_identity "magic-mesh${tab}${expected_version}${tab}${expected_rpm_release}${tab}${EXPECTED_PACKAGE_ARCH}" "$expected_version" "$expected_rpm_release" "$EXPECTED_PACKAGE_ARCH"
    if validate_package_identity "magic-mesh${tab}${expected_version}.1${tab}${expected_rpm_release}${tab}${EXPECTED_PACKAGE_ARCH}" "$expected_version" "$expected_rpm_release" "$EXPECTED_PACKAGE_ARCH"; then
        fail 'self-test accepted an installed package with the wrong platform version'
    fi
    if validate_package_identity "magic-mesh${tab}${expected_version}${tab}4${tab}${EXPECTED_PACKAGE_ARCH}" "$expected_version" "$expected_rpm_release" "$EXPECTED_PACKAGE_ARCH"; then
        fail 'self-test accepted an installed package with the wrong RPM release'
    fi
    if validate_package_identity "magic-mesh${tab}${expected_version}${tab}${expected_rpm_release}${tab}${EXPECTED_PACKAGE_ARCH}" "$expected_version" '' "$EXPECTED_PACKAGE_ARCH"; then
        fail 'self-test accepted an incomplete installed package identity'
    fi
    diagnostic=''
    diagnostic="$(diagnose_package_identity "magic-mesh${tab}${expected_version}${tab}4${tab}${EXPECTED_PACKAGE_ARCH}" "$expected_version" "$expected_rpm_release" "$EXPECTED_PACKAGE_ARCH")" || true
    grep -Fq "installed RPM release is magic-mesh-${expected_version}-4.${EXPECTED_PACKAGE_ARCH}; expected RELEASE=${expected_rpm_release}" <<<"$diagnostic" ||
        fail 'self-test did not identify the installed release-4 versus expected release-5 mismatch'
    diagnostic="$(diagnose_package_payload '/usr/bin/mde-musicd')" || true
    grep -Fq '/usr/bin/mde-shell-egui' <<<"$diagnostic" ||
        fail 'self-test did not identify the missing shell payload path'
    diagnose_package_verification 0 ''
    diagnose_package_verification 1 \
        'S.5....T.    /opt/mcnf/automation/secrets/mcnf-secret.sh'
    validate_package_verification 0 ''
    if validate_package_verification 0 'missing /usr/bin/mde-musicd'; then
        fail 'self-test accepted a non-clean installed package verification'
    fi
    validate_package_verification 1 \
        'S.5....T.    /opt/mcnf/automation/secrets/mcnf-secret.sh'
    if validate_package_verification 1 $'S.5....T.    /opt/mcnf/automation/secrets/mcnf-secret.sh\nmissing /usr/bin/mde-musicd'; then
        fail 'self-test accepted an unexpected package verification difference'
    fi
    if validate_package_verification 2 \
        'S.5....T.    /opt/mcnf/automation/secrets/mcnf-secret.sh'; then
        fail 'self-test accepted an unexpected rpm verification exit code'
    fi
    printf 'verify-music-live-seat: self-test passed (no SSH attempted)\n'
}

if [[ "${1:-}" == '--self-test' ]]; then
    [[ "$#" == 1 ]] || fail '--self-test takes no additional arguments'
    self_test
    exit 0
fi
if [[ "${1:-}" == '--help' || "${1:-}" == '-h' ]]; then usage; exit 0; fi

while (($#)); do
    case $1 in
        --play-probe)
            [[ $# -ge 2 ]] || fail '--play-probe requires SONG_ID'
            PLAY_SONG_ID=$2
            shift 2
            ;;
        *) usage; fail "unknown argument: $1" ;;
    esac
done

validate_config
RELEASE_VERSION="$(declared_release_version)" ||
    fail 'could not read the declared platform release from root Cargo.toml'
validate_declared_release_version "$RELEASE_VERSION" ||
    fail 'root Cargo.toml declares an invalid platform release version'
RPM_RELEASE="$(declared_rpm_release)" ||
    fail 'could not read the declared base RPM release from mackesd Cargo.toml'
validate_declared_rpm_release "$RPM_RELEASE" ||
    fail 'mackesd Cargo.toml declares an invalid base RPM release'
command -v ssh >/dev/null 2>&1 || fail 'ssh is required'
command -v timeout >/dev/null 2>&1 || fail 'timeout is required'
command -v python3 >/dev/null 2>&1 || fail 'python3 is required for reply checks'
[[ -r "$SSH_KEY" ]] || fail 'configured SSH key is unavailable'

printf '== Music live seat verification (%s@%s) ==\n' "$USER_" "$HOST"
printf '[INFO] declared platform release: %s\n' "$RELEASE_VERSION"
printf '[INFO] declared base RPM release: %s\n' "$RPM_RELEASE"
remote_output=''
remote_rc=0
play_song_arg="${PLAY_SONG_ID:-$PLAY_DISABLED_SENTINEL}"
remote_output="$(
    timeout --signal=TERM --kill-after=3s "${SSH_TIMEOUT}s" \
        ssh -i "$SSH_KEY" -o BatchMode=yes -o ConnectTimeout=10 \
        -o ServerAliveInterval=5 -o ServerAliveCountMax=1 \
        -o StrictHostKeyChecking=accept-new "$USER_@$HOST" bash -s -- \
        "$play_song_arg" "$BUS_ROOT" "$COMMAND_TIMEOUT" "$PLAY_TIMEOUT" \
        "$RELEASE_VERSION" "$RPM_RELEASE" "$EXPECTED_PACKAGE_ARCH" \
        2>/dev/null <<'REMOTE_SCRIPT'
set -euo pipefail
play_song_id=$1; bus_root=$2; command_timeout=$3; play_timeout=$4; expected_version=$5; expected_rpm_release=$6; expected_arch=$7
uid="$(id -u)"; failed=0

validate_reply() {
    local kind=$1
    python3 -c '
import json, sys
kind = sys.argv[1]
try: value = json.load(sys.stdin)
except (json.JSONDecodeError, TypeError) as exc: raise SystemExit(str(exc))
if isinstance(value, dict) and isinstance(value.get("body"), str): value = json.loads(value["body"])
if not isinstance(value, dict) or value.get("ok") is False: raise SystemExit("unsuccessful reply")
if kind == "state" and not any(k in value for k in ("active", "playing", "audio_available")): raise SystemExit("missing state")
if kind == "albums" and "albums" not in value and not (isinstance(value.get("result"), dict) and "albums" in value["result"]): raise SystemExit("missing albums")
' "$kind"
}

validate_package_payload() {
    local package_name=$1 payload=$2
    [[ "$package_name" == 'magic-mesh' ||
        "$package_name" =~ ^magic-mesh-[[:alnum:].:+_-]+$ ]] || return 1
    grep -Fqx '/usr/bin/mde-musicd' <<<"$payload" || return 1
    grep -Fqx '/usr/bin/mde-shell-egui' <<<"$payload"
}

validate_package_identity() {
    local identity=$1 expected_version=$2 expected_rpm_release=$3 expected_arch=$4
    local -a fields=()
    [[ -n "$identity" && -n "$expected_version" && -n "$expected_rpm_release" &&
        -n "$expected_arch" ]] || return 1
    [[ "$identity" != *$'\n'* && "$identity" != *$'\r'* ]] || return 1
    IFS=$'\t' read -r -a fields <<<"$identity"
    (( ${#fields[@]} == 4 )) || return 1
    [[ "${fields[0]}" == 'magic-mesh' ]] || return 1
    [[ "${fields[1]}" == "$expected_version" ]] || return 1
    [[ "${fields[2]}" == "$expected_rpm_release" ]] || return 1
    [[ "${fields[3]}" == "$expected_arch" ]]
}

package_identity_summary() {
    local identity=$1
    local -a fields=()
    if [[ -n "$identity" && "$identity" != *$'\n'* && "$identity" != *$'\r'* ]]; then
        IFS=$'\t' read -r -a fields <<<"$identity"
        if (( ${#fields[@]} == 4 )) &&
            [[ "${fields[0]}" =~ ^[[:alnum:]_.+-]+$ &&
                "${fields[1]}" =~ ^[[:alnum:].+-]+$ &&
                "${fields[2]}" =~ ^[[:alnum:]_.+~-]+$ &&
                "${fields[3]}" =~ ^[[:alnum:]_.+-]+$ ]]; then
            printf '%s-%s-%s.%s' "${fields[0]}" "${fields[1]}" "${fields[2]}" "${fields[3]}"
            return 0
        fi
    fi
    printf '<unavailable>'
}

diagnose_package_identity() {
    local identity=$1 expected_version=$2 expected_rpm_release=$3 expected_arch=$4
    local -a fields=()
    local expected_artifact="magic-mesh-${expected_version}-${expected_rpm_release}.${expected_arch}"
    local actual
    actual="$(package_identity_summary "$identity")"
    if [[ "$actual" == '<unavailable>' ]]; then
        printf '[FAIL] installed RPM identity is unavailable or malformed; expected artifact %s\n' \
            "$expected_artifact"
        return 1
    fi
    IFS=$'\t' read -r -a fields <<<"$identity"
    if [[ "${fields[0]}" != 'magic-mesh' ]]; then
        printf '[FAIL] installed RPM name is %s; expected magic-mesh from artifact %s\n' \
            "$actual" "$expected_artifact"
        return 1
    fi
    if [[ "${fields[1]}" != "$expected_version" ]]; then
        printf '[FAIL] installed RPM version is %s; expected VERSION=%s from artifact %s\n' \
            "$actual" "$expected_version" "$expected_artifact"
        return 1
    fi
    if [[ "${fields[2]}" != "$expected_rpm_release" ]]; then
        printf '[FAIL] installed RPM release is %s; expected RELEASE=%s from artifact %s; install that release before rerunning\n' \
            "$actual" "$expected_rpm_release" "$expected_artifact"
        return 1
    fi
    printf '[FAIL] installed RPM architecture is %s; expected ARCH=%s from artifact %s\n' \
        "$actual" "$expected_arch" "$expected_artifact"
    return 1
}

diagnose_package_payload() {
    local payload=$1
    local -a missing=()
    grep -Fqx '/usr/bin/mde-musicd' <<<"$payload" || missing+=(/usr/bin/mde-musicd)
    grep -Fqx '/usr/bin/mde-shell-egui' <<<"$payload" || missing+=(/usr/bin/mde-shell-egui)
    if ((${#missing[@]} == 0)); then
        return 0
    fi
    printf '[FAIL] installed magic-mesh payload is missing required path(s): %s; expected Music and shell assets from the release artifact\n' \
        "${missing[*]}"
    return 1
}

validate_declared_release_version() {
    [[ "$1" =~ ^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$ ]]
}

validate_declared_rpm_release() {
    [[ "$1" =~ ^[0-9][0-9A-Za-z._+~-]*$ ]]
}

if ! validate_declared_release_version "$expected_version"; then
    printf '[FAIL] invalid declared platform release supplied to seat probe\n'
    exit 1
fi
if ! validate_declared_rpm_release "$expected_rpm_release"; then
    printf '[FAIL] invalid declared base RPM release supplied to seat probe\n'
    exit 1
fi

validate_package_verification() {
    local verification_rc=$1 verification=$2 line
    [[ "$verification_rc" =~ ^[0-9]+$ ]] || return 1
    if (( verification_rc == 0 )); then
        [[ -z "$verification" ]]
        return
    fi
    (( verification_rc == 1 )) && [[ -n "$verification" ]] || return 1
    while IFS= read -r line; do
        case "$line" in
            'S.5....T.    /opt/mcnf/automation/secrets/mcnf-secret.sh') ;;
            *) return 1 ;;
        esac
    done <<<"$verification"
}

musicd_runtime_identity() {
    local main_pid executable owner
    main_pid="$(timeout --signal=TERM --kill-after=2s "${command_timeout}s" \
        systemctl --user show mde-musicd.service -p MainPID --value 2>/dev/null || true)"
    [[ "$main_pid" =~ ^[1-9][0-9]*$ ]] || return 1
    executable="$(timeout --signal=TERM --kill-after=2s "${command_timeout}s" \
        readlink -f -- "/proc/$main_pid/exe" 2>/dev/null || true)"
    [[ "$executable" == '/usr/bin/mde-musicd' ]] || return 1
    owner="$(timeout --signal=TERM --kill-after=2s "${command_timeout}s" \
        rpm -qf --qf '%{NAME}\t%{VERSION}\t%{RELEASE}\t%{ARCH}' \
        "$executable" 2>/dev/null || true)"
    validate_package_identity "$owner" "$expected_version" "$expected_rpm_release" "$expected_arch"
}

diagnose_package_verification() {
    local verification_rc=$1 verification=$2 line unexpected=0
    if (( verification_rc == 0 )) && [[ -z "$verification" ]]; then
        return 0
    fi
    while IFS= read -r line; do
        case "$line" in
            ''|'S.5....T.    /opt/mcnf/automation/secrets/mcnf-secret.sh') ;;
            *) unexpected=$((unexpected + 1)) ;;
        esac
    done <<<"$verification"
    if (( verification_rc == 1 && unexpected == 0 )); then
        return 0
    fi
    printf '[FAIL] rpm -V magic-mesh reported %s unexpected installed-file difference(s) (rc=%s); inspect the seat before treating payload proof as valid\n' \
        "$unexpected" "$verification_rc"
    return 1
}

if timeout --signal=TERM --kill-after=2s "${command_timeout}s" \
    systemctl --user is-active --quiet mde-musicd.service >/dev/null 2>&1; then
    restarts="$(timeout --signal=TERM --kill-after=2s "${command_timeout}s" \
        systemctl --user show mde-musicd.service -p NRestarts --value 2>/dev/null || true)"
    if [[ "$restarts" =~ ^[0-9]+$ && "$restarts" == 0 ]]; then
        printf '[OK] mde-musicd.service active (NRestarts=0)\n'
    elif [[ "$restarts" =~ ^[0-9]+$ ]]; then
        printf '[FAIL] mde-musicd.service active but NRestarts=%s\n' "$restarts"; failed=1
    else
        printf '[FAIL] mde-musicd.service restart count unavailable\n'; failed=1
    fi
else
    printf '[FAIL] mde-musicd.service is not active\n'; failed=1
fi

if musicd_runtime_identity; then
    printf '[OK] active mde-musicd.service executes RPM-owned /usr/bin/mde-musicd from the expected package\n'
else
    printf '[FAIL] active mde-musicd.service executable is not the expected RPM-owned /usr/bin/mde-musicd\n'
    failed=1
fi

ping_rc=0
ping_output="$(timeout --signal=TERM --kill-after=2s "${command_timeout}s" \
    mde-musicd ping --retry 0 2>/dev/null)" || ping_rc=$?
if (( ping_rc == 0 )) && [[ -n "$ping_output" ]]; then
    printf '[OK] mde-musicd ping answered\n'
else
    printf '[FAIL] mde-musicd ping failed (bounded rc=%s)\n' "$ping_rc"; failed=1
fi

state_rc=0
state_output="$(timeout --signal=TERM --kill-after=2s "${command_timeout}s" \
    env XDG_RUNTIME_DIR="/run/user/$uid" mde-bus request action/music/get-state \
    --bus-root "$bus_root" --timeout-secs "$command_timeout" --json 2>/dev/null)" || state_rc=$?
if (( state_rc == 0 )) && printf '%s' "$state_output" | validate_reply state >/dev/null 2>&1; then
    printf '[OK] action/music/get-state answered on %s\n' "$bus_root"
else
    printf '[FAIL] action/music/get-state failed (bounded rc=%s)\n' "$state_rc"; failed=1
fi

albums_rc=0
albums_output="$(timeout --signal=TERM --kill-after=2s "${command_timeout}s" \
    env XDG_RUNTIME_DIR="/run/user/$uid" mde-bus request action/music/list-albums \
    --bus-root "$bus_root" --timeout-secs "$command_timeout" --json 2>/dev/null)" || albums_rc=$?
if (( albums_rc == 0 )) && printf '%s' "$albums_output" | validate_reply albums >/dev/null 2>&1; then
    printf '[OK] action/music/list-albums answered on %s\n' "$bus_root"
else
    printf '[FAIL] action/music/list-albums failed (bounded rc=%s)\n' "$albums_rc"; failed=1
fi

if [[ "$play_song_id" != '__music_play_probe_disabled__' ]]; then
    play_rc=0
    timeout --signal=TERM --kill-after=2s "${play_timeout}s" \
        mde-musicd play "$play_song_id" >/dev/null 2>&1 || play_rc=$?
    if [[ "$play_rc" == 0 || "$play_rc" == 124 ]]; then
        if ps -u "$uid" -o args= 2>/dev/null | grep -Eq '[m]de-musicd play'; then
            printf '[FAIL] play probe left a client process\n'; failed=1
        else
            printf '[OK] explicit play probe bounded at %ss (rc=%s)\n' "$play_timeout" "$play_rc"
        fi
    else
        printf '[FAIL] explicit play probe failed (bounded rc=%s)\n' "$play_rc"; failed=1
    fi
else
    printf '[INFO] play probe disabled (pass --play-probe SONG_ID to enable)\n'
fi

package_rc=0
package_identity="$(timeout --signal=TERM --kill-after=2s "${command_timeout}s" \
    rpm -q --qf '%{NAME}\t%{VERSION}\t%{RELEASE}\t%{ARCH}' magic-mesh 2>/dev/null)" || package_rc=$?
payload_rc=0
package_payload="$(timeout --signal=TERM --kill-after=2s "${command_timeout}s" \
    rpm -ql magic-mesh 2>/dev/null)" || payload_rc=$?
verify_rc=0
package_verification="$(timeout --signal=TERM --kill-after=2s "${command_timeout}s" \
    rpm -V magic-mesh 2>/dev/null)" || verify_rc=$?
identity_failed=0
if (( package_rc != 0 )); then
    printf '[FAIL] rpm identity query failed (bounded rc=%s); expected artifact magic-mesh-%s-%s.%s\n' \
        "$package_rc" "$expected_version" "$expected_rpm_release" "$expected_arch"
    identity_failed=1
elif ! validate_package_identity "$package_identity" "$expected_version" "$expected_rpm_release" "$expected_arch"; then
    diagnose_package_identity "$package_identity" "$expected_version" "$expected_rpm_release" "$expected_arch" || true
    identity_failed=1
fi
payload_failed=0
if (( payload_rc != 0 )); then
    printf '[FAIL] rpm payload query failed (bounded rc=%s); expected /usr/bin/mde-musicd and /usr/bin/mde-shell-egui\n' \
        "$payload_rc"
    payload_failed=1
elif ! validate_package_payload 'magic-mesh' "$package_payload"; then
    diagnose_package_payload "$package_payload" || true
    payload_failed=1
else
    printf '[OK] installed magic-mesh payload includes mde-musicd and mde-shell-egui\n'
fi
verification_failed=0
if ! validate_package_verification "$verify_rc" "$package_verification"; then
    diagnose_package_verification "$verify_rc" "$package_verification" || true
    verification_failed=1
elif (( verify_rc == 1 )); then
    printf '[OK] rpm -V magic-mesh reports only the approved mutable secret-helper difference\n'
else
    printf '[OK] rpm -V magic-mesh reports no installed-file differences\n'
fi
if (( package_rc == 0 && payload_rc == 0 && identity_failed == 0 &&
    payload_failed == 0 && verification_failed == 0 )); then
    IFS=$'\t' read -r package_name package_version package_release package_arch <<<"$package_identity"
    printf '[OK] installed %s-%s-%s.%s matches declared version %s/RPM release %s and verifies mde-musicd and mde-shell-egui payloads\n' \
        "$package_name" "$package_version" "$package_release" "$package_arch" \
        "$expected_version" "$expected_rpm_release"
else
    printf '[FAIL] installed magic-mesh package proof is incomplete; expected release artifact magic-mesh-%s-%s.%s (rpm rc=%s/%s/%s)\n' \
        "$expected_version" "$expected_rpm_release" "$expected_arch" \
        "$package_rc" "$payload_rc" "$verify_rc"
    failed=1
fi
exit "$failed"
REMOTE_SCRIPT
)" || remote_rc=$?

printf '%s\n' "$remote_output"
if (( remote_rc == 0 )); then
    printf 'verify-music-live-seat: PASS\n'
else
    printf 'verify-music-live-seat: FAIL (bounded SSH rc=%s)\n' "$remote_rc" >&2
fi
exit "$remote_rc"
