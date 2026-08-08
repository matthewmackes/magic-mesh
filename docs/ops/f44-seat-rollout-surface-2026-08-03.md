# Fedora 44 seat rollout and Surface onboarding — 2026-08-03

This is a bounded live-operations evidence record, not a second worklist. The
only active tracker remains `docs/platform/WORKLIST.md`.

## Build artifact

- Native Fedora 44 build host: temporary BigBoy VM `mcnf-build-f44`
  (`172.20.0.131`), using the source state at `a7b26cec`. Repository HEAD
  `169c866c` adds only the preceding rollout handoff, so the runtime source is
  unchanged between those revisions.
- RPM: `magic-mesh-12.1.6-1.x86_64.rpm`, 85,939,314 bytes (82.0 MiB).
- RPM SHA-256:
  `edb32a228e823a16c12383792b1da63c65326cb1d3f61e3832e8adaf288c9f54`.
- Embedded binary SHA-256 values:
  - `/usr/bin/mackesd`:
    `2c95882dbae5d8646f23f6707830badf730170acf6a33c15dd20d23e17f337f0`
  - `/usr/bin/mde-shell-egui`:
    `fa8d65065b82ccbd19ca3af48e73c97d0defb89b0abd0e111ada5c4b7ce03350`
- The F44 build, 90 MiB payload-size gate, real-RPM payload manifest gate,
  header/payload digest checks, and per-seat transaction tests passed. The RPM
  is unsigned and was installed as a review build; this is not signed-channel
  promotion evidence.

After the artifact was copied and verified, `mcnf-build-f44` was halted and the
canonical BigBoy farm VM `mcnf-build-52` (`172.20.0.130`) was started again.

## Existing-seat replacement

The same-version RPM was force-replaced only after its SHA-256 and transaction
test matched on every target. `mackesd` and `mde-shell-egui` were then restarted
without rebooting the seats. All four targets reported the embedded binary
hashes above, both services `active/running`, `NRestarts=0`, and active Nebula
plus mesh-health timers:

| Seat | LAN address | Result |
|---|---:|---|
| T480 | `172.20.146.138` | Replaced; Fedora `libklvanc` dependency added; full QEMU/KVM runtime completed after enrollment correction |
| Eagle | `172.20.146.145` | Replaced; Fedora `libklvanc` dependency and full QEMU/KVM runtime added |
| Basement seat 15 | `172.20.0.15` | Replaced; low free-space condition remains |
| Dell | `172.20.146.225` | Replaced; existing Browser VM domain was not destroyed |

The package post-install emitted user-bus transient-unit warnings on some
non-root SSH sessions. Explicit system-service restarts and post-restart state
checks were green, so those warnings did not represent failed seat services.

## T480 current-mesh correction and fleet convergence

The rollout audit found that T480's old `10.42.0.7` identity was isolated behind
a different, obsolete mesh CA; it was not colliding on the current data plane
with Surface. The old T480 Nebula, role, site, and `mackesd` state was preserved
in the root-only
`/var/lib/mde/onboarding-backup-20260803-t480-old-mesh` directory. The supported
leave path then removed that obsolete identity. A stale root-local relay trust
pin correctly refused the first cross-mesh enrollment attempt; it was archived
inside the same root-only backup instead of overwritten in place.

T480 was then enrolled with a fresh fingerprint-pinned, single-use token as
`peer:T480`, role `workstation`, overlay `10.42.0.8`. Its current CA SHA-256
matches Surface and the lighthouses at
`0b359a2378a0407ec824631a153aaaec62485e20f4220061bd3ea383e829bc6c`.
Client-only etcd endpoints, the Syncthing file plane, role services, and the
full `qemu-kvm-10.2.2-1.fc44` plus `libvirt-daemon-kvm-12.0.0-3.fc44` runtime
were completed without reboot. Eagle received the same missing QEMU/KVM
runtime. Seat 15 already has those packages but still exposes no `/dev/kvm`, so
it remains a VDI client/non-KVM target rather than a local VM host.

The converged control-plane view now reports eight healthy nodes, five
Workstations, three lighthouses, zero degraded or unreachable nodes, and
`ha_ok=true`. T480 and Surface retain unique overlays (`10.42.0.8` and
`10.42.0.7`, respectively).

## Basement seat 15 file-plane correction

A same-day fleet preflight found that seat 15's Syncthing configuration still
bound its GUI and transfer listener to retired overlay `10.42.0.1`, causing a
restart loop. The pre-correction configuration was preserved root-only at
`/var/lib/mde/syncthing-backup-20260803-seat15/config.xml.before-listen-fix`.
The installed supported setup helper was then rerun for seat 15's actual
overlay, `10.42.0.5`, with the existing four-peer mesh roster. Follow-up proof
showed `syncthing.service` active/running with `NRestarts=0` and listeners only
on `10.42.0.5:8384` and `10.42.0.5:22000`. No identity or peer credentials were
copied between seats.

Seat 15 still has only about 2 GiB free and does not expose `/dev/kvm`; those
are separate placement constraints. It remains a valid Workstation/VDI client
and observation seat, not a local VM placement target.

