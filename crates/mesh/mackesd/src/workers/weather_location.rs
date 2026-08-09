//! WL-FUNC-017 S2 — daemon-owned effective weather-location authority.
//!
//! This worker is intentionally local and provider-free. It owns admission of
//! `action/weather/set-location`, atomically persists the preference together
//! with the effective generation and action cursor, resolves Auto from a fresh
//! same-host vehicle GNSS fix before the saved verified fallback, and publishes
//! one latest-wins location snapshot. A generation change first replaces the
//! current/forecast/map projections with explicit empty reset records so data
//! from the previous point cannot survive under the new authority state.

#![cfg(feature = "async-services")]

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mackes_mesh_types::location::{
    weather_location_state_topic, EffectiveLocationProvenance, EffectiveLocationSnapshot,
    EffectiveLocationState, EffectiveWeatherLocation, LocationUnavailableReason,
    SetWeatherLocationRequest, VerifiedPlace, WeatherCoverage, WeatherLocationMode,
    WeatherLocationPreference, MAX_LIVE_GNSS_AGE_MS, WEATHER_LOCATION_SCHEMA_VERSION,
    WEATHER_SET_LOCATION_TOPIC,
};
use mackes_mesh_types::nws_alert::GeoPoint;
use mackes_mesh_types::vehicle::{
    VehicleStateV2, VEHICLE_STATE_PREFIX, VEHICLE_STATE_V2_SCHEMA_VERSION,
};
use mackes_mesh_types::weather::{
    weather_current_state_topic, weather_forecast_state_topic, weather_map_state_topic,
    CurrentWeatherSnapshot, WeatherAttribution, WeatherAvailability, WeatherForecastSnapshot,
    WeatherProvider, WeatherUnavailableReason, WEATHER_CONTRACT_SCHEMA_VERSION,
};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use serde::{Deserialize, Serialize};

use super::{ShutdownToken, Worker};

const POLL: Duration = Duration::from_secs(2);
const HEARTBEAT_MS: i64 = 5 * 60 * 1_000;
const MAX_ACTION_AGE_MS: i64 = 10 * 60 * 1_000;
const LIVE_LOCATION_MOVEMENT_METRES: f64 = 1_000.0;
const DEFAULT_STATE_PATH: &str = "/var/lib/mackesd/weather-location.json";
const STATE_PATH_ENV: &str = "MDE_WEATHER_LOCATION_STATE_PATH";
const PERSISTED_SCHEMA_VERSION: u16 = 1;
const MAX_PERSISTED_BYTES: usize = 256 * 1024;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

trait Clock: Send + Sync {
    fn now_ms(&self) -> i64;
}

struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(1, |duration| {
                i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
            })
    }
}

#[derive(Debug, Clone, PartialEq)]
struct LiveLocationFix {
    point: GeoPoint,
    source_id: String,
    observed_at_ms: i64,
    time_zone: String,
}

trait FixSource: Send + Sync {
    fn latest(&self, bus_root: &Path, host: &str, now_ms: i64) -> Option<LiveLocationFix>;
}

struct VehicleBusFixSource;

impl FixSource for VehicleBusFixSource {
    fn latest(&self, bus_root: &Path, host: &str, now_ms: i64) -> Option<LiveLocationFix> {
        let persist = Persist::open(bus_root.to_path_buf()).ok()?;
        let prefix = format!("{VEHICLE_STATE_PREFIX}{host}/");
        let time_zone = local_time_zone();
        let mut newest: Option<LiveLocationFix> = None;
        for topic in persist.list_topics().ok()? {
            let Some(source_id) = topic.strip_prefix(&prefix) else {
                continue;
            };
            if source_id.is_empty() || source_id.contains('/') {
                continue;
            }
            let Ok(messages) = persist.list_since(&topic, None) else {
                continue;
            };
            let Some(message) = messages.last() else {
                continue;
            };
            let Some(body) = message.body.as_deref() else {
                continue;
            };
            if mackes_mesh_types::workloads::reject_duplicate_json_keys(body).is_err() {
                continue;
            }
            let Ok(snapshot) = serde_json::from_str::<VehicleStateV2>(body) else {
                continue;
            };
            if !valid_same_host_vehicle_fix(&snapshot, host, source_id, now_ms) {
                continue;
            }
            let point = GeoPoint {
                latitude: snapshot.gps.latitude,
                longitude: snapshot.gps.longitude,
            };
            if !point_has_nws_coverage(&point) {
                continue;
            }
            let candidate = LiveLocationFix {
                point,
                source_id: snapshot.mg90.id,
                observed_at_ms: snapshot.observed_at_ms,
                time_zone: time_zone.clone(),
            };
            if newest
                .as_ref()
                .is_none_or(|current| candidate.observed_at_ms > current.observed_at_ms)
            {
                newest = Some(candidate);
            }
        }
        newest
    }
}

