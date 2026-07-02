//! E12-20 — `storage`: the mackesd **storage worker** (`GParted` for the mesh).
//!
//! The privileged half of the Storage plane (`docs/design/workbench-storage-plane.md`).
//! Where the Workbench Storage plane (E12-21) *renders* disks and *submits* typed
//! verbs, this worker **owns and executes** them: a typed [`StorageOp`] pending
//! queue over a live `UDisks2` topology, validated at stage-time (advisory) and
//! apply-time (authoritative), guarded by hard-wall interlocks that a UI bug can't
//! bypass, published to a per-node Bus mirror.
//!
//! ## Shape (mirrors [`super::container`] / [`super::vm_lifecycle`])
//!
//! - **The queue is data.** A [`StorageQueue`] is a `Vec<StorageOp>` (serde). The
//!   UI builds it; the worker [`validate_queue`]s each op against the live
//!   [`Topology`] and — on Apply — runs [`apply_queue`], which halts on the first
//!   failure and reports a typed [`QueueOutcome`] (never a silent partial).
//! - **Injectable seams, headless-testable.** Three traits are the only doors to
//!   the outside: [`UDisks2Client`] (enumerate the block topology — production
//!   [`ZbusUDisks2Client`] over zbus, the §2 FDO-interop exception; no `UDisks2` on
//!   the build host ⇒ typed [`StorageError::Unavailable`]), [`StorageExecutor`]
//!   (apply one op — production [`UDisks2Executor`], whose live parted/udisks2
//!   verb wiring is the later E12-23 slice, so it returns a typed
//!   [`StorageError::IntegrationGated`] today, §7-honest), and the [`Interlocks`]
//!   wall sources ([`ProtectedDevices`] + [`InUseProbe`]). Tests drive fakes, so
//!   the whole pipeline — queue validate/execute/halt + every wall — runs with no
//!   `UDisks2`, no VMM, no root.
//! - **The pure core is fully unit-tested with no bus and no clock:** the op
//!   model plus [`validate_queue`], [`TopologyDrift::diff`] / [`revalidate`], the
//!   wall checks ([`Interlocks::check`]), [`check_arming`], and [`apply_queue`]
//!   over an injected executor.
//!
//! ## Safety model (lock 7 + lock 8)
//!
//! - **Hard walls** live HERE (apply-time authoritative): an op targeting the
//!   node's root/boot/EFI chain, the `/mnt/mesh-storage` backing device, or a
//!   device backing a running VM/container is [`WallRefusal`]-refused (typed, not
//!   a confirm). The protected set is read from `/proc/self/mountinfo`
//!   ([`protected_from_mountinfo`], real); the in-use set is probed from the same
//!   sources the Instances panel uses (virsh/podman) with an **assume-in-use safe
//!   default** ([`InUseStatus::Unknown`]) when the probe can't determine.
//! - **Typed arming** (lock 8): the Apply verb carries the operator-typed device
//!   name; [`check_arming`] refuses on mismatch before a single op runs.
//! - **Stage-vs-apply drift**: the Apply verb carries the topology the queue was
//!   staged against; [`apply_queue`] re-enumerates live and [`revalidate`]s — an
//!   op whose referenced device/partition drifted is [`OpStatus::Invalidated`]
//!   (never applied against a stale picture).
//!
//! ## §2/§6/§9 compliance
//!
//! `UDisks2` is FDO D-Bus (allowed interop, like `BlueZ`/`UPower`/logind). This worker
//! is platform-services — no desktop dep (the layered-tiers gate). Remote parity
//! (E12-9 lock) is inherent: the verbs are per-node topics and the walls are in
//! THIS executor, so a queue staged from peer A against peer B hits B's walls.

#![cfg(feature = "async-services")]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mde_bus::persist::Persist;
use thiserror::Error;

use crate::workers::proc::{output_with_timeout, DEFAULT_CMD_TIMEOUT};

use super::fs_tools::{
    self, CapabilityRefusal, FsToolRunner, LiveFsTools, ResizeDirection, ResizeTarget,
};
use super::{ShutdownToken, Worker};

// ───────────────────────────── topics ─────────────────────────────

/// The per-node action topic verbs are drained from: `action/storage/<node>`.
#[must_use]
pub fn action_topic(node: &str) -> String {
    format!("action/storage/{node}")
}

/// The per-node topology mirror topic: `state/storage/<node>`.
#[must_use]
pub fn state_topic(node: &str) -> String {
    format!("state/storage/{node}")
}

/// The per-node per-op apply-progress topic: `event/storage/<node>/progress`.
#[must_use]
pub fn progress_topic(node: &str) -> String {
    format!("event/storage/{node}/progress")
}

/// Action-drain cadence. The Bus read is a cheap local log scan; storage ops are
/// slow, operator-visible events, so a 2 s poll is plenty responsive.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Slow heartbeat for the topology mirror republish (between action-triggered
/// republishes) — keeps the `UDisks2` enumerate off the hot path.
pub const PUBLISH_HEARTBEAT: Duration = Duration::from_secs(30);

// ───────────────────────────── op model ─────────────────────────────

/// A partition-table scheme (lock 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartitionTable {
    /// GUID Partition Table.
    Gpt,
    /// Master Boot Record (msdos).
    Mbr,
}

impl PartitionTable {
    /// The canonical parted/udisks table id (`gpt` / `dos`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Gpt => "gpt",
            Self::Mbr => "dos",
        }
    }

    /// Parse a udisks `PartitionTable.Type` string (`gpt` / `dos`/`mbr`).
    #[must_use]
    pub fn from_udisks(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "gpt" => Some(Self::Gpt),
            "dos" | "mbr" | "msdos" => Some(Self::Mbr),
            _ => None,
        }
    }
}

/// A filesystem/format target (lock 6).
///
/// The typed op set carries all of them now; the live per-fs tooling depth (btrfs
/// subvolumes, LUKS choreography, ntfs) is the E12-23 slice — this worker validates +
/// stages every kind and executes through the injectable [`StorageExecutor`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Filesystem {
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
    /// A LUKS container (format-inside is a follow-on op, lock 6).
    Luks,
}

impl Filesystem {
    /// The canonical mkfs/udisks filesystem id.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
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

    /// Parse the filesystem id `UDisks`/`blkid` reports (`Block.IdType`) into the
    /// typed [`Filesystem`], so a resize/subvolume op can resolve the target's
    /// current fs from the live topology. `None` for an id the plane doesn't manage
    /// (the caller treats that as "unknown filesystem" — a data-loss-safe refusal,
    /// never a guess).
    #[must_use]
    pub fn from_id(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "ext2" | "ext3" | "ext4" => Some(Self::Ext4),
            "xfs" => Some(Self::Xfs),
            "vfat" | "fat" | "fat16" | "fat32" | "msdos" => Some(Self::Vfat),
            "exfat" => Some(Self::Exfat),
            "btrfs" => Some(Self::Btrfs),
            "ntfs" => Some(Self::Ntfs),
            "swap" | "swsuspend" => Some(Self::Swap),
            "crypto_luks" | "luks" => Some(Self::Luks),
            _ => None,
        }
    }
}

/// One typed storage operation — the queue element (lock 5, `GParted` parity). Sizes
/// and offsets are in **MiB** (the queue's alignment granularity); the executor
/// resolves them to bytes/sectors.
///
/// Internally tagged on `op` so the JSON the UI publishes is self-describing, e.g.
/// `{"op":"create_table","device":"/dev/sdb","table":"gpt"}`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum StorageOp {
    /// Write a fresh partition table to a whole disk (destroys existing layout).
    CreateTable {
        /// The whole-disk device (`/dev/sdb`).
        device: String,
        /// The table scheme to write.
        table: PartitionTable,
    },
    /// Create a partition in free space on a disk, optionally formatted + labelled.
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
    /// Delete an existing partition (`/dev/sdb1`).
    DeletePartition {
        /// The partition device.
        partition: String,
    },
    /// (Re)format an existing partition to `filesystem`.
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
    /// Set a partition's flags (`boot`, `esp`, `hidden`, …).
    SetFlags {
        /// The partition device.
        partition: String,
        /// The flag set to apply.
        flags: Vec<String>,
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
    /// Grow a partition (+ its filesystem) to `new_size_mib`.
    Grow {
        /// The partition device.
        partition: String,
        /// The target size (MiB), larger than current.
        new_size_mib: u64,
    },
    /// Shrink a partition (+ its filesystem) to `new_size_mib`.
    Shrink {
        /// The partition device.
        partition: String,
        /// The target size (MiB), smaller than current.
        new_size_mib: u64,
    },
    /// Move a partition to a new start offset (rewrites data — slow).
    Move {
        /// The partition device.
        partition: String,
        /// The new start offset (MiB from the disk head).
        new_start_mib: u64,
    },
    /// LUKS-format a partition into an encrypted container, optionally opening it and
    /// making an inner filesystem — **one staged op** (lock 6). The keyfile is a
    /// path the operator provisioned; an interactive passphrase is never carried on
    /// the Bus (the executor integration-gates a keyless format).
    LuksFormat {
        /// The partition device to encrypt.
        partition: String,
        /// The mapper name to open the container as (for the inner-fs step).
        mapper_name: String,
        /// The inner filesystem to create inside the opened container, if any.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        inner_filesystem: Option<Filesystem>,
        /// The keyfile path (provisioned out-of-band — never the passphrase itself).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        keyfile: Option<PathBuf>,
        /// Optional label for the inner filesystem.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
    /// Unlock (open) an existing LUKS partition as `/dev/mapper/<mapper_name>`.
    LuksOpen {
        /// The LUKS partition device.
        partition: String,
        /// The mapper name to open as.
        mapper_name: String,
        /// The keyfile path (never the passphrase).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        keyfile: Option<PathBuf>,
    },
    /// Lock (close) an open LUKS container. Carries its backing `partition` so the
    /// wall + arming resolve the whole disk uniformly.
    LuksClose {
        /// The LUKS partition device backing the mapper.
        partition: String,
        /// The open mapper name to close.
        mapper_name: String,
    },
    /// Create a btrfs subvolume under the partition's mount point (lock 6).
    SubvolumeCreate {
        /// The (mounted) btrfs partition.
        partition: String,
        /// The subvolume name (relative to the mount point).
        name: String,
    },
    /// Delete a btrfs subvolume.
    SubvolumeDelete {
        /// The (mounted) btrfs partition.
        partition: String,
        /// The subvolume name to delete.
        name: String,
    },
    /// Snapshot a btrfs subvolume (`readonly` ⇒ a `-r` snapshot).
    SubvolumeSnapshot {
        /// The (mounted) btrfs partition.
        partition: String,
        /// The source subvolume name.
        source: String,
        /// The destination snapshot name.
        dest: String,
        /// Whether the snapshot is read-only.
        #[serde(default)]
        readonly: bool,
    },
}

impl StorageOp {
    /// The whole-disk `device` field for the device-scoped ops, else `None`
    /// (partition-scoped ops resolve their disk via the topology).
    #[must_use]
    pub fn device_field(&self) -> Option<&str> {
        match self {
            Self::CreateTable { device, .. } | Self::CreatePartition { device, .. } => {
                Some(device.as_str())
            }
            _ => None,
        }
    }

    /// The `partition` device this op targets, for the partition-scoped ops.
    #[must_use]
    pub fn partition(&self) -> Option<&str> {
        match self {
            Self::DeletePartition { partition }
            | Self::Format { partition, .. }
            | Self::SetLabel { partition, .. }
            | Self::SetFlags { partition, .. }
            | Self::Mount { partition, .. }
            | Self::Unmount { partition }
            | Self::Grow { partition, .. }
            | Self::Shrink { partition, .. }
            | Self::Move { partition, .. }
            | Self::LuksFormat { partition, .. }
            | Self::LuksOpen { partition, .. }
            | Self::LuksClose { partition, .. }
            | Self::SubvolumeCreate { partition, .. }
            | Self::SubvolumeDelete { partition, .. }
            | Self::SubvolumeSnapshot { partition, .. } => Some(partition.as_str()),
            Self::CreateTable { .. } | Self::CreatePartition { .. } => None,
        }
    }

    /// The whole-disk device this op ultimately touches — the wall + arming key.
    /// For a device-scoped op it's the `device`; for a partition-scoped op it's the
    /// partition's parent disk resolved from `topo`. `None` when a partition-scoped
    /// op names a partition absent from the topology (validation reports it).
    #[must_use]
    pub fn resolve_device(&self, topo: &Topology) -> Option<String> {
        if let Some(d) = self.device_field() {
            return Some(d.to_string());
        }
        self.partition()
            .and_then(|p| topo.parent_disk_of(p))
            .map(|d| d.name.clone())
    }

    /// A short human label for progress/logging (the verb, not the params).
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::CreateTable { .. } => "create_table",
            Self::CreatePartition { .. } => "create_partition",
            Self::DeletePartition { .. } => "delete_partition",
            Self::Format { .. } => "format",
            Self::SetLabel { .. } => "set_label",
            Self::SetFlags { .. } => "set_flags",
            Self::Mount { .. } => "mount",
            Self::Unmount { .. } => "unmount",
            Self::Grow { .. } => "grow",
            Self::Shrink { .. } => "shrink",
            Self::Move { .. } => "move",
            Self::LuksFormat { .. } => "luks_format",
            Self::LuksOpen { .. } => "luks_open",
            Self::LuksClose { .. } => "luks_close",
            Self::SubvolumeCreate { .. } => "subvolume_create",
            Self::SubvolumeDelete { .. } => "subvolume_delete",
            Self::SubvolumeSnapshot { .. } => "subvolume_snapshot",
        }
    }
}

/// The staged pending-operations queue — a typed `Vec<StorageOp>` (serde). The UI
/// builds it; the worker validates + executes it.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StorageQueue {
    /// The staged ops, applied in order.
    pub ops: Vec<StorageOp>,
}

impl StorageQueue {
    /// A queue from an op vec.
    #[must_use]
    pub const fn new(ops: Vec<StorageOp>) -> Self {
        Self { ops }
    }

    /// Whether the queue has no ops.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    /// The set of whole-disk devices this queue targets, resolved against `topo`
    /// (a partition-scoped op whose partition is unknown contributes nothing —
    /// validation reports it).
    #[must_use]
    pub fn target_devices(&self, topo: &Topology) -> BTreeSet<String> {
        self.ops
            .iter()
            .filter_map(|op| op.resolve_device(topo))
            .collect()
    }
}

// ───────────────────────────── topology ─────────────────────────────

/// One partition on a disk — the live `UDisks2` record.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Partition {
    /// The partition device (`/dev/sdb1`).
    pub name: String,
    /// 1-based partition number.
    pub number: u32,
    /// Start offset (MiB from the disk head).
    pub start_mib: u64,
    /// Size (MiB).
    pub size_mib: u64,
    /// The filesystem id `UDisks` reports (`ext4`, `crypto_LUKS`, …), if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filesystem: Option<String>,
    /// The filesystem label, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The current mount point, if mounted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mountpoint: Option<String>,
    /// The filesystem UUID, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uuid: Option<String>,
}

impl Partition {
    /// Whether this partition is currently mounted.
    #[must_use]
    pub const fn is_mounted(&self) -> bool {
        self.mountpoint.is_some()
    }
}

/// One whole-disk block device with its partition layout.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BlockDevice {
    /// The whole-disk device (`/dev/sdb`).
    pub name: String,
    /// Total size (MiB).
    pub size_mib: u64,
    /// The partition table scheme, if the disk has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table: Option<PartitionTable>,
    /// Whether the drive is removable (USB stick, SD card).
    #[serde(default)]
    pub removable: bool,
    /// The partitions, in on-disk order.
    #[serde(default)]
    pub partitions: Vec<Partition>,
}

impl BlockDevice {
    /// The free (unpartitioned) space on the disk (MiB) — total minus the sum of
    /// partition sizes. A coarse, MiB-granular advisory figure (it ignores
    /// alignment gaps + table metadata); the executor does the authoritative
    /// sector math.
    #[must_use]
    pub fn free_mib(&self) -> u64 {
        let used: u64 = self.partitions.iter().map(|p| p.size_mib).sum();
        self.size_mib.saturating_sub(used)
    }

