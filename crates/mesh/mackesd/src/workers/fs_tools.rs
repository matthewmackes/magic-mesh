//! E12-23 — **filesystem depth**: the typed fs-tooling verb layer for the Storage
//! plane's physical executor ([`super::storage::UDisks2Executor`]).
//!
//! Where [`super::storage`] owns the typed op queue, walls, arming and drift, THIS
//! module is the depth under [`super::storage::StorageOp`]'s format/label/resize/LUKS/
//! subvolume verbs: the **honest per-fs capability matrix** (lock 6), the pure
//! **shrink/move choreography** state machine (lock 4 — check → fs-shrink →
//! part-shrink, ordered, each a step, halting typed on the first failure with no
//! silent partial), and the injectable **typed verb layer** ([`FsToolRunner`]) the
//! executor drives instead of raw shell (§9).
//!
//! ## Shape (mirrors `mde_kvm::qemu_img`'s runner seam)
//!
//! - **The pure core is fully unit-tested with no process.** [`FsCapabilities`] (the
//!   real support matrix per fs), the argv builders ([`ToolCmd`]), and the
//!   choreography planner ([`resize_plan`]) / executor ([`run_plan`]) are pure, so
//!   the whole mapping + the mid-failure state machine fold headless with fakes.
//! - **The I/O is a narrow typed runner.** [`FsToolRunner`] is the only door to a
//!   process; production [`LiveFsTools`] shells the real tools (`mkfs.*`,
//!   `resize2fs`, `xfs_growfs`, `btrfs`, `ntfsresize`, `cryptsetup`, `parted`, …)
//!   bounded by the EFF-20 timeout. A host without a tool answers a typed
//!   [`FsToolError::Unavailable`]; a tool that runs but fails answers
//!   [`FsToolError::Failed`] — **never a fake success** (§7). A LUKS op with no
//!   keyfile answers [`FsToolError::IntegrationGated`] (an interactive passphrase
//!   must never ride the Bus).
//! - **Honest capability states.** A fs that cannot shrink (xfs / exfat / swap)
//!   reports [`CapabilityRefusal::ShrinkUnsupported`] — a typed state the queue
//!   halts on, never a silent no-op.

#![cfg(feature = "async-services")]

use std::path::{Path, PathBuf};

use thiserror::Error;

use super::proc::{output_with_timeout, DEFAULT_CMD_TIMEOUT};
use super::storage::Filesystem;

// ───────────────────────────── capability matrix (lock 6) ─────────────────────────────

/// Whether a filesystem supports a resize, and if so whether it must be done online
/// (mounted) or offline (unmounted) — the honest per-fs state (lock 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResizeSupport {
    /// Resizable while mounted (the tool operates on the live mount point).
    Online,
    /// Resizable only while unmounted (the tool operates on the block device).
    Offline,
    /// The filesystem cannot be resized this direction — a typed honest state,
    /// never a silent no-op.
    Unsupported,
}

impl ResizeSupport {
    /// Whether the direction is possible at all.
    #[must_use]
    pub const fn is_supported(self) -> bool {
        !matches!(self, Self::Unsupported)
    }
}

/// The real support matrix for one filesystem (lock 6). Every field is the *honest*
/// capability of the shipped tooling — an unsupported op is a typed refusal, not a
/// pretend success.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FsCapabilities {
    /// The filesystem this describes.
    pub fs: Filesystem,
    /// Can be created (`mkfs.*` / `mkswap` / `cryptsetup luksFormat`).
    pub format: bool,
    /// Carries a settable label.
    pub label: bool,
    /// Grow support (and online/offline requirement).
    pub grow: ResizeSupport,
    /// Shrink support (and online/offline requirement).
    pub shrink: ResizeSupport,
    /// Holds btrfs-style subvolumes (list/create/delete/snapshot).
    pub subvolumes: bool,
}

impl Filesystem {
    /// The honest capability matrix for this filesystem (lock 6).
    ///
    /// The real tooling truth, not aspirational:
    /// - **ext4** grows online (`resize2fs` on a mount), shrinks offline.
    /// - **xfs** grows online (`xfs_growfs`) but **cannot shrink at all** — the
    ///   canonical honest-state case.
    /// - **vfat/ntfs** resize offline (`fatresize` / `ntfsresize`).
    /// - **btrfs** grows *and* shrinks online (`btrfs filesystem resize`), and holds
    ///   subvolumes.
    /// - **exfat** and **swap** cannot be resized in place at all (recreate).
    /// - **luks** is a container — resize is the inner fs's concern.
    #[must_use]
    pub const fn capabilities(self) -> FsCapabilities {
        let (grow, shrink, subvolumes) = match self {
            Self::Ext4 => (ResizeSupport::Online, ResizeSupport::Offline, false),
            Self::Xfs => (ResizeSupport::Online, ResizeSupport::Unsupported, false),
            Self::Vfat | Self::Ntfs => (ResizeSupport::Offline, ResizeSupport::Offline, false),
            Self::Btrfs => (ResizeSupport::Online, ResizeSupport::Online, true),
            // exfat + swap can't resize in place (recreate); luks is a container
            // (its inner fs owns resize) — all three report Unsupported both ways.
            Self::Exfat | Self::Swap | Self::Luks => (
                ResizeSupport::Unsupported,
                ResizeSupport::Unsupported,
                false,
            ),
        };
        FsCapabilities {
            fs: self,
            format: true,
            label: true,
            grow,
            shrink,
            subvolumes,
        }
    }
}

