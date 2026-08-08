#!/usr/bin/env bash
# Static/source verifier for the tracked seat-user QEMU Pulse endpoint.
set -euo pipefail

usage() {
    printf '%s\n' \
        "usage: $0" \
        "       $0 --helper FILE --unit FILE" >&2
}

die() {
    printf 'verify-qemu-pulse-endpoint: %s\n' "$*" >&2
    exit 1
}

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
repo_root="$(cd -- "$script_dir/.." && pwd -P)"
if [[ -f $repo_root/install-helpers/mcnf-qemu-pulse-endpoint.sh ]]; then
    helper="$repo_root/install-helpers/mcnf-qemu-pulse-endpoint.sh"
    unit="$repo_root/packaging/systemd/mcnf-qemu-pulse-endpoint.service"
else
    # The RPM installs this verifier beside the helper while the user unit
    # follows systemd's vendor layout. Keep that live-seat invocation as
    # first-class as the source-tree gate.
    helper="/usr/libexec/mackesd/mcnf-qemu-pulse-endpoint"
    unit="/usr/lib/systemd/user/mcnf-qemu-pulse-endpoint.service"
fi

if [[ $# -ne 0 ]]; then
    [[ $# -eq 4 && $1 == "--helper" && $3 == "--unit" ]] || { usage; exit 2; }
    helper=$2
    unit=$4
fi

for path in "$helper" "$unit"; do
    [[ -f $path && ! -L $path ]] || die "missing, non-regular, or symlinked input: $path"
    [[ $(stat -Lc '%s' "$path") -le 131072 ]] || die "input exceeds 128 KiB: $path"
done

command -v systemd-analyze >/dev/null 2>&1 || die "systemd-analyze is required"
bash -n "$helper"
"$helper" --self-test >/dev/null

require_unit_line() {
    grep -Fqx -- "$1" "$unit" || die "unit is missing required line: $1"
}

require_unit_line "BindsTo=pipewire-pulse.service"
require_unit_line "After=pipewire-pulse.service"
require_unit_line "PartOf=pipewire-pulse.service"
require_unit_line "Type=notify"
require_unit_line "NotifyAccess=all"
require_unit_line "Environment=XDG_RUNTIME_DIR=%t"
require_unit_line "Environment=PULSE_SERVER=unix:%t/pulse/native"
require_unit_line "ExecStart=/usr/bin/bash /usr/libexec/mackesd/mcnf-qemu-pulse-endpoint --run"
require_unit_line "TimeoutStartSec=20s"
require_unit_line "WatchdogSec=20s"
require_unit_line "Restart=on-failure"
require_unit_line "NoNewPrivileges=yes"
require_unit_line "RestrictAddressFamilies=AF_UNIX AF_NETLINK AF_INET AF_INET6"
require_unit_line "WantedBy=default.target"

if grep -Eq '^(User|Group|DynamicUser)=' "$unit"; then
    die "the user unit must run in the selected seat user's own manager"
fi
if grep -Eq '0\.0\.0\.0|\[?::\]?|auth-anonymous' "$unit"; then
    die "the unit contains a broad or anonymous listener directive"
fi

systemd-analyze --user --recursive-errors=no --man=no verify "$unit" >/dev/null
printf '%s\n' "verify-qemu-pulse-endpoint: passed"
