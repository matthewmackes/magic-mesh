# Mesh virtualization management + same-stack — design lock

**Status:** LOCKED 2026-06-30 (operator: "Go Option B", "same stack on all machines", "lock it").
**Companion:** `docs/design/onboarding-wizard.md` (the 2-role model + onboarding flow).

## The three pillars locked here

1. **Hypervisor = Fedora + KVM, not XCP-ng (Option B).** The host hypervisor is
   **cloud-hypervisor/KVM on Fedora** (the same `mde-kvm` stack the Workstation
   already uses), so one OS family runs every role and the VDI virtio-gpu→egui
   fast-path is reused. **XCP-ng is demoted from a role to a day-2 "adopt external
   hypervisor capacity" action** — magic-mesh can still enroll + drive an existing
   XCP-ng host (as the live build farm does via `xe`/tofu), but our installer never
   produces one. *(XCP-ng is its own XenServer-lineage OS; our Fedora ISO can't make
   one — that conflation was the original `XCP-NG`-role flaw.)*

2. **Two roles only — Lighthouse and Workstation.** "Headless" is **not** a role: a
   headless machine is a **Workstation without a local display** (it runs the daemon
   stack — `mackesd` + cloud-hypervisor + Podman — and serves VMs/containers to the
   mesh; managed from a peer's Workbench; the egui-DRM shell simply doesn't start
   with no seat). See `onboarding-wizard.md`.

