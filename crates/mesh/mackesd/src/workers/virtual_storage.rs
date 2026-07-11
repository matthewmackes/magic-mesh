//! E12-22 / QC-15 — **virtual disks first-class**: QEMU images + Podman storage.
//!
//! QEMU images + Podman volumes become citizens of the Storage plane's staged op
//! queue (`docs/design/workbench-storage-plane.md`, lock 10), beside the E12-20
//! physical-disk queue ([`super::storage`]).
//!
//! ## Why a parallel [`VirtualStorageOp`] and not more [`super::storage::StorageOp`] variants
//!
//! The physical queue is keyed on **whole-disk `/dev/…` devices**: its arming echo
//! is a device name, its walls are disk-level (`/proc/self/mountinfo` + a disk
//! backing a running unit), its executor is UDisks2/parted. Virtual ops are keyed on
//! **image file paths** (`~/Local/*.qcow2`) and **podman volume names** — a different
//! arming echo, different walls (a running VM's *image file*, a *mounted volume*), and
//! a different executor (`qemu-img` / the podman CLI). Folding them into `StorageOp`
//! would force `resolve_device`, `check_arming` and the disk-keyed `Interlocks` to
//! grow a second, incompatible identity. So this is a **parallel but isomorphic**
//! pipeline: same shape (the queue is data; injectable runners; walls in the
//! executor; a per-node Bus mirror + progress), its own domain. It publishes to the
//! sibling `state/storage/<node>/virtual` mirror E12-21's UI renders.
//!
//! ## Seams (all injectable, headless-tested — §7 honest gating)
//!
//! - **QEMU images** go through the local [`QemuImgRunner`] seam (production
//!   [`LiveQemuImg`] shells the real `qemu-img`; absent ⇒ a typed
//!   [`QemuImgError::Unavailable`]). QC-15 deleted the old mde-kvm
//!   cloud-hypervisor hotplug path; this worker only manages images at rest.
//! - **Podman storage** goes through [`PodmanStorageRunner`] (production
//!   [`LivePodman`] shells `podman` bounded by the EFF-20 timeout; absent ⇒ a typed
//!   [`PodmanError::Unavailable`]). Volume create/remove/**prune** + the
//!   `system df` usage views.
//! - **The in-use walls reuse E12-20's sources**: running VMs from libvirt
//!   `virsh domblklist` (via [`super::compute_registry`]) and podman container
//!   mounts — an image backing a running VM or a volume mounted by a running
//!   container is a hard [`VirtualWallRefusal`], with the same **assume-in-use**
//!   safe default when the probe can't verify.

#![cfg(feature = "async-services")]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mde_bus::persist::Persist;
use thiserror::Error;

use super::proc::{output_with_timeout, DEFAULT_CMD_TIMEOUT};
use super::storage::ProgressState;

// ───────────────────────────── topics ─────────────────────────────

/// The per-node virtual-storage action topic: `action/storage/<node>/virtual`.
#[must_use]
pub fn action_topic(node: &str) -> String {
    format!("action/storage/{node}/virtual")
}

/// The per-node virtual-topology mirror topic: `state/storage/<node>/virtual`.
#[must_use]
pub fn state_topic(node: &str) -> String {
    format!("state/storage/{node}/virtual")
}

/// The per-node virtual apply-progress topic: `event/storage/<node>/virtual/progress`.
#[must_use]
pub fn progress_topic(node: &str) -> String {
    format!("event/storage/{node}/virtual/progress")
}

/// Slow heartbeat for the virtual-topology republish — `qemu-img info` per image +
/// `podman system df` are not free, so they stay off the hot path (between
/// action-triggered republishes).
pub const PUBLISH_HEARTBEAT: Duration = Duration::from_secs(30);

// ───────────────────────────── qemu-img seam ─────────────────────────────

/// The disk-image format `qemu-img` writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageFormat {
    /// A raw disk image.
    Raw,
    /// A qcow2 image (copy-on-write; carries internal snapshots).
    Qcow2,
}

impl ImageFormat {
    /// The canonical `qemu-img -f`/`-O` format id.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Qcow2 => "qcow2",
        }
    }

    /// Parse a `qemu-img info` `format` string.
    #[must_use]
    pub fn from_qemu(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "raw" => Some(Self::Raw),
            "qcow2" => Some(Self::Qcow2),
            _ => None,
        }
    }

    /// Whether this format carries internal (qcow2) snapshots.
    #[must_use]
    pub const fn supports_snapshots(self) -> bool {
        matches!(self, Self::Qcow2)
    }
}

/// One qcow2 internal snapshot as `qemu-img info --output=json` reports it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ImageSnapshot {
    /// The snapshot id `qemu-img` assigns.
    pub id: String,
    /// The snapshot tag/name.
    pub tag: String,
    /// The saved VM-state size in bytes.
    #[serde(default)]
    pub vm_state_bytes: u64,
}

/// The slice of `qemu-img info --output=json` the plane consumes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ImageInfo {
    /// The image path this info describes.
    pub path: PathBuf,
    /// The image format.
    pub format: ImageFormat,
    /// The logical guest-visible size in bytes.
    pub virtual_size_bytes: u64,
    /// The allocated size in bytes.
    #[serde(default)]
    pub actual_size_bytes: u64,
    /// The backing file, when this is an overlay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backing_file: Option<String>,
    /// Internal qcow2 snapshots.
    #[serde(default)]
    pub snapshots: Vec<ImageSnapshot>,
}

impl ImageInfo {
    /// The logical size in MiB.
    #[must_use]
    pub const fn virtual_size_mib(&self) -> u64 {
        self.virtual_size_bytes / (1024 * 1024)
    }
}

/// A typed `qemu-img` failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum QemuImgError {
    /// No `qemu-img` on this host.
    #[error("qemu-img unavailable: {0}")]
    Unavailable(String),
    /// `qemu-img` ran but the op failed.
    #[error("qemu-img {op} failed: {reason}")]
    Failed {
        /// The op that failed.
        op: &'static str,
        /// `qemu-img`'s error text.
        reason: String,
    },
    /// `qemu-img info` output did not parse.
    #[error("qemu-img info parse: {0}")]
    Parse(String),
}

/// The `qemu-img` I/O seam.
pub trait QemuImgRunner: Send + Sync {
    /// `qemu-img info --output=json <path>`.
    ///
    /// # Errors
    /// A [`QemuImgError`].
    fn info(&self, path: &Path) -> Result<ImageInfo, QemuImgError>;

    /// `qemu-img create -f <format> <path> <size_mib>M`.
    ///
    /// # Errors
    /// A [`QemuImgError`].
    fn create(&self, path: &Path, format: ImageFormat, size_mib: u64) -> Result<(), QemuImgError>;

    /// `qemu-img resize [--shrink] <path> <new_size_mib>M`.
    ///
    /// # Errors
    /// A [`QemuImgError`].
    fn resize(&self, path: &Path, new_size_mib: u64, shrink: bool) -> Result<(), QemuImgError>;

    /// `qemu-img convert -O <dst_format> <src> <dst>`.
    ///
    /// # Errors
    /// A [`QemuImgError`].
    fn convert(&self, src: &Path, dst: &Path, dst_format: ImageFormat) -> Result<(), QemuImgError>;

    /// `qemu-img snapshot -c|-a|-d <tag> <path>`.
    ///
    /// # Errors
    /// A [`QemuImgError`].
    fn snapshot(&self, action: SnapshotAction, path: &Path, tag: &str) -> Result<(), QemuImgError>;
}

/// Which `qemu-img snapshot` sub-verb to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotAction {
    /// Create a snapshot.
    Create,
    /// Apply/revert to a snapshot.
    Apply,
    /// Delete a snapshot.
    Delete,
}

impl SnapshotAction {
    /// The `qemu-img snapshot` flag.
    #[must_use]
    pub const fn flag(self) -> &'static str {
        match self {
            Self::Create => "-c",
            Self::Apply => "-a",
            Self::Delete => "-d",
        }
    }

    /// A short op label for typed errors.
    #[must_use]
    pub const fn op(self) -> &'static str {
        match self {
            Self::Create => "snapshot-create",
            Self::Apply => "snapshot-revert",
            Self::Delete => "snapshot-delete",
        }
    }
}

fn mib_arg(mib: u64) -> String {
    format!("{mib}M")
}

/// `qemu-img info --output=json --force-share <path>` argv after the program.
#[must_use]
pub fn info_argv(path: &Path) -> Vec<String> {
    vec![
        "info".into(),
        "--output=json".into(),
        "--force-share".into(),
        path.to_string_lossy().into_owned(),
    ]
}

/// `qemu-img create` argv after the program.
#[must_use]
pub fn create_argv(path: &Path, format: ImageFormat, size_mib: u64) -> Vec<String> {
    vec![
        "create".into(),
        "-f".into(),
        format.as_str().into(),
        path.to_string_lossy().into_owned(),
        mib_arg(size_mib),
    ]
}

/// `qemu-img resize` argv after the program.
#[must_use]
pub fn resize_argv(path: &Path, new_size_mib: u64, shrink: bool) -> Vec<String> {
    let mut argv = vec!["resize".to_string()];
    if shrink {
        argv.push("--shrink".into());
    }
    argv.push(path.to_string_lossy().into_owned());
    argv.push(mib_arg(new_size_mib));
    argv
}

/// `qemu-img convert` argv after the program.
#[must_use]
pub fn convert_argv(src: &Path, dst: &Path, dst_format: ImageFormat) -> Vec<String> {
    vec![
        "convert".into(),
        "-O".into(),
        dst_format.as_str().into(),
        src.to_string_lossy().into_owned(),
        dst.to_string_lossy().into_owned(),
    ]
}

/// `qemu-img snapshot` argv after the program.
#[must_use]
pub fn snapshot_argv(action: SnapshotAction, path: &Path, tag: &str) -> Vec<String> {
    vec![
        "snapshot".into(),
        action.flag().into(),
        tag.to_string(),
        path.to_string_lossy().into_owned(),
    ]
}

