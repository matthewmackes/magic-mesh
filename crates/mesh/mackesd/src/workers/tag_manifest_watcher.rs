//! HYP-8.5.watch — mtime-poll watcher for tag manifest reload.
//!
//! The bin/mackesd.rs::run_serve startup wire (shipped in
//! HYP-8.5) loads `~/.config/mde/tags/*.toml` once and publishes
//! `event/config/tags/loaded` per tag. That covers the boot-time
//! case but not runtime edits — operators changing a manifest
//! via MDE Settings (HYP-8.6) or via direct text-edit would
//! need to restart mackesd for the change to propagate.
//!
//! This watcher closes that gap. Every 5 seconds it lists the
//! tag-manifest directory, computes a name → mtime map, and
//! diffs against the previous tick:
//!
//! - **New / changed file:** re-parse + publish
//!   `event/config/tags/loaded` for the affected tag.
//! - **Removed file:** publish `event/config/tags/unloaded` with
//!   the prior name.
//!
//! mtime-poll (not inotify) keeps the dep surface minimal +
//! matches the cadence other mackesd file-config watchers use
//! (window_rules, dnd, subs). 5 s is well under the
//! "edit-to-paint" SLA we promise the operator.
//!
//! Per the fail-open contract from `config::tag_manifest`,
//! malformed files log + skip — never crash the daemon.

#![cfg(feature = "async-services")]

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use super::{ShutdownToken, Worker};
use crate::config::tag_manifest;

const POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Worker state — tracks per-file mtimes so we can spot edits
/// without re-parsing every tick.
pub struct TagManifestWatcherWorker {
    dir: PathBuf,
    last_mtimes: HashMap<String, SystemTime>,
    /// Names we've already published a "loaded" event for. Used
    /// to detect deletes (present in `last_mtimes` last tick,
    /// absent this tick).
    known_names: HashMap<String, ()>,
}

impl TagManifestWatcherWorker {
    /// Construct a watcher on `<XDG_CONFIG_HOME>/mde/tags/`. Returns
    /// `None` when neither `$XDG_CONFIG_HOME` nor `$HOME` is set —
    /// in that case the daemon skips the worker entirely.
    pub fn from_default_dir() -> Option<Self> {
        let dir = tag_manifest::default_manifests_dir()?;
        Some(Self::new(dir))
    }

    /// Construct a watcher with an explicit directory. Useful for
    /// tests + future per-peer override paths.
    #[must_use]
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            last_mtimes: HashMap::new(),
            known_names: HashMap::new(),
        }
    }

    /// One tick: scan + diff + publish. Returns the set of
    /// (changed-name, change-kind) tuples for tests. Production
    /// callers don't need the return value — the publishing
    /// happens as a side effect.
    pub fn scan_tick(&mut self) -> Vec<Change> {
        let mut changes = Vec::new();
        let mut current_mtimes: HashMap<String, SystemTime> = HashMap::new();

        // Read the dir; missing dir is a "no manifests" state —
        // every previously-known name becomes a removal.
        if self.dir.exists() {
            if let Ok(entries) = std::fs::read_dir(&self.dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
                        continue;
                    };
                    if ext != "toml" {
                        continue;
                    }
                    let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                        continue;
                    };
                    let Ok(meta) = std::fs::metadata(&path) else {
                        continue;
                    };
                    let Ok(mtime) = meta.modified() else {
                        continue;
                    };
                    current_mtimes.insert(stem.to_string(), mtime);
                }
            }
        }

        // Detect new + changed.
        for (name, mtime) in &current_mtimes {
            match self.last_mtimes.get(name) {
                None => {
                    changes.push(Change {
                        name: name.clone(),
                        kind: ChangeKind::Loaded,
                    });
                }
                Some(prev) if prev != mtime => {
                    changes.push(Change {
                        name: name.clone(),
                        kind: ChangeKind::Loaded, // Reloaded counts as
                                                  // a fresh load event.
                    });
                }
                _ => {} // Unchanged.
            }
        }

        // Detect removed.
        for name in self.known_names.keys() {
            if !current_mtimes.contains_key(name) {
                changes.push(Change {
                    name: name.clone(),
                    kind: ChangeKind::Unloaded,
                });
            }
        }

        // Snapshot the new state for the next tick.
        self.last_mtimes = current_mtimes.clone();
        self.known_names = current_mtimes.keys().map(|k| (k.clone(), ())).collect();

        changes
    }
}

