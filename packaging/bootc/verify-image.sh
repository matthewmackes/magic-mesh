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
#        verify-image.sh --self-test     (hostile mutable-tag regression)
# Exit:  0 all checks pass; 1 any check failed (each failure itemized).
set -euo pipefail

readonly SCRIPT_PATH="$(readlink -f "${BASH_SOURCE[0]}")"

self_test() {
    local fixture output expected_id
    fixture="$(mktemp -d)"
    trap 'rm -rf -- "$fixture"' RETURN
    expected_id="sha256:$(printf '7%.0s' {1..64})"

    cat >"$fixture/podman" <<'FAKE_PODMAN'
#!/usr/bin/env bash
set -euo pipefail
case "${1:-} ${2:-}" in
    "image inspect")
        printf '%s\n' "$EXPECTED_IMAGE_ID"
        ;;
    "run --rm")
        saw_id=0
        for argument in "$@"; do
            [ "$argument" = "$EXPECTED_IMAGE_ID" ] && saw_id=1
            if [ "$argument" = "localhost/magic-mesh-bootc:hostile" ]; then
                echo "mutable tag reached execution after replacement" >&2
                exit 91
            fi
        done
        [ "$saw_id" -eq 1 ] || {
            echo "resolved image identity did not reach execution" >&2
            exit 92
        }
        cat >/dev/null
        printf '  OK   hostile fixture inspected immutable image identity\n'
        ;;
    *)
        echo "unexpected podman invocation: $*" >&2
        exit 93
        ;;
esac
FAKE_PODMAN
    chmod 0755 "$fixture/podman"

    if ! output="$(
        PATH="$fixture:$PATH" EXPECTED_IMAGE_ID="$expected_id" \
            "$SCRIPT_PATH" localhost/magic-mesh-bootc:hostile 2>&1
    )"; then
        printf '%s\n' "$output" >&2
        echo "[FAIL] mutable image tag crossed the production-verification boundary" >&2
        return 1
    fi
    grep -Fq "ALL STATIC CHECKS PASS" <<<"$output" || {
        printf '%s\n' "$output" >&2
        echo "[FAIL] hostile fixture did not complete the verifier" >&2
        return 1
    }
    echo "[PASS] hostile mutable image tag cannot replace the inspected bootc candidate"
}

if [ "${1:-}" = "--self-test" ]; then
    [ "$#" -eq 1 ] || { echo "FATAL: --self-test takes no arguments" >&2; exit 2; }
    self_test
    exit
fi

TAG="${1:-localhost/magic-mesh-bootc:latest}"

command -v podman >/dev/null 2>&1 || { echo "FATAL: podman not on PATH" >&2; exit 1; }
IMAGE_ID="$(podman image inspect --format '{{.Id}}' "$TAG")" \
    || { echo "FATAL: image not in local storage: $TAG (build it first)" >&2; exit 1; }
[[ "$IMAGE_ID" =~ ^sha256:[0-9a-f]{64}$ ]] \
    || { echo "FATAL: image has a malformed immutable identity: $TAG" >&2; exit 1; }

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
for p in kernel-surface iptsd libwacom-surface surface-control surface-secureboot fwupd; do
    rpm -q "$p" >/dev/null 2>&1 \
        && ok "required Surface package installed: $p" \
        || bad "required Surface package missing: $p"
done
virsh --version >/dev/null 2>&1 \
    && ok "virsh executes ($(virsh --version))" \
    || bad "virsh does not execute"
ovs-vsctl --version >/dev/null 2>&1 \
    && ok "ovs-vsctl executes ($(ovs-vsctl --version | head -n 1))" \
    || bad "ovs-vsctl does not execute"
[ ! -e /usr/bin/cloud-hypervisor ] \
    && ok "cloud-hypervisor absent per CONSTRUCT-CLOUD/QC-1" \
    || bad "cloud-hypervisor still present"
[ -f /usr/lib/bootc/install/50-magic-mesh.toml ] \
    && ok "bootc install rootfs config present" \
    || bad "bootc install rootfs config missing"
grep -q 'type = "xfs"' /usr/lib/bootc/install/50-magic-mesh.toml 2>/dev/null \
    && ok "bootc install rootfs default = xfs" \
    || bad "bootc install rootfs default is not xfs"
boot_status_kargs=/usr/lib/bootc/kargs.d/10-mcnf-boot-status.toml
[ -f "$boot_status_kargs" ] \
    && ok "truthful pre-Construct boot-status kargs present" \
    || bad "truthful pre-Construct boot-status kargs missing"
for arg in plymouth.enable=0 rd.plymouth=0 systemd.show_status=1 rd.systemd.show_status=1; do
    grep -Fq "\"$arg\"" "$boot_status_kargs" 2>/dev/null \
        && ok "boot-status karg present: $arg" \
        || bad "boot-status karg missing: $arg"
done
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
[ -f /usr/lib/systemd/system/iptsd@.service ] \
    && ok "iptsd per-device template present" \
    || bad "iptsd@.service missing"