/// A typed capability refusal — an op the target filesystem honestly can't do (lock
/// 6). Surfaced as a queue halt state, never a silent no-op.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CapabilityRefusal {
    /// The filesystem cannot shrink (xfs / exfat / swap).
    #[error("{fs} filesystems cannot shrink in place — no shrink tool exists")]
    ShrinkUnsupported {
        /// The filesystem.
        fs: Filesystem,
    },
    /// The filesystem cannot grow in place (exfat / swap).
    #[error("{fs} filesystems cannot grow in place — recreate at the new size")]
    GrowUnsupported {
        /// The filesystem.
        fs: Filesystem,
    },
    /// The resize target references a partition whose filesystem couldn't be
    /// determined — refused rather than guessing (data-loss safe default).
    #[error("cannot {operation} {partition}: its filesystem is unknown to the plane")]
    UnknownFilesystem {
        /// The partition.
        partition: String,
        /// The attempted operation.
        operation: &'static str,
    },
    /// A subvolume op on a non-btrfs filesystem.
    #[error("subvolumes need btrfs; {partition} is {found}")]
    NotBtrfs {
        /// The partition.
        partition: String,
        /// The filesystem it actually carries.
        found: String,
    },
}

impl std::fmt::Display for Filesystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ───────────────────────────── typed verb layer (§9) ─────────────────────────────

/// A resize direction — grow or shrink (chooses the tool's flag + the choreography
/// order).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeDirection {
    /// Enlarge (partition-grow first, then fs-grow).
    Grow,
    /// Reduce (fs-check → fs-shrink → partition-shrink).
    Shrink,
}

/// A concrete tool invocation — the program plus its argv, built purely so the exact
/// command line is unit-testable (no `Command` in the pure core).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCmd {
    /// The program to spawn (`mkfs.ext4`, `resize2fs`, `cryptsetup`, …).
    pub program: String,
    /// The arguments after the program.
    pub args: Vec<String>,
}

impl ToolCmd {
    fn new(program: impl Into<String>, args: &[&str]) -> Self {
        Self {
            program: program.into(),
            args: args.iter().map(|s| (*s).to_string()).collect(),
        }
    }
}

/// A MiB size as the tool's `<N>M` suffix argument.
fn mib(m: u64) -> String {
    format!("{m}M")
}

/// The `mkfs`/`mkswap`/`cryptsetup luksFormat` command for `fs` on `device`, with an
/// optional label. Pure.
///
/// The per-fs label flag differs (`-n` for vfat, `-L` elsewhere, `swaplabel`/`mkswap
/// -L` for swap); `mkfs.xfs`/`mkfs.btrfs`/`mkfs.ntfs` take a force flag so a
/// re-format of an existing signature doesn't wedge on a prompt.
#[must_use]
pub fn mkfs_cmd(fs: Filesystem, device: &Path, label: Option<&str>) -> ToolCmd {
    let dev = device.to_string_lossy().into_owned();
    let mut args: Vec<String> = Vec::new();
    let program = match fs {
        Filesystem::Ext4 => {
            args.push("-F".into());
            "mkfs.ext4"
        }
        Filesystem::Xfs => {
            args.push("-f".into());
            "mkfs.xfs"
        }
        Filesystem::Vfat => "mkfs.vfat",
        Filesystem::Exfat => "mkfs.exfat",
        Filesystem::Btrfs => {
            args.push("-f".into());
            "mkfs.btrfs"
        }
        Filesystem::Ntfs => {
            args.push("--fast".into());
            args.push("--force".into());
            "mkfs.ntfs"
        }
        Filesystem::Swap => "mkswap",
        // LUKS is not an mkfs target — cryptsetup handles it via luks_format_cmd.
        Filesystem::Luks => "cryptsetup",
    };
    if let Some(l) = label {
        // vfat's label flag is -n; every other tool here uses -L.
        let flag = if matches!(fs, Filesystem::Vfat) {
            "-n"
        } else {
            "-L"
        };
        args.push(flag.into());
        args.push(l.to_string());
    }
    args.push(dev);
    ToolCmd {
        program: program.into(),
        args,
    }
}

/// The set-label command for `fs` on `device`. Pure.
#[must_use]
pub fn set_label_cmd(fs: Filesystem, device: &Path, label: &str) -> ToolCmd {
    let dev = device.to_string_lossy().into_owned();
    match fs {
        Filesystem::Ext4 => ToolCmd::new("e2label", &[&dev, label]),
        Filesystem::Xfs => ToolCmd::new("xfs_admin", &["-L", label, &dev]),
        Filesystem::Vfat => ToolCmd::new("fatlabel", &[&dev, label]),
        Filesystem::Exfat => ToolCmd::new("exfatlabel", &[&dev, label]),
        Filesystem::Btrfs => ToolCmd::new("btrfs", &["filesystem", "label", &dev, label]),
        Filesystem::Ntfs => ToolCmd::new("ntfslabel", &[&dev, label]),
        Filesystem::Swap => ToolCmd::new("swaplabel", &["-L", label, &dev]),
        Filesystem::Luks => ToolCmd::new("cryptsetup", &["config", &dev, "--label", label]),
    }
}

/// The offline consistency-check command for `fs` on `device` — the first
/// shrink-choreography step.
///
/// Pure. `None` for filesystems with no offline checker (swap / luks — the
/// choreography skips the check for them).
#[must_use]
pub fn fs_check_cmd(fs: Filesystem, device: &Path) -> Option<ToolCmd> {
    let dev = device.to_string_lossy().into_owned();
    Some(match fs {
        Filesystem::Ext4 => ToolCmd::new("e2fsck", &["-f", "-y", &dev]),
        Filesystem::Xfs => ToolCmd::new("xfs_repair", &["-n", &dev]),
        Filesystem::Vfat => ToolCmd::new("fsck.vfat", &["-a", &dev]),
        Filesystem::Exfat => ToolCmd::new("fsck.exfat", &[&dev]),
        Filesystem::Btrfs => ToolCmd::new("btrfs", &["check", &dev]),
        Filesystem::Ntfs => ToolCmd::new("ntfsfix", &[&dev]),
        Filesystem::Swap | Filesystem::Luks => return None,
    })
}

