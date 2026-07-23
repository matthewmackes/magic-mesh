//! WL-FUNC-012 / OVERLAY-8 — keyless adsb.lol aircraft adapter.
//!
//! Workstations opt in with `MDE_OVERLAY_ADSB_LOL=1`. A fresh, finite local
//! MG90 fix drives the official ten-nautical-mile point query. The worker keeps
//! only direct/rebroadcast, position-qualified aircraft estimated below 3,000
//! feet AGL; MLAT, TIS-B, coarse, stale, ground, and malformed records are
//! counted or gapped explicitly. Blocking rustls HTTP and JSON normalization
//! run away from Tokio worker threads.

#![cfg(feature = "async-services")]

use std::io::{self, Read};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mackes_mesh_types::aircraft::{
    aircraft_state_topic, AircraftPositionSource, AircraftSnapshot, AircraftTrack,
};
use reqwest::blocking::Client;
use reqwest::header::{ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED};
use serde_json::{Map, Value};

use super::{ShutdownToken, Worker};

/// Explicit opt-in; unset/false is an idle no-op.
pub const ENABLED_ENV: &str = "MDE_OVERLAY_ADSB_LOL";
/// Optional operator-controlled point-endpoint base override.
pub const ENDPOINT_ENV: &str = "MDE_OVERLAY_ADSB_LOL_URL";
/// Official public point endpoint base.
pub const DEFAULT_ENDPOINT: &str = "https://api.adsb.lol/v2/point";
/// Visible-layer cadence from the locked overlay catalog.
pub const POLL: Duration = Duration::from_secs(3);
const RETRY_MAX: Duration = Duration::from_secs(30);
const NO_FIX_RETRY: Duration = Duration::from_secs(3);
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const QUERY_RADIUS_NM: f64 = 10.0;
const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
const MAX_FEED_AIRCRAFT: usize = 1_024;
const MAX_RETAINED_AIRCRAFT: usize = 256;
const MAX_GAPS: usize = 128;
const MAX_POSITION_AGE_S: f64 = 60.0;
const MAX_FEED_CLOCK_SKEW_MS: u64 = 60_000;
const MAX_LOW_ALTITUDE_AGL_FT: f64 = 3_000.0;
const MIN_PLAUSIBLE_AGL_FT: f64 = -500.0;
const MAX_GROUND_SPEED_KT: f64 = 750.0;
const MAX_COARSE_RC_M: f64 = 3_704.0;
const MIN_POSITION_NIC: f64 = 5.0;
const VEHICLE_FIX_MAX_AGE_MS: i64 = 30_000;
const VEHICLE_MAX_FUTURE_SKEW_MS: i64 = 5_000;
const USER_AGENT: &str =
    "Construct/12 mackesd adsb.lol-aircraft-overlay (+https://github.com/matthewmackes/magic-mesh)";

/// Finite vehicle position accepted for one point query.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VehicleFix {
    /// Vehicle latitude in decimal degrees.
    pub latitude: f64,
    /// Vehicle longitude in decimal degrees.
    pub longitude: f64,
    /// Vehicle GNSS altitude, converted to feet MSL.
    pub altitude_msl_ft: f32,
}

/// Conditional HTTP result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeResponse {
    /// Complete HTTP 200 JSON body.
    Modified(String),
    /// Validated HTTP 304.
    NotModified,
}

/// Injectable HTTP seam.
pub trait AircraftProbe: Send + Sync {
    /// Fetch nearby aircraft for one qualified vehicle fix.
    fn fetch(&self, fix: VehicleFix) -> io::Result<ProbeResponse>;
}

#[derive(Debug, Default)]
struct Validators {
    url: String,
    altitude_msl_ft: Option<f32>,
    etag: Option<String>,
    last_modified: Option<String>,
}

/// Production rustls probe.
pub struct AdsbLolHttpProbe {
    client: Client,
    endpoint: String,
    validators: Mutex<Validators>,
}

impl AdsbLolHttpProbe {
    fn new(endpoint: String) -> io::Result<Self> {
        let client = Client::builder()
            .timeout(HTTP_TIMEOUT)
            .user_agent(USER_AGENT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(io_other)?;
        Ok(Self {
            client,
            endpoint,
            validators: Mutex::new(Validators::default()),
        })
    }

    fn point_url(&self, fix: VehicleFix) -> io::Result<String> {
        if !fix.latitude.is_finite()
            || !fix.longitude.is_finite()
            || !fix.altitude_msl_ft.is_finite()
            || !(-90.0..=90.0).contains(&fix.latitude)
            || !(-180.0..=180.0).contains(&fix.longitude)
        {
            return Err(io::Error::other(
                "adsb.lol point fix is not finite and in range",
            ));
        }
        let mut url = reqwest::Url::parse(&self.endpoint).map_err(io_other)?;
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| io::Error::other("adsb.lol endpoint cannot hold path segments"))?;
            segments.pop_if_empty();
            segments.push(&format!("{:.4}", fix.latitude));
            segments.push(&format!("{:.4}", fix.longitude));
            segments.push("10");
        }
        Ok(url.to_string())
    }
}

