//! WL-FUNC-027 — bounded user Places bookmarks.
//!
//! Persist lives here so the Places user section does not share
//! `model/mod.rs` with FolderPrefs (WL-FUNC-026). The session list the
//! sidebar paints still comes from [`FileBrowser`](crate::model::FileBrowser)
//! so pin/unpin/rename/reorder/remove stay on the existing apply path;
//! this module is the JSON contract, path validation, and count cap.
//!
//! Store path: `<config>/mcnf/files-bookmarks.json` (`XDG_CONFIG_HOME`, else
//! `$HOME/.config`). Tests inject a directory via [`BookmarkStore::open`].

use std::fs::File;
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// On-disk file name under the config directory.
pub const BOOKMARKS_FILE: &str = "files-bookmarks.json";
/// Hard cap on user pins (lock 21 is a workgroup sidebar, not an unbounded list).
pub const CAP: usize = 48;
/// Refuse a hostile oversize store instead of slurping it.
pub const MAX_BYTES: u64 = 64 * 1024;
/// Sidebar label cap (UTF-8 scalar values).
pub const LABEL_MAX: usize = 128;
/// Empty Places user-section copy.
pub const EMPTY_HINT: &str = "Pin a folder to keep it here.";

/// A user-pinnable Places bookmark. Path is a local backend route
/// (absolute `/…` or a `local:` slug); mesh peers stay a live roster section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserBookmark {
    /// Sidebar label.
    pub label: String,
    /// Backend `list()` path the bookmark navigates to.
    pub path: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct BookmarksFile {
    #[serde(default)]
    bookmarks: Vec<UserBookmark>,
}

/// Places user-section intents the sidebar header and bookmark context raise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlacesUserIntent {
    /// Pin the focused folder of `pane`.
    PinCurrent {
        /// Active pane index.
        pane: usize,
    },
    /// Rename the bookmark at `index`.
    Rename {
        /// Index into the user bookmark list.
        index: usize,
    },
    /// Move the bookmark at `index` by `delta` (−1 up, +1 down).
    Reorder {
        /// Index into the user bookmark list.
        index: usize,
        /// Display-order delta.
        delta: i32,
    },
    /// Remove the bookmark at `index`.
    Remove {
        /// Index into the user bookmark list.
        index: usize,
    },
}

/// Every Places user-section intent for bookmark `index` on `pane`.
#[must_use]
pub fn user_section_intents(pane: usize, index: usize) -> [PlacesUserIntent; 5] {
    [
        PlacesUserIntent::PinCurrent { pane },
        PlacesUserIntent::Rename { index },
        PlacesUserIntent::Reorder { index, delta: -1 },
        PlacesUserIntent::Reorder { index, delta: 1 },
        PlacesUserIntent::Remove { index },
    ]
}

/// Production path: `<config_home>/mcnf/files-bookmarks.json`.
#[must_use]
pub fn store_path_in(config_home: impl AsRef<Path>) -> PathBuf {
    config_home.as_ref().join("mcnf").join(BOOKMARKS_FILE)
}

/// Bounded Places bookmark store. Hostile paths, duplicates, and corrupt
/// files refuse or degrade in memory; on-disk bytes stay until a mutation.
#[derive(Debug, Clone)]
pub struct BookmarkStore {
    path: Option<PathBuf>,
    bookmarks: Vec<UserBookmark>,
    last_note: Option<String>,
    dirty: bool,
}

impl BookmarkStore {
    /// Load from `dir/files-bookmarks.json` (the test injection the browser uses).
    #[must_use]
    pub fn open(dir: impl AsRef<Path>) -> Self {
        let mut store = Self {
            path: Some(dir.as_ref().join(BOOKMARKS_FILE)),
            bookmarks: Vec::new(),
            last_note: None,
            dirty: false,
        };
        store.hydrate();
        store
    }

    /// Load from the process config home (`XDG_CONFIG_HOME` / `$HOME/.config`).
    #[must_use]
    pub fn from_env() -> Self {
        let mut store = Self {
            path: default_config_file(),
            bookmarks: Vec::new(),
            last_note: None,
            dirty: false,
        };
        store.hydrate();
        store
    }