fn valid_same_host_vehicle_fix(
    snapshot: &VehicleStateV2,
    host: &str,
    topic_source_id: &str,
    now_ms: i64,
) -> bool {
    snapshot.schema_version == VEHICLE_STATE_V2_SCHEMA_VERSION
        && snapshot.management_node_id == host
        && snapshot.mg90.id == topic_source_id
        && !snapshot.mg90.id.is_empty()
        && snapshot.online
        && snapshot.gps.has_fix()
        && snapshot.gps.latitude.is_finite()
        && (-90.0..=90.0).contains(&snapshot.gps.latitude)
        && snapshot.gps.longitude.is_finite()
        && (-180.0..=180.0).contains(&snapshot.gps.longitude)
        && snapshot.observed_at_ms > 0
        && snapshot.observed_at_ms <= now_ms.saturating_add(5 * 60 * 1_000)
        && now_ms.saturating_sub(snapshot.observed_at_ms) <= MAX_LIVE_GNSS_AGE_MS
}

fn point_has_nws_coverage(point: &GeoPoint) -> bool {
    let (lat, lon) = (point.latitude, point.longitude);
    ((24.0..=50.0).contains(&lat) && (-125.0..=-66.0).contains(&lon))
        || ((51.0..=72.0).contains(&lat) && (-180.0..=-129.0).contains(&lon))
        || ((18.0..=23.0).contains(&lat) && (-161.0..=-154.0).contains(&lon))
        || ((17.0..=19.0).contains(&lat) && (-68.0..=-64.0).contains(&lon))
        || ((13.0..=21.0).contains(&lat) && (144.0..=146.0).contains(&lon))
        || ((-15.0..=-10.0).contains(&lat) && (-171.0..=-168.0).contains(&lon))
}

fn local_time_zone() -> String {
    if let Ok(value) = std::env::var("TZ") {
        if valid_time_zone_name(&value) {
            return value;
        }
    }
    if let Ok(target) = fs::read_link("/etc/localtime") {
        let text = target.to_string_lossy();
        if let Some((_, value)) = text.rsplit_once("/zoneinfo/") {
            if valid_time_zone_name(value) {
                return value.to_string();
            }
        }
    }
    "Etc/UTC".to_string()
}

fn valid_time_zone_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.contains('/')
        && !value.starts_with('/')
        && !value.ends_with('/')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'_' | b'-' | b'+'))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistedAuthority {
    schema_version: u16,
    preference: WeatherLocationPreference,
    effective: EffectiveLocationSnapshot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    action_cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_request_id: Option<String>,
}

impl PersistedAuthority {
    fn initial(host: &str, now_ms: i64) -> Self {
        let preference = WeatherLocationPreference {
            schema_version: WEATHER_LOCATION_SCHEMA_VERSION,
            generation: 1,
            mode: WeatherLocationMode::Auto,
            manual_place: None,
            saved_auto_place: None,
            updated_at_ms: now_ms,
        };
        let effective = EffectiveLocationSnapshot {
            schema_version: WEATHER_LOCATION_SCHEMA_VERSION,
            host: host.to_string(),
            generation: 1,
            mode: WeatherLocationMode::Auto,
            produced_at_ms: now_ms,
            state: EffectiveLocationState::Unavailable {
                reason: LocationUnavailableReason::NoVerifiedFallback,
            },
        };
        Self {
            schema_version: PERSISTED_SCHEMA_VERSION,
            preference,
            effective,
            action_cursor: None,
            last_request_id: None,
        }
    }

    fn validate(&self, host: &str, now_ms: i64) -> io::Result<()> {
        if self.schema_version != PERSISTED_SCHEMA_VERSION
            || self.preference.generation != self.effective.generation
            || self.preference.mode != self.effective.mode
            || self.effective.host != host
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "weather-location authority record is inconsistent",
            ));
        }
        self.preference
            .validate_at(now_ms)
            .map_err(io_invalid_data)?;
        self.effective
            .validate_at(self.effective.produced_at_ms.max(1))
            .map_err(io_invalid_data)
    }
}

fn io_invalid_data(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

fn load_record(path: &Path, host: &str, now_ms: i64) -> io::Result<Option<PersistedAuthority>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "weather-location state must be a regular file",
        ));
    }
    if metadata.len() > MAX_PERSISTED_BYTES as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "weather-location state exceeds its byte limit",
        ));
    }
    let mut body = Vec::with_capacity(metadata.len() as usize);
    let file: File = rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::NONBLOCK
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )?
    .into();
    file.take(MAX_PERSISTED_BYTES as u64 + 1)
        .read_to_end(&mut body)?;
    if body.len() > MAX_PERSISTED_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "weather-location state exceeds its byte limit",
        ));
    }
    let text = std::str::from_utf8(&body).map_err(io_invalid_data)?;
    mackes_mesh_types::workloads::reject_duplicate_json_keys(text).map_err(io_invalid_data)?;
    let record: PersistedAuthority = serde_json::from_str(text).map_err(io_invalid_data)?;
    record.validate(host, now_ms)?;
    Ok(Some(record))
}