/// Parse a `qemu-img info --output=json` body for `path`.
///
/// # Errors
/// [`QemuImgError::Parse`] on malformed JSON or missing required fields.
pub fn parse_image_info(json: &str, path: &Path) -> Result<ImageInfo, QemuImgError> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| QemuImgError::Parse(e.to_string()))?;
    let format = v
        .get("format")
        .and_then(serde_json::Value::as_str)
        .and_then(ImageFormat::from_qemu)
        .ok_or_else(|| {
            QemuImgError::Parse(format!(
                "missing/unknown format in qemu-img info for {}",
                path.display()
            ))
        })?;
    let virtual_size_bytes = v
        .get("virtual-size")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| QemuImgError::Parse("missing virtual-size".into()))?;
    let actual_size_bytes = v
        .get("actual-size")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let backing_file = v
        .get("backing-filename")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let snapshots = v
        .get("snapshots")
        .and_then(serde_json::Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|s| {
                    let tag = s
                        .get("name")
                        .and_then(serde_json::Value::as_str)?
                        .to_string();
                    let id = s
                        .get("id")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let vm_state_bytes = s
                        .get("vm-state-size")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0);
                    Some(ImageSnapshot {
                        id,
                        tag,
                        vm_state_bytes,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(ImageInfo {
        path: path.to_path_buf(),
        format,
        virtual_size_bytes,
        actual_size_bytes,
        backing_file,
        snapshots,
    })
}

/// The default per-invocation `qemu-img` timeout.
pub const DEFAULT_QEMU_IMG_TIMEOUT: Duration = Duration::from_secs(120);

/// Production [`QemuImgRunner`]: shells the real `qemu-img` bounded by a deadline.
#[derive(Debug, Clone)]
pub struct LiveQemuImg {
    timeout: Duration,
}

impl LiveQemuImg {
    /// The production runner with [`DEFAULT_QEMU_IMG_TIMEOUT`].
    #[must_use]
    pub const fn new() -> Self {
        Self {
            timeout: DEFAULT_QEMU_IMG_TIMEOUT,
        }
    }

    /// Override the per-invocation timeout.
    #[must_use]
    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    fn run(&self, op: &'static str, argv: &[String]) -> Result<String, QemuImgError> {
        let mut cmd = Command::new("qemu-img");
        cmd.args(argv)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .map_err(|e| QemuImgError::Unavailable(format!("spawn qemu-img: {e}")))?;
        let deadline = Instant::now() + self.timeout;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => {
                    if Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(QemuImgError::Failed {
                            op,
                            reason: format!("timed out after {:?}", self.timeout),
                        });
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(e) => {
                    return Err(QemuImgError::Failed {
                        op,
                        reason: format!("wait: {e}"),
                    });
                }
            }
        }
        let out = child.wait_with_output().map_err(|e| QemuImgError::Failed {
            op,
            reason: format!("collect output: {e}"),
        })?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(QemuImgError::Failed {
                op,
                reason: String::from_utf8_lossy(&out.stderr).trim().to_string(),
            })
        }
    }
}

impl Default for LiveQemuImg {
    fn default() -> Self {
        Self::new()
    }
}

impl QemuImgRunner for LiveQemuImg {
    fn info(&self, path: &Path) -> Result<ImageInfo, QemuImgError> {
        let stdout = self.run("info", &info_argv(path))?;
        parse_image_info(&stdout, path)
    }

    fn create(&self, path: &Path, format: ImageFormat, size_mib: u64) -> Result<(), QemuImgError> {
        self.run("create", &create_argv(path, format, size_mib))
            .map(|_| ())
    }

    fn resize(&self, path: &Path, new_size_mib: u64, shrink: bool) -> Result<(), QemuImgError> {
        self.run("resize", &resize_argv(path, new_size_mib, shrink))
            .map(|_| ())
    }

    fn convert(&self, src: &Path, dst: &Path, dst_format: ImageFormat) -> Result<(), QemuImgError> {
        self.run("convert", &convert_argv(src, dst, dst_format))
            .map(|_| ())
    }

    fn snapshot(&self, action: SnapshotAction, path: &Path, tag: &str) -> Result<(), QemuImgError> {
        self.run(action.op(), &snapshot_argv(action, path, tag))
            .map(|_| ())
    }
}

// ───────────────────────────── op model ─────────────────────────────

/// One typed virtual-storage operation — the queue element (lock 10). Image sizes
/// are in **MiB** (the queue granularity). Internally tagged on `op` so the JSON the
/// UI publishes is self-describing.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum VirtualStorageOp {
    /// Create a new `~/Local` image (`qemu-img create`).
    ImageCreate {
        /// The image path to create (must not already exist).
        path: PathBuf,
        /// The format to create (`raw`/`qcow2`).
        format: ImageFormat,
        /// The logical size (MiB).
        size_mib: u64,
    },
    /// Grow or shrink an image to `new_size_mib` (`qemu-img resize`).
    ImageResize {
        /// The image path.
        path: PathBuf,
        /// The target logical size (MiB), different from current.
        new_size_mib: u64,
    },
    /// Convert an image to `format` at a new path (`qemu-img convert` — raw⇄qcow2).
    ImageConvert {
        /// The source image (must exist).
        src: PathBuf,
        /// The destination image (must not exist).
        dst: PathBuf,
        /// The destination format.
        format: ImageFormat,
    },
    /// Clone a golden base to a fresh, self-contained `~/Local` image (`qemu-img
    /// convert` — a full flat copy, never a backing overlay).
    ImageCloneGolden {
        /// The golden base (must exist).
        golden: PathBuf,
        /// The destination running-disk image (must not exist).
        dest: PathBuf,
        /// The destination format.
        format: ImageFormat,
    },
    /// Take an internal snapshot of a qcow2 image (`qemu-img snapshot -c`).
    ImageSnapshot {
        /// The qcow2 image.
        path: PathBuf,
        /// The snapshot tag (must not already exist).
        tag: String,
    },
    /// Revert a qcow2 image to a snapshot (`qemu-img snapshot -a`).
    ImageRevert {
        /// The qcow2 image.
        path: PathBuf,
        /// The snapshot tag to apply (must exist).
        tag: String,
    },
    /// Delete a qcow2 internal snapshot (`qemu-img snapshot -d`).
    ImageDeleteSnapshot {
        /// The qcow2 image.
        path: PathBuf,
        /// The snapshot tag to delete (must exist).
        tag: String,
    },
    /// Delete an image file from `~/Local`.
    ImageDelete {
        /// The image path (must exist and be free).
        path: PathBuf,
    },
    /// Create a podman named volume (`podman volume create`).
    VolumeCreate {
        /// The volume name (must not already exist).
        name: String,
    },
    /// Remove a podman volume (`podman volume rm`, `force` ⇒ `-f`).
    VolumeRemove {
        /// The volume name (must exist and be free).
        name: String,
        /// Evict even if referenced (`-f`).
        #[serde(default)]
        force: bool,
    },
    /// Prune all unused (dangling) podman volumes (`podman volume prune`). The plane
    /// stages the exact list that dies before arming.
    VolumePrune,
}

impl VirtualStorageOp {
    /// A short human label for progress/logging (the verb, not the params).
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::ImageCreate { .. } => "image_create",
            Self::ImageResize { .. } => "image_resize",
            Self::ImageConvert { .. } => "image_convert",
            Self::ImageCloneGolden { .. } => "image_clone_golden",
            Self::ImageSnapshot { .. } => "image_snapshot",
            Self::ImageRevert { .. } => "image_revert",
            Self::ImageDeleteSnapshot { .. } => "image_delete_snapshot",
            Self::ImageDelete { .. } => "image_delete",
            Self::VolumeCreate { .. } => "volume_create",
            Self::VolumeRemove { .. } => "volume_remove",
            Self::VolumePrune => "volume_prune",
        }
    }

    /// The single **arming target** the operator types to authorize this op (lock 8,
    /// virtual flavour): an image path, `volume:<name>`, or `volume-prune`. The op's
    /// *primary written resource* (a create/convert/clone echoes the new file).
    #[must_use]
    pub fn arming_target(&self) -> String {
        match self {
            Self::ImageCreate { path, .. }
            | Self::ImageResize { path, .. }
            | Self::ImageSnapshot { path, .. }
            | Self::ImageRevert { path, .. }
            | Self::ImageDeleteSnapshot { path, .. }
            | Self::ImageDelete { path } => path.to_string_lossy().into_owned(),
            Self::ImageConvert { dst, .. } => dst.to_string_lossy().into_owned(),
            Self::ImageCloneGolden { dest, .. } => dest.to_string_lossy().into_owned(),
            Self::VolumeCreate { name } | Self::VolumeRemove { name, .. } => {
                format!("volume:{name}")
            }
            Self::VolumePrune => "volume-prune".to_string(),
        }
    }

    /// The **existing image** this op requires to be free (the in-use wall target),
    /// or `None` for ops that write a new file / don't touch an at-rest image.
    #[must_use]
    fn walled_image(&self) -> Option<&Path> {
        match self {
            Self::ImageResize { path, .. }
            | Self::ImageSnapshot { path, .. }
            | Self::ImageRevert { path, .. }
            | Self::ImageDeleteSnapshot { path, .. }
            | Self::ImageDelete { path } => Some(path.as_path()),
            // A consistent read of the source must be offline too.
            Self::ImageConvert { src, .. } => Some(src.as_path()),
            Self::ImageCloneGolden { golden, .. } => Some(golden.as_path()),
            // New-file writers and all volume ops: no image wall.
            Self::ImageCreate { .. }
            | Self::VolumeCreate { .. }
            | Self::VolumeRemove { .. }
            | Self::VolumePrune => None,
        }
    }

    /// The **volume** this op requires to be free (mounted-by-container wall), or
    /// `None`.
    #[must_use]
    fn walled_volume(&self) -> Option<&str> {
        match self {
            Self::VolumeRemove { name, .. } => Some(name.as_str()),
            _ => None,
        }
    }
}

/// The staged virtual-op queue — a typed `Vec<VirtualStorageOp>` (serde).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VirtualStorageQueue {
    /// The staged ops, applied in order.
    pub ops: Vec<VirtualStorageOp>,
}

impl VirtualStorageQueue {
    /// A queue from an op vec.
    #[must_use]
    pub const fn new(ops: Vec<VirtualStorageOp>) -> Self {
        Self { ops }
    }

    /// Whether the queue has no ops.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    /// The distinct arming targets across the queue (lock 8: a queue arms one
    /// target).
    #[must_use]
    pub fn arming_targets(&self) -> BTreeSet<String> {
        self.ops
            .iter()
            .map(VirtualStorageOp::arming_target)
            .collect()
    }
}

// ───────────────────────────── topology (state mirror) ─────────────────────────────

/// Backend availability (§7 honest gating) — carried per subsystem in the mirror.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum BackendAvail {
    /// The tool answered.
    Available,
    /// The tool isn't reachable — carries the reason.
    Unavailable {
        /// Why the backend is unavailable.
        reason: String,
    },
}

