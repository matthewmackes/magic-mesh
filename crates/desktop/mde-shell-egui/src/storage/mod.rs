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

mod menubar;

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
mod tests;
