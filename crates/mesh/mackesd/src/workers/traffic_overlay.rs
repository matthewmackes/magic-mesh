//! WL-FUNC-012 / OVERLAY-3 — keyless NCDOT TIMS current traffic events.

#![cfg(feature = "async-services")]

use std::io::{self, Read};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mackes_mesh_types::traffic::{traffic_state_topic, TrafficEvent, TrafficSnapshot};
use reqwest::blocking::Client;
use reqwest::header::{CONTENT_TYPE, ETAG, IF_NONE_MATCH, RETRY_AFTER};
use serde::Deserialize;

use super::{ShutdownToken, Worker};

/// Explicit opt-in; unset/false is an idle no-op.
pub const ENABLED_ENV: &str = "MDE_OVERLAY_NCDOT_TRAFFIC";
/// Official keyless NCDOT TIMS current incident-point service.
pub const DEFAULT_ENDPOINT: &str = "https://services.arcgis.com/NuWFvHYDMVmmxMeM/ArcGIS/rest/services/NCDOT_TIMS_Incidents/FeatureServer/0/query";
/// Current-event refresh cadence.
pub const POLL: Duration = Duration::from_secs(60);

const RETRY_MAX: Duration = Duration::from_secs(15 * 60);
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);
const QUERY_RADIUS_KM: u16 = 100;
const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
const MAX_FEED_FEATURES: usize = 1_000;
const MAX_RETAINED_EVENTS: usize = 256;
const MAX_STRING_BYTES: usize = 512;
const MAX_GAPS: usize = 128;
const VEHICLE_FIX_MAX_AGE_MS: i64 = 30_000;
const VEHICLE_MAX_FUTURE_SKEW_MS: i64 = 5_000;
const USER_AGENT: &str =
    "Construct/12 mackesd NCDOT-TIMS-overlay (+https://github.com/matthewmackes/magic-mesh)";

/// Fresh finite North Carolina vehicle point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrafficContext {
    /// WGS-84 latitude.
    pub latitude: f64,
    /// WGS-84 longitude.
    pub longitude: f64,
}

/// Conditional TIMS response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeResponse {
    /// HTTP 200 bounded GeoJSON.
    Modified(String),
    /// Validator-backed HTTP 304.
    NotModified,
}

/// Operator-safe fetch failure with optional ArcGIS retry timing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeFailure {
    message: String,
    retry_after: Option<Duration>,
}

impl ProbeFailure {
    fn other(message: impl std::fmt::Display) -> Self {
        Self {
            message: message.to_string(),
            retry_after: None,
        }
    }

    fn rate_limited(delay: Duration, message: impl std::fmt::Display) -> Self {
        Self {
            message: message.to_string(),
            retry_after: Some(delay.clamp(POLL, RETRY_MAX)),
        }
    }
}

impl std::fmt::Display for ProbeFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ProbeFailure {}

/// Injectable NCDOT probe; tests use captured live payloads.
pub trait TrafficProbe: Send + Sync {
    /// Query current nearby incident points.
    fn fetch(&self, context: TrafficContext) -> Result<ProbeResponse, ProbeFailure>;
}

#[derive(Debug, Default)]
struct Validator {
    context: Option<TrafficContext>,
    etag: Option<String>,
}

/// Production rustls NCDOT ArcGIS client.
pub struct NcdotHttpProbe {
    client: Client,
    validator: Mutex<Validator>,
}

