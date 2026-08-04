//! The durable, append-only, per-space **actor log**.
//!
//! Each node owns exactly one log per (space, actor) pair: the ordered record
//! of the signed events *this* actor authored in *that* space. It is the unit
//! Syncthing replicates — a peer receives a neighbour's log file, reads its
//! envelopes, and feeds them to [`merge`](crate::CollabEngine::merge). The trait
//! keeps the boundary injectable: the real [`FileActorLog`] appends JSON lines
//! to a replicable file; tests use the in-memory [`MemoryActorLog`].
//!
//! Append is **idempotent by [`EventId`]**: re-appending an event already in the
//! log is a no-op that returns `false`, so a crash between "sign" and "append",
//! or a replayed batch, never duplicates a line.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use mde_collab_types::ids::{EventId, SpaceId};
use mde_collab_types::{ActorId, CollabEventEnvelope};

use crate::error::Result;

/// An append-only log of one actor's signed events in one space.
pub trait ActorLog {
    /// Append `envelope` if its [`EventId`] is not already present. Returns
    /// `true` if it was newly appended, `false` if it was already there
    /// (idempotent). Errors only on an I/O/serialization failure.
    fn append(&mut self, envelope: &CollabEventEnvelope) -> Result<bool>;

    /// Every envelope in the log, in append order.
    fn read_all(&self) -> Result<Vec<CollabEventEnvelope>>;

    /// How many distinct events the log holds.
    fn len(&self) -> usize;

    /// Whether the log is empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// An in-memory actor log (tests, and the transient staging a worker may use
/// before flushing to disk). Ordered + deduplicated by [`EventId`].
#[derive(Debug, Default, Clone)]
pub struct MemoryActorLog {
    // BTreeMap keeps a stable, id-ordered `read_all`; dedup is the key.
    events: BTreeMap<EventId, CollabEventEnvelope>,
    // Preserve append order separately so replay order matches write order.
    order: Vec<EventId>,
}

impl MemoryActorLog {
    /// A fresh empty log.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl ActorLog for MemoryActorLog {
    fn append(&mut self, envelope: &CollabEventEnvelope) -> Result<bool> {
        if self.events.contains_key(&envelope.event_id) {
            return Ok(false);
        }
        self.order.push(envelope.event_id);
        self.events.insert(envelope.event_id, envelope.clone());
        Ok(true)
    }

    fn read_all(&self) -> Result<Vec<CollabEventEnvelope>> {
        Ok(self
            .order
            .iter()
            .filter_map(|id| self.events.get(id).cloned())
            .collect())
    }

    fn len(&self) -> usize {
        self.order.len()
    }
}

/// A Syncthing-replicable file actor log: one append-only JSON-lines file per
/// (space, actor) at `<root>/<space_id>/<actor>.jsonl`. Each line is one signed
/// [`CollabEventEnvelope`]; the directory tree is exactly what Syncthing mirrors
/// to peers.
#[derive(Debug)]
pub struct FileActorLog {
    path: PathBuf,
    // Ids already on disk — the idempotency guard, loaded on open.
    seen: std::collections::HashSet<EventId>,
    // Append order as loaded/written, so `read_all` matches disk order.
    order: Vec<EventId>,
    envelopes: BTreeMap<EventId, CollabEventEnvelope>,
    // Malformed/torn lines rejected while opening this log. A single crash-torn
    // append must not poison the actor's entire durable history forever; the
    // signed-envelope verifier remains the authority for every admitted line.
    rejected_lines: usize,
}

impl FileActorLog {
    /// The conventional path for a (space, actor) log under `root`.
    #[must_use]
    pub fn path_for(root: &Path, space: SpaceId, actor: &ActorId) -> PathBuf {
        root.join(space.to_string()).join(format!("{actor}.jsonl"))
    }

    /// Open (creating parent dirs) the log for `(space, actor)` under `root`,
    /// loading any already-persisted envelopes so appends stay idempotent and
    /// `read_all` returns the full history.
    pub fn open(root: &Path, space: SpaceId, actor: &ActorId) -> Result<Self> {
        let path = Self::path_for(root, space, actor);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut log = Self::empty(path);
        log.load()?;
        Ok(log)
    }

    /// Open an append-only handle without materializing the existing history.
    ///
    /// This is for an author that already guarantees fresh event IDs (the live
    /// collaboration worker uses UUIDv4 IDs and never replays command lanes).
    /// Durable replay remains the source of truth and uses [`Self::open`] when
    /// callers need the complete idempotency index or [`ActorLog::read_all`].
    /// Avoiding a full historical load keeps a hot actor log writable while a
    /// separate incremental projector catches up after restart.
    pub fn open_append_only(root: &Path, space: SpaceId, actor: &ActorId) -> Result<Self> {
        let path = Self::path_for(root, space, actor);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(Self::empty(path))
    }

    fn empty(path: PathBuf) -> Self {
        Self {
            path,
            seen: std::collections::HashSet::new(),
            order: Vec::new(),
            envelopes: BTreeMap::new(),
            rejected_lines: 0,
        }
    }

    fn load(&mut self) -> Result<()> {
        let file = match File::open(&self.path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        for line in BufReader::new(file).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let Ok(env) = serde_json::from_str::<CollabEventEnvelope>(&line) else {
                // Crash-torn JSONL and Syncthing conflict artifacts are
                // untrusted input. Reject only the malformed record so later
                // valid, signed records remain reachable and new events can be
                // appended. Callers can surface the count through diagnostics.
                self.rejected_lines = self.rejected_lines.saturating_add(1);
                continue;
            };
            if self.seen.insert(env.event_id) {
                self.order.push(env.event_id);
                self.envelopes.insert(env.event_id, env);
            }
        }
        Ok(())
    }