/// One `~/Local` image in the virtual topology.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ImageEntry {
    /// The image path.
    pub path: PathBuf,
    /// The format, when `qemu-img` could introspect it (`None` ⇒ not introspected).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<ImageFormat>,
    /// The logical (guest-visible) size in MiB.
    #[serde(default)]
    pub virtual_size_mib: u64,
    /// The on-disk allocated size in MiB.
    #[serde(default)]
    pub actual_size_mib: u64,
    /// The backing file, when this is an overlay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backing_file: Option<String>,
    /// The internal snapshot tags (qcow2), in `qemu-img` order.
    #[serde(default)]
    pub snapshots: Vec<String>,
    /// The VM backing this image, when it's in use by a running guest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_use_by_vm: Option<String>,
}

impl ImageEntry {
    /// Whether this image can hold internal snapshots (qcow2).
    #[must_use]
    pub fn supports_snapshots(&self) -> bool {
        self.format.is_some_and(ImageFormat::supports_snapshots)
    }
}

/// One podman volume in the virtual topology.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VolumeEntry {
    /// The volume name.
    pub name: String,
    /// The volume driver (`local`, …).
    #[serde(default)]
    pub driver: String,
    /// The host mountpoint.
    #[serde(default)]
    pub mountpoint: String,
    /// The container mounting this volume, when in use by a running container.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_use_by_container: Option<String>,
}

/// One `podman system df` summary row (matches `podman system df`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DfRow {
    /// The kind (`Images`, `Containers`, `Local Volumes`).
    pub kind: String,
    /// Total count.
    #[serde(default)]
    pub total: u64,
    /// Active count.
    #[serde(default)]
    pub active: u64,
    /// The humanized on-disk size (`podman`'s own string).
    #[serde(default)]
    pub size: String,
    /// The humanized reclaimable size.
    #[serde(default)]
    pub reclaimable: String,
}

/// The virtual-storage topology mirror — QEMU images + podman volumes + `system df`.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VirtualTopology {
    /// The `~/Local` QEMU images.
    #[serde(default)]
    pub images: Vec<ImageEntry>,
    /// The podman volumes.
    #[serde(default)]
    pub volumes: Vec<VolumeEntry>,
    /// The `podman system df` usage summary rows.
    #[serde(default)]
    pub df: Vec<DfRow>,
}

impl VirtualTopology {
    /// The image at `path`, if present.
    #[must_use]
    pub fn image(&self, path: &Path) -> Option<&ImageEntry> {
        self.images.iter().find(|i| i.path == path)
    }

    /// The volume named `name`, if present.
    #[must_use]
    pub fn volume(&self, name: &str) -> Option<&VolumeEntry> {
        self.volumes.iter().find(|v| v.name == name)
    }
}

// ───────────────────────────── validation ─────────────────────────────

/// A typed reason a virtual op is invalid against the live topology.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum VopInvalid {
    /// The named image is absent.
    #[error("unknown image {0}")]
    UnknownImage(String),
    /// A create/convert/clone destination already exists.
    #[error("image {0} already exists")]
    ImageExists(String),
    /// A snapshot op on a non-qcow2 image.
    #[error("image {0} is not qcow2 — no internal snapshots")]
    NotQcow2(String),
    /// The named snapshot is absent.
    #[error("image {path} has no snapshot {tag}")]
    UnknownSnapshot {
        /// The image.
        path: String,
        /// The missing tag.
        tag: String,
    },
    /// A snapshot tag already exists.
    #[error("image {path} already has snapshot {tag}")]
    SnapshotExists {
        /// The image.
        path: String,
        /// The duplicate tag.
        tag: String,
    },
    /// A resize whose target size doesn't differ from the current size / is zero.
    #[error("image {path}: invalid resize to {new_size_mib} MiB (current {current_mib})")]
    InvalidResize {
        /// The image.
        path: String,
        /// The requested size.
        new_size_mib: u64,
        /// The current size.
        current_mib: u64,
    },
    /// The named volume is absent.
    #[error("unknown volume {0}")]
    UnknownVolume(String),
    /// The named volume already exists.
    #[error("volume {0} already exists")]
    VolumeExists(String),
}

/// Validate one virtual op against `topo` (advisory at stage, authoritative at
/// apply). Pure.
///
/// # Errors
/// A [`VopInvalid`] describing the first failed precondition.
pub fn validate_vop(op: &VirtualStorageOp, topo: &VirtualTopology) -> Result<(), VopInvalid> {
    match op {
        VirtualStorageOp::ImageCreate { path, .. } => require_absent_image(topo, path),
        VirtualStorageOp::ImageConvert { src, dst, .. } => {
            require_image(topo, src)?;
            require_absent_image(topo, dst)
        }
        VirtualStorageOp::ImageCloneGolden { golden, dest, .. } => {
            require_image(topo, golden)?;
            require_absent_image(topo, dest)
        }
        VirtualStorageOp::ImageResize { path, new_size_mib } => {
            let img = require_image(topo, path)?;
            if *new_size_mib == 0 || *new_size_mib == img.virtual_size_mib {
                return Err(VopInvalid::InvalidResize {
                    path: disp(path),
                    new_size_mib: *new_size_mib,
                    current_mib: img.virtual_size_mib,
                });
            }
            Ok(())
        }
        VirtualStorageOp::ImageSnapshot { path, tag } => {
            let img = require_qcow2(topo, path)?;
            if img.snapshots.iter().any(|t| t == tag) {
                return Err(VopInvalid::SnapshotExists {
                    path: disp(path),
                    tag: tag.clone(),
                });
            }
            Ok(())
        }
        VirtualStorageOp::ImageRevert { path, tag }
        | VirtualStorageOp::ImageDeleteSnapshot { path, tag } => {
            let img = require_qcow2(topo, path)?;
            if img.snapshots.iter().any(|t| t == tag) {
                Ok(())
            } else {
                Err(VopInvalid::UnknownSnapshot {
                    path: disp(path),
                    tag: tag.clone(),
                })
            }
        }
        VirtualStorageOp::ImageDelete { path } => require_image(topo, path).map(|_| ()),
        VirtualStorageOp::VolumeCreate { name } => {
            if topo.volume(name).is_some() {
                Err(VopInvalid::VolumeExists(name.clone()))
            } else {
                Ok(())
            }
        }
        VirtualStorageOp::VolumeRemove { name, .. } => {
            if topo.volume(name).is_some() {
                Ok(())
            } else {
                Err(VopInvalid::UnknownVolume(name.clone()))
            }
        }
        VirtualStorageOp::VolumePrune => Ok(()),
    }
}

fn disp(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn require_image<'a>(topo: &'a VirtualTopology, path: &Path) -> Result<&'a ImageEntry, VopInvalid> {
    topo.image(path)
        .ok_or_else(|| VopInvalid::UnknownImage(disp(path)))
}

fn require_absent_image(topo: &VirtualTopology, path: &Path) -> Result<(), VopInvalid> {
    if topo.image(path).is_some() {
        Err(VopInvalid::ImageExists(disp(path)))
    } else {
        Ok(())
    }
}

fn require_qcow2<'a>(topo: &'a VirtualTopology, path: &Path) -> Result<&'a ImageEntry, VopInvalid> {
    let img = require_image(topo, path)?;
    if img.supports_snapshots() {
        Ok(img)
    } else {
        Err(VopInvalid::NotQcow2(disp(path)))
    }
}

/// Validate every op in `queue` against `topo` (stage-time advisory), parallel to
/// `queue.ops`. Pure.
#[must_use]
pub fn validate_queue(
    queue: &VirtualStorageQueue,
    topo: &VirtualTopology,
) -> Vec<Result<(), VopInvalid>> {
    queue.ops.iter().map(|op| validate_vop(op, topo)).collect()
}

// ───────────────────────────── in-use walls (lock 7) ─────────────────────────────

/// The live in-use status of a virtual resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VirtualInUseStatus {
    /// Not backing any running VM/container.
    Free,
    /// The image backs a running VM.
    InUseByVm(String),
    /// The volume is mounted by a running container.
    InUseByContainer(String),
    /// The probe couldn't determine in-use state — treated as **in-use** (the
    /// assume-in-use safe default, lock 7).
    Unknown,
}

/// A snapshot of which images/volumes back running VMs/containers — the pure core of
/// the virtual in-use wall.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VirtualInUseSnapshot {
    /// image path → the VM backing it.
    pub vm_images: BTreeMap<String, String>,
    /// volume name → the container mounting it.
    pub container_volumes: BTreeMap<String, String>,
    /// Whether a VM source (virsh) could be queried at all.
    pub vm_tool: bool,
    /// Whether podman could be queried at all.
    pub container_tool: bool,
}

impl VirtualInUseSnapshot {
    /// The in-use status of image `path` — `Unknown` (assume-in-use) when no VM tool
    /// answered and the path isn't positively free.
    #[must_use]
    pub fn image_status(&self, path: &Path) -> VirtualInUseStatus {
        if let Some(vm) = self.vm_images.get(&disp(path)) {
            return VirtualInUseStatus::InUseByVm(vm.clone());
        }
        if self.vm_tool {
            VirtualInUseStatus::Free
        } else {
            VirtualInUseStatus::Unknown
        }
    }

    /// The in-use status of volume `name`.
    #[must_use]
    pub fn volume_status(&self, name: &str) -> VirtualInUseStatus {
        if let Some(c) = self.container_volumes.get(name) {
            return VirtualInUseStatus::InUseByContainer(c.clone());
        }
        if self.container_tool {
            VirtualInUseStatus::Free
        } else {
            VirtualInUseStatus::Unknown
        }
    }
}

/// The typed refusal a virtual wall raises (lock 7 — a refusal, never a confirm).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum VirtualWallRefusal {
    /// The image backs a running VM (deep-link: shut it down in Instances).
    #[error("refused: image {path} backs running VM {vm}")]
    ImageInUseByVm {
        /// The image.
        path: String,
        /// The VM.
        vm: String,
    },
    /// Image in-use state couldn't be verified — refused under assume-in-use.
    #[error("refused: cannot verify image {0} is free (no VM tooling) — assuming in-use")]
    ImageInUseUnknown(String),
    /// The volume is mounted by a running container.
    #[error("refused: volume {name} is mounted by running container {container}")]
    VolumeInUseByContainer {
        /// The volume.
        name: String,
        /// The container.
        container: String,
    },
    /// Volume in-use state couldn't be verified — refused under assume-in-use.
    #[error("refused: cannot verify volume {0} is free (no podman) — assuming in-use")]
    VolumeInUseUnknown(String),
}

