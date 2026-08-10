#!/usr/bin/env bash
# Restore the five communal Workstation XDG bind mounts without moving,
# deleting, or obscuring local data. PID 1 owns the mounts through transient
# systemd mount units so this helper remains compatible with the hardened peer
# recovery service's private filesystem namespace.
set -u

PASSWD_FILE="${MCNF_XDG_PASSWD_FILE:-/etc/passwd}"
ROLE_FILE="${MCNF_ROLE_FILE:-/var/lib/mde/role.toml}"
MESH_HOME="${MCNF_XDG_MESH_HOME:-/mnt/mesh-storage/home}"
SYSTEMD_MOUNT="${MCNF_XDG_SYSTEMD_MOUNT:-/usr/bin/systemd-mount}"
MOUNTPOINT="${MCNF_XDG_MOUNTPOINT:-/usr/bin/mountpoint}"
SLEEP="${MCNF_RECOVERY_SLEEP:-/usr/bin/sleep}"
COMMAND_TIMEOUT="${MCNF_RECOVERY_COMMAND_TIMEOUT:-10}"
XDG_DIRS=(Documents Downloads Music Pictures Videos)

fail() { printf 'mesh-xdg-bind-recovery: REFUSED: %s\n' "$*" >&2; return 1; }

role_is_workstation() {
    /usr/bin/grep -Eq '^[[:space:]]*role[[:space:]]*=[[:space:]]*"?workstation"?[[:space:]]*$' "$ROLE_FILE"
}

desktop_homes() {
    /usr/bin/awk -F: '$3 >= 1000 && $3 < 60000 && $6 ~ /^\/home\/[A-Za-z0-9._-]+$/ { print $6 }' "$PASSWD_FILE"
}

directory_empty() {
    [ -z "$(/usr/bin/find "$1" -mindepth 1 -maxdepth 1 -print -quit 2>/dev/null)" ]
}

wait_exact_bind() {
    local source="$1" target="$2" attempt=0
    while [ "$attempt" -lt 5 ]; do
        if "$MOUNTPOINT" -q "$target" \
            && [ "$(/usr/bin/stat -Lc '%d:%i' "$source" 2>/dev/null)" = "$(/usr/bin/stat -Lc '%d:%i' "$target" 2>/dev/null)" ]; then
            return 0
        fi
        attempt=$((attempt + 1))
        "$SLEEP" 1
    done
    return 1
}

validate_desktop_homes() {
    local home name source target found=0
    while IFS= read -r home; do
        [ -n "$home" ] || continue
        found=1
        [ -d "$home" ] && [ ! -L "$home" ] \
            || { fail "desktop home is unavailable or unsafe: $home"; return 1; }
        for name in "${XDG_DIRS[@]}"; do
            source="$MESH_HOME/$name"
            target="$home/$name"
            # Do not let install(1) follow a hostile source or target symlink.
            # Validate every desktop home before the first mount so a later
            # hostile home cannot leave an earlier home partially restored.
            if [ -L "$source" ] || { [ -e "$source" ] && [ ! -d "$source" ]; }; then
                fail "communal source is unavailable or unsafe: $source"
                return 1
            fi
            if [ -L "$target" ] || { [ -e "$target" ] && [ ! -d "$target" ]; }; then
                fail "desktop target is unavailable or unsafe: $target"
                return 1
            fi
            if "$MOUNTPOINT" -q "$target"; then
                wait_exact_bind "$source" "$target" \
                    || { fail "existing mount is not the exact communal source: $target"; return 1; }
            elif [ -d "$target" ]; then
                directory_empty "$target" \
                    || { fail "local data would be obscured: $target"; return 1; }
            fi
        done
    done < <(desktop_homes)
    [ "$found" -eq 1 ] || { fail 'no-workstation-desktop-home'; return 1; }
}

main() {
    local home name source target found=0
    [ "$(id -u)" -eq 0 ] || { fail 'must-run-as-root'; return 1; }
    case "$COMMAND_TIMEOUT" in
        ''|*[!0-9]*) fail 'invalid-command-timeout'; return 2 ;;
    esac
    [ "$COMMAND_TIMEOUT" -gt 0 ] && [ "$COMMAND_TIMEOUT" -le 30 ] \
        || { fail 'invalid-command-timeout'; return 2; }
    [ -f "$ROLE_FILE" ] && role_is_workstation || return 0
    [ -d "$MESH_HOME" ] && [ ! -L "$MESH_HOME" ] \
        || { fail "communal source root unavailable: $MESH_HOME"; return 1; }

    validate_desktop_homes || return 1

    while IFS= read -r home; do
        [ -n "$home" ] || continue
        found=1
        [ -d "$home" ] && [ ! -L "$home" ] \
            || { fail "desktop home is unavailable or unsafe: $home"; return 1; }
        for name in "${XDG_DIRS[@]}"; do
            source="$MESH_HOME/$name"
            target="$home/$name"
            [ ! -L "$source" ] && [ ! -L "$target" ] \
                || { fail "source or target symlink appeared during restore: $target"; return 1; }
            /usr/bin/install -d -m 0777 -- "$source" \
                || { fail "could not create communal source: $source"; return 1; }
            /usr/bin/install -d -m 0755 -o "$(/usr/bin/stat -Lc %u "$home")" -g "$(/usr/bin/stat -Lc %g "$home")" -- "$target" \
                || { fail "could not create desktop target: $target"; return 1; }
            if "$MOUNTPOINT" -q "$target"; then
                wait_exact_bind "$source" "$target" \
                    || { fail "existing mount is not the exact communal source: $target"; return 1; }
                continue
            fi
            directory_empty "$target" \
                || { fail "local data would be obscured: $target"; return 1; }
            # systemd-mount has no --bind option: on current systemd that token
            # abbreviates --bind-device and treats the directory as a block
            # device. Express the portable bind mount explicitly.
            /usr/bin/timeout "$COMMAND_TIMEOUT" "$SYSTEMD_MOUNT" --no-block --collect \
                --type=none --options=bind "$source" "$target" >/dev/null \
                || { fail "systemd refused bind mount: $source -> $target"; return 1; }
            wait_exact_bind "$source" "$target" \
                || { fail "bind mount did not become exact and active: $target"; return 1; }
        done
    done < <(desktop_homes)
    [ "$found" -eq 1 ] || { fail 'no-workstation-desktop-home'; return 1; }
    printf 'mesh-xdg-bind-recovery: PASS: communal XDG binds restored without local-data replacement\n'
}

main "$@"