fn store_record(path: &Path, record: &PersistedAuthority) -> io::Result<()> {
    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "weather-location state path must not be a symlink",
        ));
    }
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "state path has no parent"))?;
    fs::create_dir_all(parent)?;
    let parent_metadata = fs::symlink_metadata(parent)?;
    if parent_metadata.file_type().is_symlink() || !parent_metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "weather-location state parent must be a directory",
        ));
    }
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    let body = serde_json::to_vec(record).map_err(io_invalid_data)?;
    if body.len() > MAX_PERSISTED_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "weather-location state exceeds its byte limit",
        ));
    }
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(
        ".weather-location-{}-{sequence}.tmp",
        std::process::id()
    ));
    let write_result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(&body)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        File::open(parent)?.sync_all()
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write_result
}

#[derive(Debug, Clone, Serialize)]
struct WeatherMapReset<'a> {
    schema_version: u16,
    host: &'a str,
    location_generation: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    location_point: Option<&'a GeoPoint>,
    cleared_at_ms: i64,
    state: &'static str,
}

/// Single per-node effective weather-location authority.
pub struct WeatherLocationWorker {
    host: String,
    state_path: PathBuf,
    bus_root: Option<PathBuf>,
    poll: Duration,
    clock: Arc<dyn Clock>,
    fix_source: Arc<dyn FixSource>,
    authority: Option<PersistedAuthority>,
    last_published_generation: Option<u64>,
    last_published_at_ms: i64,
}