    /// The partition with device `name`, if present.
    #[must_use]
    pub fn partition(&self, name: &str) -> Option<&Partition> {
        self.partitions.iter().find(|p| p.name == name)
    }
}

/// The live block-device topology — the model the mirror publishes and the queue validates against.
///
/// No timestamp (so a stage-vs-apply [`TopologyDrift::diff`] compares layout, not wall-
/// clock); the mirror wraps it with `published_at_ms`.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Topology {
    /// The whole-disk devices.
    pub devices: Vec<BlockDevice>,
}

impl Topology {
    /// A topology from a device vec.
    #[must_use]
    pub const fn new(devices: Vec<BlockDevice>) -> Self {
        Self { devices }
    }

    /// The whole-disk device named `name`, if present.
    #[must_use]
    pub fn device(&self, name: &str) -> Option<&BlockDevice> {
        self.devices.iter().find(|d| d.name == name)
    }

    /// The disk that owns partition `name`, if present.
    #[must_use]
    pub fn parent_disk_of(&self, partition: &str) -> Option<&BlockDevice> {
        self.devices
            .iter()
            .find(|d| d.partition(partition).is_some())
    }

    /// The partition named `name` (searching every disk), if present.
    #[must_use]
    pub fn partition(&self, name: &str) -> Option<&Partition> {
        self.devices.iter().find_map(|d| d.partition(name))
    }
}

// ───────────────────────────── validation ─────────────────────────────

/// A typed reason an op is invalid against a topology (advisory at stage-time,
/// authoritative at apply-time). The `Drift*` variants are the stage-vs-apply
/// revalidation refusals.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum OpInvalid {
    /// The named whole-disk device is absent from the topology.
    #[error("unknown device {device}")]
    UnknownDevice {
        /// The missing device.
        device: String,
    },
    /// The named partition is absent from the topology.
    #[error("unknown partition {partition}")]
    UnknownPartition {
        /// The missing partition.
        partition: String,
    },
    /// A create-partition on a disk with no partition table.
    #[error("device {device} has no partition table")]
    NoPartitionTable {
        /// The tableless device.
        device: String,
    },
    /// Not enough free space on the disk for the requested size.
    #[error("device {device}: need {need_mib} MiB, only {free_mib} MiB free")]
    NotEnoughSpace {
        /// The disk.
        device: String,
        /// The requested size.
        need_mib: u64,
        /// The free space available.
        free_mib: u64,
    },
    /// A destructive/altering op on a mounted partition (unmount first).
    #[error("partition {partition} is mounted at {mountpoint} — unmount first")]
    PartitionMounted {
        /// The mounted partition.
        partition: String,
        /// Where it's mounted.
        mountpoint: String,
    },
    /// A mount op on an already-mounted partition.
    #[error("partition {partition} is already mounted at {mountpoint}")]
    AlreadyMounted {
        /// The partition.
        partition: String,
        /// Where it's mounted.
        mountpoint: String,
    },
    /// An unmount op on a partition that isn't mounted.
    #[error("partition {partition} is not mounted")]
    NotMounted {
        /// The partition.
        partition: String,
    },
    /// An op that needs a specific filesystem (a subvolume op needs btrfs) on a
    /// partition carrying a different one.
    #[error("partition {partition} is {found}, not {need} — op needs {need}")]
    WrongFilesystem {
        /// The partition.
        partition: String,
        /// The filesystem the op requires.
        need: String,
        /// The filesystem the partition actually carries.
        found: String,
    },
    /// A resize whose target size doesn't move in the requested direction, or is
    /// otherwise out of range.
    #[error("partition {partition}: invalid resize to {new_size_mib} MiB (current {current_mib})")]
    InvalidResize {
        /// The partition.
        partition: String,
        /// The requested new size.
        new_size_mib: u64,
        /// The current size.
        current_mib: u64,
    },
    /// The op references an entity that changed between stage-time and apply-time
    /// (device unplugged, partition mounted/resized) — never applied against a
    /// stale picture.
    #[error("topology drifted since staging: {detail}")]
    Drifted {
        /// Which referenced entity drifted (added/removed/changed).
        detail: String,
    },
}

/// Validate one op against `topo` (advisory at stage, authoritative at apply). No
/// I/O — pure.
///
/// # Errors
/// An [`OpInvalid`] describing the first failed precondition.
pub fn validate_op(op: &StorageOp, topo: &Topology) -> Result<(), OpInvalid> {
    match op {
        StorageOp::CreateTable { device, .. } => {
            require_device(topo, device)?;
            Ok(())
        }
        StorageOp::CreatePartition {
            device,
            size_mib,
            start_mib,
            ..
        } => {
            let disk = require_device(topo, device)?;
            if disk.table.is_none() {
                return Err(OpInvalid::NoPartitionTable {
                    device: device.clone(),
                });
            }
            let _ = start_mib; // start offset is executor-validated at sector level
            let free = disk.free_mib();
            // Advisory MiB math (the executor does the authoritative sector math).
            if *size_mib > free {
                return Err(OpInvalid::NotEnoughSpace {
                    device: device.clone(),
                    need_mib: *size_mib,
                    free_mib: free,
                });
            }
            Ok(())
        }
        // Destructive/relocating ops (+ a LUKS-format, which erases contents) must be
        // unmounted.
        StorageOp::DeletePartition { partition }
        | StorageOp::Format { partition, .. }
        | StorageOp::Move { partition, .. }
        | StorageOp::LuksFormat { partition, .. } => {
            let p = require_partition(topo, partition)?;
            require_unmounted(p)?;
            Ok(())
        }
        // Ops that only need the partition to exist (label/flags + LUKS open/close,
        // which address the container itself with no mount precondition).
        StorageOp::SetLabel { partition, .. }
        | StorageOp::SetFlags { partition, .. }
        | StorageOp::LuksOpen { partition, .. }
        | StorageOp::LuksClose { partition, .. } => {
            require_partition(topo, partition)?;
            Ok(())
        }
        StorageOp::Mount { partition, .. } => {
            let p = require_partition(topo, partition)?;
            if let Some(mp) = &p.mountpoint {
                return Err(OpInvalid::AlreadyMounted {
                    partition: partition.clone(),
                    mountpoint: mp.clone(),
                });
            }
            Ok(())
        }
        StorageOp::Unmount { partition } => {
            let p = require_partition(topo, partition)?;
            if p.mountpoint.is_none() {
                return Err(OpInvalid::NotMounted {
                    partition: partition.clone(),
                });
            }
            Ok(())
        }
        StorageOp::Grow {
            partition,
            new_size_mib,
        } => validate_grow(topo, partition, *new_size_mib),
        StorageOp::Shrink {
            partition,
            new_size_mib,
        } => validate_shrink(topo, partition, *new_size_mib),
        // Subvolume ops need a mounted btrfs (the tool operates on the mount point).
        StorageOp::SubvolumeCreate { partition, .. }
        | StorageOp::SubvolumeDelete { partition, .. }
        | StorageOp::SubvolumeSnapshot { partition, .. } => validate_subvolume(topo, partition),
    }
}

/// Validate a `Grow`: the target must move up and fit the disk's free space.
fn validate_grow(topo: &Topology, partition: &str, new_size_mib: u64) -> Result<(), OpInvalid> {
    let p = require_partition(topo, partition)?;
    if new_size_mib <= p.size_mib {
        return Err(OpInvalid::InvalidResize {
            partition: partition.to_string(),
            new_size_mib,
            current_mib: p.size_mib,
        });
    }
    if let Some(disk) = topo.parent_disk_of(partition) {
        let grow_by = new_size_mib - p.size_mib;
        if grow_by > disk.free_mib() {
            return Err(OpInvalid::NotEnoughSpace {
                device: disk.name.clone(),
                need_mib: grow_by,
                free_mib: disk.free_mib(),
            });
        }
    }
    Ok(())
}

/// Validate a `Shrink`: offline (lock 4 choreography) — must be unmounted and the
/// target strictly smaller than the current size but non-zero.
fn validate_shrink(topo: &Topology, partition: &str, new_size_mib: u64) -> Result<(), OpInvalid> {
    let p = require_partition(topo, partition)?;
    require_unmounted(p)?;
    if new_size_mib == 0 || new_size_mib >= p.size_mib {
        return Err(OpInvalid::InvalidResize {
            partition: partition.to_string(),
            new_size_mib,
            current_mib: p.size_mib,
        });
    }
    Ok(())
}

/// Validate a btrfs subvolume op: the partition must be a mounted btrfs.
fn validate_subvolume(topo: &Topology, partition: &str) -> Result<(), OpInvalid> {
    let p = require_partition(topo, partition)?;
    require_btrfs(p)?;
    if p.mountpoint.is_none() {
        return Err(OpInvalid::NotMounted {
            partition: partition.to_string(),
        });
    }
    Ok(())
}

/// Require that a partition carries a btrfs filesystem (subvolume ops) — a typed
/// [`OpInvalid::WrongFilesystem`] otherwise (never a guess).
fn require_btrfs(p: &Partition) -> Result<(), OpInvalid> {
    let found = p.filesystem.as_deref().unwrap_or("unknown");
    if Filesystem::from_id(found) == Some(Filesystem::Btrfs) {
        Ok(())
    } else {
        Err(OpInvalid::WrongFilesystem {
            partition: p.name.clone(),
            need: "btrfs".to_string(),
            found: found.to_string(),
        })
    }
}

fn require_device<'a>(topo: &'a Topology, device: &str) -> Result<&'a BlockDevice, OpInvalid> {
    topo.device(device).ok_or_else(|| OpInvalid::UnknownDevice {
        device: device.to_string(),
    })
}

fn require_partition<'a>(topo: &'a Topology, partition: &str) -> Result<&'a Partition, OpInvalid> {
    topo.partition(partition)
        .ok_or_else(|| OpInvalid::UnknownPartition {
            partition: partition.to_string(),
        })
}

fn require_unmounted(p: &Partition) -> Result<(), OpInvalid> {
    p.mountpoint.as_ref().map_or(Ok(()), |mp| {
        Err(OpInvalid::PartitionMounted {
            partition: p.name.clone(),
            mountpoint: mp.clone(),
        })
    })
}

/// Validate every op in `queue` against `topo` (stage-time advisory). Returns the
/// per-op result parallel to `queue.ops`; the UI surfaces the invalid rows before
/// Apply. Pure.
#[must_use]
pub fn validate_queue(queue: &StorageQueue, topo: &Topology) -> Vec<Result<(), OpInvalid>> {
    queue.ops.iter().map(|op| validate_op(op, topo)).collect()
}

// ───────────────────────────── stage-vs-apply drift ─────────────────────────────

/// A typed diff between the topology a queue was staged against and the live
/// topology at apply-time — the stage-vs-apply revalidation input.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TopologyDrift {
    /// Devices present live but not at staging.
    pub added_devices: BTreeSet<String>,
    /// Devices present at staging but gone live (unplugged).
    pub removed_devices: BTreeSet<String>,
    /// Partitions present live but not at staging.
    pub added_partitions: BTreeSet<String>,
    /// Partitions present at staging but gone live.
    pub removed_partitions: BTreeSet<String>,
    /// Partitions present in both but whose record changed (size, mount, fs…).
    pub changed_partitions: BTreeSet<String>,
}

impl TopologyDrift {
    /// Whether nothing drifted between `staged` and `live`.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.added_devices.is_empty()
            && self.removed_devices.is_empty()
            && self.added_partitions.is_empty()
            && self.removed_partitions.is_empty()
            && self.changed_partitions.is_empty()
    }

    /// Compute the drift between the `staged` and `live` topologies. Pure.
    #[must_use]
    pub fn diff(staged: &Topology, live: &Topology) -> Self {
        let staged_devs: BTreeSet<&str> = staged.devices.iter().map(|d| d.name.as_str()).collect();
        let live_devs: BTreeSet<&str> = live.devices.iter().map(|d| d.name.as_str()).collect();

        let staged_parts: BTreeMap<&str, &Partition> = staged
            .devices
            .iter()
            .flat_map(|d| d.partitions.iter())
            .map(|p| (p.name.as_str(), p))
            .collect();
        let live_parts: BTreeMap<&str, &Partition> = live
            .devices
            .iter()
            .flat_map(|d| d.partitions.iter())
            .map(|p| (p.name.as_str(), p))
            .collect();

        let mut drift = Self::default();
        for d in live_devs.difference(&staged_devs) {
            drift.added_devices.insert((*d).to_string());
        }
        for d in staged_devs.difference(&live_devs) {
            drift.removed_devices.insert((*d).to_string());
        }
        for (name, sp) in &staged_parts {
            match live_parts.get(name) {
                None => {
                    drift.removed_partitions.insert((*name).to_string());
                }
                Some(lp) if lp != sp => {
                    drift.changed_partitions.insert((*name).to_string());
                }
                Some(_) => {}
            }
        }
        for name in live_parts.keys() {
            if !staged_parts.contains_key(name) {
                drift.added_partitions.insert((*name).to_string());
            }
        }
        drift
    }

    /// Whether the entity `op` references drifted — the device it targets was
    /// added/removed, or its partition was added/removed/changed.
    #[must_use]
    fn affects(&self, op: &StorageOp, staged: &Topology) -> Option<String> {
        // A device-scoped op is invalidated when its disk was added/removed.
        if let Some(dev) = op.device_field() {
            if self.removed_devices.contains(dev) {
                return Some(format!("device {dev} was unplugged"));
            }
            if self.added_devices.contains(dev) {
                return Some(format!("device {dev} appeared after staging"));
            }
        }
        // A partition-scoped op is invalidated when its partition (or its parent
        // disk) drifted.
        if let Some(part) = op.partition() {
            if self.removed_partitions.contains(part) {
                return Some(format!("partition {part} disappeared"));
            }
            if self.changed_partitions.contains(part) {
                return Some(format!("partition {part} changed on disk"));
            }
            if let Some(disk) = staged.parent_disk_of(part) {
                if self.removed_devices.contains(&disk.name) {
                    return Some(format!("disk {} backing {part} was unplugged", disk.name));
                }
            }
        }
        None
    }
}

/// Revalidate a staged queue against the live topology (the stage-vs-apply gate).
///
/// Returns, per op, `Some(OpInvalid::Drifted)` when the entity it references drifted since
/// staging — the affected ops are invalidated, never applied against a stale picture. Pure.
#[must_use]
pub fn revalidate(
    queue: &StorageQueue,
    staged: &Topology,
    live: &Topology,
) -> Vec<Option<OpInvalid>> {
    let drift = TopologyDrift::diff(staged, live);
    queue
        .ops
        .iter()
        .map(|op| {
            drift
                .affects(op, staged)
                .map(|detail| OpInvalid::Drifted { detail })
        })
        .collect()
}

// ───────────────────────────── hard walls (lock 7) ─────────────────────────────

/// Why a whole-disk device is a **protected** system device (lock 7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtectedReason {
    /// It backs the node's root / `/boot` / EFI chain (the bootc host disk).
    RootBootEfi,
    /// It backs `/mnt/mesh-storage` (the mesh shared-storage volume).
    MeshStorageBacker,
}

impl std::fmt::Display for ProtectedReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RootBootEfi => f.write_str("backs the node's root/boot/EFI chain"),
            Self::MeshStorageBacker => f.write_str("backs /mnt/mesh-storage"),
        }
    }
}

/// The live in-use status of a whole-disk device (backing a running compute unit).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InUseStatus {
    /// Not backing any running VM/container.
    Free,
    /// Backs a running VM (its image/device is attached).
    InUseByVm(String),
    /// Backs a running container (a volume/mount).
    InUseByContainer(String),
    /// The probe could not determine in-use state (no virsh/podman) — treated as
    /// **in-use** by the wall (the assume-in-use safe default, lock 7).
    Unknown,
}