    /// User pins in display order (above the fixed Places set).
    #[must_use]
    pub fn bookmarks(&self) -> &[UserBookmark] {
        &self.bookmarks
    }

    /// Most recent hydrate / mutation / persist note.
    #[must_use]
    pub fn last_note(&self) -> Option<&str> {
        self.last_note.as_deref()
    }

    /// On-disk path, if this store is backed by a file.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Pin a local route. Hostile paths, duplicates, and a full list refuse.
    pub fn pin(&mut self, route: impl AsRef<str>) -> Result<(), String> {
        let route = route.as_ref().to_string();
        if let Err(note) = validate_path(&route) {
            self.last_note = Some(note.clone());
            return Err(note);
        }
        if self.bookmarks.iter().any(|b| b.path == route) {
            let note = "That location is already pinned.".to_string();
            self.last_note = Some(note.clone());
            return Err(note);
        }
        if self.bookmarks.len() >= CAP {
            let note = format!("At most {CAP} bookmarks can be pinned.");
            self.last_note = Some(note.clone());
            return Err(note);
        }
        let label = sanitize_label(&label_for(&route), &route);
        self.bookmarks.push(UserBookmark { label, path: route });
        self.dirty = true;
        self.last_note = None;
        Ok(())
    }

    /// Remove the bookmark at `index`.
    pub fn unpin(&mut self, index: usize) {
        if index < self.bookmarks.len() {
            self.bookmarks.remove(index);
            self.dirty = true;
            self.last_note = None;
        }
    }

    /// Move bookmark `index` by `delta` (−1 up, +1 down).
    pub fn reorder(&mut self, index: usize, delta: i32) {
        let dest = index as i32 + delta;
        if dest < 0 || dest as usize >= self.bookmarks.len() || index >= self.bookmarks.len() {
            return;
        }
        self.bookmarks.swap(index, dest as usize);
        self.dirty = true;
    }

    /// Rename the bookmark at `index`. Hostile labels sanitize; empty refuses.
    pub fn rename(&mut self, index: usize, label: &str) -> Result<(), String> {
        let Some(bookmark) = self.bookmarks.get(index) else {
            let note = "No such bookmark.".to_string();
            self.last_note = Some(note.clone());
            return Err(note);
        };
        let name = sanitize_label(label, &bookmark.path);
        if name.is_empty() {
            let note = "A bookmark needs a name.".to_string();
            self.last_note = Some(note.clone());
            return Err(note);
        }
        if let Some(bookmark) = self.bookmarks.get_mut(index) {
            bookmark.label = name;
            self.dirty = true;
            self.last_note = None;
        }
        Ok(())
    }

    /// Write if dirty. Failed writes keep dirty state and surface a note.
    pub fn flush(&mut self) -> bool {
        if !self.dirty {
            return true;
        }
        if self.write() {
            self.dirty = false;
            true
        } else {
            false
        }
    }

    fn hydrate(&mut self) {
        let Some(path) = self.path.clone() else {
            return;
        };
        match load_file(&path) {
            Ok(file) => {
                let mut kept = Vec::new();
                let mut dropped = 0usize;
                let mut repaired = 0usize;
                for raw in file.bookmarks {
                    if validate_path(&raw.path).is_err()
                        || kept.iter().any(|b: &UserBookmark| b.path == raw.path)
                    {
                        dropped += 1;
                        continue;
                    }
                    if kept.len() >= CAP {
                        dropped += 1;
                        continue;
                    }
                    let label = sanitize_label(&raw.label, &raw.path);
                    if label != raw.label {
                        repaired += 1;
                    }
                    kept.push(UserBookmark {
                        label,
                        path: raw.path,
                    });
                }
                self.bookmarks = kept;
                if dropped > 0 {
                    self.last_note = Some(format!(
                        "Dropped {dropped} hostile or over-cap bookmark(s) — using the first {CAP} valid unique pins."
                    ));
                } else if repaired > 0 {
                    self.last_note = Some(format!(
                        "Repaired {repaired} bookmark label(s) from a hostile store."
                    ));
                }
            }
            Err(note) => {
                self.bookmarks.clear();
                self.last_note = Some(note);
            }
        }
    }

