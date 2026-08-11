//! Queue-independent Clock alert audio authority.
//!
//! Clock supplies only a bounded occurrence identity and either a closed-set
//! bundled tone or a stable Music catalog reference. The authority owns
//! idempotence, generation refusal, Music duck/restore, and exact stop/snooze
//! acknowledgement. Provider URLs, filesystem paths, and commands never cross
//! this boundary.

use mackes_mesh_types::clock::{
    ClockAudioActionV1, ClockAudioPlaybackPhase, ClockAudioProviderStatus, ClockAudioRef,
    ClockAudioRequestV1, ClockAudioStatusV1, CLOCK_SCHEMA_VERSION, MAX_CLOCK_AUDIO_LEDGER_RECORDS,
};

const DUCK_FACTOR: f32 = 0.25;
/// Maximum time a governed Music source may spend accepted but silent before
/// the daemon replaces it with the request's bundled fallback.
pub const MUSIC_AUDIBLE_DEADLINE_MS: i64 = 3_000;
/// Preview is intentionally short-lived so a settings probe cannot become an
/// unowned alarm renderer.
pub const MAX_CLOCK_AUDIO_PREVIEW_MS: u32 = 10_000;

/// Typed, queue-independent catalog operations owned by the same authority as
/// scheduled Clock playback. The caller supplies only a stable Clock catalog
/// reference; provider locators remain behind [`ClockAudioEffects`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClockAudioCatalogOperation {
    /// Validate one reference against the daemon's current admitted catalog.
    Resolve {
        /// Stable, locator-free Clock reference.
        audio: ClockAudioRef,
    },
    /// Audition one reference on Clock's independent renderer.
    Preview {
        /// Stable, locator-free Clock reference.
        audio: ClockAudioRef,
        /// Preview gain in thousandths.
        volume_milli: u16,
        /// Bounded automatic-stop interval.
        duration_ms: u32,
    },
}

/// Locator-free result of a Clock catalog operation.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ClockAudioCatalogResult {
    /// The unchanged stable reference supplied by the caller.
    pub audio: ClockAudioRef,
    /// Whether an isolated preview renderer was started.
    pub previewing: bool,
    /// Automatic-stop interval when previewing.
    pub preview_duration_ms: Option<u32>,
}

