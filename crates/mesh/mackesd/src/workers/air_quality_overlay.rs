//! WL-FUNC-012 / OVERLAY-7 — US EPA AirNow monitoring-site AQI.
//!
//! The official endpoint requires a free deployment key. The key is resolved
//! from mde-seal's replicated secret store and is never accepted from plaintext
//! environment variables or written to logs. Missing credentials publish an
//! explicit unconfigured snapshot and make no network request.

#![cfg(feature = "async-services")]

use std::collections::BTreeMap;
use std::io::{self, Read};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, NaiveDateTime, Utc};
use mackes_mesh_types::air_quality::{
    air_quality_state_topic, AirNowAvailability, AirQualitySnapshot, AirQualityStation,
    ATTRIBUTION, LICENSE_TIER,
};
use reqwest::blocking::Client;
use reqwest::header::{CONTENT_TYPE, RETRY_AFTER};
use serde_json::{Map, Value};

use super::{ShutdownToken, Worker};

/// Explicit overlay opt-in. A true value with no sealed key is visible as
/// unconfigured, but never contacts AirNow.
pub const ENABLED_ENV: &str = "MDE_OVERLAY_AIRNOW_AQI";
/// Stable mde-seal secret reference for the free per-deployment AirNow key.
pub const API_KEY_SECRET_REF: &str = "airnow-api-key";
/// Official monitoring-site observations endpoint.
pub const DEFAULT_ENDPOINT: &str = "https://www.airnowapi.org/aq/data/";
/// AirNow observations update hourly; poll four times per hour as recommended.
pub const POLL: Duration = Duration::from_secs(15 * 60);

const RETRY_MAX: Duration = Duration::from_secs(2 * 60 * 60);
const HTTP_TIMEOUT: Duration = Duration::from_secs(25);
const QUERY_RADIUS_KM: u16 = 100;
const QUERY_LOOKBACK_HOURS: i64 = 3;
const MAX_BODY_BYTES: usize = 1024 * 1024;
const MAX_FEED_RECORDS: usize = 2_000;
const MAX_RETAINED_STATIONS: usize = 256;
const MAX_STRING_BYTES: usize = 256;
const MAX_GAPS: usize = 128;
const VEHICLE_FIX_MAX_AGE_MS: i64 = 30_000;
const VEHICLE_MAX_FUTURE_SKEW_MS: i64 = 5_000;
const USER_AGENT: &str =
    "Construct/12 mackesd AirNow-AQI-overlay (+https://github.com/matthewmackes/magic-mesh)";

/// Fresh finite vehicle point inside supported US coverage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AirQualityContext {
    /// WGS-84 latitude.
    pub latitude: f64,
    /// WGS-84 longitude.
    pub longitude: f64,
}

/// Operator-safe fetch failure. It deliberately never carries a request URL,
/// because the AirNow key is a query parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeFailure {
    message: String,
    retry_after: Option<Duration>,
    reload_key: bool,
}

impl ProbeFailure {
    fn other(message: impl std::fmt::Display) -> Self {
        Self {
            message: message.to_string(),
            retry_after: None,
            reload_key: false,
        }
    }

    fn rate_limited(delay: Duration) -> Self {
        Self {
            message: "AirNow rate limited the query".to_string(),
            retry_after: Some(delay.clamp(POLL, RETRY_MAX)),
            reload_key: false,
        }
    }

    fn authentication() -> Self {
        Self {
            message: format!(
                "AirNow rejected the sealed credential; rotate secret:{API_KEY_SECRET_REF}"
            ),
            retry_after: Some(POLL),
            reload_key: true,
        }
    }
}

impl std::fmt::Display for ProbeFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ProbeFailure {}

/// Injectable AirNow probe; tests use the documented official JSON schema.
pub trait AirQualityProbe: Send + Sync {
    /// Query recent PM2.5 and ozone AQI records for a vehicle-centred box.
    fn fetch(&self, context: AirQualityContext, fetched_at_ms: i64)
        -> Result<String, ProbeFailure>;
}

trait ApiKeySource: Send + Sync {
    fn load(&self) -> Result<Option<String>, String>;
}

struct SealedApiKeySource;

impl ApiKeySource for SealedApiKeySource {
    fn load(&self) -> Result<Option<String>, String> {
        let store = crate::ipc::secret_store::SecretStore::resolve(
            &crate::ipc::secret_store::repo_root(),
            &crate::default_qnm_shared_root(),
        );
        store.get(API_KEY_SECRET_REF)
    }
}

