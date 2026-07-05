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

/// The on-disk ledger — one directory of `<id>.json` records.
#[derive(Debug, Clone)]
pub struct Ledger {
    dir: PathBuf,
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
        let data = std::fs::read_to_string(self.path(id)).ok()?;
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
            if let Ok(data) = std::fs::read_to_string(&path) {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workers::transfers::job::{Method, TransferPolicy, TransferState};

    fn job(source: &str) -> TransferJob {
        TransferJob::new(source, "/dest", Method::Rsync, TransferPolicy::default())
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
}
