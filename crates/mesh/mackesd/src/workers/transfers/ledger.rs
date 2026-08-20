//! TRANSFERS-1 — the persistent job ledger (Q11).
//!
//! Every job is one `<id>.json` file under `<store_root>/ledger/`, written with the
//! atomic temp-then-rename idiom the mesh uses everywhere (SEC-5 / `node_grade`), so
//! a crash mid-write never leaves a torn record. The ledger IS the durable state:
//! the daemon holds no authoritative in-memory queue, so a restart re-reads the
//! directory and the history survives reboots (Q11). Node-LOCAL on purpose — a
//! node's transfer queue is its own, not replicated (unlike the peer directory).

#![cfg(feature = "async-services")]

use std::io;
use std::path::{Path, PathBuf};

use super::job::TransferJob;
use super::v2::{project_queued_job, TransferV2Identity, TransferV2ProjectionError};
use mde_collab_types::{
    TransferControlV2, TransferId, TransferJobV2, TransferPhase, TransferState as V2State,
};

/// Transfer records are small JSON envelopes. Keep hostile or corrupt files
/// bounded before `serde_json` materializes their strings, while leaving room
/// for long paths and honest failure details.
const MAX_LEDGER_RECORD_BYTES: usize = 1024 * 1024;

/// The on-disk ledger — one directory of `<id>.json` records.
#[derive(Debug, Clone)]
pub struct Ledger {
    dir: PathBuf,
}

/// Durable node-local ledger for strict V2 transfer jobs.
///
/// V2 records remain separate from the legacy ledger: opaque Files identities
/// must never be reinterpreted as the legacy executor's raw paths or URLs.
#[derive(Debug, Clone)]
pub struct V2Ledger {
    dir: PathBuf,
}

/// A refused V2 ledger mutation.
#[derive(Debug)]
pub enum V2LedgerError {
    /// A node-local durable write/read failed.
    Io(io::Error),
    /// The shared V2 contract rejected the job shape.
    Invalid(mde_collab_types::TransferJobV2ValidationError),
    /// A submission attempted to replace an existing transfer identity.
    Duplicate(TransferId),
    /// A control named no admitted record.
    NotFound(TransferId),
    /// New submissions must enter the ledger in the queued state.
    InitialState(V2State),
    /// The requested state transition is not allowed by the shared contract.
    IllegalControl {
        /// Requested lifecycle operation.
        control: TransferControlV2,
        /// Durable state that refused the operation.
        state: V2State,
    },
    /// The caller-supplied update clock did not advance the durable record.
    StaleUpdate {
        /// Current durable update timestamp.
        current_unix_ms: u64,
        /// Refused timestamp from the command.
        proposed_unix_ms: u64,
    },
}

impl std::fmt::Display for V2LedgerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "V2 transfer ledger I/O failed: {error}"),
            Self::Invalid(error) => write!(formatter, "V2 transfer rejected: {error}"),
            Self::Duplicate(id) => write!(formatter, "V2 transfer {id} already exists"),
            Self::NotFound(id) => write!(formatter, "V2 transfer {id} was not found"),
            Self::InitialState(state) => {
                write!(formatter, "V2 submission must be queued, not {state:?}")
            }
            Self::IllegalControl { control, state } => {
                write!(formatter, "V2 control {control:?} is illegal for {state:?}")
            }
            Self::StaleUpdate {
                current_unix_ms,
                proposed_unix_ms,
            } => write!(
                formatter,
                "V2 update timestamp {proposed_unix_ms} does not advance {current_unix_ms}"
            ),
        }
    }
}

