//! The **Storage** surface — `GParted`-authentic disk/partition management
//! (E12-21), fronted as **"Local Cylinders"** (MENU-4: the operator's name for
//! the platform's `GParted`, carried by the shared menu bar's `GParted` spine).
//!
//! The desktop half of the Workbench Storage plane
//! (`docs/design/workbench-storage-plane.md`). Where the `mackesd` `storage`
//! worker (E12-20) **owns and executes** the op queue over a live `UDisks2`
//! topology, this surface **renders** each disk as a `GParted`-style segment bar +
//! partition table and **submits** a typed [`StorageOp`] queue back onto the Bus —
//! for this node and, with full parity, any mesh peer (§9 renderers-not-authorities).
//!
//! ## Placement — a dock [`Surface`](crate::dock::Surface), not a sixth Workbench plane
//!
//! Design lock 3 names a sixth Workbench plane; the design doc explicitly permits a
//! dock Surface "if the Workbench plane wiring is heavier". It is: a `GParted` view
//! wants the full shell body (segment bars, the pending-op queue, the peer picker,
//! the per-op progress lane), which is cramped in the Workbench rail's content
//! pane; and a Workbench plane would force a new `&mut` parameter through
//! `workbench::show()`'s already-8-arg signature — an invasive change colliding
//! with the other planes landing this wave. A dock Surface is purely additive
//! (distinct lines in `dock.rs`/`main.rs`), exactly like System / Clipboard, and
//! gets the room a disk manager needs. So this lands as `Surface::Storage`.
//!
//! ## The Bus contract (mirrors the E12-20 worker)
//!
//! Read: `state/storage/<node>` — a [`StorageMirror`] per peer (backend
//! availability + the live [`Topology`]). Every peer's mirror replicates to this
//! node's spool, so the fleet rollup enumerates them via `list_topics()`.
//! Write: `action/storage/<node>` — a [`StorageRequest`] (`Apply` carrying the
//! operator-typed **arming** device echo + the staged topology, or `Refresh`).
//! Progress: `event/storage/<node>/progress` — a [`StorageProgress`] per op.
//!
//! The payloads are a JSON boundary: **local** serde mirrors of the worker's
//! `storage.rs` types (the shell stays desktop-tier, leaning inward only on
//! `mde-bus`, §6). Every field is real, live worker reality — never a stand-in
//! (§7): no `UDisks2` renders the honest unavailable state; an empty spool renders
//! the honest "no peer has published storage" state.
//!
//! ## Safety mirrors the worker, never replaces it
//!
//! The hard walls (lock 7) live in the executor. This surface renders **advisory**
//! locked rows for the disks it can *see* are protected from the published
//! topology — a disk with a partition mounted at `/`, `/boot`, `/boot/efi`, `/efi`
//! or `/mnt/mesh-storage` — and disables staging against them. The VM/container
//! in-use wall isn't visible in the topology, so it's enforced at apply-time by the
//! worker and surfaces as a `Refused` progress row with a deep-link hint to free it
//! in the Cloud plane. **Typed arming** (lock 8) is always demanded before an
//! Apply: the operator types the exact target device, matched against the queue's
//! single resolved disk (the worker re-checks authoritatively).
//!
//! Live execution is E12-23 (the worker's `UDisks2Executor` is `IntegrationGated`
//! today) — this surface lands the render + the typed verb emission; an Apply
//! reaches the worker, which stages/validates/walls and reports the gated state.
//!
//! `project` / `Compose::build` / `queue_target` are pure (no Bus, no GPU) and
//! unit-tested directly; the only IO is `poll` (a `Persist` read) and `publish` (a
//! `Persist` write — the same persist-first path `mde-bus publish` takes).

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use mde_egui::egui::{self, Color32, RichText, Sense};
use mde_egui::Style;
use serde::{Deserialize, Serialize};

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;

use crate::bus_reader::BusReader;

use crate::toast_bridge::TOAST_TOPIC;

/// Topic prefix for the per-node topology mirror (`state/storage/<node>`).
const STATE_PREFIX: &str = "state/storage/";

/// Poll cadence — a `UDisks2` change signal on any peer surfaces within this window.
/// Matches the other Bus surfaces; the read is a cheap local `SQLite` scan.
const REFRESH: Duration = Duration::from_secs(5);

/// A filled-circle status glyph — the shared dot the other planes render.
const DOT: &str = "\u{25CF}";

/// The mountpoints that mark a whole disk as the node's protected root/boot/EFI
/// chain (mirrors the worker's `protected_from_mountinfo`, advisory here).
const ROOT_BOOT_MOUNTS: [&str; 4] = ["/", "/boot", "/boot/efi", "/efi"];
/// The mesh shared-storage mountpoint — its backing disk is protected.
const MESH_STORAGE_MOUNT: &str = "/mnt/mesh-storage";

// ───────────────────────── JSON boundary (read side) ─────────────────────────
// Local mirrors of the `mackesd::workers::storage` payloads. serde ignores wire
// fields we don't render; both traits are derived because the staged `Topology`
// is echoed back on the Apply verb (write side).

/// A partition-table scheme — mirrors `storage::PartitionTable`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PartTable {
    /// GUID Partition Table (the modern default a New-table op stages).
    #[default]
    Gpt,
    /// Master Boot Record.
    Mbr,
}

impl PartTable {
    /// The short display label.
    const fn label(self) -> &'static str {
        match self {
            Self::Gpt => "GPT",
            Self::Mbr => "MBR",
        }
    }
}

/// One partition on a disk — mirrors `storage::Partition`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Partition {
    /// The partition device (`/dev/sdb1`).
    name: String,
    /// 1-based partition number.
    number: u32,
    /// Start offset (MiB from the disk head).
    start_mib: u64,
    /// Size (MiB).
    size_mib: u64,
    /// The filesystem id `UDisks` reports (`ext4`, `crypto_LUKS`, …), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    filesystem: Option<String>,
    /// The filesystem label, when any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    /// The current mount point, when mounted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mountpoint: Option<String>,
    /// The filesystem UUID, when any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    uuid: Option<String>,
}

/// One whole-disk block device with its layout — mirrors `storage::BlockDevice`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct BlockDevice {
    /// The whole-disk device (`/dev/sdb`).
    name: String,
    /// Total size (MiB).
    size_mib: u64,
    /// The partition-table scheme, when the disk has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    table: Option<PartTable>,
    /// Whether the drive is removable (USB stick, SD card).
    #[serde(default)]
    removable: bool,
    /// The partitions, in on-disk order.
    #[serde(default)]
    partitions: Vec<Partition>,
}

impl BlockDevice {
    /// Free (unpartitioned) space (MiB) — total minus the sum of partition sizes.
    /// A coarse, MiB-granular advisory figure (ignores alignment gaps + metadata),
    /// exactly the worker's `free_mib`.
    fn free_mib(&self) -> u64 {
        let used: u64 = self.partitions.iter().map(|p| p.size_mib).sum();
        self.size_mib.saturating_sub(used)
    }

    /// The named partition's live row on this disk, or a typed reason — the
    /// resize/move anchor (current size / start), mirroring the worker's own
    /// resolution.
    fn partition_named(&self, partition: &str) -> Result<&Partition, String> {
        self.partitions
            .iter()
            .find(|p| p.name == partition)
            .ok_or_else(|| format!("{partition} is not on {}.", self.name))
    }

    /// The advisory protection reason for this disk, derived from what the topology
    /// makes visible (a mounted root/boot/EFI or mesh-storage partition). `None`
    /// when nothing visible protects it — the worker still enforces the in-use wall
    /// authoritatively at apply-time.
    fn protected_reason(&self) -> Option<&'static str> {
        let mut mesh = false;
        for p in &self.partitions {
            if let Some(mp) = p.mountpoint.as_deref() {
                if ROOT_BOOT_MOUNTS.contains(&mp) {
                    // Root/boot/EFI wins over a mesh-storage classification.
                    return Some("backs the node's root / boot / EFI chain");
                }
                if mp == MESH_STORAGE_MOUNT {
                    mesh = true;
                }
            }
        }
        mesh.then_some("backs /mnt/mesh-storage (the mesh shared volume)")
    }
}

/// The live block-device topology — mirrors `storage::Topology`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Topology {
    /// The whole-disk devices.
    #[serde(default)]
    devices: Vec<BlockDevice>,
}

impl Topology {
    /// The disk that owns partition `name`, when present.
    fn parent_disk_of(&self, partition: &str) -> Option<&BlockDevice> {
        self.devices
            .iter()
            .find(|d| d.partitions.iter().any(|p| p.name == partition))
    }
}

/// Backend availability — mirrors `storage::BackendStatus` (§7 honest gating).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum BackendStatus {
    /// `UDisks2` enumerated the topology.
    Available,
    /// `UDisks2` isn't reachable — carries the reason to render.
    Unavailable {
        /// Why the backend is unavailable.
        reason: String,
    },
}

/// The `state/storage/<node>` mirror body — mirrors `storage::StorageState`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct StorageMirror {
    /// The publishing node id.
    host: String,
    /// The backend availability (§7 honest gating).
    backend: BackendStatus,
    /// The live topology (empty when the backend is unavailable).
    #[serde(default)]
    topology: Topology,
    /// Publish time (ms since the Unix epoch) — the latest-wins fold key.
    published_at_ms: u64,
}

/// The terminal state of one op in the progress stream — mirrors
/// `storage::ProgressState`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum ProgressState {
    /// Applied to disk.
    Applied,
    /// Refused by a hard wall.
    Refused {
        /// The refusal text.
        reason: String,
    },
    /// Invalidated (drift / stale / precondition).
    Invalidated {
        /// The invalidation text.
        reason: String,
    },
    /// Backend execution failed.
    Failed {
        /// The failure text.
        reason: String,
    },
}

impl ProgressState {
    /// The row tone + short word for this terminal state.
    const fn tone_word(&self) -> (Color32, &'static str) {
        match self {
            Self::Applied => (Style::OK, "applied"),
            Self::Refused { .. } => (Style::DANGER, "refused"),
            Self::Invalidated { .. } => (Style::WARN, "invalidated"),
            Self::Failed { .. } => (Style::DANGER, "failed"),
        }
    }

    /// The detail text a non-`Applied` state carries (for the row's second line).
    fn detail(&self) -> Option<&str> {
        match self {
            Self::Applied => None,
            Self::Refused { reason } | Self::Invalidated { reason } | Self::Failed { reason } => {
                Some(reason)
            }
        }
    }
}

/// A per-op apply-progress event — mirrors `storage::StorageProgress`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct StorageProgress {
    /// The publishing node id.
    host: String,
    /// The armed device this apply targets.
    device: String,
    /// 0-based op index within the queue.
    op_index: usize,
    /// The total op count in the queue.
    total: usize,
    /// The op kind (its verb).
    op_kind: String,
    /// The op's terminal state.
    state: ProgressState,
    /// Publish time (ms since the Unix epoch) — the latest-wins fold key.
    published_at_ms: u64,
}

// ───────────────────────── JSON boundary (write side) ─────────────────────────

/// A filesystem/format target — mirrors `storage::Filesystem` (the subset this
/// surface stages; the worker validates all kinds).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Filesystem {
    /// ext4.
    Ext4,
    /// XFS.
    Xfs,
    /// FAT (vfat).
    Vfat,
    /// exFAT.
    Exfat,
    /// btrfs.
    Btrfs,
    /// NTFS.
    Ntfs,
    /// Linux swap.
    Swap,
    /// A LUKS container.
    Luks,
}

impl Filesystem {
    /// Every filesystem, in picker order.
    const ALL: [Self; 8] = [
        Self::Ext4,
        Self::Xfs,
        Self::Vfat,
        Self::Exfat,
        Self::Btrfs,
        Self::Ntfs,
        Self::Swap,
        Self::Luks,
    ];

    /// The display / mkfs id.
    const fn label(self) -> &'static str {
        match self {
            Self::Ext4 => "ext4",
            Self::Xfs => "xfs",
            Self::Vfat => "vfat",
            Self::Exfat => "exfat",
            Self::Btrfs => "btrfs",
            Self::Ntfs => "ntfs",
            Self::Swap => "swap",
            Self::Luks => "luks",
        }
    }
}