/// The typed refusal a wall raises (lock 7 — a refusal, never a confirm).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum WallRefusal {
    /// The device is a protected system device.
    #[error("refused: {device} {reason}")]
    Protected {
        /// The device.
        device: String,
        /// Why it's protected.
        reason: ProtectedReason,
    },
    /// The device backs a running VM (deep-link: shut it down in Instances).
    #[error("refused: {device} backs running VM {vm}")]
    InUseByVm {
        /// The device.
        device: String,
        /// The VM name/UUID.
        vm: String,
    },
    /// The device backs a running container.
    #[error("refused: {device} backs running container {container}")]
    InUseByContainer {
        /// The device.
        device: String,
        /// The container name.
        container: String,
    },
    /// In-use state couldn't be verified — refused under the assume-in-use safe
    /// default.
    #[error("refused: cannot verify {device} is free (no virsh/podman) — assuming in-use")]
    InUseUnknown {
        /// The device.
        device: String,
    },
}

/// The set of whole-disk devices protected as the node's root/boot/EFI + the
/// mesh-storage backer. Production [`MountProtectedDevices`] reads
/// `/proc/self/mountinfo`; tests inject a fixed map.
pub trait ProtectedDevices: Send + Sync {
    /// The protected whole-disk devices → the reason each is protected.
    fn protected(&self) -> BTreeMap<String, ProtectedReason>;
}

/// The "does a running VM/container back this device" probe. Production
/// [`ComputeInUseProbe`] queries virsh/podman (the same sources the Instances
/// panel uses); tests inject a snapshot.
pub trait InUseProbe: Send + Sync {
    /// The in-use status of whole-disk device `device`.
    fn status(&self, device: &str) -> InUseStatus;
}

/// The apply-time hard walls (lock 7), bundled over the two injectable sources.
///
/// [`Interlocks::check`] is where a UI bug can't reach — every op runs through it before
/// the executor is called, locally and for remote-staged queues.
pub struct Interlocks {
    protected: Arc<dyn ProtectedDevices>,
    in_use: Arc<dyn InUseProbe>,
}

impl Interlocks {
    /// Construct over the two seams.
    #[must_use]
    pub fn new(protected: Arc<dyn ProtectedDevices>, in_use: Arc<dyn InUseProbe>) -> Self {
        Self { protected, in_use }
    }

    /// The production interlocks: `/proc/self/mountinfo` protected set + the
    /// virsh/podman in-use probe.
    #[must_use]
    pub fn production() -> Self {
        Self::new(
            Arc::new(MountProtectedDevices::new()),
            Arc::new(ComputeInUseProbe::new()),
        )
    }

    /// Check whole-disk `device` against every wall. `Ok(())` ⇒ the op may run.
    ///
    /// # Errors
    /// A typed [`WallRefusal`] (protected chain / in-use VM / in-use container /
    /// unverifiable-so-assume-in-use). Protected takes precedence over in-use.
    pub fn check(&self, device: &str) -> Result<(), WallRefusal> {
        if let Some(reason) = self.protected().get(device) {
            return Err(WallRefusal::Protected {
                device: device.to_string(),
                reason: *reason,
            });
        }
        match self.in_use.status(device) {
            InUseStatus::Free => Ok(()),
            InUseStatus::InUseByVm(vm) => Err(WallRefusal::InUseByVm {
                device: device.to_string(),
                vm,
            }),
            InUseStatus::InUseByContainer(container) => Err(WallRefusal::InUseByContainer {
                device: device.to_string(),
                container,
            }),
            InUseStatus::Unknown => Err(WallRefusal::InUseUnknown {
                device: device.to_string(),
            }),
        }
    }

    /// The protected whole-disk device set (memoized per-call from the source).
    #[must_use]
    pub fn protected(&self) -> BTreeMap<String, ProtectedReason> {
        self.protected.protected()
    }
}

/// Reduce a partition (or whole-disk) device to its **parent whole disk**: `/dev/sda3` → `/dev/sda`, `/dev/nvme0n1p2` → `/dev/nvme0n1`, `/dev/mmcblk0p1` → `/dev/mmcblk0`.
///
/// A device with no trailing partition suffix (already a whole disk) is returned unchanged.
/// Pure.
#[must_use]
pub fn parent_disk(dev: &str) -> String {
    let base = dev.rsplit('/').next().unwrap_or(dev);
    let prefix = &dev[..dev.len() - base.len()];
    // Device families whose DISK names embed digits (`nvme0n1`, `mmcblk0`,
    // `loop0`, `md0`, `nbd0`) delimit a partition with `p<N>`: a whole disk has no
    // trailing `p<digits>`, so it's returned unchanged (this is why a plain digit
    // suffix like the `1` in `nvme0n1` must NOT be stripped). Classic
    // `sd`/`vd`/`hd`/`xvd` disks append the partition number directly.
    let p_separated = ["nvme", "mmcblk", "loop", "nbd", "md"]
        .iter()
        .any(|f| base.starts_with(f));
    let parent_base = if p_separated {
        base.rfind('p').map_or(base, |idx| {
            let (head, tail) = base.split_at(idx);
            let is_part = tail.len() > 1
                && tail[1..].bytes().all(|b| b.is_ascii_digit())
                && head.bytes().last().is_some_and(|b| b.is_ascii_digit());
            if is_part {
                head
            } else {
                base
            }
        })
    } else {
        base.trim_end_matches(|c: char| c.is_ascii_digit())
    };
    format!("{prefix}{parent_base}")
}

/// Production [`ProtectedDevices`]: read `/proc/self/mountinfo`, find the source
/// devices behind `/`, `/boot`, `/boot/efi`, `/efi` and `/mnt/mesh-storage`, and
/// reduce each to its parent whole disk.
#[derive(Debug, Clone, Default)]
pub struct MountProtectedDevices {
    /// Override the mountinfo path (tests). `None` ⇒ `/proc/self/mountinfo`.
    mountinfo_path: Option<PathBuf>,
}

impl MountProtectedDevices {
    /// The production probe (reads `/proc/self/mountinfo`).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            mountinfo_path: None,
        }
    }

    /// Read the protected set from a mountinfo path override (tests).
    #[must_use]
    pub fn with_mountinfo_path(mut self, path: PathBuf) -> Self {
        self.mountinfo_path = Some(path);
        self
    }
}

impl ProtectedDevices for MountProtectedDevices {
    fn protected(&self) -> BTreeMap<String, ProtectedReason> {
        let path = self
            .mountinfo_path
            .clone()
            .unwrap_or_else(|| PathBuf::from("/proc/self/mountinfo"));
        // Fail-safe: if we can't read the mount table we protect nothing here, but
        // the in-use probe's Unknown default still refuses unverifiable devices, so
        // a create/format never lands on an unclassified disk.
        std::fs::read_to_string(&path)
            .map_or_else(|_| BTreeMap::new(), |text| protected_from_mountinfo(&text))
    }
}

/// Parse `/proc/self/mountinfo` into the protected whole-disk set.
///
/// The root/boot/ EFI mounts map to [`ProtectedReason::RootBootEfi`]; `/mnt/mesh-storage`
/// maps to [`ProtectedReason::MeshStorageBacker`]. Each mount source is reduced to its
/// parent whole disk ([`parent_disk`]). Only real block-device sources (`/dev/…`) are
/// recorded. Pure.
#[must_use]
pub fn protected_from_mountinfo(text: &str) -> BTreeMap<String, ProtectedReason> {
    let mut out = BTreeMap::new();
    for line in text.lines() {
        // mountinfo fields: id pid major:minor root MOUNTPOINT opts... - fstype SOURCE super
        // The mount point is field index 4 (0-based); the source follows the " - "
        // separator.
        let Some((pre, post)) = line.split_once(" - ") else {
            continue;
        };
        let pre_fields: Vec<&str> = pre.split_whitespace().collect();
        if pre_fields.len() < 5 {
            continue;
        }
        let mountpoint = pre_fields[4];
        let post_fields: Vec<&str> = post.split_whitespace().collect();
        if post_fields.len() < 2 {
            continue;
        }
        let source = post_fields[1];
        if !source.starts_with("/dev/") {
            continue;
        }
        let reason = match mountpoint {
            "/" | "/boot" | "/boot/efi" | "/efi" => ProtectedReason::RootBootEfi,
            "/mnt/mesh-storage" => ProtectedReason::MeshStorageBacker,
            _ => continue,
        };
        let disk = parent_disk(source);
        // Root/boot/EFI wins over a mesh-storage classification on the same disk.
        out.entry(disk)
            .and_modify(|r| {
                if reason == ProtectedReason::RootBootEfi {
                    *r = ProtectedReason::RootBootEfi;
                }
            })
            .or_insert(reason);
    }
    out
}

/// A snapshot of which whole disks back running VMs/containers — the pure core of
/// the in-use wall.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InUseSnapshot {
    /// whole-disk device → the VM that backs it.
    pub vm_backed: BTreeMap<String, String>,
    /// whole-disk device → the container that backs it.
    pub container_backed: BTreeMap<String, String>,
}

impl InUseSnapshot {
    /// The in-use status of `device` from this snapshot (VM wins over container).
    #[must_use]
    pub fn status_of(&self, device: &str) -> InUseStatus {
        if let Some(vm) = self.vm_backed.get(device) {
            return InUseStatus::InUseByVm(vm.clone());
        }
        if let Some(c) = self.container_backed.get(device) {
            return InUseStatus::InUseByContainer(c.clone());
        }
        InUseStatus::Free
    }
}

/// Build an [`InUseSnapshot`] from raw backing paths: each `(vm, source_path)` / `(container, source_path)` pair's source is reduced to its parent whole disk.
///
/// Non-block sources (an image file under a filesystem, a named podman volume) map to no
/// disk and are skipped — the disk-level wall only fires on a device-backed unit; image-
/// file-at-rest ops are E12-22's concern. Pure.
#[must_use]
pub fn in_use_snapshot_from(
    vm_disks: &[(String, String)],
    container_mounts: &[(String, String)],
) -> InUseSnapshot {
    let mut snap = InUseSnapshot::default();
    for (vm, source) in vm_disks {
        if source.starts_with("/dev/") {
            snap.vm_backed.insert(parent_disk(source), vm.clone());
        }
    }
    for (container, source) in container_mounts {
        if source.starts_with("/dev/") {
            snap.container_backed
                .insert(parent_disk(source), container.clone());
        }
    }
    snap
}

/// Production [`InUseProbe`]: query virsh + podman for the block devices backing running VMs/containers (the same sources the Instances panel uses).
///
/// When the tooling is absent the snapshot is `None` and every device probes
/// [`InUseStatus::Unknown`] — the assume-in-use safe default (lock 7).
#[derive(Debug, Clone, Default)]
pub struct ComputeInUseProbe;

impl ComputeInUseProbe {
    /// The production probe.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Snapshot the in-use disks via virsh/podman, or `None` when neither tool is
    /// usable (⇒ assume-in-use). Reuses [`super::compute_registry`]'s parsers so
    /// there's no parallel model. Best-effort + bounded (EFF-20 timeout).
    fn snapshot(&self) -> Option<InUseSnapshot> {
        let _ = self;
        let mut any_tool = false;
        let mut vm_disks: Vec<(String, String)> = Vec::new();
        let mut container_mounts: Vec<(String, String)> = Vec::new();

        // ── virsh: running domains → their first disk source path ──
        if let Some(uuids) = virsh_output(&["list", "--state-running", "--uuid"]) {
            any_tool = true;
            for uuid in super::compute_registry::parse_virsh_uuid_list(&uuids) {
                if let Some(blk) = virsh_output(&["domblklist", "--details", &uuid]) {
                    if let Some(src) = super::compute_registry::parse_virsh_domblklist(&blk) {
                        let name = virsh_output(&["domname", &uuid])
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .unwrap_or(uuid);
                        vm_disks.push((name, src));
                    }
                }
            }
        }

        // ── podman: running containers → their device-backed mounts ──
        if let Some(json) = podman_output(&["ps", "--format", "json", "--filter", "status=running"])
        {
            any_tool = true;
            for c in super::compute_registry::parse_podman_ps_json(&json) {
                if let Some(mounts) = podman_output(&[
                    "inspect",
                    "--format",
                    "{{range .Mounts}}{{.Source}}\n{{end}}",
                    &c.name,
                ]) {
                    for src in mounts.lines().map(str::trim).filter(|s| !s.is_empty()) {
                        container_mounts.push((c.name.clone(), src.to_string()));
                    }
                }
            }
        }

        if any_tool {
            Some(in_use_snapshot_from(&vm_disks, &container_mounts))
        } else {
            None
        }
    }
}

impl InUseProbe for ComputeInUseProbe {
    fn status(&self, device: &str) -> InUseStatus {
        self.snapshot()
            .map_or(InUseStatus::Unknown, |snap| snap.status_of(device))
    }
}

fn virsh_output(args: &[&str]) -> Option<String> {
    bounded_stdout("virsh", args)
}

fn podman_output(args: &[&str]) -> Option<String> {
    bounded_stdout("podman", args)
}

/// Run `<program> <args>` bounded (EFF-20 timeout), returning stdout on success or
/// `None` when the tool is absent / errors — so a missing tool degrades to the
/// assume-in-use default rather than a crash.
fn bounded_stdout(program: &str, args: &[&str]) -> Option<String> {
    let mut cmd = Command::new(program);
    cmd.args(args);
    let out = output_with_timeout(cmd, DEFAULT_CMD_TIMEOUT).ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        None
    }
}

// ───────────────────────────── typed arming (lock 8) ─────────────────────────────

/// Why an Apply's typed arming was rejected (lock 8) — nothing runs on a mismatch.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ArmingError {
    /// The queue resolves to no whole-disk target (empty, or every op names an
    /// unknown partition).
    #[error("arming failed: the queue has no resolvable target device")]
    NoTarget,
    /// The queue spans more than one whole disk — arming is per-disk (type one
    /// device to apply one disk's queue).
    #[error("arming failed: the queue spans multiple devices ({0})")]
    MultiDevice(String),
    /// The operator-typed device doesn't match the queue's target.
    #[error("arming failed: typed {armed} but the queue targets {target}")]
    Mismatch {
        /// What the operator typed.
        armed: String,
        /// What the queue actually targets.
        target: String,
    },
}

/// Verify the operator-typed `armed_device` matches the single whole disk the
/// `queue` targets against `live` (lock 8). Returns the armed device on success.
///
/// # Errors
/// [`ArmingError`] — no target, a multi-device queue, or a typed mismatch.
pub fn check_arming(
    queue: &StorageQueue,
    live: &Topology,
    armed_device: &str,
) -> Result<String, ArmingError> {
    let targets: Vec<String> = queue.target_devices(live).into_iter().collect();
    match targets.as_slice() {
        [] => Err(ArmingError::NoTarget),
        [target] => {
            if target == armed_device {
                Ok(target.clone())
            } else {
                Err(ArmingError::Mismatch {
                    armed: armed_device.to_string(),
                    target: target.clone(),
                })
            }
        }
        many => Err(ArmingError::MultiDevice(many.join(", "))),
    }
}

// ───────────────────────────── executor seam ─────────────────────────────

/// A storage-worker failure.
#[derive(Debug, Error)]
pub enum StorageError {
    /// `UDisks2` is not reachable on this node (not installed / no system bus) — the
    /// plane renders the typed unavailable state (§7 honest gating).
    #[error("udisks2 unavailable: {0}")]
    Unavailable(String),
    /// A live-execution path whose backend wiring is a later slice (E12-23):
    /// carries exactly what the live call needs — never a fake success (§7).
    #[error("integration-gated: {0}")]
    IntegrationGated(String),
    /// A backend op failed at execution time (carries the op + reason).
    #[error("{op} failed: {reason}")]
    OpFailed {
        /// The op kind that failed.
        op: &'static str,
        /// The backend's error text.
        reason: String,
    },
}

/// Enumerate the live block topology.
///
/// Production [`ZbusUDisks2Client`] talks `UDisks2` over zbus (§2 FDO interop); tests inject
/// a fake returning a canned [`Topology`] (or a [`StorageError::Unavailable`] to exercise
/// honest gating).
#[async_trait::async_trait]
pub trait UDisks2Client: Send + Sync {
    /// The live block-device topology.
    ///
    /// # Errors
    /// [`StorageError::Unavailable`] when `UDisks2` isn't reachable.
    async fn enumerate(&self) -> Result<Topology, StorageError>;
}

