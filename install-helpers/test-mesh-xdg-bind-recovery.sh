#!/usr/bin/env bash
# Regression fixture for the all-homes preflight in mesh-xdg-bind-recovery.sh.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HELPER="$REPO/install-helpers/mesh-xdg-bind-recovery.sh"
ROOT="$(mktemp -d /tmp/mcnf-xdg-recovery-test.XXXXXX)"
HOME_A="/home/mcnf-xdg-test-$$-a"
HOME_B="/home/mcnf-xdg-test-$$-b"
if [ "$(id -u)" -eq 0 ]; then
    PRIVILEGE=()
else
    PRIVILEGE=(sudo -n)
fi
cleanup() {
    rm -rf -- "$ROOT"
    "${PRIVILEGE[@]}" rm -rf -- "$HOME_A" "$HOME_B"
}
trap cleanup EXIT
mkdir -p "$ROOT/bin" "$ROOT/mesh" "$ROOT/state"
"${PRIVILEGE[@]}" mkdir -p "$HOME_A" "$HOME_B"
"${PRIVILEGE[@]}" chown "$(id -u):$(id -g)" "$HOME_A" "$HOME_B"

cat >"$ROOT/passwd" <<EOF
alice:x:1000:1000::${HOME_A}:/bin/bash
bob:x:1001:1001::${HOME_B}:/bin/bash
EOF
cat >"$ROOT/bin/mountpoint" <<'SH'
#!/bin/sh
exit 1
SH
cat >"$ROOT/bin/systemd-mount" <<'SH'
#!/bin/sh
printf '%s\n' "$*" >>"${MCNF_XDG_TEST_STATE:?}/mounts"
exit 0
SH
chmod 0755 "$ROOT/bin/mountpoint" "$ROOT/bin/systemd-mount"

# The first home is safe; the later home is hostile. Preflight must refuse
# before creating any source directory or asking PID 1 for a mount.
for name in Documents Downloads Music Pictures Videos; do
    mkdir -p "$HOME_A/$name" "$HOME_B/$name"
done
rm -rf "$HOME_B/Music"
ln -s /etc "$HOME_B/Music"
: >"$ROOT/state/mounts"
printf '%s\n' 'role = "workstation"' >"$ROOT/role.toml"
if "${PRIVILEGE[@]}" env MCNF_XDG_TEST_STATE="$ROOT/state" \
    MCNF_XDG_PASSWD_FILE="$ROOT/passwd" \
    MCNF_ROLE_FILE="$ROOT/role.toml" \
    MCNF_XDG_MESH_HOME="$ROOT/mesh" \
    MCNF_XDG_SYSTEMD_MOUNT="$ROOT/bin/systemd-mount" \
    MCNF_XDG_MOUNTPOINT="$ROOT/bin/mountpoint" \
    bash "$HELPER"; then
    echo 'hostile later desktop target unexpectedly passed' >&2
    exit 1
fi
[ ! -s "$ROOT/state/mounts" ]
[ ! -e "$ROOT/mesh/Documents" ]
echo 'PASS all-home preflight: hostile later target causes zero mount mutations'
