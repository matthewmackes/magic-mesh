//! Fail-soft read access to the daemon-owned Music workspace snapshot.
//!
//! This is deliberately a read-only seam. Provider credentials, queue
//! mutations, and playback remain behind the daemon's typed action lanes; the
//! UI only consumes the latest validated retained state so storage/download
//! presentation cannot drift into a second authority.

use std::path::PathBuf;

use mde_bus::persist::Persist;
use mde_musicd::bus_responder::WORKSPACE_STATE_TOPIC;
use mde_musicd::domain::MusicWorkspaceSnapshotV1;

/// Polls the latest daemon snapshot without retaining a stale Bus connection.
#[derive(Debug, Clone)]
pub(crate) struct WorkspaceReader {
    bus_root: Option<PathBuf>,
    last_revision: u64,
}

impl WorkspaceReader {
    /// Resolve the same desktop-client Bus root as the shell.
    #[must_use]
    pub(crate) fn client() -> Self {
        Self::from_root(mde_bus::client_data_dir())
    }

    /// Construct a reader for a specific Bus root (also used by contract tests).
    #[must_use]
    pub(crate) fn from_root(bus_root: Option<PathBuf>) -> Self {
        Self {
            bus_root,
            last_revision: 0,
        }
    }

    /// Read one newer valid snapshot, or `None` when the Bus is unavailable,
    /// the topic is empty, or the retained body fails contract validation.
    pub(crate) fn poll(&mut self) -> Option<MusicWorkspaceSnapshotV1> {
        let root = self.bus_root.clone()?;
        let persist = Persist::open(root).ok()?;
        let body = persist.read_latest(WORKSPACE_STATE_TOPIC).ok()??.body?;
        let snapshot = serde_json::from_str::<MusicWorkspaceSnapshotV1>(&body).ok()?;
        if snapshot.validate().is_err() || snapshot.revision <= self.last_revision {
            return None;
        }
        self.last_revision = snapshot.revision;
        Some(snapshot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mde_bus::hooks::config::Priority;
    use mde_musicd::domain::{MusicStorageSnapshot, MusicWorkspaceSnapshotV1, PlaybackSnapshot};

    fn snapshot(revision: u64) -> MusicWorkspaceSnapshotV1 {
        MusicWorkspaceSnapshotV1 {
            schema_version: 1,
            revision,
            shelves: Vec::new(),
            bookmarks: Vec::new(),
            collections: Vec::new(),
            search: None,
            playback: PlaybackSnapshot {
                current: None,
                playing: false,
                position_ms: 0,
                duration_ms: None,
                volume_milli: 1000,
                shuffle: false,
                repeat: "off".to_string(),
                queue_revision: 0,
                seekable: false,
            },
            queue: Vec::new(),
            downloads: Vec::new(),
            storage: MusicStorageSnapshot {
                used_bytes: 10,
                cap_bytes: 100,
            },
            targets: Vec::new(),
            sources: Vec::new(),
            any_source_reachable: false,
        }
    }

    #[test]
    fn reader_accepts_newer_valid_snapshot_once() {
        let dir = tempfile::tempdir().unwrap();
        let persist = Persist::open(dir.path().to_path_buf()).unwrap();
        let body = serde_json::to_string(&snapshot(7)).unwrap();
        persist
            .write(WORKSPACE_STATE_TOPIC, Priority::Default, None, Some(&body))
            .unwrap();

        let mut reader = WorkspaceReader::from_root(Some(dir.path().to_path_buf()));
        assert_eq!(reader.poll().unwrap().revision, 7);
        assert!(reader.poll().is_none());
    }

    #[test]
    fn reader_rejects_invalid_snapshot_and_missing_bus() {
        assert!(WorkspaceReader::from_root(None).poll().is_none());
        let dir = tempfile::tempdir().unwrap();
        let persist = Persist::open(dir.path().to_path_buf()).unwrap();
        persist
            .write(
                WORKSPACE_STATE_TOPIC,
                Priority::Default,
                None,
                Some("{\"schema_version\":1}"),
            )
            .unwrap();
        assert!(WorkspaceReader::from_root(Some(dir.path().to_path_buf()))
            .poll()
            .is_none());
    }
}