/// Side effects retained behind the Music daemon's audio and catalog authority.
pub trait ClockAudioEffects {
    /// Acquire the seat-wide quarter-gain lease for this exact occurrence
    /// generation. The daemon's own Music and Clock renderer nodes are excluded
    /// by the seat authority.
    fn duck_seat_streams(&mut self, request: &ClockAudioRequestV1) -> Result<(), &'static str>;
    /// Restore every exact stream level retained by the current seat lease.
    fn restore_seat_streams(&mut self) -> Result<(), &'static str>;
    /// Current Music queue volume, when a queue renderer exists.
    fn music_volume(&self) -> Option<f32>;
    /// Change only the Music queue renderer's gain.
    fn set_music_volume(&mut self, volume: f32);
    /// Set the independent Clock alert renderer's gain.
    fn set_alert_volume(&mut self, volume: f32);
    /// Start one closed-set daemon-bundled tone.
    fn start_bundled(&mut self, tone_id: &str) -> Result<(), &'static str>;
    /// Resolve and start one governed Music catalog reference.
    fn start_music(&mut self, audio: &ClockAudioRef) -> Result<(), &'static str>;
    /// Prove that one stable reference currently resolves under Music policy.
    fn resolve_audio(&self, audio: &ClockAudioRef) -> Result<(), &'static str>;
    /// Start one queue-independent settings preview on the Clock renderer.
    fn preview_audio(&mut self, audio: &ClockAudioRef) -> Result<(), &'static str>;
    /// True only after the independent renderer has emitted Music frames.
    fn music_is_audible(&self) -> bool;
    /// Stop only Clock's independent alert renderer.
    fn stop_alert(&mut self);
    /// Revoke an unhealthy alert renderer without waiting for a provider read.
    fn revoke_alert(&mut self) {
        self.stop_alert();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OccurrenceKey {
    occurrence_id: String,
    global_event_id: String,
    generation: u64,
}

impl OccurrenceKey {
    fn from_request(request: &ClockAudioRequestV1) -> Self {
        Self {
            occurrence_id: request.occurrence_id.clone(),
            global_event_id: request.global_event_id.clone(),
            generation: request.occurrence_generation,
        }
    }

    fn matches(&self, request: &ClockAudioRequestV1) -> bool {
        self.occurrence_id == request.occurrence_id
            && self.global_event_id == request.global_event_id
            && self.generation == request.occurrence_generation
    }
}

#[derive(Debug, Clone)]
struct ActiveAlert {
    key: OccurrenceKey,
    request: ClockAudioRequestV1,
    phase: ClockAudioPlaybackPhase,
    provider_status: ClockAudioProviderStatus,
    fallback_tone_id: Option<String>,
    saved_music_volume: Option<f32>,
    music_audible_deadline_utc_ms: Option<i64>,
}

/// Terminal transition emitted when the alert renderer disappears.
#[derive(Debug, Clone)]
pub struct ClockAudioProviderLoss {
    /// Exact unavailable status to publish for the active occurrence.
    pub status: ClockAudioStatusV1,
    /// Exact pre-duck Music gain restored by the transition, when Music played.
    pub restored_music_volume: Option<f32>,
}

#[derive(Debug, Clone)]
struct LedgerRecord {
    request: ClockAudioRequestV1,
    status: ClockAudioStatusV1,
}

/// Bounded in-process authority driven by the mde-musicd serve loop.
#[derive(Debug, Default)]
pub struct ClockAudioAuthority {
    ledger: Vec<LedgerRecord>,
    latest: Vec<OccurrenceKey>,
    active: Option<ActiveAlert>,
    preview_deadline_utc_ms: Option<i64>,
}

impl ClockAudioAuthority {
    /// Resolve or preview one stable catalog reference without touching Music's
    /// queue, history, bookmarks, gain, or ownership. Preview refuses to
    /// replace an active alarm and is automatically revoked at its deadline.
    pub fn catalog_operation<E: ClockAudioEffects>(
        &mut self,
        operation: ClockAudioCatalogOperation,
        now_ms: i64,
        effects: &mut E,
    ) -> Result<ClockAudioCatalogResult, &'static str> {
        let (audio, preview) = match operation {
            ClockAudioCatalogOperation::Resolve { audio } => (audio, None),
            ClockAudioCatalogOperation::Preview {
                audio,
                volume_milli,
                duration_ms,
            } => {
                if self.active.is_some() {
                    return Err("clock_alarm_active");
                }
                if volume_milli == 0 || volume_milli > 1_000 {
                    return Err("invalid_preview_volume");
                }
                if duration_ms == 0 || duration_ms > MAX_CLOCK_AUDIO_PREVIEW_MS {
                    return Err("invalid_preview_duration");
                }
                (audio, Some((volume_milli, duration_ms)))
            }
        };

        audio.validate().map_err(|_| "invalid_music_reference")?;
        effects.resolve_audio(&audio)?;
        let Some((volume_milli, duration_ms)) = preview else {
            return Ok(ClockAudioCatalogResult {
                audio,
                previewing: false,
                preview_duration_ms: None,
            });
        };

        if self.preview_deadline_utc_ms.take().is_some() {
            effects.stop_alert();
        }
        effects.set_alert_volume(f32::from(volume_milli) / 1_000.0);
        effects.preview_audio(&audio)?;
        self.preview_deadline_utc_ms = Some(now_ms.saturating_add(i64::from(duration_ms)));
        Ok(ClockAudioCatalogResult {
            audio,
            previewing: true,
            preview_duration_ms: Some(duration_ms),
        })
    }

    /// Stop an elapsed preview without disturbing Music or an active alarm.
    pub fn poll_preview<E: ClockAudioEffects>(&mut self, now_ms: i64, effects: &mut E) -> bool {
        let Some(deadline) = self.preview_deadline_utc_ms else {
            return false;
        };
        if now_ms < deadline {
            return false;
        }
        self.preview_deadline_utc_ms = None;
        effects.stop_alert();
        true
    }

    /// Observe an in-flight Music start without sleeping. Once the injected
    /// deadline expires while the renderer is still silent, stop that source,
    /// restore the exact pre-duck gain, and start the closed-set fallback.
    ///
    /// A returned status supersedes the original request's ledger result so an
    /// exact replay observes the transition without repeating side effects.
    pub fn poll_music_start<E: ClockAudioEffects>(
        &mut self,
        now_ms: i64,
        effects: &mut E,
    ) -> Option<ClockAudioStatusV1> {
        let active = self.active.as_mut()?;
        let deadline = active.music_audible_deadline_utc_ms?;
        if effects.music_is_audible() {
            active.music_audible_deadline_utc_ms = None;
            return None;
        }
        if now_ms < deadline {
            return None;
        }

        let mut active = self
            .active
            .take()
            .expect("active alert was inspected above");
        effects.stop_alert();
        if let Some(volume) = active.saved_music_volume.take() {
            effects.set_music_volume(volume);
        }

        if let Err(reason) = effects.restore_seat_streams() {
            let status = self.status(
                &active.request,
                now_ms,
                ClockAudioPlaybackPhase::ProviderUnavailable,
                ClockAudioProviderStatus::Unavailable,
                active.fallback_tone_id.clone(),
                None,
                Some(reason),
            );
            if let Some(record) = self
                .ledger
                .iter_mut()
                .find(|record| record.request.request_id == active.request.request_id)
            {
                record.status = status.clone();
            }
            return Some(status);
        }

        let fallback_tone_id = active
            .fallback_tone_id
            .clone()
            .expect("Music starts always retain a bounded fallback tone");
        let (phase, fallback_tone_id) = match effects.start_bundled(&fallback_tone_id) {
            Ok(()) => (
                ClockAudioPlaybackPhase::PlayingFallback,
                Some(fallback_tone_id),
            ),
            Err(_) => (
                ClockAudioPlaybackPhase::ProviderUnavailable,
                Some(fallback_tone_id),
            ),
        };
        let status = self.status(
            &active.request,
            now_ms,
            phase,
            ClockAudioProviderStatus::Unavailable,
            fallback_tone_id.clone(),
            None,
            Some("music_audible_deadline_exceeded"),
        );
        if let Some(record) = self
            .ledger
            .iter_mut()
            .find(|record| record.request.request_id == active.request.request_id)
        {
            record.status = status.clone();
        }
        if phase == ClockAudioPlaybackPhase::PlayingFallback {
            active.phase = phase;
            active.provider_status = ClockAudioProviderStatus::Unavailable;
            active.fallback_tone_id = fallback_tone_id;
            active.music_audible_deadline_utc_ms = None;
            self.active = Some(active);
        }
        Some(status)
    }

    /// Terminally revoke an active alert after renderer/provider loss.
    /// The same generation cannot auto-restart; only a newer governed request
    /// may start another effect.
    pub fn provider_lost<E: ClockAudioEffects>(
        &mut self,
        now_ms: i64,
        effects: &mut E,
    ) -> Option<ClockAudioProviderLoss> {
        let active = self.active.take()?;
        effects.revoke_alert();
        if let Some(volume) = active.saved_music_volume {
            effects.set_music_volume(volume);
        }
        let restore_error = effects.restore_seat_streams().err();
        let status = self.status(
            &active.request,
            now_ms,
            ClockAudioPlaybackPhase::ProviderUnavailable,
            ClockAudioProviderStatus::Unavailable,
            active.fallback_tone_id,
            None,
            Some(restore_error.unwrap_or("renderer_unavailable")),
        );
        if let Some(record) = self
            .ledger
            .iter_mut()
            .find(|record| record.request.request_id == active.request.request_id)
        {
            record.status = status.clone();
        }
        Some(ClockAudioProviderLoss {
            status,
            restored_music_volume: active.saved_music_volume,
        })
    }

    /// Apply one already-authenticated request. Exact request replay returns
    /// the original status without repeating an audio or volume side effect.
    pub fn apply<E: ClockAudioEffects>(
        &mut self,
        request: ClockAudioRequestV1,
        now_ms: i64,
        effects: &mut E,
    ) -> ClockAudioStatusV1 {
        if let Some(record) = self
            .ledger
            .iter()
            .find(|record| record.request.request_id == request.request_id)
        {
            if semantically_same_request(&record.request, &request) {
                return record.status.clone();
            }
            return self.status(
                &request,
                now_ms,
                ClockAudioPlaybackPhase::RefusedStale,
                ClockAudioProviderStatus::NotApplicable,
                None,
                None,
                Some("request_id_conflict"),
            );
        }

        if self.is_stale(&request) {
            let status = self.status(
                &request,
                now_ms,
                ClockAudioPlaybackPhase::RefusedStale,
                ClockAudioProviderStatus::NotApplicable,
                None,
                acknowledgement(&request),
                Some("stale_occurrence"),
            );
            self.remember(request, status.clone());
            return status;
        }

        let status = match &request.body {
            ClockAudioActionV1::Resolve { audio } => {
                match self.catalog_operation(
                    ClockAudioCatalogOperation::Resolve {
                        audio: audio.clone(),
                    },
                    now_ms,
                    effects,
                ) {
                    Ok(_) => self.status(
                        &request,
                        now_ms,
                        ClockAudioPlaybackPhase::Resolved,
                        ClockAudioProviderStatus::Available,
                        None,
                        None,
                        None,
                    ),
                    Err(reason) => self.status(
                        &request,
                        now_ms,
                        ClockAudioPlaybackPhase::ProviderUnavailable,
                        ClockAudioProviderStatus::Unavailable,
                        None,
                        None,
                        Some(reason),
                    ),
                }
            }
            ClockAudioActionV1::Preview {
                audio,
                preview_volume_milli,
                preview_duration_ms,
            } => match self.catalog_operation(
                ClockAudioCatalogOperation::Preview {
                    audio: audio.clone(),
                    volume_milli: *preview_volume_milli,
                    duration_ms: *preview_duration_ms,
                },
                now_ms,
                effects,
            ) {
                Ok(_) => self.status(
                    &request,
                    now_ms,
                    ClockAudioPlaybackPhase::Previewing,
                    ClockAudioProviderStatus::Available,
                    None,
                    None,
                    None,
                ),
                Err(reason) => self.status(
                    &request,
                    now_ms,
                    ClockAudioPlaybackPhase::ProviderUnavailable,
                    ClockAudioProviderStatus::Unavailable,
                    None,
                    None,
                    Some(reason),
                ),
            },
            ClockAudioActionV1::Start {
                audio,
                alarm_volume_milli,
            } => self.start(&request, audio, *alarm_volume_milli, now_ms, effects),
            ClockAudioActionV1::Stop { acknowledgement_id } => self.finish(
                &request,
                acknowledgement_id,
                ClockAudioPlaybackPhase::Stopped,
                now_ms,
                effects,
            ),
            ClockAudioActionV1::Snooze {
                acknowledgement_id, ..
            } => self.finish(
                &request,
                acknowledgement_id,
                ClockAudioPlaybackPhase::Snoozed,
                now_ms,
                effects,
            ),
        };
        self.remember(request, status.clone());
        status
    }

    fn is_stale(&self, request: &ClockAudioRequestV1) -> bool {
        let Some(latest) = self
            .latest
            .iter()
            .find(|key| key.occurrence_id == request.occurrence_id)
        else {
            return false;
        };
        request.occurrence_generation < latest.generation
            || (request.occurrence_generation == latest.generation
                && request.global_event_id != latest.global_event_id)
            || (request.occurrence_generation == latest.generation
                && matches!(request.body, ClockAudioActionV1::Start { .. })
                && self
                    .active
                    .as_ref()
                    .is_none_or(|active| !active.key.matches(request)))
    }

    fn start<E: ClockAudioEffects>(
        &mut self,
        request: &ClockAudioRequestV1,
        audio: &ClockAudioRef,
        alarm_volume_milli: u16,
        now_ms: i64,
        effects: &mut E,
    ) -> ClockAudioStatusV1 {
        if let Some(active) = &self.active {
            if active.key.matches(request) {
                if active.request.body != request.body {
                    return self.status(
                        request,
                        now_ms,
                        ClockAudioPlaybackPhase::RefusedStale,
                        ClockAudioProviderStatus::NotApplicable,
                        None,
                        None,
                        Some("occurrence_payload_conflict"),
                    );
                }
                return self.status(
                    request,
                    now_ms,
                    active.phase,
                    active.provider_status,
                    active.fallback_tone_id.clone(),
                    None,
                    None,
                );
            }
        }
        if self.preview_deadline_utc_ms.take().is_some() {
            effects.stop_alert();
        }
        if let Err(reason) = self.clear_active(effects) {
            return self.status(
                request,
                now_ms,
                ClockAudioPlaybackPhase::ProviderUnavailable,
                ClockAudioProviderStatus::Unavailable,
                None,
                None,
                Some(reason),
            );
        }
        if let Err(reason) = effects.duck_seat_streams(request) {
            return self.status(
                request,
                now_ms,
                ClockAudioPlaybackPhase::ProviderUnavailable,
                ClockAudioProviderStatus::Unavailable,
                None,
                None,
                Some(reason),
            );
        }
        let saved_music_volume = effects.music_volume();
        if let Some(volume) = saved_music_volume {
            effects.set_music_volume((volume * DUCK_FACTOR).clamp(0.0, 1.0));
        }
        effects.set_alert_volume(f32::from(alarm_volume_milli) / 1_000.0);

        let (phase, provider_status, fallback_tone_id, reason_code, music_deadline) = match audio {
            ClockAudioRef::Bundled { tone_id } => match effects.start_bundled(tone_id) {
                Ok(()) => (
                    ClockAudioPlaybackPhase::PlayingBundled,
                    ClockAudioProviderStatus::NotApplicable,
                    None,
                    None,
                    None,
                ),
                Err(reason) => (
                    ClockAudioPlaybackPhase::ProviderUnavailable,
                    ClockAudioProviderStatus::Unavailable,
                    None,
                    Some(reason),
                    None,
                ),
            },
            ClockAudioRef::Music {
                fallback_tone_id, ..
            } => match effects.start_music(audio) {
                Ok(()) => (
                    ClockAudioPlaybackPhase::PlayingMusic,
                    ClockAudioProviderStatus::Available,
                    None,
                    None,
                    (!effects.music_is_audible())
                        .then_some(now_ms.saturating_add(MUSIC_AUDIBLE_DEADLINE_MS)),
                ),
                Err(provider_reason) => match effects.start_bundled(fallback_tone_id) {
                    Ok(()) => (
                        ClockAudioPlaybackPhase::PlayingFallback,
                        ClockAudioProviderStatus::Unavailable,
                        Some(fallback_tone_id.clone()),
                        Some(provider_reason),
                        None,
                    ),
                    Err(_) => (
                        ClockAudioPlaybackPhase::ProviderUnavailable,
                        ClockAudioProviderStatus::Unavailable,
                        Some(fallback_tone_id.clone()),
                        Some(provider_reason),
                        None,
                    ),
                },
            },
        };

        if matches!(
            phase,
            ClockAudioPlaybackPhase::PlayingFallback | ClockAudioPlaybackPhase::ProviderUnavailable
        ) {
            if let Some(volume) = saved_music_volume {
                effects.set_music_volume(volume);
            }
            if let Err(restore_reason) = effects.restore_seat_streams() {
                effects.stop_alert();
                return self.status(
                    request,
                    now_ms,
                    ClockAudioPlaybackPhase::ProviderUnavailable,
                    ClockAudioProviderStatus::Unavailable,
                    fallback_tone_id,
                    None,
                    Some(restore_reason),
                );
            }
        }
        if phase == ClockAudioPlaybackPhase::ProviderUnavailable {
            self.active = None;
        } else {
            let key = OccurrenceKey::from_request(request);
            self.note_latest(key.clone());
            let active_fallback_tone_id = match audio {
                ClockAudioRef::Music {
                    fallback_tone_id, ..
                } => Some(fallback_tone_id.clone()),
                ClockAudioRef::Bundled { .. } => fallback_tone_id.clone(),
            };
            self.active = Some(ActiveAlert {
                key,
                request: request.clone(),
                phase,
                provider_status,
                fallback_tone_id: active_fallback_tone_id,
                saved_music_volume: if matches!(
                    phase,
                    ClockAudioPlaybackPhase::PlayingBundled | ClockAudioPlaybackPhase::PlayingMusic
                ) {
                    saved_music_volume
                } else {
                    None
                },
                music_audible_deadline_utc_ms: music_deadline,
            });
        }
        self.status(
            request,
            now_ms,
            phase,
            provider_status,
            fallback_tone_id,
            None,
            reason_code,
        )
    }

    fn finish<E: ClockAudioEffects>(
        &mut self,
        request: &ClockAudioRequestV1,
        acknowledgement_id: &str,
        phase: ClockAudioPlaybackPhase,
        now_ms: i64,
        effects: &mut E,
    ) -> ClockAudioStatusV1 {
        let Some(active) = self.active.as_ref() else {
            return self.status(
                request,
                now_ms,
                ClockAudioPlaybackPhase::RefusedStale,
                ClockAudioProviderStatus::NotApplicable,
                None,
                Some(acknowledgement_id.to_string()),
                Some("inactive_occurrence"),
            );
        };
        if !active.key.matches(request) {
            return self.status(
                request,
                now_ms,
                ClockAudioPlaybackPhase::RefusedStale,
                ClockAudioProviderStatus::NotApplicable,
                None,
                Some(acknowledgement_id.to_string()),
                Some("stale_occurrence"),
            );
        }
        let key = active.key.clone();
        if let Err(reason) = self.clear_active(effects) {
            return self.status(
                request,
                now_ms,
                ClockAudioPlaybackPhase::ProviderUnavailable,
                ClockAudioProviderStatus::Unavailable,
                None,
                Some(acknowledgement_id.to_string()),
                Some(reason),
            );
        }
        self.note_latest(key);
        self.status(
            request,
            now_ms,
            phase,
            ClockAudioProviderStatus::NotApplicable,
            None,
            Some(acknowledgement_id.to_string()),
            None,
        )
    }

    fn clear_active<E: ClockAudioEffects>(&mut self, effects: &mut E) -> Result<(), &'static str> {
        if let Some(active) = self.active.take() {
            effects.stop_alert();
            if let Some(volume) = active.saved_music_volume {
                effects.set_music_volume(volume);
            }
        }
        effects.restore_seat_streams()
    }