/// The filesystem-resize command for `fs` to `new_size_mib`.
///
/// `mountpoint` is required for the online-only tools (`xfs_growfs`, `btrfs
/// filesystem resize`); when absent the builder falls back to the device (the live
/// runner surfaces the tool's own error). Pure. `None` when the fs can't be resized
/// this direction (the capability matrix should have refused earlier — a defensive
/// belt).
#[must_use]
pub fn fs_resize_cmd(
    fs: Filesystem,
    device: &Path,
    mountpoint: Option<&str>,
    new_size_mib: u64,
    direction: ResizeDirection,
) -> Option<ToolCmd> {
    let support = match direction {
        ResizeDirection::Grow => fs.capabilities().grow,
        ResizeDirection::Shrink => fs.capabilities().shrink,
    };
    if !support.is_supported() {
        return None;
    }
    let dev = device.to_string_lossy().into_owned();
    let target = mountpoint.map_or_else(|| dev.clone(), str::to_string);
    Some(match fs {
        // resize2fs takes an explicit size in MiB; on a shrink it needs the fs
        // checked first (the choreography's e2fsck step guarantees that).
        Filesystem::Ext4 => ToolCmd::new("resize2fs", &[&dev, &mib(new_size_mib)]),
        // xfs only grows, and only on a mounted fs (mountpoint), to fill the
        // partition — it takes no explicit size.
        Filesystem::Xfs => ToolCmd::new("xfs_growfs", &[&target]),
        Filesystem::Vfat => ToolCmd::new("fatresize", &["-s", &mib(new_size_mib), &dev]),
        // btrfs resizes online against the mount point, explicit size.
        Filesystem::Btrfs => ToolCmd::new(
            "btrfs",
            &["filesystem", "resize", &mib(new_size_mib), &target],
        ),
        Filesystem::Ntfs => ToolCmd::new("ntfsresize", &["-f", "-s", &mib(new_size_mib), &dev]),
        Filesystem::Exfat | Filesystem::Swap | Filesystem::Luks => return None,
    })
}

/// The partition-geometry resize command (`parted`) — the choreography's part step.
///
/// `parted` addresses the whole `disk` plus the 1-based `number`; `new_size_mib` is
/// the absolute end offset (MiB from the disk head), resolved by the caller. Pure.
#[must_use]
pub fn part_resize_cmd(disk: &Path, number: u32, new_size_mib: u64) -> ToolCmd {
    let disk = disk.to_string_lossy().into_owned();
    // `parted <disk> resizepart <n> <end>` — end is start+size; the runner computes
    // the absolute end. Here we express the new END as an offset the caller resolved
    // into MiB from the disk head (passed as new_size_mib = absolute end MiB).
    ToolCmd::new(
        "parted",
        &[
            "--script",
            &disk,
            "resizepart",
            &number.to_string(),
            &format!("{new_size_mib}MiB"),
        ],
    )
}

/// `cryptsetup luksFormat` with a keyfile. Pure. (No-keyfile is refused by the runner
/// — an interactive passphrase must never ride the Bus.)
#[must_use]
pub fn luks_format_cmd(device: &Path, keyfile: &Path) -> ToolCmd {
    let dev = device.to_string_lossy().into_owned();
    let kf = keyfile.to_string_lossy().into_owned();
    ToolCmd::new(
        "cryptsetup",
        &["luksFormat", "--batch-mode", "--key-file", &kf, &dev],
    )
}

/// `cryptsetup open` (unlock) `device` as `/dev/mapper/<mapper>` with a keyfile. Pure.
#[must_use]
pub fn luks_open_cmd(device: &Path, mapper: &str, keyfile: &Path) -> ToolCmd {
    let dev = device.to_string_lossy().into_owned();
    let kf = keyfile.to_string_lossy().into_owned();
    ToolCmd::new("cryptsetup", &["open", "--key-file", &kf, &dev, mapper])
}

/// `cryptsetup close` (lock) `/dev/mapper/<mapper>`. Pure.
#[must_use]
pub fn luks_close_cmd(mapper: &str) -> ToolCmd {
    ToolCmd::new("cryptsetup", &["close", mapper])
}

/// `btrfs subvolume <verb>` command builders (list/create/delete/snapshot). Pure.
#[must_use]
pub fn subvol_list_cmd(mountpoint: &str) -> ToolCmd {
    ToolCmd::new("btrfs", &["subvolume", "list", mountpoint])
}

/// `btrfs subvolume create <mountpoint>/<name>`. Pure.
#[must_use]
pub fn subvol_create_cmd(mountpoint: &str, name: &str) -> ToolCmd {
    let path = join_subvol(mountpoint, name);
    ToolCmd::new("btrfs", &["subvolume", "create", &path])
}

/// `btrfs subvolume delete <mountpoint>/<name>`. Pure.
#[must_use]
pub fn subvol_delete_cmd(mountpoint: &str, name: &str) -> ToolCmd {
    let path = join_subvol(mountpoint, name);
    ToolCmd::new("btrfs", &["subvolume", "delete", &path])
}

/// `btrfs subvolume snapshot [-r] <mountpoint>/<source> <mountpoint>/<dest>`. Pure.
#[must_use]
pub fn subvol_snapshot_cmd(mountpoint: &str, source: &str, dest: &str, readonly: bool) -> ToolCmd {
    let source_path = join_subvol(mountpoint, source);
    let dest_path = join_subvol(mountpoint, dest);
    let mut args = vec!["subvolume".to_string(), "snapshot".to_string()];
    if readonly {
        args.push("-r".into());
    }
    args.push(source_path);
    args.push(dest_path);
    ToolCmd {
        program: "btrfs".into(),
        args,
    }
}

/// Join a subvolume name under a mount point (a plain path join, no shell).
fn join_subvol(mountpoint: &str, name: &str) -> String {
    Path::new(mountpoint)
        .join(name)
        .to_string_lossy()
        .into_owned()
}

/// Parse `btrfs subvolume list <mp>` stdout into subvolume paths.
///
/// Each line is `ID <id> gen <g> top level <t> path <path>`; we take the `path`
/// tail. Pure.
#[must_use]
pub fn parse_subvol_list(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter_map(|line| line.split(" path ").nth(1).map(|p| p.trim().to_string()))
        .filter(|p| !p.is_empty())
        .collect()
}

// ───────────────────────────── runner seam ─────────────────────────────