impl AircraftProbe for AdsbLolHttpProbe {
    fn fetch(&self, fix: VehicleFix) -> io::Result<ProbeResponse> {
        let url = self.point_url(fix)?;
        let mut request = self.client.get(&url);
        let mut sent_validator = false;
        {
            let validators = self
                .validators
                .lock()
                .map_err(|_| io::Error::other("adsb.lol validator lock poisoned"))?;
            if validators_match_fix(&validators, &url, fix) {
                if let Some(value) = &validators.etag {
                    request = request.header(IF_NONE_MATCH, value);
                    sent_validator = true;
                }
                if let Some(value) = &validators.last_modified {
                    request = request.header(IF_MODIFIED_SINCE, value);
                    sent_validator = true;
                }
            }
        }
        let response = request.send().map_err(io_other)?;
        if response.status() == reqwest::StatusCode::NOT_MODIFIED {
            return accept_not_modified(sent_validator);
        }
        if response.status() != reqwest::StatusCode::OK {
            return Err(io::Error::other(format!(
                "adsb.lol returned unexpected HTTP {} (redirects are disabled)",
                response.status()
            )));
        }
        if response
            .content_length()
            .is_some_and(|length| length > u64::try_from(MAX_BODY_BYTES).unwrap_or(u64::MAX))
        {
            return Err(io::Error::other(format!(
                "adsb.lol response exceeds {MAX_BODY_BYTES} byte limit"
            )));
        }
        let etag = header_string(&response, ETAG);
        let last_modified = header_string(&response, LAST_MODIFIED);
        let mut response = response;
        let body = read_bounded_body(&mut response)?;
        let mut validators = self
            .validators
            .lock()
            .map_err(|_| io::Error::other("adsb.lol validator lock poisoned"))?;
        *validators = Validators {
            url,
            altitude_msl_ft: Some(fix.altitude_msl_ft),
            etag,
            last_modified,
        };
        Ok(ProbeResponse::Modified(body))
    }
}

fn validators_match_fix(validators: &Validators, url: &str, fix: VehicleFix) -> bool {
    validators.url == url
        && validators
            .altitude_msl_ft
            .is_some_and(|altitude| (altitude - fix.altitude_msl_ft).abs() <= 50.0)
}

fn accept_not_modified(sent_validator: bool) -> io::Result<ProbeResponse> {
    if sent_validator {
        Ok(ProbeResponse::NotModified)
    } else {
        Err(io::Error::other(
            "adsb.lol returned 304 although this point request sent no validator",
        ))
    }
}

fn header_string(
    response: &reqwest::blocking::Response,
    name: reqwest::header::HeaderName,
) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

fn read_bounded_body(response: &mut impl Read) -> io::Result<String> {
    let limit = u64::try_from(MAX_BODY_BYTES)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let mut bytes = Vec::with_capacity(64 * 1024);
    response.take(limit).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_BODY_BYTES {
        return Err(io::Error::other(format!(
            "adsb.lol response exceeds {MAX_BODY_BYTES} byte limit"
        )));
    }
    String::from_utf8(bytes).map_err(io_other)
}

