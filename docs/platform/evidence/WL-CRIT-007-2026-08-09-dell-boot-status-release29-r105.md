# WL-CRIT-007 — Dell release 29 truthful boot acceptance

Date: 2026-08-09

## Scope

This is the corrected-forward reboot acceptance for the Dell workstation at
`172.20.146.225` after `magic-mesh-12.1.6-29.x86_64` was installed. It confirms
that the release-25 truthful pre-Construct boot correction remains effective in
the exact release-29 payload. It does not claim physical suspend/resume or a
photographed pre-DRM frame.

The required red `AI-GENERATED-ALERT` was published by the installed
`/usr/libexec/mackesd/seat-update-warning`, which held the five-second operator
window before `systemctl reboot`. Before reboot, `browser-vm` was running,
persistent, autostart-enabled, and allocated 8 GiB. Its UUID was
`a1100a2f-5b65-4064-ac9f-925e1affa1fb`.

## Reboot and boot-status result

The boot ID changed from
`575294f6-c914-4e45-bfb4-e1dd574b2334` to
`2ee46140-f9af-4eb2-ab23-2142af2c2587`. The exact live command line is:

```text
BOOT_IMAGE=(hd0,gpt1)/vmlinuz-7.1.4-204.fc44.x86_64 root=/dev/mapper/fedora-root ro rd.lvm.lv=fedora/root quiet systemd.show_status=1 rd.systemd.show_status=1
```

`rhgb` remains absent and both initrd and userspace systemd status are enabled.
The final boot accounting was:

```text
firmware 6.420s + loader 8.810s + kernel 1.080s + initrd 5.299s
userspace 1min 8.780s; total 1min 30.390s
multi-user.target reached after 51.147s in userspace
```

Construct entered active state at monotonic `25.985852s`, with `NRestarts=0`
and `Result=success`. Its journal recorded the boot-splash handoff at 21:13:32,
approximately 43 seconds after the 21:12:49 boot start. The six grouped daemon
services were all active and `mackesd.target` entered active state at monotonic
`57.525848s`. The shell therefore did not wait for complete mesh convergence.

The shell critical path was bounded by network-online and the governed Music
credential service; it took 3.048s after its prerequisites. The mesh target's
long pole was the governed cloud-arm credential chain and completed at 51.146s
userspace. Periodic mesh-status/recipient jobs extended systemd's final
`FinishTimestampMonotonic` beyond multi-user and UI readiness; they did not gate
Construct or leave failed units.

## Preservation and integrity

- `rpm -q magic-mesh` returned `magic-mesh-12.1.6-29.x86_64`.
- `rpm -V magic-mesh` emitted no differences.
- All six grouped daemon services and `mackesd.target` were active.
- `mde-shell-egui.service` was active with one main PID, zero restarts, and a
  successful result.
- `systemctl --failed` contained zero units.
- `browser-vm` returned running with the same UUID, 4 vCPUs, 8 GiB, persistent
  definition, autostart enabled, and SELinux enforcing confinement.

The reported blinking-cursor defect is therefore corrected in release 29: the
kernel exposes truthful systemd progress before DRM ownership, Construct paints
well before complete mesh convergence, and the governed Browser workload
survives the reboot. Fleet-wide reboot/suspend proof remains part of the parent
epic.