/// One typed storage operation — the queue element (mirrors `storage::StorageOp`,
/// internally tagged on `op` so the worker's `parse_request` accepts it verbatim).
/// This surface stages the `GParted`-core set plus resize (grow/shrink) + move
/// (MENU-4 — the composer resolves the direction off the live partition size, the
/// worker re-validates authoritatively); the remaining worker verbs (flags, LUKS,
/// btrfs subvolumes) stay worker-side until their compose legs land (E12-22/23).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum StorageOp {
    /// Write a fresh partition table to a whole disk (destroys the layout).
    CreateTable {
        /// The whole-disk device.
        device: String,
        /// The table scheme.
        table: PartTable,
    },
    /// Create a partition in free space, optionally formatted + labelled.
    CreatePartition {
        /// The whole-disk device to carve from.
        device: String,
        /// Start offset (MiB from the disk head).
        start_mib: u64,
        /// Partition size (MiB).
        size_mib: u64,
        /// Format the new partition (`None` ⇒ leave unformatted).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filesystem: Option<Filesystem>,
        /// Optional filesystem label.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
    /// Delete an existing partition.
    DeletePartition {
        /// The partition device.
        partition: String,
    },
    /// (Re)format an existing partition.
    Format {
        /// The partition device.
        partition: String,
        /// The filesystem to write.
        filesystem: Filesystem,
        /// Optional filesystem label.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
    /// Set a partition's filesystem label.
    SetLabel {
        /// The partition device.
        partition: String,
        /// The new label.
        label: String,
    },
    /// Mount a partition at `mountpoint`.
    Mount {
        /// The partition device.
        partition: String,
        /// The mount point.
        mountpoint: String,
    },
    /// Unmount a partition.
    Unmount {
        /// The partition device.
        partition: String,
    },
    /// Grow a partition (+ its filesystem) to `new_size_mib` — mirrors the
    /// worker's `Grow` (lock 5 `GParted` parity: resize is a first-class queue op).
    Grow {
        /// The partition device.
        partition: String,
        /// The target size (MiB), larger than current.
        new_size_mib: u64,
    },
    /// Shrink a partition (+ its filesystem) to `new_size_mib` — mirrors `Shrink`.
    Shrink {
        /// The partition device.
        partition: String,
        /// The target size (MiB), smaller than current.
        new_size_mib: u64,
    },
    /// Move a partition to a new start offset (rewrites data — slow) — mirrors
    /// the worker's `Move`.
    Move {
        /// The partition device.
        partition: String,
        /// The new start offset (MiB from the disk head).
        new_start_mib: u64,
    },
}

impl StorageOp {
    /// The whole-disk `device` field for the device-scoped ops.
    fn device_field(&self) -> Option<&str> {
        match self {
            Self::CreateTable { device, .. } | Self::CreatePartition { device, .. } => {
                Some(device.as_str())
            }
            _ => None,
        }
    }

    /// The `partition` device for the partition-scoped ops.
    fn partition(&self) -> Option<&str> {
        match self {
            Self::DeletePartition { partition }
            | Self::Format { partition, .. }
            | Self::SetLabel { partition, .. }
            | Self::Mount { partition, .. }
            | Self::Unmount { partition }
            | Self::Grow { partition, .. }
            | Self::Shrink { partition, .. }
            | Self::Move { partition, .. } => Some(partition.as_str()),
            Self::CreateTable { .. } | Self::CreatePartition { .. } => None,
        }
    }

    /// The whole-disk device this op ultimately touches (the arming key): the
    /// `device` for a device-scoped op, else the partition's parent disk resolved
    /// from `topo`. `None` when a partition-scoped op names an unknown partition.
    fn resolve_device(&self, topo: &Topology) -> Option<String> {
        if let Some(d) = self.device_field() {
            return Some(d.to_string());
        }
        self.partition()
            .and_then(|p| topo.parent_disk_of(p))
            .map(|d| d.name.clone())
    }

    /// A short, human one-line summary for the queue row.
    fn summary(&self) -> String {
        match self {
            Self::CreateTable { device, table } => {
                format!("New {} table on {device}", table.label())
            }
            Self::CreatePartition {
                device,
                size_mib,
                filesystem,
                ..
            } => {
                let fs = filesystem.map_or("unformatted", Filesystem::label);
                format!("New {size_mib} MiB {fs} partition on {device}")
            }
            Self::DeletePartition { partition } => format!("Delete {partition}"),
            Self::Format {
                partition,
                filesystem,
                ..
            } => format!("Format {partition} as {}", filesystem.label()),
            Self::SetLabel { partition, label } => {
                format!("Label {partition} \u{201C}{label}\u{201D}")
            }
            Self::Mount {
                partition,
                mountpoint,
            } => format!("Mount {partition} at {mountpoint}"),
            Self::Unmount { partition } => format!("Unmount {partition}"),
            Self::Grow {
                partition,
                new_size_mib,
            } => format!("Grow {partition} to {new_size_mib} MiB"),
            Self::Shrink {
                partition,
                new_size_mib,
            } => format!("Shrink {partition} to {new_size_mib} MiB"),
            Self::Move {
                partition,
                new_start_mib,
            } => format!("Move {partition} to start {new_start_mib} MiB"),
        }
    }
}

/// The staged pending-operations queue body — mirrors `storage::StorageQueue`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
struct StorageQueue {
    /// The staged ops, applied in order.
    ops: Vec<StorageOp>,
}

/// A request published to `action/storage/<node>` — mirrors `storage::StorageRequest`
/// (internally tagged on `verb`, so the worker's `parse_request` accepts it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "verb", rename_all = "snake_case")]
enum StorageRequest {
    /// Apply a staged queue to a disk — carries the operator-typed arming echo
    /// (lock 8) and the topology the queue was staged against (the drift baseline).
    Apply {
        /// The operator-typed target device (lock 8 arming echo).
        armed_device: String,
        /// The topology the queue was built against (drift baseline).
        staged: Topology,
        /// The staged op queue.
        queue: StorageQueue,
    },
    /// Re-publish the live topology mirror (the operator's refresh).
    Refresh,
}

impl StorageRequest {
    /// Serialize to the request body. A fixed, derive-backed shape → serialization
    /// can't realistically fail; an empty body (never produced) the worker rejects.
    fn to_body(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

// ──────────────────────────── projected view ────────────────────────────

/// One node's live storage reality, folded from the latest `state/storage/<node>`
/// mirror seen for that host.
#[derive(Debug, Clone, PartialEq, Eq)]
struct NodeStorage {
    /// Node id (the Bus `host`).
    host: String,
    /// The backend availability (§7).
    backend: BackendStatus,
    /// The live topology (empty when unavailable).
    topology: Topology,
    /// Publish time of the mirror held (latest-wins fold key).
    published_at_ms: u64,
}

impl NodeStorage {
    /// Whether the backend enumerated a topology.
    const fn available(&self) -> bool {
        matches!(self.backend, BackendStatus::Available)
    }
}

/// Fold raw `state/storage/*` bodies into a sorted-by-host per-node view.
/// Latest message wins per host (by `published_at_ms`), so a growing topic
/// collapses to one row per node. Pure — no Bus, no GPU.
fn project(state_bodies: &[String]) -> Vec<NodeStorage> {
    let mut nodes: BTreeMap<String, NodeStorage> = BTreeMap::new();
    for body in state_bodies {
        let Ok(s) = serde_json::from_str::<StorageMirror>(body) else {
            continue;
        };
        let entry = nodes.entry(s.host.clone()).or_insert_with(|| NodeStorage {
            host: s.host.clone(),
            backend: s.backend.clone(),
            topology: s.topology.clone(),
            published_at_ms: 0,
        });
        // Latest wins (>= so a same-ms republish still refreshes).
        if s.published_at_ms >= entry.published_at_ms {
            entry.backend = s.backend;
            entry.topology = s.topology;
            entry.published_at_ms = s.published_at_ms;
        }
    }
    nodes.into_values().collect()
}

/// Fold the progress lane for one node into the latest terminal state per op index,
/// ordered by index — the current apply's per-op picture. Pure.
fn project_progress(bodies: &[String], node: &str) -> Vec<StorageProgress> {
    let mut by_index: BTreeMap<usize, StorageProgress> = BTreeMap::new();
    for body in bodies {
        let Ok(p) = serde_json::from_str::<StorageProgress>(body) else {
            continue;
        };
        if p.host != node {
            continue;
        }
        let keep = by_index
            .get(&p.op_index)
            .is_none_or(|cur| p.published_at_ms >= cur.published_at_ms);
        if keep {
            by_index.insert(p.op_index, p);
        }
    }
    by_index.into_values().collect()
}

/// The single whole disk a queue targets against `topo`, or a typed reason it can't
/// be armed (empty, or spanning multiple disks) — mirrors the worker's arming
/// pre-check so the UI can echo it before publishing (the worker re-checks).
fn queue_target(ops: &[StorageOp], topo: &Topology) -> Result<String, String> {
    let mut targets: Vec<String> = ops
        .iter()
        .filter_map(|op| op.resolve_device(topo))
        .collect();
    targets.sort_unstable();
    targets.dedup();
    match targets.as_slice() {
        [] => Err("The queue resolves to no target disk yet.".to_string()),
        [one] => Ok(one.clone()),
        many => Err(format!(
            "The queue spans {} disks ({}) — arm one disk at a time.",
            many.len(),
            many.join(", ")
        )),
    }
}

/// The `Apply` request a queue + typed arming echo authorize against `node`, or
/// `None` while the gate is shut (empty queue, no single target disk, or an echo
/// that doesn't match — lock 8). The ONE armed-apply decision both the inline
/// Apply button and the Edit → Apply All Operations menu item share (§6), so the
/// typed-confirm gate cannot fork. Pure — unit-tested directly.
fn armed_apply_request(
    node: &NodeStorage,
    queue: &[StorageOp],
    arming: &str,
) -> Option<StorageRequest> {
    if queue.is_empty() {
        return None;
    }
    let target = queue_target(queue, &node.topology).ok()?;
    (arming.trim() == target).then(|| StorageRequest::Apply {
        armed_device: target,
        staged: node.topology.clone(),
        queue: StorageQueue {
            ops: queue.to_vec(),
        },
    })
}

// ──────────────────────────── the compose form ────────────────────────────

/// Which op the compose form builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum OpKind {
    /// A new partition in free space.
    #[default]
    NewPartition,
    /// A fresh partition table on the whole disk.
    NewTable,
    /// Delete a partition.
    Delete,
    /// Resize a partition (grow or shrink — the direction falls out of the new
    /// size vs the current one, mirroring the worker's `Grow`/`Shrink` split).
    Resize,
    /// Move a partition to a new start offset.
    Move,
    /// Format a partition.
    Format,
    /// Set a partition label.
    SetLabel,
    /// Mount a partition.
    Mount,
    /// Unmount a partition.
    Unmount,
}

impl OpKind {
    /// Every op kind, in the composer's dropdown order.
    const ALL: [Self; 9] = [
        Self::NewPartition,
        Self::NewTable,
        Self::Delete,
        Self::Resize,
        Self::Move,
        Self::Format,
        Self::SetLabel,
        Self::Mount,
        Self::Unmount,
    ];

    /// The dropdown label.
    const fn label(self) -> &'static str {
        match self {
            Self::NewPartition => "New partition",
            Self::NewTable => "New partition table",
            Self::Delete => "Delete partition",
            Self::Resize => "Resize (grow / shrink)",
            Self::Move => "Move",
            Self::Format => "Format",
            Self::SetLabel => "Set label",
            Self::Mount => "Mount",
            Self::Unmount => "Unmount",
        }
    }

    /// Whether this kind targets an existing partition (vs the whole disk).
    const fn is_partition_scoped(self) -> bool {
        matches!(
            self,
            Self::Delete
                | Self::Resize
                | Self::Move
                | Self::Format
                | Self::SetLabel
                | Self::Mount
                | Self::Unmount
        )
    }
}

/// The op-composer's raw fields (parsed + built on Stage). One form, `GParted`-style
/// (stage via a dialog), targeting the selected disk. Bounded state.
#[derive(Default)]
struct Compose {
    /// The op kind being built.
    kind: OpKind,
    /// The target partition device (partition-scoped kinds).
    partition: String,
    /// New-partition size (MiB), raw text.
    size_mib: String,
    /// The filesystem for New partition / Format.
    fs: Option<Filesystem>,
    /// The label for New partition / Format / Set label, raw text.
    label: String,
    /// The mount point for Mount, raw text.
    mountpoint: String,
    /// The new start offset (MiB) for Move, raw text.
    new_start_mib: String,
    /// The table scheme for New partition table.
    table: PartTable,
}

impl Compose {
    /// Reset to defaults for a freshly-selected disk.
    fn reset(&mut self) {
        *self = Self {
            fs: Some(Filesystem::Ext4),
            ..Self::default()
        };
    }

    /// Build the staged [`StorageOp`] against whole disk `dev` (its name, free
    /// space, and — for resize/move — the target partition's current geometry),
    /// or a human-readable validation message. Pure.
    fn build(&self, dev: &BlockDevice) -> Result<StorageOp, String> {
        let device = dev.name.as_str();
        let free_mib = dev.free_mib();
        let need_partition = |p: &str| -> Result<String, String> {
            let p = p.trim();
            if p.is_empty() {
                Err("Pick a target partition.".to_string())
            } else {
                Ok(p.to_string())
            }
        };
        match self.kind {
            OpKind::NewTable => Ok(StorageOp::CreateTable {
                device: device.to_string(),
                table: self.table,
            }),
            OpKind::NewPartition => {
                let size = if self.size_mib.trim().is_empty() {
                    free_mib // default: fill the free space
                } else {
                    self.size_mib
                        .trim()
                        .parse::<u64>()
                        .map_err(|_| "Size (MiB) must be a whole number.".to_string())?
                };
                if size == 0 {
                    return Err("Size (MiB) must be greater than 0.".to_string());
                }
                if size > free_mib {
                    return Err(format!("Only {free_mib} MiB free on {device}."));
                }
                Ok(StorageOp::CreatePartition {
                    device: device.to_string(),
                    start_mib: 0, // the executor resolves the aligned start offset
                    size_mib: size,
                    filesystem: self.fs,
                    label: self.trimmed_label(),
                })
            }
            OpKind::Delete => Ok(StorageOp::DeletePartition {
                partition: need_partition(&self.partition)?,
            }),
            OpKind::Resize => self.build_resize(dev, need_partition(&self.partition)?),
            OpKind::Move => self.build_move(dev, need_partition(&self.partition)?),
            OpKind::Format => Ok(StorageOp::Format {
                partition: need_partition(&self.partition)?,
                filesystem: self.fs.ok_or_else(|| "Pick a filesystem.".to_string())?,
                label: self.trimmed_label(),
            }),
            OpKind::SetLabel => {
                let label = self.label.trim();
                if label.is_empty() {
                    return Err("A label is required.".to_string());
                }
                Ok(StorageOp::SetLabel {
                    partition: need_partition(&self.partition)?,
                    label: label.to_string(),
                })
            }
            OpKind::Mount => {
                let mp = self.mountpoint.trim();
                if mp.is_empty() {
                    return Err("A mount point is required.".to_string());
                }
                Ok(StorageOp::Mount {
                    partition: need_partition(&self.partition)?,
                    mountpoint: mp.to_string(),
                })
            }
            OpKind::Unmount => Ok(StorageOp::Unmount {
                partition: need_partition(&self.partition)?,
            }),
        }
    }

    /// The Resize leg of [`Compose::build`]: the direction falls out of the new
    /// size vs the target's current size (the worker's Grow/Shrink split), with
    /// an advisory free-space check mirroring the worker's `InvalidResize` wall
    /// (it re-checks authoritatively). Pure.
    fn build_resize(&self, dev: &BlockDevice, partition: String) -> Result<StorageOp, String> {
        let current = dev.partition_named(&partition)?.size_mib;
        let new_size = self
            .size_mib
            .trim()
            .parse::<u64>()
            .map_err(|_| "New size (MiB) must be a whole number.".to_string())?;
        if new_size == 0 {
            return Err("New size (MiB) must be greater than 0.".to_string());
        }
        match new_size.cmp(&current) {
            Ordering::Equal => Err(format!("{partition} is already {current} MiB.")),
            Ordering::Greater => {
                let growth = new_size - current;
                let free_mib = dev.free_mib();
                if growth > free_mib {
                    return Err(format!(
                        "Growing by {growth} MiB needs more than the {free_mib} MiB free on {}.",
                        dev.name
                    ));
                }
                Ok(StorageOp::Grow {
                    partition,
                    new_size_mib: new_size,
                })
            }
            Ordering::Less => Ok(StorageOp::Shrink {
                partition,
                new_size_mib: new_size,
            }),
        }
    }

    /// The Move leg of [`Compose::build`]: a new start offset for the target
    /// partition, refusing the no-op move. Pure.
    fn build_move(&self, dev: &BlockDevice, partition: String) -> Result<StorageOp, String> {
        let current = dev.partition_named(&partition)?.start_mib;
        let new_start = self
            .new_start_mib
            .trim()
            .parse::<u64>()
            .map_err(|_| "New start (MiB) must be a whole number.".to_string())?;
        if new_start == current {
            return Err(format!("{partition} already starts at {current} MiB."));
        }
        Ok(StorageOp::Move {
            partition,
            new_start_mib: new_start,
        })
    }

    /// The trimmed non-empty label, or `None`.
    fn trimmed_label(&self) -> Option<String> {
        let l = self.label.trim();
        (!l.is_empty()).then(|| l.to_string())
    }
}

// ──────────────────────────── the Storage state ────────────────────────────

/// The Storage surface's live state: the projected per-node topology plus the small
/// IO / form context to drive the pending-op queue and Apply.
pub(crate) struct StorageState {
    /// Desktop-client Bus spool (resolved once). `None` on a box with no Bus dir —
    /// the view then shows its honest empty state, never panics.
    bus_root: Option<PathBuf>,
    /// This node's locally-resolved hostname — the default selected peer.
    local_host: String,
    /// The latest projection, sorted by host. Empty until the first mirror lands.
    nodes: Vec<NodeStorage>,
    /// The peer whose disks fill the surface (its `state/storage/<node>` mirror).
    selected_node: Option<String>,
    /// The whole disk the compose form targets on the selected node.
    selected_device: Option<String>,
    /// The staging form (one open at a time, `GParted`-style).
    compose: Compose,
    /// The compose form's last validation error, shown inline.
    compose_error: Option<String>,
    /// The staged op queue for the selected node (applied in order).
    queue: Vec<StorageOp>,
    /// The operator-typed arming echo (lock 8) for the pending Apply.
    arming: String,
    /// The last Apply / publish error, surfaced inline.
    last_error: Option<String>,
    /// The latest per-op progress for the selected node, folded from the lane.
    progress: Vec<StorageProgress>,
    /// View → Device Rail: a left rail listing the selected peer's disks (MENU-4).
    view_rail: bool,
    /// View → Geometry / Cylinder Detail: the per-disk derived-geometry block.
    view_geometry: bool,
    /// When the Bus was last polled (drives the fixed cadence).
    last_poll: Option<Instant>,
}

impl Default for StorageState {
    fn default() -> Self {
        Self {
            bus_root: mde_bus::client_data_dir(),
            local_host: local_hostname(),
            nodes: Vec::new(),
            selected_node: None,
            selected_device: None,
            compose: Compose::default(),
            compose_error: None,
            queue: Vec::new(),
            arming: String::new(),
            last_error: None,
            progress: Vec::new(),
            view_rail: false,
            view_geometry: false,
            last_poll: None,
        }
    }
}

impl StorageState {
    /// The bus-poll seam: refresh the projection from the Bus when the cadence has
    /// elapsed, then keep the repaint heartbeat alive so a `UDisks2` change on any
    /// peer surfaces without input. Cheap enough to call every frame — it self-gates.
    pub(crate) fn poll(&mut self, ctx: &egui::Context) {
        let due = self.last_poll.is_none_or(|t| t.elapsed() >= REFRESH);
        if due {
            self.last_poll = Some(Instant::now());
            self.refresh();
        }
        ctx.request_repaint_after(REFRESH);
    }

    /// WIN7-4 — this seat's own local-node disk count + total free space
    /// (MiB), folded from the SAME `self.nodes` projection [`Self::poll`]
    /// already keeps current (the identical `state/storage/<node>` mirror
    /// [`project`] folds; no second read, §7). Backs the Start Menu Storage
    /// tile's live facts. `None` until this node's own mirror has landed —
    /// which, honestly, is only once the Storage surface has been opened at
    /// least once this session ([`Self::poll`] only runs while it's active),
    /// matching this module's existing pre-poll-is-honestly-empty posture.
    pub(crate) fn local_summary(&self) -> Option<(usize, u64)> {
        let node = self.nodes.iter().find(|n| n.host == self.local_host)?;
        let disks = node.topology.devices.len();
        let free_mib: u64 = node
            .topology
            .devices
            .iter()
            .map(BlockDevice::free_mib)
            .sum();
        Some((disks, free_mib))
    }

    /// Read the `state/storage/*` mirrors + the selected node's progress lane and
    /// re-project. Split from the cadence gate so the pure projection stays testable;
    /// a missing dir / unreadable topic yields an empty or last-known projection,
    /// never a panic.
    fn refresh(&mut self) {
        let Some(root) = self.bus_root.clone() else {
            self.nodes = Vec::new();
            return;
        };
        // arch-11: open through the shared BusReader seam. The no-root case above
        // clears the roster; a transient open failure keeps the last projection.
        let Some(persist) = BusReader::new(Some(root)).open() else {
            return;
        };
        let topics = persist.list_topics().unwrap_or_default();
        let mut bodies = Vec::new();
        for t in topics.iter().filter(|t| t.starts_with(STATE_PREFIX)) {
            bodies.extend(read_bodies(&persist, t));
        }
        self.nodes = project(&bodies);
        self.ensure_selection();
        if let Some(node) = self.selected_node.clone() {
            let lane = read_bodies(&persist, &progress_topic(&node));
            self.progress = project_progress(&lane, &node);
        }
    }

    /// Keep the peer + disk selection valid against the freshest projection: default
    /// to this node (else the first peer), and to that peer's first disk.
    fn ensure_selection(&mut self) {
        if self
            .selected_node
            .as_ref()
            .is_none_or(|n| !self.nodes.iter().any(|node| &node.host == n))
        {
            self.selected_node = self
                .nodes
                .iter()
                .find(|n| n.host == self.local_host)
                .or_else(|| self.nodes.first())
                .map(|n| n.host.clone());
            self.selected_device = None;
        }
        let devices = self.selected_devices();
        if self
            .selected_device
            .as_ref()
            .is_none_or(|d| !devices.iter().any(|dev| &dev.name == d))
        {
            // Default to the first *unlocked* disk — staging against a protected
            // one is advisory-walled everywhere, so a protected default would
            // grey the whole Partition spine for no reason. Fall back to the
            // first disk (still rendered honestly locked) when all are protected.
            self.selected_device = devices
                .iter()
                .find(|d| d.protected_reason().is_none())
                .or_else(|| devices.first())
                .map(|d| d.name.clone());
            self.compose.reset();
        }
    }

    /// The selected node's view, if any.
    fn selected(&self) -> Option<&NodeStorage> {
        let node = self.selected_node.as_ref()?;
        self.nodes.iter().find(|n| &n.host == node)
    }

    /// The selected node's disks (empty when none / unavailable).
    fn selected_devices(&self) -> Vec<BlockDevice> {
        self.selected()
            .map(|n| n.topology.devices.clone())
            .unwrap_or_default()
    }

    /// Switch the active peer, clearing the per-node queue + arming (a queue is
    /// meaningless against a different node's disks).
    fn select_node(&mut self, host: &str) {
        if self.selected_node.as_deref() == Some(host) {
            return;
        }
        self.selected_node = Some(host.to_string());
        self.selected_device = None;
        self.queue.clear();
        self.arming.clear();
        self.compose_error = None;
        self.last_error = None;
        self.ensure_selection();
    }

    /// Switch the compose target disk — the ONE device-selection seam the inline
    /// "Target … for staging" tap, the View rail, and the menu bar all drive (§6).
    fn select_device(&mut self, name: &str) {
        if self.selected_device.as_deref() == Some(name) {
            return;
        }
        self.selected_device = Some(name.to_string());
        self.compose.reset();
        self.compose_error = None;
    }

    /// The Apply request the current queue + typed arming echo authorize, if any —
    /// the Edit → Apply All Operations gate delegates to the same pure decision
    /// the inline Apply button uses ([`armed_apply_request`], §6 one path).
    fn armed_apply(&self) -> Option<StorageRequest> {
        let node = self.selected()?;
        armed_apply_request(node, &self.queue, &self.arming)
    }

    /// Render the Storage surface's live content.
    pub(crate) fn show(&mut self, ui: &mut egui::Ui) {
        // MENU-4 — the shared top bar, titled **Local Cylinders** (the operator's
        // name for the platform's GParted). The spine mirrors GParted's own
        // (Peer · Edit · View · Device · Partition · Help), every item the mouse
        // twin of a real storage-plane seam (§6, one path): **Edit** owns the
        // pending queue (undo / clear / the typed-armed Apply), **View** toggles
        // the device rail + geometry detail, **Device** rescans + stages a new
        // partition table, **Partition** stages every partition verb (new /
        // delete / resize-move / format-to / mount-unmount / label) through the
        // composer, whose queue only ever reaches a disk via the typed-arming
        // Apply (lock 8). Each entry is honestly gated (§7): context-gated greys,
        // absent omits — never a dead item.
        if let Some(action) = menubar::show(self, ui) {
            menubar::apply(self, action);
        }
        ui.separator();
        ui.colored_label(
            Style::TEXT_DIM,
            "Disks & partitions across the mesh — stage a queue, arm the target, apply over the Bus.",
        );
        ui.add_space(Style::SP_M);

        if self.nodes.is_empty() {
            self.show_empty(ui);
            return;
        }

        self.show_rollup(ui);
        ui.add_space(Style::SP_S);
        self.show_peer_picker(ui);
        // A Refresh re-publishes the selected peer's live topology mirror (the
        // `action/storage/<node>::Refresh` verb) so a hot-plug shows without waiting
        // for the worker's slow heartbeat.
        if let Some(node) = self.selected_node.clone() {
            ui.add_space(Style::SP_XS);
            if ui
                .button(RichText::new("\u{21BB} Refresh topology").size(Style::SMALL))
                .on_hover_text(
                    "Ask this peer's storage worker to re-enumerate + re-publish its disks.",
                )
                .clicked()
            {
                self.publish(&node, &StorageRequest::Refresh);
            }
        }
        ui.add_space(Style::SP_M);
        ui.separator();
        ui.add_space(Style::SP_M);

        // View → Device Rail: a GParted-style left rail of the selected peer's
        // disks (each a tap on the ONE select_device seam), beside the main body.
        if self.view_rail {
            egui::SidePanel::left("storage-device-rail")
                .resizable(false)
                .default_width(Style::SP_XL * 5.0)
                .show_inside(ui, |ui| self.show_device_rail(ui));
        }

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.show_selected(ui);
            });
    }

    /// The View → Device Rail body: every disk on the selected peer, the staging
    /// target highlighted, protected disks listed-but-locked (advisory, lock 7).
    fn show_device_rail(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new("Devices")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add_space(Style::SP_XS);
        let devices = self.selected_devices();
        if devices.is_empty() {
            mde_egui::muted_note(ui, "No disks on this peer.");
            return;
        }
        let mut pick: Option<String> = None;
        for dev in &devices {
            let locked = dev.protected_reason().is_some();
            let is_sel = self.selected_device.as_deref() == Some(dev.name.as_str());
            let text = RichText::new(format!(
                "{} \u{00B7} {} GiB{}",
                dev.name,
                dev.size_mib / 1024,
                if locked { " \u{1F512}" } else { "" }
            ))
            .size(Style::SMALL);
            // A locked disk stays visible for orientation but can't become the
            // staging target here — the same advisory wall the inline tap keeps.
            if ui
                .add_enabled(!locked, egui::SelectableLabel::new(is_sel, text))
                .clicked()
            {
                pick = Some(dev.name.clone());
            }
        }
        if let Some(name) = pick {
            self.select_device(&name);
        }
    }

    /// The honest empty state before any peer has published a storage mirror.
    fn show_empty(&self, ui: &mut egui::Ui) {
        ui.colored_label(
            Style::TEXT_DIM,
            "No peer has published a storage mirror yet.",
        );
        ui.add_space(Style::SP_XS);
        mde_egui::muted_note(
            ui,
            if self.bus_root.is_some() {
                "Each node's mackesd storage worker publishes state/storage/<node> to the Bus \
                 (UDisks2 topology + backend availability). This surface renders it here."
            } else {
                "No mesh Bus directory on this node, so state/storage/<node> can't be read — \
                 joining the mesh (the mde-bus spool) unblocks this surface."
            },
        );
    }

    /// The fleet rollup: disk + peer counts, and how many peers report an
    /// unavailable backend.
    fn show_rollup(&self, ui: &mut egui::Ui) {
        let peers = self.nodes.len();
        let disks: usize = self.nodes.iter().map(|n| n.topology.devices.len()).sum();
        let unavailable = self.nodes.iter().filter(|n| !n.available()).count();
        ui.horizontal(|ui| {
            mde_egui::field(
                ui,
                "Fleet",
                &format!("{disks} disks across {peers} peers"),
                Style::TEXT,
            );
            if unavailable > 0 {
                ui.add_space(Style::SP_S);
                ui.colored_label(
                    Style::WARN,
                    RichText::new(format!("{unavailable} backend(s) unavailable"))
                        .size(Style::SMALL),
                );
            }
        });
    }

    /// The peer picker — one selectable per node that has published storage.
    fn show_peer_picker(&mut self, ui: &mut egui::Ui) {
        let hosts: Vec<(String, bool)> = self
            .nodes
            .iter()
            .map(|n| (n.host.clone(), n.available()))
            .collect();
        let selected = self.selected_node.clone();
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new("Peer")
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL),
            );
            ui.add_space(Style::SP_S);
            for (host, available) in hosts {
                let is_sel = selected.as_deref() == Some(host.as_str());
                let label = if host == self.local_host {
                    format!("{host} (this node)")
                } else {
                    host.clone()
                };
                let tone = if available { Style::OK } else { Style::WARN };
                let resp = ui.selectable_label(
                    is_sel,
                    RichText::new(format!("{DOT} {label}"))
                        .color(tone)
                        .size(Style::SMALL),
                );
                if resp.clicked() {
                    self.select_node(&host);
                }
                ui.add_space(Style::SP_XS);
            }
        });
    }

    /// Render the selected node: backend state → disks (segment bars + tables +
    /// locked rows) → the compose form → the pending queue + Apply → the progress
    /// lane. Collects at most one staged op / one Apply per frame, applied after the
    /// render borrow ends.
    fn show_selected(&mut self, ui: &mut egui::Ui) {
        let Some(node) = self.selected().cloned() else {
            return;
        };

        if let BackendStatus::Unavailable { reason } = &node.backend {
            ui.group(|ui| {
                ui.colored_label(
                    Style::WARN,
                    RichText::new("Storage backend unavailable").strong(),
                );
                ui.add_space(Style::SP_XS);
                mde_egui::muted_note(
                    ui,
                    format!(
                        "{}'s mackesd reports UDisks2 isn't reachable: {reason}. No topology to \
                         render, no ops to stage — this is the honest not-available state (§7).",
                        node.host
                    ),
                );
            });
            return;
        }

        let devices = &node.topology.devices;
        if devices.is_empty() {
            mde_egui::muted_note(
                ui,
                "The backend is available but reported no block devices.",
            );
            return;
        }

        // A walled row (advisory in-use note here, or a `Refused` progress row
        // below) can deep-link to the Cloud plane to free the guest holding the disk.
        // Collected across the render, published once after the borrows end.
        let mut goto_instances = false;

        // Disks — segment bar + partition table + advisory locked rows.
        let mut pick: Option<String> = None;
        for dev in devices {
            ui.group(|ui| {
                show_disk(
                    ui,
                    dev,
                    self.selected_device.as_deref() == Some(dev.name.as_str()),
                    self.view_geometry,
                    &mut goto_instances,
                );
            });
            // A tap on the disk header selects it as the compose target (the same
            // select_device seam the View rail + menu drive).
            ui.add_space(Style::SP_XS);
            let is_sel = self.selected_device.as_deref() == Some(dev.name.as_str());
            if dev.protected_reason().is_none()
                && ui
                    .selectable_label(
                        is_sel,
                        RichText::new(format!("Target {} for staging", dev.name))
                            .size(Style::SMALL),
                    )
                    .clicked()
            {
                pick = Some(dev.name.clone());
            }
            ui.add_space(Style::SP_S);
        }
        if let Some(name) = pick {
            self.select_device(&name);
        }

        ui.separator();
        ui.add_space(Style::SP_S);

        // ── Compose (stage an op against the selected disk) ──
        let mut staged: Option<StorageOp> = None;
        let sel_dev = self
            .selected_device
            .as_deref()
            .and_then(|name| devices.iter().find(|d| d.name == name).cloned());
        {
            let Self {
                compose,
                compose_error,
                ..
            } = self;
            show_compose(ui, sel_dev.as_ref(), compose, compose_error, &mut staged);
        }
        if let Some(op) = staged {
            self.queue.push(op);
            self.compose_error = None;
        }

        ui.add_space(Style::SP_M);
        ui.separator();
        ui.add_space(Style::SP_M);

        // ── Pending queue + Apply (typed arming) ──
        let mut apply: Option<StorageRequest> = None;
        {
            let Self {
                queue,
                arming,
                last_error,
                ..
            } = self;
            show_queue_and_apply(ui, &node, queue, arming, last_error.as_deref(), &mut apply);
        }
        if let Some(req) = apply {
            self.publish(&node.host, &req);
        }

        // ── Progress lane ──
        ui.add_space(Style::SP_M);
        show_progress(ui, &self.progress, &mut goto_instances);

        // A walled-row deep-link hands off to the Cloud plane via the shell's one
        // nav grammar (a `shell/goto/instances` compatibility toast resolves
        // there after QC-15).
        if goto_instances {
            self.emit_goto(&node.host, CLOUD_COMPAT_SURFACE);
        }
    }

    /// Publish a request to `action/storage/<node>` via the persist-first path.
    /// Records any failure in `last_error`; on a successful Apply, clears the queue
    /// + arming (the worker now owns it). Never panics.
    fn publish(&mut self, node: &str, req: &StorageRequest) {
        let Some(root) = self.bus_root.as_ref() else {
            self.last_error =
                Some("No mesh Bus directory — storage actions unavailable.".to_string());
            return;
        };
        let body = req.to_body();
        // arch-11: writer — the shared BusReader seam is read-only; this publish
        // keeps Persist::open because it needs the write Result to set `last_error`.
        match Persist::open(root.clone())
            .and_then(|p| p.write(&action_topic(node), Priority::Default, None, Some(&body)))
        {
            Ok(_) => {
                self.last_error = None;
                if matches!(req, StorageRequest::Apply { .. }) {
                    self.queue.clear();
                    self.arming.clear();
                }
            }
            Err(e) => self.last_error = Some(format!("Couldn't publish storage action: {e}")),
        }
    }

    /// Emit a shell-navigation deep-link for a walled row: a toast carrying the
    /// `shell/goto/<surface>` verb the KIRON toast bridge resolves through the
    /// shell's ONE navigation grammar ([`crate::toast_bridge::resolve_action`], no
    /// second copy). This is how a row blocked by the worker's in-use wall hands the
    /// operator off to the surface that frees it — a running-VM backer routes to the
    /// **Cloud** plane, where the guest can be stopped, then the apply retried.
    /// Reuses the same persist-first publish path as a storage action; a missing Bus
    /// dir is a silent no-op (the button simply can't navigate).
    fn emit_goto(&self, source: &str, surface: &str) {
        let Some(root) = self.bus_root.as_ref() else {
            return;
        };
        let body = serde_json::json!({
            "severity": "info",
            "source_host": source,
            "flag": "STORAGE",
            "headline": format!("Free the disk on {source} to apply"),
            "action_label": "Open Cloud",
            "action_verb": format!("shell/goto/{surface}"),
        })
        .to_string();
        // arch-11: best-effort writer — kept on Persist::open (the shared
        // BusReader seam is read-only).
        let _ = Persist::open(root.clone())
            .and_then(|p| p.write(TOAST_TOPIC, Priority::Default, None, Some(&body)));
    }

    /// Help → Safety & arming posture: publish the surface's one-line safety
    /// contract as an info toast on the shell's real notification lane — the same
    /// persist-first [`TOAST_TOPIC`] path the deep-link rides, so even Help drives
    /// a live seam (§7, the `IaC` Help idiom). Menu-gated on a Bus dir existing.
    fn emit_safety_note(&self) {
        let Some(root) = self.bus_root.as_ref() else {
            return;
        };
        let body = serde_json::json!({
            "severity": "info",
            "source_host": self.local_host,
            "flag": "STORAGE",
            "headline": "Hard walls refuse root/boot/EFI, mesh-storage and in-use disks; \
                         every apply is typed-armed to exactly one disk.",
        })
        .to_string();
        // arch-11: best-effort writer — kept on Persist::open (the shared
        // BusReader seam is read-only).
        let _ = Persist::open(root.clone())
            .and_then(|p| p.write(TOAST_TOPIC, Priority::Default, None, Some(&body)));
    }
}

