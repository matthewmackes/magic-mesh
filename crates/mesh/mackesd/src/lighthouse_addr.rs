//! LIGHTHOUSE-10 — this node's persisted PUBLIC underlay address.
//!
//! A lighthouse's externally-dialable `ip:port` (the Nebula underlay peers dial)
//! is supplied once at `found` / `set-external-addr` and persisted here so the
//! telemetry heartbeat can stamp it into the replicated peer directory
//! ([`mackes_mesh_types::peers::PeerRecord::external_addr`]). The enroll roster
//! then reads every lighthouse's `external_addr` so a joining node learns the
//! FULL, redundant lighthouse set — not just the one it enrolled through.
//!
//! Tiny by design: a single trimmed line at a fixed path. Reads tolerate
//! absence (a non-lighthouse, or a lighthouse before it's been set) → `None`.

use std::io;
use std::path::{Path, PathBuf};

/// The external address is a single small line. Bound replicated/local input
/// before it becomes an owned string, while leaving room for long IPv6 forms.
const MAX_EXTERNAL_ADDR_BYTES: usize = 4 * 1024;

/// Where the public underlay address is recorded (one line, e.g. `1.2.3.4:4242`).
pub const EXTERNAL_ADDR_PATH: &str = "/etc/mackesd/external-addr";

/// The configured path (constant today; a function keeps the call sites uniform
/// and leaves room for an env override if tests ever need one).
#[must_use]
pub fn external_addr_path() -> PathBuf {
    PathBuf::from(EXTERNAL_ADDR_PATH)
}

/// Persist `addr` (the lighthouse's public `ip:port`) to [`EXTERNAL_ADDR_PATH`],
/// creating the parent dir. Idempotent — overwrites with the trimmed value.
///
/// # Errors
/// Returns the underlying [`io::Error`] if the directory or file can't be written.
pub fn write_external_addr(addr: &str) -> io::Result<()> {
    write_external_addr_to(&external_addr_path(), addr)
}

/// [`write_external_addr`] against an explicit path (tests).
///
/// # Errors
/// Returns the underlying [`io::Error`] if the directory or file can't be written.
pub fn write_external_addr_to(path: &Path, addr: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, format!("{}\n", addr.trim()))
}

/// Read the persisted public address, or `None` when the file is absent/blank
/// (a non-lighthouse, or a lighthouse whose address hasn't been set yet). Never
/// errors — a missing address is a normal state, not a failure.
#[must_use]
pub fn read_external_addr() -> Option<String> {
    read_external_addr_from(&external_addr_path())
}

/// [`read_external_addr`] against an explicit path (tests).
#[must_use]
pub fn read_external_addr_from(path: &Path) -> Option<String> {
    let s = read_bounded_external_addr(path)?;
    let trimmed = s.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Read the address through the descriptor that will actually be consumed.
/// Reject final symlinks, blocking special files, oversized input, and invalid
/// UTF-8 before the address is accepted.
fn read_bounded_external_addr(path: &Path) -> Option<String> {
    use std::io::Read as _;

    #[cfg(unix)]
    let file: std::fs::File = {
        use rustix::fs::{Mode, OFlags};

        rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .ok()?
        .into()
    };
    #[cfg(not(unix))]
    let file = {
        let metadata = std::fs::symlink_metadata(path).ok()?;
        if !metadata.file_type().is_file() {
            return None;
        }
        std::fs::File::open(path).ok()?
    };

    let metadata = file.metadata().ok()?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_EXTERNAL_ADDR_BYTES as u64 {
        return None;
    }

    let mut bytes = Vec::with_capacity(
        usize::try_from(metadata.len())
            .unwrap_or(MAX_EXTERNAL_ADDR_BYTES)
            .min(MAX_EXTERNAL_ADDR_BYTES)
            .saturating_add(1),
    );
    file.take((MAX_EXTERNAL_ADDR_BYTES as u64).saturating_add(1))
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > MAX_EXTERNAL_ADDR_BYTES {
        return None;
    }
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_the_trimmed_address() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("external-addr");
        write_external_addr_to(&p, "  203.0.113.7:4242 \n").unwrap();
        assert_eq!(
            read_external_addr_from(&p).as_deref(),
            Some("203.0.113.7:4242")
        );
    }

    #[test]
    fn absent_or_blank_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("external-addr");
        assert_eq!(read_external_addr_from(&p), None, "absent → None");
        write_external_addr_to(&p, "   \n").unwrap();
        assert_eq!(read_external_addr_from(&p), None, "blank → None");
    }

    #[test]
    fn creates_parent_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("nested/dir/external-addr");
        write_external_addr_to(&p, "1.2.3.4:4242").unwrap();
        assert_eq!(read_external_addr_from(&p).as_deref(), Some("1.2.3.4:4242"));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_final_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("target");
        let link = tmp.path().join("external-addr");
        std::fs::write(&target, "203.0.113.7:4242\n").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        assert_eq!(read_external_addr_from(&link), None);
    }

    #[test]
    fn rejects_non_regular_files() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("external-addr");
        std::fs::create_dir(&path).unwrap();

        assert_eq!(read_external_addr_from(&path), None);
    }

    #[test]
    fn rejects_oversized_contents() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("external-addr");
        std::fs::write(&path, vec![b'x'; MAX_EXTERNAL_ADDR_BYTES + 1]).unwrap();

        assert_eq!(read_external_addr_from(&path), None);
    }

    #[test]
    fn rejects_invalid_utf8() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("external-addr");
        std::fs::write(&path, [b'1', b'.', b'2', 0xff, b':', b'4']).unwrap();

        assert_eq!(read_external_addr_from(&path), None);
    }
}