/// A typed fs-tooling failure (mirrors `mde_kvm::QemuImgError`).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FsToolError {
    /// The tool isn't installed on this host — the honest §7 gate.
    #[error("{tool} unavailable: {reason}")]
    Unavailable {
        /// The missing tool.
        tool: String,
        /// Why (spawn error).
        reason: String,
    },
    /// The tool ran but the op failed (carries the verb + stderr).
    #[error("{op} failed: {reason}")]
    Failed {
        /// The op verb.
        op: &'static str,
        /// The tool's error text.
        reason: String,
    },
    /// A path that can only be exercised against a real host / a channel the Bus must
    /// not carry (a LUKS interactive passphrase) — never a fake success (§7).
    #[error("integration-gated: {0}")]
    IntegrationGated(String),
}

/// The typed verb layer (§9): the only door the executor uses to touch fs tooling —
/// no raw shell in the executor. Production [`LiveFsTools`] shells the real tools;
/// tests inject a recording fake.
pub trait FsToolRunner: Send + Sync {
    /// Create a filesystem on `device` (optionally labelled).
    ///
    /// # Errors
    /// [`FsToolError`].
    fn mkfs(&self, fs: Filesystem, device: &Path, label: Option<&str>) -> Result<(), FsToolError>;

    /// Set the filesystem label.
    ///
    /// # Errors
    /// [`FsToolError`].
    fn set_label(&self, fs: Filesystem, device: &Path, label: &str) -> Result<(), FsToolError>;

    /// Run the offline consistency check (no-op for fs with no checker).
    ///
    /// # Errors
    /// [`FsToolError`].
    fn fs_check(&self, fs: Filesystem, device: &Path) -> Result<(), FsToolError>;

    /// Resize the filesystem to `new_size_mib` (grow or shrink).
    ///
    /// # Errors
    /// [`FsToolError`].
    fn fs_resize(
        &self,
        fs: Filesystem,
        device: &Path,
        mountpoint: Option<&str>,
        new_size_mib: u64,
        direction: ResizeDirection,
    ) -> Result<(), FsToolError>;

    /// Resize the partition geometry (`parted`) — `new_end_mib` is the absolute end
    /// offset from the disk head.
    ///
    /// # Errors
    /// [`FsToolError`].
    fn part_resize(&self, disk: &Path, number: u32, new_end_mib: u64) -> Result<(), FsToolError>;

    /// `cryptsetup luksFormat` (keyfile required).
    ///
    /// # Errors
    /// [`FsToolError`].
    fn luks_format(&self, device: &Path, keyfile: Option<&Path>) -> Result<(), FsToolError>;

    /// `cryptsetup open` (unlock).
    ///
    /// # Errors
    /// [`FsToolError`].
    fn luks_open(
        &self,
        device: &Path,
        mapper: &str,
        keyfile: Option<&Path>,
    ) -> Result<(), FsToolError>;

    /// `cryptsetup close` (lock).
    ///
    /// # Errors
    /// [`FsToolError`].
    fn luks_close(&self, mapper: &str) -> Result<(), FsToolError>;

    /// List btrfs subvolumes under a mount point.
    ///
    /// # Errors
    /// [`FsToolError`].
    fn subvol_list(&self, mountpoint: &str) -> Result<Vec<String>, FsToolError>;

    /// Create a btrfs subvolume.
    ///
    /// # Errors
    /// [`FsToolError`].
    fn subvol_create(&self, mountpoint: &str, name: &str) -> Result<(), FsToolError>;

    /// Delete a btrfs subvolume.
    ///
    /// # Errors
    /// [`FsToolError`].
    fn subvol_delete(&self, mountpoint: &str, name: &str) -> Result<(), FsToolError>;

    /// Snapshot a btrfs subvolume (`readonly` ⇒ `-r`).
    ///
    /// # Errors
    /// [`FsToolError`].
    fn subvol_snapshot(
        &self,
        mountpoint: &str,
        source: &str,
        dest: &str,
        readonly: bool,
    ) -> Result<(), FsToolError>;
}

/// Production [`FsToolRunner`]: shells the real tools bounded by the EFF-20 timeout.
///
/// A missing tool degrades to [`FsToolError::Unavailable`] (§7 — the build farm has
/// no mkfs.ntfs/cryptsetup for most fs), a non-zero exit to [`FsToolError::Failed`]
/// carrying stderr; a LUKS op with no keyfile is [`FsToolError::IntegrationGated`]
/// (an interactive passphrase must not ride the Bus). Never a fabricated success.
#[derive(Debug, Clone, Default)]
pub struct LiveFsTools;

impl LiveFsTools {
    /// The production runner.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Run one [`ToolCmd`] bounded, mapping a spawn failure to `Unavailable` and a
    /// non-zero exit to `Failed`.
    fn run(&self, op: &'static str, cmd: &ToolCmd) -> Result<String, FsToolError> {
        let _ = self;
        let mut command = std::process::Command::new(&cmd.program);
        command.args(&cmd.args);
        let out = output_with_timeout(command, DEFAULT_CMD_TIMEOUT).map_err(|e| {
            FsToolError::Unavailable {
                tool: cmd.program.clone(),
                reason: e.to_string(),
            }
        })?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(FsToolError::Failed {
                op,
                reason: String::from_utf8_lossy(&out.stderr).trim().to_string(),
            })
        }
    }
}

impl FsToolRunner for LiveFsTools {
    fn mkfs(&self, fs: Filesystem, device: &Path, label: Option<&str>) -> Result<(), FsToolError> {
        if matches!(fs, Filesystem::Luks) {
            return Err(FsToolError::IntegrationGated(
                "luks is not an mkfs target — stage a LuksFormat op".into(),
            ));
        }
        self.run("mkfs", &mkfs_cmd(fs, device, label)).map(|_| ())
    }

    fn set_label(&self, fs: Filesystem, device: &Path, label: &str) -> Result<(), FsToolError> {
        self.run("set_label", &set_label_cmd(fs, device, label))
            .map(|_| ())
    }