impl std::error::Error for V2LedgerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Invalid(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for V2LedgerError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl V2Ledger {
    /// Open the strict ledger under `<store_root>/ledger-v2`.
    pub fn open(store_root: &Path) -> io::Result<Self> {
        let dir = store_root.join("ledger-v2");
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    /// Return the V2 record directory for read-only inspection and tests.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, id: TransferId) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    /// Admit a new queued V2 job without resolving its opaque endpoint.
    pub fn submit(&self, job: TransferJobV2) -> Result<(), V2LedgerError> {
        job.validate().map_err(V2LedgerError::Invalid)?;
        if job.state != V2State::Queued {
            return Err(V2LedgerError::InitialState(job.state));
        }
        self.insert(&job)
    }

    /// Read one admitted V2 record. Corrupt/oversized/symlink records fail closed.
    #[must_use]
    pub fn get(&self, id: TransferId) -> Option<TransferJobV2> {
        let body = read_record(&self.path(id)).ok()?;
        TransferJobV2::from_json(&body).ok()
    }

    /// Apply a legal operator control with a caller-supplied monotonic timestamp.
    pub fn apply_control(
        &self,
        id: TransferId,
        control: TransferControlV2,
        updated_unix_ms: u64,
    ) -> Result<TransferJobV2, V2LedgerError> {
        let mut job = self.get(id).ok_or(V2LedgerError::NotFound(id))?;
        if updated_unix_ms <= job.updated_unix_ms {
            return Err(V2LedgerError::StaleUpdate {
                current_unix_ms: job.updated_unix_ms,
                proposed_unix_ms: updated_unix_ms,
            });
        }
        if !job.can_control(control) {
            return Err(V2LedgerError::IllegalControl {
                control,
                state: job.state,
            });
        }

        match control {
            TransferControlV2::Pause => {
                job.state = V2State::Paused;
                job.progress.phase = TransferPhase::Paused;
            }
            TransferControlV2::Resume | TransferControlV2::Retry => {
                job.state = V2State::Queued;
                job.progress.phase = TransferPhase::Queued;
            }
            TransferControlV2::Cancel => {
                job.state = V2State::Canceled;
                job.progress.phase = TransferPhase::Canceled;
            }
        }
        job.progress.bytes_per_second = None;
        job.progress.error = None;
        job.updated_unix_ms = updated_unix_ms;
        job.validate().map_err(V2LedgerError::Invalid)?;
        self.upsert(&job)?;
        Ok(job)
    }

    /// Return all admitted V2 records in deterministic creation/id order.
    #[must_use]
    pub fn load_all(&self) -> Vec<TransferJobV2> {
        let mut jobs = Vec::new();
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return jobs;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("json")
                || path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with('.'))
            {
                continue;
            }
            if let Ok(body) = read_record(&path) {
                if let Ok(job) = TransferJobV2::from_json(&body) {
                    jobs.push(job);
                }
            }
        }
        jobs.sort_by(|left, right| {
            left.created_unix_ms
                .cmp(&right.created_unix_ms)
                .then_with(|| left.transfer.to_string().cmp(&right.transfer.to_string()))
        });
        jobs
    }

    fn upsert(&self, job: &TransferJobV2) -> Result<(), V2LedgerError> {
        self.write_record(job, false)
    }

    /// Install a new record without ever replacing an existing transfer id.
    ///
    /// A prior `exists` check followed by `rename` was vulnerable to two
    /// concurrent/replayed admissions racing between those operations.  The
    /// temporary file is fully written first, then a same-directory hard-link
    /// provides the atomic no-replace commit; an existing record (including a
    /// hostile symlink) wins the race and is never overwritten.
    fn insert(&self, job: &TransferJobV2) -> Result<(), V2LedgerError> {
        match self.write_record(job, true) {
            Err(V2LedgerError::Io(error)) if error.kind() == io::ErrorKind::AlreadyExists => {
                Err(V2LedgerError::Duplicate(job.transfer))
            }
            result => result,
        }
    }

    fn write_record(&self, job: &TransferJobV2, no_replace: bool) -> Result<(), V2LedgerError> {
        let body = serde_json::to_string_pretty(job)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if body.len() > MAX_LEDGER_RECORD_BYTES {
            return Err(V2LedgerError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                "V2 ledger record exceeds the byte limit",
            )));
        }
        let tmp = self.dir.join(format!(".{}.json.tmp", job.transfer));
        std::fs::write(&tmp, body)?;
        let result = if no_replace {
            std::fs::hard_link(&tmp, self.path(job.transfer))
        } else {
            std::fs::rename(&tmp, self.path(job.transfer))
        };
        let cleanup = std::fs::remove_file(&tmp);
        result?;
        cleanup.or_else(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                Ok(())
            } else {
                Err(error)
            }
        })?;
        Ok(())
    }
}

