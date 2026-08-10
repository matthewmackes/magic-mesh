//! WL-FUNC-022 S1 — bounded Clock wire contracts.

#![allow(missing_docs, reason = "closed v1 wire fields are self-describing")]
#![allow(clippy::missing_errors_doc)]

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{collections::BTreeSet, fmt};

pub const CLOCK_SCHEMA_VERSION: u16 = 1;
pub const CLOCK_COMMAND_PREFIX: &str = "action/clock/command/";
pub const CLOCK_STATE_PREFIX: &str = "state/clock/";
pub const CLOCK_NOTIFY_PREFIX: &str = "event/notify/clock/";
pub const CLOCK_REPLY_PREFIX: &str = "reply/";
pub const CLOCK_AUDIO_ACTION_TOPIC: &str = "action/music/clock-audio";
pub const CLOCK_AUDIO_STATUS_PREFIX: &str = "state/music/clock-audio/";
pub const CLOCK_COMMAND_SIGNATURE_DOMAIN: &str = "mcnf-clock-command-v1";
pub const MAX_CLOCK_COMMAND_BYTES: usize = 64 * 1024;
pub const MAX_CLOCK_SNAPSHOT_BYTES: usize = 256 * 1024;
pub const MAX_CLOCK_AUDIO_REQUEST_BYTES: usize = 16 * 1024;
pub const MAX_CLOCK_AUDIO_LEDGER_RECORDS: usize = 512;
pub const MAX_CLOCK_ID_BYTES: usize = 128;
pub const MAX_CLOCK_LABEL_BYTES: usize = 256;
pub const MAX_CLOCK_ZONE_BYTES: usize = 64;
pub const MAX_CLOCK_AUDIO_ID_BYTES: usize = 256;
pub const MAX_CLOCK_SCHEDULES: usize = 256;
pub const MAX_CLOCK_OCCURRENCES: usize = 512;
pub const MAX_CLOCK_TARGETS: usize = 32;
pub const MAX_CLOCK_LAPS: usize = 256;
pub const MAX_CLOCK_MIRRORS: usize = 32;
pub const MAX_CLOCK_TIMER_DURATION_MS: u64 = 365 * 24 * 60 * 60 * 1_000;
pub const MAX_CLOCK_STOPWATCH_ELAPSED_MS: u64 = MAX_CLOCK_TIMER_DURATION_MS;
pub const MAX_CLOCK_COMMAND_AGE_MS: i64 = 5 * 60 * 1_000;
pub const MAX_CLOCK_FUTURE_SKEW_MS: i64 = 30 * 1_000;
pub const MAX_CLOCK_COMMAND_TTL_MS: i64 = 5 * 60 * 1_000;
pub const MAX_CLOCK_AUDIO_REQUEST_TTL_MS: i64 = 30 * 1_000;
pub const DEFAULT_SNOOZE_MINUTES: u16 = 10;
pub const DEFAULT_AUTO_SILENCE_MINUTES: u16 = 10;

#[must_use]
pub fn clock_command_topic(target_node: &str) -> Result<String, ClockContractError> {
    validate_id(target_node, "target_node")?;
    Ok(format!("{CLOCK_COMMAND_PREFIX}{target_node}"))
}

#[must_use]
pub fn clock_state_topic(node: &str) -> Result<String, ClockContractError> {
    validate_id(node, "node")?;
    Ok(format!("{CLOCK_STATE_PREFIX}{node}"))
}

#[must_use]
pub fn clock_notify_topic(node: &str) -> Result<String, ClockContractError> {
    validate_id(node, "node")?;
    Ok(format!("{CLOCK_NOTIFY_PREFIX}{node}"))
}

#[must_use]
pub fn clock_reply_topic(request_id: &str) -> Result<String, ClockContractError> {
    validate_id(request_id, "request_id")?;
    Ok(format!("{CLOCK_REPLY_PREFIX}{request_id}"))
}

