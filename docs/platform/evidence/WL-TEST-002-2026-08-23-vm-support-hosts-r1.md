# WL-TEST-002 — VM support on proof seats (2026-08-23)

Observed, then corrected-forward. No dest invented. No `production_admitted` flip.

## What was wrong

Dell (`172.20.146.225`), Seat 15 (`172.20.0.15`), and Surface (`172.20.146.79`)
all have `/dev/kvm`, `kvm_intel`, `qemu-kvm`, and active `virtqemud`.
Construct still showed no VM support because:

1. `mackesd-compute` and `mackesd-observation` never started. Vendor drop-in
   `Requires=mcnf-collaboration-identity.service` failed on the missing
   `/etc/mcnf/release-inputs/collaboration/collaboration-identity-receipt.json`.
2. `event/provider/virtualization` probed only `libvirtd.service` and parsed
   pool-info for an `Active:` line. Fedora modular hosts run `virtqemud`;
   `virsh pool-info` reports `State: running`. Dell's managed pool is
   `mde-vms` / `images`, not `default`.

`event/kvm/services` already accepted `virtqemud` (MV-2 `decide`). The
provider topic and the compute group did not.

## Source fix

- Compute and observation ship
  `mackesd-collaboration-identity-optional.conf` (`Wants=`, not `Requires=`).
  Control / actions / data / integrations stay `Requires=` so authenticated
  publication remains fail-closed.
- `gather_virtualization` folds the `libvirtd` catalog probe units
  (`virtqemud.service` / `.socket`).
- Storage readiness accepts `mde-vms`, `default`, or `images`, and parses
  `State: running`.
- `workload_compute` applies the `node-virt.yml` default-pool recipe when no
  accepted pool exists.
- RPM post-install adds known seat users to `libvirt`.

## Live seat mutation (red alert + 5s)

Replaced packaged
`/usr/lib/systemd/system/mackesd-{compute,observation}.service.d/40-collaboration-identity.conf`
with the optional `Wants=` drop-in (empty `Requires=` in `/etc` does not
reset on these Fedora units). Started compute + observation. Added `mm` to
`libvirt`. Defined `default` only on Surface (Dell already had `images` +
`mde-vms`; Seat 15 already had `default`).

| Seat | compute | observation | `event/kvm/services` | provider topic (installed 13.0.0-35) |
|---|---|---|---|---|
| Dell | active | active | 5/5, virtqemud | `unknown` — libvirtd-only probe |
| Seat 15 | active | active | 5/5, virtqemud; `browser-vm` running | `unknown` — pool `Active:` parse |
| Surface | active | active | 4/5 (podman.socket down) | `unknown` — same parse |

`mm` can now `virsh --connect qemu:///system uri` on the three proof seats.
Eagle and T480 also have `/dev/kvm` and the same identity `Requires=` hang;
`sudo -n` is not available there, so they were not mutated.

## Leftover

Provider `Ready` needs the new `mackesd` on a seat. Collaboration identity
dest is still absent (fail-closed for chat/collab, not for KVM). S6 guest
launch / DRM proof remains. Do not invent the identity receipt.