/// The dock surface a running-VM/container wall routes to (free the guest there).
const CLOUD_COMPAT_SURFACE: &str = "instances";

/// The per-node action topic (`action/storage/<node>`).
fn action_topic(node: &str) -> String {
    format!("action/storage/{node}")
}

/// The per-node progress lane topic (`event/storage/<node>/progress`).
fn progress_topic(node: &str) -> String {
    format!("event/storage/{node}/progress")
}

/// Read the JSON bodies of every retained message on `topic`, oldest first.
fn read_bodies(persist: &Persist, topic: &str) -> Vec<String> {
    persist
        .list_since(topic, None)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|m| m.body)
        .collect()
}

/// The local hostname — `$HOSTNAME` → `/proc/sys/kernel/hostname` → `/etc/hostname`
/// → `"localhost"`. Only used to default the peer picker to this node.
fn local_hostname() -> String {
    if let Ok(h) = std::env::var("HOSTNAME") {
        let h = h.trim();
        if !h.is_empty() {
            return h.to_string();
        }
    }
    for path in ["/proc/sys/kernel/hostname", "/etc/hostname"] {
        if let Ok(h) = std::fs::read_to_string(path) {
            let h = h.trim();
            if !h.is_empty() {
                return h.to_string();
            }
        }
    }
    "localhost".to_string()
}