The operator then rebooted seat 15 as a live recovery drill. It returned at
08:49 EDT with the same `peer:Basement-Test-Workstation` identity and
`10.42.0.5` address. The shell, daemon, mesh-health timer, and libvirt socket
returned active. Two early Nebula starts and one Syncthing start occurred while
the hardware clock still read June; after time synchronized, the processes were
stable.

That reboot exposed a second stale artifact: `/etc/nebula/lighthouse-config.yaml`
overrode the Workstation's correct `am_lighthouse: false` setting because Nebula
merges every YAML file in its config directory. The stale file was moved to the
root-only
`/var/lib/mde/nebula-recovery-backup-20260803-seat15/lighthouse-config.yaml.before-workstation-fix`,
then Nebula was restarted with the seat identity unchanged. Seat 15 subsequently
reached all three lighthouses and all four other Workstations.

Syncthing's managed folder already contained exactly self plus those four
Workstation peers, but its global roster retained three unshared lighthouse
devices and made the watchdog report a false `4/7`. The current configuration
was backed up to
`/var/lib/mde/syncthing-backup-20260803-seat15/config.xml.before-stale-device-prune`,
and only those three named, folder-unreferenced devices were removed through the
Syncthing config API. Final proof reported four configured peers, four connected
peers, healthy coordination endpoints, and a successful `mesh-health.service`
run. All four other Workstations independently reported four of four file-plane
peers connected. The three lighthouses then agreed on eight online nodes, three
lighthouses, no unreachable nodes, and `ha_ok=true`; seat 15 remains the sole
degraded row because of its known disk-headroom alarm, so the final count is
seven healthy plus one degraded rather than an eight-healthy acceptance claim.
Both backups are local, root-only, and recoverable.

### Current VM-capable bench correction (2026-08-05)

The earlier no-`/dev/kvm` observation above is superseded by a fresh read-only
probe after the operator enabled VM support. Seat 15 now exposes `/dev/kvm`,
`qemu-kvm-10.2.2-1.fc44`, and `libvirt-daemon-kvm-12.0.0-3.fc44`; its
`virsh` daemon is reachable, root has about 17 GiB free, and no guest is yet
defined. It remains fixed-wired on `eno1` and retains the existing mesh
identity; no address, route, bridge, or firewall change was made. The seat is
now eligible for local Browser VM and Android VM test placement, subject to
the image-size and nested-guest gates.

## Microsoft Surface seat

The Microsoft Surface Pro 6 is a distinct seat at `172.20.146.79`; it is not
seat 15. The live onboarding result is:

- Fedora 44 Server, kernel `6.19.10-300.fc44.x86_64`, SELinux enforcing.
- `magic-mesh-12.1.6-1.x86_64` installed from the same verified F44 RPM.
- Full Fedora QEMU/KVM and libvirt host runtime installed, including
  `qemu-kvm-10.2.2-1.fc44` and `libvirt-daemon-kvm-12.0.0-3.fc44`;
  `virtqemud.socket` is active and boot-enabled.
- Fingerprint-pinned, single-use network enrollment completed as
  `peer:SURFACE`, role `workstation`, overlay `10.42.0.7`.
- The Workstation role provision applied all nine unit decisions with zero
  failures. Client-only etcd endpoints target all three lighthouses, and the
  overlay-only Syncthing file plane is active with zero restarts.
- All three public lighthouses list `SURFACE` as a healthy Workstation. After
  the T480 correction, the converged health view reports eight healthy nodes,
  three lighthouses, zero degraded/unreachable nodes, and `ha_ok=true`.
- The direct-DRM shell is `active/running`, `NRestarts=0`, on tty1. The live
  process holds `/dev/dri/card1`, seven input event devices, and `/dev/tty1`;
  the built-in `card1-eDP-1` connector is connected. Boot milestones reached
  seat, surfaces, mesh snapshot, and desktop handoff.
- The shell logged three non-fatal SVG marker warnings. The only failed system
  unit observed was `fwupd-refresh.service` (metadata refresh), which does not
  block the shell or mesh.

The enabled Fedora repositories do not currently provide `iptsd` or
`surface-control`; no external kernel repository, MOK operation, or reboot was
introduced in this rollout. Touch/pen support therefore remains unclaimed.
Likewise, the optional cloud-arming credential is not provisioned, so
privileged Bus mutations correctly remain disabled. The current health payload
also reports worker telemetry as `0/0` on the whole fleet; readiness and HA are
green, but that instrumentation needs separate reconciliation.

## Browser VM capacity boundary

Surface has `/dev/kvm`, eight CPUs, about 7.66 GiB total RAM, and roughly 32 GiB
free on the root filesystem after onboarding. The current Browser VM baseline
requires 8 GiB guest RAM and a 64 GiB disk, so this machine cannot honestly host
that baseline locally. Surface is ready as a Workstation and VDI client, and it
has the host runtime for smaller profiles, but the named `browser-vm` must be
placed on a roomier mesh node or receive a separately designed lower-resource
profile. No local Browser VM readiness is claimed here.

The separately built Chromium qcow2 candidate and Dell's historical SPICE
domain remain governed by `docs/ops/browser-vm-cutover-2026-08-02.md`; this
rollout did not convert that live domain into the required
Sunshine/Moonlight-default production path.