[ -f /usr/share/surface-secureboot/surface.cer ] \
    && ok "linux-surface Secure Boot certificate present" \
    || bad "linux-surface Secure Boot certificate missing"
for generation in 5 6; do
    [ -f "/usr/share/iptsd/surface-pro-${generation}.conf" ] \
        && ok "iptsd Surface Pro ${generation} preset present" \
        || bad "iptsd Surface Pro ${generation} preset missing"
done

# The seat unit, its preset, and the role gate.
[ -f /usr/lib/systemd/system/mde-shell-egui.service ] \
    && ok "seat unit installed" || bad "seat unit missing"
[ -f /usr/lib/systemd/system/mcnf-boot-status.service ] \
    && ok "informative boot status unit installed" || bad "informative boot status unit missing"
[ -f /usr/lib/systemd/system-preset/45-mcnf-quasar.preset ] \
    && ok "seat preset installed" || bad "seat preset missing"
grep -q 'ExecCondition=/usr/bin/mackesd role-gate --min-rank 1' /usr/lib/systemd/system/mde-shell-egui.service \
    && ok "typed role gate present in seat unit" || bad "typed role gate missing from seat unit"
grep -q '^Delegate=yes$' /usr/lib/systemd/system/mde-shell-egui.service \
    && ok "seat unit delegates cgroups for managed guest workloads" \
    || bad "seat unit missing Delegate=yes for managed guest workloads"
grep -q '^Environment=MDE_MAPS_DIR=/var/lib/mde/maps$' /usr/lib/systemd/system/mde-shell-egui.service \
    && ok "seat unit pins offline Maps data to persistent storage" \
    || bad "seat unit missing persistent offline Maps data root env"
grep -q '^OnFailure=getty@tty1.service$' /usr/lib/systemd/system/mde-shell-egui.service \
    && ok "seat unit restores tty1 only after terminal failure" \
    || bad "seat unit missing terminal-failure tty1 recovery"
! grep -q '^ExecStopPost=' /usr/lib/systemd/system/mde-shell-egui.service \
    && ok "seat unit does not race normal restarts with getty" \
    || bad "seat unit still has unconditional ExecStopPost recovery"

# Enablement symlinks (systemctl reads links; no running systemd needed).
for u in mde-shell-egui.service mcnf-boot-status.service podman.socket mackesd.target nebula.service \
         magic-setup.service mesh-health.timer \
         cloud-init-local.service cloud-init.service cloud-config.service \
         cloud-final.service qemu-guest-agent.service openvswitch.service; do
    state="$(systemctl is-enabled "$u" 2>/dev/null || true)"
    [ "$state" = enabled ] && ok "enabled: $u" || bad "$u is '$state' (want enabled)"
done
[ ! -e /usr/lib/systemd/system/mackesd.service ] \
    && ok "retired monolithic mackesd.service is absent" \
    || bad "retired monolithic mackesd.service remains installed"
for group in control observation actions data compute integrations; do
    unit="mackesd-$group.service"
    [ -f "/usr/lib/systemd/system/$unit" ] \
        && ok "group unit installed: $unit" || bad "group unit missing: $unit"
    grep -q "^ExecStart=/usr/bin/mackesd serve --group $group$" "/usr/lib/systemd/system/$unit" 2>/dev/null \
        && ok "group command pinned: $unit" || bad "group command invalid: $unit"
done
[ "$(systemctl get-default 2>/dev/null)" = graphical.target ] \
    && ok "default target = graphical" || bad "default target != graphical"

# Channel + state doctrine artifacts.
[ -f /etc/yum.repos.d/magic-mesh.repo ] && ok "channel .repo present" || bad "channel .repo missing"
grep -q 'mesh-storage' /usr/lib/tmpfiles.d/magic-mesh.conf 2>/dev/null \
    && ok "tmpfiles doctrine present" || bad "tmpfiles magic-mesh.conf missing/short"
grep -q '^d /var/lib/mde/maps 0755 root root -$' /usr/lib/tmpfiles.d/magic-mesh.conf 2>/dev/null \
    && ok "persistent offline Maps root present" || bad "tmpfiles missing /var/lib/mde/maps"

exit "$fail"
INNER
)"

# -i is load-bearing: without it the container's stdin is closed, `bash -s`
# reads EOF, runs ZERO checks and exits 0 — a false green (caught live).
rc=0
out="$(printf '%s\n' "$INNER_SCRIPT" | podman run --rm -i "$IMAGE_ID" /bin/bash -s)" || rc=$?
printf '%s\n' "$out"

# Silence is not success: a run that produced no itemized lines is a failure
# even if podman exited 0 (the stdin/exec regression tripwire).
grep -q '^  OK '   <<<"$out" || { echo "FATAL: no checks executed — stdin/exec regression" >&2; rc=1; }
grep -q '^  FAIL ' <<<"$out" && rc=1

if [ "$rc" -eq 0 ]; then
    echo "==> verify-image: ALL STATIC CHECKS PASS for $TAG ($IMAGE_ID; boot acceptance still gated)"
else
    echo "==> verify-image: FAILURES above for $TAG" >&2
fi
exit "$rc"