// ──────────────────────────── render helpers ────────────────────────────

/// A stable per-filesystem fill tone for the segment bar (a Style palette token,
/// never a raw literal). Free space reads dim; the families group by tone.
fn fs_tone(filesystem: Option<&str>) -> Color32 {
    filesystem.map_or(Style::BORDER /* unformatted / raw */, |fs| {
        let fs = fs.to_ascii_lowercase();
        if fs.contains("luks") || fs.contains("crypto") {
            Style::WARN
        } else if fs.contains("swap") {
            Style::TEXT_DIM
        } else if fs.starts_with("ext") || fs == "btrfs" || fs == "xfs" {
            Style::ACCENT
        } else if fs.contains("fat") || fs == "ntfs" || fs == "exfat" {
            Style::ACCENT_HI
        } else {
            Style::OK
        }
    })
}

/// Render one disk: header (name / size / removable / table / lock), the segment
/// bar, the optional derived-geometry detail (View → Geometry / Cylinder Detail),
/// and the partition table. `is_target` marks the compose target.
fn show_disk(
    ui: &mut egui::Ui,
    dev: &BlockDevice,
    is_target: bool,
    geometry: bool,
    goto_instances: &mut bool,
) {
    let protected = dev.protected_reason();
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(&dev.name)
                .color(Style::TEXT)
                .size(Style::BODY)
                .strong(),
        );
        ui.add_space(Style::SP_S);
        mde_egui::muted_note(ui, format!("{} GiB", dev.size_mib / 1024));
        if dev.removable {
            ui.add_space(Style::SP_XS);
            ui.colored_label(Style::ACCENT, RichText::new("removable").size(Style::SMALL));
        }
        if let Some(t) = dev.table {
            ui.add_space(Style::SP_XS);
            mde_egui::muted_note(ui, t.label());
        }
        if is_target {
            ui.add_space(Style::SP_XS);
            ui.colored_label(
                Style::ACCENT,
                RichText::new("\u{2699} staging target").size(Style::SMALL),
            );
        }
    });

    // Locked row (lock 7, advisory) — a protected disk can't be staged here.
    if let Some(reason) = protected {
        ui.add_space(Style::SP_XS);
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(
                Style::DANGER,
                RichText::new("\u{1F512} locked").size(Style::SMALL),
            );
            ui.add_space(Style::SP_XS);
            mde_egui::muted_note(ui, format!("{reason} — the worker refuses ops on it."));
        });
    }

    ui.add_space(Style::SP_XS);
    show_segment_bar(ui, dev);
    ui.add_space(Style::SP_XS);

    // View → Geometry / Cylinder Detail — the derived legacy-geometry readout
    // (the "Local Cylinders" identity), every figure computed from the published
    // topology and labelled derived (§7 — no invented probe data).
    if geometry {
        for line in geometry_lines(dev) {
            mde_egui::muted_note(ui, line);
        }
        ui.add_space(Style::SP_XS);
    }

    // Partition table rows.
    if dev.partitions.is_empty() {
        mde_egui::muted_note(ui, "No partitions (unpartitioned free space).");
    } else {
        for p in &dev.partitions {
            show_partition_row(ui, p);
        }
    }

    // In-use wall reminder (not visible in the topology — worker-enforced) with a
    // live deep-link to the plane that frees it (lock 7 → Cloud).
    ui.add_space(Style::SP_XS);
    ui.horizontal_wrapped(|ui| {
        mde_egui::muted_note(
            ui,
            "A disk backing a running VM/container is refused at apply-time —",
        );
        if ui
            .button(RichText::new("free it in Cloud").size(Style::SMALL))
            .on_hover_text("Jump to the Cloud plane to stop the guest holding this disk.")
            .clicked()
        {
            *goto_instances = true;
        }
    });
}

/// The logical sector size every derived-geometry figure assumes (the udev
/// convention `fdisk` reports against).
const SECTOR_BYTES: u64 = 512;
/// Legacy CHS heads — the fdisk/DOS 255×63 translation every tool derives with.
const CHS_HEADS: u64 = 255;
/// Legacy CHS sectors-per-track (the other half of the 255×63 translation).
const CHS_SECTORS: u64 = 63;

/// The two derived-geometry readout lines for a disk (View → Geometry / Cylinder
/// Detail): sectors + legacy CHS cylinders derived from the published size, and
/// the table / partition / free-space rollup. Pure — unit-tested directly. Every
/// figure is deterministic arithmetic over the worker's real topology, labelled
/// derived (§7): the mirror carries no probed sector size, so the fdisk 512 B /
/// 255×63 convention is stated, never passed off as hardware truth.
fn geometry_lines(dev: &BlockDevice) -> [String; 2] {
    let sectors = dev.size_mib * (1024 * 1024 / SECTOR_BYTES);
    let cylinders = (dev.size_mib * 1024 * 1024) / (CHS_HEADS * CHS_SECTORS * SECTOR_BYTES);
    [
        format!(
            "Geometry (derived @ {SECTOR_BYTES} B sectors): {sectors} sectors \u{00B7} \
             {cylinders} cylinders (legacy CHS {CHS_HEADS}\u{00D7}{CHS_SECTORS})"
        ),
        format!(
            "{} \u{00B7} {} partition(s) \u{00B7} {} MiB free of {} MiB",
            dev.table.map_or("no table", PartTable::label),
            dev.partitions.len(),
            dev.free_mib(),
            dev.size_mib
        ),
    ]
}

/// The `GParted`-style horizontal segment bar: one coloured segment per partition
/// (width ∝ size), free space dim, drawn on the Style palette.
// MiB disk sizes → f32 pixel widths: the precision loss is cosmetic (a segment is
// at most a few thousand px, MiB counts fit f32's 24-bit mantissa well past any
// real disk), so a lossy cast is exactly right for a layout ratio.
#[allow(clippy::cast_precision_loss)]
fn show_segment_bar(ui: &mut egui::Ui, dev: &BlockDevice) {
    let width = ui.available_width().max(Style::SP_XL);
    let height = Style::SP_L;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), Sense::hover());
    let painter = ui.painter().clone();
    // Track (whole disk) — the free-space backdrop.
    painter.rect_filled(rect, Style::RADIUS, Style::SURFACE);
    let total = dev.size_mib.max(1) as f32;
    let mut cursor = rect.left();
    // Segments are laid out in on-disk offset order so gaps read as free space.
    let mut parts: Vec<&Partition> = dev.partitions.iter().collect();
    parts.sort_by_key(|p| p.start_mib);
    for p in parts {
        let seg_w = (p.size_mib as f32 / total) * width;
        if seg_w <= 0.0 {
            continue;
        }
        let seg = egui::Rect::from_min_size(
            egui::pos2(cursor, rect.top()),
            egui::vec2(seg_w.min(rect.right() - cursor).max(0.0), height),
        );
        painter.rect_filled(seg, Style::RADIUS, fs_tone(p.filesystem.as_deref()));
        cursor += seg_w;
    }
}

/// One partition table row: a fs-tone pip + device + size + fs/label + mount state.
fn show_partition_row(ui: &mut egui::Ui, p: &Partition) {
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(DOT)
                .color(fs_tone(p.filesystem.as_deref()))
                .size(Style::SMALL),
        );
        ui.add_space(Style::SP_XS);
        ui.label(RichText::new(&p.name).color(Style::TEXT).size(Style::SMALL));
        ui.add_space(Style::SP_S);
        mde_egui::muted_note(ui, format!("{} GiB", p.size_mib / 1024));
        ui.add_space(Style::SP_S);
        mde_egui::muted_note(ui, p.filesystem.as_deref().unwrap_or("unformatted"));
        if let Some(label) = &p.label {
            ui.add_space(Style::SP_XS);
            ui.colored_label(
                Style::TEXT_DIM,
                RichText::new(format!("\u{201C}{label}\u{201D}")).size(Style::SMALL),
            );
        }
        ui.add_space(Style::SP_S);
        if let Some(mp) = &p.mountpoint {
            ui.colored_label(
                Style::OK,
                RichText::new(format!("mounted {mp}")).size(Style::SMALL),
            );
        } else {
            mde_egui::muted_note(ui, "unmounted");
        }
    });
}