    fn fs_check(&self, fs: Filesystem, device: &Path) -> Result<(), FsToolError> {
        // swap/luks have no offline checker ⇒ nothing to do.
        fs_check_cmd(fs, device).map_or(Ok(()), |cmd| self.run("fs_check", &cmd).map(|_| ()))
    }

    fn fs_resize(
        &self,
        fs: Filesystem,
        device: &Path,
        mountpoint: Option<&str>,
        new_size_mib: u64,
        direction: ResizeDirection,
    ) -> Result<(), FsToolError> {
        fs_resize_cmd(fs, device, mountpoint, new_size_mib, direction).map_or_else(
            || {
                Err(FsToolError::IntegrationGated(format!(
                    "{fs} cannot fs-resize {direction:?} — the capability matrix should have refused"
                )))
            },
            |cmd| self.run("fs_resize", &cmd).map(|_| ()),
        )
    }

    fn part_resize(&self, disk: &Path, number: u32, new_end_mib: u64) -> Result<(), FsToolError> {
        self.run("part_resize", &part_resize_cmd(disk, number, new_end_mib))
            .map(|_| ())
    }

    fn luks_format(&self, device: &Path, keyfile: Option<&Path>) -> Result<(), FsToolError> {
        let Some(kf) = keyfile else {
            return Err(FsToolError::IntegrationGated(
                "luks_format needs a keyfile — an interactive passphrase must not ride the Bus"
                    .into(),
            ));
        };
        self.run("luks_format", &luks_format_cmd(device, kf))
            .map(|_| ())
    }

    fn luks_open(
        &self,
        device: &Path,
        mapper: &str,
        keyfile: Option<&Path>,
    ) -> Result<(), FsToolError> {
        let Some(kf) = keyfile else {
            return Err(FsToolError::IntegrationGated(
                "luks_open needs a keyfile — an interactive passphrase must not ride the Bus"
                    .into(),
            ));
        };
        self.run("luks_open", &luks_open_cmd(device, mapper, kf))
            .map(|_| ())
    }

    fn luks_close(&self, mapper: &str) -> Result<(), FsToolError> {
        self.run("luks_close", &luks_close_cmd(mapper)).map(|_| ())
    }

    fn subvol_list(&self, mountpoint: &str) -> Result<Vec<String>, FsToolError> {
        self.run("subvol_list", &subvol_list_cmd(mountpoint))
            .map(|out| parse_subvol_list(&out))
    }

    fn subvol_create(&self, mountpoint: &str, name: &str) -> Result<(), FsToolError> {
        self.run("subvol_create", &subvol_create_cmd(mountpoint, name))
            .map(|_| ())
    }

    fn subvol_delete(&self, mountpoint: &str, name: &str) -> Result<(), FsToolError> {
        self.run("subvol_delete", &subvol_delete_cmd(mountpoint, name))
            .map(|_| ())
    }

    fn subvol_snapshot(
        &self,
        mountpoint: &str,
        source: &str,
        dest: &str,
        readonly: bool,
    ) -> Result<(), FsToolError> {
        self.run(
            "subvol_snapshot",
            &subvol_snapshot_cmd(mountpoint, source, dest, readonly),
        )
        .map(|_| ())
    }
}

// ───────────────────────────── shrink/move choreography (lock 4) ─────────────────────────────

/// One ordered step in a resize choreography (lock 4).
///
/// The order is the safety contract: shrink checks + shrinks the *filesystem* before
/// the *partition* (so the fs never overruns its container); grow does the reverse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChoreoStep {
    /// Offline consistency check (shrink only, first).
    FsCheck {
        /// The filesystem.
        fs: Filesystem,
        /// The partition device.
        device: PathBuf,
    },
    /// Resize the filesystem to `new_size_mib`.
    FsResize {
        /// The filesystem.
        fs: Filesystem,
        /// The partition device.
        device: PathBuf,
        /// The mount point (online tools).
        mountpoint: Option<String>,
        /// The target fs size (MiB).
        new_size_mib: u64,
        /// Grow or shrink.
        direction: ResizeDirection,
    },
    /// Resize the partition geometry to the absolute end offset `new_end_mib`.
    PartResize {
        /// The whole disk.
        disk: PathBuf,
        /// The 1-based partition number.
        number: u32,
        /// The new absolute end offset (MiB from the disk head).
        new_end_mib: u64,
    },
}

impl ChoreoStep {
    /// A short label for progress/logging.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::FsCheck { .. } => "fs-check",
            Self::FsResize { .. } => "fs-resize",
            Self::PartResize { .. } => "part-resize",
        }
    }
}

/// The identity of a partition needed to plan a resize choreography.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResizeTarget {
    /// The partition device (`/dev/sdb1`).
    pub partition: PathBuf,
    /// The whole disk (`/dev/sdb`).
    pub disk: PathBuf,
    /// The 1-based partition number.
    pub number: u32,
    /// The partition's start offset (MiB from the disk head) — the fixed anchor a
    /// resize keeps; the new end is the start plus the new size.
    pub start_mib: u64,
    /// The filesystem the partition carries.
    pub fs: Filesystem,
    /// The mount point, when mounted (online tools).
    pub mountpoint: Option<String>,
}