3. **Same stack on every machine.** One identical image; **role is configuration,
   not a different build.** Path: **Option 1 now → Option 2 over time.**
   - **Option 1 (now): uniform image, role = a config flag.** Every box ships the
     complete stack; `role=lighthouse|workstation` masks/starts systemd units. A
     Lighthouse runs the same bits as a Workstation with the desktop units off.
     Payoff: one build, one update, and **role becomes re-configurable** — flip a
     flag (+ attach a monitor) to re-role a box, *no reinstall* (this supersedes the
     onboarding survey's "reinstall to change role", lock #29).
   - **Option 2 (target): features-as-workloads.** As the management layer matures,
     role-specific features (media/Navidrome, back-office services, VMs) become
     **Podman/VM workloads the management layer schedules**, shrinking the base. The
     **egui-DRM shell is the one host binary** (it owns the seat — the Quasar
     premise — so it can't be a workload); it lights up only where a display exists.
     Everything else a role does is a managed workload.

## The management layer — no-center KVM + Podman

**Requirement:** manage **KVM VMs + Podman containers** across the mesh, **no single
center** (§0), open-source.

### Options surveyed
| # | Option | KVM+Podman | No center | OSS | Verdict |
|---|---|---|---|---|---|
| 1 | **Incus** (LXD fork) | VMs + LXC/OCI (not Podman per se) | ✅ floating Raft leader | ✅ Apache-2.0 | strong adopt-alternative |
| 2 | **Nomad** | ✅ `qemu`+`podman` drivers (best literal fit) | ⚠️ soft (Raft quorum) | ❌ **BUSL**, no fork | excluded on license + lost VDI fast-path |
| 3 | KubeVirt on K3s | VMs + CRI (not Podman) | ❌ k8s control plane | ✅ | too centralized/heavy |
| 4 | Proxmox VE | KVM + LXC | ✅ multi-master | ✅ AGPL | wrong OS family (Debian) |
| 5 | **Mesh-native** | ✅ libvirt/cloud-hypervisor + Podman direct | ✅ the mesh *is* the plane | ✅ (own code) | **CHOSEN** |

### Locked direction: #5 (mesh-native), evolving with Option 2
**Don't adopt a heavyweight orchestrator.** Each node runs a thin `mackesd` worker
driving the **local cloud-hypervisor/libvirt (KVM) + Podman sockets**; desired-state
+ placement live in the **etcd/Syncthing state already running over Nebula**; **any
node can schedule** — there is no orchestrator to centralize, so "no center" is
structural, not configured. **Cockpit** (`cockpit-machines` + `cockpit-podman`) is
the zero-build per-node console while the thin scheduler is built.
- **Adopt-fallback: Incus (#1)** if building the scheduler proves too costly — it's
  genuinely no-master and one clean tool, at the cost of using its OCI runtime
  instead of Podman.
- **Why not Nomad:** perfect literal fit, but **BUSL (source-available, no community
  fork)** undercuts "open source", it's a soft center, and its `qemu` driver exposes
  VMs over VNC/SPICE — it would *lose* the virtio-gpu→egui fast path. Viable only as
  a Server-side container/VM scheduler if the license is ever acceptable.

## Architecture
- **`mackesd` is the universal core** on every node: mesh/relay/CA(+media on a
  Lighthouse) + the **vm-lifecycle + container (Podman) workers** that ARE the
  management layer. It reads/writes desired-state in the shared etcd/Syncthing.
- **Executors are battle-tested:** cloud-hypervisor/libvirt for KVM, Podman for
  containers — `mackesd` orchestrates, it doesn't reimplement them (§6).
- **The egui shell** is the one display-conditional host binary; the Workbench is the
  human front-end that submits desired-state (start this VM / run this service).
- **No new consensus:** reuse the etcd you already run; don't add Nomad/k8s Raft.

## Per-node KVM service set (the recipe — `infra/ansible/node-virt.yml`)
The Fedora+KVM replacement for the XCP-ng 16-service toolstack. Only ~4 packages are
*added*; the kernel + the mesh + systemd cover the rest. Encoded as idempotent ansible.

- **Install:** `qemu-kvm` (VMM) · `libvirt` (+ qemu/network/storage drivers =
  `xapi`+`xenopsd`+`sm`+`xcp-networkd`) · `virt-install`/`libguestfs-tools` ·
  **`podman`** (new) · `cloud-hypervisor` (VDI fast-path) · *opt*
  `cockpit`+`cockpit-machines`+`cockpit-podman`.
- **Enable:** `libvirtd.service` · `podman.socket` · (`cockpit.socket`) + the default
  libvirt network (autostart) + a `default` dir storage pool + the mesh user in the
  `libvirt` group.
- **Add nothing for:** `xen`/`xenstored`/`xenconsoled` (in-kernel KVM) · `squeezed`
  (virtio-balloon) · `stunnel` (Nebula) · `xha` (the mesh) · `v6d` (no licensing) ·
  `message-switch`/`forkexecd` (systemd/D-Bus/`mde-bus`) · `xcp-rrdd`/`perfmon` (a
  `mackesd` metrics worker, MV-2).

This list IS the `KVM_SERVICES` catalog (worklist MV-1); the `kvm-host-health` worker
(MV-2) surfaces it to `event/kvm/services`; the Datacenter panels (MV-6) drive it.

## Acceptance (runtime-observable, §7)
- One image installs on a box; `role=lighthouse` brings up relay/media/CA with no
  desktop; the *same* image with `role=workstation` (+ a display) brings up the egui
  shell — **no separate build** (Option 1).
- From any Workstation's Workbench, "run a Podman service" and "boot a VM" both land
  on a chosen mesh node and appear in the mesh map; **no central scheduler** is
  required (any node accepts the desired-state).
- A Lighthouse and a Workstation run **byte-identical** `mackesd`; only their active
  units/workloads differ.
- An existing XCP-ng host can be **adopted** (day-2) and contribute VM capacity
  without being a role.

## Risks / open
- Building the placement/health/migration scheduler is real work (mitigated: Cockpit
  covers per-node ops day-1; Incus is the escape hatch).
- Option-1 uniform image carries inert GUI/VDI weight on cloud Lighthouses (a few
  hundred MB + attack surface — units off, not active); Option-2 migration is what
  retires that weight.
- Live migration / shared storage are not free on KVM the way XCP-ng gives them; the
  **mesh** provides node-loss resilience (sessions roam) instead of hypervisor HA.

## Out of scope
- Adopting Nomad/k8s/Proxmox (surveyed, rejected above).
- Installing XCP-ng from our media (adopt-only).