impl NcdotHttpProbe {
    fn new() -> Result<Self, ProbeFailure> {
        validate_endpoint(DEFAULT_ENDPOINT).map_err(ProbeFailure::other)?;
        let client = Client::builder()
            .timeout(HTTP_TIMEOUT)
            .user_agent(USER_AGENT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(ProbeFailure::other)?;
        Ok(Self {
            client,
            validator: Mutex::new(Validator::default()),
        })
    }
}

impl TrafficProbe for NcdotHttpProbe {
    fn fetch(&self, context: TrafficContext) -> Result<ProbeResponse, ProbeFailure> {
        let url = query_url(context).map_err(ProbeFailure::other)?;
        let mut request = self.client.get(url);
        let mut sent_validator = false;
        {
            let validator = self
                .validator
                .lock()
                .map_err(|_| ProbeFailure::other("NCDOT validator lock poisoned"))?;
            if validator
                .context
                .is_some_and(|previous| points_near(previous, context))
            {
                if let Some(etag) = &validator.etag {
                    request = request.header(IF_NONE_MATCH, etag);
                    sent_validator = true;
                }
            }
        }
        let response = request.send().map_err(ProbeFailure::other)?;
        if response.status() == reqwest::StatusCode::NOT_MODIFIED {
            return if sent_validator {
                Ok(ProbeResponse::NotModified)
            } else {
                Err(ProbeFailure::other(
                    "NCDOT returned 304 without a matching-point validator",
                ))
            };
        }
        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let delay = response
                .headers()
                .get(RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
                .map(Duration::from_secs)
                .unwrap_or(POLL);
            return Err(ProbeFailure::rate_limited(delay, "NCDOT ArcGIS HTTP 429"));
        }
        if response.status() != reqwest::StatusCode::OK {
            return Err(ProbeFailure::other(format!(
                "NCDOT returned unexpected HTTP {} (redirects are disabled)",
                response.status()
            )));
        }
        require_json(&response)?;
        let etag = response
            .headers()
            .get(ETAG)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let mut response = response;
        let body = read_bounded(&mut response)?;
        if let Some(error) = classify_arcgis_error(&body) {
            return Err(error);
        }
        *self
            .validator
            .lock()
            .map_err(|_| ProbeFailure::other("NCDOT validator lock poisoned"))? = Validator {
            context: Some(context),
            etag,
        };
        Ok(ProbeResponse::Modified(body))
    }
}

fn validate_endpoint(value: &str) -> io::Result<reqwest::Url> {
    let url = reqwest::Url::parse(value).map_err(io_other)?;
    if url.scheme() != "https"
        || url.host_str() != Some("services.arcgis.com")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path()
            != "/NuWFvHYDMVmmxMeM/ArcGIS/rest/services/NCDOT_TIMS_Incidents/FeatureServer/0/query"
    {
        return Err(io::Error::other(
            "NCDOT endpoint is outside the strict official-service allowlist",
        ));
    }
    Ok(url)
}

fn query_url(context: TrafficContext) -> io::Result<reqwest::Url> {
    validate_context(context)?;
    let mut url = validate_endpoint(DEFAULT_ENDPOINT)?;
    let (west, south, east, north) = query_envelope(context);
    let geometry = format!("{west:.6},{south:.6},{east:.6},{north:.6}");
    url.query_pairs_mut()
        .append_pair("where", "1=1")
        .append_pair("geometry", &geometry)
        .append_pair("geometryType", "esriGeometryEnvelope")
        .append_pair("inSR", "4326")
        .append_pair("spatialRel", "esriSpatialRelIntersects")
        .append_pair("outSR", "4326")
        .append_pair(
            "outFields",
            "OBJECTID,Id,Road,Reason,Condition,EventName,LanesAffected,EventType,EventSubType,StartDateTime,EndDateTime,LastUpdateDateTime,IsFullClosure,Direction,CountyName,Location",
        )
        .append_pair("returnGeometry", "true")
        .append_pair("returnZ", "false")
        .append_pair("returnM", "false")
        .append_pair("resultRecordCount", &MAX_FEED_FEATURES.to_string())
        .append_pair("orderByFields", "LastUpdateDateTime DESC")
        .append_pair("f", "geojson");
    Ok(url)
}

fn query_envelope(context: TrafficContext) -> (f64, f64, f64, f64) {
    let latitude_delta = f64::from(QUERY_RADIUS_KM) / 111.32;
    let longitude_delta =
        f64::from(QUERY_RADIUS_KM) / (111.32 * context.latitude.to_radians().cos().abs().max(0.1));
    (
        context.longitude - longitude_delta,
        context.latitude - latitude_delta,
        context.longitude + longitude_delta,
        context.latitude + latitude_delta,
    )
}

fn require_json(response: &reqwest::blocking::Response) -> Result<(), ProbeFailure> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_BODY_BYTES as u64)
    {
        return Err(ProbeFailure::other("NCDOT response exceeds byte limit"));
    }
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default();
    if !matches!(content_type, "application/json" | "application/geo+json") {
        return Err(ProbeFailure::other(format!(
            "NCDOT returned unexpected content type {content_type:?}"
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
        return Err(ProbeFailure::other("NCDOT response exceeds byte limit"));
    }
    String::from_utf8(bytes).map_err(ProbeFailure::other)
}

#[derive(Debug, Deserialize)]
struct ErrorEnvelope {
    error: Option<ArcGisError>,
}

#[derive(Debug, Deserialize)]
struct ArcGisError {
    code: u16,
    #[serde(default)]
    message: String,
    #[serde(default)]
    details: Vec<String>,
}

fn classify_arcgis_error(body: &str) -> Option<ProbeFailure> {
    let error = serde_json::from_str::<ErrorEnvelope>(body).ok()?.error?;
    if error.code == 429 {
        let detail = error
            .details
            .first()
            .map_or(error.message.as_str(), String::as_str);
        let seconds = detail
            .split_once("Retry after")
            .and_then(|(_, suffix)| {
                suffix
                    .trim_start()
                    .split_whitespace()
                    .next()
                    .and_then(|value| value.parse::<u64>().ok())
            })
            .unwrap_or(POLL.as_secs());
        Some(ProbeFailure::rate_limited(
            Duration::from_secs(seconds),
            format!("NCDOT ArcGIS rate limited the query: {detail}"),
        ))
    } else {
        Some(ProbeFailure::other(format!(
            "NCDOT ArcGIS error {}: {}",
            error.code, error.message
        )))
    }
}

#[derive(Debug, Deserialize)]
struct FeatureCollection {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    features: Vec<Feature>,
    #[serde(default, rename = "exceededTransferLimit")]
    exceeded_transfer_limit: bool,
}

#[derive(Debug, Deserialize)]
struct Feature {
    #[serde(default)]
    properties: Properties,
    geometry: Option<PointGeometry>,
}

#[derive(Debug, Default, Deserialize)]
struct Properties {
    #[serde(default, rename = "Id")]
    id: Option<i64>,
    #[serde(default, rename = "Road")]
    road: Option<String>,
    #[serde(default, rename = "Reason")]
    reason: Option<String>,
    #[serde(default, rename = "Condition")]
    condition: Option<String>,
    #[serde(default, rename = "EventName")]
    event_name: Option<String>,
    #[serde(default, rename = "LanesAffected")]
    lanes_affected: Option<String>,
    #[serde(default, rename = "EventType")]
    event_type: Option<String>,
    #[serde(default, rename = "EventSubType")]
    event_subtype: Option<String>,
    #[serde(default, rename = "StartDateTime")]
    starts_at_ms: Option<i64>,
    #[serde(default, rename = "EndDateTime")]
    ends_at_ms: Option<i64>,
    #[serde(default, rename = "LastUpdateDateTime")]
    updated_at_ms: Option<i64>,
    #[serde(default, rename = "IsFullClosure")]
    full_closure: Option<String>,
    #[serde(default, rename = "Direction")]
    direction: Option<String>,
    #[serde(default, rename = "CountyName")]
    county: Option<String>,
    #[serde(default, rename = "Location")]
    location: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PointGeometry {
    #[serde(rename = "type")]
    kind: String,
    coordinates: Vec<f64>,
}

fn parse_snapshot(
    host: &str,
    context: TrafficContext,
    body: &str,
    fetched_at_ms: i64,
) -> Result<TrafficSnapshot, ProbeFailure> {
    validate_context(context).map_err(ProbeFailure::other)?;
    if body.len() > MAX_BODY_BYTES {
        return Err(ProbeFailure::other("NCDOT response exceeds byte limit"));
    }
    if let Some(error) = classify_arcgis_error(body) {
        return Err(error);
    }
    let feed: FeatureCollection = serde_json::from_str(body).map_err(ProbeFailure::other)?;
    if feed.kind != "FeatureCollection" {
        return Err(ProbeFailure::other(
            "NCDOT payload is not a FeatureCollection",
        ));
    }
    let feed_count = feed.features.len();
    let mut snapshot = TrafficSnapshot::empty(
        host,
        fetched_at_ms,
        context.latitude,
        context.longitude,
        QUERY_RADIUS_KM,
    );
    if feed.exceeded_transfer_limit || feed_count > MAX_FEED_FEATURES {
        push_gap(
            &mut snapshot.gaps,
            "NCDOT transfer limit reached".to_string(),
        );
    }
    for (index, feature) in feed
        .features
        .into_iter()
        .take(MAX_FEED_FEATURES)
        .enumerate()
    {
        match parse_feature(feature, context, fetched_at_ms) {
            Ok(Some(event)) => snapshot.events.push(event),
            Ok(None) => snapshot.omitted_features = snapshot.omitted_features.saturating_add(1),
            Err(error) => {
                snapshot.omitted_features = snapshot.omitted_features.saturating_add(1);
                push_gap(
                    &mut snapshot.gaps,
                    format!("NCDOT feature {index} omitted: {error}"),
                );
            }
        }
    }
    snapshot.omitted_features = snapshot.omitted_features.saturating_add(
        u32::try_from(feed_count.saturating_sub(MAX_FEED_FEATURES)).unwrap_or(u32::MAX),
    );
    snapshot.events.sort_by(|a, b| {
        b.full_closure
            .cmp(&a.full_closure)
            .then_with(|| a.distance_km.total_cmp(&b.distance_km))
    });
    if snapshot.events.len() > MAX_RETAINED_EVENTS {
        let omitted = snapshot.events.len() - MAX_RETAINED_EVENTS;
        snapshot.events.truncate(MAX_RETAINED_EVENTS);
        snapshot.omitted_features = snapshot
            .omitted_features
            .saturating_add(u32::try_from(omitted).unwrap_or(u32::MAX));
        push_gap(
            &mut snapshot.gaps,
            format!("NCDOT nearby events capped at {MAX_RETAINED_EVENTS}"),
        );
    }
    Ok(snapshot)
}

fn parse_feature(
    feature: Feature,
    context: TrafficContext,
    fetched_at_ms: i64,
) -> io::Result<Option<TrafficEvent>> {
    let geometry = feature
        .geometry
        .filter(|geometry| geometry.kind == "Point")
        .ok_or_else(|| io::Error::other("missing point geometry"))?;
    let longitude = geometry.coordinates.first().copied().unwrap_or(f64::NAN);
    let latitude = geometry.coordinates.get(1).copied().unwrap_or(f64::NAN);
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
    let properties = feature.properties;
    let id = properties
        .id
        .filter(|id| *id > 0)
        .ok_or_else(|| io::Error::other("missing positive Id"))?
        .to_string();
    let road =
        bounded(properties.road.as_deref(), 160).ok_or_else(|| io::Error::other("invalid road"))?;
    let event_type = bounded(properties.event_type.as_deref(), 80)
        .ok_or_else(|| io::Error::other("invalid event type"))?;
    let event_subtype = bounded(properties.event_subtype.as_deref(), 80).unwrap_or_default();
    let summary = bounded(properties.location.as_deref(), MAX_STRING_BYTES)
        .or_else(|| bounded(properties.reason.as_deref(), MAX_STRING_BYTES))
        .or_else(|| bounded(properties.event_name.as_deref(), MAX_STRING_BYTES))
        .unwrap_or_else(|| format!("{event_type} on {road}"));
    let full_closure = match properties.full_closure.as_deref().map(str::trim) {
        Some(value) if value.eq_ignore_ascii_case("true") => true,
        Some(value) if value.eq_ignore_ascii_case("false") => false,
        None => false,
        Some(_) => return Err(io::Error::other("invalid full-closure value")),
    };
    let valid_time = |value: Option<i64>| {
        value.filter(|value| {
            *value >= 946_684_800_000
                && *value <= fetched_at_ms.saturating_add(10 * 365 * 24 * 60 * 60 * 1_000)
        })
    };
    Ok(Some(TrafficEvent {
        id,
        road,
        summary,
        event_type,
        event_subtype,
        condition: bounded(properties.condition.as_deref(), 160),
        lanes_affected: bounded(properties.lanes_affected.as_deref(), 160),
        direction: bounded(properties.direction.as_deref(), 160),
        county: bounded(properties.county.as_deref(), 160),
        full_closure,
        latitude,
        longitude,
        distance_km: distance_km as f32,
        starts_at_ms: valid_time(properties.starts_at_ms),
        ends_at_ms: valid_time(properties.ends_at_ms),
        updated_at_ms: valid_time(properties.updated_at_ms),
    }))
}

fn bounded(value: Option<&str>, max: usize) -> Option<String> {
    let value = value?.trim();
    (!value.is_empty()
        && value.len() <= max
        && value.chars().all(|character| !character.is_control()))
    .then(|| value.to_string())
}

fn validate_context(context: TrafficContext) -> io::Result<()> {
    if context.latitude.is_finite()
        && context.longitude.is_finite()
        && (33.7..=36.7).contains(&context.latitude)
        && (-84.5..=-75.2).contains(&context.longitude)
    {
        Ok(())
    } else {
        Err(io::Error::other(
            "fresh vehicle point is outside finite North Carolina coverage",
        ))
    }
}

fn points_near(a: TrafficContext, b: TrafficContext) -> bool {
    great_circle_km(a.latitude, a.longitude, b.latitude, b.longitude) <= 5.0
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

enum PreparedResponse {
    Modified(TrafficSnapshot),
}

#[derive(Debug, Clone, Copy)]
struct ApplyOutcome {
    success: bool,
    retry_after: Option<Duration>,
}

/// Workstation-side keyless NCDOT traffic adapter.
pub struct TrafficOverlayWorker {
    host: String,
    probe: Option<Arc<dyn TrafficProbe>>,
    bus_root: Option<PathBuf>,
}

impl TrafficOverlayWorker {
    /// Production wiring. Disabled unless explicitly opted in.
    #[must_use]
    pub fn new(host: String) -> Self {
        let probe = if env_truthy(ENABLED_ENV) {
            match NcdotHttpProbe::new() {
                Ok(probe) => Some(Arc::new(probe) as Arc<dyn TrafficProbe>),
                Err(error) => {
                    tracing::warn!(target: "mackesd::traffic_overlay", %error, "NCDOT client unavailable; worker idle");
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
        }
    }

    /// Inject a fixture probe.
    #[must_use]
    pub fn with_probe(mut self, probe: Arc<dyn TrafficProbe>) -> Self {
        self.probe = Some(probe);
        self
    }

    /// Override or disable Bus access.
    #[must_use]
    pub fn with_bus_root(mut self, root: Option<PathBuf>) -> Self {
        self.bus_root = root;
        self
    }

    fn current_context(&self) -> Option<TrafficContext> {
        let root = self.bus_root.clone()?;
        let persist = mde_bus::persist::Persist::open(root).ok()?;
        let topic = mackes_mesh_types::vehicle::vehicle_state_topic(&self.host);
        let body = persist.read_latest(&topic).ok().flatten()?.body?;
        let vehicle: mackes_mesh_types::vehicle::VehicleState = serde_json::from_str(&body).ok()?;
        validated_vehicle_context(&vehicle, &self.host, now_ms())
    }

    fn publish(&self, snapshot: &TrafficSnapshot) {
        if let Some(mut persist) = crate::bus_publish::open_bus(self.bus_root.clone()) {
            crate::bus_publish::publish_json(
                &mut persist,
                &traffic_state_topic(&self.host),
                snapshot,
            );
        }
    }

    fn apply_result(
        &self,
        result: Result<PreparedResponse, ProbeFailure>,
        last_good: &mut Option<TrafficSnapshot>,
    ) -> ApplyOutcome {
        match result {
            Ok(PreparedResponse::Modified(snapshot)) => {
                self.publish(&snapshot);
                *last_good = Some(snapshot);
                ApplyOutcome {
                    success: true,
                    retry_after: None,
                }
            }
            Err(error) => {
                if let Some(snapshot) = last_good {
                    snapshot
                        .gaps
                        .retain(|gap| !gap.starts_with("NCDOT traffic paused:"));
                    push_gap(&mut snapshot.gaps, format!("NCDOT traffic paused: {error}"));
                    self.publish(snapshot);
                }
                ApplyOutcome {
                    success: false,
                    retry_after: error.retry_after,
                }
            }
        }
    }

    async fn fetch_async(
        &self,
        probe: Arc<dyn TrafficProbe>,
        context: TrafficContext,
        previous: Option<TrafficSnapshot>,
        shutdown: &mut ShutdownToken,
    ) -> Option<Result<PreparedResponse, ProbeFailure>> {
        let host = self.host.clone();
        let task = tokio::task::spawn_blocking(move || {
            let fetched_at_ms = now_ms();
            let snapshot = match probe.fetch(context)? {
                ProbeResponse::Modified(body) => {
                    parse_snapshot(&host, context, &body, fetched_at_ms)?
                }
                ProbeResponse::NotModified => {
                    let mut snapshot = previous.ok_or_else(|| {
                        ProbeFailure::other("NCDOT returned 304 before a last-good snapshot")
                    })?;
                    if !points_near(
                        TrafficContext {
                            latitude: snapshot.query_latitude,
                            longitude: snapshot.query_longitude,
                        },
                        context,
                    ) {
                        return Err(ProbeFailure::other(
                            "NCDOT 304 query point does not match last-good",
                        ));
                    }
                    snapshot.fetched_at_ms = fetched_at_ms;
                    snapshot.query_latitude = context.latitude;
                    snapshot.query_longitude = context.longitude;
                    snapshot
                        .gaps
                        .retain(|gap| !gap.starts_with("NCDOT traffic paused:"));
                    snapshot
                }
            };
            Ok(PreparedResponse::Modified(snapshot))
        });
        tokio::select! {
            () = shutdown.wait() => None,
            joined = task => Some(match joined {
                Ok(result) => result,
                Err(error) => Err(ProbeFailure::other(format!("NCDOT fetch task failed: {error}"))),
            }),
        }
    }
}

fn validated_vehicle_context(
    vehicle: &mackes_mesh_types::vehicle::VehicleState,
    expected_host: &str,
    now: i64,
) -> Option<TrafficContext> {
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
    let context = TrafficContext {
        latitude: gps.latitude,
        longitude: gps.longitude,
    };
    validate_context(context).ok().map(|()| context)
}

#[async_trait::async_trait]
impl Worker for TrafficOverlayWorker {
    fn name(&self) -> &'static str {
        "traffic_overlay"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let Some(probe) = self.probe.clone() else {
            shutdown.wait().await;
            return Ok(());
        };
        let mut last_good = None;
        let mut retry = POLL;
        let mut no_fix_published = false;
        loop {
            let Some(context) = self.current_context() else {
                if !no_fix_published {
                    self.apply_result(
                        Err(ProbeFailure::other(
                            "fresh same-host North Carolina vehicle fix unavailable",
                        )),
                        &mut last_good,
                    );
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
                .fetch_async(probe.clone(), context, last_good.clone(), &mut shutdown)
                .await
            else {
                break;
            };
            let outcome = self.apply_result(result, &mut last_good);
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

    fn context() -> TrafficContext {
        TrafficContext {
            latitude: 35.70,
            longitude: -78.65,
        }
    }

    fn live_geojson() -> String {
        json!({"type":"FeatureCollection","features":[{
            "type":"Feature",
            "properties":{
                "OBJECTID":238929,"Id":2188564,"Road":"NC 55","Reason":null,
                "Condition":"Open","EventName":null,"LanesAffected":"Lane Affected",
                "EventType":"roadwork","EventSubType":"construction",
                "StartDateTime":1780330440000_i64,"EndDateTime":1830273300000_i64,
                "LastUpdateDateTime":1784760812000_i64,"IsFullClosure":"false",
                "Direction":"Both Directions","CountyName":"Wake",
                "Location":"Construction on NC 55 Both Directions in Wake County from Saunders Rd to Harnett Wake County Line. Lane Affected. Id: 2190"
            },
            "geometry":{"type":"Point","coordinates":[-78.7545313515643,35.557395945307]}
        }]}).to_string()
    }

    #[test]
    fn captured_live_ncdot_schema_normalizes_current_incident() {
        let snapshot =
            parse_snapshot("rig-1", context(), &live_geojson(), NOW_MS).expect("snapshot");
        assert_eq!(snapshot.events.len(), 1);
        let event = &snapshot.events[0];
        assert_eq!(event.id, "2188564");
        assert_eq!(event.road, "NC 55");
        assert_eq!(event.lanes_affected.as_deref(), Some("Lane Affected"));
        assert!(!event.full_closure);
        assert_eq!(event.updated_at_ms, Some(1_784_760_812_000));
    }

    #[test]
    fn endpoint_bbox_body_and_record_counts_are_bounded() {
        let url = query_url(context()).expect("query");
        assert_eq!(url.host_str(), Some("services.arcgis.com"));
        assert!(url.query().is_some_and(|query| query.contains("f=geojson")));
        for hostile in [
            DEFAULT_ENDPOINT.replace("https", "http"),
            DEFAULT_ENDPOINT.replace("services.arcgis.com", "services.arcgis.com.evil.test"),
            format!("{DEFAULT_ENDPOINT}?token=secret"),
        ] {
            assert!(validate_endpoint(&hostile).is_err(), "accepted {hostile}");
        }
        assert!(
            parse_snapshot("rig-1", context(), &"x".repeat(MAX_BODY_BYTES + 1), NOW_MS).is_err()
        );
    }

    #[test]
    fn vehicle_context_requires_fresh_same_host_north_carolina_fix() {
        let mut vehicle = VehicleState::offline("rig-1");
        vehicle.online = true;
        vehicle.published_at_ms = 100_000;
        vehicle.gps = GpsFix {
            fix_type: "gps".to_string(),
            latitude: 35.70,
            longitude: -78.65,
            satellites: 8,
            age_s: 1.0,
            ..GpsFix::default()
        };
        assert!(validated_vehicle_context(&vehicle, "rig-1", 110_000).is_some());
        assert!(validated_vehicle_context(&vehicle, "other", 110_000).is_none());
        vehicle.gps.latitude = 44.0;
        vehicle.gps.longitude = -120.0;
        assert!(validated_vehicle_context(&vehicle, "rig-1", 100_000).is_none());
    }

    #[test]
    fn arcgis_json_429_preserves_retry_after() {
        let body = r#"{"error":{"code":429,"message":"Too many requests","details":["Retry after 240 sec."]}}"#;
        let error = classify_arcgis_error(body).expect("classified");
        assert_eq!(error.retry_after, Some(Duration::from_secs(240)));
    }

    #[test]
    fn rate_limit_retains_fetch_time_and_publishes_paused_last_good() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().to_path_buf();
        let worker =
            TrafficOverlayWorker::new("rig-1".to_string()).with_bus_root(Some(root.clone()));
        let original =
            parse_snapshot("rig-1", context(), &live_geojson(), NOW_MS).expect("snapshot");
        let mut last_good = None;
        assert!(
            worker
                .apply_result(Ok(PreparedResponse::Modified(original)), &mut last_good)
                .success
        );
        let paused = worker.apply_result(
            Err(ProbeFailure::rate_limited(Duration::from_secs(60), "429")),
            &mut last_good,
        );
        assert!(!paused.success);
        let retained = last_good.as_ref().expect("last good");
        assert_eq!(retained.fetched_at_ms, NOW_MS);
        assert!(retained
            .gaps
            .iter()
            .any(|gap| gap.starts_with("NCDOT traffic paused:")));
        assert_eq!(
            Persist::open(root)
                .expect("bus")
                .list_since(&traffic_state_topic("rig-1"), None)
                .expect("rows")
                .len(),
            2
        );
    }

    struct SlowProbe;

    impl TrafficProbe for SlowProbe {
        fn fetch(&self, _context: TrafficContext) -> Result<ProbeResponse, ProbeFailure> {
            std::thread::sleep(Duration::from_millis(500));
            Ok(ProbeResponse::Modified(live_geojson()))
        }
    }

    #[tokio::test]
    async fn shutdown_wins_while_query_is_in_flight() {
        let worker = TrafficOverlayWorker::new("rig-1".to_string()).with_bus_root(None);
        let (tx, rx) = tokio::sync::watch::channel(false);
        let mut shutdown = ShutdownToken::from_receiver(rx);
        let sender = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            tx.send(true).expect("shutdown");
        });
        let result = tokio::time::timeout(
            Duration::from_millis(200),
            worker.fetch_async(Arc::new(SlowProbe), context(), None, &mut shutdown),
        )
        .await
        .expect("responsive");
        assert!(result.is_none());
        sender.await.expect("sender");
    }
}
