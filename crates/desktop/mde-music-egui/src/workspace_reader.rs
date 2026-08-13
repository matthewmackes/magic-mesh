//! Fail-soft read access to the daemon-owned Music workspace snapshot.
//!
//! This is deliberately a read-only seam. Provider credentials, queue
//! mutations, and playback remain behind the daemon's typed action lanes; the
//! UI only consumes the latest validated retained state so storage/download
//! presentation cannot drift into a second authority.

use std::{collections::BTreeSet, path::PathBuf};

use mde_bus::persist::Persist;
use mde_musicd::bus_responder::WORKSPACE_STATE_TOPIC;
use mde_musicd::domain::MusicWorkspaceSnapshotV1;

/// Polls the latest daemon snapshot through one long-lived Bus connection.
///
/// Reusing the SQLite handle avoids opening and initializing the persistence
/// store on every UI poll. The shared inode-reopen hook keeps this long-lived
/// reader convergent when the Bus self-heals by replacing its index file.
#[derive(Debug)]
pub(crate) struct WorkspaceReader {
    bus_root: Option<PathBuf>,
    persist: Option<Persist>,
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
            persist: None,
            last_revision: 0,
        }
    }

    /// Read one newer valid snapshot, or `None` when the Bus is unavailable,
    /// the topic is empty, or the retained body fails contract validation.
    pub(crate) fn poll(&mut self) -> Option<MusicWorkspaceSnapshotV1> {
        let root = self.bus_root.clone()?;
        if self.persist.is_none() {
            self.persist = Persist::open(root).ok();
        }
        self.persist.as_mut()?.reopen_if_index_changed();
        let read_result = self
            .persist
            .as_ref()
            .expect("workspace Bus handle exists after open")
            .read_latest(WORKSPACE_STATE_TOPIC);
        let body = match read_result {
            Ok(Some(message)) => message.body?,
            Ok(None) => return None,
            Err(_) => {
                // Drop a poisoned/stale handle so the next bounded poll can
                // reopen the store. Never expose a partially read snapshot.
                self.persist = None;
                return None;
            }
        };
        let snapshot = match serde_json::from_str::<MusicWorkspaceSnapshotV1>(&body) {
            Ok(snapshot) => snapshot,
            Err(_) => {
                // A retained malformed row is provider/Bus loss, not an empty
                // workspace.  Drop the handle so the next poll can reopen a
                // repaired/replaced index rather than repeatedly trusting a
                // poisoned long-lived connection.
                self.persist = None;
                return None;
            }
        };
        if snapshot.validate().is_err()
            || !has_unambiguous_source_identities(&snapshot)
            || snapshot.revision <= self.last_revision
        {
            self.persist = None;
            return None;
        }
        self.last_revision = snapshot.revision;
        Some(snapshot)
    }
}

/// A source id is the authority key used by the surface when presenting daemon
/// reachability and capabilities. Reject the complete projection when that key
/// is blank, non-canonical, or repeated: accepting either conflicting row would
/// make provider order decide which daemon state the UI displays.
fn has_unambiguous_source_identities(snapshot: &MusicWorkspaceSnapshotV1) -> bool {
    let mut source_ids = BTreeSet::new();
    snapshot.sources.iter().all(|source| {
        let source_id = source.source_id.trim();
        !source_id.is_empty() && source_id == source.source_id && source_ids.insert(source_id)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mde_bus::hooks::config::Priority;
    use mde_musicd::domain::{
        MusicStorageSnapshot, MusicWorkspaceSnapshotV1, PlaybackSnapshot, ServerCapabilities,
    };

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

    #[test]
    fn reader_reopens_after_malformed_retained_row_and_accepts_repaired_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let persist = Persist::open(dir.path().to_path_buf()).unwrap();
        persist
            .write(
                WORKSPACE_STATE_TOPIC,
                Priority::Default,
                None,
                Some("{malformed music workspace"),
            )
            .unwrap();

        let mut reader = WorkspaceReader::from_root(Some(dir.path().to_path_buf()));
        assert!(reader.poll().is_none());

        // A repaired provider/Bus writes a valid retained row at the same
        // location.  The reader must reconnect instead of retaining the
        // poisoned handle from the malformed row.
        persist
            .write(
                WORKSPACE_STATE_TOPIC,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&snapshot(9)).unwrap()),
            )
            .unwrap();
        assert_eq!(reader.poll().unwrap().revision, 9);
    }

    #[test]
    fn reader_reopens_replaced_bus_index_and_converges() {
        let dir = tempfile::tempdir().unwrap();
        let persist = Persist::open(dir.path().to_path_buf()).unwrap();
        persist
            .write(
                WORKSPACE_STATE_TOPIC,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&snapshot(7)).unwrap()),
            )
            .unwrap();

        let mut reader = WorkspaceReader::from_root(Some(dir.path().to_path_buf()));
        assert_eq!(reader.poll().unwrap().revision, 7);

        // Simulate the Bus self-heal path replacing the live SQLite inode.
        // Remove SQLite sidecars in the isolated fixture before creating the
        // replacement, so the new handle starts from an independent index.
        drop(persist);
        let index = dir.path().join("index.sqlite");
        let old_index = dir.path().join("index.sqlite.old");
        for suffix in ["-wal", "-shm"] {
            let _ = std::fs::remove_file(dir.path().join(format!("index.sqlite{suffix}")));
        }
        std::fs::rename(&index, &old_index).unwrap();

        let replacement = Persist::open(dir.path().to_path_buf()).unwrap();
        replacement
            .write(
                WORKSPACE_STATE_TOPIC,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&snapshot(8)).unwrap()),
            )
            .unwrap();
        drop(replacement);

        assert_eq!(reader.poll().unwrap().revision, 8);
    }

    #[test]
    fn equivocated_source_identity_cannot_invent_daemon_reachability() {
        let dir = tempfile::tempdir().unwrap();
        let persist = Persist::open(dir.path().to_path_buf()).unwrap();
        let mut equivocated = snapshot(7);
        equivocated.any_source_reachable = true;
        equivocated.sources = vec![
            ServerCapabilities {
                source_id: "airsonic-main".to_owned(),
                api_profile: "subsonic-v1".to_owned(),
                reachable: false,
                authentication_required: true,
                features: ["browse".to_owned()].into_iter().collect(),
            },
            ServerCapabilities {
                source_id: "airsonic-main".to_owned(),
                api_profile: "subsonic-v2".to_owned(),
                reachable: true,
                authentication_required: false,
                features: ["stream".to_owned()].into_iter().collect(),
            },
        ];
        persist
            .write(
                WORKSPACE_STATE_TOPIC,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&equivocated).unwrap()),
            )
            .unwrap();

        let mut reader = WorkspaceReader::from_root(Some(dir.path().to_path_buf()));
        assert!(reader.poll().is_none());
        assert_eq!(reader.last_revision, 0);
    }
}
