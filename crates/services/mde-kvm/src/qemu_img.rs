//! `qemu-img` image-management seam (E12-22 — virtual disks first-class).
//!
//! The KVM-image half of the Workbench Storage plane's virtual-disk lifecycle
//! (`docs/design/workbench-storage-plane.md`, lock 10). Where cloud-hypervisor
//! ([`crate::Vm`]) owns the *running* guest, this seam owns the disk **image at
//! rest** in `~/Local`: create (raw/qcow2), resize, snapshot/revert/delete-snapshot,
//! convert raw⇄qcow2, and clone-from-golden — every one a `qemu-img` sub-command.
//!
//! ## Shape (mirrors the mixer/ddc `pw-dump`/`ddcutil` runner seam)
//!
//! - **The I/O is a narrow typed runner.** [`QemuImgRunner`] is the only door to a
//!   process; production [`LiveQemuImg`] shells the real `qemu-img` through a
//!   bounded exec (no new deps — a std poll loop, like the mesh workers' EFF-20
//!   path), tests inject a fake. A host with no `qemu-img` answers a typed
//!   [`QemuImgError::Unavailable`]; a `qemu-img` that runs but fails answers
//!   [`QemuImgError::Failed`] — **never a fake success** (§7).
//! - **The pure core is fully unit-tested with no process.** The argv builders
//!   ([`create_argv`] / [`resize_argv`] / [`convert_argv`] / [`snapshot_argv`] /
//!   [`info_argv`]) and the `qemu-img info --output=json` fold
//!   ([`parse_image_info`]) are pure, so the whole mapping is checked headless.
//! - **Live is integration-gated.** Actually creating/snapshotting/reverting a real
//!   image needs the `qemu-img` binary + a real file on a real filesystem — none on
//!   the build farm — so the live qcow2 snapshot/revert + golden-clone-boot is
//!   exercised only against a real host; the headless answer is the honest
//!   [`QemuImgError::Unavailable`], never a fabricated result.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use thiserror::Error;

/// The disk-image format `qemu-img` writes — the two E12-22 first-classes.
///
/// Lock 10 (convert raw⇄qcow2): raw is cloud-hypervisor-native (the running disk,
/// lock 18); qcow2 carries internal snapshots (the snapshot/revert lifecycle).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageFormat {
    /// A raw disk image (cloud-hypervisor-native; no internal snapshots).
    Raw,
    /// A qcow2 image (copy-on-write; carries internal snapshots).
    Qcow2,
}

impl ImageFormat {
    /// The canonical `qemu-img -f`/`-O` format id (`raw` / `qcow2`).
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

    /// Whether this format carries internal (qcow2) snapshots — the snapshot/revert
    /// ops are refused on a format that can't hold them.
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
    /// The snapshot tag/name (what revert/delete address).
    pub tag: String,
    /// The saved VM-state size in bytes (0 for a disk-only snapshot).
    #[serde(default)]
    pub vm_state_bytes: u64,
}

/// The slice of `qemu-img info --output=json` the plane consumes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ImageInfo {
    /// The image path this info describes.
    pub path: PathBuf,
    /// The image format (`raw`/`qcow2`).
    pub format: ImageFormat,
    /// The logical (guest-visible) size in bytes.
    pub virtual_size_bytes: u64,
    /// The on-disk (allocated) size in bytes.
    #[serde(default)]
    pub actual_size_bytes: u64,
    /// The backing file, when this is an overlay (a thin clone).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backing_file: Option<String>,
    /// The internal snapshots (qcow2), in `qemu-img` order.
    #[serde(default)]
    pub snapshots: Vec<ImageSnapshot>,
}

impl ImageInfo {
    /// The logical size in MiB (the queue's granularity).
    #[must_use]
    pub const fn virtual_size_mib(&self) -> u64 {
        self.virtual_size_bytes / (1024 * 1024)
    }

    /// Whether a snapshot tagged `tag` exists.
    #[must_use]
    pub fn has_snapshot(&self, tag: &str) -> bool {
        self.snapshots.iter().any(|s| s.tag == tag)
    }
}

/// A typed `qemu-img` failure.
///
/// Mirrors [`crate::VirtiofsError`] — a real method returning a real typed error,
/// never a fake success.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum QemuImgError {
    /// No `qemu-img` on this host (not installed / not on `PATH`) — the plane
    /// renders the honest unavailable state (§7 gating).
    #[error("qemu-img unavailable: {0}")]
    Unavailable(String),
    /// `qemu-img` ran but the op failed (carries the sub-command + stderr).
    #[error("qemu-img {op} failed: {reason}")]
    Failed {
        /// The op that failed (`create`/`resize`/…).
        op: &'static str,
        /// `qemu-img`'s error text.
        reason: String,
    },
    /// `qemu-img info` output didn't parse.
    #[error("qemu-img info parse: {0}")]
    Parse(String),
}

