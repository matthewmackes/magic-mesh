#!/bin/sh
# Validate the identity-only cloud-init contract before the guest admits an app.
# Keep this dependency-free: it runs from the immutable App VM image.
set -eu

input_root=${MCNF_APP_VM_INPUT_ROOT:-/etc/mackesd/app-vm}
hostname_file=${MCNF_APP_VM_HOSTNAME_FILE:-/etc/hostname}
max_length=128
max_encoded_length=$((max_length + 2))

fail() {
    echo "FATAL: invalid App VM runtime input: $1" >&2
    exit 1
}

read_json_string() {
    file=$1
    path=$input_root/$file
    [ -f "$path" ] || fail "$file is missing"
    [ ! -L "$path" ] || fail "$file must not be a symbolic link"

    # Cloud-init identity files are launch authority, not replaceable input.
    # Open one descriptor, require a private regular inode, and bind that
    # descriptor back to the requested path before consuming its contents.
    path_identity=$(stat -Lc '%d:%i' -- "$path") || fail "$file cannot be inspected"
    exec 3< "$path" || fail "$file cannot be opened"
    # The function runs inside command substitutions, where `$$` still names
    # the parent shell.  Resolve the descriptor in the inspecting `stat`
    # process so fd 3 remains the file opened immediately above.
    descriptor_metadata=$(stat -Lc '%F:%h:%d:%i:%s' -- "/proc/self/fd/3") || {
        exec 3<&-
        fail "$file descriptor cannot be inspected"
    }
    descriptor_type=${descriptor_metadata%%:*}
    descriptor_rest=${descriptor_metadata#*:}
    descriptor_links=${descriptor_rest%%:*}
    descriptor_rest=${descriptor_rest#*:}
    descriptor_device=${descriptor_rest%%:*}
    descriptor_rest=${descriptor_rest#*:}
    descriptor_inode=${descriptor_rest%%:*}
    descriptor_size=${descriptor_rest##*:}
    [ "$descriptor_type" = "regular file" ] || {
        exec 3<&-
        fail "$file is not a regular file"
    }
    [ "$descriptor_links" = 1 ] || {
        exec 3<&-
        fail "$file must have exactly one link"
    }
    case "$descriptor_size" in
        ''|*[!0-9]*)
            exec 3<&-
            fail "$file has an invalid size"
            ;;
    esac
    [ "$descriptor_size" -le "$max_encoded_length" ] || {
        exec 3<&-
        fail "$file exceeds $max_length characters"
    }
    [ "$path_identity" = "$descriptor_device:$descriptor_inode" ] || {
        exec 3<&-
        fail "$file was replaced while opening"
    }
    raw=$(cat <&3)
    exec 3<&-
    [ "$(stat -Lc '%d:%i' -- "$path" 2>/dev/null || true)" = "$path_identity" ] || \
        fail "$file was replaced while reading"
    case "$raw" in
        \"*\") raw=${raw#\"}; raw=${raw%\"} ;;
        *) : ;; # cloud-init may already have decoded the YAML JSON scalar
    esac
    [ -n "$raw" ] || fail "$file is empty"
    [ "${#raw}" -le "$max_length" ] || fail "$file exceeds $max_length characters"
    printf '%s' "$raw"
}

if [ "${1:-}" = "--self-test" ]; then
    fixture=$(mktemp -d)
    trap 'rm -rf "$fixture"' EXIT HUP INT TERM
    fixture_input=$fixture/input
    mkdir -p "$fixture_input"
    printf '%s\n' 'org.example.Calculator' > "$fixture/app-id-authority"
    ln "$fixture/app-id-authority" "$fixture_input/app-id"
    printf '%s\n' 'catalog:1' > "$fixture_input/catalog-revision"
    printf '%s\n' 'wayland-standard' > "$fixture_input/guest-profile"
    printf '%s\n' 'session:1' > "$fixture_input/session-id"
    printf '%s\n' 'audio' > "$fixture_input/capabilities"
    printf '%s\n' 'app-vm-1' > "$fixture/hostname"

    if MCNF_APP_VM_INPUT_ROOT=$fixture_input \
        MCNF_APP_VM_HOSTNAME_FILE=$fixture/hostname \
        "$0" > "$fixture/stdout" 2> "$fixture/stderr"; then
        echo "FATAL: hard-linked App VM identity acquired launch authority" >&2
        exit 1
    fi
    grep -Fq 'app-id must have exactly one link' "$fixture/stderr" || {
        echo "FATAL: hard-linked App VM identity failed for the wrong reason" >&2
        exit 1
    }
    echo "App VM runtime input authority self-test passed"
    exit 0
fi

validate_identity() {
    kind=$1
    value=$2
    case "$value" in
        *[!A-Za-z0-9._:-]*) fail "$kind contains unsupported characters" ;;
        [!A-Za-z0-9]*) fail "$kind must start with an ASCII letter or digit" ;;
    esac
    case "$kind" in
        app_id)
            case "$value" in
                *[!A-Za-z0-9.-]*) fail "app_id contains unsupported characters" ;;
            esac
            # Flatpak identities are reverse-DNS names, not arbitrary labels.
            # Keep this equivalent to the typed catalog admission rule so an
            # image cannot be tricked into launching a guest identity that the
            # host catalog would never admit.
            case "$value" in
                .*|*.|*..*) fail "app_id must be a reverse-DNS identity" ;;
            esac
            app_id_parts=$value
            old_ifs=$IFS
            IFS=.
            # Intentional field splitting: each reverse-DNS component is
            # validated independently below.
            # shellcheck disable=SC2086
            set -- $app_id_parts
            IFS=$old_ifs
            [ "$#" -ge 2 ] || fail "app_id must contain at least two components"
            for app_id_part do
                case "$app_id_part" in
                    [!A-Za-z_]*|*[!A-Za-z0-9_-]*)
                        fail "app_id contains an invalid component"
                        ;;
                esac
            done
            ;;
        guest_profile)
            [ "$value" = "wayland-standard" ] || fail "guest_profile is not admitted"
            ;;
    esac
}

app_id=$(read_json_string app-id)
catalog_revision=$(read_json_string catalog-revision)
guest_profile=$(read_json_string guest-profile)
session_id=$(read_json_string session-id)
hostname=$(cat "$hostname_file")
[ -n "$hostname" ] || fail "hostname is empty"
[ "${#hostname}" -le "$max_length" ] || fail "hostname exceeds $max_length characters"

validate_identity app_id "$app_id"
validate_identity catalog_revision "$catalog_revision"
validate_identity guest_profile "$guest_profile"
validate_identity session_id "$session_id"
validate_identity vm_id "$hostname"

# The runtime currently treats capabilities as policy metadata, but still bound
# the file so a malformed declaration cannot become an unbounded guest input.
[ -f "$input_root/capabilities" ] || fail "capabilities is missing"
[ "$(wc -c < "$input_root/capabilities")" -le 4096 ] || fail "capabilities exceeds 4096 bytes"
