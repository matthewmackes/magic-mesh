#!/bin/sh
# Validate the identity-only cloud-init contract before the guest admits an app.
# Keep this dependency-free: it runs from the immutable App VM image.
set -eu

input_root=${MCNF_APP_VM_INPUT_ROOT:-/etc/mackesd/app-vm}
hostname_file=${MCNF_APP_VM_HOSTNAME_FILE:-/etc/hostname}
max_length=128

fail() {
    echo "FATAL: invalid App VM runtime input: $1" >&2
    exit 1
}

read_json_string() {
    file=$1
    [ -f "$input_root/$file" ] || fail "$file is missing"
    raw=$(cat "$input_root/$file")
    case "$raw" in
        \"*\") raw=${raw#\"}; raw=${raw%\"} ;;
        *) : ;; # cloud-init may already have decoded the YAML JSON scalar
    esac
    [ -n "$raw" ] || fail "$file is empty"
    [ "${#raw}" -le "$max_length" ] || fail "$file exceeds $max_length characters"
    printf '%s' "$raw"
}

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
