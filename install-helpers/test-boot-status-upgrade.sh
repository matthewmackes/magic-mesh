#!/bin/bash
# Focused package-contract gate for the blank pre-splash boot correction.
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
unit="$repo/packaging/bootc/units/mde-shell-egui.service"
manifest="$repo/crates/mesh/mackesd/Cargo.toml"
bootc_kargs="$repo/packaging/bootc/kargs.d/10-mcnf-boot-status.toml"
bootc_containerfile="$repo/packaging/bootc/Containerfile"

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

# Fresh bootc installs must not regress the RPM upgrade correction.  Explicit
# Plymouth disablement wins even when the upstream base image still adds rhgb.
grep -Fq 'COPY packaging/bootc/kargs.d/10-mcnf-boot-status.toml /usr/lib/bootc/kargs.d/10-mcnf-boot-status.toml' "$bootc_containerfile"
for arg in plymouth.enable=0 rd.plymouth=0 systemd.show_status=1 rd.systemd.show_status=1; do
  grep -Fq "\"$arg\"" "$bootc_kargs"
done

echo 'boot-status: RPM and bootc ordering/status contracts plus retired-unit cleanup present'
