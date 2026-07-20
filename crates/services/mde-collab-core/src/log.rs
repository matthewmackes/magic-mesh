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
use std::io::{BufRead, BufReader, Write};
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
        let mut log = Self {
            path,
            seen: std::collections::HashSet::new(),
            order: Vec::new(),
            envelopes: BTreeMap::new(),
        };
        log.load()?;
        Ok(log)
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
            let env: CollabEventEnvelope = serde_json::from_str(&line)?;
            if self.seen.insert(env.event_id) {
                self.order.push(env.event_id);
                self.envelopes.insert(env.event_id, env);
            }
        }
        Ok(())
    }
}

impl ActorLog for FileActorLog {
    fn append(&mut self, envelope: &CollabEventEnvelope) -> Result<bool> {
        if self.seen.contains(&envelope.event_id) {
            return Ok(false);
        }
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
