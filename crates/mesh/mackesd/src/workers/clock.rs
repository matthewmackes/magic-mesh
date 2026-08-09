//! WL-FUNC-022 S2 — daemon-owned local Clock persistence and deadlines.

#![cfg(feature = "async-services")]
#![allow(
    missing_docs,
    reason = "private authority machinery is exercised by operational tests"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
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
    MAX_CLOCK_COMMAND_TTL_MS, MAX_CLOCK_OCCURRENCES,
};
use mackes_mesh_types::music_auth::{self, MusicAuthContext, MUSIC_AUTH_CREDENTIAL_NAME};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use rand::RngCore as _;
use sha2::{Digest as _, Sha256};

use super::{ShutdownToken, Worker};
use crate::store::writer::{self, ClockAuthorityRecord, WriteOp};

const POLL: Duration = Duration::from_millis(250);
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
            bus_root: crate::bus_publish::default_bus_root(),
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
            self.action_cursor = record.action_cursor;
            self.snapshot = Some(snapshot);
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

    fn publish(&mut self, persist: &Persist) -> anyhow::Result<()> {
        let snapshot = self.snapshot.as_ref().expect("Clock snapshot loaded");
        let body = serde_json::to_string(snapshot)?;
        persist.write(
            &clock_state_topic(&self.node_id)?,
            Priority::Default,
            None,
            Some(&body),
        )?;
        self.published_once = true;
        Ok(())
    }

    fn commit_then_publish(
        &mut self,
        persist: &Persist,
        expected_revision: u64,
        request_id: Option<&str>,
        request_fingerprint: Option<&str>,
        audio_requests: &[writer::ClockAudioOutboxWrite],
    ) -> anyhow::Result<bool> {
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
            self.publish(persist)?;
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
            self.action_cursor = record.action_cursor;
            self.snapshot = Some(snapshot);
        }
        Ok(applied)
    }

    fn collect_actions(&self, persist: &Persist) -> anyhow::Result<Vec<(String, String)>> {
        let topic = clock_command_topic(&self.node_id)?;
        let mut actions = persist
            .list_since(&topic, self.action_cursor.as_deref())?
            .into_iter()
            .map(|message| (message.ulid, message.body.unwrap_or_default()))
            .collect::<Vec<_>>();
        actions.sort_by(|left, right| left.0.cmp(&right.0));
        Ok(actions)
    }

    fn process_command(
        &mut self,
        persist: &Persist,
        body: &[u8],
        now_ms: i64,
    ) -> anyhow::Result<()> {
        let Some(signer) = &self.signer else {
            return self.persist_cursor_only(persist, now_ms);
        };
        let context = self.context(now_ms);
        let Ok(command) = ClockCommandV1::from_json_at(body, &context) else {
            return self.persist_cursor_only(persist, now_ms);
        };
        let Ok(command) = command.admit_at(&signer.signer_id, &signer.key, &context) else {
            return self.persist_cursor_only(persist, now_ms);
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
        if (!peer_origin && command.expected_revision != expected)
            || (peer_origin && command.expected_revision > expected)
            || (peer_origin
                && (!self.approved_peer_ids.contains(&command.origin_node_id)
                    || self.blocked_origin_ids.contains(&command.origin_node_id)))
        {
            return self.persist_cursor_only(persist, now_ms);
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
                return self.persist_cursor_only(persist, now_ms);
            }
        };
        if !applied {
            self.snapshot = Some(prior_snapshot);
            return self
                .commit_then_publish(
                    persist,
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
        stamp_revision(snapshot);
        preserve_unchanged_occurrence_revisions(snapshot, &prior_snapshot);
        let audio_requests =
            clock_audio_transitions(&prior_snapshot, snapshot, now_ms, &self.node_id)?;
        self.commit_then_publish(
            persist,
            expected,
            Some(&request_id),
            Some(&request_fingerprint),
            &audio_requests,
        )?;
        Ok(())
    }

    fn persist_cursor_only(&mut self, persist: &Persist, _now_ms: i64) -> anyhow::Result<()> {
        let expected = self
            .snapshot
            .as_ref()
            .expect("Clock snapshot loaded")
            .revision;
        self.commit_then_publish(persist, expected, None, None, &[])?;
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
        let snapshot = self.snapshot.as_mut().expect("Clock snapshot loaded");
        let changed = match command {
            ClockCommandKindV1::UpsertSchedule { mut schedule } => {
                anyhow::ensure!(
                    schedule.origin_node_id == command_origin,
                    "Clock schedule origin mismatch"
                );
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
                let before = snapshot.schedules.len();
                snapshot
                    .schedules
                    .retain(|value| value.schedule_id != schedule_id);
                snapshot.schedules.len() != before
            }
            ClockCommandKindV1::SetScheduleEnabled {
                schedule_id,
                enabled,
            } => {
                let Some(schedule) = snapshot
                    .schedules
                    .iter_mut()
                    .find(|value| value.schedule_id == schedule_id)
                else {
                    anyhow::bail!("Clock schedule does not exist");
                };
                let ClockScheduleKindV1::Alarm(alarm) = &mut schedule.schedule else {
                    anyhow::bail!("timer enable command is invalid");
                };
                if peer_origin {
                    anyhow::bail!("peer Clock enable mutation is not authoritative");
                }
                if alarm.enabled == enabled {
                    return Ok(false);
                }
                alarm.enabled = enabled;
                true
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
                if peer_origin {
                    anyhow::bail!("peer Clock stopwatch mutation is not authoritative");
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

    fn tick_once(&mut self) -> anyhow::Result<()> {
        self.ensure_loaded()?;
        let Some(bus_root) = self.bus_root.clone() else {
            return Ok(());
        };
        let persist = Persist::open(bus_root)?;
        let now_ms = self.clock.now_ms();
        self.consume_audio_status(&persist, now_ms)?;
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
            stamp_revision(snapshot);
            preserve_unchanged_occurrence_revisions(snapshot, &prior_snapshot);
            let audio_requests =
                clock_audio_transitions(&prior_snapshot, snapshot, now_ms, &self.node_id)?;
            self.commit_then_publish(&persist, expected, None, None, &audio_requests)?;
        }
        for (cursor, body) in self.collect_actions(&persist)? {
            self.action_cursor = Some(cursor);
            self.process_command(&persist, body.as_bytes(), self.clock.now_ms())?;
        }
        if !self.published_once {
            self.publish(&persist)?;
        }
        self.publish_peer_convergence(&persist, self.clock.now_ms())?;
        self.publish_pending_audio(&persist, self.clock.now_ms())?;
        Ok(())
    }

    fn consume_audio_status(&mut self, persist: &Persist, now_ms: i64) -> anyhow::Result<()> {
        let topic = clock_audio_status_topic(&self.node_id)?;
        for message in persist.list_since_limit(
            &topic,
            self.audio_status_cursor.as_deref(),
            MAX_AUDIO_STATUS_PER_TICK,
        )? {
            self.audio_status_cursor = Some(message.ulid);
            let Some(body) = message.body else { continue };
            if mackes_mesh_types::workloads::reject_duplicate_json_keys(&body).is_err() {
                continue;
            }
            let Ok(status) = serde_json::from_str::<ClockAudioStatusV1>(&body) else {
                continue;
            };
            if status.validate_at(now_ms).is_ok()
                && self.store.acknowledge_audio(&self.node_id, &status)?
            {
                self.audio_last_sent_ms.remove(&status.request_id);
            }
        }
        Ok(())
    }

    fn publish_pending_audio(&mut self, persist: &Persist, now_ms: i64) -> anyhow::Result<()> {
        let Some(seed) = self.music_signing_seed else {
            return Ok(());
        };
        for pending in self.store.pending_audio(&self.node_id)? {
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
            persist.write(
                CLOCK_AUDIO_ACTION_TOPIC,
                Priority::Default,
                None,
                Some(&signed),
            )?;
            self.audio_last_sent_ms.insert(pending.request_id, now_ms);
        }
        Ok(())
    }

    fn publish_peer_convergence(&mut self, persist: &Persist, now_ms: i64) -> anyhow::Result<()> {
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
        let mut peer_snapshots = BTreeMap::new();
        for peer_id in &approved_peer_ids {
            if peer_id != &local_node_id {
                if let Some(peer) = read_peer_snapshot(persist, peer_id, &self.context(now_ms))? {
                    peer_snapshots.insert(peer_id.clone(), peer);
                }
            }
        }

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
                let Some(peer) = peer_snapshots.get(target) else {
                    continue;
                };
                let delivered = peer.schedules.iter().any(|candidate| {
                    candidate.schedule_id == schedule.schedule_id
                        && candidate.origin_node_id == schedule.origin_node_id
                        && candidate.revision >= schedule.revision
                });
                if !delivered {
                    let request_id = peer_request_id(
                        "schedule",
                        target,
                        &schedule.schedule_id,
                        schedule.revision,
                        "",
                    );
                    self.publish_peer_command(
                        persist,
                        target,
                        peer.revision,
                        request_id,
                        &schedule.origin_node_id,
                        ClockCommandKindV1::UpsertSchedule {
                            schedule: schedule.clone(),
                        },
                        now_ms,
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
                let Some(peer) = peer_snapshots.get(target) else {
                    continue;
                };
                let converged = peer
                    .occurrences
                    .iter()
                    .find(|candidate| candidate.global_event_id == occurrence.global_event_id)
                    .and_then(|candidate| candidate.acknowledgement.as_ref())
                    .is_some_and(|current| !acknowledgement_wins(acknowledgement, current));
                if converged {
                    continue;
                }
                let request_id = peer_request_id(
                    "ack",
                    target,
                    &occurrence.global_event_id,
                    acknowledgement.actor_clock,
                    &acknowledgement.acknowledgement_id,
                );
                self.publish_peer_command(
                    persist,
                    target,
                    peer.revision,
                    request_id,
                    &acknowledgement.actor_node_id,
                    ClockCommandKindV1::Acknowledge {
                        occurrence_id: occurrence.occurrence_id.clone(),
                        acknowledgement: acknowledgement.clone(),
                    },
                    now_ms,
                )?;
            }
        }
        Ok(())
    }

    fn publish_peer_command(
        &mut self,
        persist: &Persist,
        target: &str,
        expected_revision: u64,
        request_id: String,
        command_origin: &str,
        body: ClockCommandKindV1,
        now_ms: i64,
    ) -> anyhow::Result<()> {
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
        persist.write(
            &clock_command_topic(target)?,
            Priority::Default,
            None,
            Some(&serde_json::to_string(&command)?),
        )?;
        self.peer_last_sent_ms.insert(request_id, now_ms);
        Ok(())
    }
}

#[async_trait::async_trait]
impl Worker for ClockWorker {
    fn name(&self) -> &'static str {
        "clock"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        self.tick_once()?;
        let mut tick = tokio::time::interval(self.poll);
        tick.tick().await;
        loop {
            tokio::select! {
                _ = tick.tick() => self.tick_once()?,
                () = shutdown.wait() => break,
            }
        }
        Ok(())
    }
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
    candidate.actor_clock > current.actor_clock
        || (candidate.actor_clock == current.actor_clock
            && (candidate.stop && !current.stop
                || (candidate.stop == current.stop
                    && (
                        candidate.actor_node_id.as_str(),
                        candidate.acknowledgement_id.as_str(),
                    ) > (
                        current.actor_node_id.as_str(),
                        current.acknowledgement_id.as_str(),
                    ))))
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

fn stamp_revision(snapshot: &mut ClockSnapshotV1) {
    for occurrence in &mut snapshot.occurrences {
        occurrence.revision = snapshot.revision;
        for target in &mut occurrence.targets {
            target.revision = snapshot.revision;
            target.observed_at_utc_ms = snapshot.produced_at_utc_ms;
        }
    }
    for stopwatch in &mut snapshot.stopwatches {
        stopwatch.revision = snapshot.revision;
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
            issued_at_utc_ms: now_ms,
            expires_at_utc_ms: now_ms.saturating_add(MAX_CLOCK_AUDIO_REQUEST_TTL_MS),
            body,
            music_auth: None,
        };
        request.validate_at(now_ms)?;
        requests.push(writer::ClockAudioOutboxWrite {
            request_id,
            occurrence_id: request.occurrence_id.clone(),
            global_event_id: request.global_event_id.clone(),
            occurrence_generation: generation,
            request_json: serde_json::to_string(&request)?,
            created_at_ms: now_ms,
        });
    }
    Ok(requests)
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
    use std::sync::atomic::{AtomicI64, Ordering};
    use tempfile::TempDir;

    const NOW: i64 = 1_800_000_000_000;

    struct AdjustableClock(AtomicI64);
    impl WallClock for AdjustableClock {
        fn now_ms(&self) -> i64 {
            self.0.load(Ordering::Relaxed)
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
        stamp_revision(&mut replay);
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
}
