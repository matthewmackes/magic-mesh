//! WL-FUNC-022 S2 — daemon-owned local Clock persistence and deadlines.

#![cfg(feature = "async-services")]
#![allow(
    missing_docs,
    reason = "private authority machinery is exercised by operational tests"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use ed25519_dalek::{SigningKey, VerifyingKey};
use jiff::{civil::Weekday as JiffWeekday, tz::TimeZone, Timestamp, ToSpan as _};
use mackes_mesh_types::clock::{
    clock_audio_status_topic, clock_command_topic, clock_state_topic, ClockAcknowledgementV1,
    ClockAlarmRecurrenceV1, ClockAudioActionV1, ClockAudioRequestV1, ClockAudioStatusV1,
    ClockCommandKindV1, ClockCommandV1, ClockFoldPolicy, ClockOccurrencePhase, ClockOccurrenceV1,
    ClockScheduleKindV1, ClockScheduleV1, ClockSettingsV1, ClockSnapshotV1, ClockStopwatchV1,
    ClockTargetDisposition, ClockTargetState, ClockTimerPhase, ClockValidationContext,
    ClockWeekday, CLOCK_AUDIO_ACTION_TOPIC, CLOCK_SCHEMA_VERSION, MAX_CLOCK_AUDIO_REQUEST_TTL_MS,
    MAX_CLOCK_COMMAND_TTL_MS, MAX_CLOCK_OCCURRENCES, MAX_CLOCK_STOPWATCH_ELAPSED_MS,
};
use mackes_mesh_types::music_auth::{self, MusicAuthContext, MUSIC_AUTH_CREDENTIAL_NAME};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use rand::RngCore as _;
use sha2::{Digest as _, Sha256};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt as _;

use super::{ShutdownToken, Worker};
use crate::store::writer::{self, ClockAuthorityRecord, WriteOp};

const POLL: Duration = Duration::from_millis(250);
const MIN_BUS_RETRY_INTERVAL: Duration = Duration::from_millis(10);
const MAX_BUS_RETRY_INTERVAL: Duration = Duration::from_secs(2);
const VERIFYING_KEY_ENV: &str = "MDE_CLOCK_VERIFYING_KEY_FILE";
const SIGNING_KEY_ENV: &str = "MDE_CLOCK_SIGNING_KEY_FILE";
const SIGNER_ID_ENV: &str = "MDE_CLOCK_SIGNER_ID";
const APPROVED_PEERS_ENV: &str = "MDE_CLOCK_APPROVED_PEERS";
const BLOCKED_ORIGINS_ENV: &str = "MDE_CLOCK_BLOCKED_ORIGINS";
const DISABLED_SCHEDULES_ENV: &str = "MDE_CLOCK_DISABLED_SCHEDULES";
const MAX_VERIFYING_KEY_FILE_BYTES: usize = 128;
const MAX_SIGNING_KEY_FILE_BYTES: usize = 128;
const MAX_MUSIC_SIGNING_KEY_FILE_BYTES: usize = 128;
const MAX_AUDIO_STATUS_PER_TICK: usize = 512;
const MAX_PEER_COMMANDS_PER_TICK: usize = 128;
const MAX_PEER_CONVERGENCE_PROBES_PER_TICK: usize = 512;
const AUDIO_RETRY_MS: i64 = 5_000;
const PEER_RETRY_MS: i64 = 5_000;
const CLOCK_AUDIO_AUTH_VERB: &str = "music-clock-audio";
const CLOCK_AUDIO_AUTH_TARGET: &str = "clock-audio";

trait WallClock: Send + Sync {
    fn now_ms(&self) -> i64;
}

struct SystemWallClock;

impl WallClock for SystemWallClock {
    fn now_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(1, |duration| {
                i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
            })
    }
}

trait ClockStore: Send + Sync {
    fn load(&self, node_id: &str) -> anyhow::Result<Option<ClockAuthorityRecord>>;
    fn commit(
        &self,
        node_id: &str,
        expected_revision: u64,
        snapshot: &ClockSnapshotV1,
        request_id: Option<&str>,
        request_fingerprint: Option<&str>,
        action_cursor: Option<&str>,
        audio_requests: &[writer::ClockAudioOutboxWrite],
    ) -> anyhow::Result<bool>;
    fn pending_audio(&self, node_id: &str) -> anyhow::Result<Vec<writer::ClockAudioOutboxRecord>>;
    fn acknowledge_audio(&self, node_id: &str, status: &ClockAudioStatusV1)
        -> anyhow::Result<bool>;
}

struct SqliteClockStore {
    db_path: PathBuf,
}

impl SqliteClockStore {
    fn dispatch(&self, operation: WriteOp) -> anyhow::Result<writer::WriteResponse> {
        if let Some(response) = writer::request_if_serving(operation.clone())? {
            return Ok(response);
        }
        let connection = crate::store::open(&self.db_path)?;
        writer::request_or_execute(&connection, operation)
    }
}

impl ClockStore for SqliteClockStore {
    fn load(&self, node_id: &str) -> anyhow::Result<Option<ClockAuthorityRecord>> {
        self.dispatch(WriteOp::LoadClockAuthority {
            node_id: node_id.to_owned(),
        })?
        .into_clock_authority()
    }

    fn commit(
        &self,
        node_id: &str,
        expected_revision: u64,
        snapshot: &ClockSnapshotV1,
        request_id: Option<&str>,
        request_fingerprint: Option<&str>,
        action_cursor: Option<&str>,
        audio_requests: &[writer::ClockAudioOutboxWrite],
    ) -> anyhow::Result<bool> {
        self.dispatch(WriteOp::CommitClockAuthority {
            node_id: node_id.to_owned(),
            expected_revision,
            new_revision: snapshot.revision,
            request_id: request_id.map(str::to_owned),
            request_fingerprint: request_fingerprint.map(str::to_owned),
            action_cursor: action_cursor.map(str::to_owned),
            snapshot_json: serde_json::to_string(snapshot)?,
            updated_at_ms: snapshot.produced_at_utc_ms,
            audio_requests: audio_requests.to_vec(),
        })?
        .into_changed()
    }

    fn pending_audio(&self, node_id: &str) -> anyhow::Result<Vec<writer::ClockAudioOutboxRecord>> {
        self.dispatch(WriteOp::LoadPendingClockAudio {
            node_id: node_id.to_owned(),
        })?
        .into_clock_audio_outbox()
    }

    fn acknowledge_audio(
        &self,
        node_id: &str,
        status: &ClockAudioStatusV1,
    ) -> anyhow::Result<bool> {
        self.dispatch(WriteOp::AcknowledgeClockAudio {
            node_id: node_id.to_owned(),
            request_id: status.request_id.clone(),
            occurrence_id: status.occurrence_id.clone(),
            global_event_id: status.global_event_id.clone(),
            occurrence_generation: status.occurrence_generation,
            acknowledged_at_ms: status.observed_at_utc_ms,
        })?
        .into_changed()
    }
}

#[derive(Clone)]
struct TrustedSigner {
    signer_id: String,
    key: VerifyingKey,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ClockBusIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

struct ClockBusTransaction {
    root: PathBuf,
    persist: Persist,
    identity: ClockBusIdentity,
}

impl ClockBusTransaction {
    fn verify_current(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            clock_bus_identity(&self.root)? == self.identity,
            "Clock Bus index changed during transaction"
        );
        Ok(())
    }

    fn write(&self, topic: &str, body: &str) -> anyhow::Result<()> {
        self.verify_current()?;
        self.persist
            .write(topic, Priority::Default, None, Some(body))?;
        self.verify_current()
    }
}

pub struct ClockWorker {
    node_id: String,
    bus_root: Option<PathBuf>,
    poll: Duration,
    clock: Arc<dyn WallClock>,
    store: Arc<dyn ClockStore>,
    signer: Option<TrustedSigner>,
    command_signing_key: Option<SigningKey>,
    approved_peer_ids: BTreeSet<String>,
    blocked_origin_ids: BTreeSet<String>,
    disabled_schedule_ids: BTreeSet<String>,
    music_signing_seed: Option<[u8; 32]>,
    snapshot: Option<ClockSnapshotV1>,
    action_cursor: Option<String>,
    active_bus_identity: Option<ClockBusIdentity>,
    published_once: bool,
    audio_status_cursor: Option<String>,
    audio_last_sent_ms: BTreeMap<String, i64>,
    peer_last_sent_ms: BTreeMap<String, i64>,
}

#[derive(Clone)]
struct ClockMemoryCheckpoint {
    snapshot: Option<ClockSnapshotV1>,
    action_cursor: Option<String>,
    published_once: bool,
    audio_status_cursor: Option<String>,
    audio_last_sent_ms: BTreeMap<String, i64>,
    peer_last_sent_ms: BTreeMap<String, i64>,
}

impl ClockWorker {
    #[must_use]
    pub fn new(node_id: String, db_path: PathBuf) -> Self {
        let signer = load_trusted_signer();
        let command_signing_key = load_command_signing_key().filter(|key| {
            signer
                .as_ref()
                .is_some_and(|trusted| key.verifying_key() == trusted.key)
        });
        Self {
            node_id,
            // `None` is intentionally unresolved here. A daemon launched
            // before its user/XDG root exists must still select the canonical
            // system spool when this same worker activates.
            bus_root: None,
            poll: POLL,
            clock: Arc::new(SystemWallClock),
            store: Arc::new(SqliteClockStore { db_path }),
            signer,
            command_signing_key,
            approved_peer_ids: load_id_set(APPROVED_PEERS_ENV),
            blocked_origin_ids: load_id_set(BLOCKED_ORIGINS_ENV),
            disabled_schedule_ids: load_id_set(DISABLED_SCHEDULES_ENV),
            music_signing_seed: load_music_signing_seed(),
            snapshot: None,
            action_cursor: None,
            active_bus_identity: None,
            published_once: false,
            audio_status_cursor: None,
            audio_last_sent_ms: BTreeMap::new(),
            peer_last_sent_ms: BTreeMap::new(),
        }
    }