impl Ledger {
    /// Open (creating) the ledger under `store_root` (the `ledger/` subdir).
    ///
    /// # Errors
    /// Fails if the directory can't be created.
    pub fn open(store_root: &Path) -> io::Result<Self> {
        let dir = store_root.join("ledger");
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    /// The ledger directory (records live directly under it).
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The on-disk path for a job id.
    fn path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    /// Insert or replace a job's record (atomic temp + rename).
    ///
    /// # Errors
    /// Serialization or IO failures.
    pub fn upsert(&self, job: &TransferJob) -> io::Result<()> {
        let body = serde_json::to_string_pretty(job)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let tmp = self.dir.join(format!(".{}.json.tmp", job.id));
        std::fs::write(&tmp, body)?;
        std::fs::rename(&tmp, self.path(&job.id))
    }

    /// Read one job by id (`None` when absent or unparseable).
    #[must_use]
    pub fn get(&self, id: &str) -> Option<TransferJob> {
        let data = read_record(&self.path(id)).ok()?;
        serde_json::from_str(&data).ok()
    }

    /// Remove a job's record. Absent is not an error (idempotent cancel/clear).
    ///
    /// # Errors
    /// An IO failure other than "not found".
    pub fn remove(&self, id: &str) -> io::Result<()> {
        match std::fs::remove_file(self.path(id)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Load every job, sorted by submit time then id (the stable FIFO render order).
    /// Half-replicated / junk files (a stray `.tmp`, a non-json, an unparseable
    /// record) are skipped rather than failing the whole read.
    #[must_use]
    pub fn load_all(&self) -> Vec<TransferJob> {
        let mut out = Vec::new();
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return out;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with('.'))
            {
                continue;
            }
            if let Ok(data) = read_record(&path) {
                if let Ok(job) = serde_json::from_str::<TransferJob>(&data) {
                    out.push(job);
                }
            }
        }
        out.sort_by(|a, b| {
            a.created_ms
                .cmp(&b.created_ms)
                .then_with(|| a.id.cmp(&b.id))
        });
        out
    }

    /// Project one authoritative queued ledger row into the strict shared V2
    /// contract using typed identity supplied by the Files authority.
    ///
    /// The projection never parses or copies the legacy source/destination
    /// strings and never creates a `FileRefId`; rows whose state/progress cannot
    /// be represented losslessly are rejected by the bridge.
    ///
    /// # Errors
    /// Returns a typed not-found, legacy-shape, or shared-contract admission
    /// error.
    pub fn project_v2(
        &self,
        id: &str,
        identity: &TransferV2Identity,
    ) -> Result<TransferJobV2, TransferV2ProjectionError> {
        let job = self
            .get(id)
            .ok_or_else(|| TransferV2ProjectionError::LedgerJobNotFound(id.to_string()))?;
        project_queued_job(&job, identity)
    }
}

/// Read a ledger record through a descriptor that refuses a final symlink and
/// a blocking special file. The descriptor metadata and byte ceiling are
/// checked before the contents reach `serde_json`; the second bounded read
/// check also catches a regular file that grows after it was opened.
fn read_record(path: &Path) -> io::Result<String> {
    use std::io::Read as _;

    #[cfg(unix)]
    let file = {
        use rustix::fs::{Mode, OFlags};

        let fd = rustix::fs::open(
            path,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io::Error::from)?;
        std::fs::File::from(fd)
    };

    #[cfg(not(unix))]
    let file = {
        let metadata = std::fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "ledger record is a final symlink",
            ));
        }
        std::fs::File::open(path)?
    };

    let metadata = file.metadata()?;
    if !metadata.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "ledger record is not a regular file",
        ));
    }
    if metadata.len() > MAX_LEDGER_RECORD_BYTES as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "ledger record exceeds the byte limit",
        ));
    }

    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take((MAX_LEDGER_RECORD_BYTES as u64).saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_LEDGER_RECORD_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "ledger record exceeds the byte limit",
        ));
    }
    String::from_utf8(bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workers::transfers::job::{Method, TransferPolicy, TransferState};
    use mde_collab_types::{
        ChecksumPolicy, FileRefId, OpaqueNodeRef, TransferDirection, TransferEndpoint,
        TransferKind, TransferLocation, TransferOperation,
    };
    use uuid::Uuid;

    fn job(source: &str) -> TransferJob {
        TransferJob::new(source, "/dest", Method::Rsync, TransferPolicy::default())
    }

    fn v2_job(seed: u128, created_unix_ms: u64) -> TransferJobV2 {
        TransferJobV2::new(
            TransferId::from_uuid(Uuid::from_u128(seed)),
            TransferKind::Mesh,
            TransferEndpoint::new(
                TransferLocation::Mesh {
                    node: OpaqueNodeRef::new("peer-oak").unwrap(),
                    object: FileRefId::from_uuid(Uuid::from_u128(seed + 1)),
                },
                TransferLocation::Local {
                    object: FileRefId::from_uuid(Uuid::from_u128(seed + 2)),
                },
            ),
            TransferOperation::Copy {
                direction: TransferDirection::Inbound,
            },
            ChecksumPolicy::verify(),
            None,
            created_unix_ms,
        )
        .unwrap()
    }

    #[test]
    fn upsert_get_remove_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let ledger = Ledger::open(tmp.path()).unwrap();
        let j = job("/a");
        ledger.upsert(&j).unwrap();
        assert_eq!(ledger.get(&j.id).unwrap(), j);
        ledger.remove(&j.id).unwrap();
        assert!(ledger.get(&j.id).is_none());
        // Removing an absent id is a no-op, not an error.
        ledger.remove(&j.id).unwrap();
    }

    #[test]
    fn load_all_is_time_ordered_and_skips_junk() {
        let tmp = tempfile::tempdir().unwrap();
        let ledger = Ledger::open(tmp.path()).unwrap();
        let mut a = job("/a");
        a.created_ms = 100;
        let mut b = job("/b");
        b.created_ms = 50;
        ledger.upsert(&a).unwrap();
        ledger.upsert(&b).unwrap();
        // A stray temp + a non-json + a corrupt record are all ignored.
        std::fs::write(ledger.dir().join(".x.json.tmp"), "{}").unwrap();
        std::fs::write(ledger.dir().join("notes.txt"), "hi").unwrap();
        std::fs::write(ledger.dir().join("broken.json"), "{ not json").unwrap();
        let all = ledger.load_all();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].source, "/b", "earlier created_ms sorts first");
        assert_eq!(all[1].source, "/a");
    }

    #[test]
    fn oversized_records_are_skipped_before_json_parsing() {
        let tmp = tempfile::tempdir().unwrap();
        let ledger = Ledger::open(tmp.path()).unwrap();
        let path = ledger.path("oversized");
        std::fs::write(&path, vec![b'x'; MAX_LEDGER_RECORD_BYTES + 1]).unwrap();

        assert!(ledger.get("oversized").is_none());
        assert!(ledger.load_all().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn final_symlink_records_are_skipped_without_following_them() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let ledger = Ledger::open(tmp.path()).unwrap();
        let target_job = job("/target");
        ledger.upsert(&target_job).unwrap();
        symlink(ledger.path(&target_job.id), ledger.path("linked")).unwrap();

        assert!(ledger.get("linked").is_none());
        let all = ledger.load_all();
        assert_eq!(all, vec![target_job]);
    }

    #[cfg(unix)]
    #[test]
    fn non_regular_records_are_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let ledger = Ledger::open(tmp.path()).unwrap();
        std::fs::create_dir(ledger.path("directory")).unwrap();

        assert!(ledger.get("directory").is_none());
        assert!(ledger.load_all().is_empty());
    }

    #[test]
    fn records_survive_reopen() {
        let tmp = tempfile::tempdir().unwrap();
        let id = {
            let ledger = Ledger::open(tmp.path()).unwrap();
            let mut j = job("/persist");
            j.state = TransferState::Paused;
            ledger.upsert(&j).unwrap();
            j.id
        };
        // A fresh Ledger over the same root sees the record (durable across restart).
        let reopened = Ledger::open(tmp.path()).unwrap();
        let got = reopened.get(&id).unwrap();
        assert_eq!(got.state, TransferState::Paused);
        assert_eq!(got.source, "/persist");
    }

    #[test]
    fn v2_ledger_admits_controls_and_survives_reopen() {
        let tmp = tempfile::tempdir().unwrap();
        let job = v2_job(0x501, 100);
        let id = job.transfer;
        let ledger = V2Ledger::open(tmp.path()).unwrap();
        ledger.submit(job.clone()).unwrap();
        let mut replay = job.clone();
        replay.updated_unix_ms = 999;
        assert!(matches!(
            ledger.submit(replay),
            Err(V2LedgerError::Duplicate(duplicate)) if duplicate == id
        ));
        assert_eq!(
            ledger.get(id).unwrap(),
            job,
            "replay cannot replace the admitted row"
        );

        let paused = ledger
            .apply_control(id, TransferControlV2::Pause, 101)
            .unwrap();
        assert_eq!(paused.state, V2State::Paused);
        assert_eq!(paused.progress.phase, TransferPhase::Paused);
        assert!(matches!(
            ledger.apply_control(id, TransferControlV2::Resume, 101),
            Err(V2LedgerError::StaleUpdate { .. })
        ));

        let resumed = V2Ledger::open(tmp.path())
            .unwrap()
            .apply_control(id, TransferControlV2::Resume, 102)
            .unwrap();
        assert_eq!(resumed.state, V2State::Queued);
        assert_eq!(resumed.progress.phase, TransferPhase::Queued);
    }

    #[test]
    fn v2_ledger_orders_records_and_skips_hostile_files() {
        let tmp = tempfile::tempdir().unwrap();
        let ledger = V2Ledger::open(tmp.path()).unwrap();
        let later = v2_job(0x601, 200);
        let earlier = v2_job(0x701, 100);
        ledger.submit(later.clone()).unwrap();
        ledger.submit(earlier.clone()).unwrap();
        std::fs::write(ledger.dir().join("broken.json"), "{not-json").unwrap();
        std::fs::write(
            ledger.dir().join("oversized.json"),
            vec![b'x'; MAX_LEDGER_RECORD_BYTES + 1],
        )
        .unwrap();

        assert_eq!(ledger.load_all(), vec![earlier, later]);
    }
}
