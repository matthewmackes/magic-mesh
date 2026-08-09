# WL-CRIT-007 — bootc truthful pre-Construct status parity r101

Date: 2026-08-09

## Incident and ownership

The operator reported that Dell displayed only a blinking cursor for more than
60 seconds before the Construct splash. This belongs to WL-CRIT-007 because it
is a boot-order, visible recovery-state, and corrected-forward upgrade failure.

Release 25 already corrected the installed RPM path by removing `rhgb`, enabling
initrd and host systemd status, removing two retired host-Browser SELinux
oneshots, and allowing the DRM shell to paint before every mackesd group has
converged. The remaining source gap was the immutable bootc install path:
`00-mcnf-virt.toml` carried virtualization arguments but no equivalent visible
boot-status policy. A fresh image could therefore inherit `rhgb` from its base
and hide real start-job progress even though an upgraded RPM seat did not.

## Correction

`packaging/bootc/kargs.d/10-mcnf-boot-status.toml` now adds:

```text
plymouth.enable=0
rd.plymouth=0
systemd.show_status=1
rd.systemd.show_status=1
```

The explicit Plymouth disablement wins even if a base image supplies `rhgb`.
Kernel chatter remains quiet; initrd and host systemd report the unit/start job
that is actually running until `mde-shell-egui` owns DRM and paints Construct.
The Containerfile installs this contract, the image verifier checks its payload,
and the focused RPM upgrade gate now prevents RPM/bootc policy divergence.

This cannot display status during firmware or bootloader execution because the
OS has not initialized a console then. Dell's measured firmware plus loader
interval is about 15 seconds, not the former userspace delay.

## Read-only Dell diagnosis

No Dell state was changed. A bounded read-only SSH check as
`mm@172.20.146.225` reported:

```text
host=DELL-LAPTOP
rpm=magic-mesh-12.1.6-27.x86_64
boot_id=575294f6-c914-4e45-bfb4-e1dd574b2334
cmdline=... quiet systemd.show_status=1 rd.systemd.show_status=1
Startup finished in 6.216s (firmware) + 8.794s (loader) + 1.071s (kernel)
  + 5.171s (initrd) + 57.818s (userspace) = 1min 19.072s
multi-user.target reached after 52.724s in userspace.
```

There is no active `rhgb`. Dell is already running the release-25 correction
inside release 27. Its boot ID is unchanged from the release-25 proof, so the
release-27 package upgrade itself has not supplied a new reboot observation.

## Verification

Local syntax/probe checks passed:

```text
bash -n install-helpers/test-boot-status-upgrade.sh packaging/bootc/verify-image.sh
install-helpers/test-boot-status-upgrade.sh
  boot-status: RPM and bootc ordering/status contracts plus retired-unit cleanup present
git diff --check
```

Farm host `172.20.0.90`, slot `crit007-bootc-status-r101`, passed the focused
contract after `xcp-build.sh sync`:

```text
install-helpers/test-boot-status-upgrade.sh
  boot-status: RPM and bootc ordering/status contracts plus retired-unit cleanup present
bash -n packaging/bootc/verify-image.sh install-helpers/test-boot-status-upgrade.sh
  PASS
python3 (tomllib exact kargs assertion)
  boot-status TOML: 4/4 exact kargs parsed
```

The attempted farm `git diff --check` was not a test failure: farm syncs omit
`.git`, so Git correctly refused to treat the staged directory as a repository.
The equivalent check passed in the authoritative local worktree.

## Dell rollout handoff

After integration, build the next native Fedora 44 RPM on the dedicated F44
builder and bind its hash to the release evidence. Before touching Dell, publish
the required red `AI-GENERATED-ALERT` and wait five seconds. Copy the RPM, verify
its SHA-256 on Dell, run `sudo rpm -Uvh --test` as a separate command, then run
the real `sudo rpm -Uvh`; verify the installed NVR, package integrity, shell,
mackesd target, Nebula, and current kernel command line.

No reboot is required to activate this source slice on Dell: its live release
27 kernel already carries the truthful status arguments, and the new files are
bootc-image policy. A separately warned reboot is required only to obtain a new
end-to-end visual/timing proof for the integrated release. After that reboot,
confirm the boot ID changed, capture `/proc/cmdline`, `systemd-analyze time`, the
shell and mackesd activation timestamps, service restart counts, and a physical
or pre-DRM console frame if capture hardware is available.

No package was built, installed, or deployed in this slice.