    fn write(&mut self) -> bool {
        let Some(path) = self.path.clone() else {
            return true;
        };
        match write_at(
            &path,
            &BookmarksFile {
                bookmarks: self.bookmarks.clone(),
            },
        ) {
            Ok(()) => true,
            Err(error) => {
                self.last_note = Some(format!(
                    "Bookmarks could not be saved to {}: {error}",
                    path.display()
                ));
                false
            }
        }
    }
}

impl Drop for BookmarkStore {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}

/// Load bookmarks from `path`, degrading hostile files to an error string.
pub fn load_at(path: &Path) -> Result<Vec<UserBookmark>, String> {
    load_file(path).map(|file| file.bookmarks)
}

fn load_file(path: &Path) -> Result<BookmarksFile, String> {
    match read_json_store(path) {
        Ok(None) => Ok(BookmarksFile::default()),
        Ok(Some(file)) => Ok(file),
        Err(note) => Err(note),
    }
}

/// Validate a pin route: local absolute or `local:` slug; no peers, dots, or controls.
pub fn validate_path(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err("A bookmark needs a path.".to_string());
    }
    if path.chars().any(|c| c.is_control()) {
        return Err("A bookmark path can't contain a control character.".to_string());
    }
    if path.starts_with("peer:") {
        return Err(
            "Mesh peers stay in the Mesh section — pin a local folder instead.".to_string(),
        );
    }
    if let Some(slug) = path.strip_prefix("local:") {
        if slug.is_empty()
            || slug != slug.trim()
            || slug.contains('/')
            || slug.contains('\\')
            || matches!(slug, "." | "..")
        {
            return Err("That local place isn't a valid bookmark.".to_string());
        }
        return Ok(());
    }
    let p = Path::new(path);
    if !p.is_absolute() {
        return Err("Pin an absolute folder path.".to_string());
    }
    if path.split('/').any(|seg| matches!(seg, "." | "..")) {
        return Err("A bookmark path can't contain '.' or '..' components.".to_string());
    }
    Ok(())
}

/// Strip controls, empty, and over-long labels; fall back to the route's name.
#[must_use]
pub fn sanitize_label(label: &str, route: &str) -> String {
    let cleaned: String = label.chars().filter(|c| !c.is_control()).collect();
    let trimmed = cleaned.trim();
    let base = if trimmed.is_empty() {
        let fallback: String = label_for(route)
            .chars()
            .filter(|c| !c.is_control())
            .collect();
        let fallback = fallback.trim().to_string();
        if fallback.is_empty() {
            route.to_string()
        } else {
            fallback
        }
    } else {
        trimmed.to_string()
    };
    if base.chars().count() > LABEL_MAX {
        base.chars().take(LABEL_MAX).collect()
    } else {
        base
    }
}

fn label_for(route: &str) -> String {
    if let Some(slug) = route.strip_prefix("local:") {
        return slug.to_string();
    }
    Path::new(route)
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(route)
        .to_string()
}

fn default_config_file() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(store_path_in(base))
}

fn read_json_store(path: &Path) -> Result<Option<BookmarksFile>, String> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("Couldn't read {}: {e}", path.display())),
    };
    if meta.file_type().is_symlink() {
        return Err(format!(
            "{} is a symlink — bookmarks were not loaded.",
            path.display()
        ));
    }
    if !meta.file_type().is_file() {
        return Err(format!(
            "{} is not a regular file — using defaults.",
            path.display()
        ));
    }
    if meta.len() > MAX_BYTES {
        return Err(format!(
            "{} is larger than {MAX_BYTES} bytes — using defaults.",
            path.display()
        ));
    }
    let mut options = File::options();
    options.read(true);
    options.custom_flags(0o400_000 | 0o2_000_000); // O_NOFOLLOW | O_CLOEXEC
    let file = options
        .open(path)
        .map_err(|e| format!("Couldn't open {}: {e}", path.display()))?;
    let opened = file
        .metadata()
        .map_err(|e| format!("Couldn't stat {}: {e}", path.display()))?;
    if opened.nlink() != 1 {
        return Err(format!(
            "{} is multiply linked — using defaults.",
            path.display()
        ));
    }
    let mut data = String::new();
    file.take(MAX_BYTES + 1)
        .read_to_string(&mut data)
        .map_err(|e| format!("Couldn't read {}: {e}", path.display()))?;
    if data.len() as u64 > MAX_BYTES {
        return Err(format!(
            "{} is larger than {MAX_BYTES} bytes — using defaults.",
            path.display()
        ));
    }
    serde_json::from_str(&data).map(Some).map_err(|e| {
        format!(
            "{} is not valid JSON ({e}) — using defaults.",
            path.display()
        )
    })
}

