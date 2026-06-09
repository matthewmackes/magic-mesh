//! Native freedesktop.org Trash (the XDG "home trash") — the file manager's
//! delete-to-trash parity op (E11.6, Q34–Q39).
//!
//! Implements the Trash specification's home-trash half: a trashed file moves
//! into `$XDG_DATA_HOME/Trash/files/` and a sibling `info/<name>.trashinfo`
//! records where it came from + when, so the delete is reversible. Pure `std`
//! (+ `chrono` for the deletion timestamp); no GUI, no external `trash`/`gio`
//! shell-out — the whole point of the native-Rust parity push.
//!
//! Spec: <https://specifications.freedesktop.org/trash-spec/trashspec-1.0.html>.

use std::io;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use chrono::Local;

/// One item recovered from the trash's `info/` directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrashedItem {
    /// The file's name *within the trash* (matches `files/<name>` and
    /// `info/<name>.trashinfo`); may differ from the original basename when a
    /// collision forced a rename.
    pub trash_name: String,
    /// Absolute path the file was trashed from (the restore target).
    pub original_path: PathBuf,
    /// `DeletionDate`, `YYYY-MM-DDThh:mm:ss` local time, verbatim from the info file.
    pub deletion_date: String,
}

impl TrashedItem {
    /// The trashed file's current location under `files/`.
    #[must_use]
    pub fn trashed_file(&self, trash: &TrashDir) -> PathBuf {
        trash.files_dir().join(&self.trash_name)
    }
}

/// A freedesktop home-trash directory (`…/Trash`), holding `files/` + `info/`.
#[derive(Debug, Clone)]
pub struct TrashDir {
    root: PathBuf,
}

impl TrashDir {
    /// The user's home trash: `$XDG_DATA_HOME/Trash`, or `$HOME/.local/share/Trash`
    /// when `XDG_DATA_HOME` is unset. Creates `files/` + `info/` if absent.
    ///
    /// # Errors
    /// When neither `XDG_DATA_HOME` nor `HOME` is set, or the directories can't
    /// be created.
    pub fn home() -> io::Result<Self> {
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "neither XDG_DATA_HOME nor HOME is set",
                )
            })?;
        Self::with_root(base.join("Trash"))
    }

    /// A trash rooted at an explicit directory (tests, or a non-home trash).
    /// Creates `files/` + `info/`.
    ///
    /// # Errors
    /// When the `files/` or `info/` subdirectories can't be created.
    pub fn with_root(root: PathBuf) -> io::Result<Self> {
        let t = Self { root };
        std::fs::create_dir_all(t.files_dir())?;
        std::fs::create_dir_all(t.info_dir())?;
        Ok(t)
    }

    /// `…/Trash/files`.
    #[must_use]
    pub fn files_dir(&self) -> PathBuf {
        self.root.join("files")
    }

    /// `…/Trash/info`.
    #[must_use]
    pub fn info_dir(&self) -> PathBuf {
        self.root.join("info")
    }

    /// Trash `src`: pick a collision-free name, write its `.trashinfo` first (so a
    /// crash never leaves an orphaned file with no record), then move the file in.
    /// Returns the recorded item.
    ///
    /// # Errors
    /// When `src` doesn't exist, its absolute path can't be resolved, or any
    /// filesystem step (write info / move file) fails.
    pub fn trash(&self, src: &Path) -> io::Result<TrashedItem> {
        if !src.exists() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("source does not exist: {}", src.display()),
            ));
        }
        // Absolute, for the restore target. `canonicalize` would resolve symlinks
        // (wrong — we want the link's own path), so join cwd manually when relative.
        let original = if src.is_absolute() {
            src.to_path_buf()
        } else {
            std::env::current_dir()?.join(src)
        };
        let base = original
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "untitled".to_string());

        let trash_name = self.unique_name(&base);
        let info_path = self.info_path(&trash_name);
        let deletion_date = Local::now().format("%Y-%m-%dT%H:%M:%S").to_string();

        // Write info first per the spec's race-avoidance note.
        std::fs::write(
            &info_path,
            format!(
                "[Trash Info]\nPath={}\nDeletionDate={}\n",
                percent_encode_path(&original),
                deletion_date
            ),
        )?;

        let dest = self.files_dir().join(&trash_name);
        if let Err(e) = crate::fileops::move_path(&original, &dest) {
            // Roll back the info file so a failed move leaves no dangling record.
            let _ = std::fs::remove_file(&info_path);
            return Err(e);
        }
        Ok(TrashedItem {
            trash_name,
            original_path: original,
            deletion_date,
        })
    }

    /// Every recoverable item, parsed from `info/*.trashinfo` (order unspecified).
    ///
    /// # Errors
    /// When `info/` can't be read. A malformed individual `.trashinfo` is skipped,
    /// not fatal.
    pub fn list(&self) -> io::Result<Vec<TrashedItem>> {
        let mut items = Vec::new();
        for entry in std::fs::read_dir(self.info_dir())? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("trashinfo") {
                continue;
            }
            let Some(trash_name) = path.file_stem().map(|s| s.to_string_lossy().into_owned())
            else {
                continue;
            };
            let Ok(body) = std::fs::read_to_string(&path) else {
                continue;
            };
            if let Some(item) = parse_trashinfo(&trash_name, &body) {
                items.push(item);
            }
        }
        Ok(items)
    }

    /// Restore `item` to its original path, removing the trash record. Recreates
    /// missing parent directories of the target.
    ///
    /// # Errors
    /// When the trashed file is missing, the target already exists, or a
    /// filesystem step fails.
    pub fn restore(&self, item: &TrashedItem) -> io::Result<()> {
        let from = item.trashed_file(self);
        if item.original_path.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!(
                    "restore target already exists: {}",
                    item.original_path.display()
                ),
            ));
        }
        if let Some(parent) = item.original_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        crate::fileops::move_path(&from, &item.original_path)?;
        std::fs::remove_file(self.info_path(&item.trash_name))?;
        Ok(())
    }

    /// Permanently delete everything in the trash. Returns the number of items
    /// removed.
    ///
    /// # Errors
    /// When `info/` can't be read or a removal fails.
    pub fn empty(&self) -> io::Result<usize> {
        let items = self.list()?;
        for item in &items {
            let f = item.trashed_file(self);
            if f.is_dir() {
                std::fs::remove_dir_all(&f)?;
            } else if f.exists() {
                std::fs::remove_file(&f)?;
            }
            std::fs::remove_file(self.info_path(&item.trash_name))?;
        }
        Ok(items.len())
    }

    fn info_path(&self, trash_name: &str) -> PathBuf {
        self.info_dir().join(format!("{trash_name}.trashinfo"))
    }

    /// A trash name not already taken in `files/` or `info/`: `name`, then
    /// `name.1`, `name.2`, … keeping any extension intact-enough for the user to
    /// recognise (the suffix goes on the whole name, matching common FMs).
    fn unique_name(&self, base: &str) -> String {
        let taken = |n: &str| self.files_dir().join(n).exists() || self.info_path(n).exists();
        if !taken(base) {
            return base.to_string();
        }
        (1u64..)
            .map(|i| format!("{base}.{i}"))
            .find(|n| !taken(n))
            .unwrap_or_else(|| base.to_string())
    }
}