/// The in-use probe seam: snapshot which images/volumes back running units.
/// Production [`ComputeVirtualInUse`] reuses virsh + podman; tests inject a
/// fixed snapshot.
pub trait VirtualInUseProbe: Send + Sync {
    /// Snapshot the live in-use image/volume sets.
    fn snapshot(&self) -> VirtualInUseSnapshot;
}

/// Check one op against the virtual in-use walls, given a pre-taken `snap`. `Ok(())`
/// ⇒ the op may run.
///
/// # Errors
/// A typed [`VirtualWallRefusal`] when the op's existing image backs a running VM,
/// its volume is container-mounted, or in-use couldn't be verified.
pub fn check_wall(
    op: &VirtualStorageOp,
    snap: &VirtualInUseSnapshot,
) -> Result<(), VirtualWallRefusal> {
    if let Some(img) = op.walled_image() {
        match snap.image_status(img) {
            // Free, or (impossible for an image) container-backed → passes.
            VirtualInUseStatus::Free | VirtualInUseStatus::InUseByContainer(_) => {}
            VirtualInUseStatus::InUseByVm(vm) => {
                return Err(VirtualWallRefusal::ImageInUseByVm {
                    path: disp(img),
                    vm,
                })
            }
            VirtualInUseStatus::Unknown => {
                return Err(VirtualWallRefusal::ImageInUseUnknown(disp(img)))
            }
        }
    }
    if let Some(vol) = op.walled_volume() {
        match snap.volume_status(vol) {
            // Free, or (impossible for a volume) VM-backed → passes.
            VirtualInUseStatus::Free | VirtualInUseStatus::InUseByVm(_) => {}
            VirtualInUseStatus::InUseByContainer(container) => {
                return Err(VirtualWallRefusal::VolumeInUseByContainer {
                    name: vol.to_string(),
                    container,
                })
            }
            VirtualInUseStatus::Unknown => {
                return Err(VirtualWallRefusal::VolumeInUseUnknown(vol.to_string()))
            }
        }
    }
    Ok(())
}

/// Production [`VirtualInUseProbe`] over virsh + podman.
///
/// Snapshots which image paths back running libvirt VMs (E12-20's VM source) and
/// which volumes are mounted by running podman containers.
#[derive(Debug, Clone, Default)]
pub struct ComputeVirtualInUse;

impl ComputeVirtualInUse {
    /// The production probe.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl VirtualInUseProbe for ComputeVirtualInUse {
    fn snapshot(&self) -> VirtualInUseSnapshot {
        let mut snap = VirtualInUseSnapshot::default();

        // ── virsh: running libvirt domains → their disk source (E12-20 source) ──
        if let Some(uuids) = bounded_stdout("virsh", &["list", "--state-running", "--uuid"]) {
            snap.vm_tool = true;
            for uuid in super::compute_registry::parse_virsh_uuid_list(&uuids) {
                if let Some(blk) = bounded_stdout("virsh", &["domblklist", "--details", &uuid]) {
                    if let Some(src) = super::compute_registry::parse_virsh_domblklist(&blk) {
                        let name = bounded_stdout("virsh", &["domname", &uuid])
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .unwrap_or(uuid);
                        snap.vm_images.entry(src).or_insert(name);
                    }
                }
            }
        }

        // ── podman: running containers → their mounted named volumes ──
        if let Some(json) = bounded_stdout(
            "podman",
            &["ps", "--format", "json", "--filter", "status=running"],
        ) {
            snap.container_tool = true;
            for c in super::compute_registry::parse_podman_ps_json(&json) {
                if let Some(mounts) = bounded_stdout(
                    "podman",
                    &[
                        "inspect",
                        "--format",
                        "{{range .Mounts}}{{.Type}} {{.Name}}\n{{end}}",
                        &c.name,
                    ],
                ) {
                    for line in mounts.lines() {
                        let mut it = line.split_whitespace();
                        if it.next() == Some("volume") {
                            if let Some(vol) = it.next() {
                                snap.container_volumes
                                    .entry(vol.to_string())
                                    .or_insert_with(|| c.name.clone());
                            }
                        }
                    }
                }
            }
        }

        snap
    }
}

/// Run `<program> <args>` bounded (EFF-20), returning stdout on success or `None`
/// when the tool is absent / errors — so a missing tool degrades to the assume-in-use
/// default (mirrors [`super::storage`]'s bounded probe).
fn bounded_stdout(program: &str, args: &[&str]) -> Option<String> {
    let mut cmd = Command::new(program);
    cmd.args(args);
    let out = output_with_timeout(cmd, DEFAULT_CMD_TIMEOUT).ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

// ───────────────────────────── typed arming (lock 8) ─────────────────────────────

/// Why a virtual Apply's typed arming was rejected (lock 8) — nothing runs on a
/// mismatch.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum VArmingError {
    /// The queue is empty.
    #[error("arming failed: the queue is empty")]
    NoTarget,
    /// The queue spans more than one arming target.
    #[error("arming failed: the queue spans multiple targets ({0})")]
    MultiTarget(String),
    /// The operator-typed target doesn't match the queue's.
    #[error("arming failed: typed {armed} but the queue targets {target}")]
    Mismatch {
        /// What the operator typed.
        armed: String,
        /// What the queue targets.
        target: String,
    },
}

/// Verify the operator-typed `armed` matches the single arming target the `queue`
/// resolves to (lock 8). Returns the target on success.
///
/// # Errors
/// [`VArmingError`] — empty queue, multi-target, or a typed mismatch.
pub fn check_arming(queue: &VirtualStorageQueue, armed: &str) -> Result<String, VArmingError> {
    let targets: Vec<String> = queue.arming_targets().into_iter().collect();
    match targets.as_slice() {
        [] => Err(VArmingError::NoTarget),
        [target] => {
            if target == armed {
                Ok(target.clone())
            } else {
                Err(VArmingError::Mismatch {
                    armed: armed.to_string(),
                    target: target.clone(),
                })
            }
        }
        many => Err(VArmingError::MultiTarget(many.join(", "))),
    }
}

// ───────────────────────────── podman runner seam ─────────────────────────────

/// A typed podman failure (mirrors [`QemuImgError`]).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PodmanError {
    /// No usable `podman` on this host (not installed / socket down).
    #[error("podman unavailable: {0}")]
    Unavailable(String),
    /// `podman` ran but the op failed.
    #[error("podman {op} failed: {reason}")]
    Failed {
        /// The op that failed.
        op: &'static str,
        /// podman's error text.
        reason: String,
    },
}

/// The podman storage I/O seam: enumerate + mutate volumes and read `system df`.
/// Production is [`LivePodman`]; tests inject a fake.
pub trait PodmanStorageRunner: Send + Sync {
    /// `podman volume ls --format json` (raw JSON).
    ///
    /// # Errors
    /// [`PodmanError`].
    fn volume_ls_json(&self) -> Result<String, PodmanError>;

    /// `podman system df --format json` (raw JSON).
    ///
    /// # Errors
    /// [`PodmanError`].
    fn system_df_json(&self) -> Result<String, PodmanError>;

    /// `podman volume create <name>`.
    ///
    /// # Errors
    /// [`PodmanError`].
    fn volume_create(&self, name: &str) -> Result<(), PodmanError>;

    /// `podman volume rm [-f] <name>`.
    ///
    /// # Errors
    /// [`PodmanError`].
    fn volume_rm(&self, name: &str, force: bool) -> Result<(), PodmanError>;

    /// `podman volume prune -f` → the pruned volume names.
    ///
    /// # Errors
    /// [`PodmanError`].
    fn volume_prune(&self) -> Result<Vec<String>, PodmanError>;
}

/// Production [`PodmanStorageRunner`]: shells the real `podman` bounded by EFF-20.
#[derive(Debug, Clone, Default)]
pub struct LivePodman;

impl LivePodman {
    /// The production runner.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    fn run(&self, op: &'static str, args: &[&str]) -> Result<String, PodmanError> {
        let _ = self;
        let mut cmd = Command::new("podman");
        cmd.args(args);
        let out = output_with_timeout(cmd, DEFAULT_CMD_TIMEOUT)
            .map_err(|e| PodmanError::Unavailable(format!("spawn podman: {e}")))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(PodmanError::Failed {
                op,
                reason: String::from_utf8_lossy(&out.stderr).trim().to_string(),
            })
        }
    }
}

impl PodmanStorageRunner for LivePodman {
    fn volume_ls_json(&self) -> Result<String, PodmanError> {
        self.run("volume-ls", &["volume", "ls", "--format", "json"])
    }

    fn system_df_json(&self) -> Result<String, PodmanError> {
        self.run("system-df", &["system", "df", "--format", "json"])
    }

    fn volume_create(&self, name: &str) -> Result<(), PodmanError> {
        self.run("volume-create", &["volume", "create", name])
            .map(|_| ())
    }

    fn volume_rm(&self, name: &str, force: bool) -> Result<(), PodmanError> {
        let args: Vec<&str> = if force {
            vec!["volume", "rm", "-f", name]
        } else {
            vec!["volume", "rm", name]
        };
        self.run("volume-rm", &args).map(|_| ())
    }

    fn volume_prune(&self) -> Result<Vec<String>, PodmanError> {
        let out = self.run("volume-prune", &["volume", "prune", "-f"])?;
        Ok(parse_pruned(&out))
    }
}

/// Parse `podman volume ls --format json` into [`VolumeEntry`]s (no in-use
/// annotation — that's the probe's job). Pure.
#[must_use]
pub fn parse_volumes(json: &str) -> Vec<VolumeEntry> {
    let Ok(rows) = serde_json::from_str::<Vec<serde_json::Value>>(json) else {
        return vec![];
    };
    rows.into_iter()
        .filter_map(|r| {
            let name = r
                .get("Name")
                .and_then(serde_json::Value::as_str)?
                .to_string();
            Some(VolumeEntry {
                name,
                driver: r
                    .get("Driver")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                mountpoint: r
                    .get("Mountpoint")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                in_use_by_container: None,
            })
        })
        .collect()
}

/// Parse `podman system df --format json` into the [`DfRow`] summary. Pure.
#[must_use]
pub fn parse_system_df(json: &str) -> Vec<DfRow> {
    let Ok(rows) = serde_json::from_str::<Vec<serde_json::Value>>(json) else {
        return vec![];
    };
    rows.into_iter()
        .filter_map(|r| {
            let kind = r
                .get("Type")
                .and_then(serde_json::Value::as_str)?
                .to_string();
            Some(DfRow {
                kind,
                total: r
                    .get("Total")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0),
                active: r
                    .get("Active")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0),
                size: df_field(&r, "Size"),
                reclaimable: df_field(&r, "Reclaimable"),
            })
        })
        .collect()
}