fn write_at(path: &Path, value: &BookmarksFile) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(value)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let stem = path
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("store"));
    let pid = std::process::id();
    let mut temp = None;
    for attempt in 0..32u32 {
        let candidate = parent.join(format!(
            ".{}.{}.{}.tmp",
            stem.to_string_lossy(),
            pid,
            attempt
        ));
        let mut options = std::fs::OpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(0o400_000 | 0o2_000_000); // O_NOFOLLOW | O_CLOEXEC
        match options.open(&candidate) {
            Ok(mut file) => {
                file.write_all(json.as_bytes())?;
                file.sync_all()?;
                temp = Some(candidate);
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    let temp = temp.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate a temporary bookmarks file",
        )
    })?;
    let result = std::fs::rename(&temp, path);
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{
        load_at, sanitize_label, store_path_in, user_section_intents, validate_path, BookmarkStore,
        PlacesUserIntent, BOOKMARKS_FILE, CAP, EMPTY_HINT, LABEL_MAX, MAX_BYTES,
    };
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::Path;

    fn seed_json(dir: &Path, body: &[u8]) {
        std::fs::write(dir.join(BOOKMARKS_FILE), body).expect("seed");
    }

    #[test]
    fn store_path_is_config_mcnf_files_bookmarks_json() {
        assert_eq!(
            store_path_in("/root/.config"),
            Path::new("/root/.config/mcnf/files-bookmarks.json")
        );
        assert_eq!(EMPTY_HINT, "Pin a folder to keep it here.");
    }

    #[test]
    fn user_section_intents_cover_pin_rename_reorder_remove() {
        let inventory = user_section_intents(0, 1);
        assert_eq!(inventory[0], PlacesUserIntent::PinCurrent { pane: 0 });
        assert!(inventory.contains(&PlacesUserIntent::Rename { index: 1 }));
        assert!(inventory.contains(&PlacesUserIntent::Reorder {
            index: 1,
            delta: -1
        }));
        assert!(inventory.contains(&PlacesUserIntent::Reorder { index: 1, delta: 1 }));
        assert!(inventory.contains(&PlacesUserIntent::Remove { index: 1 }));
    }

    #[test]
    fn pin_rename_reorder_remove_survive_reopen() {
        let dir = tempfile::tempdir().expect("dir");
        let mut store = BookmarkStore::open(dir.path());
        store.pin("/data/alpha").expect("pin alpha");
        store.pin("/data/bravo").expect("pin bravo");
        store.pin("local:downloads").expect("pin slug");
        store.reorder(2, -1);
        store.reorder(1, -1);
        store.rename(0, "Downloads").expect("rename");
        store.unpin(1);
        assert!(store.flush());
        drop(store);

        let reopened = BookmarkStore::open(dir.path());
        assert_eq!(reopened.bookmarks().len(), 2);
        assert_eq!(reopened.bookmarks()[0].label, "Downloads");
        assert_eq!(reopened.bookmarks()[0].path, "local:downloads");
        assert_eq!(reopened.bookmarks()[1].path, "/data/bravo");
        assert!(reopened.path().is_some_and(|p| p.ends_with(BOOKMARKS_FILE)));
    }

    #[test]
    fn duplicate_and_hostile_pins_refuse() {
        let dir = tempfile::tempdir().expect("dir");
        let mut store = BookmarkStore::open(dir.path());
        store.pin("/home/me/docs").expect("pin");
        let dup = store.pin("/home/me/docs").expect_err("duplicate");
        assert!(dup.contains("already pinned"), "{dup}");
        assert_eq!(store.bookmarks().len(), 1);

        for hostile in [
            "peer:oak",
            "/tmp/../etc/passwd",
            "relative",
            "local:..",
            "/tmp/./secret",
            "local:home\n",
            "",
        ] {
            let err = validate_path(hostile).expect_err(hostile);
            assert!(!err.is_empty(), "{hostile}");
            store.pin(hostile).expect_err(hostile);
        }
        assert_eq!(store.bookmarks().len(), 1);
        assert!(validate_path("/home/me/docs").is_ok());
        assert!(validate_path("local:home").is_ok());
    }

    #[test]
    fn missing_store_is_empty_defaults() {
        let missing = load_at(Path::new("/no/such/files-bookmarks.json")).expect("missing");
        assert!(missing.is_empty());
        let dir = tempfile::tempdir().expect("dir");
        let store = BookmarkStore::open(dir.path());
        assert!(store.bookmarks().is_empty());
        assert!(store.last_note().is_none());
    }

    #[test]
    fn corrupt_store_degrades_and_keeps_bytes_until_mutation() {
        let dir = tempfile::tempdir().expect("dir");
        let corrupt = br#"{"bookmarks": [not valid json"#;
        seed_json(dir.path(), corrupt);
        let store = BookmarkStore::open(dir.path());
        assert!(store.bookmarks().is_empty());
        assert!(
            store
                .last_note()
                .is_some_and(|n| n.contains("not valid JSON") || n.contains("defaults")),
            "{:?}",
            store.last_note()
        );
        drop(store);
        assert_eq!(
            std::fs::read(dir.path().join(BOOKMARKS_FILE)).expect("bytes"),
            corrupt
        );

        let mut store = BookmarkStore::open(dir.path());
        store.pin("/tmp/docs").expect("pin after corrupt");
        assert!(store.flush());
        drop(store);
        let reloaded = BookmarkStore::open(dir.path());
        assert_eq!(reloaded.bookmarks().len(), 1);
        assert_eq!(reloaded.bookmarks()[0].path, "/tmp/docs");
    }

    #[test]
    fn symlink_store_refuses_and_leaves_target_intact() {
        let dir = tempfile::tempdir().expect("dir");
        let target = dir.path().join("outside.json");
        std::fs::write(&target, b"secret sibling bytes").expect("seed");
        std::os::unix::fs::symlink(&target, dir.path().join(BOOKMARKS_FILE)).expect("symlink");
        let note = load_at(&dir.path().join(BOOKMARKS_FILE)).expect_err("symlink");
        assert!(
            note.contains("symlink") && note.contains("bookmarks"),
            "{note}"
        );
        let store = BookmarkStore::open(dir.path());
        assert!(store.bookmarks().is_empty());
        assert!(
            store
                .last_note()
                .is_some_and(|n| n.contains("symlink") && n.contains("bookmarks")),
            "{:?}",
            store.last_note()
        );
        assert_eq!(
            std::fs::read(&target).expect("target"),
            b"secret sibling bytes"
        );
        assert!(std::fs::symlink_metadata(dir.path().join(BOOKMARKS_FILE))
            .expect("meta")
            .file_type()
            .is_symlink());
    }

    #[test]
    fn write_failure_stays_dirty_and_visible() {
        let dir = tempfile::tempdir().expect("dir");
        let blocked = dir.path().join("not-a-directory");
        std::fs::write(&blocked, b"file blocker").expect("blocker");
        let mut store = BookmarkStore::open(&blocked);
        store.pin("/tmp/docs").expect("in-memory pin");
        assert!(!store.flush());
        assert_eq!(store.bookmarks().len(), 1);
        assert!(
            store
                .last_note()
                .is_some_and(|n| n.contains("Bookmarks could not be saved")),
            "{:?}",
            store.last_note()
        );
        assert!(!blocked.join(BOOKMARKS_FILE).exists());

        std::fs::remove_file(&blocked).expect("unblock");
        std::fs::create_dir(&blocked).expect("dir");
        assert!(store.flush());
        let saved = BookmarkStore::open(&blocked);
        assert_eq!(saved.bookmarks(), store.bookmarks());
        let mode = std::fs::metadata(blocked.join(BOOKMARKS_FILE))
            .expect("meta")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "store is operator-private");
    }

    #[test]
    fn hydrate_caps_hostile_entries_and_refuses_the_next_pin() {
        let dir = tempfile::tempdir().expect("dir");
        let mut entries = Vec::new();
        entries.push(serde_json::json!({"label": "keep-0", "path": "/tmp/mcnf-bm-0"}));
        entries.push(serde_json::json!({"label": "peer", "path": "peer:oak"}));
        entries.push(serde_json::json!({"label": "escape", "path": "/tmp/../etc/passwd"}));
        entries.push(serde_json::json!({"label": "relative", "path": "relative"}));
        entries.push(serde_json::json!({"label": "dotdot-slug", "path": "local:.."}));
        entries.push(serde_json::json!({"label": "curdir", "path": "/tmp/./secret"}));
        entries.push(serde_json::json!({"label": "dup", "path": "/tmp/mcnf-bm-0"}));
        entries.push(serde_json::json!({"label": "", "path": "/tmp/mcnf-bm-empty"}));
        entries.push(serde_json::json!({
            "label": "X".repeat(LABEL_MAX + 16),
            "path": "/tmp/mcnf-bm-long"
        }));
        for i in 1..CAP {
            entries.push(serde_json::json!({
                "label": format!("keep-{i}"),
                "path": format!("/tmp/mcnf-bm-{i}")
            }));
        }
        entries.push(serde_json::json!({"label": "overflow", "path": "/tmp/mcnf-bm-overflow"}));
        seed_json(
            dir.path(),
            &serde_json::to_vec_pretty(&serde_json::json!({ "bookmarks": entries })).expect("json"),
        );
        let raw_before = std::fs::read(dir.path().join(BOOKMARKS_FILE)).expect("seed");

        let store = BookmarkStore::open(dir.path());
        assert_eq!(store.bookmarks().len(), CAP);
        assert_eq!(store.bookmarks()[0].path, "/tmp/mcnf-bm-0");
        assert!(store
            .bookmarks()
            .iter()
            .any(|bm| bm.path == "/tmp/mcnf-bm-empty" && !bm.label.is_empty()));
        let long = store
            .bookmarks()
            .iter()
            .find(|bm| bm.path == "/tmp/mcnf-bm-long")
            .expect("long");
        assert_eq!(long.label.chars().count(), LABEL_MAX);
        assert!(store.bookmarks().iter().all(|bm| {
            bm.path != "peer:oak"
                && bm.path != "/tmp/../etc/passwd"
                && bm.path != "relative"
                && bm.path != "local:.."
                && bm.path != "/tmp/./secret"
                && bm.path != "/tmp/mcnf-bm-overflow"
        }));
        assert_eq!(
            store
                .bookmarks()
                .iter()
                .filter(|bm| bm.path == "/tmp/mcnf-bm-0")
                .count(),
            1
        );
        assert!(
            store
                .last_note()
                .is_some_and(|n| n.contains("Dropped") && n.contains("hostile")),
            "{:?}",
            store.last_note()
        );
        drop(store);
        assert_eq!(
            std::fs::read(dir.path().join(BOOKMARKS_FILE)).expect("untouched"),
            raw_before
        );

        let mut store = BookmarkStore::open(dir.path());
        let err = store.pin("/tmp/extra").expect_err("cap");
        assert!(
            err.contains("At most") && err.contains(&CAP.to_string()),
            "{err}"
        );
        assert_eq!(store.bookmarks().len(), CAP);
    }

    #[test]
    fn oversize_store_degrades_to_defaults() {
        let dir = tempfile::tempdir().expect("dir");
        seed_json(dir.path(), &vec![b'x'; (MAX_BYTES as usize) + 8]);
        let err = load_at(&dir.path().join(BOOKMARKS_FILE)).expect_err("oversize");
        assert!(err.contains("larger than"), "{err}");
        let store = BookmarkStore::open(dir.path());
        assert!(store.bookmarks().is_empty());
        assert!(store.last_note().is_some_and(|n| n.contains("larger than")));
    }

    #[test]
    fn sanitize_label_strips_controls_and_caps_length() {
        assert_eq!(sanitize_label("Docs\n", "/tmp/docs"), "Docs");
        assert_eq!(sanitize_label("   ", "/tmp/secret"), "secret");
        assert_eq!(
            sanitize_label(&"X".repeat(LABEL_MAX + 4), "/tmp/x")
                .chars()
                .count(),
            LABEL_MAX
        );
        assert_eq!(sanitize_label("", "local:downloads"), "downloads");
    }
}
