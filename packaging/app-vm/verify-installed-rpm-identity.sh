#!/usr/bin/env bash
# Authenticate the compile-time identity of the already-installed repo RPM.
set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
BUILD_IDENTITY_VERIFY="$SCRIPT_DIR/verify-rpm-build-identity.py"
SOURCE_COMMIT=""
BINARY_ROOT=/
RPM_FILE_MANIFEST=""

refuse() {
    echo "App VM installed RPM identity refused: $*" >&2
    exit 2
}

verify_carrier() {
    local member=$1 installed_path=${1#.} path="$BINARY_ROOT${1#.}"
    local fd path_identity fd_identity file_type owner mode manifest_line
    local manifest_path expected_digest digest_algorithm extra actual_digest digest_name

    if [ ! -f "$path" ] || [ -L "$path" ]; then
        refuse "installed build-identity carrier is not one regular non-symlink file: $member"
    fi
    exec {fd}<"$path" \
        || refuse "installed build-identity carrier cannot be opened: $member"
    path_identity=$(stat -Lc '%d:%i' -- "$path") \
        || refuse "installed build-identity carrier cannot be identified: $member"
    fd_identity=$(stat -Lc '%d:%i' -- "/proc/self/fd/$fd") \
        || refuse "opened build-identity carrier cannot be identified: $member"
    [ "$path_identity" = "$fd_identity" ] \
        || refuse "installed build-identity carrier changed while being opened: $member"
    IFS=: read -r file_type owner mode < <(stat -Lc '%F:%u:%a' -- "/proc/self/fd/$fd") \
        || refuse "opened build-identity carrier metadata is unavailable: $member"
    if [ "$file_type" != 'regular file' ] || [ "$owner" != 0 ] \
        || [[ ! "$mode" =~ ^[0-7]{3,4}$ ]] || (( (8#$mode & 0022) != 0 )); then
        refuse "installed build-identity carrier has unsafe ownership or mode: $member"
    fi

    manifest_line=$(awk -F '\t' -v path="$installed_path" '$1 == path { print }' \
        <<<"$RPM_FILE_MANIFEST") \
        || refuse "installed RPM file manifest cannot be inspected: $member"
    [ "$(wc -l <<<"$manifest_line")" -eq 1 ] \
        || refuse "installed build-identity carrier has missing or duplicate RPM ownership: $member"
    IFS=$'\t' read -r manifest_path expected_digest digest_algorithm extra <<<"$manifest_line"
    if [ -n "${extra:-}" ] || [ "$manifest_path" != "$installed_path" ] \
        || [[ ! "$expected_digest" =~ ^[0-9a-fA-F]{64}$ ]]; then
        refuse "installed build-identity carrier has malformed RPM digest authority: $member"
    fi
    case "$digest_algorithm" in
        8|SHA256|sha256) ;;
        *) refuse "installed build-identity carrier RPM digest is not SHA-256: $member" ;;
    esac
    read -r actual_digest digest_name < <(sha256sum -- "/proc/self/fd/$fd") \
        || refuse "opened build-identity carrier cannot be hashed: $member"
    [ "$digest_name" = "/proc/self/fd/$fd" ] \
        || refuse "opened build-identity carrier hash result is ambiguous: $member"
    [ "${actual_digest,,}" = "${expected_digest,,}" ] \
        || refuse "installed build-identity carrier differs from the governed RPM payload: $member"

    "$BUILD_IDENTITY_VERIFY" \
        --source-commit "$SOURCE_COMMIT" \
        --package-version "$PACKAGE_VERSION" \
        --member "$member" <&$fd \
        || refuse "installed repo RPM does not carry the requested source revision and package version in $member"
    exec {fd}<&-
}

verify_installed() {
    local output extra
    local -a rows

    if [[ ! "$SOURCE_COMMIT" =~ ^[0-9a-f]{40}$ ]] \
        || [ "$SOURCE_COMMIT" = 0000000000000000000000000000000000000000 ]; then
        refuse '--source-commit must be a non-null 40-character lowercase Git revision'
    fi
    [ -x "$BUILD_IDENTITY_VERIFY" ] \
        || refuse 'RPM build-identity verifier is not executable'
    command -v rpm >/dev/null 2>&1 || refuse 'rpm is required'
    command -v sha256sum >/dev/null 2>&1 || refuse 'sha256sum is required'

    output=$(rpm -q --qf '%{NAME}\t%{VERSION}\n' -- magic-mesh 2>/dev/null) \
        || refuse 'governed magic-mesh repo package is not installed'
    mapfile -t rows <<<"$output"
    [ "${#rows[@]}" -eq 1 ] \
        || refuse 'installed magic-mesh package identity is ambiguous'
    IFS=$'\t' read -r package_name PACKAGE_VERSION extra <<<"${rows[0]}"
    if [ -n "${extra:-}" ] || [ "$package_name" != magic-mesh ] || [ -z "$PACKAGE_VERSION" ]; then
        refuse 'installed package identity is incomplete or not magic-mesh'
    fi

    RPM_FILE_MANIFEST=$(rpm -q --qf \
        '[%{FILENAMES}\t%{FILEDIGESTS}\t%{=FILEDIGESTALGO}\n]' \
        -- magic-mesh 2>/dev/null) \
        || refuse 'governed magic-mesh RPM file manifest is unavailable'

    verify_carrier ./usr/bin/mackesd
    verify_carrier ./usr/bin/mde-shell-egui
}

self_test() {
    local fixture script revision stale_revision
    fixture=$(mktemp -d)
    SELF_TEST_FIXTURE=$fixture
    trap 'rm -rf "$SELF_TEST_FIXTURE"' EXIT
    script=$(cd "$(dirname "$0")" && pwd)/$(basename "$0")
    revision=0123456789abcdef0123456789abcdef01234567
    stale_revision=1123456789abcdef0123456789abcdef01234567
    mkdir -p "$fixture/bin" "$fixture/root/usr/bin"

    cat > "$fixture/bin/rpm" <<'EOF'
#!/usr/bin/env bash
set -eu
case " $* " in
    *'%{FILENAMES}'*)
        printf '/usr/bin/mackesd\t%s\t8\n' "${FIXTURE_MACKESD_DIGEST:?}"
        printf '/usr/bin/mde-shell-egui\t%s\t8\n' "${FIXTURE_SHELL_DIGEST:?}"
        ;;
    *) printf '%s\t%s\n' magic-mesh 12.1.6 ;;
esac
EOF
    cat > "$fixture/bin/stat" <<'EOF'
#!/usr/bin/env bash
set -eu
case " $* " in
    *'%F:%u:%a'*)
        /usr/bin/stat "$@" | sed -E 's/^(regular file):[0-9]+:/\1:0:/'
        ;;
    *) exec /usr/bin/stat "$@" ;;