    fn note_latest(&mut self, key: OccurrenceKey) {
        if let Some(current) = self
            .latest
            .iter_mut()
            .find(|current| current.occurrence_id == key.occurrence_id)
        {
            if key.generation >= current.generation {
                *current = key;
            }
            return;
        }
        if self.latest.len() == MAX_CLOCK_AUDIO_LEDGER_RECORDS {
            self.latest.remove(0);
        }
        self.latest.push(key);
    }

    fn remember(&mut self, request: ClockAudioRequestV1, status: ClockAudioStatusV1) {
        if self.ledger.len() == MAX_CLOCK_AUDIO_LEDGER_RECORDS {
            self.ledger.remove(0);
        }
        self.ledger.push(LedgerRecord { request, status });
    }

    #[allow(clippy::too_many_arguments)]
    fn status(
        &self,
        request: &ClockAudioRequestV1,
        now_ms: i64,
        phase: ClockAudioPlaybackPhase,
        provider_status: ClockAudioProviderStatus,
        fallback_tone_id: Option<String>,
        acknowledgement_id: Option<String>,
        reason_code: Option<&str>,
    ) -> ClockAudioStatusV1 {
        ClockAudioStatusV1 {
            schema_version: CLOCK_SCHEMA_VERSION,
            request_id: request.request_id.clone(),
            occurrence_id: request.occurrence_id.clone(),
            global_event_id: request.global_event_id.clone(),
            occurrence_generation: request.occurrence_generation,
            observed_at_utc_ms: now_ms,
            phase,
            provider_status,
            fallback_tone_id,
            acknowledgement_id,
            reason_code: reason_code.map(str::to_string),
        }
    }
}

