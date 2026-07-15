#!/usr/bin/env bash
# E12-13 — STATIC acceptance checks for the built bootc image.
#
# Runs the image as a plain container (podman run, no systemd as PID 1) and
# asserts the payload + wiring the Workstation boot depends on: binaries,
# the DRM-seat unit + preset, the enabled-unit symlinks, graphical default,
# the role-gate regex, the channel .repo and the tmpfiles doctrine.
#
# ⚠ This is NOT a boot test. It proves the image *contents*; the live
# boot-to-seat acceptance (bootc-image-builder disk + a boot target) stays
# operator-gated — see README.md "Verification status".
#
# Usage: verify-image.sh [image:tag]     (default localhost/magic-mesh-bootc:latest)
# Exit:  0 all checks pass; 1 any check failed (each failure itemized).
set -euo pipefail

TAG="${1:-localhost/magic-mesh-bootc:latest}"

command -v podman >/dev/null 2>&1 || { echo "FATAL: podman not on PATH" >&2; exit 1; }
podman image exists "$TAG" || { echo "FATAL: image not in local storage: $TAG (build it first)" >&2; exit 1; }

# The in-image check script (quoted heredoc: nothing expands host-side).
INNER_SCRIPT="$(cat <<'INNER'
set -u
fail=0
ok()  { echo "  OK   $1"; }
bad() { echo "  FAIL $1"; fail=1; }

# Payload binaries (the §5/QC-1 stack: shell, daemon, libvirt/OVN, wizard, CLI).
for b in mde-shell-egui mackesd magic-setup meshctl virsh ovs-vsctl cloud-init qemu-ga; do
    [ -x "/usr/bin/$b" ] && ok "/usr/bin/$b" || bad "/usr/bin/$b missing/not executable"
done
for p in qemu-kvm libvirt-daemon-driver-qemu libvirt-daemon-config-network ovn-host openvswitch cloud-init qemu-guest-agent; do
    rpm -q "$p" >/dev/null 2>&1 && ok "package installed: $p" || bad "package missing: $p"
done
virsh --version >/dev/null 2>&1 \
    && ok "virsh executes ($(virsh --version))" \
    || bad "virsh does not execute"
ovs-vsctl --version >/dev/null 2>&1 \
    && ok "ovs-vsctl executes ($(ovs-vsctl --version | head -n 1))" \
    || bad "ovs-vsctl does not execute"
[ ! -e /usr/bin/cloud-hypervisor ] \
    && ok "cloud-hypervisor absent per QUASAR-CLOUD/QC-1" \
    || bad "cloud-hypervisor still present"
[ -f /usr/lib/bootc/install/50-magic-mesh.toml ] \
    && ok "bootc install rootfs config present" \
    || bad "bootc install rootfs config missing"
grep -q 'type = "xfs"' /usr/lib/bootc/install/50-magic-mesh.toml 2>/dev/null \
    && ok "bootc install rootfs default = xfs" \
    || bad "bootc install rootfs default is not xfs"
grep -q 'datasource_list: \[ NoCloud, None \]' /etc/cloud/cloud.cfg.d/90-mcnf-nocloud.cfg 2>/dev/null \
    && ok "cloud-init constrained to NoCloud/None" \
    || bad "cloud-init NoCloud datasource config missing"
module_count="$(find /usr/lib/modules -mindepth 1 -maxdepth 1 -type d | wc -l)"
[ "$module_count" -eq 1 ] \
    && ok "single kernel modules tree present" \
    || bad "found $module_count kernel modules trees (bootc install requires one)"
find /usr/lib/modules -mindepth 1 -maxdepth 1 -type d -name '*.surface.*' | grep -q . \
    && ok "surface kernel is the bootc kernel" \
    || bad "surface kernel modules tree missing"

# The seat unit, its preset, and the role gate.
[ -f /usr/lib/systemd/system/mde-shell-egui.service ] \
    && ok "seat unit installed" || bad "seat unit missing"
[ -f /usr/lib/systemd/system-preset/45-mcnf-quasar.preset ] \
    && ok "seat preset installed" || bad "seat preset missing"
grep -q 'ExecCondition=/usr/bin/mackesd role-gate --min-rank 1' /usr/lib/systemd/system/mde-shell-egui.service \
    && ok "typed role gate present in seat unit" || bad "typed role gate missing from seat unit"
grep -q '^Delegate=yes$' /usr/lib/systemd/system/mde-shell-egui.service \
    && ok "seat unit delegates cgroups for browser sandbox caps" \
    || bad "seat unit missing Delegate=yes for browser sandbox caps"
grep -q '^DelegateSubgroup=shell$' /usr/lib/systemd/system/mde-shell-egui.service \
    && ok "seat unit keeps shell in delegated subgroup for browser sandbox caps" \
    || bad "seat unit missing DelegateSubgroup=shell for browser sandbox caps"
grep -q '^Environment=MDE_WEB_SANDBOX_DELEGATE_SUBGROUP=shell$' /usr/lib/systemd/system/mde-shell-egui.service \
    && ok "seat unit tells browser sandbox which delegated subgroup to escape" \
    || bad "seat unit missing browser sandbox delegated-subgroup env"

# Enablement symlinks (systemctl reads links; no running systemd needed).
for u in mde-shell-egui.service podman.socket mackesd.service nebula.service \
         magic-setup.service mesh-health.timer \
         cloud-init-local.service cloud-init.service cloud-config.service \
         cloud-final.service qemu-guest-agent.service openvswitch.service; do
    state="$(systemctl is-enabled "$u" 2>/dev/null || true)"
    [ "$state" = enabled ] && ok "enabled: $u" || bad "$u is '$state' (want enabled)"
done
[ "$(systemctl get-default 2>/dev/null)" = graphical.target ] \
    && ok "default target = graphical" || bad "default target != graphical"

# Channel + state doctrine artifacts.
[ -f /etc/yum.repos.d/magic-mesh.repo ] && ok "channel .repo present" || bad "channel .repo missing"
grep -q 'mesh-storage' /usr/lib/tmpfiles.d/magic-mesh.conf 2>/dev/null \
    && ok "tmpfiles doctrine present" || bad "tmpfiles magic-mesh.conf missing/short"

exit "$fail"
INNER
)"

# -i is load-bearing: without it the container's stdin is closed, `bash -s`
# reads EOF, runs ZERO checks and exits 0 — a false green (caught live).
rc=0
out="$(printf '%s\n' "$INNER_SCRIPT" | podman run --rm -i "$TAG" /bin/bash -s)" || rc=$?
printf '%s\n' "$out"

# Silence is not success: a run that produced no itemized lines is a failure
# even if podman exited 0 (the stdin/exec regression tripwire).
grep -q '^  OK '   <<<"$out" || { echo "FATAL: no checks executed — stdin/exec regression" >&2; rc=1; }
grep -q '^  FAIL ' <<<"$out" && rc=1

if [ "$rc" -eq 0 ]; then
    echo "==> verify-image: ALL STATIC CHECKS PASS for $TAG (boot acceptance still gated)"
else
    echo "==> verify-image: FAILURES above for $TAG" >&2
fi
exit "$rc"