/// The resolved-from-topology context an op needs to execute.
///
/// The target's live filesystem, its current size, its parent-disk identity and its
/// mount point (lock 4 choreography + lock 6 per-fs tooling). Built by
/// [`resolve_context`] from the live topology (which the executor itself doesn't see).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OpContext {
    /// The filesystem the target partition carries (Format carries its own; a
    /// resize/subvolume op resolves it from the live topology). `None` ⇒ unknown.
    pub filesystem: Option<Filesystem>,
    /// The target partition's current size (MiB) — the resize choreography anchor.
    pub current_size_mib: u64,
    /// The target partition's start offset (MiB from the disk head).
    pub start_mib: u64,
    /// The parent whole disk (`/dev/sdb`) — the parted geometry target.
    pub disk: Option<String>,
    /// The 1-based partition number (parted geometry).
    pub number: u32,
    /// The mount point, when mounted (online resize + subvolume tools).
    pub mountpoint: Option<String>,
}

/// Resolve the [`OpContext`] for `op` from the live topology. Pure.
///
/// For a `Format`/`LuksFormat`, the filesystem is the op's own target; for a
/// resize/subvolume op it's the target partition's *current* fs (parsed from the
/// `UDisks` id via [`Filesystem::from_id`]).
#[must_use]
pub fn resolve_context(op: &StorageOp, live: &Topology) -> OpContext {
    let mut ctx = OpContext::default();
    if let StorageOp::Format { filesystem, .. } = op {
        ctx.filesystem = Some(*filesystem);
    }
    let Some(part_name) = op.partition() else {
        return ctx;
    };
    if let Some(disk) = live.parent_disk_of(part_name) {
        ctx.disk = Some(disk.name.clone());
    }
    if let Some(p) = live.partition(part_name) {
        ctx.current_size_mib = p.size_mib;
        ctx.start_mib = p.start_mib;
        ctx.number = p.number;
        ctx.mountpoint.clone_from(&p.mountpoint);
        // For a non-Format op the fs is the partition's current one.
        if ctx.filesystem.is_none() {
            ctx.filesystem = p.filesystem.as_deref().and_then(Filesystem::from_id);
        }
    }
    ctx
}

/// The capability pre-check (lock 6): whether the target filesystem honestly
/// supports a resize op in the requested direction.
///
/// Run BEFORE the executor so an unsupported resize (xfs/exfat/swap shrink,
/// exfat/swap grow) becomes a typed [`OpStatus::Unsupported`] halt — never a silent
/// no-op. `Ok(())` for every non-resize op. Pure.
///
/// # Errors
/// A [`CapabilityRefusal`] naming the honest reason.
pub fn capability_check(op: &StorageOp, ctx: &OpContext) -> Result<(), CapabilityRefusal> {
    let (direction, partition) = match op {
        StorageOp::Grow { partition, .. } => (ResizeDirection::Grow, partition),
        StorageOp::Shrink { partition, .. } => (ResizeDirection::Shrink, partition),
        _ => return Ok(()),
    };
    let operation = op.kind();
    let Some(fs) = ctx.filesystem else {
        return Err(CapabilityRefusal::UnknownFilesystem {
            partition: partition.clone(),
            operation,
        });
    };
    let support = match direction {
        ResizeDirection::Grow => fs.capabilities().grow,
        ResizeDirection::Shrink => fs.capabilities().shrink,
    };
    if support.is_supported() {
        Ok(())
    } else {
        Err(match direction {
            ResizeDirection::Grow => CapabilityRefusal::GrowUnsupported { fs },
            ResizeDirection::Shrink => CapabilityRefusal::ShrinkUnsupported { fs },
        })
    }
}

/// Apply one [`StorageOp`] against the live backend, given its resolved
/// [`OpContext`].
///
/// Production [`UDisks2Executor`] drives the per-fs tooling (lock 6) + the shrink/move
/// choreography (lock 4) through the typed [`FsToolRunner`] verb layer (§9 — no raw
/// shell). Tests inject a recording fake that mutates an in-memory topology.
pub trait StorageExecutor: Send + Sync {
    /// Execute `op` (the queue is already validated, walled + capability-checked by
    /// the caller). `ctx` carries the live-topology facts the executor needs.
    ///
    /// # Errors
    /// [`StorageError::OpFailed`] / [`StorageError::IntegrationGated`].
    fn apply(&self, op: &StorageOp, ctx: &OpContext) -> Result<(), StorageError>;
}

/// Production [`StorageExecutor`]: the per-fs tooling + shrink/move choreography over
/// the injectable [`FsToolRunner`] (production [`LiveFsTools`] shells the real tools).
///
/// The **filesystem depth** (format/label/resize/LUKS/subvolume) is live now; the
/// **partition-table geometry** ops (create/delete a table or partition, set flags,
/// mount/unmount, move) remain UDisks2/parted work outside this fs-depth slice, so
/// they answer a typed [`StorageError::IntegrationGated`] naming what's needed — never
/// a fake success (§7). On the headless build host the tools themselves are absent, so
/// even the wired ops answer a typed `FsToolError::Unavailable`, honestly.
#[derive(Clone)]
pub struct UDisks2Executor {
    tools: Arc<dyn FsToolRunner>,
}

impl Default for UDisks2Executor {
    fn default() -> Self {
        Self::new()
    }
}

impl UDisks2Executor {
    /// The production executor over the live fs tooling.
    #[must_use]
    pub fn new() -> Self {
        Self {
            tools: Arc::new(LiveFsTools::new()),
        }
    }

    /// Inject the fs-tool runner (tests).
    #[must_use]
    pub fn with_tools(tools: Arc<dyn FsToolRunner>) -> Self {
        Self { tools }
    }

    /// A parted geometry op this fs-depth slice doesn't wire — a typed honest gate.
    fn gated_geometry(op: &StorageOp) -> StorageError {
        StorageError::IntegrationGated(format!(
            "{} → partition-table geometry (create/delete table+partition, flags, \
             mount/unmount, move) is UDisks2/parted work outside the E12-23 fs-depth \
             slice; the filesystem/LUKS/subvolume verbs + the shrink/move \
             choreography are live",
            op.kind()
        ))
    }

    /// Run a resize choreography (lock 4) through the tool runner, mapping a
    /// mid-plan failure to a typed op error (never a silent partial).
    fn run_resize(
        &self,
        op: &StorageOp,
        ctx: &OpContext,
        new_size_mib: u64,
        direction: ResizeDirection,
    ) -> Result<(), StorageError> {
        let (fs, disk, partition) = self.resize_facts(op, ctx)?;
        let target = ResizeTarget {
            partition: PathBuf::from(partition),
            disk: PathBuf::from(disk),
            number: ctx.number,
            start_mib: ctx.start_mib,
            fs,
            mountpoint: ctx.mountpoint.clone(),
        };
        let plan = fs_tools::resize_plan(&target, new_size_mib, direction).map_err(|e| {
            StorageError::OpFailed {
                op: op.kind(),
                reason: e.to_string(),
            }
        })?;
        let outcome = fs_tools::run_plan(&plan, &*self.tools, |_, _| {});
        outcome.failure_summary(&plan).map_or(Ok(()), |reason| {
            Err(StorageError::OpFailed {
                op: op.kind(),
                reason,
            })
        })
    }

    /// The (filesystem, disk, partition) a resize needs, or a typed error when the
    /// live topology couldn't resolve them.
    fn resize_facts<'a>(
        &self,
        op: &'a StorageOp,
        ctx: &'a OpContext,
    ) -> Result<(Filesystem, &'a str, &'a str), StorageError> {
        let _ = self;
        let partition = op.partition().ok_or_else(|| StorageError::OpFailed {
            op: op.kind(),
            reason: "resize op has no partition".into(),
        })?;
        let fs = ctx.filesystem.ok_or_else(|| StorageError::OpFailed {
            op: op.kind(),
            reason: format!("cannot resize {partition}: filesystem unknown"),
        })?;
        let disk = ctx.disk.as_deref().ok_or_else(|| StorageError::OpFailed {
            op: op.kind(),
            reason: format!("cannot resize {partition}: parent disk unresolved"),
        })?;
        Ok((fs, disk, partition))
    }
}

impl StorageExecutor for UDisks2Executor {
    fn apply(&self, op: &StorageOp, ctx: &OpContext) -> Result<(), StorageError> {
        let mkfs_err = |op: &StorageOp, e: fs_tools::FsToolError| StorageError::OpFailed {
            op: op.kind(),
            reason: e.to_string(),
        };
        match op {
            // ── lock 6: per-fs format/label ──
            StorageOp::Format {
                partition,
                filesystem,
                label,
            } => {
                if matches!(filesystem, Filesystem::Luks) {
                    // A bare LUKS format with no inner fs still needs a keyfile — the
                    // executor gates a keyless one (no passphrase on the Bus).
                    return self
                        .tools
                        .luks_format(Path::new(partition), None)
                        .map_err(|e| mkfs_err(op, e));
                }
                self.tools
                    .mkfs(*filesystem, Path::new(partition), label.as_deref())
                    .map_err(|e| mkfs_err(op, e))
            }
            StorageOp::SetLabel { partition, label } => {
                let fs = ctx.filesystem.ok_or_else(|| StorageError::OpFailed {
                    op: op.kind(),
                    reason: format!("cannot label {partition}: filesystem unknown"),
                })?;
                self.tools
                    .set_label(fs, Path::new(partition), label)
                    .map_err(|e| mkfs_err(op, e))
            }
            // ── lock 4: shrink/move choreography ──
            StorageOp::Grow {
                new_size_mib: n, ..
            } => self.run_resize(op, ctx, *n, ResizeDirection::Grow),
            StorageOp::Shrink {
                new_size_mib: n, ..
            } => self.run_resize(op, ctx, *n, ResizeDirection::Shrink),
            // ── lock 6: LUKS create/unlock/lock (+ format-inside as one staged op) ──
            StorageOp::LuksFormat { .. } => self.apply_luks_format(op),
            StorageOp::LuksOpen {
                partition,
                mapper_name,
                keyfile,
            } => self
                .tools
                .luks_open(Path::new(partition), mapper_name, keyfile.as_deref())
                .map_err(|e| mkfs_err(op, e)),
            StorageOp::LuksClose { mapper_name, .. } => self
                .tools
                .luks_close(mapper_name)
                .map_err(|e| mkfs_err(op, e)),
            // ── lock 6: btrfs subvolumes ──
            StorageOp::SubvolumeCreate { .. }
            | StorageOp::SubvolumeDelete { .. }
            | StorageOp::SubvolumeSnapshot { .. } => self.apply_subvolume(op, ctx),
            // ── partition-table geometry: outside this fs-depth slice ──
            StorageOp::CreateTable { .. }
            | StorageOp::CreatePartition { .. }
            | StorageOp::DeletePartition { .. }
            | StorageOp::SetFlags { .. }
            | StorageOp::Mount { .. }
            | StorageOp::Unmount { .. }
            | StorageOp::Move { .. } => Err(Self::gated_geometry(op)),
        }
    }
}

impl UDisks2Executor {
    /// Map a fs-tool error to the typed op-level failure.
    fn op_failed(op: &StorageOp, e: &fs_tools::FsToolError) -> StorageError {
        StorageError::OpFailed {
            op: op.kind(),
            reason: e.to_string(),
        }
    }

    /// LUKS-format `op`'s partition, and — when it names an inner filesystem — open
    /// the fresh container and mkfs inside it (the format-inside-LUKS staged op, lock
    /// 6). The whole thing halts typed on the first tool failure (no silent partial).
    fn apply_luks_format(&self, op: &StorageOp) -> Result<(), StorageError> {
        let StorageOp::LuksFormat {
            partition,
            mapper_name,
            inner_filesystem,
            keyfile,
            label,
        } = op
        else {
            return Ok(());
        };
        let dev = Path::new(partition);
        let kf = keyfile.as_deref();
        self.tools
            .luks_format(dev, kf)
            .map_err(|e| Self::op_failed(op, &e))?;
        if let Some(inner) = inner_filesystem {
            self.tools
                .luks_open(dev, mapper_name, kf)
                .map_err(|e| Self::op_failed(op, &e))?;
            let mapper = PathBuf::from("/dev/mapper").join(mapper_name);
            self.tools
                .mkfs(*inner, &mapper, label.as_deref())
                .map_err(|e| Self::op_failed(op, &e))?;
        }
        Ok(())
    }

    /// Dispatch a btrfs subvolume op to the tool runner against the (mounted) fs.
    fn apply_subvolume(&self, op: &StorageOp, ctx: &OpContext) -> Result<(), StorageError> {
        let mp = self.subvol_mount(op, ctx)?;
        let r = match op {
            StorageOp::SubvolumeCreate { name, .. } => self.tools.subvol_create(&mp, name),
            StorageOp::SubvolumeDelete { name, .. } => self.tools.subvol_delete(&mp, name),
            StorageOp::SubvolumeSnapshot {
                source,
                dest,
                readonly,
                ..
            } => self.tools.subvol_snapshot(&mp, source, dest, *readonly),
            _ => return Ok(()),
        };
        r.map_err(|e| Self::op_failed(op, &e))
    }

    /// The btrfs mount point a subvolume op operates on (validation already required
    /// it mounted) — a typed error if the live topology lost it.
    fn subvol_mount(&self, op: &StorageOp, ctx: &OpContext) -> Result<String, StorageError> {
        let _ = self;
        ctx.mountpoint
            .clone()
            .ok_or_else(|| StorageError::OpFailed {
                op: op.kind(),
                reason: "subvolume op needs the btrfs mounted".into(),
            })
    }
}

// ───────────────────────────── queue executor ─────────────────────────────

/// The outcome of one op in an applied queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpStatus {
    /// Not yet reached (the queue halted before this op).
    Pending,
    /// Applied successfully.
    Applied,
    /// Refused by a hard wall (lock 7) — the queue halted here.
    Refused(WallRefusal),
    /// Invalid against the live topology or drifted since staging — the queue
    /// halted here (never applied against a stale/invalid picture).
    Invalidated(OpInvalid),
    /// The target filesystem honestly can't do this op (lock 6 — e.g. an xfs shrink)
    /// — the queue halted here with a typed capability state, never a silent no-op.
    Unsupported(CapabilityRefusal),
    /// The backend execution failed — the queue halted here (no silent partial).
    Failed(String),
}

impl OpStatus {
    /// Whether this op landed on disk.
    #[must_use]
    pub const fn is_applied(&self) -> bool {
        matches!(self, Self::Applied)
    }

    /// Whether this op is a halt point (a refusal / invalidation / unsupported /
    /// failure).
    #[must_use]
    pub const fn is_halt(&self) -> bool {
        matches!(
            self,
            Self::Refused(_) | Self::Invalidated(_) | Self::Unsupported(_) | Self::Failed(_)
        )
    }
}

/// The typed result of applying a whole queue — per-op statuses (parallel to
/// `queue.ops`), the halt index if any, and the applied count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueOutcome {
    /// Per-op status, one per `queue.ops` entry, in order.
    pub statuses: Vec<OpStatus>,
    /// The index the queue halted at (a refusal/invalidation/failure), if any.
    pub halted_at: Option<usize>,
    /// How many ops applied to disk.
    pub applied: usize,
}

impl QueueOutcome {
    /// Whether the whole queue applied with no halt.
    #[must_use]
    pub const fn is_success(&self) -> bool {
        self.halted_at.is_none()
    }
}