/// Plan the ordered choreography for resizing `target` to `new_size_mib` (lock 4).
///
/// **Shrink** = fs-check → fs-shrink → part-shrink (the fs shrinks inside its old
/// container first, so a mid-way abort never leaves the fs larger than the
/// partition). **Grow** = part-grow → fs-grow (the container enlarges first, then
/// the fs fills it). The absolute new end = `start_mib + new_size_mib`.
///
/// # Errors
/// A [`CapabilityRefusal`] when the filesystem can't resize this direction (xfs
/// shrink, exfat/swap either way) — a typed honest state, never a silent no-op.
pub fn resize_plan(
    target: &ResizeTarget,
    new_size_mib: u64,
    direction: ResizeDirection,
) -> Result<Vec<ChoreoStep>, CapabilityRefusal> {
    let caps = target.fs.capabilities();
    let support = match direction {
        ResizeDirection::Grow => caps.grow,
        ResizeDirection::Shrink => caps.shrink,
    };
    if !support.is_supported() {
        return Err(match direction {
            ResizeDirection::Grow => CapabilityRefusal::GrowUnsupported { fs: target.fs },
            ResizeDirection::Shrink => CapabilityRefusal::ShrinkUnsupported { fs: target.fs },
        });
    }
    let new_end_mib = target.start_mib + new_size_mib;
    let fs_resize = ChoreoStep::FsResize {
        fs: target.fs,
        device: target.partition.clone(),
        mountpoint: target.mountpoint.clone(),
        new_size_mib,
        direction,
    };
    let part_resize = ChoreoStep::PartResize {
        disk: target.disk.clone(),
        number: target.number,
        new_end_mib,
    };
    Ok(match direction {
        ResizeDirection::Shrink => vec![
            ChoreoStep::FsCheck {
                fs: target.fs,
                device: target.partition.clone(),
            },
            fs_resize,
            part_resize,
        ],
        ResizeDirection::Grow => vec![part_resize, fs_resize],
    })
}

/// The status of one choreography step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepStatus {
    /// Not yet reached (a prior step failed).
    Pending,
    /// Completed.
    Done,
    /// Failed — the choreography halted here (carries the typed reason). No later
    /// step ran, so there is no silent partial past this point.
    Failed(String),
}

/// The typed outcome of running a choreography.
///
/// Per-step status parallel to the plan, plus the halt index if any. A mid-plan
/// failure is captured typed (the failing step is `Failed`, every later step stays
/// `Pending`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChoreoOutcome {
    /// Per-step status, parallel to the plan.
    pub steps: Vec<StepStatus>,
    /// The index the choreography halted at, if any.
    pub halted_at: Option<usize>,
}

impl ChoreoOutcome {
    /// Whether every step completed.
    #[must_use]
    pub const fn is_success(&self) -> bool {
        self.halted_at.is_none()
    }

    /// A one-line failure summary for the op-level error (`None` on success).
    #[must_use]
    pub fn failure_summary(&self, plan: &[ChoreoStep]) -> Option<String> {
        let idx = self.halted_at?;
        let step = plan.get(idx).map_or("?", ChoreoStep::label);
        let reason = match self.steps.get(idx) {
            Some(StepStatus::Failed(r)) => r.as_str(),
            _ => "unknown",
        };
        Some(format!("{step} step failed: {reason}"))
    }
}

