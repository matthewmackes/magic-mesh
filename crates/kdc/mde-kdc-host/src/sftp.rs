//! KDC-MESH-7 — the SFTP mount seam (design #11a: browse the phone's FS).
//!
//! When a desktop asks a paired phone to browse (`kdeconnect.sftp.request`
//! `startBrowsing`), the phone stands up an on-device SFTP server and replies
//! with a [`SftpMountInfo`]. This module is the **injectable seam** that mounts
//! that server so the phone's files appear as a local directory.
//!
//! The seam ([`SftpMount`]) is a trait so the worker + tests inject a fake; the
//! **production** leg ([`SshfsMount`]) shells out to `sshfs` and **honestly
//! gates** when neither `sshfs` nor the mount target is available (§7 — a
//! CLI-less / FUSE-less host reports the tool is absent rather than faking a
//! mount). The argv builder is pure + pinned by tests so the mount command can't
//! silently drift.

use std::path::{Path, PathBuf};
use std::process::Command;

use mde_kdc_proto::plugins::sftp::SftpMountInfo;

/// A mounted phone filesystem: the local mountpoint the phone's files now live at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountedFs {
    /// The local directory the phone's FS is mounted at.
    pub mountpoint: PathBuf,
    /// The remote path that was mounted (echoed from the mount info).
    pub remote_path: String,
}

/// Why an SFTP mount failed or was gated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SftpError {
    /// The phone's reply wasn't mountable (missing addr/port/creds) — an honest
    /// no-mount, not a fake one (the phone declined or SFTP is off).
    NotMountable,
    /// The mount tool (`sshfs`) isn't installed — the live leg's honest gate on a
    /// host without FUSE/sshfs. Carries what was looked for.
    ToolAbsent(String),
    /// The mount command ran but failed (exit code + stderr).
    MountFailed(String),
    /// A filesystem error preparing the mountpoint.
    Io(String),
}

impl std::fmt::Display for SftpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotMountable => f.write_str("sftp reply is not mountable (no addr/port/creds)"),
            Self::ToolAbsent(t) => write!(f, "sftp mount tool absent: {t}"),
            Self::MountFailed(e) => write!(f, "sftp mount failed: {e}"),
            Self::Io(e) => write!(f, "sftp mount io: {e}"),
        }
    }
}

impl std::error::Error for SftpError {}

/// The injectable SFTP-mount seam. Production is [`SshfsMount`]; tests inject a
/// fake that records calls without touching FUSE.
pub trait SftpMount: Send + Sync {
    /// Mount the phone's SFTP server ([`SftpMountInfo`]) at `mountpoint`.
    ///
    /// # Errors
    /// [`SftpError::NotMountable`] for an incomplete reply; [`SftpError::ToolAbsent`]
    /// when `sshfs` is missing (the live honest gate); [`SftpError::MountFailed`] /
    /// [`SftpError::Io`] otherwise.
    fn mount(&self, info: &SftpMountInfo, mountpoint: &Path) -> Result<MountedFs, SftpError>;

    /// Unmount a previously-mounted phone filesystem.
    ///
    /// # Errors
    /// [`SftpError::ToolAbsent`] / [`SftpError::MountFailed`] as for [`mount`](Self::mount).
    fn unmount(&self, mountpoint: &Path) -> Result<(), SftpError>;
}

/// Build the `sshfs` argv (WITHOUT the leading `sshfs`) for a mount — pure +
/// pinned by tests so the command surface can't drift.
///
/// `sshfs <user>@<ip>:<remote_path> <mountpoint> -p <port> -o password_stdin,...`
/// The one-time password is piped over stdin (`password_stdin`), never on the
/// argv (no leak in `ps`). `StrictHostKeyChecking=no` because the phone mints a
/// fresh host key per session (there's no stable key to pin), and the transport
/// is already the encrypted overlay (design #3).
#[must_use]
pub fn sshfs_argv(info: &SftpMountInfo, mountpoint: &Path) -> Vec<String> {
    vec![
        format!("{}@{}:{}", info.user, info.ip, info.remote_path()),
        mountpoint.to_string_lossy().to_string(),
        "-p".to_string(),
        info.port.to_string(),
        "-o".to_string(),
        "password_stdin,reconnect,StrictHostKeyChecking=no,UserKnownHostsFile=/dev/null"
            .to_string(),
    ]
}

/// The production SFTP mount: shells out to `sshfs` (FUSE). Honest-gates when
/// `sshfs` isn't installed.
#[derive(Debug, Default, Clone, Copy)]
pub struct SshfsMount;