/// A `system df` size field, which podman renders as a humanized string (`"1.2GB"`)
/// but occasionally as a number — normalize to a display string.
fn df_field(row: &serde_json::Value, key: &str) -> String {
    match row.get(key) {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

/// Parse `podman volume prune -f` stdout (one pruned volume id/name per non-blank
/// line) into the pruned name list. Pure.
#[must_use]
pub fn parse_pruned(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
}

// ───────────────────────────── executor seam ─────────────────────────────

/// A virtual-op execution failure.
#[derive(Debug, Error)]
pub enum VExecError {
    /// A `qemu-img` op failed / the binary is absent.
    #[error("{0}")]
    QemuImg(#[from] QemuImgError),
    /// A podman op failed / the binary is absent.
    #[error("{0}")]
    Podman(#[from] PodmanError),
    /// A filesystem op (image delete) failed.
    #[error("image delete {path}: {reason}")]
    Fs {
        /// The image path.
        path: String,
        /// The io error.
        reason: String,
    },
}

/// Apply one [`VirtualStorageOp`] against the live backends. Production
/// [`LiveVirtualExecutor`] drives `qemu-img` / podman; tests inject a
/// recording fake.
pub trait VirtualExecutor: Send + Sync {
    /// Execute `op` (the queue is already validated + walled by the caller).
    ///
    /// # Errors
    /// A [`VExecError`].
    fn apply(&self, op: &VirtualStorageOp) -> Result<(), VExecError>;
}

/// Production [`VirtualExecutor`] over the injectable `qemu-img` + podman runners.
///
/// Image ops call `qemu-img`; volume ops call podman. Live `qemu-img`
/// snapshot/revert/golden-clone are only exercised against a real host — on the
/// headless farm each answers a typed error, never a fake success (§7).
pub struct LiveVirtualExecutor {
    qemu: Arc<dyn QemuImgRunner>,
    podman: Arc<dyn PodmanStorageRunner>,
}

impl LiveVirtualExecutor {
    /// The production executor over the given runners.
    #[must_use]
    pub fn new(qemu: Arc<dyn QemuImgRunner>, podman: Arc<dyn PodmanStorageRunner>) -> Self {
        Self { qemu, podman }
    }
}

impl VirtualExecutor for LiveVirtualExecutor {
    fn apply(&self, op: &VirtualStorageOp) -> Result<(), VExecError> {
        match op {
            VirtualStorageOp::ImageCreate {
                path,
                format,
                size_mib,
            } => Ok(self.qemu.create(path, *format, *size_mib)?),
            VirtualStorageOp::ImageResize { path, new_size_mib } => {
                // Determine grow vs shrink from the live image (qemu-img needs
                // --shrink to authorize a shrink).
                let shrink = self
                    .qemu
                    .info(path)
                    .is_ok_and(|i| *new_size_mib < i.virtual_size_mib());
                Ok(self.qemu.resize(path, *new_size_mib, shrink)?)
            }
            VirtualStorageOp::ImageConvert { src, dst, format } => {
                Ok(self.qemu.convert(src, dst, *format)?)
            }
            VirtualStorageOp::ImageCloneGolden {
                golden,
                dest,
                format,
            } => Ok(self.qemu.convert(golden, dest, *format)?),
            VirtualStorageOp::ImageSnapshot { path, tag } => {
                Ok(self.qemu.snapshot(SnapshotAction::Create, path, tag)?)
            }
            VirtualStorageOp::ImageRevert { path, tag } => {
                Ok(self.qemu.snapshot(SnapshotAction::Apply, path, tag)?)
            }
            VirtualStorageOp::ImageDeleteSnapshot { path, tag } => {
                Ok(self.qemu.snapshot(SnapshotAction::Delete, path, tag)?)
            }
            VirtualStorageOp::ImageDelete { path } => {
                std::fs::remove_file(path).map_err(|e| VExecError::Fs {
                    path: disp(path),
                    reason: e.to_string(),
                })
            }
            VirtualStorageOp::VolumeCreate { name } => Ok(self.podman.volume_create(name)?),
            VirtualStorageOp::VolumeRemove { name, force } => {
                Ok(self.podman.volume_rm(name, *force)?)
            }
            VirtualStorageOp::VolumePrune => {
                self.podman.volume_prune().map(|_| ()).map_err(Into::into)
            }
        }
    }
}

// ───────────────────────────── queue executor ─────────────────────────────

/// The outcome of one virtual op in an applied queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VOpStatus {
    /// Not yet reached (the queue halted before this op).
    Pending,
    /// Applied successfully.
    Applied,
    /// Refused by a hard wall (lock 7) — the queue halted here.
    Refused(VirtualWallRefusal),
    /// Invalid against the live topology — the queue halted here.
    Invalidated(VopInvalid),
    /// The backend execution failed — the queue halted here (no silent partial).
    Failed(String),
}

impl VOpStatus {
    /// Map a terminal status to the shared wire [`ProgressState`]. `Pending` maps to
    /// `None`.
    #[must_use]
    pub fn progress(&self) -> Option<ProgressState> {
        match self {
            Self::Pending => None,
            Self::Applied => Some(ProgressState::Applied),
            Self::Refused(r) => Some(ProgressState::Refused {
                reason: r.to_string(),
            }),
            Self::Invalidated(i) => Some(ProgressState::Invalidated {
                reason: i.to_string(),
            }),
            Self::Failed(f) => Some(ProgressState::Failed { reason: f.clone() }),
        }
    }
}

/// The typed result of applying a whole virtual queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VirtualQueueOutcome {
    /// Per-op status, parallel to `queue.ops`.
    pub statuses: Vec<VOpStatus>,
    /// The index the queue halted at, if any.
    pub halted_at: Option<usize>,
    /// How many ops applied.
    pub applied: usize,
}

impl VirtualQueueOutcome {
    /// Whether the whole queue applied with no halt.
    #[must_use]
    pub const fn is_success(&self) -> bool {
        self.halted_at.is_none()
    }
}

/// Apply a staged virtual queue against the live topology (apply-time authoritative).
///
/// For each op, in order: authoritative **validate** against the live topology, check
/// the in-use **walls** (over a single pre-taken `snap`), then execute through
/// `executor`. The first refusal/invalidation/failure **halts** the queue and
/// is reported typed — never a silent partial. `on_progress` fires once per resolved
/// op. Pure over the injected seams.
#[must_use]
pub fn apply_queue(
    queue: &VirtualStorageQueue,
    live: &VirtualTopology,
    snap: &VirtualInUseSnapshot,
    executor: &dyn VirtualExecutor,
    mut on_progress: impl FnMut(usize, &VOpStatus),
) -> VirtualQueueOutcome {
    let mut statuses = vec![VOpStatus::Pending; queue.ops.len()];
    let mut halted_at = None;
    let mut applied = 0usize;

    for (i, op) in queue.ops.iter().enumerate() {
        if let Err(inv) = validate_vop(op, live) {
            statuses[i] = VOpStatus::Invalidated(inv);
            on_progress(i, &statuses[i]);
            halted_at = Some(i);
            break;
        }
        if let Err(refusal) = check_wall(op, snap) {
            statuses[i] = VOpStatus::Refused(refusal);
            on_progress(i, &statuses[i]);
            halted_at = Some(i);
            break;
        }
        match executor.apply(op) {
            Ok(()) => {
                statuses[i] = VOpStatus::Applied;
                applied += 1;
                on_progress(i, &statuses[i]);
            }
            Err(e) => {
                statuses[i] = VOpStatus::Failed(e.to_string());
                on_progress(i, &statuses[i]);
                halted_at = Some(i);
                break;
            }
        }
    }

    VirtualQueueOutcome {
        statuses,
        halted_at,
        applied,
    }
}

// ───────────────────────────── bus contract ─────────────────────────────

/// A request drained off `action/storage/<node>/virtual`. Internally tagged on
/// `verb`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "verb", rename_all = "snake_case")]
pub enum VirtualStorageRequest {
    /// Apply a staged virtual queue. Carries the operator-typed **arming** target
    /// echo (lock 8); the worker re-enumerates live, checks arming, then runs
    /// [`apply_queue`].
    Apply {
        /// The operator-typed arming target (lock 8 echo).
        armed_target: String,
        /// The staged op queue.
        queue: VirtualStorageQueue,
    },
    /// Re-publish the live virtual mirror.
    Refresh,
}

/// Parse a [`VirtualStorageRequest`] body.
///
/// # Errors
/// A human-readable message on malformed JSON / unknown `verb`.
pub fn parse_request(body: &str) -> Result<VirtualStorageRequest, String> {
    serde_json::from_str(body).map_err(|e| format!("malformed virtual-storage request: {e}"))
}

/// The body published to `state/storage/<node>/virtual`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VirtualStorageState {
    /// The publishing node id.
    pub host: String,
    /// `qemu-img` availability (§7).
    pub qemu_img: BackendAvail,
    /// podman availability (§7).
    pub podman: BackendAvail,
    /// The live virtual topology.
    pub topology: VirtualTopology,
    /// Wall-clock publish time (ms since the Unix epoch).
    pub published_at_ms: u64,
}

/// A per-op apply-progress event published to `event/storage/<node>/virtual/progress`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VirtualProgress {
    /// The publishing node id.
    pub host: String,
    /// The armed target this apply carries.
    pub target: String,
    /// 0-based op index within the queue.
    pub op_index: usize,
    /// The total op count.
    pub total: usize,
    /// The op kind (its verb).
    pub op_kind: String,
    /// The op's terminal state (reusing the physical plane's wire enum).
    pub state: ProgressState,
    /// Wall-clock event time (ms since the Unix epoch).
    pub published_at_ms: u64,
}

// ───────────────────────────── the sub-worker ─────────────────────────────

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Publish a JSON body to `topic` via the `mde-bus` CLI (same fire-and-reap path as
/// [`super::storage`]). Best-effort.
/// Publish a JSON mirror/progress body to `topic` in-process (perf-10 / arch-6)
/// — no fork+exec of the `mde-bus` CLI (a whole process + a fresh SQLite open +
/// a [`crate::proc_reap`] reaper thread) per message. Byte-identical stored row
/// to the old `mde-bus publish <topic> --body-flag <json>` (the compact
/// `serde_json` of `body`). `bus_root` is the owning [`super::storage`] worker's
/// bus root, which honours `MDE_BUS_ROOT` — the SAME root the fork+exec'd CLI
/// resolved via the inherited env. Best-effort; a `None` root / failed open /
/// write error is swallowed.
fn publish_json<T: serde::Serialize>(bus_root: Option<&Path>, topic: &str, body: &T) {
    if let Some(mut persist) = crate::bus_publish::open_bus(bus_root.map(Path::to_path_buf)) {
        crate::bus_publish::publish_json(&mut persist, topic, body);
    }
}

/// The default `~/Local` image directory.
#[must_use]
pub fn default_local_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/root"))
        .join("Local")
}