/// The compose form — pick an op kind, fill its context fields, Stage it. Targets
/// the selected disk `dev`; disabled with an honest note when none is selectable.
fn show_compose(
    ui: &mut egui::Ui,
    dev: Option<&BlockDevice>,
    compose: &mut Compose,
    error: &mut Option<String>,
    staged: &mut Option<StorageOp>,
) {
    ui.label(
        RichText::new("Stage an operation")
            .color(Style::TEXT)
            .size(Style::BODY)
            .strong(),
    );
    ui.add_space(Style::SP_XS);

    let Some(dev) = dev else {
        mde_egui::muted_note(
            ui,
            "Select an unlocked disk above to stage operations against it.",
        );
        return;
    };

    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Operation")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add_space(Style::SP_S);
        egui::ComboBox::from_id_salt("storage-op-kind")
            .selected_text(compose.kind.label())
            .show_ui(ui, |ui| {
                for kind in OpKind::ALL {
                    ui.selectable_value(&mut compose.kind, kind, kind.label());
                }
            });
    });

    // Context fields per kind.
    if compose.kind.is_partition_scoped() {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Partition")
                    .color(Style::TEXT_DIM)
                    .size(Style::SMALL),
            );
            ui.add_space(Style::SP_S);
            let current = if compose.partition.is_empty() {
                "— pick —".to_string()
            } else {
                compose.partition.clone()
            };
            egui::ComboBox::from_id_salt("storage-op-partition")
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for p in &dev.partitions {
                        ui.selectable_value(
                            &mut compose.partition,
                            p.name.clone(),
                            p.name.as_str(),
                        );
                    }
                });
        });
    }
    match compose.kind {
        OpKind::NewTable => {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Table")
                        .color(Style::TEXT_DIM)
                        .size(Style::SMALL),
                );
                ui.add_space(Style::SP_S);
                ui.selectable_value(&mut compose.table, PartTable::Gpt, "GPT");
                ui.selectable_value(&mut compose.table, PartTable::Mbr, "MBR");
            });
        }
        OpKind::NewPartition => {
            compose_size_field(ui, compose, dev.free_mib());
            compose_fs_field(ui, compose, "Filesystem (optional)");
            compose_text_field(ui, "Label (optional)", &mut compose.label);
        }
        OpKind::Format => {
            compose_fs_field(ui, compose, "Filesystem");
            compose_text_field(ui, "Label (optional)", &mut compose.label);
        }
        OpKind::SetLabel => compose_text_field(ui, "Label", &mut compose.label),
        OpKind::Mount => compose_text_field(ui, "Mount point", &mut compose.mountpoint),
        OpKind::Resize => compose_resize_fields(ui, compose, dev),
        OpKind::Move => compose_move_fields(ui, compose, dev),
        OpKind::Delete | OpKind::Unmount => {}
    }

    if let Some(err) = error.as_deref() {
        ui.add_space(Style::SP_XS);
        ui.colored_label(Style::DANGER, RichText::new(err).size(Style::SMALL));
    }

    ui.add_space(Style::SP_XS);
    if ui
        .button(RichText::new("\u{FF0B} Stage").size(Style::SMALL))
        .clicked()
    {
        match compose.build(dev) {
            Ok(op) => *staged = Some(op),
            Err(e) => *error = Some(e),
        }
    }
}

/// The Resize context fields: the new-size entry plus the live current-size /
/// free-space anchor for the chosen partition (new-vs-current decides the
/// grow-vs-shrink direction, so the anchor keeps it legible).
fn compose_resize_fields(ui: &mut egui::Ui, compose: &mut Compose, dev: &BlockDevice) {
    compose_text_field(ui, "New size (MiB)", &mut compose.size_mib);
    if let Some(p) = dev.partitions.iter().find(|p| p.name == compose.partition) {
        mde_egui::muted_note(
            ui,
            format!(
                "(currently {} MiB; {} MiB free on {} to grow into)",
                p.size_mib,
                dev.free_mib(),
                dev.name
            ),
        );
    }
}

/// The Move context fields: the new-start entry plus the live current-start
/// anchor for the chosen partition.
fn compose_move_fields(ui: &mut egui::Ui, compose: &mut Compose, dev: &BlockDevice) {
    compose_text_field(ui, "New start (MiB)", &mut compose.new_start_mib);
    if let Some(p) = dev.partitions.iter().find(|p| p.name == compose.partition) {
        mde_egui::muted_note(
            ui,
            format!(
                "(currently starts at {} MiB; data is rewritten \u{2014} slow)",
                p.start_mib
            ),
        );
    }
}

/// A raw MiB size field with the free-space hint.
fn compose_size_field(ui: &mut egui::Ui, compose: &mut Compose, free_mib: u64) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Size (MiB)")
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add_space(Style::SP_S);
        ui.add(egui::TextEdit::singleline(&mut compose.size_mib).desired_width(Style::SP_XL * 3.0));
        ui.add_space(Style::SP_XS);
        mde_egui::muted_note(ui, format!("(blank = all {free_mib} MiB free)"));
    });
}

/// A filesystem picker (with a "none" option for the optional create case).
fn compose_fs_field(ui: &mut egui::Ui, compose: &mut Compose, label: &str) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(label)
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add_space(Style::SP_S);
        let text = compose.fs.map_or("none", Filesystem::label);
        egui::ComboBox::from_id_salt(("storage-fs", label))
            .selected_text(text)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut compose.fs, None, "none");
                for fs in Filesystem::ALL {
                    ui.selectable_value(&mut compose.fs, Some(fs), fs.label());
                }
            });
    });
}

/// A labelled single-line text field on the spacing grid.
fn compose_text_field(ui: &mut egui::Ui, label: &str, value: &mut String) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(label)
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
        );
        ui.add_space(Style::SP_S);
        ui.add(egui::TextEdit::singleline(value).desired_width(Style::SP_XL * 4.0));
    });
}

/// The pending-op queue (reorder / undo) + the typed-arming Apply gate. Sets
/// `apply` to a built `Apply` request only when the arming echo matches the queue's
/// single target disk (the worker re-checks authoritatively).
fn show_queue_and_apply(
    ui: &mut egui::Ui,
    node: &NodeStorage,
    queue: &mut Vec<StorageOp>,
    arming: &mut String,
    last_error: Option<&str>,
    apply: &mut Option<StorageRequest>,
) {
    ui.label(
        RichText::new("Pending operations")
            .color(Style::TEXT)
            .size(Style::BODY)
            .strong(),
    );
    ui.add_space(Style::SP_XS);

    if let Some(err) = last_error {
        ui.colored_label(Style::DANGER, RichText::new(err).size(Style::SMALL));
        ui.add_space(Style::SP_XS);
    }

    if queue.is_empty() {
        mde_egui::muted_note(
            ui,
            "Nothing staged. Nothing touches a disk until you Apply.",
        );
        return;
    }

    // Queue rows with reorder (↑ ↓) + undo (✕). At most one mutation per frame.
    let mut mv_up: Option<usize> = None;
    let mut mv_down: Option<usize> = None;
    let mut remove: Option<usize> = None;
    let len = queue.len();
    for (i, op) in queue.iter().enumerate() {
        ui.horizontal(|ui| {
            mde_egui::muted_note(ui, format!("{}.", i + 1));
            ui.add_space(Style::SP_XS);
            ui.label(
                RichText::new(op.summary())
                    .color(Style::TEXT)
                    .size(Style::SMALL),
            );
            ui.add_space(Style::SP_S);
            if ui
                .add_enabled(
                    i > 0,
                    egui::Button::new(RichText::new("\u{2191}").size(Style::SMALL)),
                )
                .clicked()
            {
                mv_up = Some(i);
            }
            if ui
                .add_enabled(
                    i + 1 < len,
                    egui::Button::new(RichText::new("\u{2193}").size(Style::SMALL)),
                )
                .clicked()
            {
                mv_down = Some(i);
            }
            if ui
                .button(RichText::new("\u{2715}").size(Style::SMALL))
                .clicked()
            {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = mv_up {
        queue.swap(i, i - 1);
    } else if let Some(i) = mv_down {
        queue.swap(i, i + 1);
    } else if let Some(i) = remove {
        queue.remove(i);
    }

    ui.add_space(Style::SP_S);
    if ui
        .button(RichText::new("Clear queue").size(Style::SMALL))
        .clicked()
    {
        queue.clear();
        arming.clear();
        return;
    }

    ui.add_space(Style::SP_M);
    ui.separator();
    ui.add_space(Style::SP_S);

    // ── Typed arming (lock 8) ──
    match queue_target(queue, &node.topology) {
        Err(why) => {
            ui.colored_label(Style::WARN, RichText::new(why).size(Style::SMALL));
        }
        Ok(target) => {
            ui.label(
                RichText::new(format!(
                    "Arming — type the target device exactly to apply {} op(s) to {} on {}:",
                    queue.len(),
                    target,
                    node.host
                ))
                .color(Style::TEXT_DIM)
                .size(Style::SMALL),
            );
            ui.add_space(Style::SP_XS);
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(arming)
                        .hint_text(target.as_str())
                        .desired_width(Style::SP_XL * 5.0),
                );
                ui.add_space(Style::SP_S);
                // The same pure decision the Edit → Apply All menu item gates on
                // (§6 one path): a request exists only when the echo matches.
                let request = armed_apply_request(node, queue, arming);
                if ui
                    .add_enabled(
                        request.is_some(),
                        egui::Button::new(RichText::new("Apply").size(Style::SMALL)),
                    )
                    .on_hover_text(
                        "Publishes action/storage/<node>::Apply. The worker re-validates, \
                         re-checks the arming + walls, and streams per-op progress.",
                    )
                    .clicked()
                {
                    *apply = request;
                }
            });
            ui.add_space(Style::SP_XS);
            mde_egui::muted_note(
                ui,
                "Live apply is E12-23 (the worker's executor is integration-gated today): the \
                 queue reaches the worker, which stages, walls, and reports the gated state.",
            );
        }
    }
}

/// The progress lane — the latest per-op terminal state of the current apply.
fn show_progress(ui: &mut egui::Ui, progress: &[StorageProgress], goto_instances: &mut bool) {
    ui.label(
        RichText::new("Apply progress")
            .color(Style::TEXT)
            .size(Style::BODY)
            .strong(),
    );
    ui.add_space(Style::SP_XS);
    if progress.is_empty() {
        mde_egui::muted_note(ui, "No apply has run for this peer yet.");
        return;
    }
    for p in progress {
        let (tone, word) = p.state.tone_word();
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(DOT).color(tone).size(Style::SMALL));
            ui.add_space(Style::SP_XS);
            mde_egui::muted_note(ui, format!("{}/{}", p.op_index + 1, p.total));
            ui.add_space(Style::SP_XS);
            ui.label(
                RichText::new(&p.op_kind)
                    .color(Style::TEXT)
                    .size(Style::SMALL),
            );
            ui.add_space(Style::SP_S);
            ui.colored_label(tone, RichText::new(word).size(Style::SMALL));
        });
        if let Some(detail) = p.state.detail() {
            ui.indent(("storage-progress", p.op_index), |ui| {
                mde_egui::muted_note(ui, detail);
                if matches!(p.state, ProgressState::Refused { .. }) {
                    ui.horizontal_wrapped(|ui| {
                        mde_egui::muted_note(ui, "\u{2192} free the disk, then re-apply:");
                        if ui
                            .button(RichText::new("Open Cloud").size(Style::SMALL))
                            .on_hover_text(
                                "Jump to the Cloud plane to stop the guest holding this disk.",
                            )
                            .clicked()
                        {
                            *goto_instances = true;
                        }
                    });
                }
            });
        }
    }
}

/// MENU-4 — the **Local Cylinders** bar: the platform's `GParted` spine over the
/// storage plane.
///
/// The spine mirrors `GParted`'s own menu order (app · Edit · View · Device ·
/// Partition · Help), with **Peer** in the app slot (the mesh dimension `GParted`
/// never had). Every item is the mouse twin of a seam the surface already drives
/// (§6, one path): **Peer** switches the active node (`select_node`); **Edit**
/// owns the pending queue — Undo Last / Clear All, and **Apply All Operations**,
/// which shares the inline button's exact typed-arming decision
/// ([`super::armed_apply_request`], lock 8) so the menu can never bypass the
/// typed confirm; **View** toggles the device rail + derived-geometry detail;
/// **Device** rescans the topology (`Refresh`) and stages a new partition table;
/// **Partition** stages every partition verb (new / delete / resize-move /
/// format-to‹fs› / mount-unmount / label) by jumping the composer — each staged
/// op only ever reaches a disk through the typed-armed Apply; **Help** carries
/// the surface identity and publishes the safety posture on the live toast lane.
/// Each entry is honestly gated (§7): Peer omits itself until a peer publishes,
/// destructive verbs grey without an unlocked target disk, Mount/Unmount grey
/// without a partition in the matching state, Apply All greys until the echo is
/// typed — never a dead entry. Chips: fleet rollup · peer health · the selected
/// device · the pending-op count.
mod menubar {
    use super::{BlockDevice, Filesystem, OpKind, StorageRequest, StorageState, DOT};
    use mde_egui::egui::Ui;
    use mde_egui::menubar::{Entry, Item, Menu, MenuBar, MenuBarModel};
    use mde_egui::{ChipTone, StatusChip, Style};