/// Apply a staged queue against the live topology (apply-time authoritative).
///
/// For each op, in order: revalidate against stage-vs-apply **drift**, authoritative
/// **validate** against the live topology, check the hard **walls**, then execute through
/// `executor`. The first refusal/invalidation/failure **halts** the queue (the remaining
/// ops stay [`OpStatus::Pending`]) and is reported typed — never a silent partial.
/// `on_progress(idx, status)` fires once per resolved op (for the Bus progress stream).
/// Pure over the injected seams.  `staged` is the topology the queue was built against
/// (carried on the Apply verb); `live` is the fresh enumeration. When they're equal there's
/// no drift.
#[must_use]
pub fn apply_queue(
    queue: &StorageQueue,
    staged: &Topology,
    live: &Topology,
    interlocks: &Interlocks,
    executor: &dyn StorageExecutor,
    mut on_progress: impl FnMut(usize, &OpStatus),
) -> QueueOutcome {
    let drift = TopologyDrift::diff(staged, live);
    let mut statuses = vec![OpStatus::Pending; queue.ops.len()];
    let mut halted_at = None;
    let mut applied = 0usize;

    for (i, op) in queue.ops.iter().enumerate() {
        // 1. Stage-vs-apply drift — never apply against a stale picture.
        if let Some(detail) = drift.affects(op, staged) {
            statuses[i] = OpStatus::Invalidated(OpInvalid::Drifted { detail });
            on_progress(i, &statuses[i]);
            halted_at = Some(i);
            break;
        }
        // 2. Authoritative validation against the live topology.
        if let Err(inv) = validate_op(op, live) {
            statuses[i] = OpStatus::Invalidated(inv);
            on_progress(i, &statuses[i]);
            halted_at = Some(i);
            break;
        }
        // 3. Hard walls (lock 7) — resolve the target disk + refuse if protected /
        //    in-use. A UI bug can't reach past here.
        let Some(device) = op.resolve_device(live) else {
            statuses[i] = OpStatus::Invalidated(OpInvalid::UnknownPartition {
                partition: op.partition().unwrap_or_default().to_string(),
            });
            on_progress(i, &statuses[i]);
            halted_at = Some(i);
            break;
        };
        if let Err(refusal) = interlocks.check(&device) {
            statuses[i] = OpStatus::Refused(refusal);
            on_progress(i, &statuses[i]);
            halted_at = Some(i);
            break;
        }
        // 4. Honest per-fs capability gate (lock 6) — an unsupported resize is a
        //    typed state, never a silent no-op. Resolve the op's live-topology facts.
        let ctx = resolve_context(op, live);
        if let Err(refusal) = capability_check(op, &ctx) {
            statuses[i] = OpStatus::Unsupported(refusal);
            on_progress(i, &statuses[i]);
            halted_at = Some(i);
            break;
        }
        // 5. Execute.
        match executor.apply(op, &ctx) {
            Ok(()) => {
                statuses[i] = OpStatus::Applied;
                applied += 1;
                on_progress(i, &statuses[i]);
            }
            Err(e) => {
                statuses[i] = OpStatus::Failed(e.to_string());
                on_progress(i, &statuses[i]);
                halted_at = Some(i);
                break;
            }
        }
    }

    QueueOutcome {
        statuses,
        halted_at,
        applied,
    }
}

// ───────────────────────────── zbus UDisks2 client ─────────────────────────────

/// A flattened `UDisks2` block object — the intermediate the thin zbus adapter fills and the pure [`assemble_topology`] consumes.
///
/// Sizes/offsets are **bytes** (as `UDisks` reports them); the assembler converts to MiB.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UDisksBlock {
    /// The D-Bus object path (`…/block_devices/sda1`) — the partition→disk link key.
    pub object_path: String,
    /// The device node (`/dev/sda1`).
    pub device: String,
    /// `Block.Size` in bytes.
    pub size_bytes: u64,
    /// `PartitionTable.Type` (`gpt`/`dos`), present ⇒ this object is a disk with a
    /// table.
    pub table_type: Option<String>,
    /// Whether the object carries `org.freedesktop.UDisks2.Partition`.
    pub is_partition: bool,
    /// `Partition.Number`.
    pub part_number: u32,
    /// `Partition.Offset` in bytes.
    pub part_offset_bytes: u64,
    /// `Partition.Size` in bytes.
    pub part_size_bytes: u64,
    /// `Partition.Table` — the object path of the disk this partition lives on.
    pub parent_table_path: Option<String>,
    /// `Block.IdType` (filesystem id).
    pub id_type: Option<String>,
    /// `Block.IdLabel`.
    pub id_label: Option<String>,
    /// `Block.IdUUID`.
    pub id_uuid: Option<String>,
    /// Whether the backing drive is removable.
    pub removable: bool,
}

const BYTES_PER_MIB: u64 = 1024 * 1024;

/// Assemble a [`Topology`] from flattened `UDisks2` block objects.
///
/// Disks are the non-partition blocks; partitions link to their disk via
/// [`UDisksBlock::parent_table_path`]. Byte sizes are converted to MiB. Mount points are
/// NOT filled here — the zbus client annotates them from `/proc/self/mountinfo`
/// ([`annotate_mounts`]), which is more reliable than the `UDisks` `aay` `MountPoints`
/// property. Pure + fully testable.
#[must_use]
pub fn assemble_topology(blocks: &[UDisksBlock]) -> Topology {
    // Disks: object_path → BlockDevice (index into a working vec).
    let mut disks: Vec<BlockDevice> = Vec::new();
    let mut disk_index: BTreeMap<String, usize> = BTreeMap::new();
    for b in blocks.iter().filter(|b| !b.is_partition) {
        disk_index.insert(b.object_path.clone(), disks.len());
        disks.push(BlockDevice {
            name: b.device.clone(),
            size_mib: b.size_bytes / BYTES_PER_MIB,
            table: b
                .table_type
                .as_deref()
                .and_then(PartitionTable::from_udisks),
            removable: b.removable,
            partitions: Vec::new(),
        });
    }
    for b in blocks.iter().filter(|b| b.is_partition) {
        let part = Partition {
            name: b.device.clone(),
            number: b.part_number,
            start_mib: b.part_offset_bytes / BYTES_PER_MIB,
            size_mib: b.part_size_bytes / BYTES_PER_MIB,
            filesystem: b.id_type.clone().filter(|s| !s.is_empty()),
            label: b.id_label.clone().filter(|s| !s.is_empty()),
            mountpoint: None,
            uuid: b.id_uuid.clone().filter(|s| !s.is_empty()),
        };
        if let Some(idx) = b.parent_table_path.as_ref().and_then(|p| disk_index.get(p)) {
            disks[*idx].partitions.push(part);
        }
    }
    for disk in &mut disks {
        disk.partitions.sort_by_key(|p| p.number);
    }
    Topology::new(disks)
}

/// Fill each partition's `mountpoint` from a `/proc/self/mountinfo` text: a
/// partition whose device node is a mount source gets that mount point. Pure.
pub fn annotate_mounts(topo: &mut Topology, mountinfo: &str) {
    let mut mounts: BTreeMap<String, String> = BTreeMap::new();
    for line in mountinfo.lines() {
        let Some((pre, post)) = line.split_once(" - ") else {
            continue;
        };
        let pre_fields: Vec<&str> = pre.split_whitespace().collect();
        let post_fields: Vec<&str> = post.split_whitespace().collect();
        if pre_fields.len() < 5 || post_fields.len() < 2 {
            continue;
        }
        let mountpoint = pre_fields[4];
        let source = post_fields[1];
        if source.starts_with("/dev/") {
            mounts
                .entry(source.to_string())
                .or_insert_with(|| mountpoint.to_string());
        }
    }
    for disk in &mut topo.devices {
        for p in &mut disk.partitions {
            if let Some(mp) = mounts.get(&p.name) {
                p.mountpoint = Some(mp.clone());
            }
        }
    }
}

/// Production [`UDisks2Client`] over zbus — the §2 FDO-interop exception (same pattern as mde-seat's `BlueZ`/`UPower`).
///
/// Calls the `UDisks2` `ObjectManager`'s `GetManagedObjects`, flattens each block object,
/// [`assemble_topology`], and [`annotate_mounts`] from `/proc/self/mountinfo`. When the
/// system bus / `UDisks2` isn't reachable (the headless build host) it returns
/// [`StorageError::Unavailable`] so the plane renders the honest unavailable state.
#[derive(Debug, Clone, Default)]
pub struct ZbusUDisks2Client;

impl ZbusUDisks2Client {
    /// The production client.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl UDisks2Client for ZbusUDisks2Client {
    async fn enumerate(&self) -> Result<Topology, StorageError> {
        let conn = zbus::Connection::system()
            .await
            .map_err(|e| StorageError::Unavailable(format!("no system bus: {e}")))?;
        let proxy = zbus::fdo::ObjectManagerProxy::builder(&conn)
            .destination("org.freedesktop.UDisks2")
            .map_err(|e| StorageError::Unavailable(format!("bad destination: {e}")))?
            .path("/org/freedesktop/UDisks2")
            .map_err(|e| StorageError::Unavailable(format!("bad path: {e}")))?
            .build()
            .await
            .map_err(|e| StorageError::Unavailable(format!("udisks2 not present: {e}")))?;
        let objects = proxy
            .get_managed_objects()
            .await
            .map_err(|e| StorageError::Unavailable(format!("GetManagedObjects: {e}")))?;
        let blocks = blocks_from_managed(&objects);
        let mut topo = assemble_topology(&blocks);
        if let Ok(mi) = std::fs::read_to_string("/proc/self/mountinfo") {
            annotate_mounts(&mut topo, &mi);
        }
        Ok(topo)
    }
}

/// The `GetManagedObjects` reply shape.
type ManagedObjects = std::collections::HashMap<
    zbus::zvariant::OwnedObjectPath,
    std::collections::HashMap<
        zbus::names::OwnedInterfaceName,
        std::collections::HashMap<String, zbus::zvariant::OwnedValue>,
    >,
>;

/// Flatten the `UDisks2` `ObjectManager` reply into [`UDisksBlock`]s. Reads the
/// well-known interface properties off each object; the pure scalar extraction is
/// via [`val_u64`] / [`val_str`] / [`val_bool`] (Value-enum matching — version
/// stable). Only exercised on a `UDisks2` host; the assembly it feeds
/// ([`assemble_topology`]) is unit-tested directly.
fn blocks_from_managed(objects: &ManagedObjects) -> Vec<UDisksBlock> {
    const IF_BLOCK: &str = "org.freedesktop.UDisks2.Block";
    const IF_PART: &str = "org.freedesktop.UDisks2.Partition";
    const IF_TABLE: &str = "org.freedesktop.UDisks2.PartitionTable";
    let mut out = Vec::new();
    for (path, ifaces) in objects {
        // Index the object's interfaces by their string name.
        let by_name: BTreeMap<
            &str,
            &std::collections::HashMap<String, zbus::zvariant::OwnedValue>,
        > = ifaces.iter().map(|(k, v)| (k.as_str(), v)).collect();
        let Some(block) = by_name.get(IF_BLOCK) else {
            continue; // not a block object (a Drive/Manager object)
        };
        let device = val_str(block.get("Device"))
            .or_else(|| val_str(block.get("PreferredDevice")))
            .unwrap_or_default();
        if device.is_empty() {
            continue;
        }
        let part = by_name.get(IF_PART);
        let table = by_name.get(IF_TABLE);
        out.push(UDisksBlock {
            object_path: path.as_str().to_string(),
            device,
            size_bytes: val_u64(block.get("Size")).unwrap_or(0),
            table_type: table.and_then(|t| val_str(t.get("Type"))),
            is_partition: part.is_some(),
            part_number: part
                .and_then(|p| val_u64(p.get("Number")))
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or(0),
            part_offset_bytes: part.and_then(|p| val_u64(p.get("Offset"))).unwrap_or(0),
            part_size_bytes: part.and_then(|p| val_u64(p.get("Size"))).unwrap_or(0),
            parent_table_path: part.and_then(|p| val_str(p.get("Table"))),
            id_type: val_str(block.get("IdType")),
            id_label: val_str(block.get("IdLabel")),
            id_uuid: val_str(block.get("IdUUID")),
            removable: val_bool(block.get("HintAuto")).unwrap_or(false),
        });
    }
    out
}

/// Extract a u64 from a `UDisks2` property value (matching the zvariant `Value`
/// enum — stable across zvariant point releases). `None` for a wrong type / absent.
fn val_u64(v: Option<&zbus::zvariant::OwnedValue>) -> Option<u64> {
    use zbus::zvariant::Value;
    match v.map(std::ops::Deref::deref)? {
        Value::U64(n) => Some(*n),
        Value::U32(n) => Some(u64::from(*n)),
        Value::I64(n) => u64::try_from(*n).ok(),
        Value::I32(n) => u64::try_from(*n).ok(),
        _ => None,
    }
}

/// Extract a String from a `UDisks2` property value (a `Str` or an `ObjectPath`).
fn val_str(v: Option<&zbus::zvariant::OwnedValue>) -> Option<String> {
    use zbus::zvariant::Value;
    match v.map(std::ops::Deref::deref)? {
        Value::Str(s) => Some(s.as_str().to_string()),
        Value::ObjectPath(p) => Some(p.as_str().to_string()),
        _ => None,
    }
    .filter(|s| !s.is_empty())
}

/// Extract a bool from a `UDisks2` property value.
fn val_bool(v: Option<&zbus::zvariant::OwnedValue>) -> Option<bool> {
    use zbus::zvariant::Value;
    match v.map(std::ops::Deref::deref)? {
        Value::Bool(b) => Some(*b),
        _ => None,
    }
}

// ───────────────────────────── bus verbs ─────────────────────────────

/// A request drained off `action/storage/<node>`. Internally tagged on `verb`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "verb", rename_all = "snake_case")]
pub enum StorageRequest {
    /// Apply a staged queue to a disk. Carries the operator-typed **arming** device
    /// echo (lock 8) and the topology the queue was **staged** against (for the
    /// stage-vs-apply drift gate). The worker re-enumerates live, checks arming,
    /// then runs [`apply_queue`].
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

/// Parse a [`StorageRequest`] body.
///
/// # Errors
/// A human-readable message on malformed JSON / unknown `verb`.
pub fn parse_request(body: &str) -> Result<StorageRequest, String> {
    serde_json::from_str(body).map_err(|e| format!("malformed storage request: {e}"))
}

/// The backend availability the mirror advertises (lock: honest gating §7).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum BackendStatus {
    /// `UDisks2` enumerated the topology.
    Available,
    /// `UDisks2` isn't reachable — carries the reason; the plane renders the typed
    /// unavailable state.
    Unavailable {
        /// Why the backend is unavailable.
        reason: String,
    },
}

/// The body published to `state/storage/<node>` — the live topology mirror.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StorageState {
    /// The publishing node id.
    pub host: String,
    /// The backend availability (§7 honest gating).
    pub backend: BackendStatus,
    /// The live topology (empty when the backend is unavailable).
    pub topology: Topology,
    /// Wall-clock publish time (ms since the Unix epoch).
    pub published_at_ms: u64,
}

/// A per-op apply-progress event published to `event/storage/<node>/progress`
/// during an Apply — the stream the plane's per-op progress bar consumes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StorageProgress {
    /// The publishing node id.
    pub host: String,
    /// The armed device this apply targets.
    pub device: String,
    /// 0-based op index within the queue.
    pub op_index: usize,
    /// The total op count in the queue.
    pub total: usize,
    /// The op kind (its verb).
    pub op_kind: String,
    /// The op's terminal state (applied / refused / invalidated / failed).
    pub state: ProgressState,
    /// Wall-clock event time (ms since the Unix epoch).
    pub published_at_ms: u64,
}

/// The terminal state of one op in the progress stream.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ProgressState {
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
    /// The target filesystem honestly can't do the op (lock 6 capability state).
    Unsupported {
        /// The capability-refusal text.
        reason: String,
    },
    /// Backend execution failed.
    Failed {
        /// The failure text.
        reason: String,
    },
}

impl ProgressState {
    /// Map an [`OpStatus`] terminal state to a wire [`ProgressState`]. `Pending`
    /// (unreached) has no progress event, so it maps to `None`.
    #[must_use]
    pub fn from_status(status: &OpStatus) -> Option<Self> {
        match status {
            OpStatus::Pending => None,
            OpStatus::Applied => Some(Self::Applied),
            OpStatus::Refused(r) => Some(Self::Refused {
                reason: r.to_string(),
            }),
            OpStatus::Invalidated(i) => Some(Self::Invalidated {
                reason: i.to_string(),
            }),
            OpStatus::Unsupported(c) => Some(Self::Unsupported {
                reason: c.to_string(),
            }),
            OpStatus::Failed(f) => Some(Self::Failed { reason: f.clone() }),
        }
    }
}

// ───────────────────────────── worker ─────────────────────────────