    fn context(&self, now_ms: i64) -> ClockValidationContext<'_> {
        ClockValidationContext {
            wall_utc_ms: now_ms,
            monotonic_ms: 1,
            zone_exists: &zone_exists,
        }
    }

    fn bus_root(&self) -> PathBuf {
        clock_bus_root(self.bus_root.clone())
    }

    fn open_bus_transaction(&self) -> anyhow::Result<ClockBusTransaction> {
        let root = self.bus_root();
        let identity_before = match clock_bus_identity(&root) {
            Ok(identity) => identity,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                // The first Persist may create a late Bus. It is only an
                // initializer: bracket the connection returned to the caller
                // with identity observations of the now-existing index.
                drop(Persist::open(root.clone())?);
                clock_bus_identity(&root)?
            }
            Err(error) => return Err(error.into()),
        };
        let persist = Persist::open(root.clone())?;
        let identity_after = clock_bus_identity(&root)?;
        anyhow::ensure!(
            identity_before == identity_after,
            "Clock Bus index changed while opening transaction"
        );
        Ok(ClockBusTransaction {
            root,
            persist,
            identity: identity_after,
        })
    }

    fn activate_bus(&mut self, transaction: &ClockBusTransaction) -> anyhow::Result<()> {
        transaction.verify_current()?;
        let replacing = self
            .active_bus_identity
            .is_some_and(|identity| identity != transaction.identity);
        let command_tail = transaction
            .persist
            .latest_ulid(&clock_command_topic(&self.node_id)?)?;
        let audio_tail = transaction
            .persist
            .latest_ulid(&clock_audio_status_topic(&self.node_id)?)?;
        let now_ms = self.clock.now_ms();
        let peer_snapshots = self.collect_peer_snapshots(transaction, now_ms)?;
        transaction.verify_current()?;

        let checkpoint = self.checkpoint();
        let result = (|| {
            if replacing {
                // A replacement index is a new transient generation. Rows it
                // already retains are baseline, not commands/statuses arriving
                // after this worker bound the generation. The authority DB and
                // audio outbox remain the durable replay source.
                self.action_cursor = command_tail;
                self.audio_status_cursor = audio_tail;
                self.audio_last_sent_ms.clear();
                self.peer_last_sent_ms.clear();
            }
            self.published_once = false;
            transaction.verify_current()?;

            if replacing {
                let expected = self
                    .snapshot
                    .as_ref()
                    .expect("Clock snapshot loaded")
                    .revision;
                self.commit_then_publish(transaction, expected, None, None, &[])?;
            } else {
                self.publish(transaction)?;
            }
            self.publish_pending_audio(transaction, now_ms)?;
            self.publish_peer_convergence(transaction, now_ms, &peer_snapshots)?;
            transaction.verify_current()?;
            Ok::<(), anyhow::Error>(())
        })();
        if let Err(error) = result {
            self.restore_checkpoint(checkpoint);
            return Err(error);
        }
        self.active_bus_identity = Some(transaction.identity);
        Ok(())
    }

    fn checkpoint(&self) -> ClockMemoryCheckpoint {
        ClockMemoryCheckpoint {
            snapshot: self.snapshot.clone(),
            action_cursor: self.action_cursor.clone(),
            published_once: self.published_once,
            audio_status_cursor: self.audio_status_cursor.clone(),
            audio_last_sent_ms: self.audio_last_sent_ms.clone(),
            peer_last_sent_ms: self.peer_last_sent_ms.clone(),
        }
    }

    fn restore_checkpoint(&mut self, checkpoint: ClockMemoryCheckpoint) {
        self.snapshot = checkpoint.snapshot;
        self.action_cursor = checkpoint.action_cursor;
        self.published_once = checkpoint.published_once;
        self.audio_status_cursor = checkpoint.audio_status_cursor;
        self.audio_last_sent_ms = checkpoint.audio_last_sent_ms;
        self.peer_last_sent_ms = checkpoint.peer_last_sent_ms;
    }

    fn reload_durable_authority(&mut self, now_ms: i64) -> anyhow::Result<()> {
        let record = self
            .store
            .load(&self.node_id)?
            .ok_or_else(|| anyhow::anyhow!("Clock durable authority disappeared"))?;
        let snapshot = ClockSnapshotV1::from_persisted_json_at(
            record.snapshot_json.as_bytes(),
            &self.context(now_ms),
        )?;
        anyhow::ensure!(snapshot.node_id == self.node_id, "Clock node mismatch");
        anyhow::ensure!(
            snapshot.revision == record.revision,
            "Clock revision mismatch"
        );
        validate_stopwatch_deadlines(&snapshot, now_ms)?;
        self.snapshot = Some(snapshot);
        self.action_cursor = record.action_cursor;
        // A durable commit may have succeeded immediately before its Bus
        // publication failed. Force the next sweep to repair that publication.
        self.published_once = false;
        Ok(())
    }

    fn initial_snapshot(&self, now_ms: i64) -> ClockSnapshotV1 {
        ClockSnapshotV1 {
            schema_version: CLOCK_SCHEMA_VERSION,
            node_id: self.node_id.clone(),
            revision: 1,
            produced_at_utc_ms: now_ms,
            schedules: Vec::new(),
            occurrences: Vec::new(),
            stopwatches: Vec::new(),
            settings: ClockSettingsV1::defaults(local_time_zone()),
        }
    }

    fn ensure_loaded(&mut self) -> anyhow::Result<()> {
        if self.snapshot.is_some() {
            return Ok(());
        }
        let now_ms = self.clock.now_ms();
        if let Some(record) = self.store.load(&self.node_id)? {
            let snapshot = ClockSnapshotV1::from_persisted_json_at(
                record.snapshot_json.as_bytes(),
                &self.context(now_ms),
            )?;
            anyhow::ensure!(snapshot.node_id == self.node_id, "Clock node mismatch");
            anyhow::ensure!(
                snapshot.revision == record.revision,
                "Clock revision mismatch"
            );
            validate_stopwatch_deadlines(&snapshot, now_ms)?;
            self.action_cursor = record.action_cursor;
            self.snapshot = Some(snapshot);
            self.recover_durable_authority(now_ms)?;
            return Ok(());
        }
        let snapshot = self.initial_snapshot(now_ms);
        anyhow::ensure!(
            self.store
                .commit(&self.node_id, 0, &snapshot, None, None, None, &[])?,
            "initial Clock authority was not persisted"
        );
        self.snapshot = Some(snapshot);
        Ok(())
    }

    fn recover_durable_authority(&mut self, now_ms: i64) -> anyhow::Result<()> {
        let prior = self
            .snapshot
            .as_ref()
            .expect("Clock snapshot loaded for recovery")
            .clone();
        let expected = prior.revision;
        let changed = self.advance_deadlines(now_ms)?;
        if changed {
            let snapshot = self.snapshot.as_mut().expect("Clock snapshot loaded");
            snapshot.revision = expected
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("Clock revision exhausted"))?;
            snapshot.produced_at_utc_ms = now_ms;
            stamp_revision(snapshot, &self.node_id);
            preserve_unchanged_occurrence_revisions(snapshot, &prior);
        }

        let snapshot = self.snapshot.as_ref().expect("Clock snapshot loaded");
        let mut audio_requests = clock_audio_transitions(&prior, snapshot, now_ms, &self.node_id)?;
        // Music's alert authority is intentionally in-process. Re-create Start
        // rows for occurrences that remain Ringing after elapsed deadlines are
        // recovered. Newly due occurrences produce the same deterministic row,
        // so retain only one copy in this atomic authority/outbox transaction.
        for request in clock_audio_recovery_requests(snapshot, now_ms, &self.node_id)? {
            if !audio_requests
                .iter()
                .any(|candidate| candidate.request_id == request.request_id)
            {
                audio_requests.push(request);
            }
        }
        if !changed && audio_requests.is_empty() {
            return Ok(());
        }

        // Recovery is a fresh production even when semantic authority is
        // unchanged. This binds reconstructed outbox rows to the restart sample
        // instead of the stale pre-crash timestamp.
        self.snapshot
            .as_mut()
            .expect("Clock snapshot loaded")
            .produced_at_utc_ms = now_ms;
        anyhow::ensure!(
            self.store.commit(
                &self.node_id,
                expected,
                self.snapshot.as_ref().expect("Clock snapshot loaded"),
                None,
                None,
                self.action_cursor.as_deref(),
                &audio_requests,
            )?,
            "Clock deadline recovery was not persisted"
        );
        // Durable scheduling must not wait for the transient Bus. Its next
        // generation publishes this recovered snapshot and drains the outbox.
        self.published_once = false;
        Ok(())
    }

    fn publish(&mut self, transaction: &ClockBusTransaction) -> anyhow::Result<()> {
        let snapshot = self.snapshot.as_ref().expect("Clock snapshot loaded");
        let body = serde_json::to_string(snapshot)?;
        transaction.write(&clock_state_topic(&self.node_id)?, &body)?;
        self.published_once = true;
        Ok(())
    }

    fn commit_then_publish(
        &mut self,
        transaction: &ClockBusTransaction,
        expected_revision: u64,
        request_id: Option<&str>,
        request_fingerprint: Option<&str>,
        audio_requests: &[writer::ClockAudioOutboxWrite],
    ) -> anyhow::Result<bool> {
        transaction.verify_current()?;
        let applied = self.store.commit(
            &self.node_id,
            expected_revision,
            self.snapshot.as_ref().expect("Clock snapshot loaded"),
            request_id,
            request_fingerprint,
            self.action_cursor.as_deref(),
            audio_requests,
        )?;
        if applied {
            transaction.verify_current()?;
            self.publish(transaction)?;
        } else {
            let record = self
                .store
                .load(&self.node_id)?
                .ok_or_else(|| anyhow::anyhow!("Clock replay lost durable authority"))?;
            let snapshot = ClockSnapshotV1::from_persisted_json_at(
                record.snapshot_json.as_bytes(),
                &self.context(self.clock.now_ms()),
            )?;
            anyhow::ensure!(snapshot.node_id == self.node_id, "Clock node mismatch");
            anyhow::ensure!(
                snapshot.revision == record.revision,
                "Clock revision mismatch"
            );
            validate_stopwatch_deadlines(&snapshot, self.clock.now_ms())?;
            self.action_cursor = record.action_cursor;
            self.snapshot = Some(snapshot);
            // A prior attempt may have committed and then failed to publish.
            // Replaying that same command reloads the durable winner and must
            // repair the required state publication before it is complete.
            transaction.verify_current()?;
            self.publish(transaction)?;
        }
        Ok(applied)
    }

    fn collect_actions(
        &self,
        transaction: &ClockBusTransaction,
    ) -> anyhow::Result<Vec<(String, String)>> {
        let topic = clock_command_topic(&self.node_id)?;
        let mut actions = transaction
            .persist
            .list_since(&topic, self.action_cursor.as_deref())?
            .into_iter()
            .map(|message| (message.ulid, message.body.unwrap_or_default()))
            .collect::<Vec<_>>();
        actions.sort_by(|left, right| left.0.cmp(&right.0));
        transaction.verify_current()?;
        Ok(actions)
    }

    fn process_command(
        &mut self,
        transaction: &ClockBusTransaction,
        body: &[u8],
        now_ms: i64,
    ) -> anyhow::Result<()> {
        let Some(signer) = &self.signer else {
            return self.persist_cursor_only(transaction, now_ms);
        };
        let context = self.context(now_ms);
        let Ok(command) = ClockCommandV1::from_json_at(body, &context) else {
            return self.persist_cursor_only(transaction, now_ms);
        };
        let Ok(command) = command.admit_at(&signer.signer_id, &signer.key, &context) else {
            return self.persist_cursor_only(transaction, now_ms);
        };
        let request_fingerprint = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&command).context("encoding admitted Clock command")?
            )
        );
        let expected = self
            .snapshot
            .as_ref()
            .expect("Clock snapshot loaded")
            .revision;
        let peer_origin = command.origin_node_id != self.node_id;
        let revision_is_invalid = if peer_origin {
            command.expected_revision > expected
        } else {
            command.expected_revision != expected
        };
        let peer_is_untrusted = peer_origin
            && (!self.approved_peer_ids.contains(&command.origin_node_id)
                || self.blocked_origin_ids.contains(&command.origin_node_id));
        if revision_is_invalid || peer_is_untrusted {
            return self.persist_cursor_only(transaction, now_ms);
        }
        let prior_snapshot = self
            .snapshot
            .as_ref()
            .expect("Clock snapshot loaded")
            .clone();
        let command_origin = command.origin_node_id.clone();
        let request_id = command.request_id.clone();
        let applied = match self.apply_command(
            command.body,
            &command_origin,
            peer_origin,
            &request_id,
            command.expected_revision,
            command.issued_at_utc_ms,
            now_ms,
        ) {
            Ok(applied) => applied,
            Err(_) => {
                self.snapshot = Some(prior_snapshot);
                return self.persist_cursor_only(transaction, now_ms);
            }
        };
        if !applied {
            self.snapshot = Some(prior_snapshot);
            return self
                .commit_then_publish(
                    transaction,
                    expected,
                    Some(&request_id),
                    Some(&request_fingerprint),
                    &[],
                )
                .map(|_| ());
        }
        let snapshot = self.snapshot.as_mut().expect("Clock snapshot loaded");
        snapshot.revision = expected
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("Clock revision exhausted"))?;
        snapshot.produced_at_utc_ms = now_ms;
        stamp_revision(snapshot, &self.node_id);
        preserve_unchanged_occurrence_revisions(snapshot, &prior_snapshot);
        let audio_requests =
            clock_audio_transitions(&prior_snapshot, snapshot, now_ms, &self.node_id)?;
        self.commit_then_publish(
            transaction,
            expected,
            Some(&request_id),
            Some(&request_fingerprint),
            &audio_requests,
        )?;
        Ok(())
    }

    fn persist_cursor_only(
        &mut self,
        transaction: &ClockBusTransaction,
        _now_ms: i64,
    ) -> anyhow::Result<()> {
        let expected = self
            .snapshot
            .as_ref()
            .expect("Clock snapshot loaded")
            .revision;
        self.commit_then_publish(transaction, expected, None, None, &[])?;
        Ok(())
    }

    fn apply_command(
        &mut self,
        command: ClockCommandKindV1,
        command_origin: &str,
        peer_origin: bool,
        request_id: &str,
        expected_snapshot_revision: u64,
        issued_at_ms: i64,
        now_ms: i64,
    ) -> anyhow::Result<bool> {
        // A locally authored command is the only point at which this node
        // chooses remote executors. Do not persist a target that cannot take
        // part in the signed convergence protocol: it would make a schedule
        // or mirror look durable even though no admitted peer can receive it.
        // Peer-originated commands deliberately retain their full target set;
        // this node only admits those when it is one of the selected targets.
        let local_node_id = self.node_id.clone();
        let approved_peer_ids = self.approved_peer_ids.clone();
        let snapshot = self.snapshot.as_mut().expect("Clock snapshot loaded");
        let changed = match command {
            ClockCommandKindV1::UpsertSchedule { mut schedule } => {
                anyhow::ensure!(
                    schedule.origin_node_id == command_origin,
                    "Clock schedule origin mismatch"
                );
                if !peer_origin {
                    anyhow::ensure!(
                        clock_targets_are_admitted(
                            &schedule.selected_target_ids,
                            &local_node_id,
                            &approved_peer_ids,
                        ),
                        "Clock schedule target is not an approved peer"
                    );
                }
                if peer_origin {
                    anyhow::ensure!(
                        schedule
                            .selected_target_ids
                            .iter()
                            .any(|id| id == &self.node_id),
                        "Clock peer schedule does not target this node"
                    );
                }
                let existing = snapshot
                    .schedules
                    .iter()
                    .find(|value| value.schedule_id == schedule.schedule_id)
                    .cloned();
                let timer_extension_acknowledgement = if let Some(existing) = &existing {
                    anyhow::ensure!(
                        existing.origin_node_id == schedule.origin_node_id,
                        "Clock schedule identity conflict"
                    );
                    anyhow::ensure!(
                        schedule.revision <= existing.revision
                            || !snapshot.occurrences.iter().any(|occurrence| {
                                occurrence.schedule_id == existing.schedule_id
                                    && occurrence.phase == ClockOccurrencePhase::Ringing
                            }),
                        "Clock ringing occurrence retains its schedule authority"
                    );
                    if schedule.revision < existing.revision {
                        return Ok(false);
                    }
                    if schedule.revision == existing.revision {
                        anyhow::ensure!(
                            !peer_origin,
                            "peer Clock timer extension is not authoritative"
                        );
                        let acknowledgement = exact_timer_extension_acknowledgement(
                            snapshot,
                            existing,
                            &schedule,
                            command_origin,
                            request_id,
                            expected_snapshot_revision,
                            issued_at_ms,
                        )?;
                        schedule.revision = existing
                            .revision
                            .checked_add(1)
                            .ok_or_else(|| anyhow::anyhow!("Clock schedule revision exhausted"))?;
                        Some(acknowledgement)
                    } else {
                        None
                    }
                } else {
                    None
                };
                // A recurring alarm has no creation watermark in its wire
                // contract. Admitting one must therefore start with its next
                // selected civil day, not manufacture a missed occurrence for
                // the previous week. The durable snapshot timestamp becomes
                // its evaluation watermark after this command commits.
                let late_due = match &schedule.schedule {
                    ClockScheduleKindV1::Alarm(alarm)
                        if matches!(alarm.recurrence, ClockAlarmRecurrenceV1::Weekdays { .. }) =>
                    {
                        None
                    }
                    _ => due_now_or_before(&schedule, now_ms)?,
                };
                if let Some(due_at) = late_due {
                    expire_schedule(&mut schedule, due_at);
                    add_occurrence(
                        snapshot,
                        &schedule,
                        due_at,
                        ClockOccurrencePhase::Missed,
                        &schedule.origin_node_id,
                        &self.disabled_schedule_ids,
                    )?;
                }
                if let Some(existing) = snapshot
                    .schedules
                    .iter_mut()
                    .find(|value| value.schedule_id == schedule.schedule_id)
                {
                    *existing = schedule;
                } else {
                    snapshot.schedules.push(schedule);
                }
                if let Some((occurrence_id, acknowledgement)) = timer_extension_acknowledgement {
                    anyhow::ensure!(
                        acknowledge(snapshot, &occurrence_id, acknowledgement)?,
                        "Clock timer extension acknowledgement lost its exact occurrence"
                    );
                }
                true
            }
            ClockCommandKindV1::RemoveSchedule { schedule_id } => {
                if peer_origin {
                    anyhow::bail!("peer Clock schedule removal is not authoritative");
                }
                if !snapshot
                    .schedules
                    .iter()
                    .any(|value| value.schedule_id == schedule_id)
                {
                    return Ok(false);
                }
                let ringing_occurrence_ids = snapshot
                    .occurrences
                    .iter()
                    .filter(|occurrence| {
                        occurrence.schedule_id == schedule_id
                            && occurrence.phase == ClockOccurrencePhase::Ringing
                    })
                    .map(|occurrence| {
                        (
                            occurrence.occurrence_id.clone(),
                            occurrence.global_event_id.clone(),
                        )
                    })
                    .collect::<Vec<_>>();
                for (occurrence_id, global_event_id) in ringing_occurrence_ids {
                    let acknowledgement = ClockAcknowledgementV1 {
                        acknowledgement_id: schedule_removal_acknowledgement_id(
                            &occurrence_id,
                            request_id,
                        ),
                        global_event_id,
                        actor_node_id: command_origin.to_owned(),
                        actor_clock: expected_snapshot_revision.checked_add(1).ok_or_else(
                            || anyhow::anyhow!("Clock schedule removal generation exhausted"),
                        )?,
                        acknowledged_at_utc_ms: issued_at_ms,
                        stop: true,
                    };
                    anyhow::ensure!(
                        acknowledge(snapshot, &occurrence_id, acknowledgement)?,
                        "Clock schedule removal could not stop its ringing occurrence"
                    );
                }
                snapshot.occurrences.retain(|occurrence| {
                    occurrence.schedule_id != schedule_id
                        || occurrence.phase != ClockOccurrencePhase::Scheduled
                });
                snapshot
                    .schedules
                    .retain(|value| value.schedule_id != schedule_id);
                true
            }
            ClockCommandKindV1::SetScheduleEnabled {
                schedule_id,
                enabled,
            } => {
                let Some(schedule_index) = snapshot
                    .schedules
                    .iter()
                    .position(|value| value.schedule_id == schedule_id)
                else {
                    anyhow::bail!("Clock schedule does not exist");
                };
                let ClockScheduleKindV1::Alarm(alarm) =
                    &mut snapshot.schedules[schedule_index].schedule
                else {
                    anyhow::bail!("timer enable command is invalid");
                };
                if peer_origin {
                    anyhow::bail!("peer Clock enable mutation is not authoritative");
                }
                let mut changed = alarm.enabled != enabled;
                alarm.enabled = enabled;
                if !enabled {
                    // Snooze creates a durable Scheduled child independently
                    // of the parent alarm's recurrence.  Once the parent is
                    // disabled, that child must not survive to ring later. A
                    // one-time alarm is already disabled when it first rings.
                    // Disable must also terminally acknowledge every exact
                    // ringing generation so the atomic audio transition queues
                    // Music Stop instead of leaving a disabled alarm audible.
                    let ringing_occurrences = snapshot
                        .occurrences
                        .iter()
                        .filter(|occurrence| {
                            occurrence.schedule_id == schedule_id
                                && occurrence.phase == ClockOccurrencePhase::Ringing
                        })
                        .map(|occurrence| {
                            (
                                occurrence.occurrence_id.clone(),
                                occurrence.global_event_id.clone(),
                            )
                        })
                        .collect::<Vec<_>>();
                    let actor_clock =
                        expected_snapshot_revision.checked_add(1).ok_or_else(|| {
                            anyhow::anyhow!("Clock schedule-disable generation exhausted")
                        })?;
                    for (occurrence_id, global_event_id) in ringing_occurrences {
                        let acknowledgement = ClockAcknowledgementV1 {
                            acknowledgement_id: schedule_disable_acknowledgement_id(
                                &occurrence_id,
                                request_id,
                            ),
                            global_event_id,
                            actor_node_id: command_origin.to_owned(),
                            actor_clock,
                            acknowledged_at_utc_ms: issued_at_ms,
                            stop: true,
                        };
                        anyhow::ensure!(
                            acknowledge(snapshot, &occurrence_id, acknowledgement)?,
                            "Clock schedule disable could not stop its ringing occurrence"
                        );
                        changed = true;
                    }
                    let occurrence_count = snapshot.occurrences.len();
                    snapshot.occurrences.retain(|occurrence| {
                        occurrence.schedule_id != schedule_id
                            || occurrence.phase != ClockOccurrencePhase::Scheduled
                    });
                    changed |= snapshot.occurrences.len() != occurrence_count;
                }
                changed
            }
            ClockCommandKindV1::Acknowledge {
                occurrence_id,
                acknowledgement,
            } => {
                anyhow::ensure!(
                    acknowledgement.actor_node_id == command_origin,
                    "Clock acknowledgement actor mismatch"
                );
                let schedule_id = snapshot
                    .occurrences
                    .iter()
                    .find(|value| value.occurrence_id == occurrence_id)
                    .map(|value| value.schedule_id.clone())
                    .ok_or_else(|| anyhow::anyhow!("Clock occurrence does not exist"))?;
                let is_alarm = snapshot
                    .schedules
                    .iter()
                    .find(|value| value.schedule_id == schedule_id)
                    .map(|schedule| matches!(schedule.schedule, ClockScheduleKindV1::Alarm(_)))
                    .ok_or_else(|| anyhow::anyhow!("Clock occurrence schedule does not exist"))?;
                if is_alarm {
                    acknowledge_alarm(
                        snapshot,
                        &occurrence_id,
                        acknowledgement,
                        expected_snapshot_revision,
                        issued_at_ms,
                        now_ms,
                        !peer_origin,
                        &self.disabled_schedule_ids,
                    )?
                } else {
                    acknowledge(snapshot, &occurrence_id, acknowledgement)?
                }
            }
            ClockCommandKindV1::UpsertStopwatch { stopwatch } => {
                validate_stopwatch_deadline(&stopwatch, now_ms)?;
                anyhow::ensure!(
                    stopwatch.origin_node_id == command_origin,
                    "Clock stopwatch origin mismatch"
                );
                if !peer_origin {
                    anyhow::ensure!(
                        clock_targets_are_admitted(
                            &stopwatch.mirror_target_ids,
                            &local_node_id,
                            &approved_peer_ids,
                        ),
                        "Clock stopwatch mirror is not an approved peer"
                    );
                }
                if peer_origin {
                    anyhow::ensure!(
                        stopwatch
                            .mirror_target_ids
                            .iter()
                            .any(|id| id == &self.node_id),
                        "Clock peer stopwatch does not target this node"
                    );
                }
                if let Some(existing) = snapshot
                    .stopwatches
                    .iter()
                    .find(|value| value.stopwatch_id == stopwatch.stopwatch_id)
                {
                    anyhow::ensure!(
                        existing.origin_node_id == stopwatch.origin_node_id,
                        "Clock stopwatch identity conflict"
                    );
                    if peer_origin {
                        if stopwatch.revision < existing.revision {
                            // The origin remains authoritative for a delivered
                            // mirror. It can legitimately repair a peer that
                            // retained a fabricated/newer conflicting payload,
                            // but only against the exact peer snapshot it
                            // observed while issuing the repair. Without this
                            // generation binding, a delayed lower-revision
                            // command could roll back a legitimate newer
                            // stopwatch after unrelated peer state advanced.
                            anyhow::ensure!(
                                expected_snapshot_revision == snapshot.revision,
                                "stale Clock peer stopwatch repair"
                            );
                        }
                        if stopwatch.revision == existing.revision {
                            anyhow::ensure!(
                                existing == &stopwatch,
                                "Clock peer stopwatch revision conflict"
                            );
                            return Ok(false);
                        }
                    }
                }
                let changed = snapshot
                    .stopwatches
                    .iter()
                    .find(|value| value.stopwatch_id == stopwatch.stopwatch_id)
                    != Some(&stopwatch);
                upsert_stopwatch(&mut snapshot.stopwatches, stopwatch);
                changed
            }
            ClockCommandKindV1::SetSettings { settings } => {
                if peer_origin {
                    anyhow::bail!("peer Clock settings mutation is not authoritative");
                }
                let changed = snapshot.settings != settings;
                snapshot.settings = settings;
                changed
            }
        };
        snapshot
            .schedules
            .sort_by(|a, b| a.schedule_id.cmp(&b.schedule_id));
        snapshot
            .stopwatches
            .sort_by(|a, b| a.stopwatch_id.cmp(&b.stopwatch_id));
        Ok(changed)
    }

    fn advance_deadlines(&mut self, now_ms: i64) -> anyhow::Result<bool> {
        let snapshot = self.snapshot.as_mut().expect("Clock snapshot loaded");
        let mut due = Vec::new();
        for schedule in &snapshot.schedules {
            let due_at = due_now_or_before(schedule, now_ms)?.filter(|due_at| {
                !matches!(
                    &schedule.schedule,
                    ClockScheduleKindV1::Alarm(mackes_mesh_types::clock::ClockAlarmV1 {
                        recurrence: ClockAlarmRecurrenceV1::Weekdays { .. },
                        ..
                    })
                ) || *due_at > snapshot.produced_at_utc_ms
            });
            if let Some(due_at) = due_at {
                let occurrence_id = format!("{}:{due_at}", schedule.schedule_id);
                if !snapshot
                    .occurrences
                    .iter()
                    .any(|value| value.occurrence_id == occurrence_id)
                {
                    due.push((schedule.schedule_id.clone(), due_at));
                }
            }
        }
        let scheduled_occurrences = snapshot
            .occurrences
            .iter()
            .filter(|occurrence| {
                occurrence.phase == ClockOccurrencePhase::Scheduled
                    && occurrence.due_at_utc_ms <= now_ms
            })
            .map(|occurrence| occurrence.occurrence_id.clone())
            .collect::<Vec<_>>();
        let mut changed = !due.is_empty() || !scheduled_occurrences.is_empty();
        for (schedule_id, due_at) in due {
            let Some(index) = snapshot
                .schedules
                .iter()
                .position(|value| value.schedule_id == schedule_id)
            else {
                continue;
            };
            let mut schedule = snapshot.schedules[index].clone();
            expire_schedule(&mut schedule, due_at);
            add_occurrence(
                snapshot,
                &schedule,
                due_at,
                ClockOccurrencePhase::Ringing,
                &self.node_id,
                &self.disabled_schedule_ids,
            )?;
            snapshot.schedules[index] = schedule;
        }
        for occurrence_id in scheduled_occurrences {
            let occurrence = snapshot
                .occurrences
                .iter_mut()
                .find(|value| value.occurrence_id == occurrence_id)
                .expect("scheduled Clock occurrence was collected from this snapshot");
            occurrence.phase = ClockOccurrencePhase::Ringing;
            for target in &mut occurrence.targets {
                if target.disposition == ClockTargetDisposition::Pending {
                    target.disposition = ClockTargetDisposition::Ringing;
                }
            }
        }
        let alarm_schedule_ids = snapshot
            .schedules
            .iter()
            .filter(|schedule| matches!(schedule.schedule, ClockScheduleKindV1::Alarm(_)))
            .map(|schedule| schedule.schedule_id.as_str())
            .collect::<BTreeSet<_>>();
        let auto_silence_ms = i64::from(snapshot.settings.auto_silence_minutes)
            .checked_mul(60_000)
            .ok_or_else(|| anyhow::anyhow!("Clock auto-silence duration overflow"))?;
        for occurrence in &mut snapshot.occurrences {
            if occurrence.phase != ClockOccurrencePhase::Ringing
                || !alarm_schedule_ids.contains(occurrence.schedule_id.as_str())
                || occurrence
                    .due_at_utc_ms
                    .checked_add(auto_silence_ms)
                    .is_none_or(|deadline| deadline > now_ms)
            {
                continue;
            }
            occurrence.phase = ClockOccurrencePhase::Missed;
            for target in &mut occurrence.targets {
                if target.disposition == ClockTargetDisposition::Ringing {
                    target.disposition = ClockTargetDisposition::Missed;
                }
            }
            changed = true;
        }
        Ok(changed)
    }

    #[cfg(test)]
    fn tick_once(&mut self) -> anyhow::Result<()> {
        self.ensure_loaded()?;
        let transaction = self.open_bus_transaction()?;
        if self.active_bus_identity != Some(transaction.identity) {
            self.activate_bus(&transaction)?;
        }
        self.tick_with_transaction(&transaction)
    }

    fn tick_with_transaction(&mut self, transaction: &ClockBusTransaction) -> anyhow::Result<()> {
        self.ensure_loaded()?;
        transaction.verify_current()?;
        let now_ms = self.clock.now_ms();
        // Complete every Bus source read against one verified index generation
        // before acknowledging audio, advancing deadlines, applying commands,
        // or publishing convergence.
        let actions = self.collect_actions(transaction)?;
        let audio_statuses = self.collect_audio_status(transaction)?;
        let peer_snapshots = self.collect_peer_snapshots(transaction, now_ms)?;
        transaction.verify_current()?;

        let audio_checkpoint = self.checkpoint();
        if let Err(error) = self.consume_audio_status(transaction, audio_statuses, now_ms) {
            self.restore_checkpoint(audio_checkpoint);
            return Err(error);
        }

        let deadline_checkpoint = self.checkpoint();
        let deadline_result = (|| {
            let prior_snapshot = self
                .snapshot
                .as_ref()
                .expect("Clock snapshot loaded")
                .clone();
            if self.advance_deadlines(now_ms)? {
                let expected = self
                    .snapshot
                    .as_ref()
                    .expect("Clock snapshot loaded")
                    .revision;
                let snapshot = self.snapshot.as_mut().expect("Clock snapshot loaded");
                snapshot.revision = expected
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!("Clock revision exhausted"))?;
                snapshot.produced_at_utc_ms = now_ms;
                stamp_revision(snapshot, &self.node_id);
                preserve_unchanged_occurrence_revisions(snapshot, &prior_snapshot);
                let audio_requests =
                    clock_audio_transitions(&prior_snapshot, snapshot, now_ms, &self.node_id)?;
                self.commit_then_publish(transaction, expected, None, None, &audio_requests)?;
            }
            Ok::<(), anyhow::Error>(())
        })();
        if let Err(error) = deadline_result {
            self.restore_checkpoint(deadline_checkpoint);
            // Deadline advancement has no command request id to replay. If the
            // atomic authority/outbox commit won and only publication failed,
            // adopt that durable winner so the next sweep repairs publication
            // instead of retrying forever from a stale expected revision.
            self.reload_durable_authority(now_ms)
                .context("reloading Clock authority after deadline failure")?;
            return Err(error);
        }

        for (cursor, body) in actions {
            let checkpoint = self.checkpoint();
            advance_clock_cursor(&mut self.action_cursor, &cursor);
            if let Err(error) = self.process_command(transaction, body.as_bytes(), now_ms) {
                self.restore_checkpoint(checkpoint);
                // The command and its replay identity may already be durable
                // even when the required state publication failed. The Bus
                // generation can be replaced before that command is replayed,
                // so do not depend on the transient row to recover our memory.
                // Adopting the SQLite winner also makes the next generation
                // publish corrected-forward authority without reapplying the
                // command or duplicating its effects.
                self.reload_durable_authority(now_ms)
                    .context("reloading Clock authority after command failure")?;
                return Err(error);
            }
        }
        if !self.published_once {
            self.publish(transaction)?;
        }
        self.publish_peer_convergence(transaction, now_ms, &peer_snapshots)?;
        self.publish_pending_audio(transaction, now_ms)?;
        transaction.verify_current()
    }

    fn collect_audio_status(
        &self,
        transaction: &ClockBusTransaction,
    ) -> anyhow::Result<Vec<mde_bus::persist::StoredMessage>> {
        let topic = clock_audio_status_topic(&self.node_id)?;
        let messages = transaction.persist.list_since_limit(
            &topic,
            self.audio_status_cursor.as_deref(),
            MAX_AUDIO_STATUS_PER_TICK,
        )?;
        transaction.verify_current()?;
        Ok(messages)
    }

    fn consume_audio_status(
        &mut self,
        transaction: &ClockBusTransaction,
        messages: Vec<mde_bus::persist::StoredMessage>,
        now_ms: i64,
    ) -> anyhow::Result<()> {
        for message in messages {
            let cursor = message.ulid;
            let Some(body) = message.body else {
                advance_clock_cursor(&mut self.audio_status_cursor, &cursor);
                continue;
            };
            if mackes_mesh_types::workloads::reject_duplicate_json_keys(&body).is_err() {
                advance_clock_cursor(&mut self.audio_status_cursor, &cursor);
                continue;
            }
            let Ok(status) = serde_json::from_str::<ClockAudioStatusV1>(&body) else {
                advance_clock_cursor(&mut self.audio_status_cursor, &cursor);
                continue;
            };
            if status.validate_at(now_ms).is_ok() {
                transaction.verify_current()?;
                let changed = self.store.acknowledge_audio(&self.node_id, &status)?;
                transaction.verify_current()?;
                if changed {
                    self.audio_last_sent_ms.remove(&status.request_id);
                }
            }
            // A durable acknowledgement failure returns before this cursor
            // advances, so the same status is retried by this worker.
            advance_clock_cursor(&mut self.audio_status_cursor, &cursor);
        }
        Ok(())
    }

    fn publish_pending_audio(
        &mut self,
        transaction: &ClockBusTransaction,
        now_ms: i64,
    ) -> anyhow::Result<()> {
        let Some(seed) = self.music_signing_seed else {
            return Ok(());
        };
        for pending in self.store.pending_audio(&self.node_id)? {
            // The outbox is durable, but the Bus generation is not. Re-check
            // before spending work on a signed publication and again after
            // signing so a replaced index cannot receive a stale request.
            transaction.verify_current()?;
            if self
                .audio_last_sent_ms
                .get(&pending.request_id)
                .is_some_and(|sent| now_ms.saturating_sub(*sent) < AUDIO_RETRY_MS)
            {
                continue;
            }
            mackes_mesh_types::workloads::reject_duplicate_json_keys(&pending.request_json)?;
            let mut request: ClockAudioRequestV1 = serde_json::from_str(&pending.request_json)?;
            anyhow::ensure!(
                request.request_id == pending.request_id
                    && request.occurrence_id == pending.occurrence_id
                    && request.global_event_id == pending.global_event_id
                    && request.occurrence_generation == pending.occurrence_generation
                    && request.music_auth.is_none(),
                "Clock audio outbox identity mismatch"
            );
            request.issued_at_utc_ms = now_ms;
            request.expires_at_utc_ms = now_ms.saturating_add(MAX_CLOCK_AUDIO_REQUEST_TTL_MS);
            request.validate_at(now_ms)?;
            let unsigned = serde_json::to_string(&request)?;
            let mut nonce_bytes = [0_u8; 32];
            rand::rngs::OsRng.fill_bytes(&mut nonce_bytes);
            let signed = music_auth::sign_request(
                &unsigned,
                MusicAuthContext {
                    verb: CLOCK_AUDIO_AUTH_VERB,
                    node: &self.node_id,
                    target: CLOCK_AUDIO_AUTH_TARGET,
                },
                &seed,
                &hex_bytes(&nonce_bytes),
                request.expires_at_utc_ms,
            )
            .map_err(anyhow::Error::msg)?;
            ClockAudioRequestV1::from_json_at(signed.as_bytes(), now_ms)?;
            transaction.verify_current()?;
            transaction.write(CLOCK_AUDIO_ACTION_TOPIC, &signed)?;
            transaction.verify_current()?;
            self.audio_last_sent_ms.insert(pending.request_id, now_ms);
        }
        Ok(())
    }

    fn collect_peer_snapshots(
        &self,
        transaction: &ClockBusTransaction,
        now_ms: i64,
    ) -> anyhow::Result<BTreeMap<String, ClockSnapshotV1>> {
        let mut peer_snapshots = BTreeMap::new();
        for peer_id in &self.approved_peer_ids {
            if peer_id != &self.node_id {
                if let Some(peer) =
                    read_peer_snapshot(&transaction.persist, peer_id, &self.context(now_ms))?
                {
                    peer_snapshots.insert(peer_id.clone(), peer);
                }
            }
        }
        transaction.verify_current()?;
        Ok(peer_snapshots)
    }

    fn publish_peer_convergence(
        &mut self,
        transaction: &ClockBusTransaction,
        now_ms: i64,
        peer_snapshots: &BTreeMap<String, ClockSnapshotV1>,
    ) -> anyhow::Result<()> {
        if self.command_signing_key.is_none() || self.signer.is_none() {
            return Ok(());
        }
        let snapshot = self
            .snapshot
            .as_ref()
            .expect("Clock snapshot loaded")
            .clone();
        let local_node_id = self.node_id.clone();
        let approved_peer_ids = self.approved_peer_ids.clone();
        let mut remaining_commands = MAX_PEER_COMMANDS_PER_TICK;
        let mut remaining_probes = MAX_PEER_CONVERGENCE_PROBES_PER_TICK;

        for schedule in snapshot
            .schedules
            .iter()
            .filter(|schedule| schedule.origin_node_id == local_node_id)
        {
            for target in schedule
                .selected_target_ids
                .iter()
                .filter(|target| *target != &local_node_id && approved_peer_ids.contains(*target))
            {
                if remaining_probes == 0 {
                    return Ok(());
                }
                remaining_probes -= 1;
                let Some(peer) = peer_snapshots.get(target) else {
                    continue;
                };
                // Revision alone is not proof of convergence. A peer can
                // retain a conflicting payload under the same (or a newer)
                // revision after a torn/replayed command. Treat that state as
                // divergent so it remains visible to the admission boundary
                // instead of silently declaring the alarm delivered.
                let delivered = peer
                    .schedules
                    .iter()
                    .any(|candidate| peer_schedule_is_converged(candidate, schedule));
                if !delivered {
                    if remaining_commands == 0 {
                        return Ok(());
                    }
                    let request_id = peer_request_id(
                        "schedule",
                        target,
                        &schedule.schedule_id,
                        schedule.revision,
                        "",
                    );
                    self.publish_peer_command(
                        transaction,
                        target,
                        peer.revision,
                        request_id,
                        &schedule.origin_node_id,
                        ClockCommandKindV1::UpsertSchedule {
                            schedule: schedule.clone(),
                        },
                        now_ms,
                        &mut remaining_commands,
                    )?;
                }
            }
        }

        for stopwatch in snapshot
            .stopwatches
            .iter()
            .filter(|stopwatch| stopwatch.origin_node_id == local_node_id)
        {
            for target in stopwatch
                .mirror_target_ids
                .iter()
                .filter(|target| *target != &local_node_id && approved_peer_ids.contains(*target))
            {
                if remaining_probes == 0 {
                    return Ok(());
                }
                remaining_probes -= 1;
                let Some(peer) = peer_snapshots.get(target) else {
                    continue;
                };
                // Revision alone is not proof that the peer retained the
                // origin's stopwatch payload. A torn or replayed delivery can
                // leave a newer conflicting generation that would otherwise
                // suppress every future repair command.
                let delivered = peer
                    .stopwatches
                    .iter()
                    .any(|candidate| peer_stopwatch_is_converged(candidate, stopwatch));
                if !delivered {
                    if remaining_commands == 0 {
                        return Ok(());
                    }
                    let request_id = peer_request_id(
                        "stopwatch",
                        target,
                        &stopwatch.stopwatch_id,
                        stopwatch.revision,
                        "",
                    );
                    self.publish_peer_command(
                        transaction,
                        target,
                        peer.revision,
                        request_id,
                        &stopwatch.origin_node_id,
                        ClockCommandKindV1::UpsertStopwatch {
                            stopwatch: stopwatch.clone(),
                        },
                        now_ms,
                        &mut remaining_commands,
                    )?;
                }
            }
        }

        for occurrence in snapshot
            .occurrences
            .iter()
            .filter(|occurrence| occurrence.acknowledgement.is_some())
        {
            let Some(schedule) = snapshot
                .schedules
                .iter()
                .find(|schedule| schedule.schedule_id == occurrence.schedule_id)
            else {
                continue;
            };
            let acknowledgement = occurrence
                .acknowledgement
                .as_ref()
                .expect("filtered acknowledgement");
            for target in schedule
                .selected_target_ids
                .iter()
                .filter(|target| *target != &local_node_id && approved_peer_ids.contains(*target))
            {
                if remaining_probes == 0 {
                    return Ok(());
                }
                remaining_probes -= 1;
                let Some(peer) = peer_snapshots.get(target) else {
                    continue;
                };
                let converged = peer
                    .occurrences
                    .iter()
                    .find(|candidate| candidate.global_event_id == occurrence.global_event_id)
                    .and_then(|candidate| candidate.acknowledgement.as_ref())
                    .is_some_and(|current| {
                        peer_acknowledgement_is_converged(current, acknowledgement)
                    });
                if converged {
                    continue;
                }
                if remaining_commands == 0 {
                    return Ok(());
                }
                let request_id = peer_request_id(
                    "ack",
                    target,
                    &occurrence.global_event_id,
                    acknowledgement.actor_clock,
                    &acknowledgement.acknowledgement_id,
                );
                self.publish_peer_command(
                    transaction,
                    target,
                    peer.revision,
                    request_id,
                    &acknowledgement.actor_node_id,
                    ClockCommandKindV1::Acknowledge {
                        occurrence_id: occurrence.occurrence_id.clone(),
                        acknowledgement: acknowledgement.clone(),
                    },
                    now_ms,
                    &mut remaining_commands,
                )?;
            }
        }
        Ok(())
    }

    fn publish_peer_command(
        &mut self,
        transaction: &ClockBusTransaction,
        target: &str,
        expected_revision: u64,
        request_id: String,
        command_origin: &str,
        body: ClockCommandKindV1,
        now_ms: i64,
        remaining_commands: &mut usize,
    ) -> anyhow::Result<()> {
        if *remaining_commands == 0 {
            return Ok(());
        }
        if self
            .peer_last_sent_ms
            .get(&request_id)
            .is_some_and(|sent| now_ms.saturating_sub(*sent) < PEER_RETRY_MS)
        {
            return Ok(());
        }
        let signer = self.signer.as_ref().expect("checked Clock signer");
        let signing_key = self
            .command_signing_key
            .as_ref()
            .expect("checked Clock signing key");
        let command = ClockCommandV1 {
            schema_version: CLOCK_SCHEMA_VERSION,
            request_id: request_id.clone(),
            origin_node_id: command_origin.to_owned(),
            expected_revision,
            issued_at_utc_ms: now_ms,
            expires_at_utc_ms: now_ms.saturating_add(MAX_CLOCK_COMMAND_TTL_MS),
            body,
            signer_id: String::new(),
            signature: String::new(),
        }
        .sign(signer.signer_id.clone(), signing_key, &self.context(now_ms))?;
        transaction.write(
            &clock_command_topic(target)?,
            &serde_json::to_string(&command)?,
        )?;
        self.peer_last_sent_ms.insert(request_id, now_ms);
        *remaining_commands -= 1;
        Ok(())
    }
}