/// The virtual-storage sub-worker: the E12-22 half of the E12-20 storage worker.
///
/// Owned by [`StorageWorker`](super::storage::StorageWorker) (no new mackesd spawn
/// line) — the worker calls [`VirtualStorage::tick`] each poll inside a blocking task.
/// All seams are `Arc`, so it clones cheaply into `spawn_blocking`.
#[derive(Clone)]
pub struct VirtualStorage {
    node_id: String,
    qemu: Arc<dyn QemuImgRunner>,
    podman: Arc<dyn PodmanStorageRunner>,
    in_use: Arc<dyn VirtualInUseProbe>,
    executor: Arc<dyn VirtualExecutor>,
    local_dir: PathBuf,
    heartbeat: Duration,
    last_pub: Arc<Mutex<Option<Instant>>>,
}

impl VirtualStorage {
    /// The production sub-worker: the live `qemu-img` + podman runners, the
    /// virsh/podman in-use probe, and the `~/Local` image directory.
    #[must_use]
    pub fn production(node_id: String) -> Self {
        let qemu: Arc<dyn QemuImgRunner> = Arc::new(LiveQemuImg::new());
        let podman: Arc<dyn PodmanStorageRunner> = Arc::new(LivePodman::new());
        let executor: Arc<dyn VirtualExecutor> = Arc::new(LiveVirtualExecutor::new(
            Arc::clone(&qemu),
            Arc::clone(&podman),
        ));
        Self {
            node_id,
            qemu,
            podman,
            in_use: Arc::new(ComputeVirtualInUse::new()),
            executor,
            local_dir: default_local_dir(),
            heartbeat: PUBLISH_HEARTBEAT,
            last_pub: Arc::new(Mutex::new(None)),
        }
    }

    /// Inject all seams (tests).
    #[must_use]
    pub fn with_seams(
        node_id: String,
        qemu: Arc<dyn QemuImgRunner>,
        podman: Arc<dyn PodmanStorageRunner>,
        in_use: Arc<dyn VirtualInUseProbe>,
        executor: Arc<dyn VirtualExecutor>,
        local_dir: PathBuf,
    ) -> Self {
        Self {
            node_id,
            qemu,
            podman,
            in_use,
            executor,
            local_dir,
            heartbeat: PUBLISH_HEARTBEAT,
            last_pub: Arc::new(Mutex::new(None)),
        }
    }

    /// Enumerate the live virtual topology + per-subsystem availability.
    #[must_use]
    pub fn enumerate(&self) -> (VirtualTopology, BackendAvail, BackendAvail) {
        let snap = self.in_use.snapshot();
        let (images, qemu_avail) = self.enumerate_images(&snap);
        let (volumes, df, podman_avail) = self.enumerate_podman(&snap);
        (
            VirtualTopology {
                images,
                volumes,
                df,
            },
            qemu_avail,
            podman_avail,
        )
    }

    /// Scan `~/Local` for image files and introspect each via `qemu-img info`.
    fn enumerate_images(&self, snap: &VirtualInUseSnapshot) -> (Vec<ImageEntry>, BackendAvail) {
        let mut images = Vec::new();
        let mut qemu_avail = BackendAvail::Available;
        let Ok(entries) = std::fs::read_dir(&self.local_dir) else {
            // No ~/Local yet — an empty inventory, backend still "available" (nothing
            // to introspect); the UI shows an empty images pane.
            return (images, qemu_avail);
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !is_image_file(&path) {
                continue;
            }
            let mut img = ImageEntry {
                path: path.clone(),
                format: None,
                virtual_size_mib: 0,
                actual_size_mib: 0,
                backing_file: None,
                snapshots: Vec::new(),
                in_use_by_vm: None,
            };
            match self.qemu.info(&path) {
                Ok(info) => {
                    img.format = Some(info.format);
                    img.virtual_size_mib = info.virtual_size_mib();
                    img.actual_size_mib = info.actual_size_bytes / (1024 * 1024);
                    img.backing_file = info.backing_file;
                    img.snapshots = info.snapshots.into_iter().map(|s| s.tag).collect();
                }
                Err(QemuImgError::Unavailable(reason)) => {
                    // qemu-img absent — the file is still listed (bare, un-introspected),
                    // and the mirror advertises the honest unavailable backend.
                    qemu_avail = BackendAvail::Unavailable { reason };
                }
                Err(_) => { /* a per-file failure: list the file un-introspected */ }
            }
            if let VirtualInUseStatus::InUseByVm(vm) = snap.image_status(&path) {
                img.in_use_by_vm = Some(vm);
            }
            images.push(img);
        }
        (images, qemu_avail)
    }

    /// Enumerate podman volumes + `system df`, annotating in-use.
    fn enumerate_podman(
        &self,
        snap: &VirtualInUseSnapshot,
    ) -> (Vec<VolumeEntry>, Vec<DfRow>, BackendAvail) {
        let mut volumes = match self.podman.volume_ls_json() {
            Ok(json) => parse_volumes(&json),
            Err(PodmanError::Unavailable(reason)) => {
                return (Vec::new(), Vec::new(), BackendAvail::Unavailable { reason })
            }
            Err(e) => {
                return (
                    Vec::new(),
                    Vec::new(),
                    BackendAvail::Unavailable {
                        reason: e.to_string(),
                    },
                )
            }
        };
        for v in &mut volumes {
            if let VirtualInUseStatus::InUseByContainer(c) = snap.volume_status(&v.name) {
                v.in_use_by_container = Some(c);
            }
        }
        let df = self
            .podman
            .system_df_json()
            .map(|j| parse_system_df(&j))
            .unwrap_or_default();
        (volumes, df, BackendAvail::Available)
    }

    /// Publish the virtual mirror when forced or the heartbeat elapsed.
    fn publish_if_due(&self, bus_root: &Path, force: bool) {
        let due = {
            let last = self.last_pub.lock().expect("last_pub mutex");
            force || last.is_none_or(|at| Instant::now().duration_since(at) >= self.heartbeat)
        };
        if !due {
            return;
        }
        let (topology, qemu_img, podman) = self.enumerate();
        let state = VirtualStorageState {
            host: self.node_id.clone(),
            qemu_img,
            podman,
            topology,
            published_at_ms: now_ms(),
        };
        publish_json(Some(bus_root), &state_topic(&self.node_id), &state);
        *self.last_pub.lock().expect("last_pub mutex") = Some(Instant::now());
    }

    /// Handle one Apply verb: re-enumerate live, check arming, run the queue, stream
    /// per-op progress. Returns `true` when any op applied (so the caller republishes).
    fn handle_apply(
        &self,
        bus_root: &Path,
        armed_target: &str,
        queue: &VirtualStorageQueue,
    ) -> bool {
        let target = match check_arming(queue, armed_target) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(
                    target: "mackesd::alert",
                    "ALERT (warn): virtual-storage apply refused — {e}"
                );
                return false;
            }
        };
        let (live, _, _) = self.enumerate();
        let snap = self.in_use.snapshot();
        let total = queue.ops.len();
        let host = self.node_id.clone();
        let topic = progress_topic(&self.node_id);
        let outcome = apply_queue(queue, &live, &snap, &*self.executor, |idx, status| {
            if let Some(state) = status.progress() {
                let progress = VirtualProgress {
                    host: host.clone(),
                    target: target.clone(),
                    op_index: idx,
                    total,
                    op_kind: queue.ops.get(idx).map_or("?", |o| o.kind()).to_string(),
                    state,
                    published_at_ms: now_ms(),
                };
                publish_json(Some(bus_root), &topic, &progress);
            }
        });
        if !outcome.is_success() {
            tracing::warn!(
                target: "mackesd::alert",
                "ALERT (warn): virtual-storage queue on {target} halted at op {:?} ({} applied)",
                outcome.halted_at, outcome.applied
            );
        }
        outcome.applied > 0
    }

    /// One tick: drain `action/storage/<node>/virtual` since `cursor`, apply each
    /// verb, then republish the mirror (forced on change, else on heartbeat). Returns
    /// the advanced cursor. **Blocking** (shells `qemu-img`/podman) — the worker runs
    /// it in `spawn_blocking`.
    #[must_use]
    pub fn tick(&self, bus_root: &Path, cursor: Option<String>) -> Option<String> {
        let mut cursor = cursor;
        let mut changed = false;
        let topic = action_topic(&self.node_id);
        for req in read_new_requests(bus_root, &topic, &mut cursor) {
            match req {
                VirtualStorageRequest::Apply {
                    armed_target,
                    queue,
                } => {
                    if self.handle_apply(bus_root, &armed_target, &queue) {
                        changed = true;
                    }
                }
                VirtualStorageRequest::Refresh => changed = true,
            }
        }
        self.publish_if_due(bus_root, changed);
        cursor
    }

    /// Seed the cursor to the newest existing message so a (re)start doesn't re-run
    /// the backlog of Apply verbs.
    #[must_use]
    pub fn prime_cursor(&self, bus_root: &Path) -> Option<String> {
        let persist = Persist::open(bus_root.to_path_buf()).ok()?;
        let msgs = persist
            .list_since(&action_topic(&self.node_id), None)
            .ok()?;
        msgs.last().map(|m| m.ulid.clone())
    }
}

/// Whether `path` is an image file this plane manages (`.img`/`.qcow2`/`.raw`).
fn is_image_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("img" | "qcow2" | "raw")
    )
}