    /// One menu action — each routes to a real Storage seam in [`apply`]. Owned (a
    /// peer id is a `String`), so `Clone` (not `Copy`) satisfies the shared bar.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(super) enum MenuAction {
        /// Switch the active peer (the picker's `select_node` seam).
        SelectPeer(String),
        /// Re-publish the selected peer's live topology (`StorageRequest::Refresh`).
        RescanDevices,
        /// Pop the most recently staged op (`GParted`'s Undo Last Operation).
        UndoLast,
        /// Drop the staged op queue + its arming echo.
        ClearQueue,
        /// Publish the typed-armed Apply (enabled only while the echo matches).
        ApplyAll,
        /// Toggle the View → Device Rail.
        ToggleRail,
        /// Toggle the View → Geometry / Cylinder Detail.
        ToggleGeometry,
        /// Jump the compose form to this op kind (its own dropdown seam).
        StageKind(OpKind),
        /// Jump the compose form to Format with this filesystem preset
        /// (`GParted`'s "Format to ›" submenu).
        StageFormat(Filesystem),
        /// Publish the safety-posture note on the toast lane (Help).
        HelpSafety,
    }

    /// Render the LOCAL CYLINDERS bar and return the action picked this frame.
    pub(super) fn show(state: &StorageState, ui: &mut Ui) -> Option<MenuAction> {
        let menus = build_menus(state);
        let status = build_status(state);
        let model = MenuBarModel {
            // The operator's name for the platform's GParted (MENU-4). Storage
            // sits in the dock's "System" group (gold), so the title wears that
            // categorical accent (lock 2).
            title: "Local Cylinders",
            accent: Style::ACCENT_SYSTEM,
            menus: &menus,
            status: &status,
        };
        MenuBar::show(ui, &model)
    }

    /// The selected disk's live row, if any.
    fn selected_disk(state: &StorageState) -> Option<BlockDevice> {
        let name = state.selected_device.as_deref()?;
        state
            .selected_devices()
            .into_iter()
            .find(|d| d.name == name)
    }

    /// Build the `GParted` spine from live state, each entry honestly gated (§7).
    fn build_menus(state: &StorageState) -> Vec<Menu<MenuAction>> {
        let mut menus = Vec::new();

        // Peer — one radio per published node (omitted entirely until a peer lands).
        if !state.nodes.is_empty() {
            let peers: Vec<Entry<MenuAction>> = state
                .nodes
                .iter()
                .map(|n| {
                    let checked = state.selected_node.as_deref() == Some(n.host.as_str());
                    let label = if n.host == state.local_host {
                        format!("{} (this node)", n.host)
                    } else {
                        n.host.clone()
                    };
                    Entry::Item(
                        Item::new(MenuAction::SelectPeer(n.host.clone()), label).checked(checked),
                    )
                })
                .collect();
            menus.push(Menu::new("Peer", peers));
        }

        menus.push(build_edit_menu(state));
        menus.push(build_view_menu(state));
        menus.push(build_device_menu(state));
        menus.push(build_partition_menu(state));
        menus.push(build_help_menu(state));
        menus
    }

    /// **Edit** — the pending-queue verbs (`GParted`'s Edit menu, lock 8 intact):
    /// Undo Last / Clear All need a queue; Apply All shares the inline button's
    /// typed-arming decision and greys until the echo matches.
    fn build_edit_menu(state: &StorageState) -> Menu<MenuAction> {
        let staged = !state.queue.is_empty();
        let armed = state.armed_apply().is_some();
        let mut entries = vec![
            Entry::Item(Item::new(MenuAction::UndoLast, "Undo Last Operation").enabled(staged)),
            Entry::Item(Item::new(MenuAction::ClearQueue, "Clear All Operations").enabled(staged)),
            Entry::Separator,
            Entry::Item(Item::new(MenuAction::ApplyAll, "Apply All Operations").enabled(armed)),
        ];
        if staged && !armed {
            // An honest caption, not a dead item: why Apply is grey right now.
            entries.push(Entry::Caption(
                "Type the target device below to arm Apply.".to_string(),
            ));
        }
        Menu::new("Edit", entries)
    }

    /// **View** — the device rail + derived-geometry toggles; greyed until the
    /// selected peer has published a disk to show.
    fn build_view_menu(state: &StorageState) -> Menu<MenuAction> {
        let has_disks = !state.selected_devices().is_empty();
        Menu::new(
            "View",
            vec![
                Entry::Item(
                    Item::new(MenuAction::ToggleRail, "Device Rail")
                        .checked(state.view_rail)
                        .enabled(has_disks),
                ),
                Entry::Item(
                    Item::new(MenuAction::ToggleGeometry, "Geometry / Cylinder Detail")
                        .checked(state.view_geometry)
                        .enabled(has_disks),
                ),
            ],
        )
    }

    /// **Device** — whole-disk verbs: rescan (the worker's `Refresh`), and a new
    /// partition table staged through the composer (typed-armed at Apply).
    fn build_device_menu(state: &StorageState) -> Menu<MenuAction> {
        let disk = selected_disk(state);
        let stageable = disk
            .as_ref()
            .is_some_and(|d| d.protected_reason().is_none());
        Menu::new(
            "Device",
            vec![
                Entry::Caption(disk.as_ref().map_or_else(
                    || "No disk selected.".to_string(),
                    |d| format!("Selected: {}", d.name),
                )),
                Entry::Item(
                    Item::new(MenuAction::RescanDevices, "Rescan Devices")
                        .enabled(state.selected_node.is_some()),
                ),
                Entry::Separator,
                Entry::Item(
                    Item::new(
                        MenuAction::StageKind(OpKind::NewTable),
                        "New Partition Table\u{2026}",
                    )
                    .enabled(stageable),
                ),
            ],
        )
    }

    /// **Partition** — the full `GParted` verb set, staged through the composer.
    /// Each verb greys honestly: no unlocked target disk shuts them all;
    /// partition-scoped verbs need a partition; Mount/Unmount need one in the
    /// matching state; New needs free space to carve.
    fn build_partition_menu(state: &StorageState) -> Menu<MenuAction> {
        let disk = selected_disk(state);
        let stageable = disk
            .as_ref()
            .is_some_and(|d| d.protected_reason().is_none());
        let has_parts = stageable && disk.as_ref().is_some_and(|d| !d.partitions.is_empty());
        let has_free = stageable && disk.as_ref().is_some_and(|d| d.free_mib() > 0);
        let any_mounted = has_parts
            && disk
                .as_ref()
                .is_some_and(|d| d.partitions.iter().any(|p| p.mountpoint.is_some()));
        let any_unmounted = has_parts
            && disk
                .as_ref()
                .is_some_and(|d| d.partitions.iter().any(|p| p.mountpoint.is_none()));

        let item_for = |kind: OpKind, label: &str, enabled: bool| {
            Entry::Item(Item::new(MenuAction::StageKind(kind), label).enabled(enabled))
        };
        let format_to: Vec<Entry<MenuAction>> = Filesystem::ALL
            .iter()
            .map(|&fs| {
                Entry::Item(Item::new(MenuAction::StageFormat(fs), fs.label()).enabled(has_parts))
            })
            .collect();

        Menu::new(
            "Partition",
            vec![
                item_for(OpKind::NewPartition, "New\u{2026}", has_free),
                Entry::Separator,
                item_for(OpKind::Delete, "Delete", has_parts),
                item_for(OpKind::Resize, "Resize (Grow / Shrink)\u{2026}", has_parts),
                item_for(OpKind::Move, "Move\u{2026}", has_parts),
                Entry::Separator,
                Entry::Submenu {
                    label: "Format to".to_string(),
                    mnemonic: None,
                    entries: format_to,
                },
                Entry::Separator,
                item_for(OpKind::Mount, "Mount\u{2026}", any_unmounted),
                item_for(OpKind::Unmount, "Unmount", any_mounted),
                Entry::Separator,
                item_for(OpKind::SetLabel, "Label\u{2026}", has_parts),
            ],
        )
    }

    /// **Help** — the honest surface identity plus one real seam: the safety
    /// posture published on the live toast lane (greyed with no Bus dir, so the
    /// item is never a silent no-op).
    fn build_help_menu(state: &StorageState) -> Menu<MenuAction> {
        Menu::new(
            "Help",
            vec![
                Entry::Caption(
                    "Local Cylinders \u{2014} GParted-class disk surgery over the mesh \
                     storage plane."
                        .to_string(),
                ),
                Entry::Item(
                    Item::new(MenuAction::HelpSafety, "Safety & arming posture\u{2026}")
                        .enabled(state.bus_root.is_some()),
                ),
            ],
        )
    }

    /// The live status cluster: the fleet rollup (disks · peers), the selected
    /// peer's backend health, the selected device, and the pending-op count.
    fn build_status(state: &StorageState) -> Vec<StatusChip> {
        let peers = state.nodes.len();
        let disks: usize = state.nodes.iter().map(|n| n.topology.devices.len()).sum();
        let mut chips = vec![StatusChip::new(
            format!(
                "{disks} disk{} \u{00B7} {peers} peer{}",
                if disks == 1 { "" } else { "s" },
                if peers == 1 { "" } else { "s" }
            ),
            ChipTone::Neutral,
        )];

        if let Some(node) = state.selected() {
            let tone = if node.available() {
                ChipTone::Ok
            } else {
                ChipTone::Warn
            };
            chips.push(StatusChip::with_icon(DOT, node.host.clone(), tone));
        }

        // The selected device + the pending-op count — the MENU-4 pair.
        if let Some(device) = &state.selected_device {
            chips.push(StatusChip::new(device.clone(), ChipTone::Info));
        }
        let staged = state.queue.len();
        chips.push(StatusChip::new(
            format!("{staged} pending"),
            if staged > 0 {
                ChipTone::Info
            } else {
                ChipTone::Neutral
            },
        ));
        chips
    }

    /// Apply a picked action to its real seam (§6, no new behaviour).
    pub(super) fn apply(state: &mut StorageState, action: MenuAction) {
        match action {
            MenuAction::SelectPeer(host) => state.select_node(&host),
            MenuAction::RescanDevices => {
                if let Some(node) = state.selected_node.clone() {
                    state.publish(&node, &StorageRequest::Refresh);
                }
            }
            MenuAction::UndoLast => {
                state.queue.pop();
                if state.queue.is_empty() {
                    state.arming.clear();
                }
            }
            MenuAction::ClearQueue => {
                state.queue.clear();
                state.arming.clear();
                state.compose_error = None;
            }
            MenuAction::ApplyAll => {
                // The gate re-decides here (never trusts the render frame's
                // enable): no matching echo ⇒ no request ⇒ nothing publishes.
                if let (Some(node), Some(req)) = (state.selected_node.clone(), state.armed_apply())
                {
                    state.publish(&node, &req);
                }
            }
            MenuAction::ToggleRail => state.view_rail = !state.view_rail,
            MenuAction::ToggleGeometry => state.view_geometry = !state.view_geometry,
            MenuAction::StageKind(kind) => {
                state.compose.kind = kind;
                state.compose_error = None;
            }
            MenuAction::StageFormat(fs) => {
                state.compose.kind = OpKind::Format;
                state.compose.fs = Some(fs);
                state.compose_error = None;
            }
            MenuAction::HelpSafety => state.emit_safety_note(),
        }
    }

    #[cfg(test)]
    #[allow(clippy::panic)]
    mod tests {
        use super::super::{
            project, state_body, BackendStatus, Filesystem, NodeStorage, OpKind, StorageOp,
            StorageState, Topology,
        };
        use super::{apply, build_menus, build_status, MenuAction};
        use mde_egui::menubar::{Entry, Menu};
        use mde_egui::ChipTone;

        /// One node view with the given backend health (an empty topology is enough
        /// for the bar's peer + rollup seams).
        fn node(host: &str, available: bool) -> NodeStorage {
            NodeStorage {
                host: host.to_string(),
                backend: if available {
                    BackendStatus::Available
                } else {
                    BackendStatus::Unavailable {
                        reason: "UDisks2 unreachable".to_string(),
                    }
                },
                topology: Topology::default(),
                published_at_ms: 0,
            }
        }

        /// A state carrying two (disk-less) nodes — the peer + rollup seams.
        /// `bus_root: None` keeps the Help gate deterministic off the build host's
        /// environment.
        fn two_node_state() -> StorageState {
            StorageState {
                nodes: vec![node("nodeA", true), node("nodeB", false)],
                local_host: "nodeA".to_string(),
                selected_node: Some("nodeA".to_string()),
                bus_root: None,
                ..StorageState::default()
            }
        }

        /// A state with the two-disk fixture topology (sda protected, sdb free),
        /// sdb selected — the full Partition-spine gating context.
        fn disk_state() -> StorageState {
            let mut state = StorageState {
                nodes: project(&[state_body("nodeA", 1, true)]),
                local_host: "nodeA".to_string(),
                bus_root: None,
                ..StorageState::default()
            };
            state.ensure_selection();
            state
        }

        /// Every activatable item of `menu`, flattened through submenus.
        fn items(menu: &Menu<MenuAction>) -> Vec<&super::Item<MenuAction>> {
            fn walk<'a>(
                entries: &'a [Entry<MenuAction>],
                out: &mut Vec<&'a super::Item<MenuAction>>,
            ) {
                for e in entries {
                    match e {
                        Entry::Item(i) => out.push(i),
                        Entry::Submenu { entries, .. } => walk(entries, out),
                        Entry::Separator | Entry::Caption(_) => {}
                    }
                }
            }
            let mut out = Vec::new();
            walk(&menu.entries, &mut out);
            out
        }

        fn menu<'a>(menus: &'a [Menu<MenuAction>], title: &str) -> &'a Menu<MenuAction> {
            menus
                .iter()
                .find(|m| m.title == title)
                .unwrap_or_else(|| panic!("{title} menu present"))
        }

        #[test]
        fn the_spine_is_the_gparted_order_and_greys_shut_when_empty() {
            let empty = StorageState {
                bus_root: None,
                ..StorageState::default()
            };
            let menus = build_menus(&empty);
            assert!(
                !menus.iter().any(|m| m.title == "Peer"),
                "no peer ⇒ the Peer menu is omitted, not present-but-empty (§7)"
            );
            // The GParted spine stays constant (a stable chrome), items greyed.
            let titles: Vec<&str> = menus.iter().map(|m| m.title.as_str()).collect();
            assert_eq!(titles, ["Edit", "View", "Device", "Partition", "Help"]);
            for m in &menus {
                for item in items(m) {
                    assert!(
                        !item.enabled,
                        "{} › {} greys with no peer / disk / queue / Bus",
                        m.title, item.label
                    );
                }
            }
        }

        #[test]
        fn peer_menu_lists_every_node_with_the_active_one_checked() {
            let state = two_node_state();
            let menus = build_menus(&state);
            let peer = menu(&menus, "Peer");
            let entries = items(peer);
            assert_eq!(entries.len(), 2, "both peers reachable");
            for item in entries {
                let is_a = item.id == MenuAction::SelectPeer("nodeA".to_string());
                assert_eq!(item.checked, Some(is_a), "only the active peer is checked");
            }
        }

        #[test]
        fn the_partition_spine_arms_over_the_unlocked_disk() {
            let state = disk_state();
            assert_eq!(
                state.selected_device.as_deref(),
                Some("/dev/sdb"),
                "the default target skips the protected root disk"
            );
            let menus = build_menus(&state);
            let partition = menu(&menus, "Partition");
            let by_label = |label: &str| {
                items(partition)
                    .into_iter()
                    .find(|i| i.label == label)
                    .unwrap_or_else(|| panic!("{label} present"))
                    .enabled
            };
            assert!(by_label("New\u{2026}"), "free space ⇒ New enabled");
            assert!(by_label("Delete"), "a partition exists ⇒ Delete enabled");
            assert!(by_label("Resize (Grow / Shrink)\u{2026}"));
            assert!(by_label("Move\u{2026}"));
            assert!(
                by_label("Mount\u{2026}"),
                "sdb1 is unmounted ⇒ Mount enabled"
            );
            assert!(
                !by_label("Unmount"),
                "nothing mounted on sdb ⇒ Unmount greys (§7)"
            );
            // Format to › carries every filesystem, enabled over the live target.
            let fs_items: Vec<_> = items(partition)
                .into_iter()
                .filter(|i| matches!(i.id, MenuAction::StageFormat(_)))
                .collect();
            assert_eq!(fs_items.len(), Filesystem::ALL.len());
            assert!(fs_items.iter().all(|i| i.enabled));
            // Device › New Partition Table is stageable over the unlocked disk.
            let device = menu(&menus, "Device");
            assert!(items(device)
                .into_iter()
                .any(|i| i.id == MenuAction::StageKind(OpKind::NewTable) && i.enabled));
        }

        #[test]
        fn apply_all_arms_only_on_the_typed_echo() {
            let mut state = disk_state();
            state.queue.push(StorageOp::DeletePartition {
                partition: "/dev/sdb1".to_string(),
            });
            let enabled_apply = |state: &StorageState| {
                items(menu(&build_menus(state), "Edit"))
                    .into_iter()
                    .find(|i| i.label == "Apply All Operations")
                    .expect("Apply All present")
                    .enabled
            };
            assert!(
                !enabled_apply(&state),
                "no echo ⇒ Apply All greys (lock 8 — the menu can't bypass arming)"
            );
            state.arming = "/dev/wrong".to_string();
            assert!(!enabled_apply(&state), "a wrong echo keeps it grey");
            state.arming = "/dev/sdb".to_string();
            assert!(enabled_apply(&state), "the exact echo arms it");
            // The apply path re-decides the gate itself; with no Bus dir the
            // publish records the honest error rather than silently dropping.
            apply(&mut state, MenuAction::ApplyAll);
            assert!(state.last_error.is_some(), "no Bus dir ⇒ the honest error");
        }

        #[test]
        fn undo_last_pops_one_op_and_clearing_empties_the_queue() {
            let mut state = disk_state();
            state.queue = vec![
                StorageOp::Unmount {
                    partition: "/dev/sdb1".to_string(),
                },
                StorageOp::DeletePartition {
                    partition: "/dev/sdb1".to_string(),
                },
            ];
            state.arming = "/dev/sdb".to_string();
            apply(&mut state, MenuAction::UndoLast);
            assert_eq!(state.queue.len(), 1, "undo pops the most recent op");
            assert_eq!(state.arming, "/dev/sdb", "a live queue keeps the echo");
            apply(&mut state, MenuAction::UndoLast);
            assert!(state.queue.is_empty());
            assert!(
                state.arming.is_empty(),
                "an emptied queue drops the stale echo"
            );

            state.queue = vec![StorageOp::Unmount {
                partition: "/dev/sdb1".to_string(),
            }];
            state.arming = "/dev/sdb".to_string();
            apply(&mut state, MenuAction::ClearQueue);
            assert!(state.queue.is_empty(), "the queue is cleared");
            assert!(state.arming.is_empty(), "the arming echo is cleared");
        }

        #[test]
        fn selecting_a_peer_switches_the_active_node() {
            let mut state = two_node_state();
            apply(&mut state, MenuAction::SelectPeer("nodeB".to_string()));
            assert_eq!(state.selected_node.as_deref(), Some("nodeB"));
        }

        #[test]
        fn stage_verbs_jump_the_compose_form() {
            let mut state = two_node_state();
            apply(&mut state, MenuAction::StageKind(OpKind::Resize));
            assert_eq!(state.compose.kind, OpKind::Resize);
            // Format to › presets the filesystem too.
            apply(&mut state, MenuAction::StageFormat(Filesystem::Xfs));
            assert_eq!(state.compose.kind, OpKind::Format);
            assert_eq!(state.compose.fs, Some(Filesystem::Xfs));
        }

        #[test]
        fn view_toggles_flip_and_read_back_checked() {
            let mut state = disk_state();
            apply(&mut state, MenuAction::ToggleRail);
            apply(&mut state, MenuAction::ToggleGeometry);
            assert!(state.view_rail && state.view_geometry);
            let menus = build_menus(&state);
            for item in items(menu(&menus, "View")) {
                assert_eq!(item.checked, Some(true), "{} reads back on", item.label);
            }
        }

        #[test]
        fn status_shows_rollup_health_device_and_pending_count() {
            let mut state = disk_state();
            state.queue = vec![StorageOp::Unmount {
                partition: "/dev/sdb1".to_string(),
            }];
            let chips = build_status(&state);
            assert!(chips.iter().any(|c| c.text.contains("2 disks")));
            assert!(chips
                .iter()
                .any(|c| c.text == "nodeA" && c.tone == ChipTone::Ok));
            assert!(
                chips
                    .iter()
                    .any(|c| c.text == "/dev/sdb" && c.tone == ChipTone::Info),
                "the selected device chip (MENU-4)"
            );
            assert!(chips
                .iter()
                .any(|c| c.text == "1 pending" && c.tone == ChipTone::Info));
            // An empty queue still reads an honest zero, never a vanished chip.
            state.queue.clear();
            let chips = build_status(&state);
            assert!(chips
                .iter()
                .any(|c| c.text == "0 pending" && c.tone == ChipTone::Neutral));
        }
    }
}

