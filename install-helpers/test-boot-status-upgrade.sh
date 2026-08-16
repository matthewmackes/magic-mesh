#!/bin/bash
# Focused package-contract gate for the blank pre-splash boot correction.
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
unit="$repo/packaging/bootc/units/mde-shell-egui.service"
manifest="$repo/crates/mesh/mackesd/Cargo.toml"
bootc_kargs="$repo/packaging/bootc/kargs.d/10-mcnf-boot-status.toml"
bootc_containerfile="$repo/packaging/bootc/Containerfile"
boot_status_unit="$repo/packaging/systemd/mcnf-boot-status.service"
boot_status_helper="$repo/install-helpers/mcnf-boot-status.sh"

command -v python3 >/dev/null 2>&1 || {
  echo 'boot-status: python3 is required for RPM transaction verification' >&2
  exit 2
}

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
grep -Fq 'mcnf-boot-status.service' "$manifest"
grep -Fq 'ExecStart=/usr/libexec/mackesd/mcnf-boot-status' "$boot_status_unit"
grep -Fq 'RuntimeDirectory=mde' "$boot_status_unit"
grep -Fq 'RuntimeDirectoryPreserve=yes' "$boot_status_unit"
grep -Fq 'Restart=on-failure' "$boot_status_unit"
grep -Fq 'boot-status.tsv' "$boot_status_helper"
grep -Fq 'logger -t mcnf-boot-status' "$boot_status_helper"
grep -Fq 'boot status projection complete' "$boot_status_helper"
! grep -Fq 'rm -f "$READY_FILE"' "$boot_status_helper"

# Fresh bootc installs must not regress the RPM upgrade correction.  Explicit
# Plymouth disablement wins even when the upstream base image still adds rhgb.
grep -Fq 'COPY packaging/bootc/kargs.d/10-mcnf-boot-status.toml /usr/lib/bootc/kargs.d/10-mcnf-boot-status.toml' "$bootc_containerfile"
for arg in plymouth.enable=0 rd.plymouth=0 systemd.show_status=1 rd.systemd.show_status=1; do
  grep -Fq "\"$arg\"" "$bootc_kargs"
done

# The base RPM replaces the retired monolithic daemon with the six grouped
# owners, including the sole Workloads/libvirt/Quadlet actuator.  Verify this as
# an ordered transaction rather than as independent payload tokens: preserve
# whether either owner was active, stop the old owner, remove both local and
# vendor unit definitions, reload systemd, enable the grouped target, and only
# then restore service.  The final corrected-forward restart must happen after
# package setup and must not hold the RPM transaction lock.
python3 - "$manifest" <<'PY'
import pathlib
import sys


def post_install(manifest: str) -> str:
    marker = 'post_install_script = """\n'
    start = manifest.find(marker)
    if start < 0:
        raise ValueError("base RPM post-install script is missing")
    start += len(marker)
    end = manifest.find('\n"""', start)
    if end < 0:
        raise ValueError("base RPM post-install script is unterminated")
    return manifest[start:end]


def require_order(script: str, labels_and_tokens: tuple[tuple[str, str], ...]) -> None:
    previous = -1
    for label, token in labels_and_tokens:
        position = script.find(token)
        if position < 0:
            raise ValueError(f"missing {label}: {token}")
        if position <= previous:
            raise ValueError(f"{label} is out of transaction order")
        previous = position


def verify(script: str) -> None:
    require_order(script, (
        ("retired-owner activity capture",
         "systemctl is-active --quiet mackesd.service && mackesd_was_active=1"),
        ("grouped-owner activity capture",
         "systemctl is-active --quiet mackesd.target && mackesd_target_was_active=1"),
        ("retired-owner stop",
         "systemctl disable --now mackesd.service"),
        ("retired local-unit removal",
         "rm -f /etc/systemd/system/mackesd.service"),
        ("retired vendor-unit removal",
         "\n      /usr/lib/systemd/system/mackesd.service \\\n"),
        ("owner-table reload",
         "systemctl daemon-reload"),
        ("grouped-owner enable",
         "systemctl enable mcnf-boot-status.service mackesd.target"),
        ("retired-owner migration guard", 'if [ "$mackesd_was_active" -eq 1 ]; then'),
        ("grouped-owner migration start", "systemctl start mackesd.target"),
        ("package setup completion", "timeout 60 update-desktop-database"),
        ("active grouped-owner restart guard",
         'if [ "$mackesd_target_was_active" -eq 1 ]; then'),
        ("non-blocking corrected-forward restart",
         "systemctl --no-block try-restart mackesd.target"),
    ))

    if "systemctl try-restart mackesd.target" in script:
        raise ValueError("synchronous grouped restart holds the RPM transaction lock")

    migration_guard = script.index('if [ "$mackesd_was_active" -eq 1 ]; then')
    migration_end = script.index("\nfi", migration_guard)
    migration_start = script.index("systemctl start mackesd.target", migration_guard)
    if migration_start > migration_end:
        raise ValueError("grouped migration start escaped its retired-owner guard")

    restart_guard = script.index('if [ "$mackesd_target_was_active" -eq 1 ]; then')
    restart_end = script.index("\nfi", restart_guard)
    restart = script.index("systemctl --no-block try-restart mackesd.target", restart_guard)
    if restart > restart_end:
        raise ValueError("grouped corrected-forward restart escaped its activity guard")


manifest_path = pathlib.Path(sys.argv[1])
script = post_install(manifest_path.read_text(encoding="utf-8"))
verify(script)

# Prove the verifier fails closed for the ordering and ownership regressions it
# exists to catch.  These fixtures mutate only the in-memory RPM script.
hostile = {
    "retired vendor owner retained": script.replace(
        "      /usr/lib/systemd/system/mackesd.service \\\n", "", 1),
    "grouped owner enabled before reload": script.replace(
        "systemctl daemon-reload >/dev/null 2>&1 || :\n", "", 1
    ).replace(
        '\nif [ "$mackesd_was_active" -eq 1 ]; then',
        '\nsystemctl daemon-reload >/dev/null 2>&1 || :\nif [ "$mackesd_was_active" -eq 1 ]; then',
        1,
    ),
    "unguarded migration start": script.replace(
        'if [ "$mackesd_was_active" -eq 1 ]; then\n  systemctl start mackesd.target >/dev/null 2>&1 || :\nfi',
        'systemctl start mackesd.target >/dev/null 2>&1 || :',
        1,
    ),
    "blocking corrected-forward restart": script.replace(
        "systemctl --no-block try-restart mackesd.target",
        "systemctl try-restart mackesd.target",
        1,
    ),
}
for label, mutated in hostile.items():
    if mutated == script:
        raise SystemExit(f"hostile fixture did not mutate contract: {label}")
    try:
        verify(mutated)
    except ValueError:
        continue
    raise SystemExit(f"upgrade contract accepted hostile fixture: {label}")

print("boot-status: Workloads owner upgrade transaction passed; rejected 4 hostile fixtures")
PY

echo 'boot-status: RPM and bootc ordering/status contracts plus retired-unit cleanup present'
