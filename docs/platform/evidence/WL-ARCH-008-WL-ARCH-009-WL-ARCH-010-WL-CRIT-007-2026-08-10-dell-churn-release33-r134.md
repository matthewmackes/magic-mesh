# Dell Dom0 churn, VM profile, privacy, and boot-status correction

Date: 2026-08-10  
Final source revision: `3fd7b0c9c64c66a6664c51e90a1672c2ea7cff84`  
Physical test seats used: Dell only

## Root causes and source corrections

Dell's four-thread Dom0 was hosting a four-vCPU Browser VM while the guest,
emulator, I/O, Construct, Syncthing, and six daemon groups could all contend on
CPU 0. The live pre-change load was `8.21 7.52 6.16`. The Bus simultaneously
held 440 MiB under `/run/mde-bus`; `index.sqlite` was 132 MiB and its WAL was
146.6 MiB. Idle music peers also rewrote one shared global state file, producing
Syncthing conflict churn.

Release 33 changes the typed Dell Browser profile to 3 vCPU / 8 GiB / 64 GiB,
one socket with three cores, pins vCPUs to host CPUs 1, 2, and 3, pins the QEMU
emulator and dedicated I/O thread to CPUs 1-3, and uses `cache=none`,
`io=native`, discard, and that I/O thread for the retained qcow2 disk. CPU 0 is
therefore reserved for Dom0. Bus persistence now indexes compact active-topic
metadata, leaves bodies over 64 KiB in their canonical JSON rather than
duplicating them into SQLite, and targets an 8 MiB WAL. Idle music peers write
only their by-peer roster snapshot; the active playing owner remains the sole
global authority.

The first live release-33 reboot exposed two cold-boot races which were fixed
forward in `4da0d60e` and `3fd7b0c9`:

- `mcnf-boot-status.service` required `/run/mde` in its sandbox before the path
  existed. It now creates and preserves `RuntimeDirectory=mde`, exits only on
  success after Construct's ready marker, and never removes that marker during
  a service restart.
- A uid-1000 desktop process could create SQLite first in tmpfiles' root-owned
  sticky Bus directory. Root workers then attempted schema initialization
  before relaxing the directory and retried `attempt to write a readonly
  database`. Shared-directory and sidecar relaxation now happens before the
  first schema write.

## Focused farm proof

The Bus redesign ran on farm node `.50`, slot
`bus-dell-amplification-r132`: eight hostile exact tests passed, the complete
`mde-bus` library suite passed 430/430, formatting passed, and clippy with
`-D warnings` passed. The music authority test passed on `.50`, slot
`music-authority-r132`. The VM XML/affinity test passed against the complete
`mackesd` library harness on BigBoy `.130`, slot `dell-vm-affinity-r133` (one
exact pass; 4,676 filtered), and the request-helper self-test passed.

The live-race corrections then passed:

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=bus-order-r136 \
  install-helpers/xcp-build.sh cargo test -p mde-bus --lib \
  open_self_heals_after_recreate_and_stays_writable -- --nocapture