/// The `qemu-img` I/O seam.
///
/// Create/resize/convert/snapshot an image + read its info. Production is
/// [`LiveQemuImg`]; tests inject a fake so the whole image-lifecycle folds
/// headless (mirrors [`crate::ChTransport`] / `DdcRunner`).
pub trait QemuImgRunner: Send + Sync {
    /// `qemu-img info --output=json <path>` → the parsed [`ImageInfo`].
    ///
    /// # Errors
    /// [`QemuImgError::Unavailable`] when `qemu-img` is absent, `Failed` on a
    /// non-zero exit, `Parse` on unreadable output.
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

    /// `qemu-img convert -O <dst_format> <src> <dst>` — a full, independent copy
    /// (the raw⇄qcow2 convert **and** the golden-clone: a fresh flat copy, never a
    /// backing overlay, so the running disk is self-contained, lock 18).
    ///
    /// # Errors
    /// A [`QemuImgError`].
    fn convert(&self, src: &Path, dst: &Path, dst_format: ImageFormat) -> Result<(), QemuImgError>;

    /// `qemu-img snapshot -c <tag> <path>` (create), `-a` (apply/revert), or `-d`
    /// (delete), per `action`.
    ///
    /// # Errors
    /// A [`QemuImgError`].
    fn snapshot(&self, action: SnapshotAction, path: &Path, tag: &str) -> Result<(), QemuImgError>;
}

/// Which `qemu-img snapshot` sub-verb to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotAction {
    /// `-c` — create a snapshot.
    Create,
    /// `-a` — apply (revert to) a snapshot.
    Apply,
    /// `-d` — delete a snapshot.
    Delete,
}

impl SnapshotAction {
    /// The `qemu-img snapshot` flag (`-c`/`-a`/`-d`).
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

// ───────────────────────────── pure argv builders ─────────────────────────────

/// A MiB size as `qemu-img`'s `<N>M` suffix argument.
fn mib_arg(mib: u64) -> String {
    format!("{mib}M")
}

/// `info --output=json --force-share <path>` — the argv after the `qemu-img`
/// program. `--force-share` lets info read an image a VM may have open (read-only).
#[must_use]
pub fn info_argv(path: &Path) -> Vec<String> {
    vec![
        "info".into(),
        "--output=json".into(),
        "--force-share".into(),
        path.to_string_lossy().into_owned(),
    ]
}

/// `create -f <format> <path> <size>M`.
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

/// `resize [--shrink] <path> <size>M`. `--shrink` is required by `qemu-img` to
/// authorize a shrink; it must NOT be passed on a grow.
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

/// `convert -O <dst_format> <src> <dst>` — a full flat copy (no `-B` backing).
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

/// `snapshot <-c|-a|-d> <tag> <path>`.
#[must_use]
pub fn snapshot_argv(action: SnapshotAction, path: &Path, tag: &str) -> Vec<String> {
    vec![
        "snapshot".into(),
        action.flag().into(),
        tag.to_string(),
        path.to_string_lossy().into_owned(),
    ]
}

/// Parse a `qemu-img info --output=json` body for `path` into an [`ImageInfo`].
///
/// Reads `format`, `virtual-size`, `actual-size`, `backing-filename`, and the
/// `snapshots` array (`name`/`id`/`vm-state-size`). Pure.
///
/// # Errors
/// [`QemuImgError::Parse`] on malformed JSON, a missing/unknown `format`, or a
/// missing `virtual-size`.
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

// ───────────────────────────── live runner ─────────────────────────────

/// The default per-invocation `qemu-img` timeout — generous for a large convert on
/// a slow disk, short enough that a wedged child frees the worker thread.
pub const DEFAULT_QEMU_IMG_TIMEOUT: Duration = Duration::from_secs(120);

/// Production [`QemuImgRunner`]: shells the real `qemu-img` bounded by a deadline.
///
/// Dependency-light — a std `try_wait` poll loop, no tokio (mirrors the mesh
/// workers' EFF-20 blocking path). A missing binary degrades to a typed
/// [`QemuImgError::Unavailable`] (§7), never a panic or a fake success. The live
/// snapshot/revert + golden-clone-boot is exercised only against a real host with
/// `qemu-img` + a real image; the build farm answers `Unavailable`.
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

    /// Override the per-invocation timeout (tests / tuning).
    #[must_use]
    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Run `qemu-img <argv>` bounded, returning stdout on success. A spawn failure
    /// (no binary) is [`QemuImgError::Unavailable`]; a non-zero exit is
    /// [`QemuImgError::Failed`] carrying stderr.
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
                    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_round_trips_and_snapshot_capability() {
        for f in [ImageFormat::Raw, ImageFormat::Qcow2] {
            assert_eq!(ImageFormat::from_qemu(f.as_str()), Some(f));
        }
        assert_eq!(ImageFormat::from_qemu("QCOW2"), Some(ImageFormat::Qcow2));
        assert_eq!(ImageFormat::from_qemu("vmdk"), None);
        assert!(ImageFormat::Qcow2.supports_snapshots());
        assert!(!ImageFormat::Raw.supports_snapshots());
    }

