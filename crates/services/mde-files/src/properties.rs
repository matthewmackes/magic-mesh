//! Native file properties — the "Properties" parity op (E11.6, Q34–Q39).
//!
//! Stats a path into the structured metadata a Properties dialog renders: kind,
//! size, Unix permissions, link count, owner ids, timestamps, and (for symlinks)
//! the link target. Pure `std` + `chrono` for timestamp formatting — no shell-out
//! to `stat(1)`. Uses `symlink_metadata`, so a symlink reports as a *link* (with
//! its target), not the file it points at.

use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Local};

/// What a path is, by its own (un-dereferenced) type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    /// A regular file.
    File,
    /// A directory.
    Directory,
    /// A symbolic link (see [`FileProperties::symlink_target`]).
    Symlink,
    /// A socket / fifo / block / char device — anything else.
    Other,
}

impl FileKind {
    /// A one-word label for the dialog / CLI.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Directory => "directory",
            Self::Symlink => "symlink",
            Self::Other => "special",
        }
    }
}

/// The metadata a Properties view renders for one path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileProperties {
    /// The path queried (as given).
    pub path: PathBuf,
    /// Its kind, by its own type (not the symlink target's).
    pub kind: FileKind,
    /// Size in bytes (the link's own size for a symlink).
    pub size_bytes: u64,
    /// The low 12 permission bits (`st_mode & 0o7777`).
    pub mode: u32,
    /// Hard-link count.
    pub links: u64,
    /// Owner uid.
    pub uid: u32,
    /// Owner gid.
    pub gid: u32,
    /// Last-modified time, when the platform reports it.
    pub modified: Option<SystemTime>,
    /// Last-accessed time, when the platform reports it.
    pub accessed: Option<SystemTime>,
    /// For a symlink, the path it points at.
    pub symlink_target: Option<PathBuf>,
}

impl FileProperties {
    /// Stat `path` (without dereferencing a final symlink).
    ///
    /// # Errors
    /// When the path can't be stat-ed (missing, permission denied, …).
    pub fn of(path: &Path) -> io::Result<Self> {
        let md = std::fs::symlink_metadata(path)?;
        let ft = md.file_type();
        let kind = if ft.is_symlink() {
            FileKind::Symlink
        } else if ft.is_dir() {
            FileKind::Directory
        } else if ft.is_file() {
            FileKind::File
        } else {
            FileKind::Other
        };
        let symlink_target = if kind == FileKind::Symlink {
            std::fs::read_link(path).ok()
        } else {
            None
        };
        Ok(Self {
            path: path.to_path_buf(),
            kind,
            size_bytes: md.size(),
            mode: md.mode() & 0o7777,
            links: md.nlink(),
            uid: md.uid(),
            gid: md.gid(),
            modified: md.modified().ok(),
            accessed: md.accessed().ok(),
            symlink_target,
        })
    }

    /// The `rwxr-xr-x`-style rendering of the 9 permission bits.
    #[must_use]
    pub fn permission_string(&self) -> String {
        let bit = |mask: u32, ch: char| if self.mode & mask != 0 { ch } else { '-' };
        [
            bit(0o400, 'r'),
            bit(0o200, 'w'),
            bit(0o100, 'x'),
            bit(0o040, 'r'),
            bit(0o020, 'w'),
            bit(0o010, 'x'),
            bit(0o004, 'r'),
            bit(0o002, 'w'),
            bit(0o001, 'x'),
        ]
        .into_iter()
        .collect()
    }

    /// The permission bits as an octal string (e.g. `0644`).
    #[must_use]
    pub fn mode_octal(&self) -> String {
        format!("{:04o}", self.mode & 0o777)
    }

    /// A human-readable size (IEC binary units: B, KiB, MiB, …).
    #[must_use]
    pub fn human_size(&self) -> String {
        human_size(self.size_bytes)
    }

    /// `modified` formatted in local time, or `"—"` when unavailable.
    #[must_use]
    pub fn modified_local(&self) -> String {
        format_local(self.modified)
    }
}

/// Format a byte count with IEC binary units; exact bytes shown under 1 KiB.
fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    #[allow(clippy::cast_precision_loss)]
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// Format an optional `SystemTime` as local `YYYY-MM-DD HH:MM:SS`, or `"—"`.
fn format_local(t: Option<SystemTime>) -> String {
    t.map_or_else(
        || "—".to_string(),
        |t| {
            DateTime::<Local>::from(t)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        },
    )
}

/// Render properties as the `mde-files --properties` report (one `key: value`
/// per line).
#[must_use]
pub fn report(p: &FileProperties) -> String {
    let mut out = format!(
        "path: {}\nkind: {}\nsize: {} ({} bytes)\npermissions: {} ({})\nlinks: {}\nowner: {}:{}\nmodified: {}\n",
        p.path.display(),
        p.kind.label(),
        p.human_size(),
        p.size_bytes,
        p.permission_string(),
        p.mode_octal(),
        p.links,
        p.uid,
        p.gid,
        p.modified_local(),
    );
    if let Some(target) = &p.symlink_target {
        out.push_str(&format!("target: {}\n", target.display()));
    }
    out
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("mde-files-props-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn of_reports_a_regular_file() {
        let dir = scratch("file");
        let f = dir.join("data.bin");
        std::fs::write(&f, vec![0u8; 2048]).unwrap();
        let p = FileProperties::of(&f).unwrap();
        assert_eq!(p.kind, FileKind::File);
        assert_eq!(p.size_bytes, 2048);
        assert!(p.links >= 1);
        assert!(p.symlink_target.is_none());
        assert_eq!(p.human_size(), "2.0 KiB");
    }

    #[test]
    fn of_reports_a_directory() {
        let dir = scratch("dir");
        let p = FileProperties::of(&dir).unwrap();
        assert_eq!(p.kind, FileKind::Directory);
        assert_eq!(p.kind.label(), "directory");
    }

    #[test]
    fn of_reports_a_symlink_without_dereferencing() {
        let dir = scratch("link");
        let target = dir.join("real.txt");
        std::fs::write(&target, b"x").unwrap();
        let link = dir.join("alias.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let p = FileProperties::of(&link).unwrap();
        assert_eq!(p.kind, FileKind::Symlink);
        assert_eq!(p.symlink_target.as_deref(), Some(target.as_path()));
    }

    #[test]
    fn permission_and_octal_strings_match_the_mode() {
        let dir = scratch("perm");
        let f = dir.join("script.sh");
        std::fs::write(&f, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o754)).unwrap();
        let p = FileProperties::of(&f).unwrap();
        assert_eq!(p.permission_string(), "rwxr-xr--");
        assert_eq!(p.mode_octal(), "0754");
    }

    #[test]
    fn human_size_uses_iec_units() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1024), "1.0 KiB");
        assert_eq!(human_size(1536), "1.5 KiB");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MiB");
    }

    #[test]
    fn report_includes_target_only_for_symlinks() {
        let dir = scratch("report");
        let f = dir.join("plain.txt");
        std::fs::write(&f, b"hi").unwrap();
        let plain = report(&FileProperties::of(&f).unwrap());
        assert!(plain.contains("kind: file"));
        assert!(!plain.contains("target:"));

        let link = dir.join("l");
        std::os::unix::fs::symlink(&f, &link).unwrap();
        let linked = report(&FileProperties::of(&link).unwrap());
        assert!(linked.contains("kind: symlink"));
        assert!(linked.contains("target:"));
    }

    #[test]
    fn of_missing_path_errors() {
        let dir = scratch("missing");
        assert_eq!(
            FileProperties::of(&dir.join("nope")).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
    }
}