impl SshfsMount {
    /// Whether `sshfs` is on `PATH` (the live gate — a host without FUSE/sshfs
    /// can't mount, so we report the tool absent rather than fake it).
    fn sshfs_present() -> bool {
        Command::new("sh")
            .arg("-c")
            .arg("command -v sshfs")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

impl SftpMount for SshfsMount {
    fn mount(&self, info: &SftpMountInfo, mountpoint: &Path) -> Result<MountedFs, SftpError> {
        use std::io::Write;
        use std::process::Stdio;
        if !info.is_mountable() {
            return Err(SftpError::NotMountable);
        }
        if !Self::sshfs_present() {
            return Err(SftpError::ToolAbsent("sshfs".to_string()));
        }
        std::fs::create_dir_all(mountpoint).map_err(|e| SftpError::Io(e.to_string()))?;
        let argv = sshfs_argv(info, mountpoint);
        let mut child = Command::new("sshfs")
            .args(&argv)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| SftpError::MountFailed(e.to_string()))?;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(info.password.as_bytes());
            let _ = stdin.write_all(b"\n");
        }
        let out = child
            .wait_with_output()
            .map_err(|e| SftpError::MountFailed(e.to_string()))?;
        if !out.status.success() {
            return Err(SftpError::MountFailed(
                String::from_utf8_lossy(&out.stderr).trim().to_string(),
            ));
        }
        Ok(MountedFs {
            mountpoint: mountpoint.to_path_buf(),
            remote_path: info.remote_path().to_string(),
        })
    }

    fn unmount(&self, mountpoint: &Path) -> Result<(), SftpError> {
        // `fusermount -u` is the FUSE unmount; honest-gate when it's absent.
        let present = Command::new("sh")
            .arg("-c")
            .arg("command -v fusermount")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !present {
            return Err(SftpError::ToolAbsent("fusermount".to_string()));
        }
        let out = Command::new("fusermount")
            .arg("-u")
            .arg(mountpoint)
            .output()
            .map_err(|e| SftpError::MountFailed(e.to_string()))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(SftpError::MountFailed(
                String::from_utf8_lossy(&out.stderr).trim().to_string(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mountable() -> SftpMountInfo {
        SftpMountInfo {
            ip: "10.42.0.9".into(),
            port: 1739,
            user: "kdeconnect".into(),
            password: "one-time".into(),
            path: "/storage/emulated/0".into(),
            ..Default::default()
        }
    }

    #[test]
    fn sshfs_argv_is_pinned_and_never_leaks_the_password() {
        let argv = sshfs_argv(&mountable(), Path::new("/run/mde/kdc-sftp/moto"));
        assert_eq!(argv[0], "kdeconnect@10.42.0.9:/storage/emulated/0");
        assert_eq!(argv[1], "/run/mde/kdc-sftp/moto");
        assert_eq!(argv[2], "-p");
        assert_eq!(argv[3], "1739");
        assert_eq!(argv[4], "-o");
        assert!(argv[5].contains("password_stdin"));
        assert!(argv[5].contains("StrictHostKeyChecking=no"));
        // The password is piped over stdin — it must never appear on the argv.
        assert!(
            !argv.iter().any(|a| a.contains("one-time")),
            "password must not appear in argv (piped over stdin)",
        );
    }

    #[test]
    fn mount_rejects_an_unmountable_reply() {
        // An incomplete reply is an honest no-mount, before any tool lookup.
        let r = SshfsMount.mount(&SftpMountInfo::default(), Path::new("/tmp/x"));
        assert_eq!(r, Err(SftpError::NotMountable));
    }

    /// A test double: records the mount call, no FUSE.
    #[derive(Default)]
    struct FakeMount {
        calls: std::sync::Mutex<Vec<(SftpMountInfo, PathBuf)>>,
    }

    impl SftpMount for FakeMount {
        fn mount(&self, info: &SftpMountInfo, mountpoint: &Path) -> Result<MountedFs, SftpError> {
            if !info.is_mountable() {
                return Err(SftpError::NotMountable);
            }
            self.calls
                .lock()
                .unwrap()
                .push((info.clone(), mountpoint.to_path_buf()));
            Ok(MountedFs {
                mountpoint: mountpoint.to_path_buf(),
                remote_path: info.remote_path().to_string(),
            })
        }
        fn unmount(&self, _mountpoint: &Path) -> Result<(), SftpError> {
            Ok(())
        }
    }

    #[test]
    fn injected_seam_records_the_mount() {
        let seam = FakeMount::default();
        let mp = Path::new("/run/mde/kdc-sftp/moto");
        let mounted = seam.mount(&mountable(), mp).expect("fake mount ok");
        assert_eq!(mounted.mountpoint, mp);
        assert_eq!(mounted.remote_path, "/storage/emulated/0");
        assert_eq!(seam.calls.lock().unwrap().len(), 1);
    }
}