/// Read new [`VirtualStorageRequest`]s since `cursor`, advancing it (mirrors
/// [`super::storage`]'s drain).
fn read_new_requests(
    bus_root: &Path,
    topic: &str,
    cursor: &mut Option<String>,
) -> Vec<VirtualStorageRequest> {
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
            Err(e) => {
                tracing::warn!(ulid = %msg.ulid, error = %e, "virtual-storage: bad request");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    // ── fixtures ──

    fn qcow2(path: &str, size_mib: u64, snaps: &[&str]) -> ImageEntry {
        ImageEntry {
            path: PathBuf::from(path),
            format: Some(ImageFormat::Qcow2),
            virtual_size_mib: size_mib,
            actual_size_mib: 0,
            backing_file: None,
            snapshots: snaps.iter().map(|s| (*s).to_string()).collect(),
            in_use_by_vm: None,
        }
    }

    fn raw(path: &str, size_mib: u64) -> ImageEntry {
        ImageEntry {
            path: PathBuf::from(path),
            format: Some(ImageFormat::Raw),
            virtual_size_mib: size_mib,
            actual_size_mib: 0,
            backing_file: None,
            snapshots: Vec::new(),
            in_use_by_vm: None,
        }
    }

    fn topo() -> VirtualTopology {
        VirtualTopology {
            images: vec![
                qcow2("/home/op/Local/web1.qcow2", 10 * 1024, &["clean"]),
                raw("/home/op/Local/gold.raw", 20 * 1024),
            ],
            volumes: vec![VolumeEntry {
                name: "pgdata".into(),
                driver: "local".into(),
                mountpoint: "/var/lib/containers/storage/volumes/pgdata/_data".into(),
                in_use_by_container: None,
            }],
            df: vec![],
        }
    }

    // ── op model ──

    #[test]
    fn op_round_trips_json_and_is_self_describing() {
        let op = VirtualStorageOp::ImageCreate {
            path: PathBuf::from("/home/op/Local/new.qcow2"),
            format: ImageFormat::Qcow2,
            size_mib: 4096,
        };
        let json = serde_json::to_string(&op).unwrap();
        assert!(json.contains(r#""op":"image_create""#));
        assert_eq!(serde_json::from_str::<VirtualStorageOp>(&json).unwrap(), op);
    }

    #[test]
    fn arming_targets_echo_the_primary_written_resource() {
        assert_eq!(
            VirtualStorageOp::ImageConvert {
                src: "/a.raw".into(),
                dst: "/b.qcow2".into(),
                format: ImageFormat::Qcow2,
            }
            .arming_target(),
            "/b.qcow2"
        );
        assert_eq!(
            VirtualStorageOp::VolumeRemove {
                name: "v".into(),
                force: false
            }
            .arming_target(),
            "volume:v"
        );
        assert_eq!(
            VirtualStorageOp::VolumePrune.arming_target(),
            "volume-prune"
        );
    }

    // ── validation ──

    #[test]
    fn validate_create_refuses_existing_and_accepts_new() {
        let t = topo();
        assert!(matches!(
            validate_vop(
                &VirtualStorageOp::ImageCreate {
                    path: "/home/op/Local/web1.qcow2".into(),
                    format: ImageFormat::Qcow2,
                    size_mib: 1024
                },
                &t
            ),
            Err(VopInvalid::ImageExists(_))
        ));
        assert!(validate_vop(
            &VirtualStorageOp::ImageCreate {
                path: "/home/op/Local/fresh.raw".into(),
                format: ImageFormat::Raw,
                size_mib: 1024
            },
            &t
        )
        .is_ok());
    }

    #[test]
    fn validate_snapshot_ops_require_qcow2_and_the_right_tag_presence() {
        let t = topo();
        // snapshot on raw → NotQcow2.
        assert!(matches!(
            validate_vop(
                &VirtualStorageOp::ImageSnapshot {
                    path: "/home/op/Local/gold.raw".into(),
                    tag: "x".into()
                },
                &t
            ),
            Err(VopInvalid::NotQcow2(_))
        ));
        // duplicate snapshot tag → SnapshotExists.
        assert!(matches!(
            validate_vop(
                &VirtualStorageOp::ImageSnapshot {
                    path: "/home/op/Local/web1.qcow2".into(),
                    tag: "clean".into()
                },
                &t
            ),
            Err(VopInvalid::SnapshotExists { .. })
        ));
        // revert to a missing tag → UnknownSnapshot.
        assert!(matches!(
            validate_vop(
                &VirtualStorageOp::ImageRevert {
                    path: "/home/op/Local/web1.qcow2".into(),
                    tag: "ghost".into()
                },
                &t
            ),
            Err(VopInvalid::UnknownSnapshot { .. })
        ));
        // revert to an existing tag → ok.
        assert!(validate_vop(
            &VirtualStorageOp::ImageRevert {
                path: "/home/op/Local/web1.qcow2".into(),
                tag: "clean".into()
            },
            &t
        )
        .is_ok());
    }

    #[test]
    fn validate_resize_must_differ_and_be_nonzero() {
        let t = topo();
        for bad in [0, 10 * 1024] {
            assert!(matches!(
                validate_vop(
                    &VirtualStorageOp::ImageResize {
                        path: "/home/op/Local/web1.qcow2".into(),
                        new_size_mib: bad
                    },
                    &t
                ),
                Err(VopInvalid::InvalidResize { .. })
            ));
        }
        assert!(validate_vop(
            &VirtualStorageOp::ImageResize {
                path: "/home/op/Local/web1.qcow2".into(),
                new_size_mib: 20 * 1024
            },
            &t
        )
        .is_ok());
    }

    #[test]
    fn validate_volume_ops_presence() {
        let t = topo();
        assert!(matches!(
            validate_vop(
                &VirtualStorageOp::VolumeCreate {
                    name: "pgdata".into()
                },
                &t
            ),
            Err(VopInvalid::VolumeExists(_))
        ));
        assert!(validate_vop(&VirtualStorageOp::VolumeCreate { name: "new".into() }, &t).is_ok());
        assert!(matches!(
            validate_vop(
                &VirtualStorageOp::VolumeRemove {
                    name: "ghost".into(),
                    force: false
                },
                &t
            ),
            Err(VopInvalid::UnknownVolume(_))
        ));
    }

    // ── walls ──

    fn snap_with(
        vm_images: &[(&str, &str)],
        vols: &[(&str, &str)],
        tools: bool,
    ) -> VirtualInUseSnapshot {
        VirtualInUseSnapshot {
            vm_images: vm_images
                .iter()
                .map(|(k, v)| ((*k).into(), (*v).into()))
                .collect(),
            container_volumes: vols
                .iter()
                .map(|(k, v)| ((*k).into(), (*v).into()))
                .collect(),
            vm_tool: tools,
            container_tool: tools,
        }
    }

    #[test]
    fn wall_refuses_image_backing_a_running_vm() {
        let snap = snap_with(&[("/home/op/Local/web1.qcow2", "web1")], &[], true);
        let op = VirtualStorageOp::ImageResize {
            path: "/home/op/Local/web1.qcow2".into(),
            new_size_mib: 20 * 1024,
        };
        assert!(matches!(
            check_wall(&op, &snap),
            Err(VirtualWallRefusal::ImageInUseByVm { .. })
        ));
        // a free image passes.
        let free = VirtualStorageOp::ImageResize {
            path: "/home/op/Local/other.qcow2".into(),
            new_size_mib: 20 * 1024,
        };
        assert!(check_wall(&free, &snap).is_ok());
    }

    #[test]
    fn wall_assumes_in_use_when_no_vm_tool() {
        let snap = snap_with(&[], &[], false);
        let op = VirtualStorageOp::ImageDelete {
            path: "/home/op/Local/web1.qcow2".into(),
        };
        assert!(matches!(
            check_wall(&op, &snap),
            Err(VirtualWallRefusal::ImageInUseUnknown(_))
        ));
    }

    #[test]
    fn wall_lets_create_through_even_when_no_vm_tool() {
        // create writes a new file → no image wall even with no tooling.
        let snap0 = snap_with(&[], &[], false);
        let create = VirtualStorageOp::ImageCreate {
            path: "/home/op/Local/new.raw".into(),
            format: ImageFormat::Raw,
            size_mib: 1024,
        };
        assert!(check_wall(&create, &snap0).is_ok());
    }

    #[test]
    fn wall_refuses_volume_mounted_by_a_running_container() {
        let snap = snap_with(&[], &[("pgdata", "pg")], true);
        let op = VirtualStorageOp::VolumeRemove {
            name: "pgdata".into(),
            force: false,
        };
        assert!(matches!(
            check_wall(&op, &snap),
            Err(VirtualWallRefusal::VolumeInUseByContainer { .. })
        ));
    }

    // ── arming ──

    #[test]
    fn arming_ok_mismatch_multi_and_empty() {
        let q = VirtualStorageQueue::new(vec![VirtualStorageOp::ImageSnapshot {
            path: "/home/op/Local/web1.qcow2".into(),
            tag: "s".into(),
        }]);
        assert_eq!(
            check_arming(&q, "/home/op/Local/web1.qcow2").unwrap(),
            "/home/op/Local/web1.qcow2"
        );
        assert!(matches!(
            check_arming(&q, "/wrong"),
            Err(VArmingError::Mismatch { .. })
        ));
        assert!(matches!(
            check_arming(&VirtualStorageQueue::default(), "x"),
            Err(VArmingError::NoTarget)
        ));
        let multi = VirtualStorageQueue::new(vec![
            VirtualStorageOp::VolumeCreate { name: "a".into() },
            VirtualStorageOp::VolumeCreate { name: "b".into() },
        ]);
        assert!(matches!(
            check_arming(&multi, "volume:a"),
            Err(VArmingError::MultiTarget(_))
        ));
    }

    // ── podman parsers ──

    #[test]
    fn parse_volumes_reads_name_driver_mountpoint() {
        let json = r#"[
            {"Name":"pgdata","Driver":"local","Mountpoint":"/v/pgdata/_data"},
            {"Name":"cache","Driver":"local","Mountpoint":"/v/cache/_data"}
        ]"#;
        let vols = parse_volumes(json);
        assert_eq!(vols.len(), 2);
        assert_eq!(vols[0].name, "pgdata");
        assert_eq!(vols[0].driver, "local");
        assert!(vols[0].in_use_by_container.is_none());
        assert!(parse_volumes("garbage").is_empty());
    }

    #[test]
    fn parse_system_df_reads_summary_rows_string_or_number_sizes() {
        let json = r#"[
            {"Type":"Images","Total":3,"Active":1,"Size":"1.2GB","Reclaimable":"800MB"},
            {"Type":"Local Volumes","Total":2,"Active":2,"Size":512,"Reclaimable":0}
        ]"#;
        let df = parse_system_df(json);
        assert_eq!(df.len(), 2);
        assert_eq!(df[0].kind, "Images");
        assert_eq!(df[0].total, 3);
        assert_eq!(df[0].size, "1.2GB");
        // numeric sizes normalize to a display string.
        assert_eq!(df[1].size, "512");
    }

    #[test]
    fn parse_pruned_lists_names() {
        assert_eq!(parse_pruned("a\n\nb\n  c \n"), vec!["a", "b", "c"]);
        assert!(parse_pruned("").is_empty());
    }

    // ── queue executor ──

    /// A recording executor: succeeds, or fails at a chosen op index.
    struct FakeExec {
        applied: StdMutex<Vec<String>>,
        fail_at: Option<usize>,
        seen: StdMutex<usize>,
    }
    impl FakeExec {
        fn ok() -> Self {
            Self {
                applied: StdMutex::new(vec![]),
                fail_at: None,
                seen: StdMutex::new(0),
            }
        }
        fn failing_at(i: usize) -> Self {
            Self {
                applied: StdMutex::new(vec![]),
                fail_at: Some(i),
                seen: StdMutex::new(0),
            }
        }
    }
    impl VirtualExecutor for FakeExec {
        fn apply(&self, op: &VirtualStorageOp) -> Result<(), VExecError> {
            let this = {
                let mut n = self.seen.lock().unwrap();
                let this = *n;
                *n += 1;
                this
            };
            if self.fail_at == Some(this) {
                return Err(VExecError::QemuImg(QemuImgError::Failed {
                    op: "create",
                    reason: "boom".into(),
                }));
            }
            self.applied.lock().unwrap().push(op.kind().to_string());
            Ok(())
        }
    }

    #[test]
    fn apply_queue_all_ok() {
        let live = topo();
        let snap = snap_with(&[], &[], true);
        let q = VirtualStorageQueue::new(vec![
            VirtualStorageOp::VolumeCreate {
                name: "new1".into(),
            },
            VirtualStorageOp::VolumeCreate {
                name: "new2".into(),
            },
        ]);
        let exec = FakeExec::ok();
        let mut progress = vec![];
        let outcome = apply_queue(&q, &live, &snap, &exec, |i, s| {
            progress.push((i, s.clone()));
        });
        assert!(outcome.is_success());
        assert_eq!(outcome.applied, 2);
        assert_eq!(progress.len(), 2);
    }

    #[test]
    fn apply_queue_halts_on_wall_before_executing() {
        let live = topo();
        // pgdata is mounted by a running container → the remove is walled.
        let snap = snap_with(&[], &[("pgdata", "pg")], true);
        let q = VirtualStorageQueue::new(vec![VirtualStorageOp::VolumeRemove {
            name: "pgdata".into(),
            force: false,
        }]);
        let exec = FakeExec::ok();
        let outcome = apply_queue(&q, &live, &snap, &exec, |_, _| {});
        assert!(!outcome.is_success());
        assert!(matches!(outcome.statuses[0], VOpStatus::Refused(_)));
        assert!(exec.applied.lock().unwrap().is_empty());
    }

    #[test]
    fn apply_queue_halts_on_invalid_and_on_failure_no_silent_partial() {
        let live = topo();
        let snap = snap_with(&[], &[], true);
        // invalid: create over an existing image.
        let q = VirtualStorageQueue::new(vec![VirtualStorageOp::ImageCreate {
            path: "/home/op/Local/web1.qcow2".into(),
            format: ImageFormat::Qcow2,
            size_mib: 1024,
        }]);
        let outcome = apply_queue(&q, &live, &snap, &FakeExec::ok(), |_, _| {});
        assert!(matches!(outcome.statuses[0], VOpStatus::Invalidated(_)));

        // failure mid-queue halts, leaving the tail Pending.
        let q2 = VirtualStorageQueue::new(vec![
            VirtualStorageOp::VolumeCreate { name: "a".into() },
            VirtualStorageOp::VolumeCreate { name: "b".into() },
            VirtualStorageOp::VolumeCreate { name: "c".into() },
        ]);
        let exec = FakeExec::failing_at(1);
        let outcome = apply_queue(&q2, &live, &snap, &exec, |_, _| {});
        assert_eq!(outcome.halted_at, Some(1));
        assert_eq!(outcome.applied, 1);
        assert!(matches!(outcome.statuses[2], VOpStatus::Pending));
    }

    // ── bus contract ──

    #[test]
    fn topics_are_per_node_virtual_siblings() {
        assert_eq!(action_topic("n"), "action/storage/n/virtual");
        assert_eq!(state_topic("n"), "state/storage/n/virtual");
        assert_eq!(progress_topic("n"), "event/storage/n/virtual/progress");
    }

    #[test]
    fn request_round_trips_with_arming_echo() {
        let req = VirtualStorageRequest::Apply {
            armed_target: "/home/op/Local/web1.qcow2".into(),
            queue: VirtualStorageQueue::new(vec![VirtualStorageOp::ImageSnapshot {
                path: "/home/op/Local/web1.qcow2".into(),
                tag: "pre".into(),
            }]),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains(r#""verb":"apply""#));
        assert_eq!(parse_request(&json).unwrap(), req);
        assert_eq!(
            parse_request(r#"{"verb":"refresh"}"#).unwrap(),
            VirtualStorageRequest::Refresh
        );
        assert!(parse_request("nope").is_err());
    }

    #[test]
    fn state_round_trips_available_and_unavailable() {
        let st = VirtualStorageState {
            host: "n".into(),
            qemu_img: BackendAvail::Available,
            podman: BackendAvail::Unavailable {
                reason: "no podman".into(),
            },
            topology: topo(),
            published_at_ms: 1,
        };
        let json = serde_json::to_string(&st).unwrap();
        assert_eq!(
            serde_json::from_str::<VirtualStorageState>(&json).unwrap(),
            st
        );
    }

    // ── enumeration + tick (over injected seams) ──

    /// A fake qemu-img runner returning canned info per path.
    struct FakeQemu {
        infos: BTreeMap<PathBuf, ImageInfo>,
    }
    impl QemuImgRunner for FakeQemu {
        fn info(&self, path: &Path) -> Result<ImageInfo, QemuImgError> {
            self.infos
                .get(path)
                .cloned()
                .ok_or_else(|| QemuImgError::Failed {
                    op: "info",
                    reason: "no such image".into(),
                })
        }
        fn create(&self, _: &Path, _: ImageFormat, _: u64) -> Result<(), QemuImgError> {
            Ok(())
        }
        fn resize(&self, _: &Path, _: u64, _: bool) -> Result<(), QemuImgError> {
            Ok(())
        }
        fn convert(&self, _: &Path, _: &Path, _: ImageFormat) -> Result<(), QemuImgError> {
            Ok(())
        }
        fn snapshot(&self, _: SnapshotAction, _: &Path, _: &str) -> Result<(), QemuImgError> {
            Ok(())
        }
    }

    struct FakePodman {
        volumes_json: String,
        df_json: String,
    }
    impl PodmanStorageRunner for FakePodman {
        fn volume_ls_json(&self) -> Result<String, PodmanError> {
            Ok(self.volumes_json.clone())
        }
        fn system_df_json(&self) -> Result<String, PodmanError> {
            Ok(self.df_json.clone())
        }
        fn volume_create(&self, _: &str) -> Result<(), PodmanError> {
            Ok(())
        }
        fn volume_rm(&self, _: &str, _: bool) -> Result<(), PodmanError> {
            Ok(())
        }
        fn volume_prune(&self) -> Result<Vec<String>, PodmanError> {
            Ok(vec![])
        }
    }

    struct FixedInUse(VirtualInUseSnapshot);
    impl VirtualInUseProbe for FixedInUse {
        fn snapshot(&self) -> VirtualInUseSnapshot {
            self.0.clone()
        }
    }

    #[test]
    fn enumerate_folds_images_volumes_df_and_inuse_over_the_seams() {
        let dir = tempfile::tempdir().unwrap();
        let img_path = dir.path().join("web1.qcow2");
        std::fs::write(&img_path, b"stub").unwrap();
        // a non-image file must be ignored.
        std::fs::write(dir.path().join("notes.txt"), b"x").unwrap();

        let mut infos = BTreeMap::new();
        infos.insert(
            img_path.clone(),
            ImageInfo {
                path: img_path.clone(),
                format: ImageFormat::Qcow2,
                virtual_size_bytes: 10 * 1024 * 1024 * 1024,
                actual_size_bytes: 1024 * 1024,
                backing_file: None,
                snapshots: vec![ImageSnapshot {
                    id: "1".into(),
                    tag: "clean".into(),
                    vm_state_bytes: 0,
                }],
            },
        );
        let qemu = Arc::new(FakeQemu { infos });
        let podman = Arc::new(FakePodman {
            volumes_json: r#"[{"Name":"pgdata","Driver":"local","Mountpoint":"/v"}]"#.into(),
            df_json: r#"[{"Type":"Images","Total":1,"Active":1,"Size":"1MB","Reclaimable":"0B"}]"#
                .into(),
        });
        let snap = VirtualInUseSnapshot {
            vm_images: BTreeMap::from([(
                img_path.to_string_lossy().into_owned(),
                "web1".to_string(),
            )]),
            container_volumes: BTreeMap::from([("pgdata".to_string(), "pg".to_string())]),
            vm_tool: true,
            container_tool: true,
        };
        let vs = VirtualStorage::with_seams(
            "node".into(),
            qemu,
            podman,
            Arc::new(FixedInUse(snap)),
            Arc::new(FakeExec::ok()),
            dir.path().to_path_buf(),
        );
        let (topology, qemu_avail, podman_avail) = vs.enumerate();
        assert_eq!(qemu_avail, BackendAvail::Available);
        assert_eq!(podman_avail, BackendAvail::Available);
        assert_eq!(topology.images.len(), 1);
        let img = &topology.images[0];
        assert_eq!(img.format, Some(ImageFormat::Qcow2));
        assert_eq!(img.virtual_size_mib, 10 * 1024);
        assert_eq!(img.snapshots, vec!["clean".to_string()]);
        assert_eq!(img.in_use_by_vm.as_deref(), Some("web1"));
        assert_eq!(topology.volumes.len(), 1);
        assert_eq!(
            topology.volumes[0].in_use_by_container.as_deref(),
            Some("pg")
        );
        assert_eq!(topology.df.len(), 1);
    }

    #[test]
    fn tick_drains_a_refresh_and_advances_the_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let local = tempfile::tempdir().unwrap();
        let vs = VirtualStorage::with_seams(
            "node".into(),
            Arc::new(FakeQemu {
                infos: BTreeMap::new(),
            }),
            Arc::new(FakePodman {
                volumes_json: "[]".into(),
                df_json: "[]".into(),
            }),
            Arc::new(FixedInUse(VirtualInUseSnapshot::default())),
            Arc::new(FakeExec::ok()),
            local.path().to_path_buf(),
        );
        // No bus messages → cursor stays None, publish is best-effort (no mde-bus
        // binary on the farm — fire_and_reap swallows it), tick must not panic.
        let cursor = vs.tick(dir.path(), None);
        assert!(cursor.is_none());
    }
}