/// A faithful `state/storage/<node>` mirror body — the exact shape the E12-20
/// worker's `StorageState` serializes. Module-scoped so the menubar + coverage
/// test modules share the ONE fixture topology (sda protected, sdb free).
#[cfg(test)]
fn state_body(host: &str, at: u64, available: bool) -> String {
    if !available {
        return format!(
            r#"{{"host":"{host}","backend":{{"status":"unavailable","reason":"no system bus"}},"topology":{{"devices":[]}},"published_at_ms":{at}}}"#
        );
    }
    format!(
        r#"{{"host":"{host}","backend":{{"status":"available"}},"topology":{{"devices":[
          {{"name":"/dev/sda","size_mib":51200,"table":"gpt","removable":false,"partitions":[
            {{"name":"/dev/sda1","number":1,"start_mib":1,"size_mib":512,"filesystem":"vfat","mountpoint":"/boot/efi"}},
            {{"name":"/dev/sda2","number":2,"start_mib":513,"size_mib":50000,"filesystem":"ext4","mountpoint":"/"}}
          ]}},
          {{"name":"/dev/sdb","size_mib":16384,"table":"gpt","removable":true,"partitions":[
            {{"name":"/dev/sdb1","number":1,"start_mib":1,"size_mib":8192,"filesystem":"ext4","label":"data"}}
          ]}}
        ]}},"published_at_ms":{at}}}"#
    )
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;
    use mde_egui::egui::{pos2, vec2, Rect};

    /// A `state/storage/<node>` progress body.
    fn progress_body(
        host: &str,
        idx: usize,
        total: usize,
        kind: &str,
        at: u64,
        refused: bool,
    ) -> String {
        let state = if refused {
            r#"{"state":"refused","reason":"backs running VM db1"}"#
        } else {
            r#"{"state":"applied"}"#
        };
        format!(
            r#"{{"host":"{host}","device":"/dev/sdb","op_index":{idx},"total":{total},"op_kind":"{kind}","state":{state},"published_at_ms":{at}}}"#
        )
    }

    /// Drive one headless 960×720 frame of the surface + tessellate it on the CPU —
    /// the same `Context::run` → `tessellate` path the DRM runner drives, minus the
    /// GPU. Returns whether it produced any draw primitives.
    fn renders(state: &mut StorageState) -> bool {
        let ctx = egui::Context::default();
        Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, 720.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| state.show(ui));
        });
        !ctx.tessellate(out.shapes, out.pixels_per_point).is_empty()
    }

    #[test]
    fn project_folds_one_row_per_host_sorted_latest_wins() {
        let bodies = vec![
            state_body("node-b", 1, true),
            state_body("node-a", 5, false),
            state_body("node-a", 9, true), // newer wins for node-a
        ];
        let nodes = project(&bodies);
        assert_eq!(nodes.len(), 2, "one row per host");
        assert_eq!(nodes[0].host, "node-a", "BTreeMap key order → host-sorted");
        assert_eq!(nodes[1].host, "node-b");
        assert!(
            nodes[0].available(),
            "the newer node-a mirror (available) wins"
        );
        assert_eq!(nodes[0].published_at_ms, 9);
        assert_eq!(nodes[0].topology.devices.len(), 2);
    }

    #[test]
    fn project_skips_malformed_bodies() {
        let bodies = vec![
            "not json".to_string(),
            "{}".to_string(), // missing required fields
            state_body("node-a", 1, true),
        ];
        let nodes = project(&bodies);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].host, "node-a");
    }

    #[test]
    fn protected_reason_derives_the_root_boot_and_mesh_walls() {
        let nodes = project(&[state_body("n", 1, true)]);
        let sda = &nodes[0].topology.devices[0];
        let sdb = &nodes[0].topology.devices[1];
        // /dev/sda has / and /boot/efi mounted → root/boot/EFI protection wins.
        assert_eq!(
            sda.protected_reason(),
            Some("backs the node's root / boot / EFI chain")
        );
        // /dev/sdb is a plain removable data disk → unprotected (stageable).
        assert!(sdb.protected_reason().is_none());

        // A mesh-storage backer is protected with its own reason.
        let mesh = r#"{"host":"n","backend":{"status":"available"},"topology":{"devices":[
          {"name":"/dev/sdc","size_mib":1024,"partitions":[
            {"name":"/dev/sdc1","number":1,"start_mib":1,"size_mib":1000,"filesystem":"xfs","mountpoint":"/mnt/mesh-storage"}]}
        ]},"published_at_ms":1}"#;
        let m = project(&[mesh.to_string()]);
        assert_eq!(
            m[0].topology.devices[0].protected_reason(),
            Some("backs /mnt/mesh-storage (the mesh shared volume)")
        );
    }

    #[test]
    fn free_mib_is_total_minus_used() {
        let nodes = project(&[state_body("n", 1, true)]);
        let sdb = &nodes[0].topology.devices[1];
        assert_eq!(sdb.free_mib(), 16384 - 8192);
    }

    /// The fixture's free data disk: /dev/sdb, 16384 MiB, one 8192 MiB ext4
    /// partition starting at 1 MiB → 8192 MiB free.
    fn sdb() -> BlockDevice {
        project(&[state_body("n", 1, true)])[0].topology.devices[1].clone()
    }

    #[test]
    fn compose_builds_each_op_kind_to_the_worker_shape() {
        let dev = sdb();
        let as_json = |op: &StorageOp| -> serde_json::Value {
            serde_json::from_str(&serde_json::to_string(op).unwrap_or_default()).unwrap_or_default()
        };

        // New partition (blank size → fills free space), ext4 + label.
        let mut new_part = Compose {
            kind: OpKind::NewPartition,
            fs: Some(Filesystem::Ext4),
            label: "scratch".to_string(),
            ..Compose::default()
        };
        let json = as_json(&new_part.build(&dev).expect("new partition builds"));
        assert_eq!(json["op"], "create_partition");
        assert_eq!(json["device"], "/dev/sdb");
        assert_eq!(json["size_mib"], 8192, "blank size fills the free space");
        assert_eq!(json["filesystem"], "ext4");
        assert_eq!(json["label"], "scratch");

        // Explicit oversize is refused.
        new_part.size_mib = "9999".to_string();
        assert!(new_part.build(&dev).is_err(), "oversize is refused");

        // New table.
        let new_table = Compose {
            kind: OpKind::NewTable,
            table: PartTable::Gpt,
            ..Compose::default()
        };
        let json = as_json(&new_table.build(&dev).expect("table builds"));
        assert_eq!(json["op"], "create_table");
        assert_eq!(json["table"], "gpt");

        // Delete needs a partition.
        let mut del = Compose {
            kind: OpKind::Delete,
            ..Compose::default()
        };
        assert!(
            del.build(&dev).is_err(),
            "delete without a partition is refused"
        );
        del.partition = "/dev/sdb1".to_string();
        let del_op = del.build(&dev).expect("delete builds");
        assert_eq!(
            del_op,
            StorageOp::DeletePartition {
                partition: "/dev/sdb1".to_string()
            }
        );

        // Format requires a filesystem.
        let mut fmt = Compose {
            kind: OpKind::Format,
            partition: "/dev/sdb1".to_string(),
            fs: None,
            ..Compose::default()
        };
        assert!(fmt.build(&dev).is_err(), "format without a fs is refused");
        fmt.fs = Some(Filesystem::Xfs);
        let json = as_json(&fmt.build(&dev).expect("format builds"));
        assert_eq!(json["op"], "format");
        assert_eq!(json["filesystem"], "xfs");
    }

    #[test]
    fn compose_resize_picks_the_worker_direction_from_the_live_size() {
        let dev = sdb(); // sdb1 is 8192 MiB; 8192 MiB free.
        let as_json = |op: &StorageOp| -> serde_json::Value {
            serde_json::from_str(&serde_json::to_string(op).unwrap_or_default()).unwrap_or_default()
        };
        let mut rz = Compose {
            kind: OpKind::Resize,
            partition: "/dev/sdb1".to_string(),
            size_mib: "12288".to_string(),
            ..Compose::default()
        };
        // Larger than current → the worker's `grow` shape, verbatim.
        let json = as_json(&rz.build(&dev).expect("grow builds"));
        assert_eq!(json["op"], "grow");
        assert_eq!(json["partition"], "/dev/sdb1");
        assert_eq!(json["new_size_mib"], 12288);
        // Smaller than current → `shrink`.
        rz.size_mib = "4096".to_string();
        let json = as_json(&rz.build(&dev).expect("shrink builds"));
        assert_eq!(json["op"], "shrink");
        assert_eq!(json["new_size_mib"], 4096);
        // The same size, an over-growth past free space, and an off-disk
        // partition are refused with typed reasons (the worker re-checks).
        rz.size_mib = "8192".to_string();
        assert!(rz.build(&dev).is_err(), "no-op resize is refused");
        rz.size_mib = "17000".to_string();
        assert!(rz.build(&dev).is_err(), "growth past free space is refused");
        rz.size_mib = "12288".to_string();
        rz.partition = "/dev/sdb9".to_string();
        assert!(rz.build(&dev).is_err(), "an unknown partition is refused");
    }

    #[test]
    fn compose_move_builds_the_worker_shape_and_refuses_a_no_op() {
        let dev = sdb(); // sdb1 starts at 1 MiB.
        let mut mv = Compose {
            kind: OpKind::Move,
            partition: "/dev/sdb1".to_string(),
            new_start_mib: "4096".to_string(),
            ..Compose::default()
        };
        let json: serde_json::Value = serde_json::from_str(
            &serde_json::to_string(&mv.build(&dev).expect("move builds")).unwrap_or_default(),
        )
        .unwrap_or_default();
        assert_eq!(json["op"], "move");
        assert_eq!(json["partition"], "/dev/sdb1");
        assert_eq!(json["new_start_mib"], 4096);
        mv.new_start_mib = "1".to_string();
        assert!(
            mv.build(&dev).is_err(),
            "moving to the current start is a no-op"
        );
    }

    #[test]
    fn geometry_lines_derive_sectors_and_cylinders_from_the_real_size() {
        let dev = sdb(); // 16384 MiB = 33_554_432 × 512 B sectors.
        let [geometry, rollup] = geometry_lines(&dev);
        assert!(geometry.contains("33554432 sectors"), "{geometry}");
        // 17_179_869_184 B / (255 × 63 × 512 B per cylinder) = 2088 full cylinders.
        assert!(geometry.contains("2088 cylinders"), "{geometry}");
        assert!(geometry.contains("derived"), "derived figures say so (§7)");
        assert!(rollup.contains("GPT"), "{rollup}");
        assert!(rollup.contains("1 partition(s)"), "{rollup}");
        assert!(rollup.contains("8192 MiB free of 16384 MiB"), "{rollup}");
    }

    #[test]
    fn armed_apply_request_demands_the_exact_typed_echo() {
        let nodes = project(&[state_body("n", 1, true)]);
        let node = &nodes[0];
        let queue = vec![StorageOp::DeletePartition {
            partition: "/dev/sdb1".to_string(),
        }];
        assert!(
            armed_apply_request(node, &[], "/dev/sdb").is_none(),
            "an empty queue never arms"
        );
        assert!(
            armed_apply_request(node, &queue, "").is_none(),
            "no echo, no request"
        );
        assert!(
            armed_apply_request(node, &queue, "/dev/sda").is_none(),
            "the wrong disk never arms"
        );
        let req = armed_apply_request(node, &queue, "  /dev/sdb  ")
            .expect("the exact echo (whitespace-trimmed) arms");
        let StorageRequest::Apply {
            armed_device,
            queue: q,
            ..
        } = req
        else {
            panic!("an armed request is an Apply");
        };
        assert_eq!(armed_device, "/dev/sdb");
        assert_eq!(q.ops.len(), 1);
    }

    #[test]
    fn queue_target_is_single_disk_or_a_typed_reason() {
        let nodes = project(&[state_body("n", 1, true)]);
        let topo = &nodes[0].topology;

        // Empty → no target.
        assert!(queue_target(&[], topo).is_err());

        // One disk's ops → that disk.
        let ops = vec![
            StorageOp::Format {
                partition: "/dev/sdb1".to_string(),
                filesystem: Filesystem::Ext4,
                label: None,
            },
            StorageOp::CreatePartition {
                device: "/dev/sdb".to_string(),
                start_mib: 0,
                size_mib: 100,
                filesystem: None,
                label: None,
            },
        ];
        assert_eq!(queue_target(&ops, topo).as_deref(), Ok("/dev/sdb"));

        // Spanning two disks → refused (arming is per-disk, mirrors the worker).
        let spanning = vec![
            StorageOp::Unmount {
                partition: "/dev/sda2".to_string(),
            },
            StorageOp::Unmount {
                partition: "/dev/sdb1".to_string(),
            },
        ];
        assert!(
            queue_target(&spanning, topo).is_err(),
            "a multi-disk queue can't be armed"
        );
    }

    #[test]
    fn apply_request_serializes_to_the_worker_verb_shape() {
        let nodes = project(&[state_body("n", 1, true)]);
        let req = StorageRequest::Apply {
            armed_device: "/dev/sdb".to_string(),
            staged: nodes[0].topology.clone(),
            queue: StorageQueue {
                ops: vec![StorageOp::DeletePartition {
                    partition: "/dev/sdb1".to_string(),
                }],
            },
        };
        let v: serde_json::Value = serde_json::from_str(&req.to_body()).unwrap_or_default();
        assert_eq!(v["verb"], "apply");
        assert_eq!(v["armed_device"], "/dev/sdb");
        assert_eq!(v["queue"]["ops"][0]["op"], "delete_partition");
        assert!(
            v["staged"]["devices"].is_array(),
            "the drift baseline rides the verb"
        );

        let refresh: serde_json::Value =
            serde_json::from_str(&StorageRequest::Refresh.to_body()).unwrap_or_default();
        assert_eq!(refresh["verb"], "refresh");
    }

    #[test]
    fn project_progress_keeps_latest_per_op_ordered() {
        let host = "node-a";
        let lane = vec![
            progress_body(host, 0, 2, "unmount", 5, false),
            progress_body(host, 1, 2, "format", 6, true),
            progress_body(host, 0, 2, "unmount", 9, false), // newer for op 0
            progress_body("other", 0, 2, "unmount", 99, false), // wrong host
        ];
        let rows = project_progress(&lane, host);
        assert_eq!(rows.len(), 2, "one row per op index, other host dropped");
        assert_eq!(rows[0].op_index, 0);
        assert_eq!(rows[0].published_at_ms, 9, "the newer op-0 event wins");
        assert!(matches!(rows[1].state, ProgressState::Refused { .. }));
    }

    #[test]
    fn empty_surface_renders_the_honest_state() {
        let mut s = StorageState {
            bus_root: None,
            ..StorageState::default()
        };
        assert!(s.nodes.is_empty());
        assert!(renders(&mut s), "the empty state still fully paints");
    }

    #[test]
    fn live_surface_mounts_and_tessellates() {
        // Feed the projection directly (bypassing the Bus IO) and select the
        // removable data disk so the compose form + queue paths are all reachable,
        // then prove the whole surface tessellates headless — with both View
        // toggles on, so the device rail + geometry detail paint too (MENU-4).
        let mut s = StorageState {
            nodes: project(&[state_body("this-node", 1, true)]),
            local_host: "this-node".to_string(),
            view_rail: true,
            view_geometry: true,
            ..StorageState::default()
        };
        s.ensure_selection();
        s.select_node("this-node");
        s.selected_device = Some("/dev/sdb".to_string());
        s.queue.push(StorageOp::DeletePartition {
            partition: "/dev/sdb1".to_string(),
        });
        s.progress = project_progress(
            &[progress_body(
                "this-node",
                0,
                1,
                "delete_partition",
                3,
                true,
            )],
            "this-node",
        );
        assert!(
            renders(&mut s),
            "the live Storage surface produced no draw primitives"
        );
    }

    #[test]
    fn unavailable_backend_renders_the_typed_not_available_state() {
        let mut s = StorageState {
            nodes: project(&[state_body("n", 1, false)]),
            local_host: "n".to_string(),
            ..StorageState::default()
        };
        s.ensure_selection();
        let node = s.selected().expect("a node is selected");
        assert!(!node.available(), "the backend is unavailable");
        assert!(renders(&mut s), "the unavailable state still fully paints");
    }

    #[test]
    fn cloud_compat_deep_link_resolves_through_the_shell_nav_grammar() {
        // The walled-row deep-link keeps the old `instances` verb for forward
        // compatibility, but QC-15 routes it to the Workbench Cloud plane.
        assert!(matches!(
            crate::toast_bridge::resolve_action(&format!("shell/goto/{CLOUD_COMPAT_SURFACE}")),
            Some(crate::toast_bridge::Navigate::Plane(
                crate::workbench::Plane::Cloud
            ))
        ));
    }

    #[test]
    fn selecting_a_new_peer_clears_the_queue() {
        let mut s = StorageState {
            nodes: project(&[state_body("a", 1, true), state_body("b", 1, true)]),
            ..StorageState::default()
        };
        s.select_node("a");
        s.queue.push(StorageOp::Unmount {
            partition: "/dev/sdb1".to_string(),
        });
        s.arming = "/dev/sdb".to_string();
        s.select_node("b");
        assert!(
            s.queue.is_empty(),
            "switching peers clears the per-node queue"
        );
        assert!(s.arming.is_empty(), "and the arming echo");
        assert_eq!(s.selected_node.as_deref(), Some("b"));
    }
}