/// Production rustls AirNow client. No `Debug` implementation is deliberate:
/// the struct owns the deployment credential.
pub struct AirNowHttpProbe {
    client: Client,
    api_key: String,
}

impl AirNowHttpProbe {
    fn new(api_key: String) -> Result<Self, ProbeFailure> {
        validate_endpoint(DEFAULT_ENDPOINT).map_err(ProbeFailure::other)?;
        let api_key = validate_api_key(&api_key).map_err(ProbeFailure::other)?;
        let client = Client::builder()
            .timeout(HTTP_TIMEOUT)
            .user_agent(USER_AGENT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| ProbeFailure::other(error.without_url()))?;
        Ok(Self { client, api_key })
    }
}

impl AirQualityProbe for AirNowHttpProbe {
    fn fetch(
        &self,
        context: AirQualityContext,
        fetched_at_ms: i64,
    ) -> Result<String, ProbeFailure> {
        let url = query_url(context, fetched_at_ms, &self.api_key).map_err(ProbeFailure::other)?;
        let response = self
            .client
            .get(url)
            .send()
            .map_err(|error| ProbeFailure::other(error.without_url()))?;
        if matches!(
            response.status(),
            reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
        ) {
            return Err(ProbeFailure::authentication());
        }
        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let delay = response
                .headers()
                .get(RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
                .map(Duration::from_secs)
                .unwrap_or(POLL);
            return Err(ProbeFailure::rate_limited(delay));
        }
        if response.status() != reqwest::StatusCode::OK {
            return Err(ProbeFailure::other(format!(
                "AirNow returned unexpected HTTP {} (redirects are disabled)",
                response.status()
            )));
        }
        require_json(&response)?;
        let mut response = response;
        read_bounded(&mut response)
    }
}

fn validate_api_key(value: &str) -> io::Result<String> {
    let value = value.trim();
    if value.len() < 8
        || value.len() > 256
        || value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(io::Error::other("sealed AirNow API key is invalid"));
    }
    Ok(value.to_string())
}

fn validate_endpoint(value: &str) -> io::Result<reqwest::Url> {
    let url = reqwest::Url::parse(value).map_err(io_other)?;
    if url.scheme() != "https"
        || url.host_str() != Some("www.airnowapi.org")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/aq/data/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(io::Error::other(
            "AirNow endpoint is outside the strict official-service allowlist",
        ));
    }
    Ok(url)
}

fn query_url(
    context: AirQualityContext,
    fetched_at_ms: i64,
    api_key: &str,
) -> io::Result<reqwest::Url> {
    validate_context(context)?;
    let api_key = validate_api_key(api_key)?;
    let end = DateTime::<Utc>::from_timestamp_millis(fetched_at_ms)
        .ok_or_else(|| io::Error::other("invalid AirNow query time"))?;
    let start = end - chrono::Duration::hours(QUERY_LOOKBACK_HOURS);
    let (west, south, east, north) = query_envelope(context);
    let bbox = format!("{west:.6},{south:.6},{east:.6},{north:.6}");
    let mut url = validate_endpoint(DEFAULT_ENDPOINT)?;
    url.query_pairs_mut()
        .append_pair("startDate", &start.format("%Y-%m-%dT%H").to_string())
        .append_pair("endDate", &end.format("%Y-%m-%dT%H").to_string())
        .append_pair("parameters", "PM25,OZONE")
        .append_pair("BBOX", &bbox)
        .append_pair("dataType", "A")
        .append_pair("format", "application/json")
        .append_pair("verbose", "1")
        .append_pair("monitorType", "0")
        .append_pair("includerawconcentrations", "0")
        .append_pair("API_KEY", &api_key);
    Ok(url)
}

fn query_envelope(context: AirQualityContext) -> (f64, f64, f64, f64) {
    let latitude_delta = f64::from(QUERY_RADIUS_KM) / 111.32;
    let longitude_delta =
        f64::from(QUERY_RADIUS_KM) / (111.32 * context.latitude.to_radians().cos().abs().max(0.1));
    (
        (context.longitude - longitude_delta).max(-180.0),
        (context.latitude - latitude_delta).max(-90.0),
        (context.longitude + longitude_delta).min(180.0),
        (context.latitude + latitude_delta).min(90.0),
    )
}

fn require_json(response: &reqwest::blocking::Response) -> Result<(), ProbeFailure> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_BODY_BYTES as u64)
    {
        return Err(ProbeFailure::other("AirNow response exceeds byte limit"));
    }
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default();
    if content_type != "application/json" {
        return Err(ProbeFailure::other(format!(
            "AirNow returned unexpected content type {content_type:?}"
        )));
    }
    Ok(())
}