#[async_trait::async_trait]
impl Worker for ClockWorker {
    fn name(&self) -> &'static str {
        "clock"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        // Durable Clock state is independent of Bus availability and must be
        // restored before waiting for a late spool.
        self.ensure_loaded()?;
        let bus_root = self.bus_root();
        let mut retry_interval = MIN_BUS_RETRY_INTERVAL;
        let mut tick = tokio::time::interval(self.poll);
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    match self.open_bus_transaction() {
                        Ok(transaction) => {
                            retry_interval = MIN_BUS_RETRY_INTERVAL;
                            let result = if self.active_bus_identity != Some(transaction.identity) {
                                self.activate_bus(&transaction)
                                    .and_then(|()| self.tick_with_transaction(&transaction))
                            } else {
                                self.tick_with_transaction(&transaction)
                            };
                            if let Err(error) = result {
                                tracing::warn!(target: "mackesd::clock", %error, "Clock sweep deferred");
                            }
                        }
                        Err(error) => {
                            tracing::warn!(
                                target: "mackesd::clock",
                                %error,
                                bus_root = %bus_root.display(),
                                "Clock Bus unavailable; sweep will retry"
                            );
                            let delay = retry_interval;
                            retry_interval = next_bus_retry_interval(retry_interval);
                            tokio::select! {
                                () = shutdown.wait() => break,
                                () = tokio::time::sleep(delay) => {}
                            }
                        }
                    }
                },
                () = shutdown.wait() => break,
            }
        }
        Ok(())
    }
}

fn clock_bus_root(override_root: Option<PathBuf>) -> PathBuf {
    clock_bus_root_or_system(override_root.or_else(crate::bus_publish::default_bus_root))
}

fn advance_clock_cursor(cursor: &mut Option<String>, candidate: &str) {
    if cursor.as_deref().is_none_or(|current| candidate > current) {
        *cursor = Some(candidate.to_owned());
    }
}

fn clock_bus_root_or_system(resolved: Option<PathBuf>) -> PathBuf {
    resolved.unwrap_or_else(|| PathBuf::from(mde_bus::SYSTEM_BUS_ROOT))
}

fn clock_bus_identity(root: &Path) -> io::Result<ClockBusIdentity> {
    let metadata = fs::metadata(root.join("index.sqlite"))?;
    if !metadata.is_file() {
        return Err(io::Error::other("Clock Bus index is not a regular file"));
    }
    #[cfg(unix)]
    {
        Ok(ClockBusIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        Ok(ClockBusIdentity {})
    }
}

fn next_bus_retry_interval(current: Duration) -> Duration {
    current
        .saturating_mul(2)
        .clamp(MIN_BUS_RETRY_INTERVAL, MAX_BUS_RETRY_INTERVAL)
}

fn read_peer_snapshot(
    persist: &Persist,
    peer_id: &str,
    context: &ClockValidationContext<'_>,
) -> anyhow::Result<Option<ClockSnapshotV1>> {
    let Some(message) = persist.read_latest(&clock_state_topic(peer_id)?)? else {
        return Ok(None);
    };
    let Some(body) = message.body else {
        return Ok(None);
    };
    if mackes_mesh_types::workloads::reject_duplicate_json_keys(&body).is_err() {
        return Ok(None);
    }
    let Ok(snapshot) = ClockSnapshotV1::from_persisted_json_at(body.as_bytes(), context) else {
        return Ok(None);
    };
    Ok((snapshot.node_id == peer_id).then_some(snapshot))
}

fn peer_request_id(
    kind: &str,
    target: &str,
    identity: &str,
    generation: u64,
    suffix: &str,
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"magic-mesh:clock-peer-command:v1\0");
    for value in [kind, target, identity, suffix] {
        digest.update(value.as_bytes());
        digest.update([0]);
    }
    digest.update(generation.to_be_bytes());
    format!("clock-peer-{kind}-{}", &hex_bytes(&digest.finalize())[..32])
}

fn due_now_or_before(schedule: &ClockScheduleV1, now_ms: i64) -> anyhow::Result<Option<i64>> {
    match &schedule.schedule {
        ClockScheduleKindV1::Alarm(alarm) if alarm.enabled => match alarm.recurrence {
            ClockAlarmRecurrenceV1::OneTime { due_at_utc_ms } if due_at_utc_ms <= now_ms => {
                Ok(Some(due_at_utc_ms))
            }
            ClockAlarmRecurrenceV1::Weekdays {
                ref local_time,
                ref weekdays,
            } => weekday_due_now_or_before(local_time, weekdays, now_ms),
            _ => Ok(None),
        },
        ClockScheduleKindV1::Timer(timer) if timer.phase == ClockTimerPhase::Running => timer
            .absolute_deadline_utc_ms
            .filter(|deadline| *deadline <= now_ms)
            .map_or(Ok(None), |deadline| Ok(Some(deadline))),
        _ => Ok(None),
    }
}

fn peer_schedule_is_converged(candidate: &ClockScheduleV1, desired: &ClockScheduleV1) -> bool {
    candidate == desired
}

fn peer_stopwatch_is_converged(candidate: &ClockStopwatchV1, desired: &ClockStopwatchV1) -> bool {
    candidate == desired
}

fn peer_acknowledgement_is_converged(
    current: &ClockAcknowledgementV1,
    desired: &ClockAcknowledgementV1,
) -> bool {
    current == desired || !acknowledgement_wins(desired, current)
}

fn weekday_due_now_or_before(
    local_time: &mackes_mesh_types::clock::ClockCivilTimeV1,
    weekdays: &[ClockWeekday],
    now_ms: i64,
) -> anyhow::Result<Option<i64>> {
    let time_zone = TimeZone::get(&local_time.time_zone)
        .with_context(|| format!("Clock IANA zone {} is unavailable", local_time.time_zone))?;
    let now = Timestamp::from_millisecond(now_ms)
        .context("Clock wall time is outside Jiff's supported range")?
        .to_zoned(time_zone.clone());

    for days_ago in 0_i64..7 {
        let date = now
            .date()
            .checked_sub(days_ago.days())
            .context("Clock weekday date arithmetic overflowed")?;
        if !weekdays.contains(&clock_weekday(date.weekday())) {
            continue;
        }
        let hour = i8::try_from(local_time.hour).context("Clock civil hour is out of range")?;
        let minute =
            i8::try_from(local_time.minute).context("Clock civil minute is out of range")?;
        let second =
            i8::try_from(local_time.second).context("Clock civil second is out of range")?;
        let civil = date.at(hour, minute, second, 0);
        let ambiguous = time_zone.to_ambiguous_zoned(civil);
        let zoned = match local_time.fold {
            // Compatible chooses the earlier instant in a fold and the next
            // valid instant after a gap, exactly matching the Clock contract.
            ClockFoldPolicy::Earlier => ambiguous.compatible(),
            ClockFoldPolicy::Later => ambiguous.later(),
        }
        .context("Clock civil alarm could not be resolved in its IANA zone")?;
        let due_at = zoned.timestamp().as_millisecond();
        if due_at <= now_ms {
            return Ok(Some(due_at));
        }
    }
    Ok(None)
}

const fn clock_weekday(weekday: JiffWeekday) -> ClockWeekday {
    match weekday {
        JiffWeekday::Monday => ClockWeekday::Monday,
        JiffWeekday::Tuesday => ClockWeekday::Tuesday,
        JiffWeekday::Wednesday => ClockWeekday::Wednesday,
        JiffWeekday::Thursday => ClockWeekday::Thursday,
        JiffWeekday::Friday => ClockWeekday::Friday,
        JiffWeekday::Saturday => ClockWeekday::Saturday,
        JiffWeekday::Sunday => ClockWeekday::Sunday,
    }
}

fn expire_schedule(schedule: &mut ClockScheduleV1, due_at: i64) {
    match &mut schedule.schedule {
        ClockScheduleKindV1::Alarm(alarm) => {
            if matches!(alarm.recurrence, ClockAlarmRecurrenceV1::OneTime { .. }) {
                alarm.enabled = false;
            }
        }
        ClockScheduleKindV1::Timer(timer) => {
            timer.phase = ClockTimerPhase::Expired;
            timer.absolute_deadline_utc_ms = Some(due_at);
            timer.paused_remaining_ms = None;
            timer.expired_at_utc_ms = Some(due_at);
        }
    }
}

fn add_occurrence(
    snapshot: &mut ClockSnapshotV1,
    schedule: &ClockScheduleV1,
    due_at: i64,
    phase: ClockOccurrencePhase,
    local_node_id: &str,
    disabled_schedule_ids: &BTreeSet<String>,
) -> anyhow::Result<()> {
    let occurrence_id = format!("{}:{due_at}", schedule.schedule_id);
    if snapshot
        .occurrences
        .iter()
        .any(|value| value.occurrence_id == occurrence_id)
    {
        return Ok(());
    }
    anyhow::ensure!(
        snapshot.occurrences.len() < MAX_CLOCK_OCCURRENCES,
        "Clock occurrence capacity exhausted"
    );
    let disposition = match phase {
        ClockOccurrencePhase::Ringing => ClockTargetDisposition::Ringing,
        ClockOccurrencePhase::Missed => ClockTargetDisposition::Missed,
        _ => ClockTargetDisposition::Pending,
    };
    snapshot.occurrences.push(ClockOccurrenceV1 {
        occurrence_id: occurrence_id.clone(),
        global_event_id: occurrence_id,
        schedule_id: schedule.schedule_id.clone(),
        revision: snapshot.revision,
        due_at_utc_ms: due_at,
        phase,
        targets: schedule
            .selected_target_ids
            .iter()
            .map(|target_node_id| ClockTargetState {
                target_node_id: target_node_id.clone(),
                disposition: if target_node_id == local_node_id
                    && disabled_schedule_ids.contains(&schedule.schedule_id)
                {
                    ClockTargetDisposition::DisabledLocally
                } else {
                    disposition
                },
                revision: snapshot.revision,
                observed_at_utc_ms: snapshot.produced_at_utc_ms,
            })
            .collect(),
        acknowledgement: None,
    });
    Ok(())
}

fn exact_timer_extension_acknowledgement(
    snapshot: &ClockSnapshotV1,
    existing: &ClockScheduleV1,
    candidate: &ClockScheduleV1,
    actor_node_id: &str,
    request_id: &str,
    expected_snapshot_revision: u64,
    issued_at_ms: i64,
) -> anyhow::Result<(String, ClockAcknowledgementV1)> {
    anyhow::ensure!(
        snapshot.revision == expected_snapshot_revision,
        "Clock timer extension snapshot generation is stale"
    );
    anyhow::ensure!(
        existing.schedule_id == candidate.schedule_id
            && existing.origin_node_id == candidate.origin_node_id
            && existing.label == candidate.label
            && existing.selected_target_ids == candidate.selected_target_ids,
        "Clock timer extension changed immutable schedule identity"
    );
    let ClockScheduleKindV1::Timer(existing_timer) = &existing.schedule else {
        anyhow::bail!("Clock generation reuse is not a timer extension");
    };
    let ClockScheduleKindV1::Timer(candidate_timer) = &candidate.schedule else {
        anyhow::bail!("Clock timer extension changed schedule kind");
    };
    let expired_at_ms = existing_timer
        .expired_at_utc_ms
        .filter(|_| {
            existing_timer.phase == ClockTimerPhase::Expired
                && existing_timer.absolute_deadline_utc_ms == existing_timer.expired_at_utc_ms
                && existing_timer.paused_remaining_ms.is_none()
        })
        .ok_or_else(|| anyhow::anyhow!("Clock timer is not exactly expired"))?;
    anyhow::ensure!(
        candidate_timer.phase == ClockTimerPhase::Running
            && candidate_timer.absolute_deadline_utc_ms == issued_at_ms.checked_add(60_000)
            && candidate_timer.paused_remaining_ms.is_none()
            && candidate_timer.expired_at_utc_ms.is_none()
            && candidate_timer.original_duration_ms == existing_timer.original_duration_ms
            && candidate_timer.sound == existing_timer.sound
            && candidate_timer.vibrate == existing_timer.vibrate,
        "Clock timer extension is not the exact signed one-minute transition"
    );

    let mut matches = snapshot.occurrences.iter().filter(|occurrence| {
        occurrence.schedule_id == existing.schedule_id
            && occurrence.due_at_utc_ms == expired_at_ms
            && occurrence.phase == ClockOccurrencePhase::Ringing
            && occurrence.revision == expected_snapshot_revision
            && occurrence.acknowledgement.is_none()
    });
    let occurrence = matches
        .next()
        .ok_or_else(|| anyhow::anyhow!("Clock timer extension has no exact ringing occurrence"))?;
    anyhow::ensure!(
        matches.next().is_none(),
        "Clock timer extension occurrence identity is ambiguous"
    );
    let actor_clock = expected_snapshot_revision
        .checked_add(1)
        .ok_or_else(|| anyhow::anyhow!("Clock acknowledgement generation exhausted"))?;
    Ok((
        occurrence.occurrence_id.clone(),
        ClockAcknowledgementV1 {
            acknowledgement_id: request_id.to_owned(),
            global_event_id: occurrence.global_event_id.clone(),
            actor_node_id: actor_node_id.to_owned(),
            actor_clock,
            acknowledged_at_utc_ms: issued_at_ms,
            stop: true,
        },
    ))
}

#[allow(clippy::too_many_arguments)]
fn acknowledge_alarm(
    snapshot: &mut ClockSnapshotV1,
    occurrence_id: &str,
    acknowledgement: ClockAcknowledgementV1,
    expected_snapshot_revision: u64,
    issued_at_ms: i64,
    now_ms: i64,
    require_exact_snapshot: bool,
    disabled_schedule_ids: &BTreeSet<String>,
) -> anyhow::Result<bool> {
    let source = snapshot
        .occurrences
        .iter()
        .find(|value| value.occurrence_id == occurrence_id)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Clock occurrence does not exist"))?;
    anyhow::ensure!(
        acknowledgement.global_event_id == source.global_event_id,
        "Clock alarm acknowledgement event mismatch"
    );
    let schedule = snapshot
        .schedules
        .iter()
        .find(|value| value.schedule_id == source.schedule_id)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Clock alarm schedule does not exist"))?;
    anyhow::ensure!(
        matches!(schedule.schedule, ClockScheduleKindV1::Alarm(_)),
        "Clock alarm acknowledgement changed schedule kind"
    );
    if require_exact_snapshot {
        anyhow::ensure!(
            snapshot.revision == expected_snapshot_revision
                && source.revision == expected_snapshot_revision
                && source.phase == ClockOccurrencePhase::Ringing
                && source.acknowledgement.is_none(),
            "Clock alarm acknowledgement generation is stale"
        );
        anyhow::ensure!(
            acknowledgement.actor_clock
                == expected_snapshot_revision
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!("Clock alarm generation exhausted"))?
                && acknowledgement.acknowledged_at_utc_ms == issued_at_ms,
            "Clock alarm acknowledgement is not bound to its signed snapshot"
        );
    } else {
        anyhow::ensure!(
            matches!(
                source.phase,
                ClockOccurrencePhase::Ringing
                    | ClockOccurrencePhase::Snoozed
                    | ClockOccurrencePhase::Stopped
            ),
            "Clock peer alarm acknowledgement is stale"
        );
    }
    if source
        .acknowledgement
        .as_ref()
        .is_some_and(|current| !acknowledgement_wins(&acknowledgement, current))
    {
        return Ok(false);
    }

    if let Some(previous) = source
        .acknowledgement
        .as_ref()
        .filter(|previous| !previous.stop)
    {
        retire_snooze_child(snapshot, &source, previous, &acknowledgement)?;
    }
    anyhow::ensure!(
        acknowledge(snapshot, occurrence_id, acknowledgement.clone())?,
        "Clock alarm acknowledgement lost its exact occurrence"
    );
    if !acknowledgement.stop {
        let snooze_ms = i64::from(snapshot.settings.snooze_minutes)
            .checked_mul(60_000)
            .ok_or_else(|| anyhow::anyhow!("Clock snooze duration overflow"))?;
        let due_at_ms = acknowledgement
            .acknowledged_at_utc_ms
            .checked_add(snooze_ms)
            .ok_or_else(|| anyhow::anyhow!("Clock snooze deadline overflow"))?;
        anyhow::ensure!(due_at_ms > now_ms, "Clock snooze deadline is already stale");
        add_snooze_occurrence(
            snapshot,
            &source,
            &schedule,
            &acknowledgement,
            due_at_ms,
            disabled_schedule_ids,
        )?;
    }
    Ok(true)
}

fn snooze_occurrence_id(
    source: &ClockOccurrenceV1,
    acknowledgement: &ClockAcknowledgementV1,
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"magic-mesh:clock-snooze-occurrence:v1\0");
    for value in [
        source.occurrence_id.as_str(),
        source.global_event_id.as_str(),
        acknowledgement.acknowledgement_id.as_str(),
        acknowledgement.actor_node_id.as_str(),
    ] {
        digest.update(value.as_bytes());
        digest.update([0]);
    }
    digest.update(acknowledgement.actor_clock.to_be_bytes());
    digest.update(acknowledgement.acknowledged_at_utc_ms.to_be_bytes());
    format!("clock-snooze-{}", &hex_bytes(&digest.finalize())[..32])
}

fn add_snooze_occurrence(
    snapshot: &mut ClockSnapshotV1,
    source: &ClockOccurrenceV1,
    schedule: &ClockScheduleV1,
    acknowledgement: &ClockAcknowledgementV1,
    due_at_ms: i64,
    disabled_schedule_ids: &BTreeSet<String>,
) -> anyhow::Result<()> {
    let occurrence_id = snooze_occurrence_id(source, acknowledgement);
    if snapshot
        .occurrences
        .iter()
        .any(|value| value.occurrence_id == occurrence_id)
    {
        return Ok(());
    }
    anyhow::ensure!(
        snapshot.occurrences.len() < MAX_CLOCK_OCCURRENCES,
        "Clock snooze occurrence capacity exhausted"
    );
    let local_node_id = snapshot.node_id.clone();
    snapshot.occurrences.push(ClockOccurrenceV1 {
        occurrence_id: occurrence_id.clone(),
        global_event_id: occurrence_id,
        schedule_id: schedule.schedule_id.clone(),
        revision: snapshot.revision,
        due_at_utc_ms: due_at_ms,
        phase: ClockOccurrencePhase::Scheduled,
        targets: schedule
            .selected_target_ids
            .iter()
            .map(|target_node_id| ClockTargetState {
                target_node_id: target_node_id.clone(),
                disposition: if target_node_id == &local_node_id
                    && disabled_schedule_ids.contains(&schedule.schedule_id)
                {
                    ClockTargetDisposition::DisabledLocally
                } else {
                    ClockTargetDisposition::Pending
                },
                revision: snapshot.revision,
                observed_at_utc_ms: snapshot.produced_at_utc_ms,
            })
            .collect(),
        acknowledgement: None,
    });
    Ok(())
}

fn retire_snooze_child(
    snapshot: &mut ClockSnapshotV1,
    source: &ClockOccurrenceV1,
    previous: &ClockAcknowledgementV1,
    winner: &ClockAcknowledgementV1,
) -> anyhow::Result<()> {
    let child_id = snooze_occurrence_id(source, previous);
    let Some(index) = snapshot
        .occurrences
        .iter()
        .position(|value| value.occurrence_id == child_id)
    else {
        return Ok(());
    };
    if snapshot.occurrences[index].phase == ClockOccurrencePhase::Scheduled {
        snapshot.occurrences.remove(index);
        return Ok(());
    }
    if snapshot.occurrences[index].phase == ClockOccurrencePhase::Ringing {
        let child_global_event_id = snapshot.occurrences[index].global_event_id.clone();
        let child_acknowledgement = ClockAcknowledgementV1 {
            acknowledgement_id: winner.acknowledgement_id.clone(),
            global_event_id: child_global_event_id,
            actor_node_id: winner.actor_node_id.clone(),
            actor_clock: winner.actor_clock,
            acknowledged_at_utc_ms: winner.acknowledged_at_utc_ms,
            stop: true,
        };
        anyhow::ensure!(
            acknowledge(snapshot, &child_id, child_acknowledgement)?,
            "Clock alarm winner could not retire a ringing snooze"
        );
    }
    Ok(())
}

fn acknowledge(
    snapshot: &mut ClockSnapshotV1,
    occurrence_id: &str,
    acknowledgement: ClockAcknowledgementV1,
) -> anyhow::Result<bool> {
    let occurrence = snapshot
        .occurrences
        .iter_mut()
        .find(|value| value.occurrence_id == occurrence_id)
        .ok_or_else(|| anyhow::anyhow!("Clock occurrence does not exist"))?;
    anyhow::ensure!(
        acknowledgement.global_event_id == occurrence.global_event_id,
        "Clock acknowledgement event mismatch"
    );
    if occurrence
        .acknowledgement
        .as_ref()
        .is_some_and(|current| !acknowledgement_wins(&acknowledgement, current))
    {
        return Ok(false);
    }
    occurrence.phase = if acknowledgement.stop {
        ClockOccurrencePhase::Stopped
    } else {
        ClockOccurrencePhase::Snoozed
    };
    for target in &mut occurrence.targets {
        target.disposition = if acknowledgement.stop {
            ClockTargetDisposition::Stopped
        } else {
            ClockTargetDisposition::Snoozed
        };
    }
    occurrence.acknowledgement = Some(acknowledgement);
    Ok(true)
}

fn acknowledgement_wins(
    candidate: &ClockAcknowledgementV1,
    current: &ClockAcknowledgementV1,
) -> bool {
    (
        candidate.actor_clock,
        candidate.stop,
        candidate.actor_node_id.as_str(),
        candidate.acknowledgement_id.as_str(),
    ) > (
        current.actor_clock,
        current.stop,
        current.actor_node_id.as_str(),
        current.acknowledgement_id.as_str(),
    )
}

fn upsert_stopwatch(stopwatches: &mut Vec<ClockStopwatchV1>, stopwatch: ClockStopwatchV1) {
    if let Some(existing) = stopwatches
        .iter_mut()
        .find(|value| value.stopwatch_id == stopwatch.stopwatch_id)
    {
        *existing = stopwatch;
    } else {
        stopwatches.push(stopwatch);
    }
}

fn clock_targets_are_admitted(
    targets: &[String],
    local_node_id: &str,
    approved_peer_ids: &BTreeSet<String>,
) -> bool {
    targets
        .iter()
        .all(|target| target == local_node_id || approved_peer_ids.contains(target))
}

fn validate_stopwatch_deadlines(snapshot: &ClockSnapshotV1, now_ms: i64) -> anyhow::Result<()> {
    for stopwatch in &snapshot.stopwatches {
        validate_stopwatch_deadline(stopwatch, now_ms)?;
    }
    Ok(())
}

fn validate_stopwatch_deadline(stopwatch: &ClockStopwatchV1, now_ms: i64) -> anyhow::Result<()> {
    if stopwatch.phase != mackes_mesh_types::clock::ClockStopwatchPhase::Running {
        return Ok(());
    }
    let started_wall_utc_ms = stopwatch
        .started_wall_utc_ms
        .ok_or_else(|| anyhow::anyhow!("running Clock stopwatch lacks a wall-clock start"))?;
    let live_elapsed_ms = u64::try_from(now_ms.saturating_sub(started_wall_utc_ms))
        .map_err(|_| anyhow::anyhow!("Clock stopwatch live elapsed time underflowed"))?;
    let elapsed_ms = stopwatch
        .accumulated_elapsed_ms
        .checked_add(live_elapsed_ms)
        .ok_or_else(|| anyhow::anyhow!("Clock stopwatch elapsed time overflowed"))?;
    anyhow::ensure!(
        elapsed_ms <= MAX_CLOCK_STOPWATCH_ELAPSED_MS,
        "Clock stopwatch exceeded its bounded elapsed deadline"
    );
    Ok(())
}

fn stamp_revision(snapshot: &mut ClockSnapshotV1, local_node_id: &str) {
    for occurrence in &mut snapshot.occurrences {
        occurrence.revision = snapshot.revision;
        for target in &mut occurrence.targets {
            target.revision = snapshot.revision;
            target.observed_at_utc_ms = snapshot.produced_at_utc_ms;
        }
    }
    for stopwatch in &mut snapshot.stopwatches {
        if stopwatch.origin_node_id == local_node_id {
            stopwatch.revision = snapshot.revision;
        }
    }
}

fn preserve_unchanged_occurrence_revisions(
    snapshot: &mut ClockSnapshotV1,
    prior: &ClockSnapshotV1,
) {
    for occurrence in &mut snapshot.occurrences {
        let Some(previous) = prior
            .occurrences
            .iter()
            .find(|value| value.occurrence_id == occurrence.occurrence_id)
        else {
            continue;
        };
        if occurrence_semantics_equal(occurrence, previous) {
            occurrence.revision = previous.revision;
            for target in &mut occurrence.targets {
                if let Some(previous_target) = previous
                    .targets
                    .iter()
                    .find(|value| value.target_node_id == target.target_node_id)
                {
                    target.revision = previous_target.revision;
                    target.observed_at_utc_ms = previous_target.observed_at_utc_ms;
                }
            }
        }
    }
}

fn occurrence_semantics_equal(left: &ClockOccurrenceV1, right: &ClockOccurrenceV1) -> bool {
    left.occurrence_id == right.occurrence_id
        && left.global_event_id == right.global_event_id
        && left.schedule_id == right.schedule_id
        && left.due_at_utc_ms == right.due_at_utc_ms
        && left.phase == right.phase
        && left.acknowledgement == right.acknowledgement
        && left.targets.len() == right.targets.len()
        && left
            .targets
            .iter()
            .zip(&right.targets)
            .all(|(left, right)| {
                left.target_node_id == right.target_node_id && left.disposition == right.disposition
            })
}

fn clock_audio_transitions(
    prior: &ClockSnapshotV1,
    current: &ClockSnapshotV1,
    now_ms: i64,
    local_node_id: &str,
) -> anyhow::Result<Vec<writer::ClockAudioOutboxWrite>> {
    let mut requests = Vec::new();
    for occurrence in &current.occurrences {
        let previous = prior
            .occurrences
            .iter()
            .find(|value| value.occurrence_id == occurrence.occurrence_id);
        let local_disposition = occurrence
            .targets
            .iter()
            .find(|target| target.target_node_id == local_node_id)
            .map(|target| target.disposition);
        let previous_local_disposition = previous.and_then(|value| {
            value
                .targets
                .iter()
                .find(|target| target.target_node_id == local_node_id)
                .map(|target| target.disposition)
        });
        let transition = match (previous.map(|value| value.phase), occurrence.phase) {
            (Some(ClockOccurrencePhase::Ringing), ClockOccurrencePhase::Ringing) => None,
            (_, ClockOccurrencePhase::Ringing)
                if local_disposition == Some(ClockTargetDisposition::Ringing) =>
            {
                let schedule = current
                    .schedules
                    .iter()
                    .find(|value| value.schedule_id == occurrence.schedule_id)
                    .ok_or_else(|| anyhow::anyhow!("Clock audio schedule is missing"))?;
                Some((
                    occurrence.revision,
                    "start",
                    ClockAudioActionV1::Start {
                        audio: schedule_audio(schedule).clone(),
                        alarm_volume_milli: 1_000,
                    },
                ))
            }
            (Some(ClockOccurrencePhase::Ringing), ClockOccurrencePhase::Stopped)
                if previous_local_disposition == Some(ClockTargetDisposition::Ringing) =>
            {
                let acknowledgement = occurrence.acknowledgement.as_ref().ok_or_else(|| {
                    anyhow::anyhow!("stopped Clock occurrence lacks acknowledgement")
                })?;
                Some((
                    previous.expect("matched Ringing occurrence").revision,
                    "stop",
                    ClockAudioActionV1::Stop {
                        acknowledgement_id: acknowledgement.acknowledgement_id.clone(),
                    },
                ))
            }
            (Some(ClockOccurrencePhase::Ringing), ClockOccurrencePhase::Missed)
                if previous_local_disposition == Some(ClockTargetDisposition::Ringing) =>
            {
                Some((
                    previous.expect("matched Ringing occurrence").revision,
                    "auto-silence",
                    ClockAudioActionV1::Stop {
                        acknowledgement_id: auto_silence_acknowledgement_id(occurrence),
                    },
                ))
            }
            (Some(ClockOccurrencePhase::Ringing), ClockOccurrencePhase::Snoozed)
                if previous_local_disposition == Some(ClockTargetDisposition::Ringing) =>
            {
                let acknowledgement = occurrence.acknowledgement.as_ref().ok_or_else(|| {
                    anyhow::anyhow!("snoozed Clock occurrence lacks acknowledgement")
                })?;
                let schedule = current
                    .schedules
                    .iter()
                    .find(|candidate| candidate.schedule_id == occurrence.schedule_id)
                    .ok_or_else(|| anyhow::anyhow!("Clock audio schedule is missing"))?;
                let snooze_ms = i64::from(current.settings.snooze_minutes)
                    .checked_mul(60_000)
                    .ok_or_else(|| anyhow::anyhow!("Clock snooze duration overflow"))?;
                let resume_at_utc_ms = if matches!(schedule.schedule, ClockScheduleKindV1::Alarm(_))
                {
                    let child_id = snooze_occurrence_id(occurrence, acknowledgement);
                    current
                        .occurrences
                        .iter()
                        .find(|candidate| {
                            candidate.occurrence_id == child_id
                                && candidate.schedule_id == occurrence.schedule_id
                                && candidate.phase == ClockOccurrencePhase::Scheduled
                        })
                        .map(|candidate| candidate.due_at_utc_ms)
                        .filter(|deadline| *deadline > now_ms)
                        .ok_or_else(|| {
                            anyhow::anyhow!("snoozed Clock alarm lacks its bounded next deadline")
                        })?
                } else {
                    now_ms
                        .checked_add(snooze_ms)
                        .ok_or_else(|| anyhow::anyhow!("Clock snooze deadline overflow"))?
                };
                Some((
                    previous.expect("matched Ringing occurrence").revision,
                    "snooze",
                    ClockAudioActionV1::Snooze {
                        acknowledgement_id: acknowledgement.acknowledgement_id.clone(),
                        resume_at_utc_ms,
                    },
                ))
            }
            _ => None,
        };
        let Some((generation, action, body)) = transition else {
            continue;
        };
        requests.push(clock_audio_outbox_write(
            occurrence, generation, action, body, now_ms,
        )?);
    }
    Ok(requests)
}

