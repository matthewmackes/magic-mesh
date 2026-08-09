#!/bin/bash
# Focused package-contract gate for the blank pre-splash boot correction.
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
unit="$repo/packaging/bootc/units/mde-shell-egui.service"
manifest="$repo/crates/mesh/mackesd/Cargo.toml"

grep -Fqx 'Wants=mackesd.target' "$unit"
if grep -Eq '^After=.*mackesd\.target' "$unit"; then
  echo 'boot-status: shell still waits for daemon convergence' >&2
  exit 1
fi

grep -Fq 'grubby --update-kernel=ALL --remove-args="rhgb"' "$manifest"
grep -Fq -- '--args="systemd.show_status=1 rd.systemd.show_status=1"' "$manifest"
grep -Fq 'mde-web-preview-selinux.service mde-web-cef-selinux.service' "$manifest"
grep -Fq '/usr/libexec/mackesd/setup-selinux-web-preview' "$manifest"
grep -Fq '/usr/libexec/mackesd/setup-selinux-web-cef' "$manifest"

echo 'boot-status: package ordering, status arguments, and retired-unit cleanup present'
