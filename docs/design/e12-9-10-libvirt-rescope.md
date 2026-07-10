# E12-9 / E12-10 re-scope — audio, VFIO/USB passthrough, and live migration on libvirt/QEMU-KVM

**Status:** DESIGN — options + recommendation, 2026-07-10. Written after QC-15 deleted
`crates/services/mde-kvm` (the cloud-hypervisor broker) outright, which reopened two
WORKLIST gaps that were still described against that now-deleted stack. This doc
re-scopes both against the live libvirt/QEMU-KVM code
(`crates/mesh/mackesd/src/workers/{compute_provision,compute_migrate,vm_lifecycle}.rs`).
Not implemented here — design/options only, per the two flagged operator-decision
points below (§"open question" in each of the migration and VFIO sections).
**Companions:** `docs/design/mesh-virt-management.md` (the management-layer lock this
doc's workers implement), `docs/design/quasar-cloud.md` (the parallel Nova/OpenStack
epic — see the tension flagged in §Live migration), `docs/design/quasar-vdi-desktop.md`
(locks 5/13/48, the original E12-9/E12-10 acceptance text).

## Current architecture (shared context for both gaps)

**Two independent VM-creation code paths exist today**, both writing into the same
`mde-vms` libvirt dir-pool (`/var/lib/mde-vms`), but built independently:

1. `compute_provision.rs` (VIRT-6) — the mesh-desktop flow. Drains
   `compute/create/<own-nebula-addr>`, allocates a Nebula IP + cert for the guest, and
   shells `virt-install` with an argv `build_virt_install_args` assembles.
2. `vm_lifecycle.rs` (MV-3) — the Datacenter-UI-facing flow. Drains
   `action/vm/lifecycle`, and shells `virsh define` against a **hand-built domain XML
   string** `build_domain_xml` assembles, applied through the injectable
   `LibvirtBackend` trait (prod impl `VirshCli`).

Neither path passes any sound/USB/PCI-hostdev flags today (grep-verified: zero
`sound`/`audio`/`hostdev`/`vfio` hits anywhere under `crates/`). **Any new device
stanza (audio, USB, PCI) needs to land in both builders, or the two paths need
reconciling into one.** This doc flags that factoring cost per-gap below rather than
solving it — picking a shared "build device stanza" helper is a reasonable follow-on
but is its own small refactor, not bundled into either recommended first slice.

**Console/render paths are also currently split**, which matters for where audio
naturally rides:
- The E12 "remote desktop over the mesh" / roaming VDI session (the shell's Desktop
  surface, `mde-shell-egui/src/vdi.rs`) renders via `mde-vdi-rdp` (primary) / `mde-vdi-vnc`
  (fallback) — quasar-vdi-desktop.md lock 21.
