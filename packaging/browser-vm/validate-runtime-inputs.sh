#!/bin/sh
# WL-ARCH-008/WL-FUNC-018 — validate the identity-only Browser VM launch record.
#
# This runs inside the guest before the compositor or VDI endpoint is started.
# Workloads owns the record; this image-owned check only admits the bounded
# identities, immutable artifact reference, and typed transport evidence. It
# never evaluates a command, URL, path, environment assignment, or
# host-provided script. transport-health is evidence for the guest contract,
# not a launch input and is never passed to a launcher.
set -eu

fail() {
    echo "FATAL: invalid Browser VM runtime input: $1" >&2
    exit 1
}

input_root=${MCNF_BROWSER_VM_INPUT_ROOT:-/etc/mcnf-browser-vm}
max_length=128
test_mode=0
if [ "${1:-}" = --test ]; then
    test_mode=1
    shift
fi
[ "$#" -eq 0 ] || fail "unexpected validator argument"

[ -d "$input_root" ] || fail "input directory is missing"
[ ! -L "$input_root" ] || fail "input directory must not be a symlink"

validate_owner_and_mode() {
    path=$1
    label=$2
    metadata=$(stat -c '%u %a' "$path" 2>/dev/null) || fail "$label metadata is unreadable"
    owner=${metadata%% *}
    mode=${metadata##* }
    if [ "$owner" != 0 ] && [ "$test_mode" != 1 ]; then
        fail "$label must be owned by root"
    fi

    # Workloads may create a private (0700/0600), group-readable (0750/0640),
    # or conventional (0755/0644) record, but group/other must never be able
    # to write it. Runtime identity files are data, not executable hooks.
    case "$mode" in
        [0-7][2367][0-7]|[0-7][0-7][2367]) fail "$label is writable by group or other" ;;
    esac
    if [ "$label" != "input directory" ]; then
        case "$mode" in
            *[1357]*) fail "$label must not be executable" ;;
        esac
    fi
}

validate_owner_and_mode "$input_root" "input directory"

newline=$(printf '\n_')
newline=${newline%_}
carriage_return=$(printf '\r_')
carriage_return=${carriage_return%_}

read_scalar() {
    name=$1
    path=$input_root/$name
    [ -f "$path" ] || fail "$name is missing"
    [ ! -L "$path" ] || fail "$name must not be a symlink"
    bytes=$(wc -c < "$path")
    [ "$bytes" -le "$max_length" ] || fail "$name exceeds $max_length bytes"
    value=$(cat "$path")
    [ -n "$value" ] || fail "$name is empty"
    case "$value" in
        *"$newline"*|*"$carriage_return"*) fail "$name must be one line" ;;
    esac
    printf '%s' "$value"
}

validate_token() {
    name=$1
    value=$2
    case "$value" in
        [!A-Za-z0-9]*|*[!A-Za-z0-9._:-]*) fail "$name is not an identity token" ;;
    esac
}

validate_digest() {
    value=$1
    case "$value" in
        sha256:*) hex=${value#sha256:} ;;
        *) fail "image-digest must use sha256 provenance" ;;
    esac
    [ "${#hex}" -eq 64 ] || fail "image-digest must contain 256 bits"
    case "$hex" in
        *[!0-9A-Fa-f]*) fail "image-digest contains non-hexadecimal data" ;;
    esac
}

# Refuse any additional material in the provisioning directory. In particular,
# a command, URL, mount, socket, or environment file must never become a guest
# launch input merely because a caller placed it beside the identity record.
for path in "$input_root"/* "$input_root"/.[!.]* "$input_root"/..?*; do
    [ -e "$path" ] || [ -L "$path" ] || continue
    [ ! -L "$path" ] || fail "$(basename "$path") must not be a symlink"
    [ -f "$path" ] || fail "$(basename "$path") must be a regular file"
    validate_owner_and_mode "$path" "$(basename "$path")"
    name=${path##*/}
    case "$name" in
        profile-id|image-id|image-digest|session-id|transport|transport-health) ;;
        *) fail "unexpected runtime input: $name" ;;
    esac
done

profile_id=$(read_scalar profile-id)
image_id=$(read_scalar image-id)
image_digest=$(read_scalar image-digest)
session_id=$(read_scalar session-id)
transport=$(read_scalar transport)
transport_health=$(read_scalar transport-health)

[ "$profile_id" = browser-vm-chromium ] || fail "profile-id is not admitted"
[ "$image_id" = browser-vm-chromium ] || fail "image-id is not admitted"
validate_digest "$image_digest"
validate_token session-id "$session_id"

case "$transport" in
    rdp|spice) ;;
    *) fail "transport is not admitted by the Browser VM image" ;;
esac

case "$transport_health" in
    connected|reconnecting|failed|unavailable) ;;
    *) fail "transport-health is not a supported typed state" ;;
esac

echo "Browser VM runtime inputs passed: profile=$profile_id image=$image_id transport=$transport transport-health=$transport_health"