fn io_other(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

#[derive(Debug)]
enum ParseDisposition {
    Keep(AircraftTrack),
    KeepWithGap(AircraftTrack, String),
    RelevanceFiltered,
    QualityFiltered,
    Malformed(String),
}

fn build_snapshot(
    host: &str,
    fix: VehicleFix,
    body: &str,
    fetched_at_ms: i64,
) -> io::Result<AircraftSnapshot> {
    if body.len() > MAX_BODY_BYTES {
        return Err(io::Error::other(format!(
            "adsb.lol response exceeds {MAX_BODY_BYTES} byte limit"
        )));
    }
    let root: Value = serde_json::from_str(body).map_err(io_other)?;
    let object = root
        .as_object()
        .ok_or_else(|| io::Error::other("adsb.lol root is not an object"))?;
    let aircraft = object
        .get("ac")
        .and_then(Value::as_array)
        .ok_or_else(|| io::Error::other("adsb.lol response has no aircraft array"))?;
    let mut snapshot = AircraftSnapshot::empty(
        host,
        fetched_at_ms,
        fix.latitude,
        fix.longitude,
        fix.altitude_msl_ft,
    );
    snapshot.radius_nm = QUERY_RADIUS_NM as f32;
    snapshot.feed_total = object
        .get("total")
        .and_then(value_u64)
        .and_then(|value| u32::try_from(value).ok());
    snapshot.feed_generated_at_ms =
        validated_feed_time(object.get("now"), fetched_at_ms, &mut snapshot.gaps);
    if aircraft.len() > MAX_FEED_AIRCRAFT {
        push_gap(
            &mut snapshot.gaps,
            format!(
                "feed contains {} aircraft; only the first {MAX_FEED_AIRCRAFT} are processed",
                aircraft.len()
            ),
        );
    }
    let observation_base_ms = snapshot.feed_generated_at_ms.unwrap_or(fetched_at_ms);
    for (index, raw) in aircraft.iter().take(MAX_FEED_AIRCRAFT).enumerate() {
        match parse_aircraft(raw, fix, observation_base_ms) {
            ParseDisposition::Keep(track) => retain_track(&mut snapshot, track),
            ParseDisposition::KeepWithGap(track, reason) => {
                push_gap(
                    &mut snapshot.gaps,
                    format!("aircraft {index} motion omitted: {reason}"),
                );
                retain_track(&mut snapshot, track);
            }
            ParseDisposition::RelevanceFiltered => {
                snapshot.relevance_filtered = snapshot.relevance_filtered.saturating_add(1);
            }
            ParseDisposition::QualityFiltered => {
                snapshot.quality_filtered = snapshot.quality_filtered.saturating_add(1);
            }
            ParseDisposition::Malformed(reason) => push_gap(
                &mut snapshot.gaps,
                format!("aircraft {index} omitted: {reason}"),
            ),
        }
    }
    Ok(snapshot)
}

fn retain_track(snapshot: &mut AircraftSnapshot, track: AircraftTrack) {
    if snapshot.aircraft.len() < MAX_RETAINED_AIRCRAFT {
        snapshot.aircraft.push(track);
        return;
    }
    let gap = format!("qualified aircraft capped at {MAX_RETAINED_AIRCRAFT} records");
    if !snapshot.gaps.contains(&gap) {
        push_gap(&mut snapshot.gaps, gap);
    }
}

fn validated_feed_time(raw: Option<&Value>, fetched: i64, gaps: &mut Vec<String>) -> Option<i64> {
    let Some(now) = raw.and_then(value_i64) else {
        push_gap(gaps, "feed generation time missing or invalid".to_string());
        return None;
    };
    if now.abs_diff(fetched) > MAX_FEED_CLOCK_SKEW_MS {
        push_gap(
            gaps,
            "feed generation time has implausible clock skew".to_string(),
        );
        None
    } else {
        Some(now)
    }
}

fn parse_aircraft(raw: &Value, fix: VehicleFix, observation_base_ms: i64) -> ParseDisposition {
    let Some(object) = raw.as_object() else {
        return ParseDisposition::Malformed("record is not an object".to_string());
    };
    let id = object
        .get("hex")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| valid_aircraft_id(value));
    let Some(id) = id else {
        return ParseDisposition::Malformed("missing or invalid hex id".to_string());
    };
    let Some(latitude) = object.get("lat").and_then(value_f64) else {
        return ParseDisposition::Malformed("missing or invalid latitude".to_string());
    };
    let Some(longitude) = object.get("lon").and_then(value_f64) else {
        return ParseDisposition::Malformed("missing or invalid longitude".to_string());
    };
    if !(-90.0..=90.0).contains(&latitude) || !(-180.0..=180.0).contains(&longitude) {
        return ParseDisposition::Malformed("coordinates are out of range".to_string());
    }
    let Some(seen_pos_s) = object.get("seen_pos").and_then(value_f64) else {
        return ParseDisposition::QualityFiltered;
    };
    if !(0.0..=MAX_POSITION_AGE_S).contains(&seen_pos_s) {
        return ParseDisposition::QualityFiltered;
    }
    let source = match qualified_position_source(object) {
        Some(source) => source,
        None => return ParseDisposition::QualityFiltered,
    };
    let nic = object.get("nic").and_then(value_f64);
    let rc = object.get("rc").and_then(value_f64);
    if nic.is_none() && rc.is_none()
        || nic.is_some_and(|value| !(MIN_POSITION_NIC..=11.0).contains(&value))
        || rc.is_some_and(|value| !(0.0..=MAX_COARSE_RC_M).contains(&value))
    {
        return ParseDisposition::QualityFiltered;
    }
    let distance_nm = great_circle_nm(fix.latitude, fix.longitude, latitude, longitude);
    if !distance_nm.is_finite() {
        return ParseDisposition::Malformed("distance is not finite".to_string());
    }
    if distance_nm > QUERY_RADIUS_NM {
        return ParseDisposition::RelevanceFiltered;
    }
    if object
        .get("alt_baro")
        .and_then(Value::as_str)
        .is_some_and(|value| value.eq_ignore_ascii_case("ground"))
    {
        return ParseDisposition::RelevanceFiltered;
    }
    let barometric = object.get("alt_baro");
    let altitude_msl_ft = object
        .get("alt_geom")
        .and_then(value_f64)
        .or_else(|| barometric.and_then(value_f64));
    let Some(altitude_msl_ft) = altitude_msl_ft else {
        if barometric.is_some() {
            return ParseDisposition::Malformed("invalid altitude".to_string());
        }
        return ParseDisposition::RelevanceFiltered;
    };
    if !(-2_000.0..=100_000.0).contains(&altitude_msl_ft) {
        return ParseDisposition::Malformed("altitude is implausible".to_string());
    }
    let estimated_agl_ft = altitude_msl_ft - f64::from(fix.altitude_msl_ft);
    if estimated_agl_ft < MIN_PLAUSIBLE_AGL_FT {
        return ParseDisposition::Malformed("estimated AGL is implausibly negative".to_string());
    }
    if estimated_agl_ft > MAX_LOW_ALTITUDE_AGL_FT {
        return ParseDisposition::RelevanceFiltered;
    }

    let mut normalization_gap = None;
    let ground_speed_kt = match object.get("gs").and_then(value_f64) {
        Some(value) if (0.0..=MAX_GROUND_SPEED_KT).contains(&value) => Some(value as f32),
        Some(_) => {
            normalization_gap = Some("extreme ground speed ignored".to_string());
            None
        }
        None => None,
    };
    let track_deg = match object.get("track").and_then(value_f64) {
        Some(value) if (0.0..360.0).contains(&value) => Some(value as f32),
        Some(_) => {
            normalization_gap = Some("invalid ground track ignored".to_string());
            None
        }
        None => None,
    };
    let callsign = object
        .get("flight")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 16
                && value.chars().all(|character| character.is_ascii_graphic())
        })
        .map(str::to_string);
    let observed_at_ms = observation_base_ms.saturating_sub((seen_pos_s * 1_000.0) as i64);
    let track = AircraftTrack {
        id: id.to_ascii_lowercase(),
        callsign,
        observed_at_ms,
        latitude,
        longitude,
        altitude_msl_ft: altitude_msl_ft as f32,
        estimated_agl_ft: estimated_agl_ft.max(0.0) as f32,
        ground_speed_kt,
        track_deg,
        position_source: source,
    };
    if let Some(reason) = normalization_gap {
        // The position remains useful, but omit the hostile motion component.
        ParseDisposition::KeepWithGap(track, reason)
    } else {
        ParseDisposition::Keep(track)
    }
}

