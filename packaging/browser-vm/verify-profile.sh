#!/usr/bin/env bash
# WL-ARCH-008 — fail-closed Browser VM profile contract verifier.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
PROFILE="${1:-$ROOT/packaging/browser-vm/profile.env}"

die() {
    echo "verify-browser-vm-profile: $*" >&2
    exit 1
}

[ -f "$PROFILE" ] || die "profile is not a regular file: $PROFILE"
[[ ! -L "$PROFILE" ]] || die "profile must not be a symlink: $PROFILE"
profile_metadata=$(stat -c '%u %a' "$PROFILE" 2>/dev/null) \
    || die "profile metadata is unreadable: $PROFILE"
profile_owner=${profile_metadata%% *}
profile_mode=${profile_metadata##* }
[[ "$profile_owner" == 0 ]] || die "profile must be owned by root: $PROFILE"
case "$profile_mode" in
    [0-7][2367][0-7]|[0-7][0-7][2367]) die "profile is writable by group or other" ;;
esac
case "$profile_mode" in
    *[1357]*) die "profile must not be executable" ;;
esac
[ -r "$PROFILE" ] || die "profile is not readable: $PROFILE"

# Parse as data. A profile must never be shell-evaluated because it is an input
# boundary for image/provisioning tooling.
declare -A values
while IFS= read -r line || [ -n "$line" ]; do
    line="${line%$'\r'}"
    [[ -z "$line" || "$line" == \#* ]] && continue
    [[ "$line" =~ ^([A-Z][A-Z0-9_]*)=([^[:space:]]+)$ ]] || die "malformed line: $line"
    key="${BASH_REMATCH[1]}"
    value="${BASH_REMATCH[2]}"
    [[ "$key" == BROWSER_VM_* ]] || die "unexpected key: $key"
    [[ -z "${values[$key]+present}" ]] || die "duplicate key: $key"
    values["$key"]="$value"
done < "$PROFILE"

required=(
    BROWSER_VM_PROFILE_SCHEMA BROWSER_VM_PROFILE_ID BROWSER_VM_IMAGE_ID
    BROWSER_VM_SOURCE_REPOSITORY BROWSER_VM_SOURCE_PATH BROWSER_VM_SOURCE_COMMIT
    BROWSER_VM_GUEST_OS
    BROWSER_VM_COMPOSITOR BROWSER_VM_BROWSER BROWSER_VM_VCPU
    BROWSER_VM_MEMORY_MB BROWSER_VM_DISK_GB BROWSER_VM_TRANSPORTS
    BROWSER_VM_DEFAULT_TRANSPORT BROWSER_VM_HOST_BROWSER BROWSER_VM_NETWORK
    BROWSER_VM_RUNTIME_FAILURE_POLICY BROWSER_VM_GUEST_TERMINAL_STATES
)
for key in "${required[@]}"; do
    [[ -n "${values[$key]+present}" && -n "${values[$key]}" ]] || die "missing required field: $key"
done

for key in "${!values[@]}"; do
    case " ${required[*]} " in
        *" $key "*) ;;
        *) die "unknown field: $key" ;;
    esac
done

[[ "${values[BROWSER_VM_PROFILE_SCHEMA]}" == 1 ]] || die "unsupported profile schema"
[[ "${values[BROWSER_VM_PROFILE_ID]}" == browser-vm-chromium ]] || die "unexpected profile id"
[[ "${values[BROWSER_VM_IMAGE_ID]}" == browser-vm-chromium ]] || die "unexpected image id"
[[ "${values[BROWSER_VM_SOURCE_REPOSITORY]}" == https://github.com/matthewmackes/magic-mesh.git ]] \
    || die "source repository is not the governed repository"
[[ "${values[BROWSER_VM_SOURCE_PATH]}" == packaging/browser-vm/profile.env ]] \
    || die "source path is not the profile itself"
[[ "${values[BROWSER_VM_SOURCE_COMMIT]}" =~ ^[0-9a-f]{40}$ ]] \
    || die "source commit must be a 40-character immutable Git revision"
[[ "${values[BROWSER_VM_SOURCE_COMMIT]}" != 0000000000000000000000000000000000000000 ]] \
    || die "source commit must identify a real Git object"
[[ "${values[BROWSER_VM_GUEST_OS]}" == fedora-bootc ]] || die "unsupported guest OS"
[[ "${values[BROWSER_VM_COMPOSITOR]}" == sway ]] || die "unsupported guest compositor"
[[ "${values[BROWSER_VM_BROWSER]}" == chromium ]] || die "guest browser must be Chromium"
[[ "${values[BROWSER_VM_VCPU]}" =~ ^[0-9]+$ && ${values[BROWSER_VM_VCPU]} -ge 4 ]] \
    || die "Browser VM needs at least 4 vCPU"
[[ "${values[BROWSER_VM_MEMORY_MB]}" =~ ^[0-9]+$ && ${values[BROWSER_VM_MEMORY_MB]} -ge 8192 ]] \
    || die "Browser VM needs at least 8192 MiB"
[[ "${values[BROWSER_VM_DISK_GB]}" =~ ^[0-9]+$ && ${values[BROWSER_VM_DISK_GB]} -ge 64 ]] \
    || die "Browser VM needs at least 64 GiB"
[[ "${values[BROWSER_VM_TRANSPORTS]}" == rdp,sunshine ]] || die "transport set must be rdp,sunshine"
[[ "${values[BROWSER_VM_DEFAULT_TRANSPORT]}" == sunshine ]] || die "Sunshine must remain the default transport"
[[ "${values[BROWSER_VM_HOST_BROWSER]}" == false ]] || die "host Browser ownership is forbidden"
[[ "${values[BROWSER_VM_NETWORK]}" == mesh-guest ]] || die "unexpected guest network"
[[ "${values[BROWSER_VM_RUNTIME_FAILURE_POLICY]}" == fail-closed ]] \
    || die "runtime failure policy must be fail-closed"
[[ "${values[BROWSER_VM_GUEST_TERMINAL_STATES]}" == failed,unavailable ]] \
    || die "guest terminal states must be failed,unavailable"

# Guard the contract against accidental reintroduction of the old host engines.
# `BROWSER_VM_HOST_BROWSER=false` is an intentional policy field, so do not use
# a broad host-browser phrase match here.
if grep -Eiq 'cef|servo|mde-web|native.?page' "$PROFILE"; then
    die "profile contains a host-browser/helper engine reference"
fi

echo "Browser VM profile contract passed: ${values[BROWSER_VM_PROFILE_ID]}"