impl WeatherLocationWorker {
    /// Construct the production per-node authority using the shared Bus, the
    /// root-owned durable state path, the system clock, and vehicle V2 fixes.
    #[must_use]
    pub fn new(host: String) -> Self {
        let state_path = std::env::var_os(STATE_PATH_ENV)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_STATE_PATH));
        Self {
            host,
            state_path,
            bus_root: crate::bus_publish::default_bus_root(),
            poll: POLL,
            clock: Arc::new(SystemClock),
            fix_source: Arc::new(VehicleBusFixSource),
            authority: None,
            last_published_generation: None,
            last_published_at_ms: 0,
        }
    }

    fn ensure_loaded(&mut self) -> io::Result<()> {
        if self.authority.is_some() {
            return Ok(());
        }
        let now_ms = self.clock.now_ms();
        let authority = match load_record(&self.state_path, &self.host, now_ms)? {
            Some(authority) => authority,
            None => {
                let authority = PersistedAuthority::initial(&self.host, now_ms);
                store_record(&self.state_path, &authority)?;
                authority
            }
        };
        self.authority = Some(authority);
        Ok(())
    }

    fn resolve(
        &self,
        preference: &WeatherLocationPreference,
        generation: u64,
        now_ms: i64,
        live_fix: Option<&LiveLocationFix>,
    ) -> EffectiveLocationSnapshot {
        let state = match preference.mode {
            WeatherLocationMode::Manual => preference.manual_place.as_ref().map_or(
                EffectiveLocationState::Unavailable {
                    reason: LocationUnavailableReason::PreferenceInvalid,
                },
                |place| EffectiveLocationState::Available {
                    location: location_from_place(place, true),
                },
            ),
            WeatherLocationMode::Auto => {
                if let Some(fix) = live_fix {
                    EffectiveLocationState::Available {
                        location: EffectiveWeatherLocation {
                            label: "Current location".to_string(),
                            point: fix.point.clone(),
                            time_zone: fix.time_zone.clone(),
                            coverage: WeatherCoverage::NwsUnitedStates,
                            provenance: EffectiveLocationProvenance::LiveGnss {
                                source_host: self.host.clone(),
                                source_id: fix.source_id.clone(),
                            },
                            source_observed_at_ms: Some(fix.observed_at_ms),
                        },
                    }
                } else if let Some(place) = preference.saved_auto_place.as_ref() {
                    EffectiveLocationState::Available {
                        location: location_from_place(place, false),
                    }
                } else {
                    EffectiveLocationState::Unavailable {
                        reason: LocationUnavailableReason::NoVerifiedFallback,
                    }
                }
            }
        };
        EffectiveLocationSnapshot {
            schema_version: WEATHER_LOCATION_SCHEMA_VERSION,
            host: self.host.clone(),
            generation,
            mode: preference.mode,
            produced_at_ms: now_ms,
            state,
        }
    }

    fn process_actions(
        &mut self,
        persist: &Persist,
        live_fix: Option<&LiveLocationFix>,
        now_ms: i64,
    ) -> io::Result<bool> {
        let cursor = self
            .authority
            .as_ref()
            .and_then(|authority| authority.action_cursor.as_deref());
        let messages = persist
            .list_since(WEATHER_SET_LOCATION_TOPIC, cursor)
            .map_err(io_other)?;
        let mut generation_changed = false;
        for message in messages {
            let body = message.body.as_deref().unwrap_or("");
            let parsed = SetWeatherLocationRequest::from_json_at(body.as_bytes(), now_ms);
            let mut next = self.authority.as_ref().expect("authority loaded").clone();
            next.action_cursor = Some(message.ulid.clone());
            let request = match parsed {
                Ok(request)
                    if now_ms.saturating_sub(request.issued_at_ms) <= MAX_ACTION_AGE_MS
                        && request.expected_generation == next.preference.generation
                        && next.last_request_id.as_deref() != Some(request.request_id.as_str()) =>
                {
                    request
                }
                Ok(request) => {
                    tracing::warn!(
                        target: "mackesd::weather_location",
                        request_id = %request.request_id,
                        "stale, replayed, or generation-mismatched location action refused"
                    );
                    store_record(&self.state_path, &next)?;
                    self.authority = Some(next);
                    continue;
                }
                Err(error) => {
                    tracing::warn!(
                        target: "mackesd::weather_location",
                        error = %error,
                        "invalid location action refused"
                    );
                    store_record(&self.state_path, &next)?;
                    self.authority = Some(next);
                    continue;
                }
            };
            let generation = next.preference.generation.saturating_add(1);
            if generation == next.preference.generation {
                return Err(io::Error::other("weather location generation exhausted"));
            }
            let saved_auto_place = match request.mode {
                WeatherLocationMode::Manual => request.manual_place.clone(),
                WeatherLocationMode::Auto => next.preference.saved_auto_place.clone(),
            };
            next.preference = WeatherLocationPreference {
                schema_version: WEATHER_LOCATION_SCHEMA_VERSION,
                generation,
                mode: request.mode,
                manual_place: request.manual_place,
                saved_auto_place,
                updated_at_ms: now_ms,
            };
            next.effective = self.resolve(&next.preference, generation, now_ms, live_fix);
            next.last_request_id = Some(request.request_id);
            next.validate(&self.host, now_ms)?;
            store_record(&self.state_path, &next)?;
            self.authority = Some(next);
            generation_changed = true;
        }
        Ok(generation_changed)
    }

    fn reconcile_fix(
        &mut self,
        live_fix: Option<&LiveLocationFix>,
        now_ms: i64,
    ) -> io::Result<bool> {
        let current = self.authority.as_ref().expect("authority loaded").clone();
        let mut desired = self.resolve(
            &current.preference,
            current.preference.generation,
            now_ms,
            live_fix,
        );
        if retain_current_live_pin(&current.effective, &desired, now_ms) {
            desired = current.effective.clone();
        }
        let location_changed = location_identity_changed(&current.effective, &desired);
        let state_changed = current.effective != desired;
        if !location_changed && !state_changed {
            return Ok(false);
        }
        let mut next = current;
        if location_changed {
            let generation = next.preference.generation.saturating_add(1);
            if generation == next.preference.generation {
                return Err(io::Error::other("weather location generation exhausted"));
            }
            next.preference.generation = generation;
            next.preference.updated_at_ms = now_ms;
            desired.generation = generation;
        }
        next.effective = desired;
        next.validate(&self.host, now_ms)?;
        store_record(&self.state_path, &next)?;
        self.authority = Some(next);
        Ok(location_changed)
    }

    fn publish_json<T: Serialize>(persist: &Persist, topic: &str, value: &T) -> io::Result<()> {
        let body = serde_json::to_string(value).map_err(io_other)?;
        persist
            .write(topic, Priority::Default, None, Some(&body))
            .map_err(io_other)?;
        Ok(())
    }

    fn clear_projections(&self, persist: &Persist, now_ms: i64) -> io::Result<()> {
        let authority = self.authority.as_ref().expect("authority loaded");
        let point = effective_point(&authority.effective);
        let time_zone = effective_time_zone(&authority.effective).unwrap_or("Etc/UTC");
        let attribution = WeatherAttribution {
            provider: WeatherProvider::NationalWeatherService,
            source_id: "nws".to_string(),
            label: "National Weather Service".to_string(),
        };
        let current = CurrentWeatherSnapshot {
            schema_version: WEATHER_CONTRACT_SCHEMA_VERSION,
            host: self.host.clone(),
            location_generation: authority.effective.generation,
            location_point: point.cloned(),
            producer_at_ms: now_ms,
            fetched_at_ms: now_ms,
            availability: WeatherAvailability::Unavailable {
                reason: WeatherUnavailableReason::ObservationUnavailable,
            },
            conditions: None,
            gaps: vec!["location changed; observation refresh pending".to_string()],
            attributions: vec![attribution.clone()],
        };
        let forecast = WeatherForecastSnapshot {
            schema_version: WEATHER_CONTRACT_SCHEMA_VERSION,
            host: self.host.clone(),
            location_generation: authority.effective.generation,
            location_point: point.cloned(),
            time_zone: time_zone.to_string(),
            producer_at_ms: now_ms,
            fetched_at_ms: now_ms,
            availability: WeatherAvailability::Unavailable {
                reason: WeatherUnavailableReason::ForecastUnavailable,
            },
            hourly: vec![],
            daily: vec![],
            alert_references: vec![],
            gaps: vec!["location changed; forecast refresh pending".to_string()],
            attributions: vec![attribution],
        };
        current.validate_at(now_ms).map_err(io_invalid_data)?;
        forecast.validate_at(now_ms).map_err(io_invalid_data)?;
        let map_reset = WeatherMapReset {
            schema_version: WEATHER_CONTRACT_SCHEMA_VERSION,
            host: &self.host,
            location_generation: authority.effective.generation,
            location_point: point,
            cleared_at_ms: now_ms,
            state: "location_changed",
        };
        Self::publish_json(persist, &weather_current_state_topic(&self.host), &current)?;
        Self::publish_json(
            persist,
            &weather_forecast_state_topic(&self.host),
            &forecast,
        )?;
        Self::publish_json(persist, &weather_map_state_topic(&self.host), &map_reset)
    }

    fn publish_location(&mut self, persist: &Persist, now_ms: i64) -> io::Result<()> {
        let effective = &self.authority.as_ref().expect("authority loaded").effective;
        Self::publish_json(
            persist,
            &weather_location_state_topic(&self.host),
            effective,
        )?;
        self.last_published_generation = Some(effective.generation);
        self.last_published_at_ms = now_ms;
        Ok(())
    }

    fn tick_once(&mut self) -> io::Result<()> {
        self.ensure_loaded()?;
        let now_ms = self.clock.now_ms();
        let live_fix = self
            .bus_root
            .as_deref()
            .and_then(|root| self.fix_source.latest(root, &self.host, now_ms));
        let Some(bus_root) = self.bus_root.clone() else {
            let _ = self.reconcile_fix(live_fix.as_ref(), now_ms)?;
            return Ok(());
        };
        let persist = Persist::open(bus_root).map_err(io_other)?;
        let action_changed = self.process_actions(&persist, live_fix.as_ref(), now_ms)?;
        let fix_changed = self.reconcile_fix(live_fix.as_ref(), now_ms)?;
        let generation = self
            .authority
            .as_ref()
            .expect("authority loaded")
            .effective
            .generation;
        let unpublished_generation = self.last_published_generation != Some(generation);
        if action_changed || fix_changed || unpublished_generation {
            self.clear_projections(&persist, now_ms)?;
            self.publish_location(&persist, now_ms)?;
        } else if now_ms.saturating_sub(self.last_published_at_ms) >= HEARTBEAT_MS {
            self.publish_location(&persist, now_ms)?;
        }
        Ok(())
    }
}

