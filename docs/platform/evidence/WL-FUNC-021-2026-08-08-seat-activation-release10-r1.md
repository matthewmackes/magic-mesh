# WL-FUNC-021 — deterministic seat activation, release 10 (2026-08-08)

## Outcome

Package upgrades now activate the executable payload they install and leave
exactly one seat-owned Music daemon authority. Release 10 was installed on all
five workstations without a follow-up service restart. T480, Eagle, seat 15,
Dell, and Surface each immediately reported one `mm`-owned `mde-musicd`, one
system `mde-shell-egui`, current `/usr/bin` executable mappings, RPM ownership
by `magic-mesh-12.1.6-10.x86_64`, active service state, and `NRestarts=0`.

This fixes the release-9 activation defect where an RPM transaction could
finish while release-8 executable inodes remained mapped. It also removes one
demonstrated source of unnecessary CPU work: global user-unit enablement had
started competing Music responders for multiple login accounts on Surface.

## Implementation and gate

- The base RPM disables the global `mde-musicd.service` edge, selects one seat
  account in bounded `mm`, then `mde` order, and disables the unit for other
  known accounts.
- User service operations connect directly to the selected account's runtime
  D-Bus. This replaces the failing `systemctl --user --machine=...` bridge.
- The scriptlet restarts the selected Music daemon and uses `try-restart` for
  already-active system `mackesd` and shell authorities after package setup.
- `install-helpers/test-rpm-seat-service-activation.sh` passed locally and on
  farm host `.50`; it also parses the embedded shell with `bash -n`.
- Native Fedora 44 builder `.131` produced
  `magic-mesh-12.1.6-10.x86_64.rpm`, 87,608,244 bytes, SHA-256
  `6e2e197e1e05b988f47b7ad96598c402d766b3c24276cbf1b9cfdac5e31afec5`.
  Full payload/size gates passed and the media sonames remain F44-native.

## Five-seat rollout proof

Every seat matched the artifact hash and passed
`rpm -Uvh --test --nosignature` before installation. Without manual service
repair, every seat then reported:

- `magic-mesh-12.1.6-10.x86_64` installed;
- global `mde-musicd.service` enablement disabled;
- exactly one `/usr/bin/mde-musicd serve`, owned by `mm`;
- exactly one `/usr/bin/mde-shell-egui`, owned by the system service;
- both paths owned by release 10, both services active, and both at
  `NRestarts=0`.

The reusable live-seat verifier passed on Dell and seat 15, including daemon
ping, typed state and album replies, package payload, and `rpm -V`. A fresh Dell
named-detail probe returned 9 albums for 38 Special, 23 for AC/DC, Black Ice
with 15 tracks and `Rock 'n' Roll Train` first, 31 podcasts, and 3 episodes for
`Wait Wait... Don't Tell Me!`. Dell's `browser-vm` remained defined and shut
off. Temporary RPM copies were removed and normal BigBoy `.130` was restored.

## Honest remaining boundary

This closes package activation and duplicate daemon authority, not all of
WL-FUNC-021. Live provider-loss continuity, physical renderer/cast proof,
cross-seat owner handoff, mutating playback and speaker judgment across every
seat, and synchronized five-seat CPU/NWS recovery remain open.