fn qualified_position_source(object: &Map<String, Value>) -> Option<AircraftPositionSource> {
    if field_is_derived(object.get("mlat"), "lat")
        || field_is_derived(object.get("mlat"), "lon")
        || field_is_derived(object.get("tisb"), "lat")
        || field_is_derived(object.get("tisb"), "lon")
    {
        return None;
    }
    match object
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        value if value.starts_with("adsb_") => Some(AircraftPositionSource::Adsb),
        value if value.starts_with("uat_") => Some(AircraftPositionSource::Uat),
        value if value.starts_with("adsr_") => Some(AircraftPositionSource::Adsr),
        _ => None,
    }
}

fn field_is_derived(raw: Option<&Value>, field: &str) -> bool {
    raw.and_then(Value::as_array)
        .is_some_and(|values| values.iter().any(|value| value.as_str() == Some(field)))
}

fn valid_aircraft_id(value: &str) -> bool {
    (6..=7).contains(&value.len())
        && value.chars().enumerate().all(|(index, character)| {
            character.is_ascii_hexdigit() || (index == 0 && character == '~')
        })
}

fn value_f64(value: &Value) -> Option<f64> {
    value.as_f64().filter(|number| number.is_finite())
}

fn value_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|number| i64::try_from(number).ok()))
}

fn value_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|number| u64::try_from(number).ok()))
}

fn great_circle_nm(lat_a: f64, lon_a: f64, lat_b: f64, lon_b: f64) -> f64 {
    let lat_a = lat_a.to_radians();
    let lat_b = lat_b.to_radians();
    let delta_lat = lat_b - lat_a;
    let delta_lon = (lon_b - lon_a).to_radians();
    let haversine = (delta_lat * 0.5).sin().powi(2)
        + lat_a.cos() * lat_b.cos() * (delta_lon * 0.5).sin().powi(2);
    3_440.065 * 2.0 * haversine.clamp(0.0, 1.0).sqrt().asin()
}

fn push_gap(gaps: &mut Vec<String>, gap: String) {
    if gaps.len() < MAX_GAPS {
        gaps.push(gap);
    } else if gaps
        .last()
        .is_some_and(|last| last != "additional normalization gaps omitted")
    {
        gaps[MAX_GAPS - 1] = "additional normalization gaps omitted".to_string();
    }
}