    /// Number of malformed records rejected while the file was opened.
    #[must_use]
    pub const fn rejected_line_count(&self) -> usize {
        self.rejected_lines
    }

    /// Ensure a prior crash-torn final record cannot be glued to the next JSON
    /// object. The torn bytes remain as one rejected forensic record; the new
    /// append starts on a clean JSONL boundary.
    fn ensure_append_boundary(&self) -> Result<()> {
        let mut file = match OpenOptions::new().read(true).open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        let len = file.metadata()?.len();
        if len == 0 {
            return Ok(());
        }
        file.seek(SeekFrom::End(-1))?;
        let mut tail = [0_u8; 1];
        file.read_exact(&mut tail)?;
        if tail[0] != b'\n' {
            let mut append = OpenOptions::new().append(true).open(&self.path)?;
            append.write_all(b"\n")?;
            append.flush()?;
        }
        Ok(())
    }
}

impl ActorLog for FileActorLog {
    fn append(&mut self, envelope: &CollabEventEnvelope) -> Result<bool> {
        if self.seen.contains(&envelope.event_id) {
            return Ok(false);
        }
        self.ensure_append_boundary()?;
        let mut line = serde_json::to_string(envelope)?;
        line.push('\n');
        // Append + flush so a crash leaves at most a torn trailing line, which
        // `load` skips (empty) or `serde` rejects — never a lost prefix.
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        file.write_all(line.as_bytes())?;
        file.flush()?;
        self.seen.insert(envelope.event_id);
        self.order.push(envelope.event_id);
        self.envelopes.insert(envelope.event_id, envelope.clone());
        Ok(true)
    }

    fn read_all(&self) -> Result<Vec<CollabEventEnvelope>> {
        Ok(self
            .order
            .iter()
            .filter_map(|id| self.envelopes.get(id).cloned())
            .collect())
    }

    fn len(&self) -> usize {
        self.order.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mde_collab_types::{ActorClock, CollabEventKind, SpaceKind};

    fn event(id: u128, space: SpaceId) -> CollabEventEnvelope {
        CollabEventEnvelope::new(
            EventId::from_uuid(uuid::Uuid::from_u128(id)),
            space,
            ActorId::new("seat-15"),
            ActorClock {
                wall_ms: id as u64,
                counter: 0,
            },
            id as i64,
            CollabEventKind::SpaceCreated {
                kind: SpaceKind::Team,
                name: "System · seat-15".into(),
            },
        )
    }

    #[test]
    fn corrupt_middle_line_is_isolated_and_valid_history_remains_appendable() {
        let root = tempfile::tempdir().expect("tempdir");
        let space = SpaceId::from_uuid(uuid::Uuid::from_u128(7));
        let actor = ActorId::new("seat-15");
        let path = FileActorLog::path_for(root.path(), space, &actor);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        let first = serde_json::to_string(&event(1, space)).expect("serialize first");
        let second = serde_json::to_string(&event(2, space)).expect("serialize second");
        std::fs::write(
            &path,
            format!("{first}\n{{\"event_id\":\"torn{{\"schema_version\":1}}\n{second}\n"),
        )
        .expect("write fixture");

        let mut log = FileActorLog::open(root.path(), space, &actor).expect("open resiliently");
        assert_eq!(log.rejected_line_count(), 1);
        assert_eq!(log.len(), 2);
        assert!(log
            .append(&event(3, space))
            .expect("append after corruption"));

        let reopened = FileActorLog::open(root.path(), space, &actor).expect("reopen");
        assert_eq!(reopened.rejected_line_count(), 1);
        assert_eq!(reopened.len(), 3);
    }

    #[test]
    fn torn_final_line_gets_a_boundary_before_the_next_append() {
        let root = tempfile::tempdir().expect("tempdir");
        let space = SpaceId::from_uuid(uuid::Uuid::from_u128(8));
        let actor = ActorId::new("seat-15");
        let path = FileActorLog::path_for(root.path(), space, &actor);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(&path, "{\"schema_version\":1").expect("write torn tail");

        let mut log = FileActorLog::open(root.path(), space, &actor).expect("open torn tail");
        assert_eq!(log.rejected_line_count(), 1);
        assert!(log
            .append(&event(9, space))
            .expect("append after torn tail"));

        let reopened = FileActorLog::open(root.path(), space, &actor).expect("reopen");
        assert_eq!(reopened.rejected_line_count(), 1);
        assert_eq!(reopened.len(), 1);
    }

    #[test]
    fn append_only_open_does_not_materialize_history_but_keeps_it_durable() {
        let root = tempfile::tempdir().expect("tempdir");
        let space = SpaceId::from_uuid(uuid::Uuid::from_u128(10));
        let actor = ActorId::new("seat-15");
        let mut initial = FileActorLog::open(root.path(), space, &actor).expect("open initial");
        assert!(initial.append(&event(10, space)).expect("append initial"));

        let mut live =
            FileActorLog::open_append_only(root.path(), space, &actor).expect("open append-only");
        assert_eq!(live.len(), 0, "historical rows stay out of the live writer");
        assert!(live.append(&event(11, space)).expect("append live"));

        let reopened = FileActorLog::open(root.path(), space, &actor).expect("reopen full log");
        assert_eq!(reopened.len(), 2);
        assert_eq!(
            reopened
                .read_all()
                .expect("read all")
                .into_iter()
                .map(|envelope| envelope.event_id)
                .collect::<Vec<_>>(),
            vec![event(10, space).event_id, event(11, space).event_id]
        );
    }
}