fn clock_audio_recovery_requests(
    snapshot: &ClockSnapshotV1,
    created_at_ms: i64,
    local_node_id: &str,
) -> anyhow::Result<Vec<writer::ClockAudioOutboxWrite>> {
    let mut requests = Vec::new();
    for occurrence in &snapshot.occurrences {
        if occurrence.phase != ClockOccurrencePhase::Ringing
            || !occurrence.targets.iter().any(|target| {
                target.target_node_id == local_node_id
                    && target.disposition == ClockTargetDisposition::Ringing
            })
        {
            continue;
        }
        let schedule = snapshot
            .schedules
            .iter()
            .find(|value| value.schedule_id == occurrence.schedule_id)
            .ok_or_else(|| anyhow::anyhow!("Clock audio schedule is missing"))?;
        requests.push(clock_audio_outbox_write(
            occurrence,
            occurrence.revision,
            "start",
            ClockAudioActionV1::Start {
                audio: schedule_audio(schedule).clone(),
                alarm_volume_milli: 1_000,
            },
            created_at_ms,
        )?);
    }
    Ok(requests)
}

fn clock_audio_outbox_write(
    occurrence: &ClockOccurrenceV1,
    generation: u64,
    action: &str,
    body: ClockAudioActionV1,
    created_at_ms: i64,
) -> anyhow::Result<writer::ClockAudioOutboxWrite> {
    let request_id = clock_audio_request_id(
        &occurrence.occurrence_id,
        &occurrence.global_event_id,
        generation,
        action,
    );
    let request = ClockAudioRequestV1 {
        schema_version: CLOCK_SCHEMA_VERSION,
        request_id: request_id.clone(),
        occurrence_id: occurrence.occurrence_id.clone(),
        global_event_id: occurrence.global_event_id.clone(),
        occurrence_generation: generation,
        issued_at_utc_ms: created_at_ms,
        expires_at_utc_ms: created_at_ms.saturating_add(MAX_CLOCK_AUDIO_REQUEST_TTL_MS),
        body,
        music_auth: None,
    };
    request.validate_at(created_at_ms)?;
    Ok(writer::ClockAudioOutboxWrite {
        request_id,
        occurrence_id: request.occurrence_id.clone(),
        global_event_id: request.global_event_id.clone(),
        occurrence_generation: generation,
        request_json: serde_json::to_string(&request)?,
        created_at_ms,
    })
}

fn schedule_audio(schedule: &ClockScheduleV1) -> &mackes_mesh_types::clock::ClockAudioRef {
    match &schedule.schedule {
        ClockScheduleKindV1::Alarm(alarm) => &alarm.sound,
        ClockScheduleKindV1::Timer(timer) => &timer.sound,
    }
}

fn clock_audio_request_id(
    occurrence_id: &str,
    global_event_id: &str,
    generation: u64,
    action: &str,
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"magic-mesh:clock-audio-outbox:v1\0");
    digest.update(occurrence_id.as_bytes());
    digest.update([0]);
    digest.update(global_event_id.as_bytes());
    digest.update([0]);
    digest.update(generation.to_be_bytes());
    digest.update([0]);
    digest.update(action.as_bytes());
    format!(
        "clock-audio-{}-{action}",
        &hex_bytes(&digest.finalize())[..32]
    )
}

fn auto_silence_acknowledgement_id(occurrence: &ClockOccurrenceV1) -> String {
    let mut digest = Sha256::new();
    digest.update(b"magic-mesh:clock-auto-silence:v1\0");
    digest.update(occurrence.occurrence_id.as_bytes());
    digest.update([0]);
    digest.update(occurrence.global_event_id.as_bytes());
    format!(
        "clock-auto-silence-{}",
        &hex_bytes(&digest.finalize())[..32]
    )
}

fn schedule_removal_acknowledgement_id(occurrence_id: &str, request_id: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"magic-mesh:clock-schedule-removal:v1\0");
    digest.update(occurrence_id.as_bytes());
    digest.update([0]);
    digest.update(request_id.as_bytes());
    format!(
        "clock-schedule-removal-{}",
        &hex_bytes(&digest.finalize())[..32]
    )
}

fn schedule_disable_acknowledgement_id(occurrence_id: &str, request_id: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"magic-mesh:clock-schedule-disable:v1\0");
    digest.update(occurrence_id.as_bytes());
    digest.update([0]);
    digest.update(request_id.as_bytes());
    format!(
        "clock-schedule-disable-{}",
        &hex_bytes(&digest.finalize())[..32]
    )
}

fn local_time_zone() -> String {
    std::env::var("TZ")
        .ok()
        .filter(|value| zone_exists(value))
        .or_else(|| {
            fs::read_link("/etc/localtime").ok().and_then(|path| {
                path.to_string_lossy()
                    .rsplit_once("/zoneinfo/")
                    .map(|(_, zone)| zone.to_owned())
                    .filter(|zone| zone_exists(zone))
            })
        })
        .unwrap_or_else(|| "Etc/UTC".to_owned())
}

fn zone_exists(zone: &str) -> bool {
    !zone.starts_with('/')
        && !zone.contains("..")
        && Path::new("/usr/share/zoneinfo").join(zone).is_file()
}

fn load_trusted_signer() -> Option<TrustedSigner> {
    let signer_id = std::env::var(SIGNER_ID_ENV).ok()?;
    clock_command_topic(&signer_id).ok()?;
    let path = std::env::var_os(VERIFYING_KEY_ENV).map(PathBuf::from)?;
    let body = read_trusted_key(&path)?;
    let text = std::str::from_utf8(&body).ok()?.trim();
    let bytes = decode_hex_32(text)?;
    let key = VerifyingKey::from_bytes(&bytes).ok()?;
    Some(TrustedSigner { signer_id, key })
}

fn load_command_signing_key() -> Option<SigningKey> {
    let path = std::env::var_os(SIGNING_KEY_ENV).map(PathBuf::from)?;
    let body = read_root_regular_file(&path, MAX_SIGNING_KEY_FILE_BYTES)?;
    let bytes = decode_hex_32(std::str::from_utf8(&body).ok()?.trim())?;
    Some(SigningKey::from_bytes(&bytes))
}

fn load_id_set(variable: &str) -> BTreeSet<String> {
    std::env::var(variable)
        .ok()
        .into_iter()
        .flat_map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .filter(|value| clock_command_topic(value).is_ok())
        .collect()
}

fn load_music_signing_seed() -> Option<[u8; 32]> {
    let directory = PathBuf::from(std::env::var_os("CREDENTIALS_DIRECTORY")?);
    if !directory.is_absolute() {
        return None;
    }
    let body = read_root_regular_file(
        &directory.join(MUSIC_AUTH_CREDENTIAL_NAME),
        MAX_MUSIC_SIGNING_KEY_FILE_BYTES,
    )?;
    decode_hex_32(std::str::from_utf8(&body).ok()?.trim())
}

#[cfg(unix)]
fn read_trusted_key(path: &Path) -> Option<Vec<u8>> {
    read_root_regular_file(path, MAX_VERIFYING_KEY_FILE_BYTES)
}

#[cfg(unix)]
fn read_root_regular_file(path: &Path, maximum: usize) -> Option<Vec<u8>> {
    use rustix::fs::{Mode, OFlags};
    use std::io::Read as _;
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    let file: std::fs::File = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .ok()?
    .into();
    let metadata = file.metadata().ok()?;
    if !metadata.file_type().is_file()
        || metadata.len() > maximum as u64
        || metadata.uid() != 0
        || metadata.permissions().mode() & 0o022 != 0
    {
        return None;
    }
    let mut body = Vec::with_capacity(maximum + 1);
    file.take((maximum + 1) as u64)
        .read_to_end(&mut body)
        .ok()?;
    (body.len() <= maximum).then_some(body)
}

#[cfg(not(unix))]
fn read_trusted_key(_path: &Path) -> Option<Vec<u8>> {
    None
}

#[cfg(not(unix))]
fn read_root_regular_file(_path: &Path, _maximum: usize) -> Option<Vec<u8>> {
    None
}