enum PreparedResponse {
    Modified(AircraftSnapshot),
    NotModified,
}

/// Workstation-side aircraft adapter.
pub struct AircraftOverlayWorker {
    host: String,
    probe: Option<Arc<dyn AircraftProbe>>,
    bus_root: Option<PathBuf>,
    poll: Duration,
}

impl AircraftOverlayWorker {
    /// Production wiring. Disabled unless explicitly opted in.
    #[must_use]
    pub fn new(host: String) -> Self {
        let probe = if env_truthy(ENABLED_ENV) {
            let endpoint = std::env::var(ENDPOINT_ENV)
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_ENDPOINT.to_string());
            match AdsbLolHttpProbe::new(endpoint) {
                Ok(probe) => Some(Arc::new(probe) as Arc<dyn AircraftProbe>),
                Err(error) => {
                    tracing::warn!(target: "mackesd::aircraft_overlay", %error, "adsb.lol client unavailable; worker idle");
                    None
                }
            }
        } else {
            None
        };
        Self {
            host,
            probe,
            bus_root: crate::bus_publish::default_bus_root(),
            poll: POLL,
        }
    }

    /// Inject a captured-fixture probe.
    #[must_use]
    pub fn with_probe(mut self, probe: Arc<dyn AircraftProbe>) -> Self {
        self.probe = Some(probe);
        self
    }

    /// Override or disable Bus access.
    #[must_use]
    pub fn with_bus_root(mut self, root: Option<PathBuf>) -> Self {
        self.bus_root = root;
        self
    }

    /// Override cadence for tests.
    #[must_use]
    pub const fn with_poll(mut self, poll: Duration) -> Self {
        self.poll = poll;
        self
    }

    fn current_vehicle_fix(&self) -> Option<VehicleFix> {
        let root = self.bus_root.clone()?;
        let persist = mde_bus::persist::Persist::open(root).ok()?;
        let topic = mackes_mesh_types::vehicle::vehicle_state_topic(&self.host);
        let body = persist.read_latest(&topic).ok().flatten()?.body?;
        let vehicle: mackes_mesh_types::vehicle::VehicleState = serde_json::from_str(&body).ok()?;
        validated_vehicle_fix(&vehicle, &self.host, now_ms())
    }

    fn publish(&self, snapshot: &AircraftSnapshot) {
        if let Some(mut persist) = crate::bus_publish::open_bus(self.bus_root.clone()) {
            crate::bus_publish::publish_json(
                &mut persist,
                &aircraft_state_topic(&self.host),
                snapshot,
            );
        }
    }

    fn apply_result(
        &self,
        result: io::Result<PreparedResponse>,
        fix: VehicleFix,
        last_good: &mut Option<AircraftSnapshot>,
    ) -> bool {
        match result {
            Ok(PreparedResponse::Modified(snapshot)) => {
                self.publish(&snapshot);
                *last_good = Some(snapshot);
                true
            }
            Ok(PreparedResponse::NotModified) => {
                if last_good
                    .as_ref()
                    .is_some_and(|snapshot| !snapshot_matches_fix(snapshot, fix))
                {
                    self.publish_failure(
                        last_good,
                        "adsb.lol refresh failed: 304 point does not match last-good snapshot",
                    );
                    return false;
                }
                if let Some(snapshot) = last_good {
                    snapshot.fetched_at_ms = now_ms();
                    snapshot
                        .gaps
                        .retain(|gap| !gap.starts_with("adsb.lol refresh failed:"));
                    self.publish(snapshot);
                    true
                } else {
                    false
                }
            }
            Err(error) => {
                self.publish_failure(last_good, &format!("adsb.lol refresh failed: {error}"));
                false
            }
        }
    }

    fn publish_failure(&self, last_good: &mut Option<AircraftSnapshot>, gap: &str) {
        tracing::warn!(target: "mackesd::aircraft_overlay", host = %self.host, error = gap, "aircraft refresh failed; retaining last-good snapshot");
        if let Some(snapshot) = last_good {
            snapshot
                .gaps
                .retain(|existing| !existing.starts_with("adsb.lol refresh failed:"));
            push_gap(&mut snapshot.gaps, gap.to_string());
            self.publish(snapshot);
        }
    }

    async fn fetch_async(
        &self,
        probe: Arc<dyn AircraftProbe>,
        fix: VehicleFix,
        shutdown: &mut ShutdownToken,
    ) -> Option<io::Result<PreparedResponse>> {
        let host = self.host.clone();
        let task = tokio::task::spawn_blocking(move || match probe.fetch(fix)? {
            ProbeResponse::Modified(body) => build_snapshot(&host, fix, &body, now_ms())
                .map(PreparedResponse::Modified)
                .map_err(|error| io::Error::other(format!("adsb.lol payload invalid: {error}"))),
            ProbeResponse::NotModified => Ok(PreparedResponse::NotModified),
        });
        tokio::select! {
            () = shutdown.wait() => None,
            joined = task => Some(match joined {
                Ok(result) => result,
                Err(error) => Err(io::Error::other(format!("adsb.lol fetch task failed: {error}"))),
            }),
        }
    }
}