fn read_bounded(reader: &mut impl Read) -> Result<String, ProbeFailure> {
    let mut bytes = Vec::with_capacity(64 * 1024);
    reader
        .take(MAX_BODY_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(ProbeFailure::other)?;
    if bytes.len() > MAX_BODY_BYTES {
        return Err(ProbeFailure::other("AirNow response exceeds byte limit"));
    }
    String::from_utf8(bytes).map_err(ProbeFailure::other)
}

fn parse_snapshot(
    host: &str,
    context: AirQualityContext,
    body: &str,
    fetched_at_ms: i64,
) -> Result<AirQualitySnapshot, ProbeFailure> {
    validate_context(context).map_err(ProbeFailure::other)?;
    if body.len() > MAX_BODY_BYTES {
        return Err(ProbeFailure::other("AirNow response exceeds byte limit"));
    }
    let records: Vec<Value> = serde_json::from_str(body).map_err(ProbeFailure::other)?;
    let record_count = records.len();
    let mut snapshot = AirQualitySnapshot::empty(
        host,
        fetched_at_ms,
        fetched_at_ms,
        context.latitude,
        context.longitude,
        QUERY_RADIUS_KM,
    );
    if record_count > MAX_FEED_RECORDS {
        push_gap(
            &mut snapshot.gaps,
            format!("AirNow records capped at {MAX_FEED_RECORDS}"),
        );
    }
    let mut latest = BTreeMap::<String, AirQualityStation>::new();
    for (index, record) in records.into_iter().take(MAX_FEED_RECORDS).enumerate() {
        match parse_record(&record, context, fetched_at_ms) {
            Ok(Some(station)) => {
                let dedup_key = format!("{}:{}", station.id, station.parameter);
                match latest.get(&dedup_key) {
                    Some(previous) if previous.observed_at_ms >= station.observed_at_ms => {
                        snapshot.omitted_records = snapshot.omitted_records.saturating_add(1);
                    }
                    Some(_) => {
                        latest.insert(dedup_key, station);
                        snapshot.omitted_records = snapshot.omitted_records.saturating_add(1);
                    }
                    None => {
                        latest.insert(dedup_key, station);
                    }
                }
            }
            Ok(None) => snapshot.omitted_records = snapshot.omitted_records.saturating_add(1),
            Err(error) => {
                snapshot.omitted_records = snapshot.omitted_records.saturating_add(1);
                push_gap(
                    &mut snapshot.gaps,
                    format!("AirNow record {index} omitted: {error}"),
                );
            }
        }
    }
    snapshot.omitted_records = snapshot.omitted_records.saturating_add(
        u32::try_from(record_count.saturating_sub(MAX_FEED_RECORDS)).unwrap_or(u32::MAX),
    );
    snapshot.stations = latest.into_values().collect();
    snapshot.stations.sort_by(|a, b| {
        b.aqi
            .cmp(&a.aqi)
            .then_with(|| a.distance_km.total_cmp(&b.distance_km))
    });
    if snapshot.stations.len() > MAX_RETAINED_STATIONS {
        let omitted = snapshot.stations.len() - MAX_RETAINED_STATIONS;
        snapshot.stations.truncate(MAX_RETAINED_STATIONS);
        snapshot.omitted_records = snapshot
            .omitted_records
            .saturating_add(u32::try_from(omitted).unwrap_or(u32::MAX));
        push_gap(
            &mut snapshot.gaps,
            format!("AirNow stations capped at {MAX_RETAINED_STATIONS}"),
        );
    }
    Ok(snapshot)
}

fn parse_record(
    record: &Value,
    context: AirQualityContext,
    fetched_at_ms: i64,
) -> io::Result<Option<AirQualityStation>> {
    let object = record
        .as_object()
        .ok_or_else(|| io::Error::other("record is not an object"))?;
    let latitude = number_field(object, &["Latitude", "latitude"])
        .ok_or_else(|| io::Error::other("missing latitude"))?;
    let longitude = number_field(object, &["Longitude", "longitude"])
        .ok_or_else(|| io::Error::other("missing longitude"))?;
    if !latitude.is_finite()
        || !longitude.is_finite()
        || !(-90.0..=90.0).contains(&latitude)
        || !(-180.0..=180.0).contains(&longitude)
    {
        return Err(io::Error::other("invalid coordinates"));
    }
    let distance_km = great_circle_km(context.latitude, context.longitude, latitude, longitude);
    if distance_km > f64::from(QUERY_RADIUS_KM) {
        return Ok(None);
    }
    let parameter = string_field(object, &["Parameter", "ParameterName", "parameter"], 32)
        .and_then(|parameter| normalize_parameter(&parameter))
        .ok_or_else(|| io::Error::other("unsupported pollutant"))?;
    let aqi = integer_field(object, &["AQI", "aqi"])
        .filter(|aqi| (0..=500).contains(aqi))
        .ok_or_else(|| io::Error::other("missing or invalid AQI"))? as u16;
    let observed = string_field(object, &["UTC", "utc"], 32)
        .ok_or_else(|| io::Error::other("missing UTC observation hour"))?;
    let observed_at_ms = parse_utc_hour(&observed)?;
    if observed_at_ms > fetched_at_ms.saturating_add(60 * 60 * 1_000)
        || fetched_at_ms.saturating_sub(observed_at_ms) > 6 * 60 * 60 * 1_000
    {
        return Ok(None);
    }
    let name = string_field(object, &["SiteName", "siteName"], MAX_STRING_BYTES);
    let id = string_field(
        object,
        &["FullAQSCode", "AQSID", "IntlAQSCode", "SiteId"],
        64,
    )
    .unwrap_or_else(|| format!("{latitude:.5},{longitude:.5}"));
    Ok(Some(AirQualityStation {
        id,
        name,
        parameter,
        aqi,
        latitude,
        longitude,
        distance_km: distance_km as f32,
        observed_at_ms,
    }))
}

fn number_field(object: &Map<String, Value>, names: &[&str]) -> Option<f64> {
    names.iter().find_map(|name| {
        object.get(*name).and_then(|value| {
            value
                .as_f64()
                .or_else(|| value.as_str()?.trim().parse::<f64>().ok())
        })
    })
}

fn integer_field(object: &Map<String, Value>, names: &[&str]) -> Option<i64> {
    names.iter().find_map(|name| {
        object.get(*name).and_then(|value| {
            value
                .as_i64()
                .or_else(|| value.as_str()?.trim().parse::<i64>().ok())
        })
    })
}

fn string_field(object: &Map<String, Value>, names: &[&str], max: usize) -> Option<String> {
    names.iter().find_map(|name| {
        let value = object.get(*name)?;
        let text = value
            .as_str()
            .map(str::to_string)
            .or_else(|| value.as_i64().map(|value| value.to_string()))?;
        let text = text.trim();
        (!text.is_empty()
            && text.len() <= max
            && text.chars().all(|character| !character.is_control()))
        .then(|| text.to_string())
    })
}

fn normalize_parameter(value: &str) -> Option<String> {
    match value.trim().to_ascii_uppercase().as_str() {
        "PM25" | "PM2.5" | "PM2_5" => Some("PM2.5".to_string()),
        "OZONE" | "O3" => Some("OZONE".to_string()),
        _ => None,
    }
}

fn parse_utc_hour(value: &str) -> io::Result<i64> {
    if let Ok(value) = DateTime::parse_from_rfc3339(value) {
        return Ok(value.timestamp_millis());
    }
    NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M")
        .map(|value| value.and_utc().timestamp_millis())
        .map_err(io_other)
}

fn validate_context(context: AirQualityContext) -> io::Result<()> {
    let valid = context.latitude.is_finite()
        && context.longitude.is_finite()
        && ((24.0..=50.0).contains(&context.latitude)
            && (-125.0..=-66.0).contains(&context.longitude)
            || (51.0..=72.0).contains(&context.latitude)
                && (-180.0..=-129.0).contains(&context.longitude)
            || (18.0..=23.0).contains(&context.latitude)
                && (-161.0..=-154.0).contains(&context.longitude)
            || (17.0..=19.0).contains(&context.latitude)
                && (-68.0..=-64.0).contains(&context.longitude));
    if valid {
        Ok(())
    } else {
        Err(io::Error::other(
            "fresh vehicle point is outside supported US AirNow coverage",
        ))
    }
}

fn great_circle_km(lat_a: f64, lon_a: f64, lat_b: f64, lon_b: f64) -> f64 {
    let lat_a = lat_a.to_radians();
    let lat_b = lat_b.to_radians();
    let delta_lat = lat_b - lat_a;
    let delta_lon = (lon_b - lon_a).to_radians();
    let haversine = (delta_lat * 0.5).sin().powi(2)
        + lat_a.cos() * lat_b.cos() * (delta_lon * 0.5).sin().powi(2);
    6_371.0 * 2.0 * haversine.clamp(0.0, 1.0).sqrt().asin()
}

fn push_gap(gaps: &mut Vec<String>, gap: String) {
    if gaps.len() < MAX_GAPS {
        gaps.push(gap);
    }
}

fn io_other(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

#[derive(Debug, Clone, Copy)]
struct ApplyOutcome {
    success: bool,
    retry_after: Option<Duration>,
    reload_key: bool,
}

/// Workstation-side credential-gated AirNow adapter.
pub struct AirQualityOverlayWorker {
    host: String,
    enabled: bool,
    probe: Option<Arc<dyn AirQualityProbe>>,
    key_source: Arc<dyn ApiKeySource>,
    bus_root: Option<PathBuf>,
}

impl AirQualityOverlayWorker {
    /// Production wiring. Disabled unless explicitly opted in.
    #[must_use]
    pub fn new(host: String) -> Self {
        Self {
            host,
            enabled: env_truthy(ENABLED_ENV),
            probe: None,
            key_source: Arc::new(SealedApiKeySource),
            bus_root: crate::bus_publish::default_bus_root(),
        }
    }

    /// Inject a fixture probe, bypassing credential resolution in tests.
    #[must_use]
    pub fn with_probe(mut self, probe: Arc<dyn AirQualityProbe>) -> Self {
        self.enabled = true;
        self.probe = Some(probe);
        self
    }

    /// Override or disable Bus access.
    #[must_use]
    pub fn with_bus_root(mut self, root: Option<PathBuf>) -> Self {
        self.bus_root = root;
        self
    }

    fn current_context(&self) -> Option<AirQualityContext> {
        let root = self.bus_root.clone()?;
        let persist = mde_bus::persist::Persist::open(root).ok()?;
        let topic = mackes_mesh_types::vehicle::vehicle_state_topic(&self.host);
        let body = persist.read_latest(&topic).ok().flatten()?.body?;
        let vehicle: mackes_mesh_types::vehicle::VehicleState = serde_json::from_str(&body).ok()?;
        validated_vehicle_context(&vehicle, &self.host, now_ms())
    }

    fn publish(&self, snapshot: &AirQualitySnapshot) {
        if let Some(mut persist) = crate::bus_publish::open_bus(self.bus_root.clone()) {
            crate::bus_publish::publish_json(
                &mut persist,
                &air_quality_state_topic(&self.host),
                snapshot,
            );
        }
    }

    fn status_snapshot(
        &self,
        availability: AirNowAvailability,
        context: Option<AirQualityContext>,
        gap: String,
    ) -> AirQualitySnapshot {
        let now = now_ms();
        AirQualitySnapshot {
            host: self.host.clone(),
            published_at_ms: now,
            fetched_at_ms: None,
            query_latitude: context.map(|context| context.latitude),
            query_longitude: context.map(|context| context.longitude),
            query_radius_km: QUERY_RADIUS_KM,
            availability,
            stations: Vec::new(),
            omitted_records: 0,
            gaps: vec![gap],
            license_tier: LICENSE_TIER.to_string(),
            attribution: ATTRIBUTION.to_string(),
        }
    }

    fn apply_result(
        &self,
        result: Result<AirQualitySnapshot, ProbeFailure>,
        context: AirQualityContext,
        last_good: &mut Option<AirQualitySnapshot>,
    ) -> ApplyOutcome {
        match result {
            Ok(snapshot) => {
                self.publish(&snapshot);
                *last_good = Some(snapshot);
                ApplyOutcome {
                    success: true,
                    retry_after: None,
                    reload_key: false,
                }
            }
            Err(error) => {
                if let Some(snapshot) = last_good {
                    snapshot.published_at_ms = now_ms();
                    snapshot
                        .gaps
                        .retain(|gap| !gap.starts_with("AirNow AQI paused:"));
                    push_gap(&mut snapshot.gaps, format!("AirNow AQI paused: {error}"));
                    self.publish(snapshot);
                } else {
                    self.publish(&self.status_snapshot(
                        AirNowAvailability::Ready,
                        Some(context),
                        format!("AirNow AQI paused: {error}"),
                    ));
                }
                ApplyOutcome {
                    success: false,
                    retry_after: error.retry_after,
                    reload_key: error.reload_key,
                }
            }
        }
    }

    async fn load_probe(
        &self,
        shutdown: &mut ShutdownToken,
    ) -> Option<Result<Option<Arc<dyn AirQualityProbe>>, ProbeFailure>> {
        let source = self.key_source.clone();
        let task = tokio::task::spawn_blocking(move || source.load());
        tokio::select! {
            () = shutdown.wait() => None,
            joined = task => Some(match joined {
                Ok(Ok(Some(key))) => AirNowHttpProbe::new(key)
                    .map(|probe| Some(Arc::new(probe) as Arc<dyn AirQualityProbe>)),
                Ok(Ok(None)) => Ok(None),
                Ok(Err(error)) => Err(ProbeFailure::other(format!("AirNow secret store unavailable: {error}"))),
                Err(error) => Err(ProbeFailure::other(format!("AirNow secret task failed: {error}"))),
            }),
        }
    }

    async fn fetch_async(
        &self,
        probe: Arc<dyn AirQualityProbe>,
        context: AirQualityContext,
        shutdown: &mut ShutdownToken,
    ) -> Option<Result<AirQualitySnapshot, ProbeFailure>> {
        let host = self.host.clone();
        let task = tokio::task::spawn_blocking(move || {
            let fetched_at_ms = now_ms();
            let body = probe.fetch(context, fetched_at_ms)?;
            parse_snapshot(&host, context, &body, fetched_at_ms)
        });
        tokio::select! {
            () = shutdown.wait() => None,
            joined = task => Some(match joined {
                Ok(result) => result,
                Err(error) => Err(ProbeFailure::other(format!("AirNow fetch task failed: {error}"))),
            }),
        }
    }
}

fn validated_vehicle_context(
    vehicle: &mackes_mesh_types::vehicle::VehicleState,
    expected_host: &str,
    now: i64,
) -> Option<AirQualityContext> {
    let mirror_age = now.saturating_sub(vehicle.published_at_ms).max(0);
    let future_skew = vehicle.published_at_ms.saturating_sub(now).max(0);
    let gps = &vehicle.gps;
    if vehicle.host != expected_host
        || !vehicle.online
        || !gps.has_fix()
        || !gps.latitude.is_finite()
        || !gps.longitude.is_finite()
        || !gps.age_s.is_finite()
        || gps.age_s < 0.0
        || future_skew > VEHICLE_MAX_FUTURE_SKEW_MS
        || mirror_age as f64 + f64::from(gps.age_s) * 1_000.0 > VEHICLE_FIX_MAX_AGE_MS as f64
    {
        return None;
    }
    let context = AirQualityContext {
        latitude: gps.latitude,
        longitude: gps.longitude,
    };
    validate_context(context).ok().map(|()| context)
}

#[async_trait::async_trait]
impl Worker for AirQualityOverlayWorker {
    fn name(&self) -> &'static str {
        "air_quality_overlay"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        if !self.enabled {
            shutdown.wait().await;
            return Ok(());
        }
        let mut probe = self.probe.clone();
        let mut last_good: Option<AirQualitySnapshot> = None;
        let mut retry = POLL;
        let mut no_fix_published = false;
        let mut unconfigured_published = false;
        loop {
            if probe.is_none() {
                let Some(result) = self.load_probe(&mut shutdown).await else {
                    break;
                };
                match result {
                    Ok(Some(loaded)) => {
                        probe = Some(loaded);
                        unconfigured_published = false;
                    }
                    Ok(None) => {
                        if !unconfigured_published {
                            self.publish(&AirQualitySnapshot::unconfigured(&self.host, now_ms()));
                            unconfigured_published = true;
                        }
                        tokio::select! {
                            () = shutdown.wait() => break,
                            () = tokio::time::sleep(POLL) => {}
                        }
                        continue;
                    }
                    Err(error) => {
                        self.publish(&self.status_snapshot(
                            AirNowAvailability::SecretStoreError,
                            None,
                            error.to_string(),
                        ));
                        tokio::select! {
                            () = shutdown.wait() => break,
                            () = tokio::time::sleep(POLL) => {}
                        }
                        continue;
                    }
                }
            }
            let Some(context) = self.current_context() else {
                if !no_fix_published {
                    let unavailable =
                        ProbeFailure::other("fresh same-host US vehicle fix unavailable");
                    if let Some(last_context) = last_good.as_ref().and_then(|snapshot| {
                        Some(AirQualityContext {
                            latitude: snapshot.query_latitude?,
                            longitude: snapshot.query_longitude?,
                        })
                    }) {
                        self.apply_result(Err(unavailable), last_context, &mut last_good);
                    } else {
                        self.publish(&self.status_snapshot(
                            AirNowAvailability::Ready,
                            None,
                            unavailable.to_string(),
                        ));
                    }
                    no_fix_published = true;
                }
                tokio::select! {
                    () = shutdown.wait() => break,
                    () = tokio::time::sleep(POLL) => {}
                }
                continue;
            };
            no_fix_published = false;
            let Some(result) = self
                .fetch_async(
                    probe.as_ref().expect("probe loaded").clone(),
                    context,
                    &mut shutdown,
                )
                .await
            else {
                break;
            };
            let outcome = self.apply_result(result, context, &mut last_good);
            if outcome.reload_key && self.probe.is_none() {
                probe = None;
            }
            let delay = if outcome.success {
                POLL
            } else {
                outcome.retry_after.unwrap_or(retry).min(RETRY_MAX)
            };
            retry = if outcome.success {
                POLL
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
    use mackes_mesh_types::vehicle::{GpsFix, VehicleState};
    use mde_bus::persist::Persist;
    use serde_json::json;

    use super::*;

    const NOW_MS: i64 = 1_784_760_900_000;

    fn context() -> AirQualityContext {
        AirQualityContext {
            latitude: 35.78,
            longitude: -78.64,
        }
    }

    fn official_schema_fixture() -> String {
        json!([
            {
                "Latitude": 35.7829,
                "Longitude": -78.5742,
                "UTC": "2026-07-22T17:00",
                "Parameter": "PM25",
                "AQI": 92,
                "Category": 2,
                "SiteName": "Millbrook School",
                "AgencyName": "North Carolina DAQ",
                "FullAQSCode": "840371830014"
            },
            {
                "Latitude": 35.7829,
                "Longitude": -78.5742,
                "UTC": "2026-07-22T18:00",
                "Parameter": "PM25",
                "AQI": 156,
                "Category": 4,
                "SiteName": "Millbrook School",
                "AgencyName": "North Carolina DAQ",
                "FullAQSCode": "840371830014"
            },
            {
                "Latitude": 35.8561,
                "Longitude": -78.5742,
                "UTC": "2026-07-22T18:00",
                "Parameter": "OZONE",
                "AQI": 48,
                "Category": 1,
                "FullAQSCode": "840371830015"
            }
        ])
        .to_string()
    }

    #[test]
    fn official_monitoring_site_schema_keeps_latest_and_high_aqi() {
        let snapshot = parse_snapshot("rig-1", context(), &official_schema_fixture(), NOW_MS)
            .expect("snapshot");
        assert_eq!(snapshot.stations.len(), 2);
        assert_eq!(snapshot.omitted_records, 1);
        assert_eq!(snapshot.stations[0].aqi, 156);
        assert_eq!(snapshot.stations[0].parameter, "PM2.5");
        assert_eq!(snapshot.stations[0].id, "840371830014");
        assert_eq!(snapshot.fetched_at_ms, Some(NOW_MS));
    }

    #[test]
    fn endpoint_bbox_key_and_payload_limits_are_strict() {
        let url = query_url(context(), NOW_MS, "test-api-key").expect("query");
        assert_eq!(url.host_str(), Some("www.airnowapi.org"));
        let query: BTreeMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(
            query.get("parameters").map(String::as_str),
            Some("PM25,OZONE")
        );
        assert_eq!(query.get("dataType").map(String::as_str), Some("A"));
        assert_eq!(
            query.get("API_KEY").map(String::as_str),
            Some("test-api-key")
        );
        assert_eq!(
            query.get("startDate").map(String::as_str),
            Some("2026-07-22T19")
        );
        assert_eq!(
            query.get("endDate").map(String::as_str),
            Some("2026-07-22T22")
        );
        for hostile in [
            DEFAULT_ENDPOINT.replace("https", "http"),
            DEFAULT_ENDPOINT.replace("www.airnowapi.org", "www.airnowapi.org.evil.test"),
            format!("{DEFAULT_ENDPOINT}?API_KEY=leak"),
        ] {
            assert!(validate_endpoint(&hostile).is_err(), "accepted {hostile}");
        }
        assert!(validate_api_key("short").is_err());
        assert!(
            parse_snapshot("rig-1", context(), &"x".repeat(MAX_BODY_BYTES + 1), NOW_MS).is_err()
        );
    }

    #[test]
    fn vehicle_context_requires_fresh_same_host_us_fix() {
        let mut vehicle = VehicleState::offline("rig-1");
        vehicle.online = true;
        vehicle.published_at_ms = 100_000;
        vehicle.gps = GpsFix {
            fix_type: "gps".to_string(),
            latitude: 35.78,
            longitude: -78.64,
            satellites: 8,
            age_s: 1.0,
            ..GpsFix::default()
        };
        assert!(validated_vehicle_context(&vehicle, "rig-1", 110_000).is_some());
        assert!(validated_vehicle_context(&vehicle, "other", 110_000).is_none());
        vehicle.gps.latitude = 0.0;
        vehicle.gps.longitude = 0.0;
        assert!(validated_vehicle_context(&vehicle, "rig-1", 100_000).is_none());
    }

    #[test]
    fn auth_and_rate_limit_failures_never_contain_the_key() {
        let auth = ProbeFailure::authentication();
        assert!(auth.reload_key);
        assert!(!auth.to_string().contains("test-api-key"));
        let limited = ProbeFailure::rate_limited(Duration::from_secs(60));
        assert_eq!(limited.retry_after, Some(POLL));
    }

    #[test]
    fn failed_refresh_retains_fetch_time_and_publishes_paused_last_good() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().to_path_buf();
        let worker =
            AirQualityOverlayWorker::new("rig-1".to_string()).with_bus_root(Some(root.clone()));
        let original = parse_snapshot("rig-1", context(), &official_schema_fixture(), NOW_MS)
            .expect("snapshot");
        let mut last_good = None;
        assert!(
            worker
                .apply_result(Ok(original), context(), &mut last_good)
                .success
        );
        let paused = worker.apply_result(
            Err(ProbeFailure::rate_limited(Duration::from_secs(60))),
            context(),
            &mut last_good,
        );
        assert!(!paused.success);
        let retained = last_good.as_ref().expect("last good");
        assert_eq!(retained.fetched_at_ms, Some(NOW_MS));
        assert!(retained
            .gaps
            .iter()
            .any(|gap| gap.starts_with("AirNow AQI paused:")));
        assert_eq!(
            Persist::open(root)
                .expect("bus")
                .list_since(&air_quality_state_topic("rig-1"), None)
                .expect("rows")
                .len(),
            2
        );
    }

    struct MissingKey;

    impl ApiKeySource for MissingKey {
        fn load(&self) -> Result<Option<String>, String> {
            Ok(None)
        }
    }

    #[tokio::test]
    async fn missing_sealed_key_publishes_unconfigured_without_fetch_time() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().to_path_buf();
        let mut worker =
            AirQualityOverlayWorker::new("rig-1".to_string()).with_bus_root(Some(root.clone()));
        worker.enabled = true;
        worker.key_source = Arc::new(MissingKey);
        let (tx, rx) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(async move { worker.run(ShutdownToken::from_receiver(rx)).await });
        let topic = air_quality_state_topic("rig-1");
        let mut decoded = None;
        for _ in 0..20 {
            if let Some(body) = Persist::open(root.clone())
                .ok()
                .and_then(|persist| persist.read_latest(&topic).ok().flatten())
                .and_then(|event| event.body)
            {
                decoded = serde_json::from_str::<AirQualitySnapshot>(&body).ok();
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        tx.send(true).expect("shutdown");
        task.await.expect("join").expect("worker");
        let snapshot = decoded.expect("unconfigured snapshot");
        assert_eq!(snapshot.availability, AirNowAvailability::Unconfigured);
        assert_eq!(snapshot.fetched_at_ms, None);
        assert!(snapshot.stations.is_empty());
    }

    struct SlowProbe;

    impl AirQualityProbe for SlowProbe {
        fn fetch(
            &self,
            _context: AirQualityContext,
            _fetched_at_ms: i64,
        ) -> Result<String, ProbeFailure> {
            std::thread::sleep(Duration::from_millis(500));
            Ok(official_schema_fixture())
        }
    }

    #[tokio::test]
    async fn shutdown_wins_while_airnow_query_is_in_flight() {
        let worker = AirQualityOverlayWorker::new("rig-1".to_string()).with_bus_root(None);
        let (tx, rx) = tokio::sync::watch::channel(false);
        let mut shutdown = ShutdownToken::from_receiver(rx);
        let sender = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            tx.send(true).expect("shutdown");
        });
        let result = tokio::time::timeout(
            Duration::from_millis(200),
            worker.fetch_async(Arc::new(SlowProbe), context(), &mut shutdown),
        )
        .await
        .expect("responsive");
        assert!(result.is_none());
        sender.await.expect("sender");
    }
}