fn io_other(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

fn location_from_place(place: &VerifiedPlace, manual: bool) -> EffectiveWeatherLocation {
    EffectiveWeatherLocation {
        label: place.label.clone(),
        point: place.point.clone(),
        time_zone: place.time_zone.clone(),
        coverage: place.coverage,
        provenance: if manual {
            EffectiveLocationProvenance::ManualVerifiedPlace {
                place_id: place.place_id.clone(),
            }
        } else {
            EffectiveLocationProvenance::SavedVerifiedPlace {
                place_id: place.place_id.clone(),
            }
        },
        source_observed_at_ms: None,
    }
}

fn effective_location(snapshot: &EffectiveLocationSnapshot) -> Option<&EffectiveWeatherLocation> {
    match &snapshot.state {
        EffectiveLocationState::Available { location }
        | EffectiveLocationState::Stale { location, .. } => Some(location),
        EffectiveLocationState::Unavailable { .. } => None,
    }
}

fn effective_point(snapshot: &EffectiveLocationSnapshot) -> Option<&GeoPoint> {
    effective_location(snapshot).map(|location| &location.point)
}

fn effective_time_zone(snapshot: &EffectiveLocationSnapshot) -> Option<&str> {
    effective_location(snapshot).map(|location| location.time_zone.as_str())
}

fn retain_current_live_pin(
    current: &EffectiveLocationSnapshot,
    desired: &EffectiveLocationSnapshot,
    now_ms: i64,
) -> bool {
    let (Some(current), Some(desired)) = (effective_location(current), effective_location(desired))
    else {
        return false;
    };
    let (
        EffectiveLocationProvenance::LiveGnss {
            source_host: current_host,
            source_id: current_source,
        },
        EffectiveLocationProvenance::LiveGnss {
            source_host: desired_host,
            source_id: desired_source,
        },
    ) = (&current.provenance, &desired.provenance)
    else {
        return false;
    };
    let current_observed = current.source_observed_at_ms.unwrap_or(0);
    current_host == desired_host
        && current_source == desired_source
        && distance_metres(&current.point, &desired.point) < LIVE_LOCATION_MOVEMENT_METRES
        && now_ms.saturating_sub(current_observed) < MAX_LIVE_GNSS_AGE_MS / 2
}

fn location_identity_changed(
    current: &EffectiveLocationSnapshot,
    desired: &EffectiveLocationSnapshot,
) -> bool {
    match (effective_location(current), effective_location(desired)) {
        (None, None) => false,
        (Some(_), None) | (None, Some(_)) => true,
        (Some(current), Some(desired)) => {
            current.provenance != desired.provenance
                || current.time_zone != desired.time_zone
                || current.coverage != desired.coverage
                || distance_metres(&current.point, &desired.point) >= LIVE_LOCATION_MOVEMENT_METRES
        }
    }
}

fn distance_metres(left: &GeoPoint, right: &GeoPoint) -> f64 {
    let latitude_delta = (right.latitude - left.latitude).to_radians();
    let longitude_delta = (right.longitude - left.longitude).to_radians();
    let left_latitude = left.latitude.to_radians();
    let right_latitude = right.latitude.to_radians();
    let a = (latitude_delta / 2.0).sin().powi(2)
        + left_latitude.cos() * right_latitude.cos() * (longitude_delta / 2.0).sin().powi(2);
    6_371_000.0 * 2.0 * a.sqrt().atan2((1.0 - a).sqrt())
}

#[async_trait::async_trait]
impl Worker for WeatherLocationWorker {
    fn name(&self) -> &'static str {
        "weather_location"
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

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::vehicle::{
        ApprovalState, GpsFix, ManagerSet, Mg90Identity, RadioInventory, ShareState,
        SnapshotProvenance, VehicleDomainFreshness, VehicleTelem, WanStatus,
    };
    use std::sync::Mutex;
    use tempfile::TempDir;

    const NOW: i64 = 1_800_000_000_000;

    struct TestClock(AtomicU64);

    impl TestClock {
        fn new(now_ms: i64) -> Self {
            Self(AtomicU64::new(now_ms as u64))
        }

        fn set(&self, now_ms: i64) {
            self.0.store(now_ms as u64, Ordering::Relaxed);
        }
    }

    impl Clock for TestClock {
        fn now_ms(&self) -> i64 {
            self.0.load(Ordering::Relaxed) as i64
        }
    }

    #[derive(Default)]
    struct TestFixSource(Mutex<Option<LiveLocationFix>>);

    impl TestFixSource {
        fn set(&self, fix: Option<LiveLocationFix>) {
            *self.0.lock().expect("fix lock") = fix;
        }
    }

    impl FixSource for TestFixSource {
        fn latest(&self, _bus_root: &Path, _host: &str, _now_ms: i64) -> Option<LiveLocationFix> {
            self.0.lock().expect("fix lock").clone()
        }
    }

    struct Fixture {
        _temp: TempDir,
        bus: PathBuf,
        state: PathBuf,
        clock: Arc<TestClock>,
        fixes: Arc<TestFixSource>,
    }

    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().expect("tempdir");
            let bus = temp.path().join("bus");
            let state = temp.path().join("state/weather-location.json");
            fs::create_dir_all(&bus).expect("bus root");
            Self {
                _temp: temp,
                bus,
                state,
                clock: Arc::new(TestClock::new(NOW)),
                fixes: Arc::new(TestFixSource::default()),
            }
        }

        fn worker(&self) -> WeatherLocationWorker {
            WeatherLocationWorker {
                host: "workstation-1".to_string(),
                state_path: self.state.clone(),
                bus_root: Some(self.bus.clone()),
                poll: Duration::from_millis(5),
                clock: self.clock.clone(),
                fix_source: self.fixes.clone(),
                authority: None,
                last_published_generation: None,
                last_published_at_ms: 0,
            }
        }

        fn publish<T: Serialize>(&self, topic: &str, value: &T) {
            let persist = Persist::open(self.bus.clone()).expect("open bus");
            let body = serde_json::to_string(value).expect("serialize");
            persist
                .write(topic, Priority::Default, None, Some(&body))
                .expect("publish");
        }

        fn latest<T: serde::de::DeserializeOwned>(&self, topic: &str) -> T {
            let persist = Persist::open(self.bus.clone()).expect("open bus");
            let messages = persist.list_since(topic, None).expect("list topic");
            serde_json::from_str(
                messages
                    .last()
                    .and_then(|message| message.body.as_deref())
                    .expect("latest body"),
            )
            .expect("decode latest")
        }
    }

    fn place() -> VerifiedPlace {
        VerifiedPlace {
            place_id: "gazetteer:us:bos".to_string(),
            label: "Boston, Massachusetts".to_string(),
            point: GeoPoint {
                latitude: 42.3601,
                longitude: -71.0589,
            },
            time_zone: "America/New_York".to_string(),
            coverage: WeatherCoverage::NwsUnitedStates,
            verified_at_ms: NOW - 1_000,
        }
    }

    fn live_fix(latitude: f64, observed_at_ms: i64) -> LiveLocationFix {
        LiveLocationFix {
            point: GeoPoint {
                latitude,
                longitude: -71.0589,
            },
            source_id: "mg90:ABC123".to_string(),
            observed_at_ms,
            time_zone: "America/New_York".to_string(),
        }
    }

    fn request(
        request_id: &str,
        generation: u64,
        issued_at_ms: i64,
        mode: WeatherLocationMode,
    ) -> SetWeatherLocationRequest {
        SetWeatherLocationRequest {
            schema_version: WEATHER_LOCATION_SCHEMA_VERSION,
            request_id: request_id.to_string(),
            expected_generation: generation,
            issued_at_ms,
            mode,
            manual_place: (mode == WeatherLocationMode::Manual).then(place),
        }
    }

    #[test]
    fn auto_prefers_fresh_fix_then_saved_verified_fallback_and_recovers_restart() {
        let fixture = Fixture::new();
        let mut worker = fixture.worker();
        worker.tick_once().expect("initial tick");
        fixture.publish(
            WEATHER_SET_LOCATION_TOPIC,
            &request("manual-1", 1, NOW, WeatherLocationMode::Manual),
        );
        worker.tick_once().expect("manual tick");
        fixture.publish(
            WEATHER_SET_LOCATION_TOPIC,
            &request("auto-1", 2, NOW, WeatherLocationMode::Auto),
        );
        worker.tick_once().expect("auto fallback tick");
        let fallback: EffectiveLocationSnapshot =
            fixture.latest(&weather_location_state_topic("workstation-1"));
        assert!(matches!(
            fallback.state,
            EffectiveLocationState::Available {
                location: EffectiveWeatherLocation {
                    provenance: EffectiveLocationProvenance::SavedVerifiedPlace { .. },
                    ..
                }
            }
        ));

        fixture.fixes.set(Some(live_fix(42.5, NOW)));
        worker.tick_once().expect("fresh fix tick");
        let live: EffectiveLocationSnapshot =
            fixture.latest(&weather_location_state_topic("workstation-1"));
        assert_eq!(live.generation, 4);
        assert!(matches!(
            live.state,
            EffectiveLocationState::Available {
                location: EffectiveWeatherLocation {
                    provenance: EffectiveLocationProvenance::LiveGnss { .. },
                    ..
                }
            }
        ));

        fixture.fixes.set(None);
        let mut restarted = fixture.worker();
        restarted.tick_once().expect("restart tick");
        let recovered: EffectiveLocationSnapshot =
            fixture.latest(&weather_location_state_topic("workstation-1"));
        assert_eq!(recovered.generation, 5);
        assert!(matches!(
            recovered.state,
            EffectiveLocationState::Available {
                location: EffectiveWeatherLocation {
                    provenance: EffectiveLocationProvenance::SavedVerifiedPlace { .. },
                    ..
                }
            }
        ));
        let persisted = load_record(&fixture.state, "workstation-1", NOW)
            .expect("read state")
            .expect("state exists");
        assert_eq!(persisted.preference.mode, WeatherLocationMode::Auto);
        assert!(persisted.preference.saved_auto_place.is_some());
    }

    #[test]
    fn stale_replayed_and_invalid_actions_do_not_replace_last_good_preference() {
        let fixture = Fixture::new();
        let mut worker = fixture.worker();
        worker.tick_once().expect("initial tick");
        fixture.publish(
            WEATHER_SET_LOCATION_TOPIC,
            &request("manual-1", 1, NOW, WeatherLocationMode::Manual),
        );
        worker.tick_once().expect("accepted action");
        fixture.publish(
            WEATHER_SET_LOCATION_TOPIC,
            &request("manual-1", 2, NOW, WeatherLocationMode::Auto),
        );
        fixture.publish(
            WEATHER_SET_LOCATION_TOPIC,
            &request(
                "stale-1",
                2,
                NOW - MAX_ACTION_AGE_MS - 1,
                WeatherLocationMode::Auto,
            ),
        );
        let hostile = r#"{"schema_version":1,"request_id":"bad","expected_generation":2,"issued_at_ms":1800000000000,"mode":"manual","manual_place":{"place_id":"p","label":"Bad","point":{"latitude":999.0,"longitude":0.0},"time_zone":"America/New_York","coverage":"nws_united_states","verified_at_ms":1799999999000}}"#;
        let persist = Persist::open(fixture.bus.clone()).expect("open bus");
        persist
            .write(
                WEATHER_SET_LOCATION_TOPIC,
                Priority::Default,
                None,
                Some(hostile),
            )
            .expect("hostile action");
        worker.tick_once().expect("refuse actions");
        let state = load_record(&fixture.state, "workstation-1", NOW)
            .expect("read state")
            .expect("state exists");
        assert_eq!(state.preference.generation, 2);
        assert_eq!(state.preference.mode, WeatherLocationMode::Manual);
        assert_eq!(state.last_request_id.as_deref(), Some("manual-1"));
    }

    #[test]
    fn location_movement_increments_generation_and_clears_all_old_projections() {
        let fixture = Fixture::new();
        fixture.fixes.set(Some(live_fix(42.36, NOW)));
        let mut worker = fixture.worker();
        worker.tick_once().expect("first fix");
        fixture.fixes.set(Some(live_fix(43.36, NOW + 1_000)));
        fixture.clock.set(NOW + 1_000);
        worker.tick_once().expect("moved fix");
        let location: EffectiveLocationSnapshot =
            fixture.latest(&weather_location_state_topic("workstation-1"));
        let current: CurrentWeatherSnapshot =
            fixture.latest(&weather_current_state_topic("workstation-1"));
        let forecast: WeatherForecastSnapshot =
            fixture.latest(&weather_forecast_state_topic("workstation-1"));
        let map: serde_json::Value = fixture.latest(&weather_map_state_topic("workstation-1"));
        assert_eq!(location.generation, 3);
        assert_eq!(current.location_generation, location.generation);
        assert_eq!(forecast.location_generation, location.generation);
        assert_eq!(map["location_generation"], location.generation);
        assert!(matches!(
            current.availability,
            WeatherAvailability::Unavailable { .. }
        ));
        assert!(matches!(
            forecast.availability,
            WeatherAvailability::Unavailable { .. }
        ));
        assert!(current.conditions.is_none());
        assert!(forecast.hourly.is_empty() && forecast.daily.is_empty());
    }

    fn vehicle_snapshot(host: &str, source_id: &str, observed_at_ms: i64) -> VehicleStateV2 {
        VehicleStateV2 {
            schema_version: VEHICLE_STATE_V2_SCHEMA_VERSION,
            sequence: 1,
            observed_at_ms,
            published_at_ms: observed_at_ms,
            expected_interval_ms: 5_000,
            management_node_id: host.to_string(),
            mg90: Mg90Identity {
                id: source_id.to_string(),
                esn: "ABC123".to_string(),
                alias: None,
                model: "MG90".to_string(),
                firmware: "1".to_string(),
            },
            approval: ApprovalState::Unknown,
            sharing: ShareState::Unknown,
            managers: ManagerSet::default(),
            provenance: SnapshotProvenance::default(),
            online: true,
            freshness: VehicleDomainFreshness::default(),
            radios: RadioInventory::default(),
            gps: GpsFix {
                fix_type: "gps".to_string(),
                latitude: 42.3601,
                longitude: -71.0589,
                satellites: 8,
                ..GpsFix::default()
            },
            imu: None,
            wan: WanStatus::default(),
            telem: VehicleTelem::default(),
            gaps: vec![],
        }
    }

    #[test]
    fn production_fix_reader_rejects_stale_wrong_host_and_unsupported_coverage() {
        let fixture = Fixture::new();
        let source = VehicleBusFixSource;
        fixture.publish(
            "state/vehicle/other-host/other",
            &vehicle_snapshot("other-host", "other", NOW),
        );
        fixture.publish(
            "state/vehicle/workstation-1/stale",
            &vehicle_snapshot("workstation-1", "stale", NOW - MAX_LIVE_GNSS_AGE_MS - 1),
        );
        let mut outside = vehicle_snapshot("workstation-1", "outside", NOW);
        outside.gps.latitude = 0.0;
        outside.gps.longitude = 0.0;
        fixture.publish("state/vehicle/workstation-1/outside", &outside);
        assert!(source.latest(&fixture.bus, "workstation-1", NOW).is_none());

        fixture.publish(
            "state/vehicle/workstation-1/local",
            &vehicle_snapshot("workstation-1", "local", NOW),
        );
        let accepted = source
            .latest(&fixture.bus, "workstation-1", NOW)
            .expect("fresh same-host fix");
        assert_eq!(accepted.source_id, "local");
        assert_eq!(accepted.point, place().point);
    }

    #[test]
    fn atomic_state_rejects_symlink_and_failed_persistence_does_not_admit_action() {
        let fixture = Fixture::new();
        let target = fixture._temp.path().join("outside.json");
        fs::write(&target, b"outside").expect("outside file");
        let link = fixture._temp.path().join("state-link.json");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");
        let record = PersistedAuthority::initial("workstation-1", NOW);
        assert!(store_record(&link, &record).is_err());
        assert_eq!(fs::read(&target).expect("outside intact"), b"outside");

        let mut worker = fixture.worker();
        worker.tick_once().expect("initial state");
        fixture.publish(
            WEATHER_SET_LOCATION_TOPIC,
            &request("manual-1", 1, NOW, WeatherLocationMode::Manual),
        );
        fs::remove_file(&fixture.state).expect("remove state file");
        fs::create_dir(&fixture.state).expect("replace state with directory");
        assert!(worker.tick_once().is_err());
        assert_eq!(
            worker
                .authority
                .as_ref()
                .expect("in-memory authority")
                .preference
                .generation,
            1
        );
    }
}