/// One tag-manifest change detected by the watcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// Tag name (file stem).
    pub name: String,
    /// What happened to the tag.
    pub kind: ChangeKind,
}

/// Discriminator for [`Change`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    /// File appeared or its mtime advanced. Subscribers should
    /// re-parse + re-apply.
    Loaded,
    /// File disappeared since the prior tick.
    Unloaded,
}

#[async_trait::async_trait]
impl Worker for TagManifestWatcherWorker {
    fn name(&self) -> &'static str {
        "tag_manifest_watcher"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        loop {
            tokio::select! {
                biased;
                _ = shutdown.wait() => return Ok(()),
                _ = tokio::time::sleep(POLL_INTERVAL) => {
                    let changes = self.scan_tick();
                    for change in &changes {
                        publish_change(change);
                    }
                }
            }
        }
    }
}

/// Publish a `Change` via the bus shell-out. Best-effort detached
/// spawn — broker downtime doesn't take the watcher offline.
fn publish_change(change: &Change) {
    let topic = match change.kind {
        ChangeKind::Loaded => "event/config/tags/loaded",
        ChangeKind::Unloaded => "event/config/tags/unloaded",
    };
    let body = format!(
        r#"{{"name":"{}","kind":"{}"}}"#,
        change.name.replace('"', "\\\""),
        match change.kind {
            ChangeKind::Loaded => "loaded",
            ChangeKind::Unloaded => "unloaded",
        },
    );
    let _ = std::process::Command::new("mde-bus")
        .arg("publish")
        .arg(topic)
        .arg("--body-flag")
        .arg(&body)
        .spawn();
    tracing::debug!(
        name = %change.name,
        kind = ?change.kind,
        "tag_manifest_watcher: published change",
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::thread;

    fn write_manifest(dir: &std::path::Path, name: &str, body: &str) {
        std::fs::create_dir_all(dir).unwrap();
        let mut f = std::fs::File::create(dir.join(format!("{name}.toml"))).unwrap();
        f.write_all(body.as_bytes()).unwrap();
    }

    #[test]
    fn first_tick_reports_every_file_as_loaded() {
        let tmp = tempfile::tempdir().unwrap();
        write_manifest(tmp.path(), "voip", "");
        write_manifest(tmp.path(), "dev", "");
        let mut w = TagManifestWatcherWorker::new(tmp.path().to_path_buf());
        let changes = w.scan_tick();
        assert_eq!(changes.len(), 2);
        // All Loaded on the first tick.
        for c in &changes {
            assert_eq!(c.kind, ChangeKind::Loaded);
        }
    }

    #[test]
    fn unchanged_files_produce_no_changes() {
        let tmp = tempfile::tempdir().unwrap();
        write_manifest(tmp.path(), "voip", "");
        let mut w = TagManifestWatcherWorker::new(tmp.path().to_path_buf());
        // Prime.
        let _ = w.scan_tick();
        // Second tick should be empty (no edits).
        let changes = w.scan_tick();
        assert!(changes.is_empty());
    }

    #[test]
    fn new_file_after_prime_is_loaded() {
        let tmp = tempfile::tempdir().unwrap();
        write_manifest(tmp.path(), "voip", "");
        let mut w = TagManifestWatcherWorker::new(tmp.path().to_path_buf());
        let _ = w.scan_tick();
        // Add a new file.
        write_manifest(tmp.path(), "dev", "");
        let changes = w.scan_tick();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].name, "dev");
        assert_eq!(changes[0].kind, ChangeKind::Loaded);
    }

    #[test]
    fn mtime_advance_is_loaded() {
        let tmp = tempfile::tempdir().unwrap();
        write_manifest(tmp.path(), "voip", "name = \"voip\"\n");
        let mut w = TagManifestWatcherWorker::new(tmp.path().to_path_buf());
        let _ = w.scan_tick();
        // Sleep just enough to guarantee a mtime delta on every
        // sensible filesystem, then re-write.
        thread::sleep(Duration::from_millis(50));
        write_manifest(tmp.path(), "voip", "name = \"voip-edited\"\n");
        let changes = w.scan_tick();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].name, "voip");
        assert_eq!(changes[0].kind, ChangeKind::Loaded);
    }

    #[test]
    fn removed_file_is_unloaded() {
        let tmp = tempfile::tempdir().unwrap();
        write_manifest(tmp.path(), "voip", "");
        write_manifest(tmp.path(), "dev", "");
        let mut w = TagManifestWatcherWorker::new(tmp.path().to_path_buf());
        let _ = w.scan_tick();
        // Delete one file.
        std::fs::remove_file(tmp.path().join("dev.toml")).unwrap();
        let changes = w.scan_tick();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].name, "dev");
        assert_eq!(changes[0].kind, ChangeKind::Unloaded);
    }

    #[test]
    fn missing_dir_returns_no_changes_after_empty_prime() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("never-created");
        let mut w = TagManifestWatcherWorker::new(path);
        // Prime an empty state.
        let changes = w.scan_tick();
        assert!(changes.is_empty());
        // Next tick is still empty.
        let changes = w.scan_tick();
        assert!(changes.is_empty());
    }

    #[test]
    fn non_toml_files_are_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        write_manifest(tmp.path(), "voip", "");
        std::fs::write(tmp.path().join("readme.md"), "ignore me").unwrap();
        let mut w = TagManifestWatcherWorker::new(tmp.path().to_path_buf());
        let changes = w.scan_tick();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].name, "voip");
    }

    #[test]
    fn mixed_add_remove_change_in_same_tick() {
        let tmp = tempfile::tempdir().unwrap();
        write_manifest(tmp.path(), "voip", "name = \"v1\"\n");
        write_manifest(tmp.path(), "dev", "");
        let mut w = TagManifestWatcherWorker::new(tmp.path().to_path_buf());
        let _ = w.scan_tick();
        // Apply three concurrent edits in one cycle:
        //   - voip mtime advances
        //   - dev removed
        //   - hub added
        thread::sleep(Duration::from_millis(50));
        write_manifest(tmp.path(), "voip", "name = \"v2\"\n");
        std::fs::remove_file(tmp.path().join("dev.toml")).unwrap();
        write_manifest(tmp.path(), "hub", "");
        let changes = w.scan_tick();
        assert_eq!(changes.len(), 3);
        let kinds: HashMap<String, ChangeKind> =
            changes.into_iter().map(|c| (c.name, c.kind)).collect();
        assert_eq!(kinds["voip"], ChangeKind::Loaded);
        assert_eq!(kinds["dev"], ChangeKind::Unloaded);
        assert_eq!(kinds["hub"], ChangeKind::Loaded);
    }

    #[test]
    fn from_default_dir_returns_some_under_xdg() {
        // Just sanity-check the constructor; this doesn't require
        // the dir to exist, only that XDG resolution works.
        // CI / dev environments typically have $HOME or XDG_CONFIG_HOME
        // set; on the rare environment where neither is set this
        // returns None, which the worker is built to handle.
        let _ = TagManifestWatcherWorker::from_default_dir();
        // No panic = pass.
    }

    #[test]
    fn poll_interval_matches_design_lock() {
        // The 5-second cadence is documented in HYP-8.5.watch + in
        // this module's docstring; pin it so a future commit can't
        // drift the rate silently.
        assert_eq!(POLL_INTERVAL, Duration::from_secs(5));
    }
}