```

Result: one passed, 429 filtered. On `.90`, slot `boot-preserve-r136`,
`bash install-helpers/test-boot-status-upgrade.sh` passed the RPM/bootc ordering,
runtime-directory preservation, ready-marker, and retired-unit contracts.

## Privacy retention deployment

The fleet privacy contract permanently deletes system logs and application
histories and retains no more than six hours. It includes journald, flat logs,
Bus history, collaboration JSONL, transfer ledgers/outbox, Syncthing conflict
files, and version history; it preserves identities, settings, databases,
media, transfer payloads, and VM disks. System-log sweeps run hourly and
application epochs run synchronously at 00:00, 06:00, 12:00, and 18:00 with
`AccuracySec=1s` so a delayed replica cannot republish erased history.

The synchronized application timer is enabled on Dell, seat 15, Surface,
Dom0s `.9`, `.145.193`, `.145.165`, `.145.194`, and `.145.196`, build VMs
`.50`, `.90`, `.130`, `.170`, and `.196`, all three lighthouses, and the dev
host. T480 and Eagle remained outside the exact application-timer deployment
proof because root access was unavailable; this is the explicit privacy
boundary rather than an inferred pass.

On Dell, a warned manual epoch completed successfully in 30.056 seconds and
restarted all six daemon groups plus Syncthing. Bus storage immediately fell
from 440 MiB to 13 MiB; the index fell to 1.7 MiB and WAL to 1.1 MiB.

## Dell installation and reboot proof

The initial release-33 F44 compatibility-container artifact from `5b0f1e5e`
was 90,932,520 bytes with SHA-256
`26e2ddee201343e08b51193619c464e86c8ecdfae3ac496442e3c53089282795`.
Its size gate passed at 86.7 MiB, all expected payload assets and F44 QEMU
requirements were present, Dell matched the hash, and
`rpm -Uvh --test --replacepkgs --force --nosignature` passed before install.
The Fedora 44 container image was tag-pinned rather than digest-pinned and the
live rustup installer had no checksum pin; those are explicit reproducibility
limits. Native F44 builder `.131` was unreachable, so the documented F44
compatibility lane on BigBoy `.130` was used.

Dell's Browser guest shut down gracefully before the warned host reboot. The
host left the network at 10:05:33 EDT, ping returned after 38 seconds, and SSH
returned after 43 seconds with boot ID
`afeca5a0-fae2-427c-95f2-3e77c7f55631`. Firmware through userspace completed
in 59.134 seconds; multi-user was reached after 26.702 seconds of userspace.
The VM autostarted with UUID `a1100a2f-5b65-4064-ac9f-925e1affa1fb`, exact
overlay `browser-vm-r13-af3348bc-overlay.qcow2`, and exact control seed intact.
Live and configured vCPU counts are both 3; vCPUs are pinned 0→1, 1→2, 2→3;
emulator and I/O thread 1 are pinned to 1-3; QEMU's live command line confirms
one socket/three cores, native direct I/O, and `iothread1`.

After the live Bus directory correction and grouped restart, no read-only
database message occurred after 10:13 EDT. With the VM running, load fell to
`3.25 3.95 2.57`; no core remained pegged in the sampled interval. Bus storage
was 68 MiB during normal repopulation, with a 14 MiB index and 1.2 MiB WAL.

The corrected boot projector was hot-applied with the visible five-second
warning. It produced a 3.9 KiB `/run/mde/boot-status.tsv`, observed Construct's
ready marker, exited with `Result=success` / status 0, and preserved both files.

## Final exact-package handoff

The final immutable `3fd7b0c9` Fedora 44 artifact is 90,932,237 bytes with
SHA-256
`fc0eee5e8319d06e449bc1f20e74b1c8a913f640727a68ec074f40b9d32904a6`.
The 86.7 MiB payload size gate passed. Dell independently matched that hash,
passed a separate warned `rpm -Uvh --test --replacepkgs --force --nosignature`,
and then installed the same bytes with a second warning. `rpm -V magic-mesh`
is clean.

A final exact-package cold boot began at 10:42:29 EDT. Ping returned after 44
seconds and SSH after 51 seconds with boot ID
`cf9379f8-e375-403e-8a8c-36462e89192f`. The shared Bus directory was already
mode 0777 when inspected, its SQLite files were cross-uid writable, every
daemon group was active, and the entire boot journal contained zero `readonly
database` messages. The status projector started with zero restarts, wrote the
feed during boot, observed Construct's ready marker, logged graphical handoff
at 10:44:41, and exited status 0 while preserving the 3.9 KiB feed and marker.
No system unit was failed.

The exact guest again autostarted with 3 live/configured vCPUs and the required
pins, emulator pin, I/O-thread pin, and unchanged overlay/control seed. During
the first minutes after boot, the Windows guest was still consuming up to
127% host CPU while starting; that transient boot load is not presented as an
idle result. A later 30-second `vmstat` sample had a run queue of 0-3 and
30-44% aggregate idle CPU; a separate 15-second `/proc/stat` delta measured
CPU0/1/2/3 at 48.1%, 44.8%, 48.9%, and 58.9% utilization respectively, proving
that no core remained pegged. QEMU had fallen to 84.9% of one host core. Bus
storage was 55 MiB during repopulation, the index 12 MiB, and WAL 1.1 MiB.