fn decode_hex_32(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut decoded = [0_u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        decoded[index] = (hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?;
    }
    Some(decoded)
}

const fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn hex_bytes(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use mackes_mesh_types::clock::{
        ClockAlarmV1, ClockAudioPlaybackPhase, ClockAudioProviderStatus, ClockAudioRef,
        ClockGapPolicy, ClockTimerV1, MAX_CLOCK_COMMAND_TTL_MS,
    };
    use mackes_mesh_types::music_auth;
    use rusqlite::Connection;
    use std::process::Command;
    use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
    use tempfile::TempDir;

    const NOW: i64 = 1_800_000_000_000;

    struct AdjustableClock(AtomicI64);
    impl WallClock for AdjustableClock {
        fn now_ms(&self) -> i64 {
            self.0.load(Ordering::Relaxed)
        }
    }

    struct CountingClock {
        now_ms: i64,
        reads: AtomicUsize,
    }

    impl WallClock for CountingClock {
        fn now_ms(&self) -> i64 {
            self.reads.fetch_add(1, Ordering::SeqCst);
            self.now_ms
        }
    }

    struct FailNextCommitStore {
        inner: SqliteClockStore,
        fail_next: AtomicBool,
        fail_next_acknowledge: AtomicBool,
        acknowledge_calls: AtomicUsize,
    }

    impl ClockStore for FailNextCommitStore {
        fn load(&self, node_id: &str) -> anyhow::Result<Option<ClockAuthorityRecord>> {
            self.inner.load(node_id)
        }

        fn commit(
            &self,
            node_id: &str,
            expected_revision: u64,
            snapshot: &ClockSnapshotV1,
            request_id: Option<&str>,
            request_fingerprint: Option<&str>,
            action_cursor: Option<&str>,
            audio_requests: &[writer::ClockAudioOutboxWrite],
        ) -> anyhow::Result<bool> {
            if self.fail_next.swap(false, Ordering::SeqCst) {
                anyhow::bail!("injected Clock commit failure");
            }
            self.inner.commit(
                node_id,
                expected_revision,
                snapshot,
                request_id,
                request_fingerprint,
                action_cursor,
                audio_requests,
            )
        }

        fn pending_audio(
            &self,
            node_id: &str,
        ) -> anyhow::Result<Vec<writer::ClockAudioOutboxRecord>> {
            self.inner.pending_audio(node_id)
        }

        fn acknowledge_audio(
            &self,
            node_id: &str,
            status: &ClockAudioStatusV1,
        ) -> anyhow::Result<bool> {
            self.acknowledge_calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_next_acknowledge.swap(false, Ordering::SeqCst) {
                anyhow::bail!("injected Clock audio acknowledgement failure");
            }
            self.inner.acknowledge_audio(node_id, status)
        }
    }

    struct Fixture {
        _temp: TempDir,
        bus: PathBuf,
        db: PathBuf,
        clock: Arc<AdjustableClock>,
        signing_key: SigningKey,
        worker: ClockWorker,
    }

    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let bus = temp.path().join("bus");
            let db = temp.path().join("mackesd.db");
            let clock = Arc::new(AdjustableClock(AtomicI64::new(NOW)));
            let signing_key = SigningKey::from_bytes(&[9; 32]);
            let worker = ClockWorker {
                node_id: "seat-1".into(),
                bus_root: Some(bus.clone()),
                poll: POLL,
                clock: clock.clone(),
                store: Arc::new(SqliteClockStore {
                    db_path: db.clone(),
                }),
                signer: Some(TrustedSigner {
                    signer_id: "seat-1-key".into(),
                    key: signing_key.verifying_key(),
                }),
                command_signing_key: Some(signing_key.clone()),
                approved_peer_ids: BTreeSet::new(),
                blocked_origin_ids: BTreeSet::new(),
                disabled_schedule_ids: BTreeSet::new(),
                music_signing_seed: Some([7; 32]),
                snapshot: None,
                action_cursor: None,
                active_bus_identity: None,
                published_once: false,
                audio_status_cursor: None,
                audio_last_sent_ms: BTreeMap::new(),
                peer_last_sent_ms: BTreeMap::new(),
            };
            Self {
                _temp: temp,
                bus,
                db,
                clock,
                signing_key,
                worker,
            }
        }

        fn timer_command(
            &self,
            request_id: &str,
            expected_revision: u64,
            deadline: i64,
        ) -> ClockCommandV1 {
            ClockCommandV1 {
                schema_version: CLOCK_SCHEMA_VERSION,
                request_id: request_id.into(),
                origin_node_id: "seat-1".into(),
                expected_revision,
                issued_at_utc_ms: NOW - 100,
                expires_at_utc_ms: NOW - 100 + MAX_CLOCK_COMMAND_TTL_MS,
                body: ClockCommandKindV1::UpsertSchedule {
                    schedule: ClockScheduleV1 {
                        schedule_id: format!("timer-{request_id}"),
                        origin_node_id: "seat-1".into(),
                        revision: 1,
                        label: "Tea".into(),
                        selected_target_ids: vec!["seat-1".into()],
                        schedule: ClockScheduleKindV1::Timer(ClockTimerV1 {
                            original_duration_ms: 60_000,
                            phase: ClockTimerPhase::Running,
                            absolute_deadline_utc_ms: Some(deadline),
                            paused_remaining_ms: None,
                            expired_at_utc_ms: None,
                            sound: ClockAudioRef::Bundled {
                                tone_id: "bell".into(),
                            },
                            vibrate: false,
                        }),
                    },
                },
                signer_id: String::new(),
                signature: String::new(),
            }
            .sign(
                "seat-1-key",
                &self.signing_key,
                &ClockValidationContext {
                    wall_utc_ms: NOW,
                    monotonic_ms: 1,
                    zone_exists: &zone_exists,
                },
            )
            .unwrap()
        }

        fn alarm_command(
            &self,
            request_id: &str,
            expected_revision: u64,
            due_at_utc_ms: i64,
        ) -> ClockCommandV1 {
            ClockCommandV1 {
                schema_version: CLOCK_SCHEMA_VERSION,
                request_id: request_id.into(),
                origin_node_id: "seat-1".into(),
                expected_revision,
                issued_at_utc_ms: NOW - 100,
                expires_at_utc_ms: NOW - 100 + MAX_CLOCK_COMMAND_TTL_MS,
                body: ClockCommandKindV1::UpsertSchedule {
                    schedule: ClockScheduleV1 {
                        schedule_id: format!("alarm-{request_id}"),
                        origin_node_id: "seat-1".into(),
                        revision: 1,
                        label: "Wake".into(),
                        selected_target_ids: vec!["seat-1".into()],
                        schedule: ClockScheduleKindV1::Alarm(ClockAlarmV1 {
                            enabled: true,
                            label: "Wake".into(),
                            recurrence: ClockAlarmRecurrenceV1::OneTime { due_at_utc_ms },
                            sound: ClockAudioRef::Bundled {
                                tone_id: "bell".into(),
                            },
                            vibrate: false,
                        }),
                    },
                },
                signer_id: String::new(),
                signature: String::new(),
            }
            .sign(
                "seat-1-key",
                &self.signing_key,
                &ClockValidationContext {
                    wall_utc_ms: NOW,
                    monotonic_ms: 1,
                    zone_exists: &zone_exists,
                },
            )
            .unwrap()
        }

        fn remove_schedule_command(
            &self,
            request_id: &str,
            expected_revision: u64,
            schedule_id: &str,
            now_ms: i64,
        ) -> ClockCommandV1 {
            ClockCommandV1 {
                schema_version: CLOCK_SCHEMA_VERSION,
                request_id: request_id.into(),
                origin_node_id: "seat-1".into(),
                expected_revision,
                issued_at_utc_ms: now_ms,
                expires_at_utc_ms: now_ms + MAX_CLOCK_COMMAND_TTL_MS,
                body: ClockCommandKindV1::RemoveSchedule {
                    schedule_id: schedule_id.into(),
                },
                signer_id: String::new(),
                signature: String::new(),
            }
            .sign(
                "seat-1-key",
                &self.signing_key,
                &ClockValidationContext {
                    wall_utc_ms: now_ms,
                    monotonic_ms: 1,
                    zone_exists: &zone_exists,
                },
            )
            .unwrap()
        }

        fn snooze_command(
            &self,
            request_id: &str,
            expected_revision: u64,
            occurrence: &ClockOccurrenceV1,
            now_ms: i64,
        ) -> ClockCommandV1 {
            self.alarm_ack_command(
                request_id,
                "ack-snooze-1",
                expected_revision,
                occurrence,
                now_ms,
                false,
            )
        }

        fn alarm_ack_command(
            &self,
            request_id: &str,
            acknowledgement_id: &str,
            expected_revision: u64,
            occurrence: &ClockOccurrenceV1,
            now_ms: i64,
            stop: bool,
        ) -> ClockCommandV1 {
            ClockCommandV1 {
                schema_version: CLOCK_SCHEMA_VERSION,
                request_id: request_id.into(),
                origin_node_id: "seat-1".into(),
                expected_revision,
                issued_at_utc_ms: now_ms,
                expires_at_utc_ms: now_ms + MAX_CLOCK_COMMAND_TTL_MS,
                body: ClockCommandKindV1::Acknowledge {
                    occurrence_id: occurrence.occurrence_id.clone(),
                    acknowledgement: ClockAcknowledgementV1 {
                        acknowledgement_id: acknowledgement_id.into(),
                        global_event_id: occurrence.global_event_id.clone(),
                        actor_node_id: "seat-1".into(),
                        actor_clock: expected_revision.saturating_add(1),
                        acknowledged_at_utc_ms: now_ms,
                        stop,
                    },
                },
                signer_id: String::new(),
                signature: String::new(),
            }
            .sign(
                "seat-1-key",
                &self.signing_key,
                &ClockValidationContext {
                    wall_utc_ms: now_ms,
                    monotonic_ms: 1,
                    zone_exists: &zone_exists,
                },
            )
            .unwrap()
        }

        fn timer_extension_command(
            &self,
            request_id: &str,
            expected_revision: u64,
            issued_at_ms: i64,
            mut schedule: ClockScheduleV1,
        ) -> ClockCommandV1 {
            let ClockScheduleKindV1::Timer(timer) = &mut schedule.schedule else {
                panic!("fixture extension schedule must be a timer");
            };
            timer.phase = ClockTimerPhase::Running;
            timer.absolute_deadline_utc_ms = Some(issued_at_ms + 60_000);
            timer.paused_remaining_ms = None;
            timer.expired_at_utc_ms = None;
            ClockCommandV1 {
                schema_version: CLOCK_SCHEMA_VERSION,
                request_id: request_id.into(),
                origin_node_id: "seat-1".into(),
                expected_revision,
                issued_at_utc_ms: issued_at_ms,
                expires_at_utc_ms: issued_at_ms + MAX_CLOCK_COMMAND_TTL_MS,
                body: ClockCommandKindV1::UpsertSchedule { schedule },
                signer_id: String::new(),
                signature: String::new(),
            }
            .sign(
                "seat-1-key",
                &self.signing_key,
                &ClockValidationContext {
                    wall_utc_ms: issued_at_ms,
                    monotonic_ms: 1,
                    zone_exists: &zone_exists,
                },
            )
            .unwrap()
        }

        fn publish(&self, command: &ClockCommandV1) {
            let persist = Persist::open(self.bus.clone()).unwrap();
            persist
                .write(
                    &clock_command_topic("seat-1").unwrap(),
                    Priority::Default,
                    None,
                    Some(&serde_json::to_string(command).unwrap()),
                )
                .unwrap();
        }

        fn audio_messages(&self) -> Vec<String> {
            Persist::open(self.bus.clone())
                .unwrap()
                .list_since(CLOCK_AUDIO_ACTION_TOPIC, None)
                .unwrap()
                .into_iter()
                .map(|message| message.body.unwrap())
                .collect()
        }
    }

    #[test]
    fn clock_bus_root_honors_override_and_falls_back_to_system_spool() {
        assert_eq!(
            clock_bus_root_or_system(None),
            PathBuf::from(mde_bus::SYSTEM_BUS_ROOT)
        );
        assert_eq!(
            clock_bus_root_or_system(Some(PathBuf::from("/tmp/clock-explicit-bus"))),
            PathBuf::from("/tmp/clock-explicit-bus")
        );
    }

    #[tokio::test]
    async fn late_bus_recovers_same_worker_and_observes_external_forward_command() {
        let Fixture {
            _temp,
            bus,
            db,
            clock: _,
            signing_key,
            mut worker,
        } = Fixture::new();
        worker.poll = Duration::from_millis(5);
        let command = ClockCommandV1 {
            schema_version: CLOCK_SCHEMA_VERSION,
            request_id: "late-forward".into(),
            origin_node_id: "seat-1".into(),
            expected_revision: 1,
            issued_at_utc_ms: NOW - 100,
            expires_at_utc_ms: NOW - 100 + MAX_CLOCK_COMMAND_TTL_MS,
            body: ClockCommandKindV1::UpsertSchedule {
                schedule: ClockScheduleV1 {
                    schedule_id: "timer-late-forward".into(),
                    origin_node_id: "seat-1".into(),
                    revision: 1,
                    label: "Tea".into(),
                    selected_target_ids: vec!["seat-1".into()],
                    schedule: ClockScheduleKindV1::Timer(ClockTimerV1 {
                        original_duration_ms: 60_000,
                        phase: ClockTimerPhase::Running,
                        absolute_deadline_utc_ms: Some(NOW + 60_000),
                        paused_remaining_ms: None,
                        expired_at_utc_ms: None,
                        sound: ClockAudioRef::Bundled {
                            tone_id: "bell".into(),
                        },
                        vibrate: false,
                    }),
                },
            },
            signer_id: String::new(),
            signature: String::new(),
        }
        .sign(
            "seat-1-key",
            &signing_key,
            &ClockValidationContext {
                wall_utc_ms: NOW,
                monotonic_ms: 1,
                zone_exists: &zone_exists,
            },
        )
        .unwrap();

        std::fs::write(&bus, b"blocks Persist::open").unwrap();
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let task =
            tokio::spawn(
                async move { worker.run(ShutdownToken::from_receiver(shutdown_rx)).await },
            );

        tokio::time::sleep(Duration::from_millis(35)).await;
        assert!(
            !task.is_finished(),
            "late Bus must not terminate the worker"
        );
        assert!(
            SqliteClockStore { db_path: db }
                .load("seat-1")
                .unwrap()
                .is_some(),
            "durable Clock authority must load before Bus recovery"
        );

        std::fs::remove_file(&bus).unwrap();
        let external = Persist::open(bus.clone()).unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if external
                    .read_latest(&clock_state_topic("seat-1").unwrap())
                    .unwrap()
                    .is_some()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("Clock worker did not activate on the late Bus");

        external
            .write(
                &clock_command_topic("seat-1").unwrap(),
                Priority::Default,
                None,
                Some(&serde_json::to_string(&command).unwrap()),
            )
            .unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let state = external
                    .read_latest(&clock_state_topic("seat-1").unwrap())
                    .unwrap()
                    .and_then(|message| message.body)
                    .map(|body| serde_json::from_str::<ClockSnapshotV1>(&body).unwrap());
                if state.is_some_and(|snapshot| {
                    snapshot
                        .schedules
                        .iter()
                        .any(|schedule| schedule.schedule_id == "timer-late-forward")
                }) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("external forward command was not applied");

        shutdown_tx.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .expect("shutdown timed out")
            .expect("Clock task panicked")
            .expect("Clock worker returned an error");
    }

    #[test]
    fn commit_and_publication_failures_retain_action_for_same_worker_retry() {
        let mut fixture = Fixture::new();
        fixture.worker.tick_once().unwrap();
        let fail_store = Arc::new(FailNextCommitStore {
            inner: SqliteClockStore {
                db_path: fixture.db.clone(),
            },
            fail_next: AtomicBool::new(false),
            fail_next_acknowledge: AtomicBool::new(false),
            acknowledge_calls: AtomicUsize::new(0),
        });
        fixture.worker.store = fail_store.clone();

        let first = fixture.timer_command("commit-retry", 1, NOW + 60_000);
        fixture.publish(&first);
        fail_store.fail_next.store(true, Ordering::SeqCst);
        assert!(fixture.worker.tick_once().is_err());
        assert_eq!(fixture.worker.action_cursor, None);
        assert_eq!(fixture.worker.snapshot.as_ref().unwrap().revision, 1);
        assert!(fixture
            .worker
            .snapshot
            .as_ref()
            .unwrap()
            .schedules
            .is_empty());

        fixture.worker.tick_once().unwrap();
        let first_cursor = fixture.worker.action_cursor.clone();
        assert_eq!(fixture.worker.snapshot.as_ref().unwrap().revision, 2);
        assert_eq!(fixture.worker.snapshot.as_ref().unwrap().schedules.len(), 1);

        let second = fixture.timer_command("publish-retry", 2, NOW + 120_000);
        fixture.publish(&second);
        let state_root = fixture.bus.join("state");
        std::fs::remove_dir_all(&state_root).unwrap();
        std::fs::write(&state_root, b"blocks Clock state publication").unwrap();
        assert!(fixture.worker.tick_once().is_err());
        // The SQLite authority commit may win before Bus publication fails.
        // Adopt its cursor so the next sweep repairs publication without
        // replaying the already durable command.
        assert!(fixture.worker.action_cursor > first_cursor);
        assert_eq!(fixture.worker.snapshot.as_ref().unwrap().revision, 3);
        assert_eq!(fixture.worker.snapshot.as_ref().unwrap().schedules.len(), 2);

        std::fs::remove_file(&state_root).unwrap();
        fixture.worker.tick_once().unwrap();
        assert_eq!(fixture.worker.snapshot.as_ref().unwrap().revision, 3);
        assert_eq!(fixture.worker.snapshot.as_ref().unwrap().schedules.len(), 2);
        assert!(fixture.worker.action_cursor > first_cursor);
        let published: ClockSnapshotV1 = serde_json::from_str(
            &Persist::open(fixture.bus.clone())
                .unwrap()
                .read_latest(&clock_state_topic("seat-1").unwrap())
                .unwrap()
                .unwrap()
                .body
                .unwrap(),
        )
        .unwrap();
        assert_eq!(published.revision, 3);
    }

    #[test]
    fn deadline_publish_failure_reloads_durable_occurrence_before_replay() {
        let mut fixture = Fixture::new();
        fixture.worker.tick_once().unwrap();
        fixture.publish(&fixture.timer_command("deadline-publish-replay", 1, NOW + 5_000));
        fixture.worker.tick_once().unwrap();

        fixture.clock.0.store(NOW + 10_000, Ordering::Relaxed);
        let state_root = fixture.bus.join("state");
        std::fs::remove_dir_all(&state_root).unwrap();
        std::fs::write(&state_root, b"blocks Clock state publication").unwrap();

        assert!(fixture.worker.tick_once().is_err());
        let durable = SqliteClockStore {
            db_path: fixture.db.clone(),
        }
        .load("seat-1")
        .unwrap()
        .unwrap();
        let durable_snapshot: ClockSnapshotV1 =
            serde_json::from_str(&durable.snapshot_json).unwrap();
        assert_eq!(durable_snapshot.revision, 3);
        assert_eq!(durable_snapshot.occurrences.len(), 1);
        assert_eq!(
            durable_snapshot.occurrences[0].phase,
            ClockOccurrencePhase::Ringing
        );
        assert_eq!(fixture.worker.snapshot.as_ref().unwrap(), &durable_snapshot);
        assert!(!fixture.worker.published_once);
        assert!(fixture.audio_messages().is_empty());
        assert_eq!(
            Connection::open(&fixture.db)
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM clock_audio_outbox WHERE node_id = 'seat-1' AND acknowledged_at_ms IS NULL",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1,
            "the effect must be durable before its Bus publication"
        );

        std::fs::remove_file(&state_root).unwrap();
        fixture.worker.tick_once().unwrap();
        assert_eq!(
            fixture.worker.snapshot.as_ref().unwrap().occurrences.len(),
            1
        );
        assert_eq!(fixture.audio_messages().len(), 1);

        fixture.worker.tick_once().unwrap();
        assert_eq!(
            fixture.worker.snapshot.as_ref().unwrap().occurrences.len(),
            1
        );
        assert_eq!(
            fixture.audio_messages().len(),
            1,
            "same-worker replay must not duplicate the recovered effect"
        );
    }

    #[test]
    fn command_commit_survives_publication_failure_and_bus_generation_loss() {
        let mut fixture = Fixture::new();
        fixture.worker.tick_once().unwrap();
        fixture.publish(&fixture.timer_command("command-generation-loss", 1, NOW + 60_000));

        let state_root = fixture.bus.join("state");
        std::fs::remove_dir_all(&state_root).unwrap();
        std::fs::write(&state_root, b"blocks Clock state publication").unwrap();

        assert!(fixture.worker.tick_once().is_err());
        let durable = SqliteClockStore {
            db_path: fixture.db.clone(),
        }
        .load("seat-1")
        .unwrap()
        .unwrap();
        let durable_snapshot: ClockSnapshotV1 =
            serde_json::from_str(&durable.snapshot_json).unwrap();
        assert_eq!(durable_snapshot.revision, 2);
        assert_eq!(durable_snapshot.schedules.len(), 1);
        assert_eq!(fixture.worker.snapshot.as_ref(), Some(&durable_snapshot));
        assert!(!fixture.worker.published_once);

        // Replace the transient Bus with an empty generation. The committed
        // command no longer exists anywhere in the new index, so recovery can
        // only come from the durable Clock authority adopted above.
        let replacement_root = fixture._temp.path().join("command-replacement-bus");
        drop(Persist::open(replacement_root.clone()).unwrap());
        let old_identity = clock_bus_identity(&fixture.bus).unwrap();
        let retired_index = fixture._temp.path().join("command-retired-index.sqlite");
        fs::rename(fixture.bus.join("index.sqlite"), &retired_index).unwrap();
        fs::rename(
            replacement_root.join("index.sqlite"),
            fixture.bus.join("index.sqlite"),
        )
        .unwrap();
        let replacement_identity = clock_bus_identity(&fixture.bus).unwrap();
        assert_ne!(old_identity, replacement_identity);
        std::fs::remove_file(&state_root).unwrap();

        fixture.worker.tick_once().unwrap();
        let recovered = fixture.worker.snapshot.as_ref().unwrap();
        assert_eq!(recovered.revision, 2);
        assert_eq!(recovered.schedules.len(), 1);
        assert_eq!(
            recovered.schedules[0].schedule_id,
            "timer-command-generation-loss"
        );
        assert_eq!(
            fixture.worker.active_bus_identity,
            Some(replacement_identity)
        );
        let published: ClockSnapshotV1 = serde_json::from_str(
            &Persist::open(fixture.bus.clone())
                .unwrap()
                .read_latest(&clock_state_topic("seat-1").unwrap())
                .unwrap()
                .unwrap()
                .body
                .unwrap(),
        )
        .unwrap();
        assert_eq!(published, *recovered);
        assert_eq!(
            Connection::open(&fixture.db)
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM clock_request_ledger WHERE node_id = 'seat-1' AND request_id = 'command-generation-loss'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1,
            "corrected-forward publication must not reapply the command"
        );
    }

    #[test]
    fn audio_acknowledgement_failure_retains_status_for_same_worker_retry() {
        let mut fixture = Fixture::new();
        fixture.worker.tick_once().unwrap();
        let fail_store = Arc::new(FailNextCommitStore {
            inner: SqliteClockStore {
                db_path: fixture.db.clone(),
            },
            fail_next: AtomicBool::new(false),
            fail_next_acknowledge: AtomicBool::new(true),
            acknowledge_calls: AtomicUsize::new(0),
        });
        fixture.worker.store = fail_store.clone();

        let status = ClockAudioStatusV1 {
            schema_version: CLOCK_SCHEMA_VERSION,
            request_id: "audio-ack-retry".into(),
            occurrence_id: "occurrence-audio-ack-retry".into(),
            global_event_id: "global-audio-ack-retry".into(),
            occurrence_generation: 1,
            observed_at_utc_ms: NOW,
            phase: ClockAudioPlaybackPhase::PlayingBundled,
            provider_status: ClockAudioProviderStatus::NotApplicable,
            fallback_tone_id: None,
            acknowledgement_id: None,
            reason_code: None,
        };
        status.validate_at(NOW).unwrap();
        let message = Persist::open(fixture.bus.clone())
            .unwrap()
            .write(
                &clock_audio_status_topic("seat-1").unwrap(),
                Priority::Default,
                None,
                Some(&serde_json::to_string(&status).unwrap()),
            )
            .unwrap();

        assert!(fixture.worker.tick_once().is_err());
        assert_eq!(fixture.worker.audio_status_cursor, None);
        assert_eq!(fail_store.acknowledge_calls.load(Ordering::SeqCst), 1);

        fixture.worker.tick_once().unwrap();
        assert_eq!(
            fixture.worker.audio_status_cursor.as_deref(),
            Some(message.ulid.as_str())
        );
        assert_eq!(fail_store.acknowledge_calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn same_path_bus_replacement_skips_retained_lanes_and_consumes_forward_once() {
        let mut fixture = Fixture::new();
        fixture.worker.tick_once().unwrap();
        fixture.publish(&fixture.timer_command("durable-before-replacement", 1, NOW + 60_000));
        fixture.worker.tick_once().unwrap();
        assert_eq!(fixture.worker.snapshot.as_ref().unwrap().revision, 2);

        let tracked_store = Arc::new(FailNextCommitStore {
            inner: SqliteClockStore {
                db_path: fixture.db.clone(),
            },
            fail_next: AtomicBool::new(false),
            fail_next_acknowledge: AtomicBool::new(false),
            acknowledge_calls: AtomicUsize::new(0),
        });
        fixture.worker.store = tracked_store.clone();

        let replacement_root = fixture._temp.path().join("replacement-bus");
        let replacement = Persist::open(replacement_root.clone()).unwrap();
        let retained = fixture.timer_command("retained-replacement", 2, NOW + 120_000);
        replacement
            .write(
                &clock_command_topic("seat-1").unwrap(),
                Priority::Default,
                None,
                Some(&serde_json::to_string(&retained).unwrap()),
            )
            .unwrap();
        let retained_status = ClockAudioStatusV1 {
            schema_version: CLOCK_SCHEMA_VERSION,
            request_id: "retained-audio-status".into(),
            occurrence_id: "retained-occurrence".into(),
            global_event_id: "retained-global-event".into(),
            occurrence_generation: 1,
            observed_at_utc_ms: NOW,
            phase: ClockAudioPlaybackPhase::PlayingBundled,
            provider_status: ClockAudioProviderStatus::NotApplicable,
            fallback_tone_id: None,
            acknowledgement_id: None,
            reason_code: None,
        };
        replacement
            .write(
                &clock_audio_status_topic("seat-1").unwrap(),
                Priority::Default,
                None,
                Some(&serde_json::to_string(&retained_status).unwrap()),
            )
            .unwrap();
        drop(replacement);

        let old_identity = clock_bus_identity(&fixture.bus).unwrap();
        let retired_index = fixture._temp.path().join("retired-index.sqlite");
        fs::rename(fixture.bus.join("index.sqlite"), &retired_index).unwrap();
        fs::rename(
            replacement_root.join("index.sqlite"),
            fixture.bus.join("index.sqlite"),
        )
        .unwrap();
        let replacement_identity = clock_bus_identity(&fixture.bus).unwrap();
        assert_ne!(old_identity, replacement_identity);

        fixture.worker.tick_once().unwrap();
        let after_activation = fixture.worker.snapshot.as_ref().unwrap();
        assert_eq!(after_activation.revision, 2);
        assert!(after_activation
            .schedules
            .iter()
            .any(|schedule| schedule.schedule_id == "timer-durable-before-replacement"));
        assert!(!after_activation
            .schedules
            .iter()
            .any(|schedule| schedule.schedule_id == "timer-retained-replacement"));
        assert_eq!(tracked_store.acknowledge_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            fixture.worker.active_bus_identity,
            Some(replacement_identity)
        );

        let live = Persist::open(fixture.bus.clone()).unwrap();
        let forward = fixture.timer_command("first-forward", 2, NOW + 180_000);
        let forward_message = live
            .write(
                &clock_command_topic("seat-1").unwrap(),
                Priority::Default,
                None,
                Some(&serde_json::to_string(&forward).unwrap()),
            )
            .unwrap();
        let mut forward_status = retained_status;
        forward_status.request_id = "first-forward-audio-status".into();
        live.write(
            &clock_audio_status_topic("seat-1").unwrap(),
            Priority::Default,
            None,
            Some(&serde_json::to_string(&forward_status).unwrap()),
        )
        .unwrap();

        fixture.worker.tick_once().unwrap();
        let after_forward = fixture.worker.snapshot.as_ref().unwrap();
        assert_eq!(after_forward.revision, 3);
        assert_eq!(
            after_forward
                .schedules
                .iter()
                .filter(|schedule| schedule.schedule_id == "timer-first-forward")
                .count(),
            1
        );
        assert_eq!(
            fixture.worker.action_cursor.as_deref(),
            Some(forward_message.ulid.as_str())
        );
        assert_eq!(tracked_store.acknowledge_calls.load(Ordering::SeqCst), 1);

        fixture.worker.tick_once().unwrap();
        assert_eq!(fixture.worker.snapshot.as_ref().unwrap().revision, 3);
        assert_eq!(tracked_store.acknowledge_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            SqliteClockStore {
                db_path: fixture.db.clone(),
            }
            .load("seat-1")
            .unwrap()
            .unwrap()
            .action_cursor
            .as_deref(),
            Some(forward_message.ulid.as_str())
        );
    }

    struct PeerNode {
        clock: Arc<AdjustableClock>,
        worker: ClockWorker,
    }

    impl PeerNode {
        fn new(root: &Path, bus: &Path, node_id: &str, key: &SigningKey) -> Self {
            let clock = Arc::new(AdjustableClock(AtomicI64::new(NOW)));
            let approved_peer_ids = ["node-a", "node-b", "node-c"]
                .into_iter()
                .map(str::to_owned)
                .collect();
            Self {
                clock: clock.clone(),
                worker: ClockWorker {
                    node_id: node_id.to_owned(),
                    bus_root: Some(bus.to_path_buf()),
                    poll: POLL,
                    clock,
                    store: Arc::new(SqliteClockStore {
                        db_path: root.join(format!("{node_id}.db")),
                    }),
                    signer: Some(TrustedSigner {
                        signer_id: "clock-mesh-key".into(),
                        key: key.verifying_key(),
                    }),
                    command_signing_key: Some(key.clone()),
                    approved_peer_ids,
                    blocked_origin_ids: BTreeSet::new(),
                    disabled_schedule_ids: BTreeSet::new(),
                    music_signing_seed: None,
                    snapshot: None,
                    action_cursor: None,
                    active_bus_identity: None,
                    published_once: false,
                    audio_status_cursor: None,
                    audio_last_sent_ms: BTreeMap::new(),
                    peer_last_sent_ms: BTreeMap::new(),
                },
            }
        }

        fn revision(&self) -> u64 {
            self.worker.snapshot.as_ref().unwrap().revision
        }

        fn occurrence(&self, schedule_id: &str) -> &ClockOccurrenceV1 {
            self.worker
                .snapshot
                .as_ref()
                .unwrap()
                .occurrences
                .iter()
                .find(|value| value.schedule_id == schedule_id)
                .unwrap()
        }
    }

    const PROCESS_FIXTURE_ROOT_ENV: &str = "MDE_CLOCK_PROCESS_FIXTURE_ROOT";
    const PROCESS_FIXTURE_NODE_ENV: &str = "MDE_CLOCK_PROCESS_FIXTURE_NODE";
    const PROCESS_FIXTURE_NOW_ENV: &str = "MDE_CLOCK_PROCESS_FIXTURE_NOW";
    const PROCESS_FIXTURE_DISABLED_ENV: &str = "MDE_CLOCK_PROCESS_FIXTURE_DISABLED";

    fn process_fixture_worker(
        root: &Path,
        node_id: &str,
        now_ms: i64,
        disabled_schedule_ids: BTreeSet<String>,
    ) -> ClockWorker {
        let key = SigningKey::from_bytes(&[31; 32]);
        ClockWorker {
            node_id: node_id.to_owned(),
            bus_root: Some(root.join("bus")),
            poll: POLL,
            clock: Arc::new(AdjustableClock(AtomicI64::new(now_ms))),
            store: Arc::new(SqliteClockStore {
                db_path: root.join(format!("{node_id}.db")),
            }),
            signer: Some(TrustedSigner {
                signer_id: "clock-mesh-key".into(),
                key: key.verifying_key(),
            }),
            command_signing_key: Some(key),
            approved_peer_ids: ["node-a", "node-b", "node-c"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            blocked_origin_ids: BTreeSet::new(),
            disabled_schedule_ids,
            music_signing_seed: None,
            snapshot: None,
            action_cursor: None,
            active_bus_identity: None,
            published_once: false,
            audio_status_cursor: None,
            audio_last_sent_ms: BTreeMap::new(),
            peer_last_sent_ms: BTreeMap::new(),
        }
    }

    #[test]
    fn clock_process_fixture_child() {
        let Ok(root) = std::env::var(PROCESS_FIXTURE_ROOT_ENV) else {
            return;
        };
        let node_id = std::env::var(PROCESS_FIXTURE_NODE_ENV).unwrap();
        let now_ms = std::env::var(PROCESS_FIXTURE_NOW_ENV)
            .unwrap()
            .parse::<i64>()
            .unwrap();
        let disabled_schedule_ids = std::env::var(PROCESS_FIXTURE_DISABLED_ENV)
            .unwrap_or_default()
            .split(',')
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .collect();
        process_fixture_worker(Path::new(&root), &node_id, now_ms, disabled_schedule_ids)
            .tick_once()
            .unwrap();
    }

    fn run_clock_process(root: &Path, node_id: &str, now_ms: i64, disabled: &[&str]) {
        let status = Command::new(std::env::current_exe().unwrap())
            .arg("clock_process_fixture_child")
            .arg("--nocapture")
            .arg("--test-threads=1")
            .env(PROCESS_FIXTURE_ROOT_ENV, root)
            .env(PROCESS_FIXTURE_NODE_ENV, node_id)
            .env(PROCESS_FIXTURE_NOW_ENV, now_ms.to_string())
            .env(PROCESS_FIXTURE_DISABLED_ENV, disabled.join(","))
            .status()
            .unwrap();
        assert!(status.success(), "Clock child process for {node_id} failed");
    }

    fn process_fixture_snapshot(root: &Path, node_id: &str) -> ClockSnapshotV1 {
        let store = SqliteClockStore {
            db_path: root.join(format!("{node_id}.db")),
        };
        let record = store.load(node_id).unwrap().unwrap();
        serde_json::from_str(&record.snapshot_json).unwrap()
    }

    fn signed_timer_for(
        key: &SigningKey,
        origin: &str,
        request_id: &str,
        expected_revision: u64,
        schedule_id: &str,
        deadline: i64,
        targets: &[&str],
        now_ms: i64,
    ) -> ClockCommandV1 {
        ClockCommandV1 {
            schema_version: CLOCK_SCHEMA_VERSION,
            request_id: request_id.into(),
            origin_node_id: origin.into(),
            expected_revision,
            issued_at_utc_ms: now_ms - 1,
            expires_at_utc_ms: now_ms - 1 + MAX_CLOCK_COMMAND_TTL_MS,
            body: ClockCommandKindV1::UpsertSchedule {
                schedule: ClockScheduleV1 {
                    schedule_id: schedule_id.into(),
                    origin_node_id: origin.into(),
                    revision: 1,
                    label: schedule_id.into(),
                    selected_target_ids: targets.iter().map(|value| (*value).into()).collect(),
                    schedule: ClockScheduleKindV1::Timer(ClockTimerV1 {
                        original_duration_ms: 60_000,
                        phase: ClockTimerPhase::Running,
                        absolute_deadline_utc_ms: Some(deadline),
                        paused_remaining_ms: None,
                        expired_at_utc_ms: None,
                        sound: ClockAudioRef::Bundled {
                            tone_id: "bell".into(),
                        },
                        vibrate: false,
                    }),
                },
            },
            signer_id: String::new(),
            signature: String::new(),
        }
        .sign(
            "clock-mesh-key",
            key,
            &ClockValidationContext {
                wall_utc_ms: now_ms,
                monotonic_ms: 1,
                zone_exists: &zone_exists,
            },
        )
        .unwrap()
    }

    fn weekday_alarm_schedule(
        schedule_id: &str,
        hour: u8,
        minute: u8,
        fold: ClockFoldPolicy,
    ) -> ClockScheduleV1 {
        ClockScheduleV1 {
            schedule_id: schedule_id.into(),
            origin_node_id: "seat-1".into(),
            revision: 1,
            label: "DST alarm".into(),
            selected_target_ids: vec!["seat-1".into()],
            schedule: ClockScheduleKindV1::Alarm(ClockAlarmV1 {
                enabled: true,
                label: "DST alarm".into(),
                recurrence: ClockAlarmRecurrenceV1::Weekdays {
                    local_time: mackes_mesh_types::clock::ClockCivilTimeV1 {
                        hour,
                        minute,
                        second: 0,
                        time_zone: "America/New_York".into(),
                        fold,
                        gap: ClockGapPolicy::NextValid,
                    },
                    weekdays: vec![ClockWeekday::Sunday],
                },
                sound: ClockAudioRef::Bundled {
                    tone_id: "bell".into(),
                },
                vibrate: false,
            }),
        }
    }

    fn timestamp_ms(value: &str) -> i64 {
        value.parse::<Timestamp>().unwrap().as_millisecond()
    }

    #[test]
    fn weekday_alarm_resolves_dst_and_advances_once_per_selected_civil_day() {
        let spring_due = timestamp_ms("2024-03-10T07:30:00Z");
        let spring = weekday_alarm_schedule("weekly-spring", 2, 30, ClockFoldPolicy::Earlier);
        assert_eq!(
            due_now_or_before(&spring, spring_due).unwrap(),
            Some(spring_due),
            "a gap must advance to the next valid local instant"
        );

        let fall_earlier_due = timestamp_ms("2024-11-03T05:30:00Z");
        let fall_later_due = timestamp_ms("2024-11-03T06:30:00Z");
        let fall_earlier =
            weekday_alarm_schedule("weekly-fold-early", 1, 30, ClockFoldPolicy::Earlier);
        let fall_later = weekday_alarm_schedule("weekly-fold-late", 1, 30, ClockFoldPolicy::Later);
        assert_eq!(
            due_now_or_before(&fall_earlier, fall_earlier_due).unwrap(),
            Some(fall_earlier_due)
        );
        assert_eq!(
            due_now_or_before(&fall_later, fall_later_due).unwrap(),
            Some(fall_later_due)
        );

        let mut fixture = Fixture::new();
        fixture.worker.tick_once().unwrap();
        let snapshot = fixture.worker.snapshot.as_mut().unwrap();
        snapshot.schedules.push(spring);
        snapshot.produced_at_utc_ms = spring_due - 1;

        assert!(fixture.worker.advance_deadlines(spring_due).unwrap());
        assert!(!fixture.worker.advance_deadlines(spring_due).unwrap());
        let snapshot = fixture.worker.snapshot.as_mut().unwrap();
        assert_eq!(snapshot.occurrences.len(), 1);
        assert_eq!(snapshot.occurrences[0].due_at_utc_ms, spring_due);
        let ClockScheduleKindV1::Alarm(alarm) = &snapshot.schedules[0].schedule else {
            panic!("weekday schedule changed kind");
        };
        assert!(alarm.enabled, "a recurring alarm must remain armed");

        snapshot.produced_at_utc_ms = spring_due;
        let next_due = timestamp_ms("2024-03-17T06:30:00Z");
        assert!(fixture.worker.advance_deadlines(next_due).unwrap());
        assert_eq!(
            fixture.worker.snapshot.as_ref().unwrap().occurrences.len(),
            2,
            "the next selected civil Sunday must create exactly one new occurrence"
        );
    }

    #[test]
    fn restart_auto_silences_elapsed_alarm_and_durably_stops_audio() {
        let mut fixture = Fixture::new();
        fixture.worker.tick_once().unwrap();
        let due_at = NOW + 5_000;
        fixture.publish(&fixture.alarm_command("auto-silence", 1, due_at));
        fixture.worker.tick_once().unwrap();

        fixture.clock.0.store(due_at, Ordering::Relaxed);
        fixture.worker.tick_once().unwrap();
        let ringing = fixture.worker.snapshot.as_ref().unwrap().occurrences[0].clone();
        assert_eq!(ringing.phase, ClockOccurrencePhase::Ringing);

        let auto_silence_at = due_at
            + i64::from(
                fixture
                    .worker
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .settings
                    .auto_silence_minutes,
            ) * 60_000;
        fixture.clock.0.store(auto_silence_at, Ordering::Relaxed);
        let mut restarted = ClockWorker {
            node_id: "seat-1".into(),
            bus_root: Some(fixture.bus.clone()),
            poll: POLL,
            clock: fixture.clock.clone(),
            store: Arc::new(SqliteClockStore {
                db_path: fixture.db.clone(),
            }),
            signer: fixture.worker.signer.clone(),
            command_signing_key: fixture.worker.command_signing_key.clone(),
            approved_peer_ids: fixture.worker.approved_peer_ids.clone(),
            blocked_origin_ids: fixture.worker.blocked_origin_ids.clone(),
            disabled_schedule_ids: fixture.worker.disabled_schedule_ids.clone(),
            music_signing_seed: fixture.worker.music_signing_seed,
            snapshot: None,
            action_cursor: None,
            active_bus_identity: None,
            published_once: false,
            audio_status_cursor: None,
            audio_last_sent_ms: BTreeMap::new(),
            peer_last_sent_ms: BTreeMap::new(),
        };
        restarted.tick_once().unwrap();

        let snapshot = restarted.snapshot.as_ref().unwrap();
        let missed = &snapshot.occurrences[0];
        assert_eq!(missed.phase, ClockOccurrencePhase::Missed);
        assert_eq!(missed.acknowledgement, None);
        assert!(missed
            .targets
            .iter()
            .all(|target| target.disposition == ClockTargetDisposition::Missed));
        let durable: String = Connection::open(&fixture.db)
            .unwrap()
            .query_row(
                "SELECT snapshot_json FROM clock_authority WHERE node_id = 'seat-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<ClockSnapshotV1>(&durable).unwrap(),
            *snapshot
        );

        let stops = fixture
            .audio_messages()
            .into_iter()
            .map(|body| serde_json::from_str::<ClockAudioRequestV1>(&body).unwrap())
            .filter_map(|request| match request.body {
                ClockAudioActionV1::Stop { acknowledgement_id } => Some((
                    request.occurrence_id,
                    request.occurrence_generation,
                    acknowledgement_id,
                )),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            stops,
            vec![(
                ringing.occurrence_id.clone(),
                ringing.revision,
                auto_silence_acknowledgement_id(&ringing),
            )]
        );
    }

    #[test]
    fn restart_persists_elapsed_timer_before_bus_recovery() {
        let mut fixture = Fixture::new();
        fixture.worker.tick_once().unwrap();
        let due_at = NOW + 5_000;
        fixture.publish(&fixture.timer_command("late-bus", 1, due_at));
        fixture.worker.tick_once().unwrap();

        fixture.clock.0.store(due_at, Ordering::Relaxed);
        let unavailable_bus = fixture._temp.path().join("unavailable-bus");
        fs::write(&unavailable_bus, b"not a directory").unwrap();
        let mut restarted = ClockWorker {
            node_id: "seat-1".into(),
            bus_root: Some(unavailable_bus),
            poll: POLL,
            clock: fixture.clock.clone(),
            store: Arc::new(SqliteClockStore {
                db_path: fixture.db.clone(),
            }),
            signer: fixture.worker.signer.clone(),
            command_signing_key: fixture.worker.command_signing_key.clone(),
            approved_peer_ids: fixture.worker.approved_peer_ids.clone(),
            blocked_origin_ids: fixture.worker.blocked_origin_ids.clone(),
            disabled_schedule_ids: fixture.worker.disabled_schedule_ids.clone(),
            music_signing_seed: fixture.worker.music_signing_seed,
            snapshot: None,
            action_cursor: None,
            active_bus_identity: None,
            published_once: false,
            audio_status_cursor: None,
            audio_last_sent_ms: BTreeMap::new(),
            peer_last_sent_ms: BTreeMap::new(),
        };
        assert!(restarted.open_bus_transaction().is_err());
        restarted.ensure_loaded().unwrap();

        let snapshot = restarted.snapshot.as_ref().unwrap();
        let ClockScheduleKindV1::Timer(timer) = &snapshot.schedules[0].schedule else {
            panic!("recovered schedule must remain a timer");
        };
        assert_eq!(timer.phase, ClockTimerPhase::Expired);
        assert_eq!(snapshot.occurrences[0].phase, ClockOccurrencePhase::Ringing);
        let durable = restarted.store.load("seat-1").unwrap().unwrap();
        assert_eq!(durable.revision, snapshot.revision);
        assert_eq!(
            ClockSnapshotV1::from_persisted_json_at(
                durable.snapshot_json.as_bytes(),
                &restarted.context(due_at),
            )
            .unwrap(),
            *snapshot
        );
        let pending = restarted.store.pending_audio("seat-1").unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending[0].occurrence_id,
            snapshot.occurrences[0].occurrence_id
        );
    }

    #[test]
    fn removing_a_ringing_schedule_atomically_stops_audio_and_persists_the_terminal_occurrence() {
        let mut fixture = Fixture::new();
        fixture.worker.tick_once().unwrap();
        let due_at = NOW + 5_000;
        fixture.publish(&fixture.alarm_command("remove-ringing", 1, due_at));
        fixture.worker.tick_once().unwrap();

        fixture.clock.0.store(due_at, Ordering::Relaxed);
        fixture.worker.tick_once().unwrap();
        let ringing = fixture.worker.snapshot.as_ref().unwrap().occurrences[0].clone();
        assert_eq!(ringing.phase, ClockOccurrencePhase::Ringing);

        let remove_request_id = "remove-ringing-schedule";
        fixture.publish(&fixture.remove_schedule_command(
            remove_request_id,
            fixture.worker.snapshot.as_ref().unwrap().revision,
            &ringing.schedule_id,
            due_at,
        ));
        fixture.worker.tick_once().unwrap();

        let snapshot = fixture.worker.snapshot.as_ref().unwrap();
        assert!(snapshot.schedules.is_empty());
        let stopped = snapshot
            .occurrences
            .iter()
            .find(|occurrence| occurrence.occurrence_id == ringing.occurrence_id)
            .unwrap();
        assert_eq!(stopped.phase, ClockOccurrencePhase::Stopped);
        assert!(stopped
            .targets
            .iter()
            .all(|target| target.disposition == ClockTargetDisposition::Stopped));
        let acknowledgement = stopped.acknowledgement.as_ref().unwrap();
        assert!(acknowledgement.stop);
        assert_eq!(acknowledgement.actor_node_id, "seat-1");
        assert_eq!(acknowledgement.actor_clock, snapshot.revision);
        assert_eq!(acknowledgement.acknowledged_at_utc_ms, due_at);
        assert_eq!(
            acknowledgement.acknowledgement_id,
            schedule_removal_acknowledgement_id(&ringing.occurrence_id, remove_request_id)
        );

        let durable = fixture.worker.store.load("seat-1").unwrap().unwrap();
        assert_eq!(durable.revision, snapshot.revision);
        assert_eq!(
            ClockSnapshotV1::from_persisted_json_at(
                durable.snapshot_json.as_bytes(),
                &fixture.worker.context(due_at),
            )
            .unwrap(),
            *snapshot
        );

        let stops = fixture
            .audio_messages()
            .into_iter()
            .map(|body| serde_json::from_str::<ClockAudioRequestV1>(&body).unwrap())
            .filter_map(|request| match request.body {
                ClockAudioActionV1::Stop { acknowledgement_id } => {
                    Some((request.occurrence_id, acknowledgement_id))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            stops,
            vec![(
                ringing.occurrence_id.clone(),
                schedule_removal_acknowledgement_id(&ringing.occurrence_id, remove_request_id),
            )]
        );
    }

    fn signed_ack_for(
        key: &SigningKey,
        actor: &str,
        request_id: &str,
        expected_revision: u64,
        occurrence: &ClockOccurrenceV1,
        actor_clock: u64,
        stop: bool,
        now_ms: i64,
    ) -> ClockCommandV1 {
        ClockCommandV1 {
            schema_version: CLOCK_SCHEMA_VERSION,
            request_id: request_id.into(),
            origin_node_id: actor.into(),
            expected_revision,
            issued_at_utc_ms: now_ms - 1,
            expires_at_utc_ms: now_ms - 1 + MAX_CLOCK_COMMAND_TTL_MS,
            body: ClockCommandKindV1::Acknowledge {
                occurrence_id: occurrence.occurrence_id.clone(),
                acknowledgement: ClockAcknowledgementV1 {
                    acknowledgement_id: request_id.into(),
                    global_event_id: occurrence.global_event_id.clone(),
                    actor_node_id: actor.into(),
                    actor_clock,
                    acknowledged_at_utc_ms: now_ms,
                    stop,
                },
            },
            signer_id: String::new(),
            signature: String::new(),
        }
        .sign(
            "clock-mesh-key",
            key,
            &ClockValidationContext {
                wall_utc_ms: now_ms,
                monotonic_ms: 1,
                zone_exists: &zone_exists,
            },
        )
        .unwrap()
    }

    fn publish_to(bus: &Path, target: &str, command: &ClockCommandV1) {
        Persist::open(bus.to_path_buf())
            .unwrap()
            .write(
                &clock_command_topic(target).unwrap(),
                Priority::Default,
                None,
                Some(&serde_json::to_string(command).unwrap()),
            )
            .unwrap();
    }

    #[test]
    fn elapsed_restart_durably_emits_exact_signed_audio_and_replays_until_receipt() {
        let mut fixture = Fixture::new();
        fixture.worker.tick_once().unwrap();
        fixture.publish(&fixture.timer_command("req-1", 1, NOW + 5_000));
        fixture.worker.tick_once().unwrap();
        assert_eq!(fixture.worker.snapshot.as_ref().unwrap().revision, 2);
        fixture.publish(&fixture.alarm_command("alarm-1", 2, NOW + 5_000));
        fixture.worker.tick_once().unwrap();
        assert_eq!(fixture.worker.snapshot.as_ref().unwrap().revision, 3);

        fixture.clock.0.store(NOW + 10_000, Ordering::Relaxed);
        let mut restarted = ClockWorker {
            node_id: "seat-1".into(),
            bus_root: Some(fixture.bus.clone()),
            poll: POLL,
            clock: fixture.clock.clone(),
            store: Arc::new(SqliteClockStore {
                db_path: fixture.db.clone(),
            }),
            signer: fixture.worker.signer.clone(),
            command_signing_key: fixture.worker.command_signing_key.clone(),
            approved_peer_ids: fixture.worker.approved_peer_ids.clone(),
            blocked_origin_ids: fixture.worker.blocked_origin_ids.clone(),
            disabled_schedule_ids: fixture.worker.disabled_schedule_ids.clone(),
            music_signing_seed: fixture.worker.music_signing_seed,
            snapshot: None,
            action_cursor: None,
            active_bus_identity: None,
            published_once: false,
            audio_status_cursor: None,
            audio_last_sent_ms: BTreeMap::new(),
            peer_last_sent_ms: BTreeMap::new(),
        };
        restarted.tick_once().unwrap();
        let snapshot = restarted.snapshot.as_ref().unwrap();
        assert_eq!(snapshot.revision, 4);
        assert_eq!(snapshot.occurrences.len(), 2);
        assert!(snapshot
            .occurrences
            .iter()
            .all(|occurrence| occurrence.phase == ClockOccurrencePhase::Ringing));
        let connection = Connection::open(&fixture.db).unwrap();
        let durable: String = connection
            .query_row(
                "SELECT snapshot_json FROM clock_authority WHERE node_id = 'seat-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<ClockSnapshotV1>(&durable).unwrap(),
            *snapshot
        );

        let first_emission = fixture.audio_messages();
        assert_eq!(first_emission.len(), 2);
        let music_key = ed25519_dalek::SigningKey::from_bytes(&[7; 32]).verifying_key();
        let mut first_ids = Vec::new();
        for body in &first_emission {
            music_auth::verify_request(
                body,
                MusicAuthContext {
                    verb: CLOCK_AUDIO_AUTH_VERB,
                    node: "seat-1",
                    target: CLOCK_AUDIO_AUTH_TARGET,
                },
                &music_key,
            )
            .unwrap();
            let request = ClockAudioRequestV1::from_json_at(body.as_bytes(), NOW + 10_000).unwrap();
            let occurrence = snapshot
                .occurrences
                .iter()
                .find(|value| value.occurrence_id == request.occurrence_id)
                .unwrap();
            assert_eq!(request.global_event_id, occurrence.global_event_id);
            assert_eq!(request.occurrence_generation, occurrence.revision);
            assert!(matches!(request.body, ClockAudioActionV1::Start { .. }));
            first_ids.push(request.request_id);
        }
        first_ids.sort();

        let mut replayed = ClockWorker {
            node_id: "seat-1".into(),
            bus_root: Some(fixture.bus.clone()),
            poll: POLL,
            clock: fixture.clock.clone(),
            store: Arc::new(SqliteClockStore {
                db_path: fixture.db.clone(),
            }),
            signer: fixture.worker.signer.clone(),
            command_signing_key: fixture.worker.command_signing_key.clone(),
            approved_peer_ids: fixture.worker.approved_peer_ids.clone(),
            blocked_origin_ids: fixture.worker.blocked_origin_ids.clone(),
            disabled_schedule_ids: fixture.worker.disabled_schedule_ids.clone(),
            music_signing_seed: Some([7; 32]),
            snapshot: None,
            action_cursor: None,
            active_bus_identity: None,
            published_once: false,
            audio_status_cursor: None,
            audio_last_sent_ms: BTreeMap::new(),
            peer_last_sent_ms: BTreeMap::new(),
        };
        replayed.tick_once().unwrap();
        let replay_emission = fixture.audio_messages();
        assert_eq!(replay_emission.len(), 4);
        let mut replay_ids = replay_emission[2..]
            .iter()
            .map(|body| {
                ClockAudioRequestV1::from_json_at(body.as_bytes(), NOW + 10_000)
                    .unwrap()
                    .request_id
            })
            .collect::<Vec<_>>();
        replay_ids.sort();
        assert_eq!(replay_ids, first_ids);

        let persist = Persist::open(fixture.bus.clone()).unwrap();
        for body in &replay_emission[2..] {
            let request = ClockAudioRequestV1::from_json_at(body.as_bytes(), NOW + 10_000).unwrap();
            let status = ClockAudioStatusV1 {
                schema_version: CLOCK_SCHEMA_VERSION,
                request_id: request.request_id,
                occurrence_id: request.occurrence_id,
                global_event_id: request.global_event_id,
                occurrence_generation: request.occurrence_generation,
                observed_at_utc_ms: NOW + 10_000,
                phase: ClockAudioPlaybackPhase::PlayingBundled,
                provider_status: ClockAudioProviderStatus::NotApplicable,
                fallback_tone_id: None,
                acknowledgement_id: None,
                reason_code: None,
            };
            persist
                .write(
                    &clock_audio_status_topic("seat-1").unwrap(),
                    Priority::Default,
                    None,
                    Some(&serde_json::to_string(&status).unwrap()),
                )
                .unwrap();
        }
        replayed.tick_once().unwrap();
        let pending: i64 = Connection::open(&fixture.db)
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM clock_audio_outbox WHERE acknowledged_at_ms IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pending, 0);
        assert_eq!(fixture.audio_messages().len(), 4);

        let ringing = replayed.snapshot.as_ref().unwrap().occurrences[0].clone();
        let ringing_generation = ringing.revision;
        let snooze_minutes = replayed.snapshot.as_ref().unwrap().settings.snooze_minutes;
        fixture.publish(&fixture.snooze_command("snooze-command-1", 4, &ringing, NOW + 10_000));
        replayed.tick_once().unwrap();
        let audio = fixture.audio_messages();
        assert_eq!(audio.len(), 5);
        let snooze =
            ClockAudioRequestV1::from_json_at(audio.last().unwrap().as_bytes(), NOW + 10_000)
                .unwrap();
        assert_eq!(snooze.occurrence_id, ringing.occurrence_id);
        assert_eq!(snooze.global_event_id, ringing.global_event_id);
        assert_eq!(snooze.occurrence_generation, ringing_generation);
        assert!(matches!(
            snooze.body,
            ClockAudioActionV1::Snooze {
                ref acknowledgement_id,
                resume_at_utc_ms,
            } if acknowledgement_id == "ack-snooze-1"
                && resume_at_utc_ms
                    == NOW + 10_000 + i64::from(snooze_minutes) * 60_000
        ));
        let pending: i64 = Connection::open(&fixture.db)
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM clock_audio_outbox WHERE acknowledged_at_ms IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pending, 1, "snooze must be durable before publication");
    }

    #[test]
    fn restart_reasserts_acknowledged_ringing_audio_with_same_effect_id() {
        let mut fixture = Fixture::new();
        fixture.worker.tick_once().unwrap();
        fixture.publish(&fixture.timer_command("restart-ringing", 1, NOW + 5_000));
        fixture.worker.tick_once().unwrap();

        fixture.clock.0.store(NOW + 10_000, Ordering::Relaxed);
        fixture.worker.tick_once().unwrap();
        let first_audio = fixture.audio_messages();
        assert_eq!(first_audio.len(), 1);
        let first_request =
            ClockAudioRequestV1::from_json_at(first_audio[0].as_bytes(), NOW + 10_000).unwrap();
        assert!(matches!(
            first_request.body,
            ClockAudioActionV1::Start { .. }
        ));

        let status = ClockAudioStatusV1 {
            schema_version: CLOCK_SCHEMA_VERSION,
            request_id: first_request.request_id.clone(),
            occurrence_id: first_request.occurrence_id.clone(),
            global_event_id: first_request.global_event_id.clone(),
            occurrence_generation: first_request.occurrence_generation,
            observed_at_utc_ms: NOW + 10_000,
            phase: ClockAudioPlaybackPhase::PlayingBundled,
            provider_status: ClockAudioProviderStatus::NotApplicable,
            fallback_tone_id: None,
            acknowledgement_id: None,
            reason_code: None,
        };
        Persist::open(fixture.bus.clone())
            .unwrap()
            .write(
                &clock_audio_status_topic("seat-1").unwrap(),
                Priority::Default,
                None,
                Some(&serde_json::to_string(&status).unwrap()),
            )
            .unwrap();
        fixture.worker.tick_once().unwrap();
        assert_eq!(
            Connection::open(&fixture.db)
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM clock_audio_outbox WHERE node_id = 'seat-1'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0,
            "the original Start must be acknowledged before recovery is exercised"
        );

        let restart_now = NOW + 10_000 + MAX_CLOCK_AUDIO_REQUEST_TTL_MS + 1;
        fixture.clock.0.store(restart_now, Ordering::Relaxed);

        let mut restarted = ClockWorker {
            node_id: "seat-1".into(),
            bus_root: Some(fixture.bus.clone()),
            poll: POLL,
            clock: fixture.clock.clone(),
            store: Arc::new(SqliteClockStore {
                db_path: fixture.db.clone(),
            }),
            signer: fixture.worker.signer.clone(),
            command_signing_key: fixture.worker.command_signing_key.clone(),
            approved_peer_ids: fixture.worker.approved_peer_ids.clone(),
            blocked_origin_ids: fixture.worker.blocked_origin_ids.clone(),
            disabled_schedule_ids: fixture.worker.disabled_schedule_ids.clone(),
            music_signing_seed: fixture.worker.music_signing_seed,
            snapshot: None,
            action_cursor: None,
            active_bus_identity: None,
            published_once: false,
            audio_status_cursor: None,
            audio_last_sent_ms: BTreeMap::new(),
            peer_last_sent_ms: BTreeMap::new(),
        };
        restarted.tick_once().unwrap();

        let recovered_audio = fixture.audio_messages();
        assert_eq!(recovered_audio.len(), 2);
        let recovered_request = ClockAudioRequestV1::from_json_at(
            recovered_audio.last().unwrap().as_bytes(),
            restart_now,
        )
        .unwrap();
        assert_eq!(recovered_request.issued_at_utc_ms, restart_now);
        assert_eq!(
            recovered_request.expires_at_utc_ms,
            restart_now + MAX_CLOCK_AUDIO_REQUEST_TTL_MS
        );
        assert_eq!(recovered_request.request_id, first_request.request_id);
        assert_eq!(recovered_request.occurrence_id, first_request.occurrence_id);
        assert_eq!(
            recovered_request.occurrence_generation,
            first_request.occurrence_generation
        );
        assert!(matches!(
            recovered_request.body,
            ClockAudioActionV1::Start { .. }
        ));
        assert_eq!(
            restarted.snapshot.as_ref().unwrap().occurrences[0].phase,
            ClockOccurrencePhase::Ringing
        );
    }

    #[test]
    fn ringing_occurrence_cannot_inherit_replacement_schedule_authority_after_restart() {
        let mut fixture = Fixture::new();
        fixture.worker.tick_once().unwrap();
        fixture.publish(&fixture.alarm_command("frozen-ring", 1, NOW + 5_000));
        fixture.worker.tick_once().unwrap();

        let ringing_at = NOW + 10_000;
        fixture.clock.0.store(ringing_at, Ordering::Relaxed);
        fixture.worker.tick_once().unwrap();
        let ringing = fixture.worker.snapshot.as_ref().unwrap().clone();
        assert_eq!(ringing.revision, 3);
        assert_eq!(ringing.occurrences[0].phase, ClockOccurrencePhase::Ringing);

        let mut replacement = ringing.schedules[0].clone();
        replacement.revision += 1;
        let ClockScheduleKindV1::Alarm(alarm) = &mut replacement.schedule else {
            panic!("fixture schedule changed kind");
        };
        alarm.sound = ClockAudioRef::Bundled {
            tone_id: "substituted-tone".into(),
        };
        let command = ClockCommandV1 {
            schema_version: CLOCK_SCHEMA_VERSION,
            request_id: "replace-ringing-schedule".into(),
            origin_node_id: "seat-1".into(),
            expected_revision: ringing.revision,
            issued_at_utc_ms: ringing_at,
            expires_at_utc_ms: ringing_at + MAX_CLOCK_COMMAND_TTL_MS,
            body: ClockCommandKindV1::UpsertSchedule {
                schedule: replacement,
            },
            signer_id: String::new(),
            signature: String::new(),
        }
        .sign(
            "seat-1-key",
            &fixture.signing_key,
            &fixture.worker.context(ringing_at),
        )
        .unwrap();
        fixture.publish(&command);
        fixture.worker.tick_once().unwrap();

        let retained = fixture.worker.snapshot.as_ref().unwrap();
        assert_eq!(retained.revision, ringing.revision);
        let ClockScheduleKindV1::Alarm(alarm) = &retained.schedules[0].schedule else {
            panic!("ringing alarm changed kind");
        };
        assert_eq!(
            alarm.sound,
            ClockAudioRef::Bundled {
                tone_id: "bell".into()
            }
        );

        let recovered = clock_audio_recovery_requests(retained, ringing_at, "seat-1").unwrap();
        assert_eq!(recovered.len(), 1);
        let request: ClockAudioRequestV1 =
            serde_json::from_str(&recovered[0].request_json).unwrap();
        assert!(matches!(
            request.body,
            ClockAudioActionV1::Start {
                audio: ClockAudioRef::Bundled { ref tone_id },
                ..
            } if tone_id == "bell"
        ));
    }

    #[test]
    fn alarm_banner_snooze_and_stop_are_atomic_bounded_and_replay_closed() {
        let mut fixture = Fixture::new();
        fixture.worker.tick_once().unwrap();
        fixture.publish(&fixture.alarm_command("banner-alarm", 1, NOW + 5_000));
        fixture.worker.tick_once().unwrap();

        let snooze_time = NOW + 10_000;
        fixture.clock.0.store(snooze_time, Ordering::Relaxed);
        fixture.worker.tick_once().unwrap();
        let ringing = fixture.worker.snapshot.as_ref().unwrap().occurrences[0].clone();
        assert_eq!(fixture.worker.snapshot.as_ref().unwrap().revision, 3);
        assert_eq!(ringing.phase, ClockOccurrencePhase::Ringing);
        assert_eq!(ringing.revision, 3);

        let snooze = fixture.alarm_ack_command(
            "alarm-banner-snooze",
            "alarm-banner-snooze-ack",
            3,
            &ringing,
            snooze_time,
            false,
        );
        fixture.publish(&snooze);
        fixture.worker.tick_once().unwrap();
        let snoozed_snapshot = fixture.worker.snapshot.as_ref().unwrap().clone();
        assert_eq!(snoozed_snapshot.revision, 4);
        let source = snoozed_snapshot
            .occurrences
            .iter()
            .find(|value| value.occurrence_id == ringing.occurrence_id)
            .unwrap();
        assert_eq!(source.phase, ClockOccurrencePhase::Snoozed);
        let source_acknowledgement = source.acknowledgement.as_ref().unwrap();
        assert_eq!(
            source_acknowledgement.global_event_id,
            ringing.global_event_id
        );
        assert_eq!(source_acknowledgement.actor_clock, 4);
        let next_id = snooze_occurrence_id(source, source_acknowledgement);
        let next = snoozed_snapshot
            .occurrences
            .iter()
            .find(|value| value.occurrence_id == next_id)
            .unwrap();
        let expected_due =
            snooze_time + i64::from(snoozed_snapshot.settings.snooze_minutes) * 60_000;
        assert_eq!(next.phase, ClockOccurrencePhase::Scheduled);
        assert_eq!(next.due_at_utc_ms, expected_due);
        assert_ne!(next.global_event_id, ringing.global_event_id);
        assert_eq!(snoozed_snapshot.occurrences.len(), 2);

        let audio = fixture.audio_messages();
        let snooze_audio =
            ClockAudioRequestV1::from_json_at(audio.last().unwrap().as_bytes(), snooze_time)
                .unwrap();
        assert_eq!(snooze_audio.occurrence_id, ringing.occurrence_id);
        assert_eq!(snooze_audio.global_event_id, ringing.global_event_id);
        assert_eq!(snooze_audio.occurrence_generation, 3);
        assert!(matches!(
            snooze_audio.body,
            ClockAudioActionV1::Snooze {
                ref acknowledgement_id,
                resume_at_utc_ms,
            } if acknowledgement_id == "alarm-banner-snooze-ack"
                && resume_at_utc_ms == expected_due
        ));
        let connection = Connection::open(&fixture.db).unwrap();
        let (durable_snapshot, ledger_revision, fingerprint_len, outbox_rows):
            (String, i64, i64, i64) = connection
            .query_row(
                "SELECT a.snapshot_json, l.revision, length(l.request_fingerprint), (SELECT COUNT(*) FROM clock_audio_outbox o WHERE o.node_id = a.node_id AND o.occurrence_id = ?3 AND o.global_event_id = ?4 AND o.occurrence_generation = 3) FROM clock_authority a JOIN clock_request_ledger l ON l.node_id = a.node_id WHERE a.node_id = ?1 AND l.request_id = ?2",
                (
                    "seat-1",
                    "alarm-banner-snooze",
                    &ringing.occurrence_id,
                    &ringing.global_event_id,
                ),
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(ledger_revision, 4);
        assert_eq!(fingerprint_len, 64);
        assert_eq!(
            outbox_rows, 2,
            "the original Start and atomic Snooze finish share the ringing generation"
        );
        assert_eq!(
            serde_json::from_str::<ClockSnapshotV1>(&durable_snapshot).unwrap(),
            snoozed_snapshot
        );

        fixture.publish(&snooze);
        fixture.worker.tick_once().unwrap();
        assert_eq!(fixture.worker.snapshot.as_ref().unwrap().revision, 4);
        assert_eq!(fixture.audio_messages().len(), audio.len());

        let mut conflicting_settings = fixture.worker.snapshot.as_ref().unwrap().settings.clone();
        conflicting_settings.alarm_crescendo = !conflicting_settings.alarm_crescendo;
        let conflict = ClockCommandV1 {
            schema_version: CLOCK_SCHEMA_VERSION,
            request_id: "alarm-banner-snooze".into(),
            origin_node_id: "seat-1".into(),
            expected_revision: 4,
            issued_at_utc_ms: snooze_time,
            expires_at_utc_ms: snooze_time + MAX_CLOCK_COMMAND_TTL_MS,
            body: ClockCommandKindV1::SetSettings {
                settings: conflicting_settings,
            },
            signer_id: String::new(),
            signature: String::new(),
        }
        .sign(
            "seat-1-key",
            &fixture.signing_key,
            &fixture.worker.context(snooze_time),
        )
        .unwrap();
        let settings_before = fixture.worker.snapshot.as_ref().unwrap().settings.clone();
        fixture.publish(&conflict);
        fixture.worker.tick_once().unwrap();
        assert_eq!(fixture.worker.snapshot.as_ref().unwrap().revision, 4);
        assert_eq!(
            fixture.worker.snapshot.as_ref().unwrap().settings,
            settings_before
        );

        let persist = Persist::open(fixture.bus.clone()).unwrap();
        for body in &audio {
            let request = ClockAudioRequestV1::from_json_at(body.as_bytes(), snooze_time).unwrap();
            let status = ClockAudioStatusV1 {
                schema_version: CLOCK_SCHEMA_VERSION,
                request_id: request.request_id,
                occurrence_id: request.occurrence_id,
                global_event_id: request.global_event_id,
                occurrence_generation: request.occurrence_generation,
                observed_at_utc_ms: snooze_time,
                phase: ClockAudioPlaybackPhase::PlayingBundled,
                provider_status: ClockAudioProviderStatus::NotApplicable,
                fallback_tone_id: None,
                acknowledgement_id: None,
                reason_code: None,
            };
            persist
                .write(
                    &clock_audio_status_topic("seat-1").unwrap(),
                    Priority::Default,
                    None,
                    Some(&serde_json::to_string(&status).unwrap()),
                )
                .unwrap();
        }
        fixture.worker.tick_once().unwrap();
        let pending: i64 = Connection::open(&fixture.db)
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM clock_audio_outbox WHERE acknowledged_at_ms IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pending, 0, "music receipts clear the atomic audio outbox");

        fixture.clock.0.store(expected_due, Ordering::Relaxed);
        fixture.worker.tick_once().unwrap();
        assert_eq!(fixture.worker.snapshot.as_ref().unwrap().revision, 5);
        let rerung = fixture
            .worker
            .snapshot
            .as_ref()
            .unwrap()
            .occurrences
            .iter()
            .find(|value| value.occurrence_id == next_id)
            .unwrap()
            .clone();
        assert_eq!(rerung.phase, ClockOccurrencePhase::Ringing);
        assert_eq!(rerung.revision, 5);

        let stop = fixture.alarm_ack_command(
            "alarm-banner-stop",
            "alarm-banner-stop-ack",
            5,
            &rerung,
            expected_due,
            true,
        );
        fixture.publish(&stop);
        fixture.worker.tick_once().unwrap();
        let stopped_snapshot = fixture.worker.snapshot.as_ref().unwrap().clone();
        assert_eq!(stopped_snapshot.revision, 6);
        let stopped = stopped_snapshot
            .occurrences
            .iter()
            .find(|value| value.occurrence_id == next_id)
            .unwrap();
        assert_eq!(stopped.phase, ClockOccurrencePhase::Stopped);
        assert_eq!(
            stopped_snapshot.occurrences.len(),
            2,
            "Stop does not reschedule"
        );
        let stop_audio = ClockAudioRequestV1::from_json_at(
            fixture.audio_messages().last().unwrap().as_bytes(),
            expected_due,
        )
        .unwrap();
        assert_eq!(stop_audio.occurrence_id, next_id);
        assert_eq!(stop_audio.global_event_id, rerung.global_event_id);
        assert_eq!(stop_audio.occurrence_generation, 5);
        assert!(matches!(
            stop_audio.body,
            ClockAudioActionV1::Stop { ref acknowledgement_id }
                if acknowledgement_id == "alarm-banner-stop-ack"
        ));

        let audio_count = fixture.audio_messages().len();
        fixture.publish(&stop);
        fixture.worker.tick_once().unwrap();
        assert_eq!(fixture.worker.snapshot.as_ref().unwrap().revision, 6);
        assert_eq!(fixture.audio_messages().len(), audio_count);

        let stale = fixture.alarm_ack_command(
            "alarm-banner-stale",
            "alarm-banner-stale-ack",
            6,
            &ringing,
            expected_due,
            true,
        );
        fixture.publish(&stale);
        fixture.worker.tick_once().unwrap();
        assert_eq!(fixture.worker.snapshot.as_ref().unwrap().revision, 6);

        let mut wrong_event = fixture.alarm_ack_command(
            "alarm-banner-wrong-event",
            "alarm-banner-wrong-event-ack",
            6,
            &rerung,
            expected_due,
            true,
        );
        let ClockCommandKindV1::Acknowledge {
            acknowledgement, ..
        } = &mut wrong_event.body
        else {
            panic!("fixture acknowledgement command changed kind");
        };
        acknowledgement.global_event_id = "wrong-global-event".into();
        wrong_event = wrong_event
            .sign(
                "seat-1-key",
                &fixture.signing_key,
                &fixture.worker.context(expected_due),
            )
            .unwrap();
        fixture.publish(&wrong_event);
        fixture.worker.tick_once().unwrap();
        assert_eq!(fixture.worker.snapshot.as_ref().unwrap().revision, 6);

        let mut unsigned = stale;
        unsigned.request_id = "alarm-banner-unsigned".into();
        unsigned.signature.clear();
        fixture.publish(&unsigned);
        fixture.worker.tick_once().unwrap();
        assert_eq!(fixture.worker.snapshot.as_ref().unwrap().revision, 6);
        assert_eq!(
            fixture.worker.snapshot.as_ref().unwrap().occurrences.len(),
            2
        );
    }

    #[test]
    fn disabled_alarm_cannot_ring_a_preexisting_snooze_generation() {
        let mut fixture = Fixture::new();
        fixture.worker.tick_once().unwrap();
        fixture.publish(&fixture.alarm_command("disable-snooze", 1, NOW + 5_000));
        fixture.worker.tick_once().unwrap();

        let snooze_time = NOW + 10_000;
        fixture.clock.0.store(snooze_time, Ordering::Relaxed);
        fixture.worker.tick_once().unwrap();
        let ringing = fixture.worker.snapshot.as_ref().unwrap().occurrences[0].clone();
        fixture.publish(&fixture.snooze_command("disable-snooze-ack", 3, &ringing, snooze_time));
        fixture.worker.tick_once().unwrap();

        let snoozed = fixture.worker.snapshot.as_ref().unwrap().clone();
        let source = snoozed
            .occurrences
            .iter()
            .find(|occurrence| occurrence.occurrence_id == ringing.occurrence_id)
            .unwrap();
        let child_id = snooze_occurrence_id(source, source.acknowledgement.as_ref().unwrap());
        let child_due = snoozed
            .occurrences
            .iter()
            .find(|occurrence| occurrence.occurrence_id == child_id)
            .unwrap()
            .due_at_utc_ms;

        let persist = Persist::open(fixture.bus.clone()).unwrap();
        for body in fixture.audio_messages() {
            let request = ClockAudioRequestV1::from_json_at(body.as_bytes(), snooze_time).unwrap();
            let status = ClockAudioStatusV1 {
                schema_version: CLOCK_SCHEMA_VERSION,
                request_id: request.request_id,
                occurrence_id: request.occurrence_id,
                global_event_id: request.global_event_id,
                occurrence_generation: request.occurrence_generation,
                observed_at_utc_ms: snooze_time,
                phase: ClockAudioPlaybackPhase::PlayingBundled,
                provider_status: ClockAudioProviderStatus::NotApplicable,
                fallback_tone_id: None,
                acknowledgement_id: None,
                reason_code: None,
            };
            persist
                .write(
                    &clock_audio_status_topic("seat-1").unwrap(),
                    Priority::Default,
                    None,
                    Some(&serde_json::to_string(&status).unwrap()),
                )
                .unwrap();
        }

        let disable = ClockCommandV1 {
            schema_version: CLOCK_SCHEMA_VERSION,
            request_id: "disable-snooze-parent".into(),
            origin_node_id: "seat-1".into(),
            expected_revision: 4,
            issued_at_utc_ms: snooze_time,
            expires_at_utc_ms: snooze_time + MAX_CLOCK_COMMAND_TTL_MS,
            body: ClockCommandKindV1::SetScheduleEnabled {
                schedule_id: "alarm-disable-snooze".into(),
                enabled: false,
            },
            signer_id: String::new(),
            signature: String::new(),
        }
        .sign(
            "seat-1-key",
            &fixture.signing_key,
            &fixture.worker.context(snooze_time),
        )
        .unwrap();
        fixture.publish(&disable);
        fixture.worker.tick_once().unwrap();

        let disabled = fixture.worker.snapshot.as_ref().unwrap();
        assert_eq!(disabled.revision, 5);
        assert!(!matches!(
            &disabled.schedules[0].schedule,
            ClockScheduleKindV1::Alarm(ClockAlarmV1 { enabled: true, .. })
        ));
        assert!(
            disabled
                .occurrences
                .iter()
                .all(|occurrence| occurrence.occurrence_id != child_id),
            "disabling the parent must cancel its scheduled snooze generation"
        );

        fixture.clock.0.store(child_due, Ordering::Relaxed);
        fixture.worker.tick_once().unwrap();
        let after_deadline = fixture.worker.snapshot.as_ref().unwrap();
        assert_eq!(after_deadline.revision, 5);
        assert!(after_deadline.occurrences.iter().all(|occurrence| {
            occurrence.occurrence_id != child_id
                && occurrence.phase != ClockOccurrencePhase::Ringing
        }));
    }

    #[test]
    fn disabling_a_ringing_alarm_atomically_stops_its_exact_audio_generation() {
        let mut fixture = Fixture::new();
        fixture.worker.tick_once().unwrap();
        fixture.publish(&fixture.alarm_command("disable-ringing", 1, NOW + 5_000));
        fixture.worker.tick_once().unwrap();

        let disable_at = NOW + 10_000;
        fixture.clock.0.store(disable_at, Ordering::Relaxed);
        fixture.worker.tick_once().unwrap();
        let ringing = fixture.worker.snapshot.as_ref().unwrap().occurrences[0].clone();
        assert_eq!(ringing.phase, ClockOccurrencePhase::Ringing);
        assert_eq!(ringing.revision, 3);

        let request_id = "disable-active-alarm";
        let disable = ClockCommandV1 {
            schema_version: CLOCK_SCHEMA_VERSION,
            request_id: request_id.into(),
            origin_node_id: "seat-1".into(),
            expected_revision: 3,
            issued_at_utc_ms: disable_at,
            expires_at_utc_ms: disable_at + MAX_CLOCK_COMMAND_TTL_MS,
            body: ClockCommandKindV1::SetScheduleEnabled {
                schedule_id: ringing.schedule_id.clone(),
                enabled: false,
            },
            signer_id: String::new(),
            signature: String::new(),
        }
        .sign(
            "seat-1-key",
            &fixture.signing_key,
            &fixture.worker.context(disable_at),
        )
        .unwrap();
        fixture.publish(&disable);
        fixture.worker.tick_once().unwrap();

        let disabled = fixture.worker.snapshot.as_ref().unwrap();
        assert_eq!(disabled.revision, 4);
        assert!(matches!(
            &disabled.schedules[0].schedule,
            ClockScheduleKindV1::Alarm(ClockAlarmV1 { enabled: false, .. })
        ));
        let stopped = disabled
            .occurrences
            .iter()
            .find(|occurrence| occurrence.occurrence_id == ringing.occurrence_id)
            .unwrap();
        assert_eq!(stopped.phase, ClockOccurrencePhase::Stopped);
        let acknowledgement = stopped.acknowledgement.as_ref().unwrap();
        assert!(acknowledgement.stop);
        assert_eq!(acknowledgement.actor_clock, 4);
        assert_eq!(acknowledgement.acknowledged_at_utc_ms, disable_at);
        assert_eq!(
            acknowledgement.acknowledgement_id,
            schedule_disable_acknowledgement_id(&ringing.occurrence_id, request_id)
        );

        let audio = fixture.audio_messages();
        assert_eq!(audio.len(), 2, "disable must publish one exact Music Stop");
        let stop = ClockAudioRequestV1::from_json_at(audio.last().unwrap().as_bytes(), disable_at)
            .unwrap();
        assert_eq!(stop.occurrence_id, ringing.occurrence_id);
        assert_eq!(stop.global_event_id, ringing.global_event_id);
        assert_eq!(stop.occurrence_generation, ringing.revision);
        assert!(matches!(
            stop.body,
            ClockAudioActionV1::Stop { ref acknowledgement_id }
                if acknowledgement_id
                    == &schedule_disable_acknowledgement_id(&ringing.occurrence_id, request_id)
        ));

        fixture.publish(&disable);
        fixture.worker.tick_once().unwrap();
        assert_eq!(fixture.worker.snapshot.as_ref().unwrap().revision, 4);
        assert_eq!(fixture.audio_messages().len(), 2, "replay must stay closed");
    }

    #[test]
    fn timer_banner_extension_atomically_acknowledges_exact_generation_and_replays_closed() {
        let mut fixture = Fixture::new();
        fixture.worker.tick_once().unwrap();
        fixture.publish(&fixture.timer_command("timer-create", 1, NOW + 5_000));
        fixture.worker.tick_once().unwrap();

        let action_time = NOW + 10_000;
        fixture.clock.0.store(action_time, Ordering::Relaxed);
        fixture.worker.tick_once().unwrap();
        let expired_snapshot = fixture.worker.snapshot.as_ref().unwrap().clone();
        assert_eq!(expired_snapshot.revision, 3);
        let ringing = expired_snapshot.occurrences[0].clone();
        assert_eq!(ringing.phase, ClockOccurrencePhase::Ringing);
        assert_eq!(ringing.revision, expired_snapshot.revision);
        let expired_schedule = expired_snapshot.schedules[0].clone();

        let extension = fixture.timer_extension_command(
            "timer-add-minute",
            expired_snapshot.revision,
            action_time,
            expired_schedule.clone(),
        );
        let ClockCommandKindV1::UpsertSchedule {
            schedule: extension_schedule,
        } = &extension.body
        else {
            panic!("fixture extension must be an upsert");
        };
        exact_timer_extension_acknowledgement(
            &expired_snapshot,
            &expired_schedule,
            extension_schedule,
            "seat-1",
            "timer-add-minute",
            expired_snapshot.revision,
            action_time,
        )
        .unwrap();
        fixture.publish(&extension);
        fixture.worker.tick_once().unwrap();

        let extended = fixture.worker.snapshot.as_ref().unwrap();
        assert_eq!(extended.revision, 4);
        let schedule = &extended.schedules[0];
        assert_eq!(schedule.revision, expired_schedule.revision + 1);
        let ClockScheduleKindV1::Timer(timer) = &schedule.schedule else {
            panic!("extended schedule must remain a timer");
        };
        assert_eq!(timer.phase, ClockTimerPhase::Running);
        assert_eq!(timer.absolute_deadline_utc_ms, Some(action_time + 60_000));
        let acknowledged = extended
            .occurrences
            .iter()
            .find(|value| value.occurrence_id == ringing.occurrence_id)
            .unwrap();
        assert_eq!(acknowledged.phase, ClockOccurrencePhase::Stopped);
        let acknowledgement = acknowledged.acknowledgement.as_ref().unwrap();
        assert_eq!(acknowledgement.acknowledgement_id, "timer-add-minute");
        assert_eq!(acknowledgement.global_event_id, ringing.global_event_id);
        assert_eq!(acknowledgement.actor_clock, 4);

        let audio = fixture.audio_messages();
        let stop = ClockAudioRequestV1::from_json_at(audio.last().unwrap().as_bytes(), action_time)
            .unwrap();
        assert_eq!(stop.occurrence_id, ringing.occurrence_id);
        assert_eq!(stop.global_event_id, ringing.global_event_id);
        assert_eq!(stop.occurrence_generation, 3);
        assert!(matches!(
            stop.body,
            ClockAudioActionV1::Stop { ref acknowledgement_id }
                if acknowledgement_id == "timer-add-minute"
        ));

        let connection = Connection::open(&fixture.db).unwrap();
        let (durable_revision, durable_snapshot, ledger_revision, fingerprint_len):
            (i64, String, i64, i64) = connection
            .query_row(
                "SELECT a.revision, a.snapshot_json, l.revision, length(l.request_fingerprint) FROM clock_authority a JOIN clock_request_ledger l ON l.node_id = a.node_id WHERE a.node_id = 'seat-1' AND l.request_id = 'timer-add-minute'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(durable_revision, 4);
        assert_eq!(ledger_revision, 4);
        assert_eq!(fingerprint_len, 64);
        assert_eq!(
            serde_json::from_str::<ClockSnapshotV1>(&durable_snapshot).unwrap(),
            *extended
        );

        fixture.publish(&extension);
        fixture.worker.tick_once().unwrap();
        assert_eq!(fixture.worker.snapshot.as_ref().unwrap().revision, 4);
        assert_eq!(fixture.audio_messages().len(), audio.len());

        let mut conflicting_settings = fixture.worker.snapshot.as_ref().unwrap().settings.clone();
        conflicting_settings.alarm_crescendo = !conflicting_settings.alarm_crescendo;
        let conflict = ClockCommandV1 {
            schema_version: CLOCK_SCHEMA_VERSION,
            request_id: "timer-add-minute".into(),
            origin_node_id: "seat-1".into(),
            expected_revision: 4,
            issued_at_utc_ms: action_time,
            expires_at_utc_ms: action_time + MAX_CLOCK_COMMAND_TTL_MS,
            body: ClockCommandKindV1::SetSettings {
                settings: conflicting_settings,
            },
            signer_id: String::new(),
            signature: String::new(),
        }
        .sign(
            "seat-1-key",
            &fixture.signing_key,
            &fixture.worker.context(action_time),
        )
        .unwrap();
        let settings_before = fixture.worker.snapshot.as_ref().unwrap().settings.clone();
        fixture.publish(&conflict);
        fixture.worker.tick_once().unwrap();
        assert_eq!(fixture.worker.snapshot.as_ref().unwrap().revision, 4);
        assert_eq!(
            fixture.worker.snapshot.as_ref().unwrap().settings,
            settings_before
        );
        let ledger_rows: i64 = Connection::open(&fixture.db)
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM clock_request_ledger WHERE node_id = 'seat-1' AND request_id = 'timer-add-minute'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(ledger_rows, 1, "conflicting request-id reuse stays closed");

        let stale = fixture.timer_extension_command(
            "timer-stale-generation",
            3,
            action_time,
            expired_schedule,
        );
        fixture.publish(&stale);
        fixture.worker.tick_once().unwrap();
        assert_eq!(fixture.worker.snapshot.as_ref().unwrap().revision, 4);

        let mut unsigned = conflict;
        unsigned.request_id = "timer-unsigned".into();
        unsigned.signature.clear();
        fixture.publish(&unsigned);
        fixture.worker.tick_once().unwrap();
        assert_eq!(fixture.worker.snapshot.as_ref().unwrap().revision, 4);

        let mut expired_fixture = Fixture::new();
        expired_fixture.worker.tick_once().unwrap();
        let mut expired_settings = expired_fixture
            .worker
            .snapshot
            .as_ref()
            .unwrap()
            .settings
            .clone();
        expired_settings.timer_crescendo = !expired_settings.timer_crescendo;
        let expired_action = ClockCommandV1 {
            schema_version: CLOCK_SCHEMA_VERSION,
            request_id: "timer-expired-action".into(),
            origin_node_id: "seat-1".into(),
            expected_revision: 1,
            issued_at_utc_ms: NOW,
            expires_at_utc_ms: NOW + MAX_CLOCK_COMMAND_TTL_MS,
            body: ClockCommandKindV1::SetSettings {
                settings: expired_settings,
            },
            signer_id: String::new(),
            signature: String::new(),
        }
        .sign(
            "seat-1-key",
            &expired_fixture.signing_key,
            &expired_fixture.worker.context(NOW),
        )
        .unwrap();
        expired_fixture
            .clock
            .0
            .store(NOW + MAX_CLOCK_COMMAND_TTL_MS + 1, Ordering::Relaxed);
        expired_fixture.publish(&expired_action);
        expired_fixture.worker.tick_once().unwrap();
        assert_eq!(
            expired_fixture.worker.snapshot.as_ref().unwrap().revision,
            1,
            "expired signed actions fail closed"
        );
    }

    #[test]
    fn first_received_late_is_missed_and_request_replay_is_idempotent() {
        let mut fixture = Fixture::new();
        fixture.worker.tick_once().unwrap();
        let refused = ClockCommandV1 {
            schema_version: CLOCK_SCHEMA_VERSION,
            request_id: "refused-1".into(),
            origin_node_id: "seat-1".into(),
            expected_revision: 1,
            issued_at_utc_ms: NOW - 100,
            expires_at_utc_ms: NOW - 100 + MAX_CLOCK_COMMAND_TTL_MS,
            body: ClockCommandKindV1::SetScheduleEnabled {
                schedule_id: "missing-alarm".into(),
                enabled: false,
            },
            signer_id: String::new(),
            signature: String::new(),
        }
        .sign(
            "seat-1-key",
            &fixture.signing_key,
            &ClockValidationContext {
                wall_utc_ms: NOW,
                monotonic_ms: 1,
                zone_exists: &zone_exists,
            },
        )
        .unwrap();
        let before_refusal =
            serde_json::to_string(fixture.worker.snapshot.as_ref().unwrap()).unwrap();
        let before_produced_at = fixture.worker.snapshot.as_ref().unwrap().produced_at_utc_ms;
        fixture.publish(&refused);
        fixture.worker.tick_once().unwrap();
        let refused_cursor = fixture.worker.action_cursor.clone();
        assert!(refused_cursor.is_some());
        assert_eq!(fixture.worker.snapshot.as_ref().unwrap().revision, 1);
        assert_eq!(
            fixture.worker.snapshot.as_ref().unwrap().produced_at_utc_ms,
            before_produced_at
        );
        assert_eq!(
            serde_json::to_string(fixture.worker.snapshot.as_ref().unwrap()).unwrap(),
            before_refusal
        );
        let durable_after_refusal = fixture.worker.store.load("seat-1").unwrap().unwrap();
        assert_eq!(durable_after_refusal.snapshot_json, before_refusal);
        assert_eq!(durable_after_refusal.action_cursor, refused_cursor);
        assert!(fixture
            .worker
            .snapshot
            .as_ref()
            .unwrap()
            .schedules
            .is_empty());
        fixture.worker.tick_once().unwrap();
        assert_eq!(fixture.worker.action_cursor, refused_cursor);
        assert_eq!(fixture.worker.snapshot.as_ref().unwrap().revision, 1);

        fixture.publish(&fixture.timer_command("late-1", 1, NOW - 1));
        fixture.worker.tick_once().unwrap();
        let snapshot = fixture.worker.snapshot.as_ref().unwrap();
        assert_eq!(snapshot.occurrences[0].phase, ClockOccurrencePhase::Missed);
        assert_eq!(snapshot.revision, 2);

        let persisted = snapshot.clone();
        let mut replay = ClockSnapshotV1 {
            revision: 3,
            produced_at_utc_ms: NOW,
            ..persisted
        };
        stamp_revision(&mut replay, "seat-1");
        let changed = fixture
            .worker
            .store
            .commit(
                "seat-1",
                2,
                &replay,
                Some("late-1"),
                Some(&"0".repeat(64)),
                fixture.worker.action_cursor.as_deref(),
                &[],
            )
            .unwrap();
        assert!(!changed);
        let durable = fixture.worker.store.load("seat-1").unwrap().unwrap();
        assert_eq!(durable.revision, 2);
    }

    #[test]
    fn duplicate_clock_replay_cannot_regress_or_clear_action_cursor() {
        let mut fixture = Fixture::new();
        fixture.worker.tick_once().unwrap();
        let command = fixture.timer_command("cursor-replay", 1, NOW + 60_000);
        fixture.publish(&command);
        fixture.worker.tick_once().unwrap();

        let durable = fixture.worker.store.load("seat-1").unwrap().unwrap();
        let cursor = durable.action_cursor.clone().expect("admitted cursor");
        let mut replay = fixture.worker.snapshot.as_ref().unwrap().clone();
        replay.revision += 1;
        replay.produced_at_utc_ms = NOW;
        stamp_revision(&mut replay, "seat-1");

        assert!(!fixture
            .worker
            .store
            .commit(
                "seat-1",
                durable.revision,
                &replay,
                Some("cursor-replay"),
                Some(&"0".repeat(64)),
                Some("00000000000000000000000000"),
                &[],
            )
            .unwrap());
        assert_eq!(
            fixture
                .worker
                .store
                .load("seat-1")
                .unwrap()
                .unwrap()
                .action_cursor,
            Some(cursor.clone())
        );

        let mut replay = fixture.worker.snapshot.as_ref().unwrap().clone();
        replay.revision += 1;
        replay.produced_at_utc_ms = NOW;
        stamp_revision(&mut replay, "seat-1");
        assert!(!fixture
            .worker
            .store
            .commit(
                "seat-1",
                durable.revision,
                &replay,
                Some("cursor-replay"),
                Some(&"0".repeat(64)),
                None,
                &[],
            )
            .unwrap());
        assert_eq!(
            fixture
                .worker
                .store
                .load("seat-1")
                .unwrap()
                .unwrap()
                .action_cursor,
            Some(cursor)
        );
    }

    #[test]
    fn stale_clock_action_and_audio_cursors_cannot_regress() {
        let mut action_cursor = Some(String::from("01J00000000000000000000002"));
        advance_clock_cursor(&mut action_cursor, "01J00000000000000000000001");
        assert_eq!(action_cursor.as_deref(), Some("01J00000000000000000000002"));
        advance_clock_cursor(&mut action_cursor, "01J00000000000000000000003");
        assert_eq!(action_cursor.as_deref(), Some("01J00000000000000000000003"));

        let mut audio_cursor = None;
        advance_clock_cursor(&mut audio_cursor, "01J00000000000000000000007");
        advance_clock_cursor(&mut audio_cursor, "01J00000000000000000000006");
        assert_eq!(audio_cursor.as_deref(), Some("01J00000000000000000000007"));
    }

    #[test]
    fn stopwatch_commands_cannot_claim_a_foreign_origin() {
        let mut fixture = Fixture::new();
        fixture.worker.tick_once().unwrap();

        let command = |request_id: &str, origin_node_id: &str| {
            ClockCommandV1 {
                schema_version: CLOCK_SCHEMA_VERSION,
                request_id: request_id.into(),
                origin_node_id: "seat-1".into(),
                expected_revision: 1,
                issued_at_utc_ms: NOW,
                expires_at_utc_ms: NOW + MAX_CLOCK_COMMAND_TTL_MS,
                body: ClockCommandKindV1::UpsertStopwatch {
                    stopwatch: ClockStopwatchV1 {
                        stopwatch_id: "stopwatch-1".into(),
                        origin_node_id: origin_node_id.into(),
                        mirror_target_ids: vec!["seat-1".into()],
                        revision: 1,
                        phase: mackes_mesh_types::clock::ClockStopwatchPhase::Running,
                        started_wall_utc_ms: Some(NOW),
                        started_monotonic_ms: Some(1),
                        accumulated_elapsed_ms: 0,
                        laps: Vec::new(),
                    },
                },
                signer_id: String::new(),
                signature: String::new(),
            }
            .sign(
                "seat-1-key",
                &fixture.signing_key,
                &ClockValidationContext {
                    wall_utc_ms: NOW,
                    monotonic_ms: 1,
                    zone_exists: &zone_exists,
                },
            )
            .unwrap()
        };

        let forged = command("stopwatch-forged-origin", "seat-2");
        let owned = command("stopwatch-owned-origin", "seat-1");

        fixture.publish(&forged);
        fixture.worker.tick_once().unwrap();
        assert_eq!(fixture.worker.snapshot.as_ref().unwrap().revision, 1);
        assert!(fixture
            .worker
            .snapshot
            .as_ref()
            .unwrap()
            .stopwatches
            .is_empty());

        fixture.publish(&owned);
        fixture.worker.tick_once().unwrap();
        let snapshot = fixture.worker.snapshot.as_ref().unwrap();
        assert_eq!(snapshot.revision, 2);
        assert_eq!(snapshot.stopwatches.len(), 1);
        assert_eq!(snapshot.stopwatches[0].origin_node_id, "seat-1");

        let durable = fixture.worker.store.load("seat-1").unwrap().unwrap();
        assert_eq!(durable.revision, 2);
        assert_eq!(
            serde_json::from_str::<ClockSnapshotV1>(&durable.snapshot_json)
                .unwrap()
                .stopwatches[0]
                .origin_node_id,
            "seat-1"
        );
    }

    #[test]
    fn local_clock_targets_must_be_self_or_approved_peers() {
        let mut fixture = Fixture::new();
        fixture.worker.tick_once().unwrap();

        let mut unapproved_schedule =
            fixture.timer_command("unapproved-schedule-target", 1, NOW + 60_000);
        let ClockCommandKindV1::UpsertSchedule { schedule } = &mut unapproved_schedule.body else {
            unreachable!();
        };
        schedule.selected_target_ids = vec!["seat-9".into()];
        unapproved_schedule.signature.clear();
        unapproved_schedule.signer_id.clear();
        let unapproved_schedule = unapproved_schedule
            .sign(
                "seat-1-key",
                &fixture.signing_key,
                &ClockValidationContext {
                    wall_utc_ms: NOW,
                    monotonic_ms: 1,
                    zone_exists: &zone_exists,
                },
            )
            .unwrap();
        fixture.publish(&unapproved_schedule);

        let unapproved_stopwatch = signed_stopwatch_for(
            &fixture.signing_key,
            "seat-1-key",
            "seat-1",
            "seat-1",
            "unapproved-stopwatch-mirror",
            1,
            "unapproved-stopwatch",
            1,
            &["seat-9"],
            0,
        );
        fixture.publish(&unapproved_stopwatch);
        fixture.worker.tick_once().unwrap();

        let rejected = fixture.worker.snapshot.as_ref().unwrap();
        assert_eq!(rejected.revision, 1);
        assert!(rejected.schedules.is_empty());
        assert!(rejected.stopwatches.is_empty());

        fixture.worker.approved_peer_ids.insert("seat-2".into());
        let mut approved_schedule =
            fixture.timer_command("approved-schedule-target", 1, NOW + 60_000);
        let ClockCommandKindV1::UpsertSchedule { schedule } = &mut approved_schedule.body else {
            unreachable!();
        };
        schedule.selected_target_ids = vec!["seat-2".into()];
        approved_schedule.signature.clear();
        approved_schedule.signer_id.clear();
        let approved_schedule = approved_schedule
            .sign(
                "seat-1-key",
                &fixture.signing_key,
                &ClockValidationContext {
                    wall_utc_ms: NOW,
                    monotonic_ms: 1,
                    zone_exists: &zone_exists,
                },
            )
            .unwrap();
        fixture.publish(&approved_schedule);
        fixture.worker.tick_once().unwrap();
        fixture.publish(&signed_stopwatch_for(
            &fixture.signing_key,
            "seat-1-key",
            "seat-1",
            "seat-1",
            "approved-stopwatch-mirror",
            2,
            "approved-stopwatch",
            1,
            &["seat-2"],
            0,
        ));
        fixture.worker.tick_once().unwrap();

        let admitted = fixture.worker.snapshot.as_ref().unwrap();
        assert_eq!(admitted.revision, 3);
        assert_eq!(admitted.schedules[0].selected_target_ids, ["seat-2"]);
        assert_eq!(admitted.stopwatches[0].mirror_target_ids, ["seat-2"]);
    }

    #[test]
    fn running_stopwatch_past_elapsed_deadline_is_not_admitted() {
        let mut fixture = Fixture::new();
        fixture.worker.tick_once().unwrap();

        let mut command = signed_stopwatch_for(
            &fixture.signing_key,
            "seat-1-key",
            "seat-1",
            "seat-1",
            "stopwatch-overdue",
            1,
            "stopwatch-overdue",
            1,
            &["seat-1"],
            0,
        );
        let ClockCommandKindV1::UpsertStopwatch { stopwatch } = &mut command.body else {
            unreachable!();
        };
        stopwatch.phase = mackes_mesh_types::clock::ClockStopwatchPhase::Running;
        stopwatch.started_wall_utc_ms = Some(
            NOW.checked_sub(MAX_CLOCK_STOPWATCH_ELAPSED_MS as i64)
                .and_then(|value| value.checked_sub(1))
                .unwrap(),
        );
        stopwatch.started_monotonic_ms = Some(1);
        command.signature.clear();
        command.signer_id.clear();
        let command = command
            .sign(
                "seat-1-key",
                &fixture.signing_key,
                &ClockValidationContext {
                    wall_utc_ms: NOW,
                    monotonic_ms: 90_000,
                    zone_exists: &zone_exists,
                },
            )
            .unwrap();

        fixture.publish(&command);
        fixture.worker.tick_once().unwrap();

        let snapshot = fixture.worker.snapshot.as_ref().unwrap();
        assert_eq!(snapshot.revision, 1);
        assert!(snapshot.stopwatches.is_empty());
    }

    #[test]
    fn stopwatch_identity_conflict_cannot_transfer_an_existing_origin() {
        let mut fixture = Fixture::new();
        fixture.worker.tick_once().unwrap();
        let foreign = ClockStopwatchV1 {
            stopwatch_id: "stopwatch-conflict".into(),
            origin_node_id: "seat-2".into(),
            mirror_target_ids: vec!["seat-1".into()],
            revision: 1,
            phase: mackes_mesh_types::clock::ClockStopwatchPhase::Paused,
            started_wall_utc_ms: None,
            started_monotonic_ms: None,
            accumulated_elapsed_ms: 5_000,
            laps: Vec::new(),
        };
        fixture
            .worker
            .snapshot
            .as_mut()
            .unwrap()
            .stopwatches
            .push(foreign.clone());
        let mut takeover = foreign.clone();
        takeover.origin_node_id = "seat-1".into();

        let error = fixture
            .worker
            .apply_command(
                ClockCommandKindV1::UpsertStopwatch {
                    stopwatch: takeover,
                },
                "seat-1",
                false,
                "stopwatch-takeover",
                1,
                NOW,
                NOW,
            )
            .expect_err("an existing stopwatch identity cannot change origin");

        assert_eq!(error.to_string(), "Clock stopwatch identity conflict");
        assert_eq!(
            fixture.worker.snapshot.as_ref().unwrap().stopwatches,
            vec![foreign]
        );
    }

    fn signed_stopwatch_for(
        key: &SigningKey,
        signer_id: &str,
        command_origin: &str,
        stopwatch_origin: &str,
        request_id: &str,
        expected_revision: u64,
        stopwatch_id: &str,
        stopwatch_revision: u64,
        targets: &[&str],
        accumulated_elapsed_ms: u64,
    ) -> ClockCommandV1 {
        ClockCommandV1 {
            schema_version: CLOCK_SCHEMA_VERSION,
            request_id: request_id.into(),
            origin_node_id: command_origin.into(),
            expected_revision,
            issued_at_utc_ms: NOW,
            expires_at_utc_ms: NOW + MAX_CLOCK_COMMAND_TTL_MS,
            body: ClockCommandKindV1::UpsertStopwatch {
                stopwatch: ClockStopwatchV1 {
                    stopwatch_id: stopwatch_id.into(),
                    origin_node_id: stopwatch_origin.into(),
                    mirror_target_ids: targets.iter().map(|value| (*value).into()).collect(),
                    revision: stopwatch_revision,
                    phase: mackes_mesh_types::clock::ClockStopwatchPhase::Paused,
                    started_wall_utc_ms: None,
                    started_monotonic_ms: None,
                    accumulated_elapsed_ms,
                    laps: Vec::new(),
                },
            },
            signer_id: String::new(),
            signature: String::new(),
        }
        .sign(
            signer_id,
            key,
            &ClockValidationContext {
                wall_utc_ms: NOW,
                monotonic_ms: 90_000,
                zone_exists: &zone_exists,
            },
        )
        .unwrap()
    }

    #[test]
    fn approved_peer_stopwatch_admission_fails_closed_for_hostile_variants() {
        let mut fixture = Fixture::new();
        fixture.worker.approved_peer_ids = ["seat-2", "seat-3"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        fixture.worker.tick_once().unwrap();

        let admitted = signed_stopwatch_for(
            &fixture.signing_key,
            "seat-1-key",
            "seat-2",
            "seat-2",
            "peer-stopwatch-admitted",
            1,
            "peer-stopwatch",
            5,
            &["seat-1"],
            5_000,
        );
        fixture.publish(&admitted);
        fixture.worker.tick_once().unwrap();
        let admitted_snapshot = fixture.worker.snapshot.as_ref().unwrap().clone();
        assert_eq!(admitted_snapshot.revision, 2);
        assert_eq!(admitted_snapshot.stopwatches.len(), 1);
        assert_eq!(admitted_snapshot.stopwatches[0].origin_node_id, "seat-2");
        assert_eq!(admitted_snapshot.stopwatches[0].revision, 5);

        let hostile = [
            signed_stopwatch_for(
                &fixture.signing_key,
                "seat-1-key",
                "seat-2",
                "seat-2",
                "peer-stopwatch-stale",
                2,
                "peer-stopwatch",
                4,
                &["seat-1"],
                9_000,
            ),
            signed_stopwatch_for(
                &fixture.signing_key,
                "seat-1-key",
                "seat-2",
                "seat-2",
                "peer-stopwatch-revision-conflict",
                2,
                "peer-stopwatch",
                5,
                &["seat-1"],
                9_000,
            ),
            signed_stopwatch_for(
                &fixture.signing_key,
                "seat-1-key",
                "seat-2",
                "seat-2",
                "peer-stopwatch-untargeted",
                2,
                "peer-stopwatch",
                6,
                &["seat-9"],
                9_000,
            ),
            signed_stopwatch_for(
                &fixture.signing_key,
                "seat-1-key",
                "seat-3",
                "seat-3",
                "peer-stopwatch-cross-origin",
                2,
                "peer-stopwatch",
                6,
                &["seat-1"],
                9_000,
            ),
            signed_stopwatch_for(
                &fixture.signing_key,
                "seat-1-key",
                "seat-4",
                "seat-4",
                "peer-stopwatch-unapproved",
                2,
                "unapproved-stopwatch",
                1,
                &["seat-1"],
                1_000,
            ),
            signed_stopwatch_for(
                &fixture.signing_key,
                "seat-1-key",
                "seat-1",
                "seat-2",
                "peer-stopwatch-locally-forged-origin",
                2,
                "forged-stopwatch",
                1,
                &["seat-1"],
                1_000,
            ),
        ];
        for command in hostile {
            fixture.publish(&command);
            fixture.worker.tick_once().unwrap();
            assert_eq!(
                fixture.worker.snapshot.as_ref().unwrap(),
                &admitted_snapshot,
                "hostile stopwatch command changed Clock authority"
            );
        }

        fixture.worker.blocked_origin_ids.insert("seat-2".into());
        let blocked = signed_stopwatch_for(
            &fixture.signing_key,
            "seat-1-key",
            "seat-2",
            "seat-2",
            "peer-stopwatch-blocked",
            2,
            "peer-stopwatch",
            6,
            &["seat-1"],
            9_000,
        );
        fixture.publish(&blocked);
        fixture.worker.tick_once().unwrap();
        assert_eq!(
            fixture.worker.snapshot.as_ref().unwrap(),
            &admitted_snapshot
        );

        let durable = fixture.worker.store.load("seat-1").unwrap().unwrap();
        let durable_snapshot: ClockSnapshotV1 =
            serde_json::from_str(&durable.snapshot_json).unwrap();
        assert_eq!(durable_snapshot, admitted_snapshot);
    }

    #[test]
    fn peer_stopwatch_transport_preserves_origin_revision_and_clock_domain() {
        let temp = tempfile::tempdir().unwrap();
        let bus = temp.path().join("bus");
        let key = SigningKey::from_bytes(&[31; 32]);
        let mut origin = PeerNode::new(temp.path(), &bus, "node-a", &key);
        let mut mirror = PeerNode::new(temp.path(), &bus, "node-b", &key);
        origin.worker.tick_once().unwrap();
        mirror.worker.tick_once().unwrap();

        let mut command = signed_stopwatch_for(
            &key,
            "clock-mesh-key",
            "node-a",
            "node-a",
            "mirror-running-stopwatch",
            1,
            "mesh-stopwatch",
            1,
            &["node-b"],
            2_000,
        );
        let ClockCommandKindV1::UpsertStopwatch { stopwatch } = &mut command.body else {
            unreachable!();
        };
        stopwatch.phase = mackes_mesh_types::clock::ClockStopwatchPhase::Running;
        stopwatch.started_wall_utc_ms = Some(NOW);
        stopwatch.started_monotonic_ms = Some(75_000);
        command.signature.clear();
        command.signer_id.clear();
        command = command
            .sign(
                "clock-mesh-key",
                &key,
                &ClockValidationContext {
                    wall_utc_ms: NOW,
                    monotonic_ms: 90_000,
                    zone_exists: &zone_exists,
                },
            )
            .unwrap();

        publish_to(&bus, "node-a", &command);
        origin.worker.tick_once().unwrap();
        assert_eq!(origin.revision(), 2);
        assert_eq!(
            origin.worker.snapshot.as_ref().unwrap().stopwatches[0].revision,
            2
        );

        mirror.worker.tick_once().unwrap();
        let mirrored = &mirror.worker.snapshot.as_ref().unwrap().stopwatches[0];
        assert_eq!(mirror.revision(), 2);
        assert_eq!(mirrored.stopwatch_id, "mesh-stopwatch");
        assert_eq!(mirrored.origin_node_id, "node-a");
        assert_eq!(mirrored.revision, 2);
        assert_eq!(mirrored.started_monotonic_ms, Some(75_000));

        mirror.worker.tick_once().unwrap();
        assert_eq!(mirror.revision(), 2, "converged mirror must not replay");
    }

    #[test]
    fn peer_convergence_is_bounded_per_tick() {
        let mut fixture = Fixture::new();
        fixture.worker.approved_peer_ids.insert("seat-2".into());
        fixture.worker.tick_once().unwrap();
        fixture
            .worker
            .snapshot
            .as_mut()
            .unwrap()
            .stopwatches
            .extend(
                (0..=MAX_PEER_COMMANDS_PER_TICK).map(|index| ClockStopwatchV1 {
                    stopwatch_id: format!("stopwatch-{index}"),
                    origin_node_id: "seat-1".into(),
                    mirror_target_ids: vec!["seat-2".into()],
                    revision: 1,
                    phase: mackes_mesh_types::clock::ClockStopwatchPhase::Paused,
                    started_wall_utc_ms: None,
                    started_monotonic_ms: None,
                    accumulated_elapsed_ms: 0,
                    laps: Vec::new(),
                }),
            );

        let transaction = fixture.worker.open_bus_transaction().unwrap();
        let peer_snapshot = fixture.worker.initial_snapshot(NOW);
        let peer_snapshots = BTreeMap::from([(String::from("seat-2"), peer_snapshot)]);
        fixture
            .worker
            .publish_peer_convergence(&transaction, NOW, &peer_snapshots)
            .unwrap();

        let messages = transaction
            .persist
            .list_since(&clock_command_topic("seat-2").unwrap(), None)
            .unwrap();
        assert_eq!(messages.len(), MAX_PEER_COMMANDS_PER_TICK);
    }

    #[test]
    fn peer_convergence_probe_budget_bounds_retry_suppressed_work() {
        let mut fixture = Fixture::new();
        fixture.worker.approved_peer_ids.insert("seat-2".into());
        fixture.worker.tick_once().unwrap();

        let stopwatches: Vec<_> = (0..=MAX_PEER_CONVERGENCE_PROBES_PER_TICK)
            .map(|index| ClockStopwatchV1 {
                stopwatch_id: format!("probe-budget-{index}"),
                origin_node_id: "seat-1".into(),
                mirror_target_ids: vec!["seat-2".into()],
                revision: 1,
                phase: mackes_mesh_types::clock::ClockStopwatchPhase::Paused,
                started_wall_utc_ms: None,
                started_monotonic_ms: None,
                accumulated_elapsed_ms: 0,
                laps: Vec::new(),
            })
            .collect();
        for stopwatch in stopwatches
            .iter()
            .take(MAX_PEER_CONVERGENCE_PROBES_PER_TICK)
        {
            fixture.worker.peer_last_sent_ms.insert(
                peer_request_id(
                    "stopwatch",
                    "seat-2",
                    &stopwatch.stopwatch_id,
                    stopwatch.revision,
                    "",
                ),
                NOW,
            );
        }
        fixture.worker.snapshot.as_mut().unwrap().stopwatches = stopwatches;

        let transaction = fixture.worker.open_bus_transaction().unwrap();
        let peer_snapshot = fixture.worker.initial_snapshot(NOW);
        fixture
            .worker
            .publish_peer_convergence(
                &transaction,
                NOW,
                &BTreeMap::from([(String::from("seat-2"), peer_snapshot)]),
            )
            .unwrap();

        let messages = transaction
            .persist
            .list_since(&clock_command_topic("seat-2").unwrap(), None)
            .unwrap();
        assert!(
            messages.is_empty(),
            "retry-suppressed convergence must stop at the probe budget"
        );
    }

    #[test]
    fn clock_tick_reuses_one_wall_clock_sample_after_loading() {
        let mut fixture = Fixture::new();
        fixture.worker.ensure_loaded().unwrap();
        let clock = Arc::new(CountingClock {
            now_ms: NOW,
            reads: AtomicUsize::new(0),
        });
        fixture.worker.clock = clock.clone();
        let transaction = fixture.worker.open_bus_transaction().unwrap();

        fixture.worker.tick_with_transaction(&transaction).unwrap();

        assert_eq!(
            clock.reads.load(Ordering::SeqCst),
            1,
            "one Clock tick must validate and publish against one wall-clock sample"
        );
    }

    #[test]
    fn peer_schedule_convergence_rejects_revision_only_matches() {
        let desired = weekday_alarm_schedule("mesh-alarm", 7, 30, ClockFoldPolicy::Earlier);

        assert!(peer_schedule_is_converged(&desired, &desired));

        let mut same_revision_conflict = desired.clone();
        same_revision_conflict.label = "tampered label".into();
        assert!(!peer_schedule_is_converged(
            &same_revision_conflict,
            &desired
        ));

        let mut newer_revision_conflict = desired.clone();
        newer_revision_conflict.revision += 1;
        assert!(!peer_schedule_is_converged(
            &newer_revision_conflict,
            &desired
        ));
    }

    #[test]
    fn peer_stopwatch_convergence_repairs_newer_conflict_without_stale_rollback() {
        let mut fixture = Fixture::new();
        fixture.worker.approved_peer_ids.insert("seat-2".into());
        fixture.worker.tick_once().unwrap();
        let desired = ClockStopwatchV1 {
            stopwatch_id: "mesh-stopwatch".into(),
            origin_node_id: "seat-1".into(),
            mirror_target_ids: vec!["seat-2".into()],
            revision: 4,
            phase: mackes_mesh_types::clock::ClockStopwatchPhase::Paused,
            started_wall_utc_ms: None,
            started_monotonic_ms: None,
            accumulated_elapsed_ms: 9_000,
            laps: Vec::new(),
        };

        fixture
            .worker
            .snapshot
            .as_mut()
            .unwrap()
            .stopwatches
            .push(desired.clone());
        let mut peer = fixture.worker.initial_snapshot(NOW);
        let mut conflict = desired.clone();
        conflict.revision += 1;
        conflict.accumulated_elapsed_ms += 1;
        peer.stopwatches.push(conflict);

        let transaction = fixture.worker.open_bus_transaction().unwrap();
        fixture
            .worker
            .publish_peer_convergence(
                &transaction,
                NOW,
                &BTreeMap::from([(String::from("seat-2"), peer.clone())]),
            )
            .unwrap();

        let messages = transaction
            .persist
            .list_since(&clock_command_topic("seat-2").unwrap(), None)
            .unwrap();
        assert_eq!(messages.len(), 1, "conflicting peer payload needs repair");
        let command: ClockCommandV1 =
            serde_json::from_str(messages[0].body.as_deref().unwrap()).unwrap();
        let ClockCommandKindV1::UpsertStopwatch { stopwatch } = command.body else {
            panic!("peer repair must carry the stopwatch payload");
        };
        assert_eq!(stopwatch, desired);

        let mut receiver = Fixture::new();
        receiver.worker.node_id = "seat-2".into();
        receiver.worker.snapshot = Some(peer);
        assert!(receiver
            .worker
            .apply_command(
                ClockCommandKindV1::UpsertStopwatch {
                    stopwatch: desired.clone(),
                },
                "seat-1",
                true,
                "repair-current-generation",
                1,
                NOW,
                NOW,
            )
            .expect("origin repair bound to the observed peer generation"));
        assert_eq!(
            receiver.worker.snapshot.as_ref().unwrap().stopwatches,
            [desired.clone()],
            "the origin payload must replace the newer conflicting mirror"
        );

        let snapshot = receiver.worker.snapshot.as_mut().unwrap();
        snapshot.revision = 2;
        let mut newer = desired.clone();
        newer.revision += 2;
        newer.accumulated_elapsed_ms += 2;
        snapshot.stopwatches = vec![newer.clone()];
        let error = receiver
            .worker
            .apply_command(
                ClockCommandKindV1::UpsertStopwatch { stopwatch: desired },
                "seat-1",
                true,
                "repair-stale-generation",
                1,
                NOW,
                NOW,
            )
            .expect_err("delayed repair must not overwrite an advanced peer generation");
        assert_eq!(error.to_string(), "stale Clock peer stopwatch repair");
        assert_eq!(
            receiver.worker.snapshot.as_ref().unwrap().stopwatches,
            [newer]
        );
    }

    #[test]
    fn peer_acknowledgement_convergence_accepts_exact_replay() {
        let acknowledgement = ClockAcknowledgementV1 {
            acknowledgement_id: "ack-1".into(),
            global_event_id: "event-1".into(),
            actor_node_id: "seat-1".into(),
            actor_clock: 7,
            acknowledged_at_utc_ms: NOW,
            stop: true,
        };

        assert!(peer_acknowledgement_is_converged(
            &acknowledgement,
            &acknowledgement
        ));
    }

    #[test]
    fn three_node_delivery_loss_rejoin_replay_and_acknowledgement_converge() {
        let temp = tempfile::tempdir().unwrap();
        let bus = temp.path().join("bus");
        let key = SigningKey::from_bytes(&[31; 32]);
        let mut a = PeerNode::new(temp.path(), &bus, "node-a", &key);
        let mut b = PeerNode::new(temp.path(), &bus, "node-b", &key);
        let mut c = PeerNode::new(temp.path(), &bus, "node-c", &key);

        for node in [&mut a, &mut b, &mut c] {
            node.worker.tick_once().unwrap();
        }

        let schedule = signed_timer_for(
            &key,
            "node-a",
            "mesh-schedule-1",
            a.revision(),
            "mesh-timer-1",
            NOW + 5_000,
            &["node-a", "node-b", "node-c"],
            NOW,
        );
        publish_to(&bus, "node-a", &schedule);
        a.worker.tick_once().unwrap();
        b.worker.tick_once().unwrap();
        assert!(b
            .worker
            .snapshot
            .as_ref()
            .unwrap()
            .schedules
            .iter()
            .any(|value| value.schedule_id == "mesh-timer-1"));

        // Node C was unavailable while the source delivered. Its independent
        // process/store consumes the retained signed command on rejoin.
        c.worker.tick_once().unwrap();
        assert!(c
            .worker
            .snapshot
            .as_ref()
            .unwrap()
            .schedules
            .iter()
            .any(|value| value.schedule_id == "mesh-timer-1"));

        // A reordered duplicate carries an older target revision and the same
        // semantic generation. It is admitted but cannot advance authority.
        let b_revision = b.revision();
        let duplicate = signed_timer_for(
            &key,
            "node-a",
            "mesh-schedule-duplicate",
            1,
            "mesh-timer-1",
            NOW + 5_000,
            &["node-a", "node-b", "node-c"],
            NOW,
        );
        publish_to(&bus, "node-b", &duplicate);
        b.worker.tick_once().unwrap();
        assert_eq!(b.revision(), b_revision);

        for node in [&mut a, &mut b, &mut c] {
            node.clock.0.store(NOW + 10_000, Ordering::Relaxed);
            node.worker.tick_once().unwrap();
            assert_eq!(
                node.occurrence("mesh-timer-1").phase,
                ClockOccurrencePhase::Ringing
            );
        }

        let snooze = signed_ack_for(
            &key,
            "node-b",
            "ack-snooze-tie",
            b.revision(),
            b.occurrence("mesh-timer-1"),
            7,
            false,
            NOW + 10_000,
        );
        let stop = signed_ack_for(
            &key,
            "node-c",
            "ack-stop-tie",
            c.revision(),
            c.occurrence("mesh-timer-1"),
            7,
            true,
            NOW + 10_000,
        );
        publish_to(&bus, "node-b", &snooze);
        publish_to(&bus, "node-c", &stop);
        b.worker.tick_once().unwrap();
        c.worker.tick_once().unwrap();
        a.worker.tick_once().unwrap();
        b.worker.tick_once().unwrap();
        c.worker.tick_once().unwrap();
        for node in [&a, &b, &c] {
            let occurrence = node.occurrence("mesh-timer-1");
            assert_eq!(occurrence.phase, ClockOccurrencePhase::Stopped);
            assert!(occurrence.acknowledgement.as_ref().unwrap().stop);
            assert_eq!(occurrence.acknowledgement.as_ref().unwrap().actor_clock, 7);
        }

        // A second command is durably published while C is unavailable. Its
        // first execution after the deadline is Missed and never rings late.
        let late = signed_timer_for(
            &key,
            "node-a",
            "mesh-late-1",
            a.revision(),
            "mesh-timer-late",
            NOW + 15_000,
            &["node-a", "node-c"],
            NOW + 10_000,
        );
        publish_to(&bus, "node-a", &late);
        a.worker.tick_once().unwrap();
        c.clock.0.store(NOW + 20_000, Ordering::Relaxed);
        c.worker.tick_once().unwrap();
        assert_eq!(
            c.occurrence("mesh-timer-late").phase,
            ClockOccurrencePhase::Missed
        );

        // Local disable and origin blocking affect only the receiving target.
        // B keeps a disabled local copy; C refuses A's new copy; A remains live.
        b.worker
            .disabled_schedule_ids
            .insert("mesh-timer-policy".into());
        c.worker.blocked_origin_ids.insert("node-a".into());
        a.clock.0.store(NOW + 20_000, Ordering::Relaxed);
        b.clock.0.store(NOW + 20_000, Ordering::Relaxed);
        a.worker.tick_once().unwrap();
        let policy = signed_timer_for(
            &key,
            "node-a",
            "mesh-policy-1",
            a.revision(),
            "mesh-timer-policy",
            NOW + 25_000,
            &["node-a", "node-b", "node-c"],
            NOW + 20_000,
        );
        publish_to(&bus, "node-a", &policy);
        a.worker.tick_once().unwrap();
        b.worker.tick_once().unwrap();
        c.worker.tick_once().unwrap();
        assert!(a
            .worker
            .snapshot
            .as_ref()
            .unwrap()
            .schedules
            .iter()
            .any(|value| value.schedule_id == "mesh-timer-policy"));
        assert!(b
            .worker
            .snapshot
            .as_ref()
            .unwrap()
            .schedules
            .iter()
            .any(|value| value.schedule_id == "mesh-timer-policy"));
        assert!(!c
            .worker
            .snapshot
            .as_ref()
            .unwrap()
            .schedules
            .iter()
            .any(|value| value.schedule_id == "mesh-timer-policy"));
        a.clock.0.store(NOW + 30_000, Ordering::Relaxed);
        b.clock.0.store(NOW + 30_000, Ordering::Relaxed);
        a.worker.tick_once().unwrap();
        b.worker.tick_once().unwrap();
        assert_eq!(
            b.occurrence("mesh-timer-policy")
                .targets
                .iter()
                .find(|target| target.target_node_id == "node-b")
                .unwrap()
                .disposition,
            ClockTargetDisposition::DisabledLocally
        );
        assert_eq!(
            a.occurrence("mesh-timer-policy")
                .targets
                .iter()
                .find(|target| target.target_node_id == "node-a")
                .unwrap()
                .disposition,
            ClockTargetDisposition::Ringing
        );
    }

    #[test]
    fn persisted_bus_multi_process_peer_rejoin_opt_out_and_global_ack_converge() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let bus = root.join("bus");
        let key = SigningKey::from_bytes(&[31; 32]);

        // Each invocation is a separate OS process. It independently opens the
        // retained Bus index and its node's SQLite Clock authority, then exits.
        for node_id in ["node-a", "node-b", "node-c"] {
            run_clock_process(root, node_id, NOW, &[]);
        }

        let schedule = signed_timer_for(
            &key,
            "node-a",
            "process-schedule-1",
            process_fixture_snapshot(root, "node-a").revision,
            "process-timer-1",
            NOW + 5_000,
            &["node-a", "node-b", "node-c"],
            NOW,
        );
        publish_to(&bus, "node-a", &schedule);
        run_clock_process(root, "node-a", NOW, &[]);
        run_clock_process(root, "node-b", NOW, &[]);
        assert!(process_fixture_snapshot(root, "node-b")
            .schedules
            .iter()
            .any(|value| value.schedule_id == "process-timer-1"));
        assert!(process_fixture_snapshot(root, "node-c")
            .schedules
            .is_empty());

        // C was absent for the initial delivery. A fresh process consumes the
        // retained signed peer command when C rejoins, with no in-memory relay.
        run_clock_process(root, "node-c", NOW, &[]);
        assert!(process_fixture_snapshot(root, "node-c")
            .schedules
            .iter()
            .any(|value| value.schedule_id == "process-timer-1"));

        run_clock_process(root, "node-a", NOW + 10_000, &[]);
        run_clock_process(root, "node-b", NOW + 10_000, &["process-timer-1"]);
        run_clock_process(root, "node-c", NOW + 10_000, &[]);
        let a_ringing = process_fixture_snapshot(root, "node-a");
        let b_disabled = process_fixture_snapshot(root, "node-b");
        let c_ringing = process_fixture_snapshot(root, "node-c");
        assert_eq!(
            b_disabled
                .occurrences
                .iter()
                .find(|value| value.schedule_id == "process-timer-1")
                .unwrap()
                .targets
                .iter()
                .find(|target| target.target_node_id == "node-b")
                .unwrap()
                .disposition,
            ClockTargetDisposition::DisabledLocally
        );
        for (snapshot, node_id) in [(&a_ringing, "node-a"), (&c_ringing, "node-c")] {
            assert_eq!(
                snapshot
                    .occurrences
                    .iter()
                    .find(|value| value.schedule_id == "process-timer-1")
                    .unwrap()
                    .targets
                    .iter()
                    .find(|target| target.target_node_id == node_id)
                    .unwrap()
                    .disposition,
                ClockTargetDisposition::Ringing
            );
        }

        let b_occurrence = b_disabled
            .occurrences
            .iter()
            .find(|value| value.schedule_id == "process-timer-1")
            .unwrap();
        let c_occurrence = c_ringing
            .occurrences
            .iter()
            .find(|value| value.schedule_id == "process-timer-1")
            .unwrap();
        publish_to(
            &bus,
            "node-b",
            &signed_ack_for(
                &key,
                "node-b",
                "process-snooze-tie",
                b_disabled.revision,
                b_occurrence,
                7,
                false,
                NOW + 10_000,
            ),
        );
        publish_to(
            &bus,
            "node-c",
            &signed_ack_for(
                &key,
                "node-c",
                "process-stop-tie",
                c_ringing.revision,
                c_occurrence,
                7,
                true,
                NOW + 10_000,
            ),
        );

        // Independent actors commit concurrently chosen Snooze/Stop outcomes;
        // subsequent fresh processes replay peer publications until Stop wins
        // the exact actor-clock tie on every durable authority.
        for node_id in ["node-b", "node-c", "node-a", "node-b", "node-c"] {
            let disabled = (node_id == "node-b").then_some("process-timer-1");
            run_clock_process(
                root,
                node_id,
                NOW + 10_000,
                &disabled.into_iter().collect::<Vec<_>>(),
            );
        }
        for node_id in ["node-a", "node-b", "node-c"] {
            let snapshot = process_fixture_snapshot(root, node_id);
            let occurrence = snapshot
                .occurrences
                .iter()
                .find(|value| value.schedule_id == "process-timer-1")
                .unwrap();
            assert_eq!(occurrence.phase, ClockOccurrencePhase::Stopped);
            let acknowledgement = occurrence.acknowledgement.as_ref().unwrap();
            assert!(acknowledgement.stop);
            assert_eq!(acknowledgement.actor_clock, 7);
            assert_eq!(acknowledgement.actor_node_id, "node-c");
        }
    }
}
