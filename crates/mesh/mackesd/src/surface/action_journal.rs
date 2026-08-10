//! Root-owned durable arbitration for Surface actions and pending-only cancellation.
//!
//! One record is keyed by the original action identity.  The first durable
//! claimant wins: either the action, or a cancellation that carries the full
//! original action binding.  Terminal results remain in the journal until the
//! exact result hash has been marked published, allowing crash-safe Bus retry
//! without repeating a hardware effect.

use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use fs2::FileExt;
use mackes_mesh_types::surface_hardware::SurfaceFirmwareApplyTarget;
use rustix::fs::{AtFlags, Dir, Mode, OFlags};
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

pub const DEFAULT_SURFACE_ACTION_JOURNAL_ROOT: &str = "/var/lib/mackesd/surface-action-journal";
const SCHEMA_VERSION: u16 = 1;
const MAX_RECORDS: usize = 128;
const MAX_RECORD_BYTES: u64 = 128 * 1024;
const MAX_RESULT_BYTES: usize = 64 * 1024;
const MAX_FIELD_BYTES: usize = 256;
const LOCK_NAME: &str = ".lock";
/// Replay arbitration survives capability expiry for one hour. This protects
/// delayed retained Bus rows and bounds disk retention without reopening an
/// authorization window immediately after its token expires.
pub const SURFACE_ACTION_JOURNAL_RETENTION_MS: u64 = 60 * 60 * 1_000;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalAction {
    Enable,
    FirmwareApply,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalKey {
    pub node: String,
    pub action: JournalAction,
    pub target_request_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionClaim {
    pub key: JournalKey,
    pub source_ulid: String,
    pub request_id: String,
    pub exact_body_sha256: String,
    pub model_product: String,
    pub model_generation: String,
    pub firmware_target: Option<SurfaceFirmwareApplyTarget>,
    pub claimed_at_ms: u64,
    pub expires_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelIntent {
    pub source_ulid: String,
    pub cancellation_id: String,
    pub exact_body_sha256: String,
    /// The complete original binding prevents cancellation from manufacturing
    /// a weaker or foreign target when it wins before the action is observed.
    pub target: ActionClaim,
    pub claimed_at_ms: u64,
    pub expires_at_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalOutcome {
    ActionCompleted,
    Cancelled,
    Refused,
    Interrupted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalWinner {
    Action,
    Cancellation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalDecision {
    pub outcome: JournalOutcome,
    pub decided_at_ms: u64,
    pub result_body: String,
    pub result_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum JournalPhase {
    ActionClaimed {
        action: ActionClaim,
    },
    /// The action won first, but the exact too-late cancellation is retained so
    /// its closed result can survive publication failure and restart.
    ActionClaimedCancel {
        action: ActionClaim,
        cancel: CancelIntent,
        late_cancel_decision: Option<JournalDecision>,
        late_cancel_published: bool,
    },
    CancelClaimed {
        action: ActionClaim,
        cancel: CancelIntent,
    },
    Closed {
        action: ActionClaim,
        cancel: Option<CancelIntent>,
        winner: JournalWinner,
        decision: JournalDecision,
        published: bool,
        late_cancel_decision: Option<JournalDecision>,
        late_cancel_published: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalRecord {
    pub schema_version: u16,
    pub key: JournalKey,
    pub phase: JournalPhase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimDisposition {
    Claimed,
    AlreadyClaimed,
    CancellationWon,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelDisposition {
    CancelledPending,
    AlreadyCancelled,
    ActionAlreadyClaimed,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionDisposition {
    Recorded,
    AlreadyRecorded,
}

#[derive(Debug)]
pub struct SurfaceActionJournal {
    root: PathBuf,
    directory: File,
    trusted_uid: u32,
}

impl SurfaceActionJournal {
    pub fn open_default() -> Result<Self, String> {
        if !rustix::process::geteuid().is_root() {
            return Err("Surface action journal requires the root service process".into());
        }
        Self::open_at(PathBuf::from(DEFAULT_SURFACE_ACTION_JOURNAL_ROOT), 0)
    }

    pub fn open_at(root: PathBuf, trusted_uid: u32) -> Result<Self, String> {
        ensure_root(&root, trusted_uid)?;
        let directory: File = rustix::fs::open(
            &root,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| format!("open Surface action journal directory: {error}"))?
        .into();
        validate_directory(&directory, trusted_uid)?;
        let journal = Self {
            root,
            directory,
            trusted_uid,
        };
        journal.with_lock(|_| Ok(()))?;
        Ok(journal)
    }

    pub fn claim_action(&self, claim: &ActionClaim) -> Result<ClaimDisposition, String> {
        validate_action_claim(claim)?;
        let key = claim.key.clone();
        self.with_lock(|this| match this.read_locked(&key)? {
            None => {
                this.write_locked(
                    &JournalRecord {
                        schema_version: SCHEMA_VERSION,
                        key,
                        phase: JournalPhase::ActionClaimed {
                            action: claim.clone(),
                        },
                    },
                    true,
                )?;
                Ok(ClaimDisposition::Claimed)
            }
            Some(record) => match record.phase {
                JournalPhase::ActionClaimed { action } if action == *claim => {
                    Ok(ClaimDisposition::AlreadyClaimed)
                }
                JournalPhase::ActionClaimedCancel { action, .. } if action == *claim => {
                    Ok(ClaimDisposition::AlreadyClaimed)
                }
                JournalPhase::CancelClaimed { action, .. } if action == *claim => {
                    Ok(ClaimDisposition::CancellationWon)
                }
                JournalPhase::Closed { action, .. } if action == *claim => {
                    Ok(ClaimDisposition::Closed)
                }
                _ => Err("Surface action journal contains a conflicting action binding".into()),
            },
        })
    }

    pub fn record_cancel_intent(
        &self,
        key: &JournalKey,
        cancel: &CancelIntent,
    ) -> Result<CancelDisposition, String> {
        validate_key(key)?;
        validate_cancel(cancel)?;
        if &cancel.target.key != key {
            return Err("cancellation target does not match its journal key".into());
        }
        self.with_lock(|this| match this.read_locked(key)? {
            None => {
                this.write_locked(
                    &JournalRecord {
                        schema_version: SCHEMA_VERSION,
                        key: key.clone(),
                        phase: JournalPhase::CancelClaimed {
                            action: cancel.target.clone(),
                            cancel: cancel.clone(),
                        },
                    },
                    true,
                )?;
                Ok(CancelDisposition::CancelledPending)
            }
            Some(record) => match record.phase {
                JournalPhase::ActionClaimed { action } if action == cancel.target => {
                    this.write_locked(
                        &JournalRecord {
                            schema_version: SCHEMA_VERSION,
                            key: key.clone(),
                            phase: JournalPhase::ActionClaimedCancel {
                                action,
                                cancel: cancel.clone(),
                                late_cancel_decision: None,
                                late_cancel_published: false,
                            },
                        },
                        false,
                    )?;
                    Ok(CancelDisposition::ActionAlreadyClaimed)
                }
                JournalPhase::ActionClaimedCancel {
                    action,
                    cancel: prior,
                    ..
                } if action == cancel.target && prior == *cancel => {
                    Ok(CancelDisposition::ActionAlreadyClaimed)
                }
                JournalPhase::CancelClaimed {
                    action,
                    cancel: prior,
                } if action == cancel.target && prior == *cancel => {
                    Ok(CancelDisposition::AlreadyCancelled)
                }
                JournalPhase::Closed {
                    action,
                    cancel: prior,
                    winner: JournalWinner::Cancellation,
                    ..
                } if action == cancel.target && prior.as_ref() == Some(cancel) => {
                    Ok(CancelDisposition::Closed)
                }
                JournalPhase::Closed {
                    action,
                    cancel: prior,
                    winner: JournalWinner::Action,
                    decision,
                    published,
                    late_cancel_decision,
                    late_cancel_published,
                } if action == cancel.target
                    && prior.as_ref().is_none_or(|prior| prior == cancel) =>
                {
                    this.write_locked(
                        &JournalRecord {
                            schema_version: SCHEMA_VERSION,
                            key: key.clone(),
                            phase: JournalPhase::Closed {
                                action,
                                cancel: Some(cancel.clone()),
                                winner: JournalWinner::Action,
                                decision,
                                published,
                                late_cancel_decision,
                                late_cancel_published,
                            },
                        },
                        false,
                    )?;
                    Ok(CancelDisposition::ActionAlreadyClaimed)
                }
                _ => {
                    Err("Surface action journal contains a conflicting cancellation binding".into())
                }
            },
        })
    }

    pub fn record_decision(
        &self,
        key: &JournalKey,
        decision: &JournalDecision,
    ) -> Result<DecisionDisposition, String> {
        validate_key(key)?;
        validate_decision(decision)?;
        self.with_lock(|this| {
            let record = this
                .read_locked(key)?
                .ok_or_else(|| "cannot close an unclaimed Surface action".to_string())?;
            let phase = match record.phase {
                JournalPhase::ActionClaimed { action } => JournalPhase::Closed {
                    action,
                    cancel: None,
                    winner: JournalWinner::Action,
                    decision: decision.clone(),
                    published: false,
                    late_cancel_decision: None,
                    late_cancel_published: false,
                },
                JournalPhase::ActionClaimedCancel {
                    action,
                    cancel,
                    late_cancel_decision,
                    late_cancel_published,
                } => JournalPhase::Closed {
                    action,
                    cancel: Some(cancel),
                    winner: JournalWinner::Action,
                    decision: decision.clone(),
                    published: false,
                    late_cancel_decision,
                    late_cancel_published,
                },
                JournalPhase::CancelClaimed { action, cancel } => JournalPhase::Closed {
                    action,
                    cancel: Some(cancel),
                    winner: JournalWinner::Cancellation,
                    decision: decision.clone(),
                    published: false,
                    late_cancel_decision: None,
                    late_cancel_published: false,
                },
                JournalPhase::Closed {
                    decision: prior, ..
                } if prior == *decision => return Ok(DecisionDisposition::AlreadyRecorded),
                JournalPhase::Closed { .. } => {
                    return Err("Surface action already has a different terminal decision".into())
                }
            };
            this.write_locked(
                &JournalRecord {
                    schema_version: SCHEMA_VERSION,
                    key: key.clone(),
                    phase,
                },
                false,
            )?;
            Ok(DecisionDisposition::Recorded)
        })
    }

    pub fn unpublished(&self) -> Result<Vec<JournalRecord>, String> {
        self.with_lock(|this| {
            Ok(this
                .scan_locked()?
                .into_iter()
                .filter(|record| {
                    matches!(
                        record.phase,
                        JournalPhase::Closed {
                            published: false,
                            ..
                        }
                    )
                })
                .collect())
        })
    }

    /// Persist the exact too-late cancellation result without replacing the
    /// action's independent terminal decision.
    pub fn record_late_cancel_decision(
        &self,
        key: &JournalKey,
        decision: &JournalDecision,
    ) -> Result<DecisionDisposition, String> {
        validate_key(key)?;
        validate_decision(decision)?;
        if decision.outcome != JournalOutcome::Refused {
            return Err("late Surface cancellation must be a refused result".into());
        }
        self.with_lock(|this| {
            let mut record = this.read_locked(key)?.ok_or_else(|| {
                "cannot attach a late cancellation to an absent action".to_string()
            })?;
            let (slot, published) = match &mut record.phase {
                JournalPhase::ActionClaimedCancel {
                    late_cancel_decision,
                    late_cancel_published,
                    ..
                }
                | JournalPhase::Closed {
                    winner: JournalWinner::Action,
                    late_cancel_decision,
                    late_cancel_published,
                    ..
                } => (late_cancel_decision, late_cancel_published),
                _ => return Err("Surface action has no action-won late cancellation".into()),
            };
            if slot.as_ref() == Some(decision) {
                return Ok(DecisionDisposition::AlreadyRecorded);
            }
            if slot.is_some() {
                return Err(
                    "Surface action already has a different late cancellation result".into(),
                );
            }
            *slot = Some(decision.clone());
            *published = false;
            this.write_locked(&record, false)?;
            Ok(DecisionDisposition::Recorded)
        })
    }

    /// Return exact late-cancellation results that still need Bus publication.
    pub fn unpublished_late_cancellations(&self) -> Result<Vec<JournalRecord>, String> {
        self.with_lock(|this| {
            Ok(this
                .scan_locked()?
                .into_iter()
                .filter(|record| match &record.phase {
                    JournalPhase::ActionClaimedCancel {
                        late_cancel_decision: Some(_),
                        late_cancel_published: false,
                        ..
                    }
                    | JournalPhase::Closed {
                        winner: JournalWinner::Action,
                        late_cancel_decision: Some(_),
                        late_cancel_published: false,
                        ..
                    } => true,
                    _ => false,
                })
                .collect())
        })
    }

    /// Return every durable nonterminal row that a restarted worker must close.
    /// This is independent of retained Bus requests: the row contains the full
    /// authenticated action and, when present, cancellation binding.
    pub fn pending_recovery(&self) -> Result<Vec<JournalRecord>, String> {
        self.with_lock(|this| {
            Ok(this
                .scan_locked()?
                .into_iter()
                .filter(|record| !matches!(record.phase, JournalPhase::Closed { .. }))
                .collect())
        })
    }

    pub fn mark_published(&self, key: &JournalKey, result_sha256: &str) -> Result<(), String> {
        validate_key(key)?;
        validate_sha256(result_sha256)?;
        self.with_lock(|this| {
            let mut record = this
                .read_locked(key)?
                .ok_or_else(|| "Surface action journal record is absent".to_string())?;
            match &mut record.phase {
                JournalPhase::Closed {
                    decision,
                    published,
                    ..
                } if decision.result_sha256 == result_sha256 => {
                    if !*published {
                        *published = true;
                        this.write_locked(&record, false)?;
                    }
                    Ok(())
                }
                JournalPhase::Closed { .. } => {
                    Err("published result hash does not match the terminal decision".into())
                }
                _ => Err("cannot publish a non-terminal Surface action".into()),
            }
        })
    }

    /// Mark the exact late-cancellation outbox row published.
    pub fn mark_late_cancel_published(
        &self,
        key: &JournalKey,
        result_sha256: &str,
    ) -> Result<(), String> {
        validate_key(key)?;
        validate_sha256(result_sha256)?;
        self.with_lock(|this| {
            let mut record = this
                .read_locked(key)?
                .ok_or_else(|| "Surface action journal record is absent".to_string())?;
            let (decision, published) = match &mut record.phase {
                JournalPhase::ActionClaimedCancel {
                    late_cancel_decision: Some(decision),
                    late_cancel_published,
                    ..
                }
                | JournalPhase::Closed {
                    winner: JournalWinner::Action,
                    late_cancel_decision: Some(decision),
                    late_cancel_published,
                    ..
                } => (decision, late_cancel_published),
                _ => return Err("Surface action has no late cancellation result".into()),
            };
            if decision.result_sha256 != result_sha256 {
                return Err("late cancellation hash does not match its durable result".into());
            }
            if !*published {
                *published = true;
                this.write_locked(&record, false)?;
            }
            Ok(())
        })
    }

    pub fn gc_expired(&self, now_ms: u64) -> Result<usize, String> {
        self.with_lock(|this| {
            let records = this.scan_locked()?;
            let mut removed = 0;
            for record in records {
                let retain_until = record.retain_until_ms();
                // Recovery, rather than GC, closes every orphaned claim. Only
                // an exact terminal result already published to Bus is safe to
                // forget.
                let collectable = matches!(
                    record.phase,
                    JournalPhase::Closed {
                        published: true,
                        late_cancel_decision: None,
                        ..
                    } | JournalPhase::Closed {
                        published: true,
                        late_cancel_decision: Some(_),
                        late_cancel_published: true,
                        ..
                    }
                );
                if now_ms > retain_until && collectable {
                    rustix::fs::unlinkat(
                        &this.directory,
                        this.record_name(&record.key).as_str(),
                        AtFlags::empty(),
                    )
                    .map_err(|error| format!("remove expired Surface journal record: {error}"))?;
                    removed += 1;
                }
            }
            if removed > 0 {
                sync_file(&this.directory)?;
            }
            Ok(removed)
        })
    }

    pub fn get(&self, key: &JournalKey) -> Result<Option<JournalRecord>, String> {
        validate_key(key)?;
        self.with_lock(|this| this.read_locked(key))
    }

    fn record_name(&self, key: &JournalKey) -> String {
        format!("{}.json", key_digest(key))
    }

    #[cfg(test)]
    fn record_path(&self, key: &JournalKey) -> PathBuf {
        self.root.join(self.record_name(key))
    }

    fn with_lock<T>(
        &self,
        operation: impl FnOnce(&Self) -> Result<T, String>,
    ) -> Result<T, String> {
        validate_directory(&self.directory, self.trusted_uid)?;
        let lock = open_lock_at(&self.directory)
            .map_err(|error| format!("open Surface action journal lock: {error}"))?;
        validate_file(&lock, self.trusted_uid, 0o600, 0)?;
        if lock.metadata().map_err(|error| error.to_string())?.len() != 0 {
            return Err("Surface action journal lock file is not empty".into());
        }
        sync_file(&self.directory)?;
        lock.lock_exclusive()
            .map_err(|error| format!("lock Surface action journal: {error}"))?;
        validate_directory(&self.directory, self.trusted_uid)?;
        cleanup_temps(&self.directory, self.trusted_uid)?;
        let result = operation(self);
        let _ = FileExt::unlock(&lock);
        result
    }

    fn read_locked(&self, key: &JournalKey) -> Result<Option<JournalRecord>, String> {
        let name = self.record_name(key);
        let file = match openat_nofollow(&self.directory, &name, false, 0) {
            Ok(file) => file,
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(error) => return Err(format!("open Surface journal record: {error}")),
        };
        validate_file(&file, self.trusted_uid, 0o600, MAX_RECORD_BYTES)?;
        let mut body = Vec::new();
        file.take(MAX_RECORD_BYTES + 1)
            .read_to_end(&mut body)
            .map_err(|error| error.to_string())?;
        if body.len() as u64 > MAX_RECORD_BYTES {
            return Err("Surface journal record is oversized".into());
        }
        reject_duplicate_json_keys(&body)?;
        let record: JournalRecord = serde_json::from_slice(&body)
            .map_err(|error| format!("decode Surface journal record: {error}"))?;
        validate_record(&record)?;
        if record.key != *key || name != self.record_name(&record.key) {
            return Err("Surface journal filename or identity mismatch".into());
        }
        Ok(Some(record))
    }

    fn scan_locked(&self) -> Result<Vec<JournalRecord>, String> {
        let mut keys = Vec::new();
        let mut aggregate_bytes = 0_u64;
        for entry in Dir::read_from(&self.directory).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let name = entry
                .file_name()
                .to_str()
                .map_err(|_| "Surface journal contains a non-UTF8 entry")?
                .to_owned();
            if name == "." || name == ".." {
                continue;
            }
            if name == LOCK_NAME || name.starts_with('.') {
                continue;
            }
            if name.len() != 69
                || !name.ends_with(".json")
                || !name[..64]
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            {
                return Err("Surface journal contains an unknown entry".into());
            }
            if keys.len() == MAX_RECORDS {
                return Err("Surface action journal exceeds its bounded capacity".into());
            }
            let metadata =
                rustix::fs::statat(&self.directory, name.as_str(), AtFlags::SYMLINK_NOFOLLOW)
                    .map_err(|error| error.to_string())?;
            aggregate_bytes = aggregate_bytes
                .checked_add(metadata.st_size as u64)
                .ok_or_else(|| "Surface action journal aggregate size overflow".to_string())?;
            if aggregate_bytes > MAX_RECORD_BYTES * MAX_RECORDS as u64 {
                return Err("Surface action journal exceeds its aggregate byte bound".into());
            }
            keys.push(name);
        }
        keys.sort();
        let mut records = Vec::with_capacity(keys.len());
        for name in keys {
            let file = openat_nofollow(&self.directory, &name, false, 0)
                .map_err(|error| error.to_string())?;
            validate_file(&file, self.trusted_uid, 0o600, MAX_RECORD_BYTES)?;
            let mut body = Vec::new();
            file.take(MAX_RECORD_BYTES + 1)
                .read_to_end(&mut body)
                .map_err(|error| error.to_string())?;
            if body.len() as u64 > MAX_RECORD_BYTES {
                return Err("Surface journal record is oversized".into());
            }
            reject_duplicate_json_keys(&body)?;
            let record: JournalRecord =
                serde_json::from_slice(&body).map_err(|error| error.to_string())?;
            validate_record(&record)?;
            if name != self.record_name(&record.key) {
                return Err("Surface journal row filename mismatch".into());
            }
            records.push(record);
        }
        Ok(records)
    }

    fn write_locked(&self, record: &JournalRecord, no_clobber: bool) -> Result<(), String> {
        validate_record(record)?;
        let existing = self.scan_locked()?.len();
        let destination = self.record_name(&record.key);
        if no_clobber && existing >= MAX_RECORDS {
            return Err("Surface action journal is full".into());
        }
        let body = serde_json::to_vec(record).map_err(|error| error.to_string())?;
        if body.len() as u64 > MAX_RECORD_BYTES {
            return Err("Surface journal record is oversized".into());
        }
        let temporary = format!(
            ".{}.{}.tmp",
            key_digest(&record.key),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        );
        let mut file = openat_nofollow(&self.directory, &temporary, true, 0o600)
            .map_err(|error| format!("create Surface journal temporary: {error}"))?;
        let result = (|| {
            file.write_all(&body).map_err(|error| error.to_string())?;
            file.sync_all().map_err(|error| error.to_string())?;
            drop(file);
            if no_clobber {
                rustix::fs::linkat(
                    &self.directory,
                    temporary.as_str(),
                    &self.directory,
                    destination.as_str(),
                    AtFlags::empty(),
                )
                .map_err(|error| format!("claim Surface journal record: {error}"))?;
                rustix::fs::unlinkat(&self.directory, temporary.as_str(), AtFlags::empty())
                    .map_err(|error| error.to_string())?;
            } else {
                rustix::fs::renameat(
                    &self.directory,
                    temporary.as_str(),
                    &self.directory,
                    destination.as_str(),
                )
                .map_err(|error| error.to_string())?;
            }
            sync_file(&self.directory)
        })();
        if result.is_err() {
            let _ = rustix::fs::unlinkat(&self.directory, temporary.as_str(), AtFlags::empty());
        }
        result
    }
}

impl JournalRecord {
    fn action(&self) -> &ActionClaim {
        match &self.phase {
            JournalPhase::ActionClaimed { action }
            | JournalPhase::ActionClaimedCancel { action, .. }
            | JournalPhase::CancelClaimed { action, .. }
            | JournalPhase::Closed { action, .. } => action,
        }
    }

    fn retain_until_ms(&self) -> u64 {
        let action_expiry = self.action().expires_at_ms;
        let cancel_expiry = match &self.phase {
            JournalPhase::ActionClaimedCancel { cancel, .. }
            | JournalPhase::CancelClaimed { cancel, .. }
            | JournalPhase::Closed {
                cancel: Some(cancel),
                ..
            } => cancel.expires_at_ms,
            _ => 0,
        };
        action_expiry
            .max(cancel_expiry)
            .saturating_add(SURFACE_ACTION_JOURNAL_RETENTION_MS)
    }
}

fn validate_key(key: &JournalKey) -> Result<(), String> {
    validate_field(&key.node)?;
    validate_field(&key.target_request_id)
}
fn validate_action_claim(claim: &ActionClaim) -> Result<(), String> {
    validate_key(&claim.key)?;
    validate_ulid(&claim.source_ulid)?;
    validate_field(&claim.request_id)?;
    validate_sha256(&claim.exact_body_sha256)?;
    match (
        claim.model_product.as_str(),
        claim.model_generation.as_str(),
    ) {
        ("Surface Pro 5", "pro5") | ("Surface Pro 6", "pro6") => {}
        _ => return Err("Surface journal model/generation is not exact Pro 5/6".into()),
    }
    if let Some(target) = &claim.firmware_target {
        validate_field(&target.device_id)?;
        validate_field(&target.release_version)?;
        validate_sha256(&target.release_checksum)?;
        if target.inventory_published_at_ms == 0 {
            return Err("Surface firmware journal inventory timestamp is invalid".into());
        }
    }
    if claim.request_id != claim.key.target_request_id || claim.expires_at_ms <= claim.claimed_at_ms
    {
        return Err("Surface action claim identity or lifetime is invalid".into());
    }
    match claim.key.action {
        JournalAction::Enable if claim.firmware_target.is_some() => {
            Err("enable claim cannot carry a firmware target".into())
        }
        JournalAction::FirmwareApply if claim.firmware_target.is_none() => {
            Err("firmware claim requires its exact target".into())
        }
        _ => Ok(()),
    }
}
fn validate_cancel(cancel: &CancelIntent) -> Result<(), String> {
    validate_ulid(&cancel.source_ulid)?;
    validate_field(&cancel.cancellation_id)?;
    validate_sha256(&cancel.exact_body_sha256)?;
    validate_action_claim(&cancel.target)?;
    if cancel.claimed_at_ms < cancel.target.claimed_at_ms
        || cancel.claimed_at_ms > cancel.target.expires_at_ms
        || cancel.expires_at_ms <= cancel.claimed_at_ms
    {
        return Err("Surface cancellation timestamps are outside the target lifetime".into());
    }
    Ok(())
}
fn validate_decision(decision: &JournalDecision) -> Result<(), String> {
    if decision.decided_at_ms == 0 {
        return Err("Surface terminal decision timestamp is invalid".into());
    }
    if decision.result_body.is_empty() || decision.result_body.len() > MAX_RESULT_BYTES {
        return Err("Surface terminal result is empty or oversized".into());
    }
    validate_sha256(&decision.result_sha256)?;
    if sha256(decision.result_body.as_bytes()) != decision.result_sha256 {
        return Err("Surface terminal result hash mismatch".into());
    }
    Ok(())
}
fn validate_record(record: &JournalRecord) -> Result<(), String> {
    if record.schema_version != SCHEMA_VERSION {
        return Err("unknown Surface action journal schema".into());
    }
    validate_key(&record.key)?;
    let action = record.action();
    validate_action_claim(action)?;
    if action.key != record.key {
        return Err("Surface journal action/key mismatch".into());
    }
    match &record.phase {
        JournalPhase::ActionClaimedCancel {
            action,
            cancel,
            late_cancel_decision,
            late_cancel_published,
        } => {
            validate_cancel(cancel)?;
            if cancel.target != *action {
                return Err("Surface cancellation target does not equal the phase action".into());
            }
            validate_late_cancel_state(late_cancel_decision.as_ref(), *late_cancel_published)?;
        }
        JournalPhase::CancelClaimed { action, cancel } => {
            validate_cancel(cancel)?;
            if cancel.target != *action {
                return Err("Surface cancellation target does not equal the phase action".into());
            }
        }
        JournalPhase::Closed {
            cancel,
            winner,
            decision,
            late_cancel_decision,
            late_cancel_published,
            ..
        } => {
            if let Some(cancel) = cancel {
                validate_cancel(cancel)?;
                if cancel.target != *action {
                    return Err(
                        "Surface cancellation target does not equal the phase action".into(),
                    );
                }
            }
            if *winner == JournalWinner::Cancellation && cancel.is_none() {
                return Err("Surface cancellation winner lacks its exact intent".into());
            }
            validate_decision(decision)?;
            let earliest = cancel
                .as_ref()
                .map_or(action.claimed_at_ms, |intent| intent.claimed_at_ms);
            if decision.decided_at_ms < earliest {
                return Err("Surface terminal decision predates its durable claim".into());
            }
            if (*winner == JournalWinner::Cancellation)
                != (decision.outcome == JournalOutcome::Cancelled)
            {
                return Err("Surface terminal outcome contradicts the durable winner".into());
            }
            if *winner == JournalWinner::Cancellation && late_cancel_decision.is_some() {
                return Err("cancellation winner cannot carry a late cancellation result".into());
            }
            validate_late_cancel_state(late_cancel_decision.as_ref(), *late_cancel_published)?;
        }
        JournalPhase::ActionClaimed { .. } => {}
    }
    Ok(())
}
fn validate_late_cancel_state(
    decision: Option<&JournalDecision>,
    published: bool,
) -> Result<(), String> {
    match decision {
        Some(decision) if decision.outcome == JournalOutcome::Refused => {
            validate_decision(decision)
        }
        Some(_) => Err("late Surface cancellation is not a refused result".into()),
        None if published => Err("late Surface cancellation publication lacks a result".into()),
        None => Ok(()),
    }
}
fn validate_field(value: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > MAX_FIELD_BYTES || value.chars().any(char::is_control) {
        Err("Surface journal field is invalid".into())
    } else {
        Ok(())
    }
}
fn validate_ulid(value: &str) -> Result<(), String> {
    if value.len() == 26
        && value
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    {
        Ok(())
    } else {
        Err("Surface journal source ULID is invalid".into())
    }
}
fn validate_sha256(value: &str) -> Result<(), String> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err("Surface journal SHA-256 is invalid".into())
    }
}
fn sha256(body: &[u8]) -> String {
    format!("{:x}", Sha256::digest(body))
}
fn key_digest(key: &JournalKey) -> String {
    sha256(
        serde_json::to_string(key)
            .expect("JournalKey serializes")
            .as_bytes(),
    )
}

fn ensure_root(root: &Path, trusted_uid: u32) -> Result<(), String> {
    if root.exists() {
        return validate_root(root, trusted_uid);
    }
    let parent = root
        .parent()
        .ok_or_else(|| "Surface journal root has no parent".to_string())?;
    let metadata = std::fs::symlink_metadata(parent).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.uid() != trusted_uid
        || metadata.permissions().mode() & 0o022 != 0
    {
        return Err("Surface journal parent is untrusted".into());
    }
    std::fs::DirBuilder::new()
        .recursive(false)
        .mode(0o700)
        .create(root)
        .map_err(|error| error.to_string())?;
    sync_dir(parent)?;
    validate_root(root, trusted_uid)
}
fn validate_root(root: &Path, trusted_uid: u32) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(root).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.uid() != trusted_uid
        || metadata.permissions().mode() & 0o777 != 0o700
    {
        Err("Surface journal root has unsafe type, owner, or mode".into())
    } else {
        Ok(())
    }
}
fn openat_nofollow(
    directory: &File,
    name: &str,
    create: bool,
    mode: u32,
) -> rustix::io::Result<File> {
    let flags = if create {
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC
    } else {
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC
    };
    rustix::fs::openat(directory, name, flags, Mode::from_raw_mode(mode)).map(Into::into)
}
fn open_lock_at(directory: &File) -> rustix::io::Result<File> {
    let open_existing = || {
        rustix::fs::openat(
            directory,
            LOCK_NAME,
            OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map(Into::into)
    };
    match open_existing() {
        Ok(file) => Ok(file),
        Err(rustix::io::Errno::NOENT) => {
            match rustix::fs::openat(
                directory,
                LOCK_NAME,
                OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            )
            .map(Into::into)
            {
                Ok(file) => Ok(file),
                Err(rustix::io::Errno::EXIST) => open_existing(),
                Err(error) => Err(error),
            }
        }
        Err(error) => Err(error),
    }
}
fn validate_file(file: &File, trusted_uid: u32, mode: u32, max: u64) -> Result<(), String> {
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file()
        || metadata.uid() != trusted_uid
        || metadata.permissions().mode() & 0o777 != mode
        || (max > 0 && metadata.len() > max)
    {
        Err("Surface journal file has unsafe type, owner, mode, or size".into())
    } else {
        Ok(())
    }
}
fn validate_directory(directory: &File, trusted_uid: u32) -> Result<(), String> {
    let metadata = directory.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_dir()
        || metadata.uid() != trusted_uid
        || metadata.permissions().mode() & 0o777 != 0o700
    {
        Err("Surface journal directory descriptor has unsafe type, owner, or mode".into())
    } else {
        Ok(())
    }
}
fn cleanup_temps(directory: &File, trusted_uid: u32) -> Result<(), String> {
    let mut removed = false;
    let mut entry_count = 0;
    let mut temp_count = 0;
    for entry in Dir::read_from(directory).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry
            .file_name()
            .to_str()
            .map_err(|_| "Surface journal contains non-UTF8 entry")?
            .to_owned();
        if name == "." || name == ".." {
            continue;
        }
        entry_count += 1;
        // A full journal has MAX_RECORDS rows plus its lock sentinel. Count
        // before classifying or skipping so hostile non-hidden entries cannot
        // turn crash-temp cleanup into an unbounded directory traversal.
        if entry_count > MAX_RECORDS + 1 {
            return Err("Surface journal cleanup exceeds its directory entry bound".into());
        }
        if name == LOCK_NAME || !name.starts_with('.') {
            continue;
        }
        if !valid_temp_name(&name) {
            return Err("Surface journal contains an unknown hidden entry".into());
        }
        temp_count += 1;
        if temp_count > MAX_RECORDS {
            return Err("Surface journal temporary cleanup exceeds its bound".into());
        }
        let file =
            openat_nofollow(directory, &name, false, 0).map_err(|error| error.to_string())?;
        validate_file(&file, trusted_uid, 0o600, MAX_RECORD_BYTES)?;
        drop(file);
        rustix::fs::unlinkat(directory, name.as_str(), AtFlags::empty())
            .map_err(|error| error.to_string())?;
        removed = true;
    }
    if removed {
        sync_file(directory)?;
    }
    Ok(())
}

fn valid_temp_name(name: &str) -> bool {
    let Some(inner) = name
        .strip_prefix('.')
        .and_then(|value| value.strip_suffix(".tmp"))
    else {
        return false;
    };
    let Some((digest, sequence)) = inner.rsplit_once('.') else {
        return false;
    };
    digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        && !sequence.is_empty()
        && sequence.len() <= 20
        && sequence.bytes().all(|byte| byte.is_ascii_digit())
}
fn sync_dir(path: &Path) -> Result<(), String> {
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|error| error.to_string())
}
fn sync_file(file: &File) -> Result<(), String> {
    file.sync_all().map_err(|error| error.to_string())
}

struct NoDuplicateKeys;
impl<'de> DeserializeSeed<'de> for NoDuplicateKeys {
    type Value = ();
    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_any(NoDuplicateVisitor)
    }
}
struct NoDuplicateVisitor;
impl<'de> Visitor<'de> for NoDuplicateVisitor {
    type Value = ();
    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("bounded JSON without duplicate object keys")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        let mut keys = HashSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !keys.insert(key) {
                return Err(de::Error::custom("duplicate JSON object key"));
            }
            map.next_value_seed(NoDuplicateKeys)?;
        }
        Ok(())
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        while seq.next_element_seed(NoDuplicateKeys)?.is_some() {}
        Ok(())
    }
    fn visit_bool<E: de::Error>(self, _: bool) -> Result<(), E> {
        Ok(())
    }
    fn visit_i64<E: de::Error>(self, _: i64) -> Result<(), E> {
        Ok(())
    }
    fn visit_u64<E: de::Error>(self, _: u64) -> Result<(), E> {
        Ok(())
    }
    fn visit_f64<E: de::Error>(self, _: f64) -> Result<(), E> {
        Ok(())
    }
    fn visit_str<E: de::Error>(self, _: &str) -> Result<(), E> {
        Ok(())
    }
    fn visit_string<E: de::Error>(self, _: String) -> Result<(), E> {
        Ok(())
    }
    fn visit_none<E: de::Error>(self) -> Result<(), E> {
        Ok(())
    }
    fn visit_some<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        NoDuplicateKeys.deserialize(deserializer)
    }
    fn visit_unit<E: de::Error>(self) -> Result<(), E> {
        Ok(())
    }
}
fn reject_duplicate_json_keys(body: &[u8]) -> Result<(), String> {
    let mut deserializer = serde_json::Deserializer::from_slice(body);
    NoDuplicateKeys
        .deserialize(&mut deserializer)
        .map_err(|error| error.to_string())?;
    deserializer.end().map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim(action: JournalAction) -> ActionClaim {
        ActionClaim {
            key: JournalKey {
                node: "surface".into(),
                action,
                target_request_id: "request-1".into(),
            },
            source_ulid: "01K23F4Q6X8A0B2C4D6E8F0G2H".into(),
            request_id: "request-1".into(),
            exact_body_sha256: "a".repeat(64),
            model_product: "Surface Pro 6".into(),
            model_generation: "pro6".into(),
            firmware_target: (action == JournalAction::FirmwareApply).then(|| {
                SurfaceFirmwareApplyTarget {
                    device_id: "device-1".into(),
                    inventory_published_at_ms: 90,
                    release_version: "1.2.3".into(),
                    release_checksum: "f".repeat(64),
                }
            }),
            claimed_at_ms: 100,
            expires_at_ms: 1_000,
        }
    }
    fn new_journal() -> (tempfile::TempDir, SurfaceActionJournal) {
        let parent = tempfile::tempdir().unwrap();
        let uid = rustix::process::geteuid().as_raw();
        let journal = SurfaceActionJournal::open_at(parent.path().join("journal"), uid).unwrap();
        (parent, journal)
    }
    fn decision(outcome: JournalOutcome) -> JournalDecision {
        let body = format!(r#"{{"outcome":"{outcome:?}"}}"#);
        JournalDecision {
            outcome,
            decided_at_ms: 200,
            result_sha256: sha256(body.as_bytes()),
            result_body: body,
        }
    }
    fn cancel(action: &ActionClaim) -> CancelIntent {
        CancelIntent {
            source_ulid: "01K23F4Q6X8A0B2C4D6E8F0G2J".into(),
            cancellation_id: "cancel-1".into(),
            exact_body_sha256: "b".repeat(64),
            target: action.clone(),
            claimed_at_ms: 110,
            expires_at_ms: 140,
        }
    }

    #[test]
    fn action_claim_closes_retries_and_gcs_only_after_publication() {
        let (_root, journal) = new_journal();
        let claim = claim(JournalAction::Enable);
        assert_eq!(
            journal.claim_action(&claim).unwrap(),
            ClaimDisposition::Claimed
        );
        assert_eq!(
            journal.claim_action(&claim).unwrap(),
            ClaimDisposition::AlreadyClaimed
        );
        let decision = decision(JournalOutcome::ActionCompleted);
        assert_eq!(
            journal.record_decision(&claim.key, &decision).unwrap(),
            DecisionDisposition::Recorded
        );
        assert_eq!(journal.unpublished().unwrap().len(), 1);
        assert!(journal.mark_published(&claim.key, &"0".repeat(64)).is_err());
        journal
            .mark_published(&claim.key, &decision.result_sha256)
            .unwrap();
        assert!(journal.unpublished().unwrap().is_empty());
        assert_eq!(journal.gc_expired(1_001).unwrap(), 0);
        assert_eq!(
            journal
                .gc_expired(1_000 + SURFACE_ACTION_JOURNAL_RETENTION_MS)
                .unwrap(),
            0
        );
        assert_eq!(
            journal
                .gc_expired(1_001 + SURFACE_ACTION_JOURNAL_RETENTION_MS)
                .unwrap(),
            1
        );
        assert!(journal.get(&claim.key).unwrap().is_none());
    }

    #[test]
    fn cancellation_can_win_absent_but_never_after_action_claim() {
        let (_root, journal) = new_journal();
        let action = claim(JournalAction::FirmwareApply);
        let cancel = cancel(&action);
        assert_eq!(
            journal.record_cancel_intent(&action.key, &cancel).unwrap(),
            CancelDisposition::CancelledPending
        );
        assert_eq!(
            journal.claim_action(&action).unwrap(),
            ClaimDisposition::CancellationWon
        );
        let (_root2, journal2) = new_journal();
        assert_eq!(
            journal2.claim_action(&action).unwrap(),
            ClaimDisposition::Claimed
        );
        assert_eq!(
            journal2.record_cancel_intent(&action.key, &cancel).unwrap(),
            CancelDisposition::ActionAlreadyClaimed
        );
        let too_late = decision(JournalOutcome::Refused);
        assert_eq!(
            journal2
                .record_late_cancel_decision(&action.key, &too_late)
                .unwrap(),
            DecisionDisposition::Recorded
        );
        assert!(journal2.unpublished().unwrap().is_empty());
        assert_eq!(journal2.unpublished_late_cancellations().unwrap().len(), 1);
        let interrupted = decision(JournalOutcome::Interrupted);
        assert_eq!(
            journal2.record_decision(&action.key, &interrupted).unwrap(),
            DecisionDisposition::Recorded
        );
        let retained = journal2.unpublished().unwrap();
        assert_eq!(retained.len(), 1);
        assert!(matches!(
            &retained[0].phase,
            JournalPhase::Closed {
                cancel: Some(bound_cancel),
                winner: JournalWinner::Action,
                published: false,
                ..
            } if bound_cancel == &cancel
        ));
        journal2
            .mark_published(&action.key, &interrupted.result_sha256)
            .unwrap();
        assert_eq!(journal2.gc_expired(u64::MAX).unwrap(), 0);
        journal2
            .mark_late_cancel_published(&action.key, &too_late.result_sha256)
            .unwrap();
        assert_eq!(journal2.gc_expired(u64::MAX).unwrap(), 1);

        let (_root3, journal3) = new_journal();
        assert_eq!(
            journal3.claim_action(&action).unwrap(),
            ClaimDisposition::Claimed
        );
        let completed = decision(JournalOutcome::ActionCompleted);
        journal3.record_decision(&action.key, &completed).unwrap();
        assert_eq!(
            journal3.record_cancel_intent(&action.key, &cancel).unwrap(),
            CancelDisposition::ActionAlreadyClaimed,
            "a cancellation arriving after action completion still needs its own TooLate outbox"
        );
        journal3
            .record_late_cancel_decision(&action.key, &too_late)
            .unwrap();
        assert_eq!(journal3.unpublished().unwrap().len(), 1);
        assert_eq!(journal3.unpublished_late_cancellations().unwrap().len(), 1);
    }

    #[test]
    fn hostile_paths_modes_and_bounds_fail_closed() {
        use std::os::unix::fs::symlink;
        let (root, journal) = new_journal();
        let victim = root.path().join("victim");
        std::fs::write(&victim, b"victim").unwrap();
        let path = journal.record_path(&claim(JournalAction::Enable).key);
        symlink(&victim, &path).unwrap();
        assert!(journal.claim_action(&claim(JournalAction::Enable)).is_err());
        assert_eq!(std::fs::read(&victim).unwrap(), b"victim");
    }

    #[test]
    fn replacing_root_path_cannot_redirect_descriptor_anchored_operations() {
        use std::os::unix::fs::PermissionsExt as _;
        let (parent, journal) = new_journal();
        let original_path = journal.root.clone();
        let retained_path = parent.path().join("retained-journal");
        std::fs::rename(&original_path, &retained_path).unwrap();
        std::fs::create_dir(&original_path).unwrap();
        std::fs::set_permissions(&original_path, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(original_path.join("attacker-marker"), b"untouched").unwrap();

        let action = claim(JournalAction::Enable);
        assert_eq!(
            journal.claim_action(&action).unwrap(),
            ClaimDisposition::Claimed
        );
        assert!(retained_path
            .join(journal.record_name(&action.key))
            .is_file());
        assert_eq!(
            std::fs::read(original_path.join("attacker-marker")).unwrap(),
            b"untouched"
        );
        assert_eq!(std::fs::read_dir(&original_path).unwrap().count(), 1);
    }

    #[test]
    fn reopen_recovers_claim_and_unpublished_terminal_without_repeating() {
        let (root, journal) = new_journal();
        let path = journal.root.clone();
        let uid = journal.trusted_uid;
        let action = claim(JournalAction::Enable);
        journal.claim_action(&action).unwrap();
        drop(journal);
        let reopened = SurfaceActionJournal::open_at(path.clone(), uid).unwrap();
        assert!(matches!(
            reopened.get(&action.key).unwrap().unwrap().phase,
            JournalPhase::ActionClaimed { .. }
        ));
        let terminal = decision(JournalOutcome::Interrupted);
        reopened.record_decision(&action.key, &terminal).unwrap();
        drop(reopened);
        let restarted = SurfaceActionJournal::open_at(path, uid).unwrap();
        assert_eq!(restarted.unpublished().unwrap().len(), 1);
        assert_eq!(
            restarted.record_decision(&action.key, &terminal).unwrap(),
            DecisionDisposition::AlreadyRecorded
        );
        drop(root);
    }

    #[test]
    fn competing_action_claims_and_cancel_action_race_have_one_durable_winner() {
        use std::sync::{Arc, Barrier};
        use std::thread;
        let (_root, journal) = new_journal();
        let journal = Arc::new(journal);
        let first = claim(JournalAction::Enable);
        let mut conflicting = first.clone();
        conflicting.exact_body_sha256 = "c".repeat(64);
        let barrier = Arc::new(Barrier::new(3));
        let handles = [first.clone(), conflicting].map(|candidate| {
            let journal = Arc::clone(&journal);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                journal.claim_action(&candidate)
            })
        });
        barrier.wait();
        let outcomes = handles.map(|handle| handle.join().unwrap());
        assert_eq!(
            outcomes
                .iter()
                .filter(|result| matches!(result, Ok(ClaimDisposition::Claimed)))
                .count(),
            1
        );
        assert!(journal.get(&first.key).unwrap().is_some());

        let (_root2, race) = new_journal();
        let race = Arc::new(race);
        let action = claim(JournalAction::FirmwareApply);
        let cancellation = cancel(&action);
        let barrier = Arc::new(Barrier::new(3));
        let action_handle = {
            let race = Arc::clone(&race);
            let barrier = Arc::clone(&barrier);
            let action = action.clone();
            thread::spawn(move || {
                barrier.wait();
                race.claim_action(&action)
            })
        };
        let cancel_handle = {
            let race = Arc::clone(&race);
            let barrier = Arc::clone(&barrier);
            let key = action.key.clone();
            thread::spawn(move || {
                barrier.wait();
                race.record_cancel_intent(&key, &cancellation)
            })
        };
        barrier.wait();
        let _ = action_handle.join().unwrap().unwrap();
        let _ = cancel_handle.join().unwrap().unwrap();
        assert!(matches!(
            race.get(&action.key).unwrap().unwrap().phase,
            JournalPhase::CancelClaimed { .. } | JournalPhase::ActionClaimedCancel { .. }
        ));
    }

    #[test]
    fn gc_is_conservative_for_live_cancel_and_unpublished_decision() {
        let (_root, journal) = new_journal();
        let action = claim(JournalAction::FirmwareApply);
        journal
            .record_cancel_intent(&action.key, &cancel(&action))
            .unwrap();
        assert_eq!(
            journal.gc_expired(u64::MAX).unwrap(),
            0,
            "live cancellation must never be GCed"
        );
        let terminal = decision(JournalOutcome::Cancelled);
        journal.record_decision(&action.key, &terminal).unwrap();
        assert_eq!(
            journal.gc_expired(u64::MAX).unwrap(),
            0,
            "unpublished result must never be GCed"
        );

        let (_root2, claimed) = new_journal();
        let action = claim(JournalAction::Enable);
        claimed.claim_action(&action).unwrap();
        assert_eq!(
            claimed
                .gc_expired(action.expires_at_ms + SURFACE_ACTION_JOURNAL_RETENTION_MS)
                .unwrap(),
            0
        );
        assert_eq!(
            claimed
                .gc_expired(action.expires_at_ms + SURFACE_ACTION_JOURNAL_RETENTION_MS + 1)
                .unwrap(),
            0,
            "orphan claims require a durable Interrupted result before GC"
        );
        assert!(matches!(
            claimed.pending_recovery().unwrap()[0].phase,
            JournalPhase::ActionClaimed { .. }
        ));
    }

    #[test]
    fn unsafe_owner_mode_oversize_unknown_duplicate_and_temp_fail_closed() {
        use std::os::unix::fs::{symlink, PermissionsExt as _};
        let (parent, journal) = new_journal();
        let uid = journal.trusted_uid;
        assert!(
            SurfaceActionJournal::open_at(journal.root.clone(), uid.saturating_add(1)).is_err()
        );
        let mut permissions = std::fs::metadata(&journal.root).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&journal.root, permissions).unwrap();
        assert!(journal.get(&claim(JournalAction::Enable).key).is_err());
        drop(parent);

        let (_root, journal) = new_journal();
        std::fs::write(journal.root.join("unknown"), b"x").unwrap();
        assert!(journal.unpublished().is_err());

        let (_root, journal) = new_journal();
        let action = claim(JournalAction::Enable);
        journal.claim_action(&action).unwrap();
        let path = journal.record_path(&action.key);
        let raw = std::fs::read_to_string(&path).unwrap();
        std::fs::write(
            &path,
            raw.replacen(
                "{\"schema_version\":1",
                "{\"schema_version\":1,\"schema_version\":1",
                1,
            ),
        )
        .unwrap();
        assert!(journal.get(&action.key).is_err());

        let (_root, journal) = new_journal();
        let path = journal.record_path(&action.key);
        std::fs::write(&path, vec![b'x'; MAX_RECORD_BYTES as usize + 1]).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(journal.get(&action.key).is_err());

        let (root, journal) = new_journal();
        let victim = root.path().join("temp-victim");
        std::fs::write(&victim, b"safe").unwrap();
        symlink(&victim, journal.root.join(".hostile.tmp")).unwrap();
        assert!(journal.unpublished().is_err());
        assert_eq!(std::fs::read(&victim).unwrap(), b"safe");

        let (_root, journal) = new_journal();
        let valid_temp = journal.root.join(format!(".{}.1.tmp", "d".repeat(64)));
        std::fs::write(&valid_temp, b"incomplete").unwrap();
        std::fs::set_permissions(&valid_temp, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(journal.unpublished().unwrap().is_empty());
        assert!(!valid_temp.exists(), "safe crash temp must be reaped");

        let (_root, journal) = new_journal();
        for index in 0..=MAX_RECORDS {
            std::fs::write(journal.root.join(format!("hostile-{index}")), b"x").unwrap();
        }
        assert_eq!(
            cleanup_temps(&journal.directory, journal.trusted_uid).unwrap_err(),
            "Surface journal cleanup exceeds its directory entry bound"
        );
    }

    #[test]
    fn capacity_is_a_hard_refusal_without_eviction() {
        let (_root, journal) = new_journal();
        for index in 0..MAX_RECORDS {
            let mut action = claim(JournalAction::Enable);
            action.key.target_request_id = format!("request-{index}");
            action.request_id.clone_from(&action.key.target_request_id);
            action.source_ulid = format!("{index:026}");
            assert_eq!(
                journal.claim_action(&action).unwrap(),
                ClaimDisposition::Claimed
            );
        }
        let mut overflow = claim(JournalAction::Enable);
        overflow.key.target_request_id = "overflow".into();
        overflow.request_id = "overflow".into();
        overflow.source_ulid = "99999999999999999999999999".into();
        assert!(journal.claim_action(&overflow).is_err());
        assert_eq!(journal.scan_locked().unwrap().len(), MAX_RECORDS);
    }

    #[test]
    fn timestamp_ordering_is_fail_closed() {
        let (_root, journal) = new_journal();
        let action = claim(JournalAction::Enable);
        journal.claim_action(&action).unwrap();
        let mut invalid = decision(JournalOutcome::ActionCompleted);
        invalid.decided_at_ms = 0;
        assert!(journal.record_decision(&action.key, &invalid).is_err());
        let mut early = decision(JournalOutcome::ActionCompleted);
        early.decided_at_ms = action.claimed_at_ms - 1;
        assert!(journal.record_decision(&action.key, &early).is_err());
        let mut invalid_cancel = cancel(&action);
        invalid_cancel.claimed_at_ms = action.expires_at_ms + 1;
        assert!(journal
            .record_cancel_intent(&action.key, &invalid_cancel)
            .is_err());

        let (_root, models) = new_journal();
        let mut foreign_model = claim(JournalAction::Enable);
        foreign_model.model_product = "Surface Pro 7".into();
        assert!(models.claim_action(&foreign_model).is_err());

        let (_root, binding) = new_journal();
        let action = claim(JournalAction::FirmwareApply);
        let mut cancellation = cancel(&action);
        cancellation.target.exact_body_sha256 = "e".repeat(64);
        binding.claim_action(&action).unwrap();
        assert!(binding
            .record_cancel_intent(&action.key, &cancellation)
            .is_err());
    }
}