/// Parse a `.trashinfo` body into a [`TrashedItem`]; `None` when it lacks a
/// `[Trash Info]` header or a `Path` key.
fn parse_trashinfo(trash_name: &str, body: &str) -> Option<TrashedItem> {
    let mut in_section = false;
    let mut original_path: Option<PathBuf> = None;
    let mut deletion_date = String::new();
    for line in body.lines() {
        let line = line.trim();
        if line.eq_ignore_ascii_case("[Trash Info]") {
            in_section = true;
        } else if let Some(rest) = line.strip_prefix('[') {
            // a different section begins
            let _ = rest;
            in_section = false;
        } else if in_section {
            if let Some(v) = line.strip_prefix("Path=") {
                original_path = Some(percent_decode_path(v));
            } else if let Some(v) = line.strip_prefix("DeletionDate=") {
                deletion_date = v.to_string();
            }
        }
    }
    Some(TrashedItem {
        trash_name: trash_name.to_string(),
        original_path: original_path?,
        deletion_date,
    })
}

/// Percent-encode an absolute path for a `.trashinfo` `Path=` value, leaving the
/// `/` separators and RFC-3986 unreserved bytes unescaped (matches what gio /
/// other trash implementations emit, so they can read our records).
fn percent_encode_path(path: &Path) -> String {
    let mut out = String::new();
    for &b in path.as_os_str().as_encoded_bytes() {
        match b {
            b'/' | b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Inverse of [`percent_encode_path`]: decode `%XX` escapes back into raw bytes
/// and rebuild the path (bytes that aren't valid UTF-8 are preserved).
fn percent_decode_path(s: &str) -> PathBuf {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    PathBuf::from(std::ffi::OsString::from_vec(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An isolated trash + a scratch source dir under the system temp dir, unique
    /// to this test name so parallel tests don't collide.
    fn scratch(tag: &str) -> (TrashDir, PathBuf) {
        let base =
            std::env::temp_dir().join(format!("mde-files-trash-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let work = base.join("work");
        std::fs::create_dir_all(&work).unwrap();
        let trash = TrashDir::with_root(base.join("Trash")).unwrap();
        (trash, work)
    }

    #[test]
    fn trash_then_list_then_restore_round_trips() {
        let (trash, work) = scratch("roundtrip");
        let f = work.join("notes.txt");
        std::fs::write(&f, b"hello").unwrap();

        let item = trash.trash(&f).unwrap();
        assert_eq!(item.trash_name, "notes.txt");
        assert_eq!(item.original_path, f);
        assert!(!f.exists(), "original is gone after trashing");
        assert!(item.trashed_file(&trash).exists(), "file is in the trash");

        let listed = trash.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0], item);

        trash.restore(&item).unwrap();
        assert!(f.exists(), "restore puts the file back");
        assert_eq!(std::fs::read(&f).unwrap(), b"hello");
        assert!(
            trash.list().unwrap().is_empty(),
            "record removed on restore"
        );
    }

    #[test]
    fn colliding_names_get_unique_trash_names() {
        let (trash, work) = scratch("collide");
        let a = work.join("dup.log");
        std::fs::write(&a, b"first").unwrap();
        let first = trash.trash(&a).unwrap();
        // recreate a file with the same basename and trash it again
        std::fs::write(&a, b"second").unwrap();
        let second = trash.trash(&a).unwrap();

        assert_eq!(first.trash_name, "dup.log");
        assert_eq!(second.trash_name, "dup.log.1");
        // both records present, both pointing at the same original path
        assert_eq!(trash.list().unwrap().len(), 2);
        assert_eq!(first.original_path, second.original_path);
    }

    #[test]
    fn empty_purges_files_and_records() {
        let (trash, work) = scratch("empty");
        for name in ["a.txt", "b.txt", "c.txt"] {
            let p = work.join(name);
            std::fs::write(&p, b"x").unwrap();
            trash.trash(&p).unwrap();
        }
        assert_eq!(trash.list().unwrap().len(), 3);
        let removed = trash.empty().unwrap();
        assert_eq!(removed, 3);
        assert!(trash.list().unwrap().is_empty());
        // files/ is empty too
        assert_eq!(std::fs::read_dir(trash.files_dir()).unwrap().count(), 0);
    }

    #[test]
    fn trashing_a_directory_moves_the_whole_tree() {
        let (trash, work) = scratch("dir");
        let dir = work.join("project");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub/deep.txt"), b"deep").unwrap();

        let item = trash.trash(&dir).unwrap();
        assert!(!dir.exists());
        assert!(item.trashed_file(&trash).join("sub/deep.txt").exists());

        trash.restore(&item).unwrap();
        assert_eq!(std::fs::read(dir.join("sub/deep.txt")).unwrap(), b"deep");
    }

    #[test]
    fn percent_encoding_round_trips_spaces_and_unicode() {
        for raw in ["/home/mm/a file.txt", "/tmp/résumé (1).pdf", "/p/100%done"] {
            let p = PathBuf::from(raw);
            let enc = percent_encode_path(&p);
            assert!(!enc.contains(' '), "spaces are escaped: {enc}");
            assert!(enc.starts_with('/'), "slashes stay bare: {enc}");
            assert_eq!(percent_decode_path(&enc), p, "round-trips: {raw}");
        }
    }

    #[test]
    fn trashinfo_parses_the_spec_shape() {
        let body = "[Trash Info]\nPath=/home/mm/a%20b.txt\nDeletionDate=2026-06-09T06:40:12\n";
        let item = parse_trashinfo("a b.txt", body).unwrap();
        assert_eq!(item.original_path, PathBuf::from("/home/mm/a b.txt"));
        assert_eq!(item.deletion_date, "2026-06-09T06:40:12");
        // missing Path key -> not a recoverable record
        assert!(parse_trashinfo("x", "[Trash Info]\nDeletionDate=now\n").is_none());
    }

    #[test]
    fn trashing_a_missing_source_errors() {
        let (trash, work) = scratch("missing");
        let err = trash.trash(&work.join("ghost")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn restore_refuses_to_clobber_an_existing_target() {
        let (trash, work) = scratch("clobber");
        let f = work.join("keep.txt");
        std::fs::write(&f, b"trashed").unwrap();
        let item = trash.trash(&f).unwrap();
        // something re-creates the original path before restore
        std::fs::write(&f, b"new").unwrap();
        let err = trash.restore(&item).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            std::fs::read(&f).unwrap(),
            b"new",
            "the new file is untouched"
        );
    }
}
