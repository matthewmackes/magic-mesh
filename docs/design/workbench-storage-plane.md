# Workbench Storage Plane — GParted for the Mesh (E12-20..E12-23)

> **Status: LOCKED 2026-07-01** — 10-question operator survey (this session, per
> `/plan`). GParted-class disk/partition management as a **sixth Workbench
> plane**, with **virtual disks (KVM images + Podman storage) as first-class
> citizens** beside physical block devices, staged through an authentic
> pending-operations queue and executed by a privileged `mackesd` worker —
> locally and, with full parity, on any mesh peer.

## The locks

| # | Fork | Lock |
|---|------|------|
| 1 | Ownership | The **`mackesd` `storage` worker executes everything** — the op queue is worker state (async, progress on the Bus, survives a shell restart mid-format). The Workbench renders + submits typed verbs, even locally (§9 renderers-not-authorities). |
| 2 | Backend | **UDisks2 over D-Bus** via zbus (§2 FDO-interop exception, alongside BlueZ/UPower/logind): enumeration + change signals, partition ops, format, mount, LUKS. |
| 3 | Placement | A **sixth Workbench plane: Storage** (This Node · Controller · Network · Fleet · Provisioning · **Storage**). This node's disks in GParted layout, plus the fleet-wide storage rollup with a peer picker. |
| 4 | UX | **GParted-authentic**: per-disk horizontal segment bar (colored, used/free shading) + partition table + the **pending-operations queue** — edits stage, nothing touches a disk until **Apply**, per-op progress streams during apply, staged ops reorder/undo. Rendered in Quasar `Style`. |
| 5 | Op scope | **GParted parity**: GPT/MBR table create, partition create/delete, format, label, flags, mount/unmount, resize **grow + shrink**, move — **plus virtual-disk operations as first-class queue citizens** (lock 10). |
| 6 | Filesystems | **ext4 · xfs · vfat · exfat · btrfs (incl. subvolume list/create/delete/snapshot) · LUKS (create/unlock/lock; format-inside-LUKS as one staged op) · ntfs · swap**. |
| 7 | Protection | **Hard typed refusals** (not confirms) for: the node's **root/boot/EFI** chain (the bootc host disk), the **/mnt/mesh-storage backing device**, and devices/images **backing running VMs or containers**. Shown as locked rows with the reason + a deep-link to the freeing action (VM shutdown in Instances, etc.). |
| 8 | Apply gate | **Typed arming on every Apply**, destructive or not: the operator types the target device name (remote: peer + device) to arm. |
| 9 | Mesh reach | **Full remote parity**: stage a queue against any peer's disks; the verbs run on THAT node's storage worker, and **the hard walls live in the executor**, not the UI. |
| 10 | Virtual depth | **Full lifecycle.** KVM images (`~/Local`, raw/qcow2): create, resize, **snapshot/revert/delete-snapshot**, convert raw⇄qcow2, clone-from-golden, attach/detach to a `VmSpec` (reusing `mde-kvm` types — §6 glue, no parallel model). Podman: volume create/inspect/**prune**, image-store + per-container usage views. |

## Architecture

```
mde-shell-egui (Workbench)                 mackesd
┌───────────────────────────────┐          ┌─────────────────────────────────┐
│ Storage plane                 │          │ storage worker                  │
│  ├ disk picker (local+peers)  │  verbs   │  · UDisks2 zbus client          │
│  ├ segment bar + table        │─────────▶│  · op queue executor (staged →  │
│  ├ pending-ops queue          │   Bus    │    Apply → per-op progress)     │
│  ├ Apply (typed arming)       │◀─────────│  · hard-wall interlocks         │
│  └ virtual disks: KVM images  │  state/  │  · KVM image ops (qemu-img,     │
│    + podman volumes/usage     │  progress│    mde-kvm types) + podman API  │
└───────────────────────────────┘          │  · state/storage/<node> mirror  │
                                           │  · action/storage/<node> verbs  │
                                           └─────────────────────────────────┘
```

- **The queue is data**: staged ops are a typed `Vec<StorageOp>` (serde) — the UI
  builds it, the worker validates each op against the live topology + walls at
  BOTH stage-time (advisory) and apply-time (authoritative), then executes with
  progress events. An op that fails mid-queue halts the queue and reports typed
  state (never a silent partial).
- **Resize choreography lives in the worker**: shrink = fs-check → fs-shrink →
  partition-shrink (ordered, each a progress step); grow is the reverse. Move is
  UDisks2/parted-mediated with an explicit "this rewrites data, slow" flag.
- **Virtual ops reuse the owners**: KVM image ops call `qemu-img`-class tooling
  through a typed runner and reuse `mde-kvm`'s `VmSpec`/`running_disk_path`
  (attach/detach = spec edit through the existing broker); Podman ops go through
  the podman socket API. The in-use walls query the CH broker (running VM) and
  podman (mounted volume) — the same sources the Instances panel uses (§6).
- **§2/§6/§9 compliance**: UDisks2 is FDO D-Bus (allowed interop); the plane is
  desktop-shell, the worker platform-services, nothing in mesh-substrate grows a
  desktop dep; remote actions are typed verbs + signed job bundles, the verb set
  is the allowlist, typed-arming echo (device name) rides the verb payload.

## Safety model (extends the host-controls interlocks)

1. **Hard walls** (lock 7) are enforced in the worker at apply-time — a UI bug
   cannot bypass them; remote queues hit the same wall on the executor node.
2. **Typed arming always** (lock 8): the Apply verb carries the operator-typed
   device string; the worker refuses on mismatch.
3. **Stage-vs-apply revalidation**: topology drift between staging and Apply
   (device unplugged, VM started, partition mounted) invalidates the affected
   ops with a typed diff — never applies against a stale picture.
4. **Honest gating** (§7): no UDisks2 → the plane renders the typed unavailable
   state; a peer without the worker version shows "storage verbs unsupported".

## The units

- **E12-20 — storage worker core.** UDisks2 zbus client (enumerate + signals),
  the `StorageOp` model + queue executor + progress events, hard-wall
  interlocks, `state/storage/<node>` mirror + `action/storage/<node>` verbs
  (typed-arming echo). Injectable UDisks/qemu-img/podman transports; headless
  tests for queue validate/execute/halt + every wall.
- **E12-21 — the Storage plane (GParted-authentic UI).** Sixth Workbench plane:
  segment bar + table + pending queue + Apply with typed arming; locked rows
  with reasons + deep-links; fleet rollup + peer picker driving remote queues
  through the same verbs. *(Serializes on E12-15's shell wiring; owns
  workbench.rs nav for this wave.)*
- **E12-22 — virtual disks first-class.** The KVM image lifecycle (create/
  resize/snapshot/revert/convert/clone/attach-detach via mde-kvm types) +
  Podman storage (volumes, prune, usage) staged in the same queue, walled by
  the same in-use checks.
- **E12-23 — filesystem depth + packaging.** The full fs set incl. btrfs
  subvolumes, LUKS flows, shrink/move choreography per fs; udisks2 + e2fsprogs/
  xfsprogs/btrfs-progs/exfatprogs/ntfs-3g/cryptsetup/qemu-img into the RPM
  requires + bootc Containerfile + ansible farm toolchain where needed.

**Serialization**: E12-20 is mackesd-only (dispatchable now, after the clippy-debt
unit lands); E12-21 waits for E12-15 (shell wiring); E12-22/23 parallelize after
E12-20 (22 = kvm/podman lane, 23 = fs depth + packaging).

## Acceptance (epic-level, runtime-observable)

1. A USB disk: staged GPT + two partitions (ext4 + LUKS-ext4) + labels applies
   with one typed-armed Apply, progress bar per op; the segment bar re-renders
   live from UDisks2 signals.
2. A staged shrink of a mounted partition is refused typed at stage time;
   unmounted, the shrink choreography (check → fs-shrink → part-shrink)
   completes and the freed space accepts a new staged partition in the same
   queue.
3. The node's root disk, the mesh-storage backer, and a running VM's image all
   render locked with reasons; the VM row's deep-link lands in Instances, and
   after shutdown the image unlocks.
4. A qcow2 image: snapshot → risky change → revert works from the plane; a raw
   golden clones to `~/Local` and attaches to a VmSpec that then boots.
5. Podman: a staged volume-prune shows exactly what dies before arming; usage
   views match `podman system df`.
6. From node A, a staged queue against node B's USB disk applies (typed arming
   `B:/dev/sdX`), B's worker enforcing every wall; B's disks live-update in A's
   fleet rollup.

## Risks / out of scope

- **Risks**: shrink/move data-loss edges (mitigated: fs-check first, staged
  choreography, typed arming); UDisks2 coverage gaps for exotic ops (fall back
  to typed tool-runners, never raw shell verbs); qcow2 snapshot semantics with
  cloud-hypervisor (raw is CH-native — qcow2 ops are image-at-rest only, walls
  keep them offline); podman socket availability on headless roles.
- **Out of scope v1**: RAID/mdadm, LVM, ZFS, multipath, iSCSI/network block
  devices, SMART health (a Fleet-plane candidate later), whole-disk secure-erase.
