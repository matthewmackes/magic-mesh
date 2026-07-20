# CONSTRUCT-CLOUD cutover map — what dies, what replaces it, what stays

> **Status: tracks the LOCKED design** (`docs/design/quasar-cloud.md`,
> 2026-07-03). This is the operator-facing version of the design's
> replaced-vs-kept map: what to expect at cutover, in migration terms. The
> design doc's table is the maintained source; this note explains the
> consequences.
>
> **Provider-neutral runway note, 2026-07-18:** this document describes the
> installed OpenStack/Kolla backend and the earlier cutover from the retired VM
> stack. It is no longer the product architecture target by itself. Current
> Construct Cloud work must move shell surfaces, Bus contracts, persisted
> mirrors, and operator copy toward provider-neutral cloud contracts where this
> OpenStack path is one compatibility adapter.

## Cutover semantics (read first)

- **Hard cutover, per node** (Q72): a node runs the old VM stack **or** Nova —
  never both. There is no coexistence window on a node.
- **Everywhere at once** (Q71): the fleet state declares the cloud and every
  node converges together. The convergence path is proven on the farm-VM dev
  cloud first (QC-16).
- **Replaced code is deleted in the same epic** (Q73, QC-15): §7 tolerates no
  dead code. After cutover, `/audit` must find no orphaned modules from the
  replaced stack.
- **Existing VMs are not imported** (Q58): **fresh start** — rebuild every VM
  from a Glance image and/or Cinder volume. Plan for this: anything living
  only inside a pre-cutover VM's disk must be moved to survivable storage
  before the flip.

## What dies at cutover → what replaces it

| Dies | Replaced by | Where |
|---|---|---|
| mackesd `vm_lifecycle` worker + the mesh-native VM scheduler paths | **Nova + Placement** own ALL VM lifecycle; the mackesd verbs become wrappers over the Nova API | Q31/40, QC-15 |
| `cloud-hypervisor` (hypervisor + its glue, incl. the `mde-kvm` broker crate) | **libvirt/QEMU-KVM**; image/host virt foundation proven in QC-1, QEMU virtio-gpu→egui fast path split to QC-23 | Q32, QC-1/QC-15/QC-23 |
| The shell's local-VM **Instances** surface (`mde-shell-egui`, the cloud-hypervisor broker view) | the **Cloud plane** — one VM surface for local and mesh-wide alike | QC-12/QC-15 |
| `install-helpers/build-mde-vm-golden.sh` + on-disk golden qcow2s | **Glance + diskimage-builder** pipeline; images replicate between API nodes | Q36/53, QC-9 |
| Cockpit VM console (the interim) | the **Cloud plane** (+ optional mesh-only Horizon) | Q66, QC-15 |
| Ad-hoc VM consoles | **SPICE via the mde-vdi-spice viewer** — THE console experience | Q34, QC-13 |
| Per-instance mesh certs (the VDI dual-homing precedent) | **none** — instances connect via Neutron/OVN and are "inside" without certs; the precedent is narrowed to VDI, and QC-14 re-bases VDI guests onto Nova too | Q44, QC-7/QC-14 |
| Human identity in the mesh CA/KDC | **Keystone** absorbs human users; the CA/KDC narrows to machine certs | Q62, QC-5 |
| Peer-directory-only DNS/naming | **Designate** serves mesh zones, **fed by the peer directory** — which stays the source that can re-seed it from scratch | Q46, QC-17 (wave 2) |
| DO-Spaces-only object store | **Swift hot tier + DO Spaces off-site** (two tiers); cinder-backups land there | Q54/55/57, QC-18 |
| Hand-placed media stack (Navidrome) | **Nova instances** — the media re-platform is the first platform proof on the cloud | Q60, QC-18 |
| Raw disks on the writable partition (block storage) | **Cinder LVM** volumes (node-local; fits the no-live-migration lock) | Q51, QC-8 |

Superseded design documents (kept as history, no longer normative):
`docs/design/mesh-virt-management.md` pillar 1 (cloud-hypervisor) and its
mesh-native-scheduler lock · the Cockpit-interim console · the §9
"Controller plane" framing (it becomes the Cloud plane) · the per-instance
mesh-cert extension of the VDI precedent.

## What stays (explicitly KEPT)

| Kept | Note |
|---|---|
| **Nebula** | the substrate — identity, WAN, NAT punch; OVN rides on top; Nebula IS the API transport security (Q41/23) |
| **etcd** | + tooz for OpenStack coordination (Q17) |
| **Syncthing** | SUBSTRATE-V2 stands; also carries the Kolla image archives (Q52/18) |
| **Podman + mackesd** | the container stack — no Zun/Magnum; OpenStack replaces VM methods only (Q35) |
| **mde-bus** | THE platform bus; RabbitMQ is OpenStack-internal RPC only (Q67) |
| **netdata + mesh-health** | extended with cloud checks (Q64) |
| **Mesh chat notifications** | extended with cloud service contacts (Q65, QC-20) |
| **ISO + role-chooser enrollment** | no Ironic (Q68) |
| **Typed mackesd verbs (§9)** | still the contract; OpenStack is the backend (Q40, QC-11) |
| **Lighthouse Caddy** | keeps *platform* ingress; Octavia serves *instance* workloads (Q47) |
| **The build farm** | untouched — it builds the platform; the cloud deploys between node machines (Q9) |

## Verifying a cutover node

Design acceptance #6 — a cutover node is clean when:

- no cloud-hypervisor binary is in use;
- no Cockpit VM console is reachable;
- `build-mde-vm-golden.sh` is deleted from the tree;
- `/audit` finds no orphaned modules from the replaced stack;
- pre-existing VMs have been rebuilt fresh from image/volume (Q58);
- VDI sessions boot via Nova and render through the existing viewer path
  (QC-14).

## Related documents

- `docs/design/quasar-cloud.md` — the locked design and the maintained
  replaced-vs-kept table.
- `docs/ops/quasar-cloud-runbook.md` — standing up and operating the cloud.
- `docs/help/cloud-self-service.md` — the member how-to.
- `DISCLAIMER.md` — blast radius, extended for cloud instances.