fn default_bus_root() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("mde").join("bus"))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Publish a JSON body to `topic` via the `mde-bus` CLI — the same fire-and-reap
/// path `container`/`vm_lifecycle` use. Best-effort: a missing `mde-bus` binary
/// (pre-RPM dev box) is swallowed.
fn publish_json<T: serde::Serialize>(topic: &str, body: &T) {
    let Ok(json) = serde_json::to_string(body) else {
        return;
    };
    let mut cmd = Command::new("mde-bus");
    cmd.args(["publish", topic, "--body-flag", &json]);
    crate::proc_reap::fire_and_reap(cmd, crate::proc_reap::DEFAULT_REAP_TIMEOUT);
}

/// Read new [`ACTION_TOPIC`] messages since `cursor`, advancing it. A short sync
/// open-read-drop (never crosses an `.await`), mirroring `container`.
fn read_new_requests(
    bus_root: &Path,
    topic: &str,
    cursor: &mut Option<String>,
) -> Vec<StorageRequest> {
    let Ok(persist) = Persist::open(bus_root.to_path_buf()) else {
        return vec![];
    };
    let Ok(msgs) = persist.list_since(topic, cursor.as_deref()) else {
        return vec![];
    };
    let mut out = Vec::new();
    for msg in msgs {
        *cursor = Some(msg.ulid.clone());
        let body = msg.body.as_deref().unwrap_or("");
        match parse_request(body) {
            Ok(r) => out.push(r),
            Err(e) => tracing::warn!(ulid = %msg.ulid, error = %e, "storage: bad storage request"),
        }
    }
    out
}

/// Seed the cursor to the newest existing message so a (re)start doesn't re-run the
/// backlog of Apply verbs. `None` when the topic is empty.
fn prime_cursor(bus_root: &Path, topic: &str) -> Option<String> {
    let persist = Persist::open(bus_root.to_path_buf()).ok()?;
    let msgs = persist.list_since(topic, None).ok()?;
    msgs.last().map(|m| m.ulid.clone())
}

/// The E12-20 storage worker: the privileged owner of the per-node Storage plane.
///
/// Universal like `vm_lifecycle`/`container` — any node has disks — so it runs everywhere
/// (rank-0-default). Per-node topics keep an N-node mesh from cross-driving.
pub struct StorageWorker {
    /// This node's id — the topic namespace + the mirror/progress `host` stamp.
    node_id: String,
    /// The `UDisks2` enumeration seam (production: [`ZbusUDisks2Client`]).
    udisks: Arc<dyn UDisks2Client>,
    /// The op-execution seam (production: [`UDisks2Executor`]).
    executor: Arc<dyn StorageExecutor>,
    /// The hard-wall interlocks (production: [`Interlocks::production`]).
    interlocks: Arc<Interlocks>,
    /// Action-drain cadence.
    poll: Duration,
    /// Topology-mirror republish heartbeat.
    heartbeat: Duration,
    /// Bus root override (tests). `None` ⇒ [`default_bus_root`].
    bus_root_override: Option<PathBuf>,
    /// The E12-22 virtual-disks sub-worker (KVM images + Podman storage), drained +
    /// published in the same loop (no separate mackesd spawn line). See
    /// [`super::virtual_storage`].
    virtual_storage: super::virtual_storage::VirtualStorage,
}

impl StorageWorker {
    /// Construct with production defaults: the live zbus `UDisks2` client, the
    /// integration-gated executor, the mountinfo+compute interlocks, and the
    /// auto-resolved bus root.
    #[must_use]
    pub fn new(node_id: String) -> Self {
        Self {
            virtual_storage: super::virtual_storage::VirtualStorage::production(node_id.clone()),
            node_id,
            udisks: Arc::new(ZbusUDisks2Client::new()),
            executor: Arc::new(UDisks2Executor::new()),
            interlocks: Arc::new(Interlocks::production()),
            poll: DEFAULT_POLL_INTERVAL,
            heartbeat: PUBLISH_HEARTBEAT,
            bus_root_override: None,
        }
    }

    /// Inject the virtual-disks sub-worker (tests).
    #[must_use]
    pub fn with_virtual(mut self, virtual_storage: super::virtual_storage::VirtualStorage) -> Self {
        self.virtual_storage = virtual_storage;
        self
    }

    /// Inject the `UDisks2` client (tests).
    #[must_use]
    pub fn with_udisks(mut self, udisks: Arc<dyn UDisks2Client>) -> Self {
        self.udisks = udisks;
        self
    }

    /// Inject the executor (tests).
    #[must_use]
    pub fn with_executor(mut self, executor: Arc<dyn StorageExecutor>) -> Self {
        self.executor = executor;
        self
    }

    /// Inject the interlocks (tests).
    #[must_use]
    pub fn with_interlocks(mut self, interlocks: Arc<Interlocks>) -> Self {
        self.interlocks = interlocks;
        self
    }

    /// Override the action-drain cadence (tests).
    #[must_use]
    pub const fn with_poll(mut self, poll: Duration) -> Self {
        self.poll = poll;
        self
    }

    /// Override the Bus root (tests).
    #[must_use]
    pub fn with_bus_root(mut self, root: PathBuf) -> Self {
        self.bus_root_override = Some(root);
        self
    }

    fn bus_root(&self) -> Option<PathBuf> {
        self.bus_root_override.clone().or_else(default_bus_root)
    }

    /// Enumerate the live topology, or the typed unavailable reason.
    async fn enumerate(&self) -> Result<Topology, String> {
        self.udisks.enumerate().await.map_err(|e| e.to_string())
    }

    /// Publish the topology mirror to `state/storage/<node>` (honest backend
    /// state).
    async fn publish_state(&self) {
        let (backend, topology) = match self.enumerate().await {
            Ok(topo) => (BackendStatus::Available, topo),
            Err(reason) => (BackendStatus::Unavailable { reason }, Topology::default()),
        };
        let state = StorageState {
            host: self.node_id.clone(),
            backend,
            topology,
            published_at_ms: now_ms(),
        };
        publish_json(&state_topic(&self.node_id), &state);
    }

    /// Handle one Apply verb: re-enumerate live, check arming, run the queue, and
    /// stream per-op progress. Returns `true` when the topology likely changed (any
    /// op applied) so the caller republishes the mirror.
    async fn handle_apply(
        &self,
        armed_device: &str,
        staged: &Topology,
        queue: &StorageQueue,
    ) -> bool {
        let live = match self.enumerate().await {
            Ok(t) => t,
            Err(reason) => {
                tracing::warn!(
                    target: "mackesd::alert",
                    "ALERT (warn): storage apply refused — backend unavailable: {reason}"
                );
                return false;
            }
        };
        // Lock 8 — typed arming. A mismatch refuses the whole queue, nothing runs.
        let device = match check_arming(queue, &live, armed_device) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(
                    target: "mackesd::alert",
                    "ALERT (warn): storage apply refused — {e}"
                );
                return false;
            }
        };
        let total = queue.ops.len();
        let host = self.node_id.clone();
        let dev = device.clone();
        let topic = progress_topic(&self.node_id);
        // Run the (sync, pure-over-seams) queue on a blocking thread; stream
        // per-op progress to the Bus as each op resolves.
        let interlocks = Arc::clone(&self.interlocks);
        let executor = Arc::clone(&self.executor);
        let staged = staged.clone();
        let queue = queue.clone();
        let live_for_apply = live;
        let outcome = tokio::task::spawn_blocking(move || {
            apply_queue(
                &queue,
                &staged,
                &live_for_apply,
                &interlocks,
                &*executor,
                |idx, status| {
                    if let Some(state) = ProgressState::from_status(status) {
                        let progress = StorageProgress {
                            host: host.clone(),
                            device: dev.clone(),
                            op_index: idx,
                            total,
                            op_kind: queue_op_kind(&queue, idx),
                            state,
                            published_at_ms: now_ms(),
                        };
                        publish_json(&topic, &progress);
                    }
                },
            )
        })
        .await;
        match outcome {
            Ok(o) => {
                if !o.is_success() {
                    tracing::warn!(
                        target: "mackesd::alert",
                        "ALERT (warn): storage queue on {device} halted at op {:?} ({} applied)",
                        o.halted_at, o.applied
                    );
                }
                o.applied > 0
            }
            Err(e) => {
                tracing::warn!(error = %e, "storage: apply task join failed");
                false
            }
        }
    }

    /// Drain + handle new requests addressed to this node's topic. Returns `true`
    /// when the topology likely changed (so the caller republishes the mirror).
    async fn drain_and_apply(&self, bus_root: &Path, cursor: &mut Option<String>) -> bool {
        let topic = action_topic(&self.node_id);
        let requests = read_new_requests(bus_root, &topic, cursor);
        let mut changed = false;
        for req in requests {
            match req {
                StorageRequest::Apply {
                    armed_device,
                    staged,
                    queue,
                } => {
                    if self.handle_apply(&armed_device, &staged, &queue).await {
                        changed = true;
                    }
                }
                StorageRequest::Refresh => changed = true,
            }
        }
        changed
    }

    /// Republish the mirror when forced (an op applied) or the heartbeat elapsed.
    async fn publish_snapshot(&self, last_at: &mut Option<Instant>, force: bool) {
        let now = Instant::now();
        let due = force
            || last_at
                .as_ref()
                .is_none_or(|at| now.duration_since(*at) >= self.heartbeat);
        if !due {
            return;
        }
        self.publish_state().await;
        *last_at = Some(now);
    }
}

/// The op kind at `idx`, or `"?"` when out of range (defensive — the index always
/// comes from within the queue).
fn queue_op_kind(queue: &StorageQueue, idx: usize) -> String {
    queue
        .ops
        .get(idx)
        .map_or_else(|| "?".to_string(), |op| op.kind().to_string())
}

