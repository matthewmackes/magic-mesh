#!/usr/bin/env bash
# Regression gate for the base RPM's single-owner Music and corrected-forward
# service activation contract. This reads the manifest only; it never contacts
# systemd or mutates a live user manager.
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
manifest="$repo_root/crates/mesh/mackesd/Cargo.toml"

command -v python3 >/dev/null 2>&1 || {
  printf 'test-rpm-seat-service-activation: python3 is required\n' >&2
  exit 2
}

python3 - "$manifest" <<'PY'
import sys

path = sys.argv[1]
with open(path, encoding="utf-8") as handle:
    manifest = handle.read()
marker = 'post_install_script = """\n'
start = manifest.index(marker) + len(marker)
script = manifest[start:manifest.index('\n"""', start)]

required = {
    "global duplicate removal": "systemctl --global disable mde-musicd.service",
    "bounded seat preference": "for candidate in mm mde; do",
    "duplicate owner cleanup": 'mcnf_user_systemctl "$candidate" disable --now mde-musicd.service',
    "direct user bus": 'DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$mcnf_uid/bus"',
    "music activation": 'mcnf_user_systemctl "$candidate" restart mde-musicd.service',
    "system activation": "systemctl try-restart mackesd.service mde-shell-egui.service",
}
for label, token in required.items():
    if token not in script:
        raise SystemExit(f"missing {label}: {token}")

for forbidden in (
    "systemctl --global enable mde-musicd.service",
    "systemctl --user --machine=",
):
    if forbidden in script:
        raise SystemExit(f"forbidden activation path remains: {forbidden}")

setup_end = script.find("timeout 60 update-desktop-database")
system_restart = script.find("systemctl try-restart mackesd.service mde-shell-egui.service")
if setup_end < 0 or system_restart <= setup_end:
    raise SystemExit("system service restart must occur after package setup")

print("test-rpm-seat-service-activation: contract passed")
PY

python3 - "$manifest" <<'PY' | bash -n
import sys
with open(sys.argv[1], encoding="utf-8") as handle:
    manifest = handle.read()
marker = 'post_install_script = """\n'
start = manifest.index(marker) + len(marker)
print(manifest[start:manifest.index('\n"""', start)])
PY

printf 'test-rpm-seat-service-activation: shell syntax passed\n'