fn validated_vehicle_fix(
    vehicle: &mackes_mesh_types::vehicle::VehicleState,
    expected_host: &str,
    now: i64,
) -> Option<VehicleFix> {
    let mirror_age = now.saturating_sub(vehicle.published_at_ms).max(0);
    let future_skew = vehicle.published_at_ms.saturating_sub(now).max(0);
    let gps = &vehicle.gps;
    if vehicle.host != expected_host
        || !vehicle.online
        || !gps.has_fix()
        || !gps.latitude.is_finite()
        || !gps.longitude.is_finite()
        || !(-90.0..=90.0).contains(&gps.latitude)
        || !(-180.0..=180.0).contains(&gps.longitude)
        || !gps.altitude_m.is_finite()
        || !(-500.0..=9_000.0).contains(&gps.altitude_m)
        || !gps.age_s.is_finite()
        || gps.age_s < 0.0
        || future_skew > VEHICLE_MAX_FUTURE_SKEW_MS
        || mirror_age as f64 + f64::from(gps.age_s) * 1_000.0 > VEHICLE_FIX_MAX_AGE_MS as f64
    {
        return None;
    }
    Some(VehicleFix {
        latitude: gps.latitude,
        longitude: gps.longitude,
        altitude_msl_ft: gps.altitude_m * 3.280_84,
    })
}

fn snapshot_matches_fix(snapshot: &AircraftSnapshot, fix: VehicleFix) -> bool {
    (snapshot.query_latitude - fix.latitude).abs() <= 0.000_05
        && (snapshot.query_longitude - fix.longitude).abs() <= 0.000_05
        && (snapshot.query_altitude_msl_ft - fix.altitude_msl_ft).abs() <= 50.0
}

#[async_trait::async_trait]
impl Worker for AircraftOverlayWorker {
    fn name(&self) -> &'static str {
        "aircraft_overlay"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let Some(probe) = self.probe.clone() else {
            tracing::info!(target: "mackesd::aircraft_overlay", env = ENABLED_ENV, "aircraft overlay not configured; worker idle");
            shutdown.wait().await;
            return Ok(());
        };
        let mut last_good = None;
        let mut retry = self.poll;
        loop {
            let Some(fix) = self.current_vehicle_fix() else {
                tokio::select! {
                    () = shutdown.wait() => break,
                    () = tokio::time::sleep(NO_FIX_RETRY.min(self.poll)) => {}
                }
                continue;
            };
            let Some(result) = self.fetch_async(probe.clone(), fix, &mut shutdown).await else {
                break;
            };
            let success = self.apply_result(result, fix, &mut last_good);
            let delay = if success { self.poll } else { retry };
            retry = if success {
                self.poll
            } else {
                retry.saturating_mul(2).min(RETRY_MAX)
            };
            tokio::select! {
                () = shutdown.wait() => break,
                () = tokio::time::sleep(delay) => {}
            }
        }
        Ok(())
    }
}