    #[test]
    fn argv_builders_are_exact() {
        let p = Path::new("/home/op/Local/web1.img");
        assert_eq!(
            create_argv(p, ImageFormat::Qcow2, 4096),
            vec!["create", "-f", "qcow2", "/home/op/Local/web1.img", "4096M"]
        );
        // grow: no --shrink; shrink: --shrink present.
        assert_eq!(
            resize_argv(p, 8192, false),
            vec!["resize", "/home/op/Local/web1.img", "8192M"]
        );
        assert_eq!(
            resize_argv(p, 2048, true),
            vec!["resize", "--shrink", "/home/op/Local/web1.img", "2048M"]
        );
        assert_eq!(
            convert_argv(Path::new("/gold.raw"), p, ImageFormat::Raw),
            vec![
                "convert",
                "-O",
                "raw",
                "/gold.raw",
                "/home/op/Local/web1.img"
            ]
        );
        assert_eq!(
            snapshot_argv(SnapshotAction::Create, p, "pre-update"),
            vec!["snapshot", "-c", "pre-update", "/home/op/Local/web1.img"]
        );
        assert_eq!(
            snapshot_argv(SnapshotAction::Apply, p, "s")
                .first()
                .unwrap(),
            "snapshot"
        );
        assert_eq!(snapshot_argv(SnapshotAction::Apply, p, "s")[1], "-a");
        assert_eq!(snapshot_argv(SnapshotAction::Delete, p, "s")[1], "-d");
        assert_eq!(
            info_argv(p),
            vec![
                "info",
                "--output=json",
                "--force-share",
                "/home/op/Local/web1.img"
            ]
        );
    }

    #[test]
    fn parse_info_reads_format_sizes_backing_and_snapshots() {
        let json = r#"{
            "virtual-size": 10737418240,
            "filename": "/home/op/Local/web1.qcow2",
            "format": "qcow2",
            "actual-size": 2147483648,
            "backing-filename": "/home/op/Local/golden.raw",
            "snapshots": [
                {"id": "1", "name": "pre-update", "vm-state-size": 0},
                {"id": "2", "name": "clean", "vm-state-size": 1048576}
            ]
        }"#;
        let info = parse_image_info(json, Path::new("/home/op/Local/web1.qcow2")).unwrap();
        assert_eq!(info.format, ImageFormat::Qcow2);
        assert_eq!(info.virtual_size_bytes, 10_737_418_240);
        assert_eq!(info.virtual_size_mib(), 10 * 1024);
        assert_eq!(info.actual_size_bytes, 2_147_483_648);
        assert_eq!(
            info.backing_file.as_deref(),
            Some("/home/op/Local/golden.raw")
        );
        assert_eq!(info.snapshots.len(), 2);
        assert!(info.has_snapshot("pre-update"));
        assert!(!info.has_snapshot("nope"));
        assert_eq!(info.snapshots[1].vm_state_bytes, 1_048_576);
    }

    #[test]
    fn parse_info_raw_has_no_snapshots() {
        let json = r#"{"virtual-size": 1048576, "format": "raw", "actual-size": 4096}"#;
        let info = parse_image_info(json, Path::new("/d.raw")).unwrap();
        assert_eq!(info.format, ImageFormat::Raw);
        assert!(info.snapshots.is_empty());
        assert!(info.backing_file.is_none());
    }

    #[test]
    fn parse_info_rejects_garbage_and_unknown_format() {
        assert!(matches!(
            parse_image_info("not json", Path::new("/x")),
            Err(QemuImgError::Parse(_))
        ));
        assert!(matches!(
            parse_image_info(r#"{"format":"vmdk","virtual-size":1}"#, Path::new("/x")),
            Err(QemuImgError::Parse(_))
        ));
    }

    #[test]
    fn info_round_trips_serde() {
        let info = ImageInfo {
            path: PathBuf::from("/d.qcow2"),
            format: ImageFormat::Qcow2,
            virtual_size_bytes: 1024 * 1024 * 1024,
            actual_size_bytes: 4096,
            backing_file: None,
            snapshots: vec![ImageSnapshot {
                id: "1".into(),
                tag: "s".into(),
                vm_state_bytes: 0,
            }],
        };
        let json = serde_json::to_string(&info).unwrap();
        assert_eq!(serde_json::from_str::<ImageInfo>(&json).unwrap(), info);
    }

    #[test]
    fn live_runner_is_honest_when_qemu_img_is_absent_or_the_file_is_missing() {
        // On a headless host the answer is a typed error (Unavailable when the
        // binary is absent, Failed when qemu-img runs but the image is missing) —
        // never a fabricated ImageInfo (§7).
        let live = LiveQemuImg::new().with_timeout(Duration::from_secs(5));
        let res = live.info(Path::new("/nonexistent/e12-22-does-not-exist.qcow2"));
        assert!(
            matches!(
                res,
                Err(QemuImgError::Unavailable(_) | QemuImgError::Failed { .. })
            ),
            "expected a typed error, got {res:?}"
        );
    }
}
