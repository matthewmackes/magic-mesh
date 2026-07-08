//! TRANSFERS-1 — the typed verb set + its CLI↔daemon transport (Q14, §9).
//!
//! The design's verb set is `transfer.submit(job) / .cancel(id) / .pause(id) /
//! .resume(id) / .list` — a small typed contract both the CLI (`mackesd transfer …`)
//! and the future GUI drive. [`TransferVerb`] is that contract as one serde enum.
//!
//! The MUTATING verbs (submit/cancel/pause/resume) are handed to the running daemon
//! through a node-local **inbox**: each verb is one `<seq>.json` file under
//! `<store_root>/inbox/`, drained + applied by the worker every tick. Going through
//! the inbox keeps the **daemon the single writer** of job state (§9 one-state), so
//! the CLI never races the worker's in-flight lane tasks. `list` is a pure query —
//! it is served by reading the ledger directly (no daemon round-trip needed), so it
//! is not carried on the inbox; it is in the enum for contract completeness (the GUI
//! Bus path).

#![cfg(feature = "async-services")]

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::job::TransferJob;
use super::sync_pair::SyncPair;

/// The typed verb set (Q14). `Submit` carries the whole client-minted job; the
/// lifecycle verbs carry a job id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "verb", content = "arg")]
pub enum TransferVerb {
    /// `transfer.submit(job)` — enqueue a new job.
    Submit(TransferJob),
    /// `transfer.cancel(id)` — remove a job (frees any slot it held).
    Cancel(String),
    /// `transfer.pause(id)` — hold a Queued/Running job.
    Pause(String),
    /// `transfer.resume(id)` — re-arm a Paused job.
    Resume(String),
    /// Save or update a recurring rsync sync pair.
    SaveSyncPair(SyncPair),
    /// Remove a recurring sync pair by id.
    RemoveSyncPair(String),
    /// `transfer.list` — a pure read (served directly off the ledger, not inboxed).
    List,
}

impl TransferVerb {
    /// The verb token (logs + the CLI surface).
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Submit(_) => "submit",
            Self::Cancel(_) => "cancel",
            Self::Pause(_) => "pause",
            Self::Resume(_) => "resume",
            Self::SaveSyncPair(_) => "save-sync-pair",
            Self::RemoveSyncPair(_) => "remove-sync-pair",
            Self::List => "list",
        }
    }
}

/// The inbox directory the CLI writes verbs into and the worker drains.
#[must_use]
pub fn inbox_dir(store_root: &Path) -> PathBuf {
    store_root.join("inbox")
}

/// Enqueue a mutating verb for the daemon (atomic temp + rename). `List` is a read;
/// writing it is accepted but the worker treats it as a no-op.
///
/// # Errors
/// Serialization or IO failures.
pub fn write_verb(store_root: &Path, verb: &TransferVerb) -> io::Result<()> {
    let dir = inbox_dir(store_root);
    std::fs::create_dir_all(&dir)?;
    // The seq LEADS the filename (zero-padded, fixed width) so a filename sort in
    // `take_verbs` drains in submission order — leading with the verb name would sort
    // `pause` before `submit` and reorder the queue.
    let stem = format!("{:020}-{}", next_seq(), verb.name());
    let body =
        serde_json::to_string(verb).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let tmp = dir.join(format!(".{stem}.json.tmp"));
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, dir.join(format!("{stem}.json")))
}

/// Drain every pending verb from the inbox (removing each file).
///
/// Returns them in filename order (submit before a later cancel of the same job,
/// etc.). Unparseable / partially-written files are removed + skipped rather than
/// wedging the drain.
#[must_use]
pub fn take_verbs(store_root: &Path) -> Vec<TransferVerb> {
    let dir = inbox_dir(store_root);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension().and_then(|e| e.to_str()) == Some("json")
                && !p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with('.'))
        })
        .collect();
    paths.sort();
    let mut out = Vec::with_capacity(paths.len());
    for path in paths {
        let parsed = std::fs::read_to_string(&path)
            .ok()
            .and_then(|d| serde_json::from_str::<TransferVerb>(&d).ok());
        // Consume the file either way (a corrupt inbox entry must not replay forever).
        let _ = std::fs::remove_file(&path);
        if let Some(verb) = parsed {
            out.push(verb);
        }
    }
    out
}

/// Monotonic per-process sequence so two verbs minted in the same millisecond keep a
/// stable filename order; the leading `<verb-name>` + the ms floor keep cross-process
/// ordering good enough for the drain (submit lands before its later cancel).
fn next_seq() -> u64 {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0);
    // Blend a millisecond floor with a process-local counter (low bits) so the
    // filename is time-sortable AND unique within a burst.
    (ms << 16) | (SEQ.fetch_add(1, Ordering::Relaxed) & 0xFFFF)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workers::transfers::job::{Method, TransferPolicy};

    fn job() -> TransferJob {
        TransferJob::new("/a", "/b", Method::Http, TransferPolicy::default())
    }

    #[test]
    fn verb_json_is_tagged_and_round_trips() {
        let v = TransferVerb::Submit(job());
        let json = serde_json::to_string(&v).unwrap();
        assert!(json.contains("\"verb\":\"submit\""), "tagged shape: {json}");
        assert_eq!(serde_json::from_str::<TransferVerb>(&json).unwrap(), v);

        let c = TransferVerb::Cancel("id-1".into());
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains("\"verb\":\"cancel\"") && json.contains("\"arg\":\"id-1\""));
        assert_eq!(serde_json::from_str::<TransferVerb>(&json).unwrap(), c);

        let json = serde_json::to_string(&TransferVerb::List).unwrap();
        assert_eq!(
            serde_json::from_str::<TransferVerb>(&json).unwrap(),
            TransferVerb::List
        );

        let pair = SyncPair::new("docs", "/src/", "/dst/", 30, TransferPolicy::default());
        let json = serde_json::to_string(&TransferVerb::SaveSyncPair(pair.clone())).unwrap();
        assert!(json.contains("\"verb\":\"save_sync_pair\""));
        assert_eq!(
            serde_json::from_str::<TransferVerb>(&json).unwrap(),
            TransferVerb::SaveSyncPair(pair)
        );
    }

    #[test]
    fn inbox_write_then_take_drains_in_order_and_clears() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let j = job();
        write_verb(root, &TransferVerb::Submit(j.clone())).unwrap();
        write_verb(root, &TransferVerb::Pause(j.id.clone())).unwrap();
        let drained = take_verbs(root);
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0], TransferVerb::Submit(j.clone()));
        assert_eq!(drained[1], TransferVerb::Pause(j.id));
        // The inbox is now empty (each verb consumed exactly once).
        assert!(take_verbs(root).is_empty());
    }

    #[test]
    fn a_corrupt_inbox_entry_is_dropped_not_replayed() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = inbox_dir(tmp.path());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("cancel-00000001.json"), "{ not json").unwrap();
        assert!(take_verbs(tmp.path()).is_empty(), "corrupt entry skipped");
        // ...and consumed, so it never replays.
        assert!(take_verbs(tmp.path()).is_empty());
    }
}