/// MENU-6 — the **menubar coverage backstop**: no workspace ships bare again.
///
/// Every routed [`Surface`](crate::dock::Surface) is enumerated against ONE
/// recorded register: it either fronts the shared `MenuBarModel` (with its
/// recorded, non-empty bar title) or sits on the explicit exemption list with the
/// reason + its MENUBAR-SWEEP follow-on. The register's `match` is deliberately
/// exhaustive (no wildcard arm), so adding a `Surface` variant without recording
/// its menubar posture **fails this crate's build** — the enforcement is the
/// compiler, not a reviewer's memory. On top of the register, the surfaces whose
/// states are cheaply constructible are driven through a REAL headless frame and
/// their UPPERCASE bar title asserted in the emitted text shapes, tying the
/// register to rendered reality (the rest are recorded with why a headless
/// construction isn't reachable from a crate-local test).
///
/// This module lives in `storage.rs` because `mde-shell-egui` is a binary-only
/// crate (no lib target): an integration test under `tests/` can't reach the
/// crate's private modules, and a dedicated `src/menubar_coverage.rs` would need
/// a `mod` line in `main.rs` — out of this unit's blast radius. The register is
/// surface-agnostic; only its file placement is a compromise.
#[cfg(test)]
#[allow(clippy::panic)]
mod menubar_coverage {
    use crate::dock::Surface;

    /// The recorded menubar posture of one routed surface.
    enum Coverage {
        /// The surface fronts the shared bar — its recorded title.
        Covered { title: &'static str },
        /// The surface is currently bare — the recorded reason + follow-on.
        Exempt { reason: &'static str },
    }

    /// The ONE recorded decision per routed `Surface` (exhaustive on purpose).
    const fn coverage(surface: Surface) -> Coverage {
        match surface {
            // ── covered: the MENUBAR-ALL / MENUBAR-SWEEP bars ──
            Surface::Workbench => Coverage::Covered {
                title: "State of the Mesh", // MENU-1 (workbench.rs)
            },
            Surface::InfraCode => Coverage::Covered {
                title: "Infra as Code", // IAC-5 (iac.rs)
            },
            Surface::Desktop => Coverage::Covered {
                title: "Desktop", // vdi.rs desktop_menubar, mounted by the shell
            },
            Surface::Browser => Coverage::Covered {
                title: "Browser", // MENU-3 (web.rs, the two-engine bar)
            },
            Surface::Bookmarks => Coverage::Exempt {
                reason: "bare — mde-bookmarks-egui mounts with its own manager \
                         header; folding it onto the shared bar is a MENUBAR-SWEEP \
                         follow-on",
            },
            Surface::Chat => Coverage::Covered {
                title: "Contacts", // MENU-2 (chat.rs)
            },
            Surface::System => Coverage::Covered { title: "System" },
            Surface::Storage => Coverage::Covered {
                title: "Local Cylinders", // MENU-4 (this file)
            },
            Surface::About => Coverage::Covered {
                title: "About", // MENU-5 / DEVMGR (device_manager.rs)
            },
            // ── recorded exemptions: bare today, each a MENUBAR-SWEEP follow-on ──
            Surface::MeshView => Coverage::Exempt {
                reason: "bare — the Mesh Map canvas renders headerless; a bar \
                         (layout toggles, fold source) is a MENUBAR-SWEEP follow-on",
            },
            Surface::Explorer => Coverage::Exempt {
                reason: "bare — the Explorer discovery hero card renders headerless \
                         (filters/actions live on the card itself); a shared bar is \
                         a MENUBAR-SWEEP follow-on",
            },
            Surface::Music => Coverage::Exempt {
                reason: "bare — mde-music-egui mounts with its own header; folding \
                         it onto the shared bar is a MENUBAR-SWEEP follow-on",
            },
            Surface::Media => Coverage::Exempt {
                reason: "bare — mde-media-egui mounts with its own header; folding \
                         it onto the shared bar is a MENUBAR-SWEEP follow-on",
            },
            Surface::Files => Coverage::Exempt {
                reason: "bare — mde-files-egui mounts with its own header; folding \
                         it onto the shared bar is a MENUBAR-SWEEP follow-on",
            },
            Surface::Voice => Coverage::Exempt {
                reason: "bare — mde-voice-egui mounts with its own header; folding \
                         it onto the shared bar is a MENUBAR-SWEEP follow-on",
            },
            Surface::Phones => Coverage::Exempt {
                reason: "bare — the KDC-MESH-9 Phones hub carries its own tab header \
                         (Phones · Files · Commands · Pair); folding it onto the \
                         shared bar is a MENUBAR-SWEEP follow-on",
            },
            Surface::Terminal => Coverage::Exempt {
                reason: "bare — mde-term-egui carries its own tmux/session menu \
                         strip; migrating it onto the shared bar is a MENUBAR-SWEEP \
                         follow-on",
            },
            Surface::Editor => Coverage::Exempt {
                reason: "bare in the shell — mde-editor-egui's Word-97 bar (EDTB-7) \
                         lives inside the editor crate; surfacing it as the shell \
                         bar is a MENUBAR-SWEEP follow-on",
            },
            Surface::Timers => Coverage::Exempt {
                reason: "bare — the clock-cell Timers & Alarms surface (VDOCK-5) is \
                         deliberately chrome-light; a bar is a MENUBAR-SWEEP \
                         follow-on",
            },
        }
    }

    /// Routed operator-reachable views that are NOT `Surface` variants (the
    /// pre-session / overlay screens), inventoried here so the MENU-6 sweep list
    /// is complete. Each is bare today; each entry records why + the follow-on.
    const ROUTED_NON_SURFACE_VIEWS: [(&str, &str); 2] = [
        (
            "explorer/discovery",
            "bare — the Explorer/Discovery flow renders its own headers; folding \
             onto the shared bar is a MENUBAR-SWEEP follow-on",
        ),
        (
            "chooser",
            "bare — the pre-session Desktop Chooser is a full-screen picker with \
             no workspace chrome; a bar is a MENUBAR-SWEEP follow-on",
        ),
    ];

    /// Every routed surface: the picker set plus the clock-cell Timers surface
    /// (deliberately outside `Surface::ALL`, still routed by the dock).
    fn every_routed() -> Vec<Surface> {
        let mut all = Surface::ALL.to_vec();
        all.push(Surface::Timers);
        all
    }

    #[test]
    fn every_routed_surface_records_a_menubar_posture() {
        let mut covered = 0usize;
        let mut exempt = 0usize;
        for surface in every_routed() {
            match coverage(surface) {
                Coverage::Covered { title } => {
                    assert!(
                        !title.trim().is_empty(),
                        "{surface:?}: a covered surface records a non-empty bar title"
                    );
                    covered += 1;
                }
                Coverage::Exempt { reason } => {
                    assert!(
                        reason.contains("MENUBAR-SWEEP"),
                        "{surface:?}: an exemption names its follow-on, not just a shrug"
                    );
                    exempt += 1;
                }
            }
        }
        assert_eq!(covered + exempt, every_routed().len());
        assert_eq!(covered, 8, "the covered set is the eight landed bars");
        for (view, reason) in ROUTED_NON_SURFACE_VIEWS {
            assert!(
                reason.contains("MENUBAR-SWEEP"),
                "{view}: a non-Surface view exemption names its follow-on"
            );
        }
    }

    #[test]
    fn the_bare_inventory_is_exactly_the_recorded_follow_on_set() {
        let bare: Vec<Surface> = every_routed()
            .into_iter()
            .filter(|s| matches!(coverage(*s), Coverage::Exempt { .. }))
            .collect();
        assert_eq!(
            bare,
            [
                Surface::MeshView,
                Surface::Explorer,
                Surface::Music,
                Surface::Media,
                Surface::Files,
                Surface::Voice,
                Surface::Bookmarks,
                Surface::Terminal,
                Surface::Editor,
                Surface::Phones,
                Surface::Timers,
            ],
            "a surface leaving (or joining) the bare set updates this inventory \
             consciously — that's the backstop"
        );
    }

    // ── the register is tied to rendered reality where a crate-local test can ──

    /// Drive one headless frame and collect every text run the surface painted
    /// (the same `Context::run` path the DRM runner drives, minus the GPU).
    fn rendered_text(mut run: impl FnMut(&mut mde_egui::egui::Ui)) -> String {
        use mde_egui::egui;
        fn collect(shape: &egui::epaint::Shape, out: &mut String) {
            match shape {
                egui::epaint::Shape::Text(t) => {
                    out.push_str(t.galley.text());
                    out.push('\n');
                }
                egui::epaint::Shape::Vec(shapes) => {
                    for s in shapes {
                        collect(s, out);
                    }
                }
                _ => {}
            }
        }
        let ctx = egui::Context::default();
        mde_egui::Style::install(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1280.0, 800.0),
            )),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| run(ui));
        });
        let mut text = String::new();
        for clipped in &out.shapes {
            collect(&clipped.shape, &mut text);
        }
        text
    }

    /// The three surfaces whose states construct cheaply from here render for
    /// real, and each bar's UPPERCASE DISPLAY title appears in the painted text —
    /// the register's `Covered` claim proven at the pixel-feed level for them.
    /// The other five covered bars (Workbench / `IaC` / Desktop / Browser / System)
    /// need the shell's full wiring or testkit scaffolding owned by their own
    /// files' tests, so their register rows rest on those files' render tests.
    #[test]
    fn covered_titles_render_on_the_cheaply_constructible_bars() {
        let proofs: [(Surface, fn() -> String); 3] = [
            (Surface::Storage, || {
                let mut s = crate::storage::StorageState {
                    nodes: crate::storage::project(&[crate::storage::state_body("nodeA", 1, true)]),
                    bus_root: None,
                    ..crate::storage::StorageState::default()
                };
                rendered_text(|ui| s.show(ui))
            }),
            (Surface::Chat, || {
                let mut s = crate::chat::ChatState::default();
                rendered_text(|ui| s.show(ui))
            }),
            (Surface::About, || {
                let mut s = crate::device_manager::DeviceManagerState::default();
                rendered_text(|ui| s.show(ui))
            }),
        ];
        for (surface, render) in proofs {
            let Coverage::Covered { title } = coverage(surface) else {
                panic!("{surface:?} is registered Covered");
            };
            let text = render();
            assert!(
                text.contains(&title.to_uppercase()),
                "{surface:?}: the live bar paints \u{201C}{}\u{201D} (register: {title})",
                title.to_uppercase()
            );
        }
    }
}