esac
EOF
    chmod 0755 "$fixture/bin/rpm" "$fixture/bin/stat"
    printf '\177ELF12.1.6Construct%s\n' "$revision" > "$fixture/root/usr/bin/mackesd"
    printf '\177ELF12.1.6Construct%s\n' "$revision" > "$fixture/root/usr/bin/mde-shell-egui"
    chmod 0555 "$fixture/root/usr/bin/mackesd" "$fixture/root/usr/bin/mde-shell-egui"
    FIXTURE_MACKESD_DIGEST=$(sha256sum "$fixture/root/usr/bin/mackesd" | awk '{print $1}')
    FIXTURE_SHELL_DIGEST=$(sha256sum "$fixture/root/usr/bin/mde-shell-egui" | awk '{print $1}')
    export FIXTURE_MACKESD_DIGEST FIXTURE_SHELL_DIGEST

    PATH="$fixture/bin:/usr/bin:/bin" "$script" \
        --source-commit "$revision" --binary-root "$fixture/root"

    # Preserve the exact accepted BuildInfo identity while changing executable
    # bytes. Embedded identity alone must not authorize an ELF that differs
    # from the governed package payload recorded by RPM.
    chmod 0755 "$fixture/root/usr/bin/mackesd"
    printf '%s\n' 'hostile replacement bytes' >> "$fixture/root/usr/bin/mackesd"
    chmod 0555 "$fixture/root/usr/bin/mackesd"
    if PATH="$fixture/bin:/usr/bin:/bin" "$script" \
        --source-commit "$revision" --binary-root "$fixture/root" >/dev/null 2>&1; then
        refuse 'self-test admitted a modified installed ELF with copied BuildInfo identity'
    fi

    # Model a correctly signed/installable repo package whose payload is stale:
    # authoritative RPM metadata still resolves magic-mesh 12.1.6, but one
    # installed production ELF carries a different compile-time revision.
    chmod 0755 "$fixture/root/usr/bin/mackesd"
    printf '\177ELF12.1.6Construct%s\n' "$stale_revision" > "$fixture/root/usr/bin/mackesd"
    chmod 0555 "$fixture/root/usr/bin/mackesd"
    FIXTURE_MACKESD_DIGEST=$(sha256sum "$fixture/root/usr/bin/mackesd" | awk '{print $1}')
    export FIXTURE_MACKESD_DIGEST
    if PATH="$fixture/bin:/usr/bin:/bin" "$script" \
        --source-commit "$revision" --binary-root "$fixture/root" >/dev/null 2>&1; then
        refuse 'self-test admitted a signed but stale installed repo payload'
    fi

    echo 'App VM installed RPM identity self-test passed (RPM payload digest + repo package version + both BuildInfo carriers)'
}

if [ "${1:-}" = --self-test ]; then
    [ "$#" -eq 1 ] || refuse '--self-test takes no other arguments'
    self_test
    exit 0
fi

while [ "$#" -gt 0 ]; do
    case "$1" in
        --source-commit) SOURCE_COMMIT="${2:?--source-commit needs a revision}"; shift 2 ;;
        --binary-root) BINARY_ROOT="${2:?--binary-root needs a path}"; shift 2 ;;
        *) refuse "unknown option: $1" ;;
    esac
done

[ -n "$SOURCE_COMMIT" ] || refuse '--source-commit is required'
[[ "$BINARY_ROOT" = /* ]] || refuse '--binary-root must be absolute'
verify_installed