/// Run a choreography `plan` through `runner`, halting on the first failure.
///
/// The failing step becomes a typed [`StepStatus::Failed`] and every later step
/// stays `Pending` (no silent partial). `on_step(idx, status)` fires once per
/// resolved step. Pure over the injected runner — the mid-failure state machine is
/// unit-tested with a fake.
#[must_use]
pub fn run_plan(
    plan: &[ChoreoStep],
    runner: &dyn FsToolRunner,
    mut on_step: impl FnMut(usize, &StepStatus),
) -> ChoreoOutcome {
    let mut steps = vec![StepStatus::Pending; plan.len()];
    let mut halted_at = None;
    for (i, step) in plan.iter().enumerate() {
        let res = match step {
            ChoreoStep::FsCheck { fs, device } => runner.fs_check(*fs, device),
            ChoreoStep::FsResize {
                fs,
                device,
                mountpoint,
                new_size_mib,
                direction,
            } => runner.fs_resize(
                *fs,
                device,
                mountpoint.as_deref(),
                *new_size_mib,
                *direction,
            ),
            ChoreoStep::PartResize {
                disk,
                number,
                new_end_mib,
            } => runner.part_resize(disk, *number, *new_end_mib),
        };
        match res {
            Ok(()) => {
                steps[i] = StepStatus::Done;
                on_step(i, &steps[i]);
            }
            Err(e) => {
                steps[i] = StepStatus::Failed(e.to_string());
                on_step(i, &steps[i]);
                halted_at = Some(i);
                break;
            }
        }
    }
    ChoreoOutcome { steps, halted_at }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // ── capability matrix ──

    #[test]
    fn capability_matrix_is_honest_per_fs() {
        // xfs grows but never shrinks — the canonical honest-state case.
        let xfs = Filesystem::Xfs.capabilities();
        assert_eq!(xfs.grow, ResizeSupport::Online);
        assert_eq!(xfs.shrink, ResizeSupport::Unsupported);
        assert!(!xfs.subvolumes);
        // ext4 grows online, shrinks offline.
        let ext4 = Filesystem::Ext4.capabilities();
        assert_eq!(ext4.grow, ResizeSupport::Online);
        assert_eq!(ext4.shrink, ResizeSupport::Offline);
        // btrfs resizes both ways online + holds subvolumes.
        let btrfs = Filesystem::Btrfs.capabilities();
        assert_eq!(btrfs.grow, ResizeSupport::Online);
        assert_eq!(btrfs.shrink, ResizeSupport::Online);
        assert!(btrfs.subvolumes);
        // exfat + swap can't resize either way.
        for fs in [Filesystem::Exfat, Filesystem::Swap] {
            assert_eq!(fs.capabilities().grow, ResizeSupport::Unsupported);
            assert_eq!(fs.capabilities().shrink, ResizeSupport::Unsupported);
        }
        // ntfs + vfat resize offline both ways.
        for fs in [Filesystem::Ntfs, Filesystem::Vfat] {
            assert_eq!(fs.capabilities().grow, ResizeSupport::Offline);
            assert_eq!(fs.capabilities().shrink, ResizeSupport::Offline);
        }
        // every fs formats + labels.
        for fs in [
            Filesystem::Ext4,
            Filesystem::Xfs,
            Filesystem::Vfat,
            Filesystem::Exfat,
            Filesystem::Btrfs,
            Filesystem::Ntfs,
            Filesystem::Swap,
        ] {
            assert!(fs.capabilities().format);
            assert!(fs.capabilities().label);
        }
    }

    // ── argv builders (exact) ──

    #[test]
    fn mkfs_argv_per_fs_label_flag() {
        let dev = Path::new("/dev/sdb1");
        assert_eq!(
            mkfs_cmd(Filesystem::Ext4, dev, Some("data")),
            ToolCmd::new("mkfs.ext4", &["-F", "-L", "data", "/dev/sdb1"])
        );
        // vfat uses -n, not -L.
        assert_eq!(
            mkfs_cmd(Filesystem::Vfat, dev, Some("BOOT")),
            ToolCmd::new("mkfs.vfat", &["-n", "BOOT", "/dev/sdb1"])
        );
        assert_eq!(
            mkfs_cmd(Filesystem::Xfs, dev, None),
            ToolCmd::new("mkfs.xfs", &["-f", "/dev/sdb1"])
        );
        assert_eq!(
            mkfs_cmd(Filesystem::Swap, dev, Some("swap0")),
            ToolCmd::new("mkswap", &["-L", "swap0", "/dev/sdb1"])
        );
        assert_eq!(
            mkfs_cmd(Filesystem::Ntfs, dev, None),
            ToolCmd::new("mkfs.ntfs", &["--fast", "--force", "/dev/sdb1"])
        );
    }

    #[test]
    fn set_label_argv_per_fs() {
        let dev = Path::new("/dev/sdb1");
        assert_eq!(
            set_label_cmd(Filesystem::Ext4, dev, "x"),
            ToolCmd::new("e2label", &["/dev/sdb1", "x"])
        );
        assert_eq!(
            set_label_cmd(Filesystem::Xfs, dev, "x"),
            ToolCmd::new("xfs_admin", &["-L", "x", "/dev/sdb1"])
        );
        assert_eq!(
            set_label_cmd(Filesystem::Btrfs, dev, "x"),
            ToolCmd::new("btrfs", &["filesystem", "label", "/dev/sdb1", "x"])
        );
    }

    #[test]
    fn fs_resize_argv_and_unsupported() {
        let dev = Path::new("/dev/sdb1");
        // ext4 shrink → resize2fs with explicit MiB.
        assert_eq!(
            fs_resize_cmd(Filesystem::Ext4, dev, None, 4096, ResizeDirection::Shrink),
            Some(ToolCmd::new("resize2fs", &["/dev/sdb1", "4096M"]))
        );
        // xfs grow → xfs_growfs against the mount point, no size.
        assert_eq!(
            fs_resize_cmd(
                Filesystem::Xfs,
                dev,
                Some("/mnt/x"),
                8192,
                ResizeDirection::Grow
            ),
            Some(ToolCmd::new("xfs_growfs", &["/mnt/x"]))
        );
        // xfs shrink → None (unsupported).
        assert_eq!(
            fs_resize_cmd(Filesystem::Xfs, dev, None, 100, ResizeDirection::Shrink),
            None
        );
        // btrfs resize online against mountpoint.
        assert_eq!(
            fs_resize_cmd(
                Filesystem::Btrfs,
                dev,
                Some("/mnt/b"),
                2048,
                ResizeDirection::Shrink
            ),
            Some(ToolCmd::new(
                "btrfs",
                &["filesystem", "resize", "2048M", "/mnt/b"]
            ))
        );
    }

    #[test]
    fn luks_and_subvol_argv() {
        assert_eq!(
            luks_format_cmd(Path::new("/dev/sdb1"), Path::new("/run/key")),
            ToolCmd::new(
                "cryptsetup",
                &[
                    "luksFormat",
                    "--batch-mode",
                    "--key-file",
                    "/run/key",
                    "/dev/sdb1"
                ]
            )
        );
        assert_eq!(
            luks_open_cmd(Path::new("/dev/sdb1"), "cryptdata", Path::new("/run/key")),
            ToolCmd::new(
                "cryptsetup",
                &["open", "--key-file", "/run/key", "/dev/sdb1", "cryptdata"]
            )
        );
        assert_eq!(
            luks_close_cmd("cryptdata"),
            ToolCmd::new("cryptsetup", &["close", "cryptdata"])
        );
        assert_eq!(
            subvol_create_cmd("/mnt/b", "home"),
            ToolCmd::new("btrfs", &["subvolume", "create", "/mnt/b/home"])
        );
        assert_eq!(
            subvol_snapshot_cmd("/mnt/b", "home", "home-snap", true),
            ToolCmd::new(
                "btrfs",
                &[
                    "subvolume",
                    "snapshot",
                    "-r",
                    "/mnt/b/home",
                    "/mnt/b/home-snap"
                ]
            )
        );
    }

    #[test]
    fn parse_subvol_list_extracts_paths() {
        let out = "\
ID 256 gen 9 top level 5 path home
ID 257 gen 9 top level 5 path var/log
";
        assert_eq!(parse_subvol_list(out), vec!["home", "var/log"]);
        assert!(parse_subvol_list("").is_empty());
    }

    // ── choreography planner ──

    fn target(fs: Filesystem) -> ResizeTarget {
        ResizeTarget {
            partition: PathBuf::from("/dev/sdb1"),
            disk: PathBuf::from("/dev/sdb"),
            number: 1,
            start_mib: 1,
            fs,
            mountpoint: None,
        }
    }

    #[test]
    fn shrink_plan_is_check_fs_part_ordered() {
        let plan = resize_plan(&target(Filesystem::Ext4), 4096, ResizeDirection::Shrink).unwrap();
        assert_eq!(plan.len(), 3);
        assert!(matches!(plan[0], ChoreoStep::FsCheck { .. }));
        assert!(matches!(
            plan[1],
            ChoreoStep::FsResize {
                direction: ResizeDirection::Shrink,
                ..
            }
        ));
        // part end = start(1) + new_size(4096).
        assert!(matches!(
            plan[2],
            ChoreoStep::PartResize {
                new_end_mib: 4097,
                ..
            }
        ));
    }

    #[test]
    fn grow_plan_is_part_then_fs() {
        let plan = resize_plan(&target(Filesystem::Ext4), 8192, ResizeDirection::Grow).unwrap();
        assert_eq!(plan.len(), 2);
        assert!(matches!(plan[0], ChoreoStep::PartResize { .. }));
        assert!(matches!(
            plan[1],
            ChoreoStep::FsResize {
                direction: ResizeDirection::Grow,
                ..
            }
        ));
    }

    #[test]
    fn shrink_plan_refuses_xfs_typed() {
        assert_eq!(
            resize_plan(&target(Filesystem::Xfs), 100, ResizeDirection::Shrink),
            Err(CapabilityRefusal::ShrinkUnsupported {
                fs: Filesystem::Xfs
            })
        );
        // exfat can't grow either.
        assert_eq!(
            resize_plan(&target(Filesystem::Exfat), 100, ResizeDirection::Grow),
            Err(CapabilityRefusal::GrowUnsupported {
                fs: Filesystem::Exfat
            })
        );
    }

    // ── choreography executor (state machine incl. mid-failure) ──

    /// A recording runner that succeeds, or fails a chosen verb.
    #[derive(Default)]
    struct FakeRunner {
        calls: Mutex<Vec<String>>,
        fail_verb: Option<&'static str>,
    }
    impl FakeRunner {
        fn failing(verb: &'static str) -> Self {
            Self {
                calls: Mutex::new(vec![]),
                fail_verb: Some(verb),
            }
        }
        fn record(&self, verb: &'static str) -> Result<(), FsToolError> {
            self.calls.lock().unwrap().push(verb.to_string());
            if self.fail_verb == Some(verb) {
                Err(FsToolError::Failed {
                    op: verb,
                    reason: "boom".into(),
                })
            } else {
                Ok(())
            }
        }
    }
    impl FsToolRunner for FakeRunner {
        fn mkfs(&self, _: Filesystem, _: &Path, _: Option<&str>) -> Result<(), FsToolError> {
            self.record("mkfs")
        }
        fn set_label(&self, _: Filesystem, _: &Path, _: &str) -> Result<(), FsToolError> {
            self.record("set_label")
        }
        fn fs_check(&self, _: Filesystem, _: &Path) -> Result<(), FsToolError> {
            self.record("fs_check")
        }
        fn fs_resize(
            &self,
            _: Filesystem,
            _: &Path,
            _: Option<&str>,
            _: u64,
            _: ResizeDirection,
        ) -> Result<(), FsToolError> {
            self.record("fs_resize")
        }
        fn part_resize(&self, _: &Path, _: u32, _: u64) -> Result<(), FsToolError> {
            self.record("part_resize")
        }
        fn luks_format(&self, _: &Path, _: Option<&Path>) -> Result<(), FsToolError> {
            self.record("luks_format")
        }
        fn luks_open(&self, _: &Path, _: &str, _: Option<&Path>) -> Result<(), FsToolError> {
            self.record("luks_open")
        }
        fn luks_close(&self, _: &str) -> Result<(), FsToolError> {
            self.record("luks_close")
        }
        fn subvol_list(&self, _: &str) -> Result<Vec<String>, FsToolError> {
            self.record("subvol_list").map(|()| vec![])
        }
        fn subvol_create(&self, _: &str, _: &str) -> Result<(), FsToolError> {
            self.record("subvol_create")
        }
        fn subvol_delete(&self, _: &str, _: &str) -> Result<(), FsToolError> {
            self.record("subvol_delete")
        }
        fn subvol_snapshot(&self, _: &str, _: &str, _: &str, _: bool) -> Result<(), FsToolError> {
            self.record("subvol_snapshot")
        }
    }

    #[test]
    fn run_plan_all_steps_succeed() {
        let plan = resize_plan(&target(Filesystem::Ext4), 4096, ResizeDirection::Shrink).unwrap();
        let runner = FakeRunner::default();
        let mut seen = vec![];
        let outcome = run_plan(&plan, &runner, |i, s| seen.push((i, s.clone())));
        assert!(outcome.is_success());
        assert_eq!(
            *runner.calls.lock().unwrap(),
            vec!["fs_check", "fs_resize", "part_resize"]
        );
        assert_eq!(seen.len(), 3);
    }

    #[test]
    fn run_plan_halts_typed_on_mid_failure_no_silent_partial() {
        // The fs-shrink (step 1) fails: the fs-check ran, the part-shrink must NOT.
        let plan = resize_plan(&target(Filesystem::Ext4), 4096, ResizeDirection::Shrink).unwrap();
        let runner = FakeRunner::failing("fs_resize");
        let outcome = run_plan(&plan, &runner, |_, _| {});
        assert!(!outcome.is_success());
        assert_eq!(outcome.halted_at, Some(1));
        assert_eq!(outcome.steps[0], StepStatus::Done); // fs-check completed
        assert!(matches!(outcome.steps[1], StepStatus::Failed(_))); // fs-shrink failed
        assert_eq!(outcome.steps[2], StepStatus::Pending); // part-shrink never ran
                                                           // The partition geometry was never touched — no silent partial corruption.
        assert_eq!(*runner.calls.lock().unwrap(), vec!["fs_check", "fs_resize"]);
        assert!(outcome
            .failure_summary(&plan)
            .unwrap()
            .contains("fs-resize"));
    }

    #[test]
    fn live_runner_is_honest_when_a_tool_is_absent() {
        // On a headless build host mkfs.ntfs/cryptsetup are absent → typed error,
        // never a fabricated success (§7).
        let live = LiveFsTools::new();
        let res = live.mkfs(Filesystem::Ntfs, Path::new("/dev/mcnf-e12-23-nope"), None);
        assert!(
            matches!(
                res,
                Err(FsToolError::Unavailable { .. } | FsToolError::Failed { .. })
            ),
            "expected a typed error, got {res:?}"
        );
        // A LUKS op with no keyfile is integration-gated (no passphrase on the Bus).
        assert!(matches!(
            live.luks_format(Path::new("/dev/x"), None),
            Err(FsToolError::IntegrationGated(_))
        ));
    }
}