fn acknowledgement(request: &ClockAudioRequestV1) -> Option<String> {
    match &request.body {
        ClockAudioActionV1::Stop { acknowledgement_id }
        | ClockAudioActionV1::Snooze {
            acknowledgement_id, ..
        } => Some(acknowledgement_id.clone()),
        ClockAudioActionV1::Resolve { .. }
        | ClockAudioActionV1::Preview { .. }
        | ClockAudioActionV1::Start { .. } => None,
    }
}

fn semantically_same_request(left: &ClockAudioRequestV1, right: &ClockAudioRequestV1) -> bool {
    let mut left = left.clone();
    let mut right = right.clone();
    left.music_auth = None;
    right.music_auth = None;
    left.issued_at_utc_ms = 0;
    right.issued_at_utc_ms = 0;
    left.expires_at_utc_ms = 0;
    right.expires_at_utc_ms = 0;
    left == right
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use mackes_mesh_types::clock::{ClockMusicKind, MAX_CLOCK_AUDIO_REQUEST_TTL_MS};
    use mackes_mesh_types::music_auth::{self, MusicAuthContext};

    const NOW: i64 = 1_800_000_000_000;

    #[derive(Default)]
    struct Effects {
        music_volume: Option<f32>,
        alert_volume: f32,
        starts: Vec<String>,
        stops: usize,
        volume_writes: Vec<f32>,
        provider_available: bool,
        output_available: bool,
        music_audible: bool,
        start_music_error: Option<&'static str>,
        queue_generation: u64,
        history_generation: u64,
        bookmark_generation: u64,
        seat_levels: Vec<f64>,
        seat_writes: Vec<Vec<f64>>,
        saved_seat_levels: Option<Vec<f64>>,
        seat_generation: Option<(String, String, u64)>,
        seat_unavailable: bool,
        seat_restore_unavailable: bool,
    }

    impl ClockAudioEffects for Effects {
        fn duck_seat_streams(&mut self, request: &ClockAudioRequestV1) -> Result<(), &'static str> {
            if self.seat_unavailable {
                return Err("seat_audio_authority_unavailable");
            }
            let generation = (
                request.occurrence_id.clone(),
                request.global_event_id.clone(),
                request.occurrence_generation,
            );
            if let Some(active) = &self.seat_generation {
                return (active == &generation)
                    .then_some(())
                    .ok_or("seat_audio_generation_conflict");
            }
            self.saved_seat_levels = Some(self.seat_levels.clone());
            self.seat_generation = Some(generation);
            self.seat_levels
                .iter_mut()
                .for_each(|level| *level *= f64::from(DUCK_FACTOR));
            self.seat_writes.push(self.seat_levels.clone());
            Ok(())
        }

        fn restore_seat_streams(&mut self) -> Result<(), &'static str> {
            if self.seat_restore_unavailable {
                return Err("seat_audio_control_failed");
            }
            if let Some(saved) = self.saved_seat_levels.take() {
                self.seat_levels = saved;
                self.seat_writes.push(self.seat_levels.clone());
            }
            self.seat_generation = None;
            Ok(())
        }

        fn music_volume(&self) -> Option<f32> {
            self.music_volume
        }

        fn set_music_volume(&mut self, volume: f32) {
            self.music_volume = Some(volume);
            self.volume_writes.push(volume);
        }

        fn set_alert_volume(&mut self, volume: f32) {
            self.alert_volume = volume;
        }

        fn start_bundled(&mut self, tone_id: &str) -> Result<(), &'static str> {
            if !self.output_available {
                return Err("audio_output_unavailable");
            }
            self.starts.push(format!("tone:{tone_id}"));
            Ok(())
        }

        fn start_music(&mut self, audio: &ClockAudioRef) -> Result<(), &'static str> {
            if !self.output_available {
                return Err("audio_output_unavailable");
            }
            if !self.provider_available {
                return Err("provider_unavailable");
            }
            if let Some(reason) = self.start_music_error {
                return Err(reason);
            }
            let ClockAudioRef::Music { remote_id, .. } = audio else {
                return Err("invalid_music_reference");
            };
            self.starts.push(format!("music:{remote_id}"));
            Ok(())
        }

        fn resolve_audio(&self, audio: &ClockAudioRef) -> Result<(), &'static str> {
            if !self.provider_available {
                return Err("provider_unavailable");
            }
            let ClockAudioRef::Music { remote_id, .. } = audio else {
                return Err("invalid_music_reference");
            };
            (!remote_id.trim().is_empty())
                .then_some(())
                .ok_or("catalog_reference_missing")
        }

        fn preview_audio(&mut self, audio: &ClockAudioRef) -> Result<(), &'static str> {
            if !self.output_available {
                return Err("audio_output_unavailable");
            }
            self.resolve_audio(audio)?;
            let ClockAudioRef::Music { remote_id, .. } = audio else {
                return Err("invalid_music_reference");
            };
            self.starts.push(format!("preview:{remote_id}"));
            Ok(())
        }

        fn music_is_audible(&self) -> bool {
            self.music_audible
        }

        fn stop_alert(&mut self) {
            self.stops += 1;
        }
    }

    fn request(request_id: &str, generation: u64, body: ClockAudioActionV1) -> ClockAudioRequestV1 {
        ClockAudioRequestV1 {
            schema_version: CLOCK_SCHEMA_VERSION,
            request_id: request_id.into(),
            occurrence_id: "occurrence-1".into(),
            global_event_id: "global-1".into(),
            occurrence_generation: generation,
            issued_at_utc_ms: NOW - 1,
            expires_at_utc_ms: NOW - 1 + MAX_CLOCK_AUDIO_REQUEST_TTL_MS,
            body,
            music_auth: None,
        }
    }

    fn music_start(request_id: &str, generation: u64) -> ClockAudioRequestV1 {
        request(
            request_id,
            generation,
            ClockAudioActionV1::Start {
                audio: ClockAudioRef::Music {
                    source_id: "source-1".into(),
                    remote_id: "track-1".into(),
                    content_kind: ClockMusicKind::Track,
                    fallback_tone_id: "bell".into(),
                },
                alarm_volume_milli: 800,
            },
        )
    }

    fn signed_request(
        request: &ClockAudioRequestV1,
        nonce: &str,
        now_ms: i64,
    ) -> ClockAudioRequestV1 {
        let key = [7_u8; 32];
        let body = serde_json::to_string(request).unwrap();
        let signed = music_auth::sign_request(
            &body,
            MusicAuthContext {
                verb: "music-clock-audio",
                node: "seat-1",
                target: "clock-audio",
            },
            &key,
            nonce,
            request.expires_at_utc_ms,
        )
        .unwrap();
        music_auth::verify_request(
            &signed,
            MusicAuthContext {
                verb: "music-clock-audio",
                node: "seat-1",
                target: "clock-audio",
            },
            &SigningKey::from_bytes(&key).verifying_key(),
        )
        .unwrap();
        ClockAudioRequestV1::from_json_at(signed.as_bytes(), now_ms).unwrap()
    }

    #[test]
    fn governed_start_replay_and_stop_preserve_music_ownership() {
        let mut authority = ClockAudioAuthority::default();
        let mut effects = Effects {
            music_volume: Some(0.8),
            seat_levels: vec![0.73, 0.4],
            provider_available: true,
            output_available: true,
            music_audible: true,
            queue_generation: 41,
            history_generation: 17,
            bookmark_generation: 23,
            ..Effects::default()
        };
        let start = signed_request(&music_start("start-1", 1), "nonce-original", NOW);
        let status = authority.apply(start.clone(), NOW, &mut effects);
        assert_eq!(status.phase, ClockAudioPlaybackPhase::PlayingMusic);
        assert_eq!(effects.starts, ["music:track-1"]);
        assert_eq!(effects.music_volume, Some(0.2));
        assert_eq!(effects.seat_levels, [0.1825, 0.1]);
        assert_eq!(effects.alert_volume, 0.8);
        assert_eq!(effects.queue_generation, 41);
        assert_eq!(effects.history_generation, 17);
        assert_eq!(effects.bookmark_generation, 23);

        let mut renewed = start;
        renewed.issued_at_utc_ms = NOW + 1_000;
        renewed.expires_at_utc_ms = NOW + 1_000 + MAX_CLOCK_AUDIO_REQUEST_TTL_MS;
        renewed.music_auth = None;
        let renewed = signed_request(&renewed, "nonce-renewed", NOW + 1_000);
        assert_eq!(authority.apply(renewed, NOW + 1_000, &mut effects), status);
        assert_eq!(effects.starts.len(), 1);
        assert_eq!(effects.volume_writes, [0.2]);

        let mut conflicting = music_start("start-1", 1);
        conflicting.issued_at_utc_ms = NOW + 2_000;
        conflicting.expires_at_utc_ms = NOW + 2_000 + MAX_CLOCK_AUDIO_REQUEST_TTL_MS;
        let ClockAudioActionV1::Start {
            alarm_volume_milli, ..
        } = &mut conflicting.body
        else {
            unreachable!("Music start fixture changed action")
        };
        *alarm_volume_milli = 700;
        let conflicting = signed_request(&conflicting, "nonce-conflict", NOW + 2_000);
        let refused = authority.apply(conflicting, NOW + 2_000, &mut effects);
        assert_eq!(refused.phase, ClockAudioPlaybackPhase::RefusedStale);
        assert_eq!(refused.reason_code.as_deref(), Some("request_id_conflict"));
        assert_eq!(effects.starts.len(), 1);
        assert_eq!(effects.volume_writes, [0.2]);

        let stopped = authority.apply(
            request(
                "stop-1",
                1,
                ClockAudioActionV1::Stop {
                    acknowledgement_id: "ack-stop-1".into(),
                },
            ),
            NOW,
            &mut effects,
        );
        assert_eq!(stopped.phase, ClockAudioPlaybackPhase::Stopped);
        assert_eq!(stopped.acknowledgement_id.as_deref(), Some("ack-stop-1"));
        assert_eq!(effects.stops, 1);
        assert_eq!(effects.music_volume, Some(0.8));
        assert_eq!(effects.seat_levels, [0.73, 0.4]);
        assert_eq!(
            effects.seat_writes,
            [vec![0.1825, 0.1], vec![0.73, 0.4]],
            "exact replay must not acquire a second seat lease"
        );
        assert_eq!(effects.queue_generation, 41);
        assert_eq!(effects.history_generation, 17);
        assert_eq!(effects.bookmark_generation, 23);
    }

    #[test]
    fn stale_stop_and_generation_replay_have_no_effect() {
        let mut authority = ClockAudioAuthority::default();
        let mut effects = Effects {
            music_volume: Some(1.0),
            provider_available: true,
            output_available: true,
            music_audible: true,
            ..Effects::default()
        };
        authority.apply(music_start("start-2", 2), NOW, &mut effects);
        let before = (
            effects.starts.len(),
            effects.stops,
            effects.volume_writes.clone(),
        );
        let stale = authority.apply(
            request(
                "stale-stop",
                1,
                ClockAudioActionV1::Stop {
                    acknowledgement_id: "ack-stale".into(),
                },
            ),
            NOW,
            &mut effects,
        );
        assert_eq!(stale.phase, ClockAudioPlaybackPhase::RefusedStale);
        assert_eq!(
            (
                effects.starts.len(),
                effects.stops,
                effects.volume_writes.clone()
            ),
            before
        );

        let snoozed = authority.apply(
            request(
                "snooze-2",
                2,
                ClockAudioActionV1::Snooze {
                    acknowledgement_id: "ack-snooze-2".into(),
                    resume_at_utc_ms: NOW + 60_000,
                },
            ),
            NOW,
            &mut effects,
        );
        assert_eq!(snoozed.phase, ClockAudioPlaybackPhase::Snoozed);
        assert_eq!(effects.music_volume, Some(1.0));
    }

    #[test]
    fn active_occurrence_cannot_acknowledge_a_conflicting_audio_payload() {
        let mut authority = ClockAudioAuthority::default();
        let mut effects = Effects {
            music_volume: Some(0.8),
            provider_available: true,
            output_available: true,
            music_audible: true,
            ..Effects::default()
        };
        assert_eq!(
            authority
                .apply(music_start("original-start", 3), NOW, &mut effects)
                .phase,
            ClockAudioPlaybackPhase::PlayingMusic
        );

        let mut substituted = music_start("substituted-start", 3);
        let ClockAudioActionV1::Start { audio, .. } = &mut substituted.body else {
            unreachable!("Music start fixture changed action")
        };
        let ClockAudioRef::Music { remote_id, .. } = audio else {
            unreachable!("Music start fixture changed reference")
        };
        *remote_id = "attacker-track".into();

        let refused = authority.apply(substituted, NOW + 1, &mut effects);
        assert_eq!(refused.phase, ClockAudioPlaybackPhase::RefusedStale);
        assert_eq!(
            refused.reason_code.as_deref(),
            Some("occurrence_payload_conflict")
        );
        assert_eq!(effects.starts, ["music:track-1"]);
        assert_eq!(effects.volume_writes, [0.2]);
        assert_eq!(effects.stops, 0);
    }

    #[test]
    fn provider_loss_uses_governed_fallback_or_reports_unavailable() {
        let mut authority = ClockAudioAuthority::default();
        let mut effects = Effects {
            music_volume: Some(0.6),
            seat_levels: vec![0.52, 1.2],
            provider_available: false,
            output_available: true,
            ..Effects::default()
        };
        let fallback = authority.apply(music_start("fallback-1", 1), NOW, &mut effects);
        assert_eq!(fallback.phase, ClockAudioPlaybackPhase::PlayingFallback);
        assert_eq!(
            fallback.provider_status,
            ClockAudioProviderStatus::Unavailable
        );
        assert_eq!(fallback.fallback_tone_id.as_deref(), Some("bell"));
        assert_eq!(effects.starts, ["tone:bell"]);
        assert_eq!(effects.seat_levels, [0.52, 1.2]);
        assert_eq!(effects.seat_writes, [vec![0.13, 0.3], vec![0.52, 1.2]]);

        let mut unavailable_authority = ClockAudioAuthority::default();
        let mut unavailable = Effects {
            music_volume: Some(0.6),
            provider_available: false,
            output_available: false,
            ..Effects::default()
        };
        let status =
            unavailable_authority.apply(music_start("unavailable-1", 1), NOW, &mut unavailable);
        assert_eq!(status.phase, ClockAudioPlaybackPhase::ProviderUnavailable);
        assert_eq!(unavailable.music_volume, Some(0.6));
        assert!(unavailable.starts.is_empty());
    }

    #[test]
    fn active_renderer_loss_restores_gain_and_terminally_refuses_replay() {
        let mut authority = ClockAudioAuthority::default();
        let mut effects = Effects {
            music_volume: Some(0.72),
            seat_levels: vec![0.36, 0.91],
            provider_available: true,
            output_available: true,
            music_audible: true,
            ..Effects::default()
        };
        let start = music_start("loss-start", 4);
        assert_eq!(
            authority.apply(start.clone(), NOW, &mut effects).phase,
            ClockAudioPlaybackPhase::PlayingMusic
        );
        assert_eq!(effects.music_volume, Some(0.18));
        assert_eq!(effects.seat_levels, [0.09, 0.2275]);

        let transition = authority
            .provider_lost(NOW + 1, &mut effects)
            .expect("active renderer loss must publish a terminal transition");
        assert_eq!(
            transition.status.phase,
            ClockAudioPlaybackPhase::ProviderUnavailable
        );
        assert_eq!(transition.status.occurrence_id, "occurrence-1");
        assert_eq!(transition.status.occurrence_generation, 4);
        assert_eq!(transition.restored_music_volume, Some(0.72));
        assert_eq!(effects.music_volume, Some(0.72));
        assert_eq!(effects.seat_levels, [0.36, 0.91]);
        assert_eq!(effects.stops, 1);

        let before = (
            effects.starts.len(),
            effects.stops,
            effects.volume_writes.clone(),
        );
        assert_eq!(
            authority.apply(start, NOW + 2, &mut effects).phase,
            ClockAudioPlaybackPhase::ProviderUnavailable
        );
        let stopped = authority.apply(
            request(
                "loss-stop",
                4,
                ClockAudioActionV1::Stop {
                    acknowledgement_id: "ack-after-loss".into(),
                },
            ),
            NOW + 2,
            &mut effects,
        );
        assert_eq!(stopped.phase, ClockAudioPlaybackPhase::RefusedStale);
        assert_eq!(
            (
                effects.starts.len(),
                effects.stops,
                effects.volume_writes.clone()
            ),
            before
        );
        assert!(authority.provider_lost(NOW + 3, &mut effects).is_none());
    }

    #[test]
    fn unavailable_seat_authority_refuses_before_any_renderer_or_music_mutation() {
        let mut authority = ClockAudioAuthority::default();
        let mut effects = Effects {
            music_volume: Some(0.67),
            seat_levels: vec![0.8],
            seat_unavailable: true,
            provider_available: true,
            output_available: true,
            music_audible: true,
            queue_generation: 9,
            history_generation: 17,
            bookmark_generation: 23,
            ..Effects::default()
        };

        let status = authority.apply(music_start("seat-unavailable", 8), NOW, &mut effects);
        assert_eq!(status.phase, ClockAudioPlaybackPhase::ProviderUnavailable);
        assert_eq!(
            status.reason_code.as_deref(),
            Some("seat_audio_authority_unavailable")
        );
        assert_eq!(effects.music_volume, Some(0.67));
        assert_eq!(effects.seat_levels, [0.8]);
        assert!(effects.starts.is_empty());
        assert!(effects.volume_writes.is_empty());
        assert_eq!(effects.queue_generation, 9);
        assert_eq!(effects.history_generation, 17);
        assert_eq!(effects.bookmark_generation, 23);
    }

    #[test]
    fn silent_music_falls_back_at_exact_deadline_without_touching_music_state() {
        let mut authority = ClockAudioAuthority::default();
        let start = music_start("deadline-start", 7);
        let mut effects = Effects {
            music_volume: Some(0.84),
            provider_available: true,
            output_available: true,
            music_audible: false,
            queue_generation: 41,
            history_generation: 17,
            bookmark_generation: 23,
            ..Effects::default()
        };

        let accepted = authority.apply(start.clone(), NOW, &mut effects);
        assert_eq!(accepted.phase, ClockAudioPlaybackPhase::PlayingMusic);
        assert_eq!(effects.music_volume, Some(0.21));
        assert_eq!(effects.starts, ["music:track-1"]);
        assert!(authority
            .poll_music_start(NOW + MUSIC_AUDIBLE_DEADLINE_MS - 1, &mut effects)
            .is_none());

        let fallback = authority
            .poll_music_start(NOW + MUSIC_AUDIBLE_DEADLINE_MS, &mut effects)
            .expect("a silent governed source must transition at 3,000 ms");
        assert_eq!(fallback.phase, ClockAudioPlaybackPhase::PlayingFallback);
        assert_eq!(
            fallback.reason_code.as_deref(),
            Some("music_audible_deadline_exceeded")
        );
        assert_eq!(fallback.fallback_tone_id.as_deref(), Some("bell"));
        assert_eq!(effects.starts, ["music:track-1", "tone:bell"]);
        assert_eq!(effects.stops, 1);
        assert_eq!(effects.music_volume, Some(0.84));
        assert_eq!(effects.volume_writes, [0.21, 0.84]);
        assert_eq!(effects.queue_generation, 41);
        assert_eq!(effects.history_generation, 17);
        assert_eq!(effects.bookmark_generation, 23);

        assert_eq!(
            authority.apply(start, NOW + MUSIC_AUDIBLE_DEADLINE_MS, &mut effects),
            fallback,
            "exact replay must observe the fallback without replaying effects"
        );
        assert_eq!(effects.starts, ["music:track-1", "tone:bell"]);
        assert!(authority
            .poll_music_start(NOW + MUSIC_AUDIBLE_DEADLINE_MS + 1, &mut effects)
            .is_none());
    }

    #[test]
    fn audible_before_deadline_cancels_fallback_and_stop_restores_exact_gain() {
        let mut authority = ClockAudioAuthority::default();
        let mut effects = Effects {
            music_volume: Some(0.73),
            provider_available: true,
            output_available: true,
            music_audible: false,
            ..Effects::default()
        };
        authority.apply(music_start("audible-start", 2), NOW, &mut effects);
        assert_eq!(effects.music_volume, Some(0.1825));

        effects.music_audible = true;
        assert!(authority
            .poll_music_start(NOW + 2_999, &mut effects)
            .is_none());
        effects.music_audible = false;
        assert!(authority
            .poll_music_start(NOW + 3_500, &mut effects)
            .is_none());

        let stopped = authority.apply(
            request(
                "audible-stop",
                2,
                ClockAudioActionV1::Stop {
                    acknowledgement_id: "ack-audible-stop".into(),
                },
            ),
            NOW + 3_500,
            &mut effects,
        );
        assert_eq!(stopped.phase, ClockAudioPlaybackPhase::Stopped);
        assert_eq!(effects.music_volume, Some(0.73));
        assert_eq!(effects.volume_writes, [0.1825, 0.73]);
    }

    #[test]
    fn typed_music_reference_failures_fall_back_immediately_and_restore_gain() {
        for reason in [
            "invalid_music_reference",
            "catalog_reference_missing",
            "unsupported_source",
            "source_unavailable",
        ] {
            let mut authority = ClockAudioAuthority::default();
            let mut effects = Effects {
                music_volume: Some(0.64),
                provider_available: true,
                output_available: true,
                start_music_error: Some(reason),
                ..Effects::default()
            };
            let status = authority.apply(music_start(reason, 1), NOW, &mut effects);
            assert_eq!(status.phase, ClockAudioPlaybackPhase::PlayingFallback);
            assert_eq!(status.reason_code.as_deref(), Some(reason));
            assert_eq!(effects.music_volume, Some(0.64));
            assert_eq!(effects.volume_writes, [0.16, 0.64]);
            assert_eq!(effects.starts, ["tone:bell"]);
        }
    }

    #[test]
    fn typed_resolve_and_preview_are_bounded_locator_free_and_state_isolated() {
        let audio = ClockAudioRef::Music {
            source_id: "mde-musicd:local-alarm".into(),
            remote_id: "morning-bell".into(),
            content_kind: ClockMusicKind::Track,
            fallback_tone_id: "bell".into(),
        };
        let mut authority = ClockAudioAuthority::default();
        let mut effects = Effects {
            music_volume: Some(0.8),
            provider_available: true,
            output_available: true,
            queue_generation: 9,
            history_generation: 11,
            bookmark_generation: 13,
            ..Effects::default()
        };

        let resolved = authority.apply(
            request(
                "resolve-local",
                1,
                ClockAudioActionV1::Resolve {
                    audio: audio.clone(),
                },
            ),
            NOW,
            &mut effects,
        );
        assert_eq!(resolved.phase, ClockAudioPlaybackPhase::Resolved);
        assert!(effects.starts.is_empty());

        let preview = authority.apply(
            request(
                "preview-local",
                1,
                ClockAudioActionV1::Preview {
                    audio: audio.clone(),
                    preview_volume_milli: 600,
                    preview_duration_ms: 2_000,
                },
            ),
            NOW + 1,
            &mut effects,
        );
        assert_eq!(preview.phase, ClockAudioPlaybackPhase::Previewing);
        assert_eq!(effects.starts, ["preview:morning-bell"]);
        assert_eq!(effects.alert_volume, 0.6);
        assert_eq!(effects.music_volume, Some(0.8));
        assert!(effects.volume_writes.is_empty());
        assert_eq!(effects.queue_generation, 9);
        assert_eq!(effects.history_generation, 11);
        assert_eq!(effects.bookmark_generation, 13);
        assert!(!authority.poll_preview(NOW + 2_000, &mut effects));
        assert!(authority.poll_preview(NOW + 2_001, &mut effects));
        assert_eq!(effects.stops, 1);

        let wire = serde_json::to_string(&ClockAudioCatalogResult {
            audio,
            previewing: true,
            preview_duration_ms: Some(2_000),
        })
        .unwrap();
        assert!(!wire.contains('/') && !wire.contains("file:") && !wire.contains("http"));
    }

    #[test]
    fn preview_refuses_unbounded_or_unresolved_audio_and_active_alarm_ownership() {
        let audio = ClockAudioRef::Music {
            source_id: "source-1".into(),
            remote_id: "track-1".into(),
            content_kind: ClockMusicKind::Track,
            fallback_tone_id: "bell".into(),
        };
        let mut authority = ClockAudioAuthority::default();
        let mut effects = Effects {
            provider_available: true,
            output_available: true,
            music_audible: true,
            ..Effects::default()
        };
        assert_eq!(
            authority.catalog_operation(
                ClockAudioCatalogOperation::Preview {
                    audio: audio.clone(),
                    volume_milli: 500,
                    duration_ms: MAX_CLOCK_AUDIO_PREVIEW_MS + 1,
                },
                NOW,
                &mut effects,
            ),
            Err("invalid_preview_duration")
        );
        effects.provider_available = false;
        assert_eq!(
            authority.catalog_operation(
                ClockAudioCatalogOperation::Resolve {
                    audio: audio.clone(),
                },
                NOW,
                &mut effects,
            ),
            Err("provider_unavailable")
        );
        let raw_locator = ClockAudioRef::Music {
            source_id: "source-1".into(),
            remote_id: "file:///etc/shadow".into(),
            content_kind: ClockMusicKind::Track,
            fallback_tone_id: "bell".into(),
        };
        assert_eq!(
            authority.catalog_operation(
                ClockAudioCatalogOperation::Resolve { audio: raw_locator },
                NOW,
                &mut effects,
            ),
            Err("invalid_music_reference")
        );
        effects.provider_available = true;
        authority.apply(music_start("owned-alarm", 2), NOW, &mut effects);
        assert_eq!(
            authority.catalog_operation(
                ClockAudioCatalogOperation::Preview {
                    audio,
                    volume_milli: 500,
                    duration_ms: 1_000,
                },
                NOW + 1,
                &mut effects,
            ),
            Err("clock_alarm_active")
        );
    }
}