#[must_use]
pub fn clock_audio_status_topic(node: &str) -> Result<String, ClockContractError> {
    validate_id(node, "node")?;
    Ok(format!("{CLOCK_AUDIO_STATUS_PREFIX}{node}"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClockContractError {
    BodyTooLarge { bytes: usize, max: usize },
    MalformedWire,
    UnsupportedSchema(u16),
    InvalidField(&'static str),
    FieldTooLong { field: &'static str, max: usize },
    CapacityExceeded { field: &'static str, max: usize },
    Duplicate(&'static str),
    Stale(&'static str),
    Future(&'static str),
    InvalidRevision,
    MalformedSignature,
    UntrustedSigner,
    SignatureMismatch,
}

impl fmt::Display for ClockContractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid Clock contract: {self:?}")
    }
}

impl std::error::Error for ClockContractError {}

#[derive(Clone, Copy)]
pub struct ClockValidationContext<'a> {
    pub wall_utc_ms: i64,
    pub monotonic_ms: u64,
    pub zone_exists: &'a dyn Fn(&str) -> bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClockWeekday {
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    Sunday,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClockFoldPolicy {
    Earlier,
    Later,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClockGapPolicy {
    NextValid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClockCivilTimeV1 {
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub time_zone: String,
    pub fold: ClockFoldPolicy,
    pub gap: ClockGapPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClockAudioRef {
    Bundled {
        tone_id: String,
    },
    Music {
        source_id: String,
        remote_id: String,
        content_kind: ClockMusicKind,
        fallback_tone_id: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClockMusicKind {
    Track,
    PodcastEpisode,
    Radio,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClockAudioActionV1 {
    Resolve {
        audio: ClockAudioRef,
    },
    Preview {
        audio: ClockAudioRef,
        preview_volume_milli: u16,
        preview_duration_ms: u32,
    },
    Start {
        audio: ClockAudioRef,
        alarm_volume_milli: u16,
    },
    Stop {
        acknowledgement_id: String,
    },
    Snooze {
        acknowledgement_id: String,
        resume_at_utc_ms: i64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClockAudioRequestV1 {
    pub schema_version: u16,
    pub request_id: String,
    pub occurrence_id: String,
    pub global_event_id: String,
    pub occurrence_generation: u64,
    pub issued_at_utc_ms: i64,
    pub expires_at_utc_ms: i64,
    pub body: ClockAudioActionV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub music_auth: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClockAudioPlaybackPhase {
    Resolved,
    Previewing,
    PlayingBundled,
    PlayingMusic,
    PlayingFallback,
    Stopped,
    Snoozed,
    RefusedStale,
    ProviderUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClockAudioProviderStatus {
    NotApplicable,
    Available,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClockAudioStatusV1 {
    pub schema_version: u16,
    pub request_id: String,
    pub occurrence_id: String,
    pub global_event_id: String,
    pub occurrence_generation: u64,
    pub observed_at_utc_ms: i64,
    pub phase: ClockAudioPlaybackPhase,
    pub provider_status: ClockAudioProviderStatus,
    pub fallback_tone_id: Option<String>,
    pub acknowledgement_id: Option<String>,
    pub reason_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "recurrence", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClockAlarmRecurrenceV1 {
    OneTime {
        due_at_utc_ms: i64,
    },
    Weekdays {
        local_time: ClockCivilTimeV1,
        weekdays: Vec<ClockWeekday>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClockAlarmV1 {
    pub enabled: bool,
    pub label: String,
    pub recurrence: ClockAlarmRecurrenceV1,
    pub sound: ClockAudioRef,
    pub vibrate: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClockTimerPhase {
    Running,
    Paused,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClockTimerV1 {
    pub original_duration_ms: u64,
    pub phase: ClockTimerPhase,
    pub absolute_deadline_utc_ms: Option<i64>,
    pub paused_remaining_ms: Option<u64>,
    pub expired_at_utc_ms: Option<i64>,
    pub sound: ClockAudioRef,
    pub vibrate: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClockScheduleKindV1 {
    Alarm(ClockAlarmV1),
    Timer(ClockTimerV1),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClockScheduleV1 {
    pub schedule_id: String,
    pub origin_node_id: String,
    pub revision: u64,
    pub label: String,
    pub selected_target_ids: Vec<String>,
    pub schedule: ClockScheduleKindV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClockTargetDisposition {
    Pending,
    Delivered,
    Ringing,
    Snoozed,
    Stopped,
    Missed,
    DisabledLocally,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClockTargetState {
    pub target_node_id: String,
    pub disposition: ClockTargetDisposition,
    pub revision: u64,
    pub observed_at_utc_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClockOccurrencePhase {
    Scheduled,
    Ringing,
    Snoozed,
    Stopped,
    Missed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClockAcknowledgementV1 {
    pub acknowledgement_id: String,
    pub global_event_id: String,
    pub actor_node_id: String,
    pub actor_clock: u64,
    pub acknowledged_at_utc_ms: i64,
    pub stop: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClockOccurrenceV1 {
    pub occurrence_id: String,
    pub global_event_id: String,
    pub schedule_id: String,
    pub revision: u64,
    pub due_at_utc_ms: i64,
    pub phase: ClockOccurrencePhase,
    pub targets: Vec<ClockTargetState>,
    pub acknowledgement: Option<ClockAcknowledgementV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClockLapV1 {
    pub lap_id: String,
    pub split_elapsed_ms: u64,
    pub total_elapsed_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClockStopwatchPhase {
    Reset,
    Running,
    Paused,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClockStopwatchV1 {
    pub stopwatch_id: String,
    pub origin_node_id: String,
    pub mirror_target_ids: Vec<String>,
    pub revision: u64,
    pub phase: ClockStopwatchPhase,
    pub started_wall_utc_ms: Option<i64>,
    pub started_monotonic_ms: Option<u64>,
    pub accumulated_elapsed_ms: u64,
    pub laps: Vec<ClockLapV1>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClockVolumeKeyBehavior {
    Volume,
    Snooze,
    Stop,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClockSettingsV1 {
    pub use_24_hour: bool,
    pub this_node_time_zone: String,
    pub world_clock_time_zones: Vec<String>,
    pub snooze_minutes: u16,
    pub auto_silence_minutes: u16,
    pub alarm_crescendo: bool,
    pub timer_crescendo: bool,
    pub volume_key_behavior: ClockVolumeKeyBehavior,
}

impl ClockSettingsV1 {
    #[must_use]
    pub fn defaults(this_node_time_zone: String) -> Self {
        Self {
            use_24_hour: true,
            this_node_time_zone,
            world_clock_time_zones: Vec::new(),
            snooze_minutes: DEFAULT_SNOOZE_MINUTES,
            auto_silence_minutes: DEFAULT_AUTO_SILENCE_MINUTES,
            alarm_crescendo: false,
            timer_crescendo: false,
            volume_key_behavior: ClockVolumeKeyBehavior::Volume,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClockCommandKindV1 {
    UpsertSchedule {
        schedule: ClockScheduleV1,
    },
    RemoveSchedule {
        schedule_id: String,
    },
    SetScheduleEnabled {
        schedule_id: String,
        enabled: bool,
    },
    Acknowledge {
        occurrence_id: String,
        acknowledgement: ClockAcknowledgementV1,
    },
    UpsertStopwatch {
        stopwatch: ClockStopwatchV1,
    },
    SetSettings {
        settings: ClockSettingsV1,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClockCommandV1 {
    pub schema_version: u16,
    pub request_id: String,
    pub origin_node_id: String,
    pub expected_revision: u64,
    pub issued_at_utc_ms: i64,
    pub expires_at_utc_ms: i64,
    pub body: ClockCommandKindV1,
    pub signer_id: String,
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClockSnapshotV1 {
    pub schema_version: u16,
    pub node_id: String,
    pub revision: u64,
    pub produced_at_utc_ms: i64,
    pub schedules: Vec<ClockScheduleV1>,
    pub occurrences: Vec<ClockOccurrenceV1>,
    pub stopwatches: Vec<ClockStopwatchV1>,
    pub settings: ClockSettingsV1,
}

impl ClockCommandV1 {
    pub fn sign(
        mut self,
        signer_id: impl Into<String>,
        signing_key: &SigningKey,
        context: &ClockValidationContext<'_>,
    ) -> Result<Self, ClockContractError> {
        self.signer_id = signer_id.into();
        validate_id(&self.signer_id, "signer_id")?;
        self.signature.clear();
        self.validate_unsigned_at(context)?;
        self.signature = encode_hex(&signing_key.sign(&self.signing_bytes()?).to_bytes());
        Ok(self)
    }

    pub fn admit_at(
        self,
        trusted_signer_id: &str,
        verifying_key: &VerifyingKey,
        context: &ClockValidationContext<'_>,
    ) -> Result<Self, ClockContractError> {
        self.validate_at(context)?;
        if self.signer_id != trusted_signer_id {
            return Err(ClockContractError::UntrustedSigner);
        }
        let signature =
            decode_hex_64(&self.signature).ok_or(ClockContractError::MalformedSignature)?;
        verifying_key
            .verify(&self.signing_bytes()?, &Signature::from_bytes(&signature))
            .map_err(|_| ClockContractError::SignatureMismatch)?;
        Ok(self)
    }

    pub fn from_json_at(
        bytes: &[u8],
        context: &ClockValidationContext<'_>,
    ) -> Result<Self, ClockContractError> {
        let value: Self = decode_json(bytes, MAX_CLOCK_COMMAND_BYTES)?;
        value.validate_at(context)?;
        Ok(value)
    }

    pub fn validate_at(
        &self,
        context: &ClockValidationContext<'_>,
    ) -> Result<(), ClockContractError> {
        self.validate_unsigned_at(context)?;
        validate_id(&self.signer_id, "signer_id")?;
        if decode_hex_64(&self.signature).is_none() {
            return Err(ClockContractError::MalformedSignature);
        }
        Ok(())
    }

    fn validate_unsigned_at(
        &self,
        context: &ClockValidationContext<'_>,
    ) -> Result<(), ClockContractError> {
        validate_version(self.schema_version)?;
        validate_id(&self.request_id, "request_id")?;
        validate_id(&self.origin_node_id, "origin_node_id")?;
        if self.expected_revision == u64::MAX {
            return Err(ClockContractError::InvalidRevision);
        }
        validate_command_window(
            self.issued_at_utc_ms,
            self.expires_at_utc_ms,
            context.wall_utc_ms,
        )?;
        match &self.body {
            ClockCommandKindV1::UpsertSchedule { schedule } => {
                schedule.validate_with_elapsed(context, true)
            }
            ClockCommandKindV1::RemoveSchedule { schedule_id }
            | ClockCommandKindV1::SetScheduleEnabled { schedule_id, .. } => {
                validate_id(schedule_id, "schedule_id")
            }
            ClockCommandKindV1::Acknowledge {
                occurrence_id,
                acknowledgement,
            } => {
                validate_id(occurrence_id, "occurrence_id")?;
                acknowledgement.validate_at(context.wall_utc_ms)
            }
            ClockCommandKindV1::UpsertStopwatch { stopwatch } => stopwatch.validate_at(context),
            ClockCommandKindV1::SetSettings { settings } => settings.validate_at(context),
        }
    }

    fn signing_bytes(&self) -> Result<Vec<u8>, ClockContractError> {
        let mut unsigned = self.clone();
        unsigned.signature.clear();
        let payload =
            serde_json::to_vec(&unsigned).map_err(|_| ClockContractError::MalformedWire)?;
        let mut bytes =
            Vec::with_capacity(CLOCK_COMMAND_SIGNATURE_DOMAIN.len() + payload.len() + 1);
        bytes.extend_from_slice(CLOCK_COMMAND_SIGNATURE_DOMAIN.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&payload);
        Ok(bytes)
    }
}

impl ClockAudioRequestV1 {
    pub fn from_json_at(bytes: &[u8], now_ms: i64) -> Result<Self, ClockContractError> {
        let value: Self = decode_json(bytes, MAX_CLOCK_AUDIO_REQUEST_BYTES)?;
        value.validate_at(now_ms)?;
        Ok(value)
    }

    pub fn validate_at(&self, now_ms: i64) -> Result<(), ClockContractError> {
        validate_version(self.schema_version)?;
        validate_id(&self.request_id, "request_id")?;
        validate_id(&self.occurrence_id, "occurrence_id")?;
        validate_id(&self.global_event_id, "global_event_id")?;
        validate_revision(self.occurrence_generation)?;
        validate_command_window(self.issued_at_utc_ms, self.expires_at_utc_ms, now_ms)?;
        if self.expires_at_utc_ms.saturating_sub(self.issued_at_utc_ms)
            > MAX_CLOCK_AUDIO_REQUEST_TTL_MS
        {
            return Err(ClockContractError::InvalidField("clock_audio_window"));
        }
        match &self.body {
            ClockAudioActionV1::Resolve { audio } => audio.validate()?,
            ClockAudioActionV1::Preview {
                audio,
                preview_volume_milli,
                preview_duration_ms,
            } => {
                audio.validate()?;
                if *preview_volume_milli == 0 || *preview_volume_milli > 1_000 {
                    return Err(ClockContractError::InvalidField("preview_volume_milli"));
                }
                if *preview_duration_ms == 0 || *preview_duration_ms > 10_000 {
                    return Err(ClockContractError::InvalidField("preview_duration_ms"));
                }
            }
            ClockAudioActionV1::Start {
                audio,
                alarm_volume_milli,
            } => {
                audio.validate()?;
                if *alarm_volume_milli == 0 || *alarm_volume_milli > 1_000 {
                    return Err(ClockContractError::InvalidField("alarm_volume_milli"));
                }
            }
            ClockAudioActionV1::Stop { acknowledgement_id } => {
                validate_id(acknowledgement_id, "acknowledgement_id")?;
            }
            ClockAudioActionV1::Snooze {
                acknowledgement_id,
                resume_at_utc_ms,
            } => {
                validate_id(acknowledgement_id, "acknowledgement_id")?;
                if *resume_at_utc_ms <= now_ms
                    || *resume_at_utc_ms > now_ms.saturating_add(MAX_CLOCK_TIMER_DURATION_MS as i64)
                {
                    return Err(ClockContractError::InvalidField("resume_at_utc_ms"));
                }
            }
        }
        if self
            .music_auth
            .as_ref()
            .is_some_and(|value| !value.is_object())
        {
            return Err(ClockContractError::InvalidField("music_auth"));
        }
        Ok(())
    }
}

impl ClockAudioStatusV1 {
    pub fn validate_at(&self, now_ms: i64) -> Result<(), ClockContractError> {
        validate_version(self.schema_version)?;
        validate_id(&self.request_id, "request_id")?;
        validate_id(&self.occurrence_id, "occurrence_id")?;
        validate_id(&self.global_event_id, "global_event_id")?;
        validate_revision(self.occurrence_generation)?;
        validate_observed(self.observed_at_utc_ms, now_ms, "observed_at_utc_ms")?;
        if let Some(value) = &self.fallback_tone_id {
            validate_audio_id(value, "fallback_tone_id")?;
        }
        if let Some(value) = &self.acknowledgement_id {
            validate_id(value, "acknowledgement_id")?;
        }
        if let Some(value) = &self.reason_code {
            validate_id(value, "reason_code")?;
        }
        match self.phase {
            ClockAudioPlaybackPhase::Resolved | ClockAudioPlaybackPhase::Previewing
                if self.provider_status != ClockAudioProviderStatus::Available =>
            {
                Err(ClockContractError::InvalidField("provider_status"))
            }
            ClockAudioPlaybackPhase::PlayingMusic
                if self.provider_status != ClockAudioProviderStatus::Available =>
            {
                Err(ClockContractError::InvalidField("provider_status"))
            }
            ClockAudioPlaybackPhase::PlayingFallback
                if self.provider_status != ClockAudioProviderStatus::Unavailable
                    || self.fallback_tone_id.is_none() =>
            {
                Err(ClockContractError::InvalidField("fallback_status"))
            }
            ClockAudioPlaybackPhase::ProviderUnavailable
                if self.provider_status != ClockAudioProviderStatus::Unavailable =>
            {
                Err(ClockContractError::InvalidField("provider_status"))
            }
            ClockAudioPlaybackPhase::Stopped | ClockAudioPlaybackPhase::Snoozed
                if self.acknowledgement_id.is_none() =>
            {
                Err(ClockContractError::InvalidField("acknowledgement_id"))
            }
            _ => Ok(()),
        }
    }
}

impl ClockSnapshotV1 {
    pub fn from_json_at(
        bytes: &[u8],
        context: &ClockValidationContext<'_>,
    ) -> Result<Self, ClockContractError> {
        let value: Self = decode_json(bytes, MAX_CLOCK_SNAPSHOT_BYTES)?;
        value.validate_at(context)?;
        Ok(value)
    }

    pub fn validate_at(
        &self,
        context: &ClockValidationContext<'_>,
    ) -> Result<(), ClockContractError> {
        self.validate_with_elapsed(context, false)
    }

    pub fn from_persisted_json_at(
        bytes: &[u8],
        context: &ClockValidationContext<'_>,
    ) -> Result<Self, ClockContractError> {
        let value: Self = decode_json(bytes, MAX_CLOCK_SNAPSHOT_BYTES)?;
        value.validate_with_elapsed(context, true)?;
        Ok(value)
    }

    fn validate_with_elapsed(
        &self,
        context: &ClockValidationContext<'_>,
        allow_elapsed_deadlines: bool,
    ) -> Result<(), ClockContractError> {
        validate_version(self.schema_version)?;
        validate_id(&self.node_id, "node_id")?;
        validate_revision(self.revision)?;
        validate_observed(
            self.produced_at_utc_ms,
            context.wall_utc_ms,
            "produced_at_utc_ms",
        )?;
        validate_capacity(&self.schedules, MAX_CLOCK_SCHEDULES, "schedules")?;
        validate_capacity(&self.occurrences, MAX_CLOCK_OCCURRENCES, "occurrences")?;
        validate_capacity(&self.stopwatches, MAX_CLOCK_MIRRORS, "stopwatches")?;
        unique(
            self.schedules.iter().map(|v| v.schedule_id.as_str()),
            "schedules",
        )?;
        unique(
            self.occurrences.iter().map(|v| v.occurrence_id.as_str()),
            "occurrences",
        )?;
        unique(
            self.stopwatches.iter().map(|v| v.stopwatch_id.as_str()),
            "stopwatches",
        )?;
        for value in &self.schedules {
            value.validate_with_elapsed(context, allow_elapsed_deadlines)?;
        }
        for value in &self.occurrences {
            value.validate_at(context)?;
        }
        for value in &self.stopwatches {
            value.validate_at(context)?;
        }
        self.settings.validate_at(context)
    }
}

impl ClockScheduleV1 {
    pub fn validate_at(
        &self,
        context: &ClockValidationContext<'_>,
    ) -> Result<(), ClockContractError> {
        self.validate_with_elapsed(context, false)
    }

    fn validate_with_elapsed(
        &self,
        context: &ClockValidationContext<'_>,
        allow_elapsed_deadlines: bool,
    ) -> Result<(), ClockContractError> {
        validate_id(&self.schedule_id, "schedule_id")?;
        validate_id(&self.origin_node_id, "origin_node_id")?;
        validate_revision(self.revision)?;
        validate_label(&self.label)?;
        validate_ids(
            &self.selected_target_ids,
            MAX_CLOCK_TARGETS,
            "selected_target_ids",
        )?;
        if self.selected_target_ids.is_empty() {
            return Err(ClockContractError::InvalidField("selected_target_ids"));
        }
        match &self.schedule {
            ClockScheduleKindV1::Alarm(alarm) => {
                alarm.validate_at(context, allow_elapsed_deadlines)
            }
            ClockScheduleKindV1::Timer(timer) => {
                timer.validate_at(context.wall_utc_ms, allow_elapsed_deadlines)
            }
        }
    }
}

impl ClockAlarmV1 {
    fn validate_at(
        &self,
        context: &ClockValidationContext<'_>,
        allow_elapsed_deadlines: bool,
    ) -> Result<(), ClockContractError> {
        validate_label(&self.label)?;
        self.sound.validate()?;
        match &self.recurrence {
            ClockAlarmRecurrenceV1::OneTime { due_at_utc_ms } => {
                if *due_at_utc_ms <= 0
                    || *due_at_utc_ms
                        > context
                            .wall_utc_ms
                            .saturating_add(MAX_CLOCK_TIMER_DURATION_MS as i64)
                {
                    return Err(ClockContractError::InvalidField("due_at_utc_ms"));
                }
                if self.enabled && !allow_elapsed_deadlines && *due_at_utc_ms <= context.wall_utc_ms
                {
                    return Err(ClockContractError::Stale("due_at_utc_ms"));
                }
                Ok(())
            }
            ClockAlarmRecurrenceV1::Weekdays {
                local_time,
                weekdays,
            } => {
                if weekdays.is_empty() || weekdays.len() > 7 {
                    return Err(ClockContractError::CapacityExceeded {
                        field: "weekdays",
                        max: 7,
                    });
                }
                unique(weekdays.iter().map(|v| format!("{v:?}")), "weekdays")?;
                local_time.validate(context)
            }
        }
    }
}

impl ClockCivilTimeV1 {
    fn validate(&self, context: &ClockValidationContext<'_>) -> Result<(), ClockContractError> {
        if self.hour > 23 || self.minute > 59 || self.second > 59 {
            return Err(ClockContractError::InvalidField("local_time"));
        }
        validate_zone(&self.time_zone, context)
    }
}

impl ClockAudioRef {
    /// Validate that every caller-visible component is a bounded opaque ID,
    /// never a provider URL or filesystem locator.
    pub fn validate(&self) -> Result<(), ClockContractError> {
        match self {
            Self::Bundled { tone_id } => validate_audio_id(tone_id, "tone_id"),
            Self::Music {
                source_id,
                remote_id,
                fallback_tone_id,
                ..
            } => {
                validate_audio_id(source_id, "source_id")?;
                validate_audio_id(remote_id, "remote_id")?;
                validate_audio_id(fallback_tone_id, "fallback_tone_id")
            }
        }
    }
}

impl ClockTimerV1 {
    fn validate_at(
        &self,
        now_ms: i64,
        allow_elapsed_deadlines: bool,
    ) -> Result<(), ClockContractError> {
        if self.original_duration_ms == 0 || self.original_duration_ms > MAX_CLOCK_TIMER_DURATION_MS
        {
            return Err(ClockContractError::InvalidField("original_duration_ms"));
        }
        self.sound.validate()?;
        match self.phase {
            ClockTimerPhase::Running => match (
                self.absolute_deadline_utc_ms,
                self.paused_remaining_ms,
                self.expired_at_utc_ms,
            ) {
                (Some(deadline), None, None)
                    if deadline > 0 && (allow_elapsed_deadlines || deadline > now_ms) =>
                {
                    Ok(())
                }
                _ => Err(ClockContractError::InvalidField("timer_running_state")),
            },
            ClockTimerPhase::Paused => match (
                self.absolute_deadline_utc_ms,
                self.paused_remaining_ms,
                self.expired_at_utc_ms,
            ) {
                (None, Some(remaining), None) if remaining <= self.original_duration_ms => Ok(()),
                _ => Err(ClockContractError::InvalidField("timer_paused_state")),
            },
            ClockTimerPhase::Expired => match (
                self.absolute_deadline_utc_ms,
                self.paused_remaining_ms,
                self.expired_at_utc_ms,
            ) {
                (Some(deadline), None, Some(expired))
                    if deadline == expired && expired <= now_ms =>
                {
                    Ok(())
                }
                _ => Err(ClockContractError::InvalidField("timer_expired_state")),
            },
        }
    }
}

impl ClockTargetState {
    fn validate_at(&self, now_ms: i64) -> Result<(), ClockContractError> {
        validate_id(&self.target_node_id, "target_node_id")?;
        validate_revision(self.revision)?;
        validate_observed(self.observed_at_utc_ms, now_ms, "target_observed_at_utc_ms")
    }
}

impl ClockAcknowledgementV1 {
    fn validate_at(&self, now_ms: i64) -> Result<(), ClockContractError> {
        validate_id(&self.acknowledgement_id, "acknowledgement_id")?;
        validate_id(&self.global_event_id, "global_event_id")?;
        validate_id(&self.actor_node_id, "actor_node_id")?;
        if self.actor_clock == 0 {
            return Err(ClockContractError::InvalidRevision);
        }
        validate_observed(
            self.acknowledged_at_utc_ms,
            now_ms,
            "acknowledged_at_utc_ms",
        )
    }
}

impl ClockOccurrenceV1 {
    fn validate_at(&self, context: &ClockValidationContext<'_>) -> Result<(), ClockContractError> {
        validate_id(&self.occurrence_id, "occurrence_id")?;
        validate_id(&self.global_event_id, "global_event_id")?;
        validate_id(&self.schedule_id, "schedule_id")?;
        validate_revision(self.revision)?;
        if self.due_at_utc_ms <= 0
            || self.due_at_utc_ms
                > context
                    .wall_utc_ms
                    .saturating_add(MAX_CLOCK_TIMER_DURATION_MS as i64)
        {
            return Err(ClockContractError::Future("due_at_utc_ms"));
        }
        validate_capacity(&self.targets, MAX_CLOCK_TARGETS, "targets")?;
        unique(
            self.targets.iter().map(|v| v.target_node_id.as_str()),
            "targets",
        )?;
        for target in &self.targets {
            target.validate_at(context.wall_utc_ms)?;
        }
        if let Some(ack) = &self.acknowledgement {
            ack.validate_at(context.wall_utc_ms)?;
            if ack.global_event_id != self.global_event_id {
                return Err(ClockContractError::InvalidField(
                    "acknowledgement.global_event_id",
                ));
            }
        }
        Ok(())
    }
}

impl ClockStopwatchV1 {
    fn validate_at(&self, context: &ClockValidationContext<'_>) -> Result<(), ClockContractError> {
        validate_id(&self.stopwatch_id, "stopwatch_id")?;
        validate_id(&self.origin_node_id, "origin_node_id")?;
        validate_revision(self.revision)?;
        validate_ids(
            &self.mirror_target_ids,
            MAX_CLOCK_MIRRORS,
            "mirror_target_ids",
        )?;
        if self.accumulated_elapsed_ms > MAX_CLOCK_STOPWATCH_ELAPSED_MS {
            return Err(ClockContractError::InvalidField("accumulated_elapsed_ms"));
        }
        validate_capacity(&self.laps, MAX_CLOCK_LAPS, "laps")?;
        unique(self.laps.iter().map(|lap| lap.lap_id.as_str()), "laps")?;
        let mut last_total = 0;
        for lap in &self.laps {
            validate_id(&lap.lap_id, "lap_id")?;
            if lap.split_elapsed_ms == 0
                || lap.total_elapsed_ms <= last_total
                || lap.total_elapsed_ms > self.accumulated_elapsed_ms
            {
                return Err(ClockContractError::InvalidField("laps"));
            }
            last_total = lap.total_elapsed_ms;
        }
        match self.phase {
            ClockStopwatchPhase::Running => {
                match (self.started_wall_utc_ms, self.started_monotonic_ms) {
                    // A monotonic timestamp is meaningful only in the
                    // originating process' clock domain.  Its presence binds
                    // the running state, but a receiver (especially a peer)
                    // must not compare the opaque value with its own uptime.
                    (Some(wall), Some(_))
                        if wall <= context.wall_utc_ms.saturating_add(MAX_CLOCK_FUTURE_SKEW_MS) =>
                    {
                        Ok(())
                    }
                    _ => Err(ClockContractError::InvalidField("stopwatch_running_state")),
                }
            }
            ClockStopwatchPhase::Reset
                if self.started_wall_utc_ms.is_none()
                    && self.started_monotonic_ms.is_none()
                    && self.accumulated_elapsed_ms == 0
                    && self.laps.is_empty() =>
            {
                Ok(())
            }
            ClockStopwatchPhase::Paused
                if self.started_wall_utc_ms.is_none() && self.started_monotonic_ms.is_none() =>
            {
                Ok(())
            }
            _ => Err(ClockContractError::InvalidField("stopwatch_state")),
        }
    }
}

impl ClockSettingsV1 {
    fn validate_at(&self, context: &ClockValidationContext<'_>) -> Result<(), ClockContractError> {
        if !self.use_24_hour {
            return Err(ClockContractError::InvalidField("use_24_hour"));
        }
        validate_zone(&self.this_node_time_zone, context)?;
        validate_capacity(
            &self.world_clock_time_zones,
            MAX_CLOCK_TARGETS,
            "world_clock_time_zones",
        )?;
        unique(
            self.world_clock_time_zones.iter().map(String::as_str),
            "world_clock_time_zones",
        )?;
        for zone in &self.world_clock_time_zones {
            validate_zone(zone, context)?;
        }
        if !(1..=60).contains(&self.snooze_minutes) {
            return Err(ClockContractError::InvalidField("snooze_minutes"));
        }
        if !(1..=60).contains(&self.auto_silence_minutes) {
            return Err(ClockContractError::InvalidField("auto_silence_minutes"));
        }
        Ok(())
    }
}

fn decode_json<T: DeserializeOwned>(bytes: &[u8], max: usize) -> Result<T, ClockContractError> {
    if bytes.len() > max {
        return Err(ClockContractError::BodyTooLarge {
            bytes: bytes.len(),
            max,
        });
    }
    let text = std::str::from_utf8(bytes).map_err(|_| ClockContractError::MalformedWire)?;
    crate::workloads::reject_duplicate_json_keys(text)
        .map_err(|_| ClockContractError::MalformedWire)?;
    serde_json::from_str(text).map_err(|_| ClockContractError::MalformedWire)
}

fn validate_version(version: u16) -> Result<(), ClockContractError> {
    if version == CLOCK_SCHEMA_VERSION {
        Ok(())
    } else {
        Err(ClockContractError::UnsupportedSchema(version))
    }
}

fn validate_revision(revision: u64) -> Result<(), ClockContractError> {
    if revision == 0 || revision == u64::MAX {
        Err(ClockContractError::InvalidRevision)
    } else {
        Ok(())
    }
}

fn validate_id(value: &str, field: &'static str) -> Result<(), ClockContractError> {
    if value.is_empty()
        || value.len() > MAX_CLOCK_ID_BYTES
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
    {
        return Err(ClockContractError::InvalidField(field));
    }
    Ok(())
}

fn validate_label(value: &str) -> Result<(), ClockContractError> {
    if value.trim().is_empty()
        || value.len() > MAX_CLOCK_LABEL_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(ClockContractError::InvalidField("label"));
    }
    Ok(())
}

fn validate_audio_id(value: &str, field: &'static str) -> Result<(), ClockContractError> {
    if value.trim().is_empty()
        || value.len() > MAX_CLOCK_AUDIO_ID_BYTES
        || value.chars().any(char::is_control)
        || value.contains("://")
        || value.starts_with('/')
        || value.starts_with("file:")
        || value.contains('\\')
        || value.split('/').any(|component| component == "..")
    {
        return Err(ClockContractError::InvalidField(field));
    }
    Ok(())
}

fn validate_zone(
    value: &str,
    context: &ClockValidationContext<'_>,
) -> Result<(), ClockContractError> {
    if value.is_empty()
        || value.len() > MAX_CLOCK_ZONE_BYTES
        || value.starts_with('/')
        || value.contains("..")
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'-' | b'+'))
        || !(context.zone_exists)(value)
    {
        return Err(ClockContractError::InvalidField("time_zone"));
    }
    Ok(())
}

fn validate_command_window(issued: i64, expires: i64, now: i64) -> Result<(), ClockContractError> {
    if issued <= 0 || expires <= issued || expires.saturating_sub(issued) > MAX_CLOCK_COMMAND_TTL_MS
    {
        return Err(ClockContractError::InvalidField("command_window"));
    }
    if issued < now.saturating_sub(MAX_CLOCK_COMMAND_AGE_MS) || expires < now {
        return Err(ClockContractError::Stale("issued_at_utc_ms"));
    }
    if issued > now.saturating_add(MAX_CLOCK_FUTURE_SKEW_MS) {
        return Err(ClockContractError::Future("issued_at_utc_ms"));
    }
    Ok(())
}

fn validate_observed(value: i64, now: i64, field: &'static str) -> Result<(), ClockContractError> {
    if value <= 0 {
        return Err(ClockContractError::InvalidField(field));
    }
    if value > now.saturating_add(MAX_CLOCK_FUTURE_SKEW_MS) {
        return Err(ClockContractError::Future(field));
    }
    Ok(())
}

fn validate_capacity<T>(
    values: &[T],
    max: usize,
    field: &'static str,
) -> Result<(), ClockContractError> {
    if values.len() > max {
        Err(ClockContractError::CapacityExceeded { field, max })
    } else {
        Ok(())
    }
}

fn validate_ids(
    values: &[String],
    max: usize,
    field: &'static str,
) -> Result<(), ClockContractError> {
    validate_capacity(values, max, field)?;
    unique(values.iter().map(String::as_str), field)?;
    for value in values {
        validate_id(value, field)?;
    }
    Ok(())
}

fn unique<I, T>(values: I, field: &'static str) -> Result<(), ClockContractError>
where
    I: IntoIterator<Item = T>,
    T: Ord,
{
    let mut seen = BTreeSet::new();
    for value in values {
        if !seen.insert(value) {
            return Err(ClockContractError::Duplicate(field));
        }
    }
    Ok(())
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn decode_hex_64(value: &str) -> Option<[u8; 64]> {
    if value.len() != 128 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut decoded = [0_u8; 64];
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

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 2_000_000;
    fn zones(zone: &str) -> bool {
        matches!(zone, "America/New_York" | "Europe/London" | "UTC")
    }
    fn context() -> ClockValidationContext<'static> {
        ClockValidationContext {
            wall_utc_ms: NOW,
            monotonic_ms: 90_000,
            zone_exists: &zones,
        }
    }
    fn audio() -> ClockAudioRef {
        ClockAudioRef::Bundled {
            tone_id: "bright-bell".into(),
        }
    }
    fn schedule() -> ClockScheduleV1 {
        ClockScheduleV1 {
            schedule_id: "alarm-1".into(),
            origin_node_id: "seat-1".into(),
            revision: 1,
            label: "Weekday alarm".into(),
            selected_target_ids: vec!["seat-1".into()],
            schedule: ClockScheduleKindV1::Alarm(ClockAlarmV1 {
                enabled: true,
                label: "Wake".into(),
                recurrence: ClockAlarmRecurrenceV1::Weekdays {
                    local_time: ClockCivilTimeV1 {
                        hour: 7,
                        minute: 30,
                        second: 0,
                        time_zone: "America/New_York".into(),
                        fold: ClockFoldPolicy::Earlier,
                        gap: ClockGapPolicy::NextValid,
                    },
                    weekdays: vec![ClockWeekday::Monday, ClockWeekday::Friday],
                },
                sound: audio(),
                vibrate: false,
            }),
        }
    }
    fn command() -> ClockCommandV1 {
        ClockCommandV1 {
            schema_version: 1,
            request_id: "request-1".into(),
            origin_node_id: "seat-1".into(),
            expected_revision: 0,
            issued_at_utc_ms: NOW - 1_000,
            expires_at_utc_ms: NOW + 1_000,
            body: ClockCommandKindV1::UpsertSchedule {
                schedule: schedule(),
            },
            signer_id: "seat-1-key".into(),
            signature: "00".repeat(64),
        }
    }

    fn signed_command() -> ClockCommandV1 {
        command()
            .sign("seat-1-key", &SigningKey::from_bytes(&[7; 32]), &context())
            .unwrap()
    }

    #[test]
    fn command_round_trip_is_closed_and_bounded() {
        let encoded = serde_json::to_vec(&signed_command()).unwrap();
        assert_eq!(
            ClockCommandV1::from_json_at(&encoded, &context()).unwrap(),
            signed_command()
        );
        let unknown = String::from_utf8(encoded)
            .unwrap()
            .replacen("{", "{\"surprise\":true,", 1);
        assert!(ClockCommandV1::from_json_at(unknown.as_bytes(), &context()).is_err());
        assert!(ClockCommandV1::from_json_at(
            br#"{"schema_version":1,"schema_version":1}"#,
            &context()
        )
        .is_err());
        let oversized = vec![b' '; MAX_CLOCK_COMMAND_BYTES + 1];
        assert!(matches!(
            ClockCommandV1::from_json_at(&oversized, &context()),
            Err(ClockContractError::BodyTooLarge { .. })
        ));
    }

    #[test]
    fn signed_command_rejects_wrong_signer_signature_and_replay_window() {
        let signing_key = SigningKey::from_bytes(&[7; 32]);
        let command = signed_command();
        assert!(command
            .clone()
            .admit_at("seat-1-key", &signing_key.verifying_key(), &context())
            .is_ok());
        assert!(matches!(
            command
                .clone()
                .admit_at("seat-2-key", &signing_key.verifying_key(), &context()),
            Err(ClockContractError::UntrustedSigner)
        ));
        let mut tampered = command;
        tampered.expected_revision = 9;
        assert!(matches!(
            tampered.admit_at("seat-1-key", &signing_key.verifying_key(), &context()),
            Err(ClockContractError::SignatureMismatch)
        ));
    }

    #[test]
    fn alarm_timer_zone_and_time_boundaries_fail_closed() {
        assert!(schedule().validate_at(&context()).is_ok());
        let mut one_time = schedule();
        let ClockScheduleKindV1::Alarm(alarm) = &mut one_time.schedule else {
            unreachable!();
        };
        alarm.recurrence = ClockAlarmRecurrenceV1::OneTime {
            due_at_utc_ms: NOW + 60_000,
        };
        assert!(one_time.validate_at(&context()).is_ok());
        let ClockScheduleKindV1::Alarm(alarm) = &mut one_time.schedule else {
            unreachable!();
        };
        alarm.recurrence = ClockAlarmRecurrenceV1::OneTime {
            due_at_utc_ms: NOW - 1,
        };
        assert!(one_time.validate_at(&context()).is_err());
        let mutate_alarm =
            |mut value: ClockScheduleV1,
             update: &dyn Fn(&mut ClockCivilTimeV1, &mut Vec<ClockWeekday>)| {
                let ClockScheduleKindV1::Alarm(alarm) = &mut value.schedule else {
                    unreachable!();
                };
                let ClockAlarmRecurrenceV1::Weekdays {
                    local_time,
                    weekdays,
                } = &mut alarm.recurrence
                else {
                    unreachable!();
                };
                update(local_time, weekdays);
                value
            };
        let bad_time = mutate_alarm(schedule(), &|time, _| time.hour = 24);
        assert!(bad_time.validate_at(&context()).is_err());
        let bad_zone = mutate_alarm(schedule(), &|time, _| time.time_zone = "Not/AZone".into());
        assert!(bad_zone.validate_at(&context()).is_err());
        let duplicate_day = mutate_alarm(schedule(), &|_, days| days.push(ClockWeekday::Monday));
        assert!(duplicate_day.validate_at(&context()).is_err());
        let running = ClockTimerV1 {
            original_duration_ms: 60_000,
            phase: ClockTimerPhase::Running,
            absolute_deadline_utc_ms: Some(NOW + 60_000),
            paused_remaining_ms: None,
            expired_at_utc_ms: None,
            sound: audio(),
            vibrate: false,
        };
        assert!(running.validate_at(NOW, false).is_ok());
        let paused = ClockTimerV1 {
            phase: ClockTimerPhase::Paused,
            absolute_deadline_utc_ms: None,
            paused_remaining_ms: Some(30_000),
            ..running.clone()
        };
        assert!(paused.validate_at(NOW, false).is_ok());
        let expired = ClockTimerV1 {
            phase: ClockTimerPhase::Expired,
            absolute_deadline_utc_ms: Some(NOW - 1),
            paused_remaining_ms: None,
            expired_at_utc_ms: Some(NOW - 1),
            ..running
        };
        assert!(expired.validate_at(NOW, false).is_ok());

        let timer_schedule = |id: &str, timer: ClockTimerV1| ClockScheduleV1 {
            schedule_id: id.into(),
            origin_node_id: "seat-1".into(),
            revision: 1,
            label: id.into(),
            selected_target_ids: vec!["seat-1".into()],
            schedule: ClockScheduleKindV1::Timer(timer),
        };
        let snapshot = ClockSnapshotV1 {
            schema_version: 1,
            node_id: "seat-1".into(),
            revision: 1,
            produced_at_utc_ms: NOW,
            schedules: vec![
                timer_schedule("timer-running", paused.clone()),
                timer_schedule("timer-expired", expired),
            ],
            occurrences: Vec::new(),
            stopwatches: Vec::new(),
            settings: ClockSettingsV1::defaults("America/New_York".into()),
        };
        assert!(snapshot.validate_at(&context()).is_ok());
    }

    #[test]
    fn stale_future_revision_identity_and_cap_properties_reject() {
        for offset in [-MAX_CLOCK_COMMAND_AGE_MS - 1, MAX_CLOCK_FUTURE_SKEW_MS + 1] {
            let mut value = signed_command();
            value.issued_at_utc_ms = NOW + offset;
            value.expires_at_utc_ms = value.issued_at_utc_ms + 1_000;
            assert!(value.validate_at(&context()).is_err());
        }
        for bad in [
            "",
            "has space",
            "slash/id",
            &"x".repeat(MAX_CLOCK_ID_BYTES + 1),
        ] {
            assert!(clock_command_topic(bad).is_err());
        }
        let mut value = schedule();
        value.revision = 0;
        assert!(value.validate_at(&context()).is_err());
        value.revision = 1;
        value.selected_target_ids = (0..=MAX_CLOCK_TARGETS)
            .map(|i| format!("seat-{i}"))
            .collect();
        assert!(value.validate_at(&context()).is_err());
    }

    #[test]
    fn stopwatch_occurrence_audio_settings_and_topics_are_operational() {
        let stopwatch = ClockStopwatchV1 {
            stopwatch_id: "sw-1".into(),
            origin_node_id: "seat-1".into(),
            mirror_target_ids: vec!["seat-2".into()],
            revision: 1,
            phase: ClockStopwatchPhase::Running,
            started_wall_utc_ms: Some(NOW - 5_000),
            started_monotonic_ms: Some(85_000),
            accumulated_elapsed_ms: 5_000,
            laps: vec![ClockLapV1 {
                lap_id: "lap-1".into(),
                split_elapsed_ms: 5_000,
                total_elapsed_ms: 5_000,
            }],
        };
        assert!(stopwatch.validate_at(&context()).is_ok());
        let ack = ClockAcknowledgementV1 {
            acknowledgement_id: "ack-1".into(),
            global_event_id: "event-1".into(),
            actor_node_id: "seat-1".into(),
            actor_clock: 1,
            acknowledged_at_utc_ms: NOW,
            stop: true,
        };
        let occurrence = ClockOccurrenceV1 {
            occurrence_id: "occ-1".into(),
            global_event_id: "event-1".into(),
            schedule_id: "alarm-1".into(),
            revision: 1,
            due_at_utc_ms: NOW,
            phase: ClockOccurrencePhase::Stopped,
            targets: vec![ClockTargetState {
                target_node_id: "seat-1".into(),
                disposition: ClockTargetDisposition::Stopped,
                revision: 1,
                observed_at_utc_ms: NOW,
            }],
            acknowledgement: Some(ack),
        };
        assert!(occurrence.validate_at(&context()).is_ok());
        let settings = ClockSettingsV1::defaults("America/New_York".into());
        assert!(settings.validate_at(&context()).is_ok());
        let raw = ClockAudioRef::Music {
            source_id: "source-1".into(),
            remote_id: "https://bad".into(),
            content_kind: ClockMusicKind::Radio,
            fallback_tone_id: "bell".into(),
        };
        assert!(raw.validate().is_err());
        assert_eq!(
            clock_command_topic("seat-1").unwrap(),
            "action/clock/command/seat-1"
        );
        assert_eq!(clock_state_topic("seat-1").unwrap(), "state/clock/seat-1");
        assert_eq!(
            clock_notify_topic("seat-1").unwrap(),
            "event/notify/clock/seat-1"
        );
        assert_eq!(clock_reply_topic("request-1").unwrap(), "reply/request-1");
    }

    #[test]
    fn clock_audio_handoff_is_bounded_and_identity_exact() {
        let request = ClockAudioRequestV1 {
            schema_version: CLOCK_SCHEMA_VERSION,
            request_id: "audio-request-1".into(),
            occurrence_id: "occurrence-1".into(),
            global_event_id: "event-1".into(),
            occurrence_generation: 7,
            issued_at_utc_ms: NOW - 100,
            expires_at_utc_ms: NOW + 1_000,
            body: ClockAudioActionV1::Start {
                audio: ClockAudioRef::Music {
                    source_id: "music-source-1".into(),
                    remote_id: "track-1".into(),
                    content_kind: ClockMusicKind::Track,
                    fallback_tone_id: "bright-bell".into(),
                },
                alarm_volume_milli: 800,
            },
            music_auth: None,
        };
        let encoded = serde_json::to_vec(&request).unwrap();
        assert_eq!(
            ClockAudioRequestV1::from_json_at(&encoded, NOW).unwrap(),
            request
        );
        let stable_local = ClockAudioRef::Music {
            source_id: "mde-musicd:local-alarm".into(),
            remote_id: "morning-bell".into(),
            content_kind: ClockMusicKind::Track,
            fallback_tone_id: "bright-bell".into(),
        };
        let mut resolve = request.clone();
        resolve.body = ClockAudioActionV1::Resolve {
            audio: stable_local.clone(),
        };
        assert!(resolve.validate_at(NOW).is_ok());
        assert!(!serde_json::to_string(&resolve).unwrap().contains("file:"));
        let mut preview = request.clone();
        preview.body = ClockAudioActionV1::Preview {
            audio: stable_local,
            preview_volume_milli: 500,
            preview_duration_ms: 10_000,
        };
        assert!(preview.validate_at(NOW).is_ok());
        let ClockAudioActionV1::Preview {
            preview_duration_ms,
            ..
        } = &mut preview.body
        else {
            unreachable!();
        };
        *preview_duration_ms = 10_001;
        assert!(preview.validate_at(NOW).is_err());
        let unknown =
            String::from_utf8(encoded.clone())
                .unwrap()
                .replacen('{', r#"{"unexpected":true,"#, 1);
        assert!(ClockAudioRequestV1::from_json_at(unknown.as_bytes(), NOW).is_err());
        assert!(ClockAudioRequestV1::from_json_at(
            br#"{"schema_version":1,"schema_version":1}"#,
            NOW,
        )
        .is_err());
        assert!(ClockAudioRequestV1::from_json_at(
            &vec![b' '; MAX_CLOCK_AUDIO_REQUEST_BYTES + 1],
            NOW,
        )
        .is_err());

        let mut stale = request.clone();
        stale.expires_at_utc_ms = NOW - 1;
        assert!(stale.validate_at(NOW).is_err());
        let mut no_generation = request.clone();
        no_generation.occurrence_generation = 0;
        assert!(no_generation.validate_at(NOW).is_err());
        let mut raw_url = request;
        let ClockAudioActionV1::Start { audio, .. } = &mut raw_url.body else {
            unreachable!();
        };
        let ClockAudioRef::Music { remote_id, .. } = audio else {
            unreachable!();
        };
        *remote_id = "https://untrusted.invalid/audio".into();
        assert!(raw_url.validate_at(NOW).is_err());
        let mut raw_path = raw_url;
        let ClockAudioActionV1::Start { audio, .. } = &mut raw_path.body else {
            unreachable!();
        };
        let ClockAudioRef::Music { remote_id, .. } = audio else {
            unreachable!();
        };
        *remote_id = "/srv/private/alarm.wav".into();
        assert!(raw_path.validate_at(NOW).is_err());
        assert_eq!(
            clock_audio_status_topic("seat-1").unwrap(),
            "state/music/clock-audio/seat-1"
        );
    }
}