#[async_trait::async_trait]
impl Worker for StorageWorker {
    fn name(&self) -> &'static str {
        "storage"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let bus_root = self.bus_root();
        // Publish an immediate mirror so a panel doesn't wait a heartbeat.
        let mut last_pub: Option<Instant> = None;
        self.publish_snapshot(&mut last_pub, true).await;
        // Skip any Apply backlog so a restart doesn't re-run stale destructive ops.
        let mut cursor = bus_root
            .as_deref()
            .and_then(|r| prime_cursor(r, &action_topic(&self.node_id)));
        // The E12-22 virtual queue drains its own sibling topic; prime its cursor too.
        let mut virtual_cursor = bus_root
            .as_deref()
            .and_then(|r| self.virtual_storage.prime_cursor(r));
        let mut tick = tokio::time::interval(self.poll);
        tick.tick().await; // consume the immediate first tick
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    let changed = if let Some(root) = &bus_root {
                        self.drain_and_apply(root, &mut cursor).await
                    } else {
                        false
                    };
                    self.publish_snapshot(&mut last_pub, changed).await;
                    // The virtual sub-worker shells qemu-img/podman (blocking, bounded
                    // by EFF-20), so run its tick off the runtime thread.
                    if let Some(root) = &bus_root {
                        let vs = self.virtual_storage.clone();
                        let root = root.clone();
                        let cur = virtual_cursor.clone();
                        virtual_cursor = tokio::task::spawn_blocking(move || vs.tick(&root, cur))
                            .await
                            .unwrap_or(virtual_cursor);
                    }
                }
                () = shutdown.wait() => break,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // ── fixtures ──

    fn part(name: &str, number: u32, start: u64, size: u64) -> Partition {
        Partition {
            name: name.to_string(),
            number,
            start_mib: start,
            size_mib: size,
            filesystem: Some("ext4".to_string()),
            label: None,
            mountpoint: None,
            uuid: None,
        }
    }

    /// A 100 GiB disk `/dev/sdb` (GPT) with one 10 GiB partition and 90 GiB free.
    fn sample_topo() -> Topology {
        Topology::new(vec![BlockDevice {
            name: "/dev/sdb".into(),
            size_mib: 100 * 1024,
            table: Some(PartitionTable::Gpt),
            removable: true,
            partitions: vec![part("/dev/sdb1", 1, 1, 10 * 1024)],
        }])
    }

    // ── model / topology ──

    #[test]
    fn op_round_trips_json_and_is_self_describing() {
        let op = StorageOp::CreatePartition {
            device: "/dev/sdb".into(),
            start_mib: 1,
            size_mib: 4096,
            filesystem: Some(Filesystem::Ext4),
            label: Some("data".into()),
        };
        let json = serde_json::to_string(&op).unwrap();
        assert!(json.contains(r#""op":"create_partition""#));
        assert_eq!(serde_json::from_str::<StorageOp>(&json).unwrap(), op);
    }

    #[test]
    fn resolve_device_device_and_partition_scoped() {
        let topo = sample_topo();
        let dev_op = StorageOp::CreateTable {
            device: "/dev/sdb".into(),
            table: PartitionTable::Gpt,
        };
        assert_eq!(dev_op.resolve_device(&topo).as_deref(), Some("/dev/sdb"));
        let part_op = StorageOp::Format {
            partition: "/dev/sdb1".into(),
            filesystem: Filesystem::Xfs,
            label: None,
        };
        assert_eq!(part_op.resolve_device(&topo).as_deref(), Some("/dev/sdb"));
        // Unknown partition resolves to no disk.
        let ghost = StorageOp::DeletePartition {
            partition: "/dev/sdz9".into(),
        };
        assert_eq!(ghost.resolve_device(&topo), None);
    }

    #[test]
    fn parent_disk_covers_naming_schemes() {
        assert_eq!(parent_disk("/dev/sda3"), "/dev/sda");
        assert_eq!(parent_disk("/dev/sdb"), "/dev/sdb");
        assert_eq!(parent_disk("/dev/nvme0n1p2"), "/dev/nvme0n1");
        assert_eq!(parent_disk("/dev/nvme0n1"), "/dev/nvme0n1");
        assert_eq!(parent_disk("/dev/mmcblk0p1"), "/dev/mmcblk0");
        assert_eq!(parent_disk("/dev/vda1"), "/dev/vda");
    }

    #[test]
    fn free_space_math() {
        let d = &sample_topo().devices[0];
        assert_eq!(d.free_mib(), 90 * 1024);
    }

    // ── validation ──

    #[test]
    fn validate_create_partition_space() {
        let topo = sample_topo();
        let ok = StorageOp::CreatePartition {
            device: "/dev/sdb".into(),
            start_mib: 1,
            size_mib: 50 * 1024,
            filesystem: None,
            label: None,
        };
        assert!(validate_op(&ok, &topo).is_ok());
        let too_big = StorageOp::CreatePartition {
            device: "/dev/sdb".into(),
            start_mib: 1,
            size_mib: 200 * 1024,
            filesystem: None,
            label: None,
        };
        assert!(matches!(
            validate_op(&too_big, &topo),
            Err(OpInvalid::NotEnoughSpace { .. })
        ));
    }

    #[test]
    fn validate_create_partition_needs_table() {
        let topo = Topology::new(vec![BlockDevice {
            name: "/dev/sdc".into(),
            size_mib: 1024,
            table: None,
            removable: true,
            partitions: vec![],
        }]);
        let op = StorageOp::CreatePartition {
            device: "/dev/sdc".into(),
            start_mib: 1,
            size_mib: 100,
            filesystem: None,
            label: None,
        };
        assert!(matches!(
            validate_op(&op, &topo),
            Err(OpInvalid::NoPartitionTable { .. })
        ));
    }

    #[test]
    fn validate_refuses_ops_on_mounted_partition() {
        let mut topo = sample_topo();
        topo.devices[0].partitions[0].mountpoint = Some("/mnt/data".into());
        for op in [
            StorageOp::DeletePartition {
                partition: "/dev/sdb1".into(),
            },
            StorageOp::Format {
                partition: "/dev/sdb1".into(),
                filesystem: Filesystem::Ext4,
                label: None,
            },
            StorageOp::Shrink {
                partition: "/dev/sdb1".into(),
                new_size_mib: 5 * 1024,
            },
            StorageOp::Move {
                partition: "/dev/sdb1".into(),
                new_start_mib: 2,
            },
        ] {
            assert!(
                matches!(
                    validate_op(&op, &topo),
                    Err(OpInvalid::PartitionMounted { .. })
                ),
                "{} should refuse on a mounted partition",
                op.kind()
            );
        }
    }

    #[test]
    fn validate_mount_unmount_and_resize_directions() {
        let topo = sample_topo();
        // Unmount an unmounted partition → NotMounted.
        assert!(matches!(
            validate_op(
                &StorageOp::Unmount {
                    partition: "/dev/sdb1".into()
                },
                &topo
            ),
            Err(OpInvalid::NotMounted { .. })
        ));
        // Grow beyond current is ok (fits in 90 GiB free); grow that doesn't move
        // up is InvalidResize.
        assert!(validate_op(
            &StorageOp::Grow {
                partition: "/dev/sdb1".into(),
                new_size_mib: 20 * 1024
            },
            &topo
        )
        .is_ok());
        assert!(matches!(
            validate_op(
                &StorageOp::Grow {
                    partition: "/dev/sdb1".into(),
                    new_size_mib: 5 * 1024
                },
                &topo
            ),
            Err(OpInvalid::InvalidResize { .. })
        ));
        // Shrink to a larger size is InvalidResize.
        assert!(matches!(
            validate_op(
                &StorageOp::Shrink {
                    partition: "/dev/sdb1".into(),
                    new_size_mib: 20 * 1024
                },
                &topo
            ),
            Err(OpInvalid::InvalidResize { .. })
        ));
        // A valid shrink.
        assert!(validate_op(
            &StorageOp::Shrink {
                partition: "/dev/sdb1".into(),
                new_size_mib: 5 * 1024
            },
            &topo
        )
        .is_ok());
    }

    #[test]
    fn validate_grow_beyond_free_space() {
        let topo = sample_topo();
        // Grow by more than the 90 GiB free.
        let op = StorageOp::Grow {
            partition: "/dev/sdb1".into(),
            new_size_mib: 150 * 1024,
        };
        assert!(matches!(
            validate_op(&op, &topo),
            Err(OpInvalid::NotEnoughSpace { .. })
        ));
    }

    #[test]
    fn validate_unknown_device_and_partition() {
        let topo = sample_topo();
        assert!(matches!(
            validate_op(
                &StorageOp::CreateTable {
                    device: "/dev/ghost".into(),
                    table: PartitionTable::Gpt
                },
                &topo
            ),
            Err(OpInvalid::UnknownDevice { .. })
        ));
        assert!(matches!(
            validate_op(
                &StorageOp::Unmount {
                    partition: "/dev/ghost1".into()
                },
                &topo
            ),
            Err(OpInvalid::UnknownPartition { .. })
        ));
    }

    #[test]
    fn validate_queue_parallels_ops() {
        let topo = sample_topo();
        let q = StorageQueue::new(vec![
            StorageOp::SetLabel {
                partition: "/dev/sdb1".into(),
                label: "x".into(),
            },
            StorageOp::Unmount {
                partition: "/dev/sdb1".into(),
            },
        ]);
        let res = validate_queue(&q, &topo);
        assert!(res[0].is_ok());
        assert!(res[1].is_err()); // not mounted
    }

    // ── stage-vs-apply drift ──

    #[test]
    fn drift_diff_detects_added_removed_changed() {
        let staged = sample_topo();
        let mut live = sample_topo();
        // Change sdb1 size + add a new partition + remove the disk? Keep disk,
        // mutate a partition and add another.
        live.devices[0].partitions[0].size_mib = 8 * 1024; // changed
        live.devices[0]
            .partitions
            .push(part("/dev/sdb2", 2, 10241, 1024)); // added
        let drift = TopologyDrift::diff(&staged, &live);
        assert!(drift.changed_partitions.contains("/dev/sdb1"));
        assert!(drift.added_partitions.contains("/dev/sdb2"));
        assert!(drift.removed_partitions.is_empty());
        assert!(!drift.is_empty());
    }

    #[test]
    fn drift_empty_when_identical() {
        let a = sample_topo();
        let b = sample_topo();
        assert!(TopologyDrift::diff(&a, &b).is_empty());
    }

    #[test]
    fn revalidate_invalidates_drifted_ops_only() {
        let staged = sample_topo();
        let mut live = sample_topo();
        live.devices[0].partitions[0].mountpoint = Some("/mnt/x".into()); // sdb1 changed
        let q = StorageQueue::new(vec![
            StorageOp::Format {
                partition: "/dev/sdb1".into(),
                filesystem: Filesystem::Ext4,
                label: None,
            },
            StorageOp::CreateTable {
                device: "/dev/sdb".into(),
                table: PartitionTable::Gpt,
            },
        ]);
        let res = revalidate(&q, &staged, &live);
        assert!(matches!(res[0], Some(OpInvalid::Drifted { .. }))); // touches sdb1
        assert!(res[1].is_none()); // create_table on /dev/sdb — disk itself didn't drift
    }

    // ── walls ──

    struct FakeProtected(BTreeMap<String, ProtectedReason>);
    impl ProtectedDevices for FakeProtected {
        fn protected(&self) -> BTreeMap<String, ProtectedReason> {
            self.0.clone()
        }
    }
    struct FakeInUse(InUseStatus);
    impl InUseProbe for FakeInUse {
        fn status(&self, _device: &str) -> InUseStatus {
            self.0.clone()
        }
    }

    fn interlocks(protected: BTreeMap<String, ProtectedReason>, in_use: InUseStatus) -> Interlocks {
        Interlocks::new(
            Arc::new(FakeProtected(protected)),
            Arc::new(FakeInUse(in_use)),
        )
    }

    #[test]
    fn wall_refuses_root_boot_efi() {
        let mut p = BTreeMap::new();
        p.insert("/dev/sda".to_string(), ProtectedReason::RootBootEfi);
        let il = interlocks(p, InUseStatus::Free);
        assert!(matches!(
            il.check("/dev/sda"),
            Err(WallRefusal::Protected {
                reason: ProtectedReason::RootBootEfi,
                ..
            })
        ));
        // A different disk is fine.
        assert!(il.check("/dev/sdb").is_ok());
    }

    #[test]
    fn wall_refuses_mesh_storage_backer() {
        let mut p = BTreeMap::new();
        p.insert("/dev/sdc".to_string(), ProtectedReason::MeshStorageBacker);
        let il = interlocks(p, InUseStatus::Free);
        assert!(matches!(
            il.check("/dev/sdc"),
            Err(WallRefusal::Protected {
                reason: ProtectedReason::MeshStorageBacker,
                ..
            })
        ));
    }

    #[test]
    fn wall_refuses_in_use_vm_and_container() {
        let vm = interlocks(BTreeMap::new(), InUseStatus::InUseByVm("build-vm".into()));
        assert!(matches!(
            vm.check("/dev/sdb"),
            Err(WallRefusal::InUseByVm { .. })
        ));
        let ct = interlocks(BTreeMap::new(), InUseStatus::InUseByContainer("pg".into()));
        assert!(matches!(
            ct.check("/dev/sdb"),
            Err(WallRefusal::InUseByContainer { .. })
        ));
    }

    #[test]
    fn wall_assumes_in_use_on_unknown() {
        let il = interlocks(BTreeMap::new(), InUseStatus::Unknown);
        assert!(matches!(
            il.check("/dev/sdb"),
            Err(WallRefusal::InUseUnknown { .. })
        ));
    }

    #[test]
    fn protected_from_mountinfo_classifies() {
        // Realistic mountinfo lines: root on nvme0n1p3, ESP on nvme0n1p1, mesh
        // storage on sdb1, and an unrelated tmpfs (no /dev source).
        let mi = "\
25 1 259:3 / / rw,relatime shared:1 - ext4 /dev/nvme0n1p3 rw
26 25 259:1 / /boot/efi rw,relatime shared:2 - vfat /dev/nvme0n1p1 rw
30 25 8:17 / /mnt/mesh-storage rw,relatime shared:3 - xfs /dev/sdb1 rw
40 25 0:22 / /run tmpfs rw - tmpfs tmpfs rw
";
        let got = protected_from_mountinfo(mi);
        assert_eq!(got.get("/dev/nvme0n1"), Some(&ProtectedReason::RootBootEfi));
        assert_eq!(
            got.get("/dev/sdb"),
            Some(&ProtectedReason::MeshStorageBacker)
        );
        assert!(!got.contains_key("tmpfs"));
    }

    #[test]
    fn protected_mountinfo_root_wins_over_mesh_on_same_disk() {
        // Contrived: both / and /mnt/mesh-storage on the same physical disk.
        let mi = "\
25 1 259:3 / /mnt/mesh-storage rw - ext4 /dev/sda2 rw
26 1 259:3 / / rw - ext4 /dev/sda1 rw
";
        let got = protected_from_mountinfo(mi);
        assert_eq!(got.get("/dev/sda"), Some(&ProtectedReason::RootBootEfi));
    }

    #[test]
    fn in_use_snapshot_reduces_and_skips_non_block() {
        let snap = in_use_snapshot_from(
            &[
                ("vm-a".into(), "/dev/sdb".into()),
                ("vm-b".into(), "/var/lib/libvirt/images/x.qcow2".into()), // file → skipped
            ],
            &[("ctr".into(), "/dev/sdc1".into())],
        );
        assert_eq!(
            snap.status_of("/dev/sdb"),
            InUseStatus::InUseByVm("vm-a".into())
        );
        assert_eq!(
            snap.status_of("/dev/sdc"),
            InUseStatus::InUseByContainer("ctr".into())
        );
        assert_eq!(snap.status_of("/dev/sdd"), InUseStatus::Free);
    }

    // ── arming ──

    #[test]
    fn arming_ok_mismatch_multi_and_none() {
        let topo = sample_topo();
        let q = StorageQueue::new(vec![StorageOp::Format {
            partition: "/dev/sdb1".into(),
            filesystem: Filesystem::Ext4,
            label: None,
        }]);
        assert_eq!(check_arming(&q, &topo, "/dev/sdb").unwrap(), "/dev/sdb");
        assert!(matches!(
            check_arming(&q, &topo, "/dev/sda"),
            Err(ArmingError::Mismatch { .. })
        ));
        // Empty queue → NoTarget.
        assert!(matches!(
            check_arming(&StorageQueue::default(), &topo, "/dev/sdb"),
            Err(ArmingError::NoTarget)
        ));
    }

    #[test]
    fn arming_multi_device_refused() {
        // Two disks, one op each.
        let topo = Topology::new(vec![
            BlockDevice {
                name: "/dev/sdb".into(),
                size_mib: 1024,
                table: Some(PartitionTable::Gpt),
                removable: true,
                partitions: vec![part("/dev/sdb1", 1, 1, 100)],
            },
            BlockDevice {
                name: "/dev/sdc".into(),
                size_mib: 1024,
                table: Some(PartitionTable::Gpt),
                removable: true,
                partitions: vec![part("/dev/sdc1", 1, 1, 100)],
            },
        ]);
        let q = StorageQueue::new(vec![
            StorageOp::SetLabel {
                partition: "/dev/sdb1".into(),
                label: "a".into(),
            },
            StorageOp::SetLabel {
                partition: "/dev/sdc1".into(),
                label: "b".into(),
            },
        ]);
        assert!(matches!(
            check_arming(&q, &topo, "/dev/sdb"),
            Err(ArmingError::MultiDevice(_))
        ));
    }

    // ── queue executor ──

    /// A recording executor that succeeds, or fails at a chosen op index.
    struct FakeExecutor {
        applied: Mutex<Vec<String>>,
        fail_at: Option<usize>,
        seen: Mutex<usize>,
    }
    impl FakeExecutor {
        fn ok() -> Self {
            Self {
                applied: Mutex::new(vec![]),
                fail_at: None,
                seen: Mutex::new(0),
            }
        }
        fn failing_at(idx: usize) -> Self {
            Self {
                applied: Mutex::new(vec![]),
                fail_at: Some(idx),
                seen: Mutex::new(0),
            }
        }
    }
    impl StorageExecutor for FakeExecutor {
        fn apply(&self, op: &StorageOp, _ctx: &OpContext) -> Result<(), StorageError> {
            let mut n = self.seen.lock().unwrap();
            let this = *n;
            *n += 1;
            if self.fail_at == Some(this) {
                return Err(StorageError::OpFailed {
                    op: op.kind(),
                    reason: "boom".into(),
                });
            }
            self.applied.lock().unwrap().push(op.kind().to_string());
            Ok(())
        }
    }

    fn free_interlocks() -> Interlocks {
        interlocks(BTreeMap::new(), InUseStatus::Free)
    }

    #[test]
    fn apply_queue_all_ok() {
        let q = StorageQueue::new(vec![
            StorageOp::SetLabel {
                partition: "/dev/sdb1".into(),
                label: "x".into(),
            },
            StorageOp::Unmount {
                partition: "/dev/sdb1".into(),
            },
        ]);
        // Make sdb1 mounted so the unmount validates.
        let mut live = sample_topo();
        live.devices[0].partitions[0].mountpoint = Some("/mnt/x".into());
        let exec = FakeExecutor::ok();
        let mut progress = vec![];
        let outcome = apply_queue(&q, &live, &live, &free_interlocks(), &exec, |i, s| {
            progress.push((i, s.clone()))
        });
        assert!(outcome.is_success());
        assert_eq!(outcome.applied, 2);
        assert_eq!(exec.applied.lock().unwrap().len(), 2);
        assert_eq!(progress.len(), 2);
    }

    #[test]
    fn apply_queue_halts_on_failure_no_silent_partial() {
        let live = {
            let mut t = sample_topo();
            t.devices[0].partitions[0].mountpoint = Some("/mnt/x".into());
            t
        };
        let q = StorageQueue::new(vec![
            StorageOp::SetLabel {
                partition: "/dev/sdb1".into(),
                label: "a".into(),
            },
            StorageOp::Unmount {
                partition: "/dev/sdb1".into(),
            },
            StorageOp::SetLabel {
                partition: "/dev/sdb1".into(),
                label: "c".into(),
            },
        ]);
        let exec = FakeExecutor::failing_at(1);
        let outcome = apply_queue(&q, &live, &live, &free_interlocks(), &exec, |_, _| {});
        assert!(!outcome.is_success());
        assert_eq!(outcome.halted_at, Some(1));
        assert_eq!(outcome.applied, 1);
        assert!(matches!(outcome.statuses[0], OpStatus::Applied));
        assert!(matches!(outcome.statuses[1], OpStatus::Failed(_)));
        assert!(matches!(outcome.statuses[2], OpStatus::Pending)); // never reached
    }

    #[test]
    fn apply_queue_halts_on_wall() {
        let live = sample_topo();
        let q = StorageQueue::new(vec![StorageOp::CreateTable {
            device: "/dev/sdb".into(),
            table: PartitionTable::Gpt,
        }]);
        let mut prot = BTreeMap::new();
        prot.insert("/dev/sdb".to_string(), ProtectedReason::RootBootEfi);
        let il = interlocks(prot, InUseStatus::Free);
        let exec = FakeExecutor::ok();
        let outcome = apply_queue(&q, &live, &live, &il, &exec, |_, _| {});
        assert!(!outcome.is_success());
        assert!(matches!(outcome.statuses[0], OpStatus::Refused(_)));
        assert!(exec.applied.lock().unwrap().is_empty()); // never executed
    }

    #[test]
    fn apply_queue_halts_on_drift() {
        let staged = sample_topo();
        let mut live = sample_topo();
        live.devices[0].partitions[0].size_mib = 5 * 1024; // sdb1 drifted
        let q = StorageQueue::new(vec![StorageOp::SetLabel {
            partition: "/dev/sdb1".into(),
            label: "x".into(),
        }]);
        let exec = FakeExecutor::ok();
        let outcome = apply_queue(&q, &staged, &live, &free_interlocks(), &exec, |_, _| {});
        assert!(matches!(
            outcome.statuses[0],
            OpStatus::Invalidated(OpInvalid::Drifted { .. })
        ));
        assert!(exec.applied.lock().unwrap().is_empty());
    }

    #[test]
    fn apply_queue_authoritative_validation_beats_stale_stage() {
        // No drift (staged == live), but the op is invalid against live (unmount a
        // partition that isn't mounted) → Invalidated, not executed.
        let live = sample_topo();
        let q = StorageQueue::new(vec![StorageOp::Unmount {
            partition: "/dev/sdb1".into(),
        }]);
        let exec = FakeExecutor::ok();
        let outcome = apply_queue(&q, &live, &live, &free_interlocks(), &exec, |_, _| {});
        assert!(matches!(
            outcome.statuses[0],
            OpStatus::Invalidated(OpInvalid::NotMounted { .. })
        ));
    }

    // ── E12-23: filesystem depth ──

    fn xfs_topo() -> Topology {
        Topology::new(vec![BlockDevice {
            name: "/dev/sdb".into(),
            size_mib: 100 * 1024,
            table: Some(PartitionTable::Gpt),
            removable: true,
            partitions: vec![Partition {
                name: "/dev/sdb1".into(),
                number: 1,
                start_mib: 1,
                size_mib: 10 * 1024,
                filesystem: Some("xfs".into()),
                label: None,
                mountpoint: None,
                uuid: None,
            }],
        }])
    }

    #[test]
    fn filesystem_from_id_maps_udisks_strings() {
        assert_eq!(Filesystem::from_id("ext4"), Some(Filesystem::Ext4));
        assert_eq!(Filesystem::from_id("EXT3"), Some(Filesystem::Ext4));
        assert_eq!(Filesystem::from_id("crypto_LUKS"), Some(Filesystem::Luks));
        assert_eq!(Filesystem::from_id("btrfs"), Some(Filesystem::Btrfs));
        assert_eq!(Filesystem::from_id("zfs"), None);
    }

    #[test]
    fn resolve_context_pulls_fs_disk_number_and_mount() {
        let mut topo = sample_topo();
        topo.devices[0].partitions[0].mountpoint = Some("/mnt/x".into());
        let ctx = resolve_context(
            &StorageOp::Shrink {
                partition: "/dev/sdb1".into(),
                new_size_mib: 5 * 1024,
            },
            &topo,
        );
        assert_eq!(ctx.filesystem, Some(Filesystem::Ext4));
        assert_eq!(ctx.disk.as_deref(), Some("/dev/sdb"));
        assert_eq!(ctx.number, 1);
        assert_eq!(ctx.current_size_mib, 10 * 1024);
        assert_eq!(ctx.mountpoint.as_deref(), Some("/mnt/x"));
        // Format carries its OWN target fs, not the partition's current one.
        let fctx = resolve_context(
            &StorageOp::Format {
                partition: "/dev/sdb1".into(),
                filesystem: Filesystem::Btrfs,
                label: None,
            },
            &topo,
        );
        assert_eq!(fctx.filesystem, Some(Filesystem::Btrfs));
    }

    #[test]
    fn apply_queue_reports_unsupported_xfs_shrink_typed() {
        // An xfs shrink must halt as a typed Unsupported capability state, never a
        // silent no-op — and the executor must never be called for it.
        let topo = xfs_topo();
        let q = StorageQueue::new(vec![StorageOp::Shrink {
            partition: "/dev/sdb1".into(),
            new_size_mib: 5 * 1024,
        }]);
        let exec = FakeExecutor::ok();
        let outcome = apply_queue(&q, &topo, &topo, &free_interlocks(), &exec, |_, _| {});
        assert!(!outcome.is_success());
        assert!(matches!(
            outcome.statuses[0],
            OpStatus::Unsupported(CapabilityRefusal::ShrinkUnsupported {
                fs: Filesystem::Xfs
            })
        ));
        assert!(exec.applied.lock().unwrap().is_empty());
    }

    #[test]
    fn validate_subvolume_needs_btrfs_and_mount() {
        // ext4 partition → WrongFilesystem.
        let topo = sample_topo();
        assert!(matches!(
            validate_op(
                &StorageOp::SubvolumeCreate {
                    partition: "/dev/sdb1".into(),
                    name: "home".into(),
                },
                &topo
            ),
            Err(OpInvalid::WrongFilesystem { .. })
        ));
        // btrfs but unmounted → NotMounted.
        let mut btrfs = sample_topo();
        btrfs.devices[0].partitions[0].filesystem = Some("btrfs".into());
        assert!(matches!(
            validate_op(
                &StorageOp::SubvolumeCreate {
                    partition: "/dev/sdb1".into(),
                    name: "home".into(),
                },
                &btrfs
            ),
            Err(OpInvalid::NotMounted { .. })
        ));
        // btrfs + mounted → ok.
        btrfs.devices[0].partitions[0].mountpoint = Some("/mnt/b".into());
        assert!(validate_op(
            &StorageOp::SubvolumeSnapshot {
                partition: "/dev/sdb1".into(),
                source: "home".into(),
                dest: "home-snap".into(),
                readonly: true,
            },
            &btrfs
        )
        .is_ok());
    }

    #[test]
    fn luks_format_requires_unmounted_and_round_trips() {
        let mut topo = sample_topo();
        topo.devices[0].partitions[0].mountpoint = Some("/mnt/x".into());
        assert!(matches!(
            validate_op(
                &StorageOp::LuksFormat {
                    partition: "/dev/sdb1".into(),
                    mapper_name: "cryptdata".into(),
                    inner_filesystem: Some(Filesystem::Ext4),
                    keyfile: Some(PathBuf::from("/run/key")),
                    label: None,
                },
                &topo
            ),
            Err(OpInvalid::PartitionMounted { .. })
        ));
        // JSON is self-describing.
        let op = StorageOp::LuksOpen {
            partition: "/dev/sdb1".into(),
            mapper_name: "cryptdata".into(),
            keyfile: None,
        };
        let json = serde_json::to_string(&op).unwrap();
        assert!(json.contains(r#""op":"luks_open""#));
        assert_eq!(serde_json::from_str::<StorageOp>(&json).unwrap(), op);
    }

    /// A recording fs-tool runner: succeeds, logging each verb.
    #[derive(Default)]
    struct RecordingTools(Mutex<Vec<String>>);
    impl fs_tools::FsToolRunner for RecordingTools {
        fn mkfs(
            &self,
            fs: Filesystem,
            _: &Path,
            _: Option<&str>,
        ) -> Result<(), fs_tools::FsToolError> {
            self.0.lock().unwrap().push(format!("mkfs:{}", fs.as_str()));
            Ok(())
        }
        fn set_label(&self, _: Filesystem, _: &Path, _: &str) -> Result<(), fs_tools::FsToolError> {
            self.0.lock().unwrap().push("set_label".into());
            Ok(())
        }
        fn fs_check(&self, _: Filesystem, _: &Path) -> Result<(), fs_tools::FsToolError> {
            self.0.lock().unwrap().push("fs_check".into());
            Ok(())
        }
        fn fs_resize(
            &self,
            _: Filesystem,
            _: &Path,
            _: Option<&str>,
            _: u64,
            _: fs_tools::ResizeDirection,
        ) -> Result<(), fs_tools::FsToolError> {
            self.0.lock().unwrap().push("fs_resize".into());
            Ok(())
        }
        fn part_resize(&self, _: &Path, _: u32, _: u64) -> Result<(), fs_tools::FsToolError> {
            self.0.lock().unwrap().push("part_resize".into());
            Ok(())
        }
        fn luks_format(&self, _: &Path, _: Option<&Path>) -> Result<(), fs_tools::FsToolError> {
            self.0.lock().unwrap().push("luks_format".into());
            Ok(())
        }
        fn luks_open(
            &self,
            _: &Path,
            _: &str,
            _: Option<&Path>,
        ) -> Result<(), fs_tools::FsToolError> {
            self.0.lock().unwrap().push("luks_open".into());
            Ok(())
        }
        fn luks_close(&self, _: &str) -> Result<(), fs_tools::FsToolError> {
            self.0.lock().unwrap().push("luks_close".into());
            Ok(())
        }
        fn subvol_list(&self, _: &str) -> Result<Vec<String>, fs_tools::FsToolError> {
            self.0.lock().unwrap().push("subvol_list".into());
            Ok(vec![])
        }
        fn subvol_create(&self, _: &str, _: &str) -> Result<(), fs_tools::FsToolError> {
            self.0.lock().unwrap().push("subvol_create".into());
            Ok(())
        }
        fn subvol_delete(&self, _: &str, _: &str) -> Result<(), fs_tools::FsToolError> {
            self.0.lock().unwrap().push("subvol_delete".into());
            Ok(())
        }
        fn subvol_snapshot(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: bool,
        ) -> Result<(), fs_tools::FsToolError> {
            self.0.lock().unwrap().push("subvol_snapshot".into());
            Ok(())
        }
    }

    #[test]
    fn executor_drives_fs_tooling_and_gates_geometry() {
        let tools = Arc::new(RecordingTools::default());
        let exec =
            UDisks2Executor::with_tools(Arc::clone(&tools) as Arc<dyn fs_tools::FsToolRunner>);
        // Format → mkfs through the typed verb layer.
        exec.apply(
            &StorageOp::Format {
                partition: "/dev/sdb1".into(),
                filesystem: Filesystem::Btrfs,
                label: Some("data".into()),
            },
            &OpContext {
                filesystem: Some(Filesystem::Btrfs),
                ..OpContext::default()
            },
        )
        .unwrap();
        // LuksFormat with an inner fs → format → open → mkfs (one staged op).
        exec.apply(
            &StorageOp::LuksFormat {
                partition: "/dev/sdb1".into(),
                mapper_name: "cryptdata".into(),
                inner_filesystem: Some(Filesystem::Ext4),
                keyfile: Some(PathBuf::from("/run/key")),
                label: None,
            },
            &OpContext::default(),
        )
        .unwrap();
        // A shrink runs the check→fs→part choreography.
        exec.apply(
            &StorageOp::Shrink {
                partition: "/dev/sdb1".into(),
                new_size_mib: 4096,
            },
            &OpContext {
                filesystem: Some(Filesystem::Ext4),
                current_size_mib: 10 * 1024,
                start_mib: 1,
                disk: Some("/dev/sdb".into()),
                number: 1,
                mountpoint: None,
            },
        )
        .unwrap();
        let calls = {
            let guard = tools.0.lock().unwrap();
            guard.clone()
        };
        assert_eq!(
            calls,
            vec![
                "mkfs:btrfs",
                "luks_format",
                "luks_open",
                "mkfs:ext4",
                "fs_check",
                "fs_resize",
                "part_resize",
            ]
        );
        // A partition-table geometry op is honestly integration-gated (§7).
        assert!(matches!(
            exec.apply(
                &StorageOp::Move {
                    partition: "/dev/sdb1".into(),
                    new_start_mib: 2,
                },
                &OpContext::default(),
            ),
            Err(StorageError::IntegrationGated(_))
        ));
    }

    // ── zbus assembly (pure) ──

    #[test]
    fn assemble_topology_links_partitions_to_disks() {
        let blocks = vec![
            UDisksBlock {
                object_path: "/o/sdb".into(),
                device: "/dev/sdb".into(),
                size_bytes: 2 * BYTES_PER_MIB,
                table_type: Some("gpt".into()),
                is_partition: false,
                removable: true,
                ..UDisksBlock::default()
            },
            UDisksBlock {
                object_path: "/o/sdb1".into(),
                device: "/dev/sdb1".into(),
                is_partition: true,
                part_number: 1,
                part_offset_bytes: BYTES_PER_MIB,
                part_size_bytes: BYTES_PER_MIB,
                parent_table_path: Some("/o/sdb".into()),
                id_type: Some("ext4".into()),
                id_label: Some("data".into()),
                id_uuid: Some("uuid-1".into()),
                ..UDisksBlock::default()
            },
        ];
        let topo = assemble_topology(&blocks);
        assert_eq!(topo.devices.len(), 1);
        let d = &topo.devices[0];
        assert_eq!(d.name, "/dev/sdb");
        assert_eq!(d.size_mib, 2);
        assert_eq!(d.table, Some(PartitionTable::Gpt));
        assert_eq!(d.partitions.len(), 1);
        assert_eq!(d.partitions[0].name, "/dev/sdb1");
        assert_eq!(d.partitions[0].filesystem.as_deref(), Some("ext4"));
        assert_eq!(d.partitions[0].label.as_deref(), Some("data"));
    }

    #[test]
    fn annotate_mounts_fills_partition_mountpoints() {
        let mut topo = sample_topo();
        let mi = "30 25 8:17 / /mnt/data rw - ext4 /dev/sdb1 rw\n";
        annotate_mounts(&mut topo, mi);
        assert_eq!(
            topo.devices[0].partitions[0].mountpoint.as_deref(),
            Some("/mnt/data")
        );
    }

    // ── bus contract ──

    #[test]
    fn topics_are_per_node_and_namespaced() {
        assert_eq!(action_topic("node-a"), "action/storage/node-a");
        assert_eq!(state_topic("node-a"), "state/storage/node-a");
        assert_eq!(progress_topic("node-a"), "event/storage/node-a/progress");
    }

    #[test]
    fn apply_request_round_trips_with_arming_echo() {
        let req = StorageRequest::Apply {
            armed_device: "/dev/sdb".into(),
            staged: sample_topo(),
            queue: StorageQueue::new(vec![StorageOp::CreateTable {
                device: "/dev/sdb".into(),
                table: PartitionTable::Gpt,
            }]),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains(r#""verb":"apply""#));
        assert!(json.contains(r#""armed_device":"/dev/sdb""#));
        assert_eq!(parse_request(&json).unwrap(), req);
        // Refresh verb.
        assert_eq!(
            parse_request(r#"{"verb":"refresh"}"#).unwrap(),
            StorageRequest::Refresh
        );
        assert!(parse_request("nope").is_err());
    }

    #[test]
    fn storage_state_round_trips_available_and_unavailable() {
        let avail = StorageState {
            host: "n".into(),
            backend: BackendStatus::Available,
            topology: sample_topo(),
            published_at_ms: 1,
        };
        let json = serde_json::to_string(&avail).unwrap();
        assert_eq!(serde_json::from_str::<StorageState>(&json).unwrap(), avail);
        let unavail = StorageState {
            host: "n".into(),
            backend: BackendStatus::Unavailable {
                reason: "no udisks2".into(),
            },
            topology: Topology::default(),
            published_at_ms: 1,
        };
        let json = serde_json::to_string(&unavail).unwrap();
        assert!(json.contains(r#""status":"unavailable""#));
        assert_eq!(
            serde_json::from_str::<StorageState>(&json).unwrap(),
            unavail
        );
    }

    #[test]
    fn progress_state_maps_from_status() {
        assert!(ProgressState::from_status(&OpStatus::Pending).is_none());
        assert_eq!(
            ProgressState::from_status(&OpStatus::Applied),
            Some(ProgressState::Applied)
        );
        assert!(matches!(
            ProgressState::from_status(&OpStatus::Failed("x".into())),
            Some(ProgressState::Failed { .. })
        ));
    }

    // ── worker plumbing ──

    struct FakeUDisks(Result<Topology, String>);
    #[async_trait::async_trait]
    impl UDisks2Client for FakeUDisks {
        async fn enumerate(&self) -> Result<Topology, StorageError> {
            self.0.clone().map_err(StorageError::Unavailable)
        }
    }

    #[test]
    fn worker_name_matches_module() {
        let w = StorageWorker::new("node".into());
        assert_eq!(w.name(), "storage");
    }

    #[tokio::test]
    async fn tick_loop_exits_on_shutdown() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = tokio::sync::watch::channel(false);
        let mut w = StorageWorker::new("node".into())
            .with_udisks(Arc::new(FakeUDisks(Ok(sample_topo()))))
            .with_executor(Arc::new(FakeExecutor::ok()))
            .with_interlocks(Arc::new(free_interlocks()))
            .with_bus_root(dir.path().to_path_buf())
            .with_poll(Duration::from_millis(10));
        let token = ShutdownToken::from_receiver(rx);
        let handle = tokio::spawn(async move { w.run(token).await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        tx.send(true).expect("signal shutdown");
        let joined = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(joined.is_ok(), "worker must exit promptly on shutdown");
        assert!(joined.unwrap().expect("join").is_ok());
    }

    #[tokio::test]
    async fn unavailable_backend_publishes_typed_state() {
        // enumerate() → Unavailable must not panic the mirror publish.
        let dir = tempfile::tempdir().unwrap();
        let w = StorageWorker::new("node".into())
            .with_udisks(Arc::new(FakeUDisks(Err("no udisks2".into()))))
            .with_bus_root(dir.path().to_path_buf());
        // Directly exercise publish_state (no bus binary needed — publish is
        // best-effort fire-and-reap).
        w.publish_state().await;
    }
}