fn env_truthy(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, Ordering};

    use mackes_mesh_types::vehicle::{GpsFix, VehicleState};
    use mde_bus::persist::Persist;

    use super::*;

    // Captured from the official /v2/point/40.7128/-74.0060/10 schema on
    // 2026-07-22, then reduced and adversarial cases added without changing the
    // live field shapes (`alt_baro:"ground"`, `mlat`, `tisb`, `nic`, `rc`).
    const FETCHED_AT_MS: i64 = 1_784_753_989_504;
    const CAPTURED_ADSB: &str = r#"{
      "now":1784753989504,"total":10,"ac":[
        {"hex":"aaacc3","type":"adsb_icao","flight":"N123AB  ","alt_baro":625,"alt_geom":425,"gs":157.9,"track":206.73,"lat":40.7000,"lon":-74.0100,"seen":0.1,"seen_pos":0.342,"nic":8,"rc":186,"mlat":[],"tisb":[]},
        {"hex":"abc001","type":"adsb_icao","alt_baro":"ground","lat":40.701,"lon":-74.011,"seen_pos":0.1,"nic":8,"rc":186},
        {"hex":"abc002","type":"adsb_icao","alt_geom":4500,"lat":40.702,"lon":-74.012,"seen_pos":0.1,"nic":8,"rc":186},
        {"hex":"abc003","type":"adsb_icao","alt_geom":700,"lat":40.703,"lon":-74.013,"seen_pos":0.1,"nic":8,"rc":186,"mlat":["lat","lon"]},
        {"hex":"abc004","type":"adsb_icao","alt_geom":800,"lat":40.704,"lon":-74.014,"seen_pos":0.1,"nic":8,"rc":186,"tisb":["lat"]},
        {"hex":"abc005","type":"adsb_icao","alt_geom":900,"lat":40.705,"lon":-74.015,"seen_pos":0.1,"nic":3,"rc":10000},
        {"hex":"abc006","type":"adsb_icao","alt_geom":1000,"lat":40.706,"lon":-74.016,"seen_pos":61,"nic":8,"rc":186},
        {"hex":"abc007","type":"adsb_icao","alt_geom":1100,"gs":999,"track":90,"lat":40.707,"lon":-74.017,"seen_pos":0.1,"nic":8,"rc":186},
        {"hex":"abc008","type":"adsb_icao","alt_geom":1200,"lat":"NaN","lon":-74.018,"seen_pos":0.1,"nic":8,"rc":186},
        {"hex":"abc009","type":"adsb_icao","alt_baro":"hostile","lat":40.709,"lon":-74.019,"seen_pos":0.1,"nic":8,"rc":186}
      ]
    }"#;

    fn fix() -> VehicleFix {
        VehicleFix {
            latitude: 40.7128,
            longitude: -74.0060,
            altitude_msl_ft: 0.0,
        }
    }

    fn worker() -> AircraftOverlayWorker {
        AircraftOverlayWorker::new("rig-1".to_string()).with_bus_root(None)
    }

    #[test]
    fn captured_schema_filters_ground_mlat_tisb_coarse_stale_and_hostile_values() {
        let snapshot =
            build_snapshot("rig-1", fix(), CAPTURED_ADSB, FETCHED_AT_MS).expect("captured parse");
        assert_eq!(snapshot.feed_total, Some(10));
        assert_eq!(snapshot.aircraft.len(), 2);
        assert_eq!(snapshot.relevance_filtered, 2, "ground plus high altitude");
        assert_eq!(snapshot.quality_filtered, 4, "MLAT/TIS-B/coarse/stale");
        assert_eq!(snapshot.aircraft[0].callsign.as_deref(), Some("N123AB"));
        assert_eq!(
            snapshot.aircraft[0].position_source,
            AircraftPositionSource::Adsb
        );
        assert_eq!(snapshot.aircraft[1].ground_speed_kt, None);
        assert!(snapshot
            .gaps
            .iter()
            .any(|gap| gap.contains("extreme ground speed")));
        assert!(snapshot
            .gaps
            .iter()
            .any(|gap| gap.contains("invalid latitude")));
        assert!(snapshot
            .gaps
            .iter()
            .any(|gap| gap.contains("invalid altitude")));
        assert_eq!(snapshot.license_tier, "open-data-attribution");
    }

    #[test]
    fn body_cardinality_retention_and_gap_limits_are_bounded() {
        let record = serde_json::json!({
            "hex":"aaacc3", "type":"adsb_icao", "alt_geom":425,
            "lat":40.7000, "lon":-74.0100, "seen_pos":0.1, "nic":8, "rc":186
        });
        let body = serde_json::json!({
            "now": FETCHED_AT_MS,
            "total": MAX_FEED_AIRCRAFT + 1,
            "ac": vec![record; MAX_FEED_AIRCRAFT + 1]
        })
        .to_string();
        let snapshot = build_snapshot("rig-1", fix(), &body, FETCHED_AT_MS).expect("bounded");
        assert_eq!(snapshot.aircraft.len(), MAX_RETAINED_AIRCRAFT);
        assert!(snapshot.gaps.iter().any(|gap| gap.contains("first 1024")));
        assert!(snapshot
            .gaps
            .iter()
            .any(|gap| gap.contains("capped at 256")));
        assert!(snapshot.gaps.len() <= MAX_GAPS);

        let oversized = " ".repeat(MAX_BODY_BYTES + 1);
        assert!(build_snapshot("rig-1", fix(), &oversized, FETCHED_AT_MS).is_err());
    }

    #[test]
    fn point_url_is_finite_exact_radius_and_path_scoped() {
        let probe = AdsbLolHttpProbe::new(DEFAULT_ENDPOINT.to_string()).expect("client");
        assert_eq!(
            probe.point_url(fix()).expect("url"),
            "https://api.adsb.lol/v2/point/40.7128/-74.0060/10"
        );
        let mut hostile = fix();
        hostile.latitude = f64::NAN;
        assert!(probe.point_url(hostile).is_err());
    }

    #[test]
    fn vehicle_fix_requires_same_host_online_finite_fresh_mg90_position() {
        let mut vehicle = VehicleState::offline("rig-1");
        vehicle.online = true;
        vehicle.published_at_ms = 100_000;
        vehicle.gps = GpsFix {
            fix_type: "gps".to_string(),
            latitude: 40.7128,
            longitude: -74.006,
            altitude_m: 12.0,
            satellites: 8,
            age_s: 1.0,
            ..GpsFix::default()
        };
        assert!(validated_vehicle_fix(&vehicle, "rig-1", 110_000).is_some());
        assert!(validated_vehicle_fix(&vehicle, "other", 110_000).is_none());
        vehicle.gps.age_s = 25.0;
        assert!(validated_vehicle_fix(&vehicle, "rig-1", 110_001).is_none());
        vehicle.gps.age_s = 0.0;
        vehicle.gps.latitude = f64::NAN;
        assert!(validated_vehicle_fix(&vehicle, "rig-1", 100_000).is_none());
    }

    #[test]
    fn conditional_response_requires_a_sent_validator() {
        assert!(accept_not_modified(false).is_err());
        assert_eq!(
            accept_not_modified(true).expect("validated"),
            ProbeResponse::NotModified
        );
        let validators = Validators {
            url: "point".to_string(),
            altitude_msl_ft: Some(100.0),
            etag: Some("tag".to_string()),
            last_modified: None,
        };
        let mut moved = fix();
        moved.altitude_msl_ft = 151.0;
        assert!(!validators_match_fix(&validators, "point", moved));
        moved.altitude_msl_ft = 150.0;
        assert!(validators_match_fix(&validators, "point", moved));
    }

    #[test]
    fn http_client_refuses_redirects_before_contacting_target() {
        let target = TcpListener::bind("127.0.0.1:0").expect("target");
        target.set_nonblocking(true).expect("nonblocking");
        let target_addr = target.local_addr().expect("target addr");
        let contacted = Arc::new(AtomicBool::new(false));
        let contacted_thread = contacted.clone();
        let target_thread = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_millis(400);
            while std::time::Instant::now() < deadline {
                match target.accept() {
                    Ok(_) => {
                        contacted_thread.store(true, Ordering::Relaxed);
                        return;
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => return,
                }
            }
        });
        let redirect = TcpListener::bind("127.0.0.1:0").expect("redirect");
        let redirect_addr = redirect.local_addr().expect("redirect addr");
        let redirect_thread = std::thread::spawn(move || {
            let (mut stream, _) = redirect.accept().expect("request");
            let mut request = [0_u8; 2048];
            let _ = stream.read(&mut request);
            write!(
                stream,
                "HTTP/1.1 302 Found\r\nLocation: http://{target_addr}/escaped\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .expect("response");
        });
        let probe =
            AdsbLolHttpProbe::new(format!("http://{redirect_addr}/v2/point")).expect("client");
        let error = probe.fetch(fix()).expect_err("redirect rejected");
        assert!(error.to_string().contains("redirects are disabled"));
        redirect_thread.join().expect("redirect join");
        target_thread.join().expect("target join");
        assert!(!contacted.load(Ordering::Relaxed));
    }

    #[test]
    fn failed_refresh_keeps_original_timestamp_and_publishes_gap() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().to_path_buf();
        let worker = worker().with_bus_root(Some(root.clone()));
        let original = build_snapshot("rig-1", fix(), CAPTURED_ADSB, FETCHED_AT_MS).expect("parse");
        let mut last = None;
        assert!(worker.apply_result(Ok(PreparedResponse::Modified(original)), fix(), &mut last));
        assert!(!worker.apply_result(Err(io::Error::other("timeout")), fix(), &mut last));
        assert_eq!(last.as_ref().expect("last").fetched_at_ms, FETCHED_AT_MS);
        assert!(last
            .as_ref()
            .expect("last")
            .gaps
            .iter()
            .any(|gap| gap.contains("timeout")));
        let persist = Persist::open(root).expect("bus");
        let rows = persist
            .list_since(&aircraft_state_topic("rig-1"), None)
            .expect("read");
        assert_eq!(rows.len(), 2);
    }

    struct SlowProbe;

    impl AircraftProbe for SlowProbe {
        fn fetch(&self, _fix: VehicleFix) -> io::Result<ProbeResponse> {
            std::thread::sleep(Duration::from_millis(500));
            Ok(ProbeResponse::Modified(CAPTURED_ADSB.to_string()))
        }
    }

    #[tokio::test]
    async fn shutdown_wins_while_blocking_http_is_in_flight() {
        let worker = worker();
        let (tx, rx) = tokio::sync::watch::channel(false);
        let mut shutdown = ShutdownToken::from_receiver(rx);
        let sender = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            tx.send(true).expect("shutdown");
        });
        let result = tokio::time::timeout(
            Duration::from_millis(200),
            worker.fetch_async(Arc::new(SlowProbe), fix(), &mut shutdown),
        )
        .await
        .expect("runtime remains responsive");
        assert!(result.is_none());
        sender.await.expect("sender");
    }
}