- The QC-13 Cloud-plane console-attach (`mde-shell-egui/src/cloud_plane.rs`,
  `console_attach_request`) now dials native SPICE via `mde-vdi-spice` when Nova
  returns a direct `spice://host:port` descriptor — quasar-cloud.md Q34 ("SPICE +
  `mde-vdi-spice` AS THE console experience").
- `vm_lifecycle.rs::build_domain_xml` already defaults every VM's `<graphics>` to
  `type='spice'` (`autoport='yes'`, localhost-listen) — consistent with the QC-13
  direction, not the E12 RDP-primary lock, since it's authoring the libvirt side QC-13
  actually dials.

**No shared VM-disk storage exists.** Each peer's `/var/lib/mde-vms` is local-only;
the live VIRT-8 cold migration (`compute_migrate.rs`, **already done, not touched by
this doc**) ships the whole disk by `rsync` after an ACPI shutdown specifically
because there is no shared filesystem to migrate onto. This was already flagged as a
known, accepted risk *before* QC-15 — `mesh-virt-management.md`'s Risks section:
*"Live migration / shared storage are not free on KVM the way XCP-ng gives them; the
mesh provides node-loss resilience (sessions roam) instead of hypervisor HA."* QC-15
didn't create this constraint; it deleted code that (per the now-disproven "CORES
LANDED" WORKLIST claim) apparently papered over it on cloud-hypervisor.

**A parallel, broader epic already made adjacent calls that pull the other way.**
`quasar-cloud.md` (QUASAR-CLOUD, locked 2026-07-03, "Nova+Placement own ALL VM
lifecycle") locks Q37 passthrough as **"Nova PCI + vGPU flavors"** (a heavier,
Placement-inventory-driven mechanism for Nova instances — not a substitute for the
direct-libvirt `<hostdev>` XML this doc scopes against the VIRT-6/MV-3 layer) and Q38
migration as **"Not a goal — roaming = rebuild from image/volume; no live
migration,"** explicitly listing live migration as out of scope for the cloud-instance
path. E12-10's original lock 48 ("everything in v1") predates that call by two days
and targets a different VM population (the mesh-native/VDI-session path this doc's
target files implement, not Nova-brokered cloud instances) — so it's not a flat
contradiction, but it is a live, unresolved tension. See §Live migration.

---

## Gap 1 (E12-9): audio bridging

### What's already built and waiting

The E12-16 DAW mixer (`crates/desktop/mde-seat/src/mixer.rs`, **already shipped**) reads
the PipeWire graph (`pw-dump`) and classifies every stream's origin:

```rust
// mde-seat/src/mixer.rs (existing, unmodified)
const PROP_VM_NAME: &str = "mde.vm.name";   // presence ⇒ StripOrigin::LocalVm(name)

fn classify_origin(props: &serde_json::Value) -> StripOrigin {
    // mde.mesh.peer checked first, then:
    if let Some(vm) = props.get(PROP_VM_NAME).and_then(serde_json::Value::as_str) {
        return StripOrigin::LocalVm(vm.to_owned());
    }
    StripOrigin::HostSession
}
```

Nothing in the codebase stamps `mde.vm.name` today (repo-wide grep confirms it) — the
doc comment above it describes what "the VM audio bridge" *would* do. **The consumer
is fully built and tested; only the producer is missing.** This is the load-bearing
fact for the recommendation below: closing the local-audio half is mostly about
getting one PipeWire property set correctly, not building a mixer.

### Options

| # | Path | Mechanism | New client protocol code? | Verdict |
|---|---|---|---|---|
| A | **Local** | QEMU/libvirt native `<audio type='pipewire'>` audiodev | None | **Recommended first slice** |
| B | **Remote, SPICE** | SPICE native playback/record channel | Yes — real (see below) | Right direction, not the first slice |
| C | **Remote, RDP** | ironrdp RDPSND virtual channel | Yes — blocked on pinned ironrdp | Stays WON'T-DO (operator, 2026-07-03) |
| D | **Remote, VNC** | — | — | Ruled out: RFB has no standard audio channel |

### Option A — local audio via QEMU's native PipeWire audiodev (recommended)

**Mechanism.** libvirt (verified against `formatdomain.html` and the libvirt-devel
"Introduce pipewire audio backend" patchset) supports a domain-level `<audio>` element
whose `pipewire` backend takes `name` (target sink/source), `streamName` (an
identifying label for the stream — exactly what's needed here), and `latency`
attributes on its `<input>`/`<output>` children, paired with a `<sound>` device:

```xml
<!-- illustrative addition to vm_lifecycle.rs::build_domain_xml's <devices> block -->
<sound model='virtio'/>
<audio id='1' type='pipewire'>
  <output name='mde-vms' streamName='vm-{name}' latency='40'/>
</audio>
```

QEMU's QEMU-native `pipewire` audiodev backend has shipped since QEMU 8.0 (2023); the
farm's QC-1 evidence already shows QEMU 9.2.4 / libvirt 11.0.0 live, so version support
isn't a concern. This makes the QEMU process itself a normal PipeWire client on
whichever host is running the VM — it reaches a user physically at (or otherwise
locally consuming PipeWire from) *that* host only. It does not cross the network. That
is exactly quasar-vdi-desktop.md lock 5's "virtio-sound for local VMs" half.

**Where it lives.** Both create paths need it: `vm_lifecycle.rs::build_domain_xml`
(add the XML above) and `compute_provision.rs::build_virt_install_args` (the
`virt-install` equivalent, e.g. `--sound model=virtio --video ...` plus an
`--qemu-commandline`/audio flag, or a shared helper — see the two-paths note above).

**The one open verification step.** libvirt's `streamName` attribute is confirmed to
exist and to be "the name to identify the stream associated with the VM" — but which
`pw-dump` JSON property it actually lands under (`media.name`, `node.description`, or
something else) needs empirical checking against a real running VM, which this
research pass can't spin up. Two ways to close it, either is fine as a follow-on:
(a) once verified, extend `classify_origin` to also match a `vm-<name>`-prefixed value
on whatever prop it lands in, or (b) skip depending on libvirt's exact prop mapping
entirely and have a small mackesd-side step apply `mde.vm.name` directly via
`pw-metadata` (or an addition to the mixer's existing `PwRunner` seam) once the VM is
observed running — more decoupled from libvirt-version behavior, marginally more code.

**Effort/risk:** small. Pure XML/argv string-building (same shape as every other
builder in these two files), zero new dependencies, plugs into an already-tested
consumer. The real remaining risk is guest-side: virtio-sound needs an adequately
modern guest kernel/driver, so the actual golden images in use need a quick check
(not assumed) before this is "done."

### Option B — remote audio via the SPICE playback channel (direction, not first slice)

**Mechanism.** Same guest-side device, `<audio type='spice'>` instead of `pipewire` —
QEMU's SPICE server has native playback/record channel support built in server-side
(a QEMU/SPICE-project feature, nothing this codebase has to implement on that end).
This is the mechanism that matches quasar-cloud.md Q34's "SPICE is THE console
experience" lock for Nova/cloud instances, and would ride the same `mde-vdi-spice`
crate QC-13 just wired for console rendering.

**Where it lives.** `crates/desktop/mde-vdi-spice` (client-side decode) — a shell-side
crate, not a mackesd worker (this is rendering/playback, like `mde-vdi-rdp`/
`mde-vdi-vnc`).

**Why this isn't the first slice — a verified, concrete finding.** `mde-vdi-spice`
wraps the `spice-client` crate (pinned `0.2.0`, `Cargo.lock`-verified) for the wire
protocol. Checked its upstream source tree
(`github.com/arsfeld/quickemu-manager/tree/main/spice-client/src/channels`) directly:
the channel modules present are `connection.rs`, `cursor.rs`, `display.rs`,
`display_wasm.rs`, `inputs.rs`, `main.rs` — **no `playback`/`record`/`audio` channel
exists in this dependency at all.** Adding SPICE audio therefore means either
upstreaming real new protocol work into a thin, single-maintainer crate (4,213
downloads on crates.io at last check, and its docs.rs build for 0.2.0 is currently
failing — a real maintenance-health signal), or hand-rolling the playback-channel wire
decode inside `mde-vdi-spice` directly, comparable in size to what `input.rs` (622
lines) already took for SPICE input. This is genuine new client-protocol work, not a
quick follow-on to Option A.

**Recommendation:** record this as the target direction for remote audio (it's
protocol-native and doesn't need an ironrdp bump), but scope it as its own future PR
once/if prioritized, budgeted like a new protocol feature — not bundled with Option A.

### Option C — RDP RDPSND

Unchanged from the existing operator call
(`docs/NEEDS-OPERATOR.md`, "E12-9 remote audio DESCOPED", 2026-07-03): blocked on the
pinned ironrdp not exposing an RDPSND/audio virtual-channel API; WON'T-DO for the
current release. Not reopened by this doc. (Note: `docs/NEEDS-OPERATOR.md`'s existing
line for this also says *"Local CH virtio-sound stays in scope"* — that half is now
stale for the same reason the WORKLIST flagged: cloud-hypervisor is deleted, not
merely uncertain. Corrected there in this pass, pointing at Option A above.)

### Recommended first slice

**Option A only:** the `<sound>` + `<audio type='pipewire'>` addition in both
`vm_lifecycle.rs` and `compute_provision.rs`, plus the mixer prop-matching follow-on,
plus a golden-image guest-driver check. One PR, no new dependencies, closes the local
half of lock 5 for real, and gives the already-built E12-16 mixer its first live
producer. Leave remote audio (Option B) explicitly parked with the SPICE-channel
direction recorded, not the RDPSND path.

---

## Gap 2 (E12-10): VFIO passthrough, USB, live migration

Prior art exists for PCI passthrough on the *old* stack: **DATACENTER-22** ("Enhanced
Workstation profile — passthrough Primary Desktop VM", marked `[!]` HARDWARE-GATED)
plus `install-helpers/setup-workstation-passthrough.sh`, which targets **Xen's
`xen-pciback`** for XCP-ng dom0 hosts (day-2-adopted capacity per
`mesh-virt-management.md`) — an architecturally distinct mechanism from libvirt's
`vfio-pci`, zero code reuse, but its hardware-gating conclusion ("IOMMU/VT-d BIOS
gates … not provable in this environment") turns out to transfer directly to the new
stack too. See below.

### VFIO (GPU/PCI) passthrough

**Mechanism.** Real and stable (unlike the SPICE-channel gap above, this isn't a thin
dependency — it's a long-established libvirt/QEMU feature). Host IOMMU enabled
(`intel_iommu=on`/`amd_iommu=on`, with the target device in a reasonably isolated
IOMMU group), the device bound to the `vfio-pci` host driver, and a `<hostdev>`
element in the domain XML:

```xml
<!-- illustrative addition to vm_lifecycle.rs::build_domain_xml -->
<hostdev mode='subsystem' type='pci' managed='yes'>
  <source>
    <address domain='0x0000' bus='0x01' slot='0x00' function='0x0'/>
  </source>
</hostdev>
```

`managed='yes'` lets libvirt handle detach-from-host/attach-to-guest/reset
automatically. Consumer GPU passthrough typically also wants OVMF/UEFI firmware and
`<kvm><hidden state='on'/></kvm>` to dodge vendor anti-virtualization driver checks —
tunable knobs, not blockers.

**Where it lives.** `vm_lifecycle.rs` (MV-3) is the natural home — it already owns
domain-XML authorship for the Datacenter-UI-facing lifecycle path via
`build_domain_xml`/`LibvirtBackend`. A new `VmSpec.pci_passthrough: Vec<PciAddress>`
field (mirroring how `image_path`/`network` are already optional fields) plus argv/XML
builder functions in the exact style of the existing ones is the natural shape. The
same addition would need mirroring into `compute_provision.rs` for mesh-desktop-flow
VMs (the two-paths factoring note from the top of this doc applies directly here).

**Open question — is this testable in this project's actual environment?** This is
the one place this doc can't just recommend a slice and move on; it needs an honest
answer up front, and I don't have enough hardware-inventory evidence to answer
"yes."
- **Farm build-VM slots** run atop XCP-ng/Xen dom0s (4 dom0s / 9 heavy slots per the
  farm topology). A `vm_lifecycle`/libvirt test on those slots would run *nested*
  (Xen hosting a build VM, which would then run KVM/libvirt inside it) — nested VFIO
  (an L1 Xen guest exposing a real IOMMU-isolated PCI device into an L2 KVM guest) is
  exotic and not how these general-purpose build/test slots are provisioned. Not
  realistically testable there.
- **Physical test seats** (Eagle, `.138`, `.2` — bare-metal DRM boxes) are the better
  candidate in principle, but passing through a box's *only* GPU would leave that
  physical host with no display for the egui-DRM shell that's supposed to own the seat
  directly (lock 31/33) — close to a contradiction for this project's own shell model,
  unless the box has a second/dedicated GPU, or the passthrough target is a non-GPU
  PCI device instead (a USB controller, a NIC) to sidestep the conflict.
- I could not find an inventory record (memory notes or repo docs) confirming a second
  GPU or a checked IOMMU/VT-d BIOS state on any live physical or farm machine. **This
  mirrors DATACENTER-22's own conclusion on the old stack almost exactly** — same
  wall, new hypervisor.

**Recommendation.** Build the pure/injectable-tested half now — it's genuinely cheap
(pure XML/argv-string logic, zero new dependencies, same shape as everything else in
these files) and is real, honest progress. But mark the runtime demo hardware-gated
from the start, the same way DATACENTER-22 is marked, rather than repeating the
"CORES LANDED" overclaim the deleted mde-kvm-era text made. If GPU-passthrough proof
matters enough to unblock, the actual next step is a hardware decision (dedicate a
second GPU on a physical seat, or confirm IOMMU on a spare box), not more code.

### USB passthrough / redirection

Two genuinely distinct libvirt/QEMU mechanisms live under this one WORKLIST phrase —
worth separating explicitly, because they have very different first-slice cost:

**1. Static host-device passthrough (recommended first slice).**
`<hostdev mode='subsystem' type='usb'>` addressed by vendor/product id (or
hostbus/hostaddr), admin-selected ahead of time or hot-attached via
`virsh attach-device`/`detach-device`:

```xml
<hostdev mode='subsystem' type='usb'>
  <source>
    <vendor id='0x0000'/>
    <product id='0x0000'/>
  </source>
</hostdev>
```

Buildable now as pure XML, zero new client protocol work, and — unlike the GPU case —
genuinely testable on demand (any USB stick/webcam on any dev/farm box, no scarce
hardware). Natural shape: a new `LifecycleAction::AttachUsb { host, name, vendor,
product }` / `DetachUsb` variant in `vm_lifecycle.rs`, following the exact pattern
`Pause`/`Resume` already established (parse → `plan_transition`-style precondition →
`LibvirtBackend` method → argv builder → unit tests). This is the realistic path to
E12-10's "a USB device redirects into a guest" acceptance bullet.

**2. Dynamic SPICE `usbredir` (defer — same wall as Option B above).**
`<redirdev bus='usb' type='spicevmc'>`, driven live by whichever SPICE client is
attached — the polished "click a device in the client's UI to redirect it" UX the
acceptance text likely has in mind. This needs `mde-vdi-spice` to implement the
`usbredir` protocol, and the same upstream-source check done for the audio channel
applies here too: the pinned `spice-client`'s channel list has no `usbredir` channel
either. Same recommendation as Option B: record it as the target UX, don't put it in
a first slice.

### Live migration

**Constraint recap:** no shared VM-disk storage substrate (see top of doc). Note
explicitly: **Syncthing is not a substitute.** It's eventually-consistent multi-writer
file replication, not a POSIX-safe concurrent-access block/file store — pointing a
live-written qcow2 backing an active QEMU process at a Syncthing-synced path would
risk silent disk corruption, not just lag. This needs saying because it's the kind of
"don't we already have sync for this" shortcut someone will otherwise reach for.

`virsh migrate --live` needs one of:
- **(a) Real shared storage** (NFS/Ceph/etc., mounted identically on both ends) — a
  new mesh storage primitive. Its own epic if ever pursued; explicitly **not** part of
  any first slice here.
- **(b) Non-shared-storage live migration** (`virsh migrate --live
  --copy-storage-inc`, QEMU NBD-mirrors the disk live, then a short pause to cut
  over). `--copy-storage-inc` in particular fits reasonably well here since
  `vm_lifecycle.rs`'s `VmSpec.image_path` already creates VMs as qcow2 overlays over a
  common golden base, and Syncthing already distributes golden images
  (quasar-vdi-desktop.md lock 14) — so "a common base already present on both hosts"
  is close to already true. Still real new integration work with a materially larger
  failure surface than cold rsync (a Nebula hiccup mid-live-migrate is more disruptive
  than mid-cold-rsync, which just retries cleanly from scratch).

**A concrete, verified blocker independent of storage.** `vm_lifecycle.rs`'s
`build_domain_xml` hardcodes `<cpu mode='host-passthrough' check='none'/>` for every
VM today. `host-passthrough` exposes the exact source CPU's full feature set to the
guest — best local performance, but a well-known live-migration hazard: migrating to
a host with a different CPU model/microarchitecture/stepping can fail outright or
crash the guest after migration. This project's farm is explicitly heterogeneous
(BigBoy vs the 4-vCPU nodes vs distinct physical dom0s). **VMs as currently created
are not safely live-migratable to a different physical host, independent of any
storage work.** Enabling it needs `<cpu mode='host-model'>` (or a manually pinned
baseline) for new VMs — a real, visible perf/portability trade-off, not a free syntax
change — and already-running host-passthrough VMs would need a reboot/redefine to
pick up a migratable mode.

**Open question — a live product-scope tension, flagged rather than silently
resolved.** `quasar-cloud.md` (the newer, broader, operator-locked epic covering the
same underlying libvirt/QEMU-KVM VMs) already locked **Q38: "Migration: Not a goal —
roaming = rebuild from image/volume; no live migration,"** explicitly out-of-scoping
live migration on the grounds that VDI session roaming (E12-8, already done — a
session reconnects to a freshly-placed VM elsewhere rather than migrating the running
one) already answers the practical "a node is going away" need. E12-10's lock 48
("everything in v1") predates that call by a few days and targets a different VM
population in principle (the mesh-native/VDI-session path this doc's files implement,
not Nova-brokered cloud instances) — so it's not a flat contradiction, but it *is* a
live tension between two operator-facing docs that both cover VMs on the same
libvirt/QEMU-KVM host stack. This doc doesn't resolve it — that's an operator call,
not an engineering one — but it shapes the recommendation below: don't sink real
effort into option (b) until that call is made.

**Recommendation — sidestep the tension with a genuinely useful, low-risk first
slice.** Extend the *existing*, already-shipped VIRT-8 cold-migration flow
(`compute_migrate.rs` — untouched by this doc) with a **pre-copy pass**: `rsync` the
disk once (or repeatedly, using rsync's own delta transfer) *while the VM is still
running*, and only then run the existing ACPI-shutdown → final short delta-rsync →
undefine/define/start sequence. This shrinks the downtime window from "however long a
full-disk copy takes" to "however long a final delta-copy takes" — a real, measurable
improvement, using the same topic shape, same test patterns, same failure-recovery
story already proven in `compute_migrate.rs`, with **none** of the CPU-mode or
shared-storage landmines above. It's also framed as an incremental improvement to an
already-shipped, already-scoped feature rather than new "live migration" scope, which
sidesteps the QUASAR-CLOUD Q38 tension rather than re-litigating it. True `virsh
migrate --live` (option b) should stay a separate, explicitly operator-gated
follow-on — not bundled into a first PR — pending the CPU-mode decision and the
Q38 scope question above.

---

## Rollup: what changes about how big/risky these really are

- **Audio's local half is small and mostly done already** — the hard part (the
  mixer) shipped as E12-16; only a domain-XML producer is missing. This is the
  cheapest, highest-confidence win of everything in this doc.
- **A single thin dependency (`spice-client` 0.2.0) is the recurring wall** for both
  SPICE remote-audio and SPICE dynamic USB redirect — verified by reading its actual
  upstream channel-module list, not assumed. Its docs.rs build is currently broken and
  it has one maintainer; that's a real risk factor for any future SPICE-channel work,
  not just these two features.
- **VFIO GPU passthrough may have no reachable live-proof path in this project's
  current hardware fleet.** This mirrors a finding the project already made once on
  the old Xen stack (DATACENTER-22) — worth taking seriously rather than re-learning
  the hard way. The code is cheap to write and unit-test; the runtime demo is not
  currently something this doc can promise a path to.
- **Live migration is the one item that got measurably bigger and more contested**
  during this re-scope, not smaller: a verified, concrete CPU-mode blocker
  (`host-passthrough`) exists in the current code independent of the already-known
  storage gap, *and* a parallel operator-locked epic already deprioritized the feature
  for the overlapping cloud-instance case. The honest first slice is a scoped
  improvement to the already-working cold-migration path, not live migration itself.

## Out of scope (this doc)

- Implementing any of the above in the workspace crates (design/options only).
- A new shared VM-disk storage substrate (needed for live-migration option (a), if
  ever pursued — its own epic).
- Nova PCI/vGPU-flavor passthrough for cloud instances (quasar-cloud.md Q37 territory,
  a different mechanism for a different VM population than the one this doc scopes).
- Resolving the QUASAR-CLOUD Q38 vs E12-10-lock-48 tension — flagged for an operator
  call, not decided here.
- Reconciling `compute_provision.rs` vs `vm_lifecycle.rs` into one VM-creation path —
  flagged as a recurring factoring cost, not solved here.
