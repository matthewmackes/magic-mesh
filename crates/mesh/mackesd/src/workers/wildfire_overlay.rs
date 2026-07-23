//! WL-FUNC-012 / OVERLAY-6 — keyless NIFC WFIGS current wildfire perimeters.
//!
//! A Workstation opts in with `MDE_OVERLAY_NIFC_WILDFIRE=1`. The adapter makes
//! a bounded, vehicle-centred query against the official WFIGS current-perimeter
//! FeatureServer every fifteen minutes, normalizes Polygon/MultiPolygon GeoJSON,
//! and publishes a complete latest-wins Bus snapshot. NASA FIRMS hotspots remain
//! a separate optional free-key input and are never fabricated when unconfigured.

#![cfg(feature = "async-services")]

use std::io::{self, Read};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mackes_mesh_types::wildfire::{
    wildfire_state_topic, WildfirePerimeter, WildfirePoint, WildfirePolygon, WildfireSnapshot,
};
use reqwest::blocking::Client;
use reqwest::header::{
    CONTENT_TYPE, ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED, RETRY_AFTER,
};
use serde::Deserialize;
use serde_json::Value;

use super::{ShutdownToken, Worker};

/// Explicit opt-in. Unset/false is an idle no-op.
pub const ENABLED_ENV: &str = "MDE_OVERLAY_NIFC_WILDFIRE";
/// Optional official-service URL override for a renamed WFIGS current layer.
pub const ENDPOINT_ENV: &str = "MDE_OVERLAY_NIFC_WILDFIRE_URL";
/// Official keyless WFIGS current-perimeters query endpoint.
pub const DEFAULT_ENDPOINT: &str = "https://services3.arcgis.com/T4QMspbfLg3qTGWY/arcgis/rest/services/WFIGS_Interagency_Perimeters_Current/FeatureServer/0/query";
/// Producer-appropriate perimeter refresh cadence.
pub const POLL: Duration = Duration::from_secs(15 * 60);

const RETRY_MIN: Duration = Duration::from_secs(60);
const RETRY_MAX: Duration = Duration::from_secs(15 * 60);
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const QUERY_RADIUS_KM: u16 = 200;
const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;
const MAX_FEED_FEATURES: usize = 2_000;
const MAX_RETAINED_PERIMETERS: usize = 256;
const MAX_POLYGONS_PER_FEATURE: usize = 64;
const MAX_RINGS_PER_POLYGON: usize = 32;
const MAX_POINTS_PER_RING: usize = 4_096;
const MAX_TOTAL_POINTS: usize = 100_000;
const MAX_STRING_BYTES: usize = 160;
const MAX_GAPS: usize = 128;
const VEHICLE_FIX_MAX_AGE_MS: i64 = 30_000;
const VEHICLE_MAX_FUTURE_SKEW_MS: i64 = 5_000;
const USER_AGENT: &str =
    "Construct/12 mackesd NIFC-WFIGS-overlay (+https://github.com/matthewmackes/magic-mesh)";

/// Fresh finite vehicle point used for the server-side envelope.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WildfireContext {
    /// WGS-84 latitude.
    pub latitude: f64,
    /// WGS-84 longitude.
    pub longitude: f64,
}

/// Conditional WFIGS query response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeResponse {
    /// HTTP 200 with bounded GeoJSON.
    Modified(String),
    /// Validator-backed HTTP 304.
    NotModified,
}

/// Fetch-failure class used to preserve ArcGIS rate-limit timing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeFailureKind {
    /// ArcGIS HTTP or JSON error code 429 with a bounded retry delay.
    RateLimited(Duration),
    /// Any other transport, schema, or service failure.
    Other,
}

/// One operator-safe WFIGS probe error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeFailure {
    kind: ProbeFailureKind,
    message: String,
}

impl ProbeFailure {
    fn other(message: impl std::fmt::Display) -> Self {
        Self {
            kind: ProbeFailureKind::Other,
            message: message.to_string(),
        }
    }

    fn rate_limited(delay: Duration, message: impl std::fmt::Display) -> Self {
        Self {
            kind: ProbeFailureKind::RateLimited(delay.clamp(RETRY_MIN, RETRY_MAX)),
            message: message.to_string(),
        }
    }

    fn retry_after(&self) -> Option<Duration> {
        match self.kind {
            ProbeFailureKind::RateLimited(delay) => Some(delay),
            ProbeFailureKind::Other => None,
        }
    }
}

impl std::fmt::Display for ProbeFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ProbeFailure {}

/// Injectable WFIGS query seam. Tests use captured GeoJSON and never contact
/// the internet.
pub trait WildfireProbe: Send + Sync {
    /// Query current wildfire perimeters around one validated point.
    fn fetch(&self, context: WildfireContext) -> Result<ProbeResponse, ProbeFailure>;
}

#[derive(Debug, Default)]
struct Validators {
    context: Option<WildfireContext>,
    etag: Option<String>,
    last_modified: Option<String>,
}

/// Production rustls client for the official ArcGIS service.
pub struct WfigsHttpProbe {
    client: Client,
    endpoint: String,
    validators: Mutex<Validators>,
}

impl WfigsHttpProbe {
    fn new(endpoint: String) -> Result<Self, ProbeFailure> {
        validate_endpoint(&endpoint).map_err(ProbeFailure::other)?;
        let client = Client::builder()
            .timeout(HTTP_TIMEOUT)
            .user_agent(USER_AGENT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(ProbeFailure::other)?;
        Ok(Self {
            client,
            endpoint,
            validators: Mutex::new(Validators::default()),
        })
    }
}

impl WildfireProbe for WfigsHttpProbe {
    fn fetch(&self, context: WildfireContext) -> Result<ProbeResponse, ProbeFailure> {
        validate_context(context).map_err(ProbeFailure::other)?;
        let url = query_url(&self.endpoint, context).map_err(ProbeFailure::other)?;
        let mut request = self.client.get(url);
        let mut sent_validator = false;
        {
            let validators = self
                .validators
                .lock()
                .map_err(|_| ProbeFailure::other("WFIGS validator lock poisoned"))?;
            if validators
                .context
                .is_some_and(|previous| points_near(previous, context))
            {
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

        let response = request.send().map_err(ProbeFailure::other)?;
        if response.status() == reqwest::StatusCode::NOT_MODIFIED {
            return if sent_validator {
                Ok(ProbeResponse::NotModified)
            } else {
                Err(ProbeFailure::other(
                    "WFIGS returned 304 although no matching-point validator was sent",
                ))
            };
        }
        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let delay = retry_after_header(&response).unwrap_or(RETRY_MIN);
            return Err(ProbeFailure::rate_limited(
                delay,
                "WFIGS rate limited the query (HTTP 429)",
            ));
        }
        if response.status() != reqwest::StatusCode::OK {
            return Err(ProbeFailure::other(format!(
                "WFIGS returned unexpected HTTP {} (redirects are disabled)",
                response.status()
            )));
        }
        require_json_type(&response)?;
        let etag = header_string(&response, ETAG);
        let last_modified = header_string(&response, LAST_MODIFIED);
        let mut response = response;
        let body = read_bounded_string(&mut response, MAX_BODY_BYTES)?;
        if let Some(error) = classify_arcgis_error(&body) {
            return Err(error);
        }
        *self
            .validators
            .lock()
            .map_err(|_| ProbeFailure::other("WFIGS validator lock poisoned"))? = Validators {
            context: Some(context),
            etag,
            last_modified,
        };
        Ok(ProbeResponse::Modified(body))
    }
}

fn validate_endpoint(value: &str) -> io::Result<reqwest::Url> {
    let url = reqwest::Url::parse(value).map_err(io_other)?;
    if url.scheme() != "https"
        || url.host_str() != Some("services3.arcgis.com")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path().contains("..")
    {
        return Err(io::Error::other(
            "WFIGS endpoint is outside the strict official-host allowlist",
        ));
    }
    let segments: Vec<_> = url
        .path_segments()
        .ok_or_else(|| io::Error::other("WFIGS endpoint has no path"))?
        .collect();
    if segments.len() != 8
        || segments[0] != "T4QMspbfLg3qTGWY"
        || !segments[1].eq_ignore_ascii_case("arcgis")
        || segments[2] != "rest"
        || segments[3] != "services"
        || !segments[4].starts_with("WFIGS_Interagency_Perimeters_")
        || segments[5] != "FeatureServer"
        || segments[6] != "0"
        || segments[7] != "query"
    {
        return Err(io::Error::other("WFIGS endpoint path is not canonical"));
    }
    Ok(url)
}

fn query_url(endpoint: &str, context: WildfireContext) -> io::Result<reqwest::Url> {
    let mut url = validate_endpoint(endpoint)?;
    validate_context(context)?;
    let (west, south, east, north) = query_envelope(context)?;
    let geometry = format!("{west:.6},{south:.6},{east:.6},{north:.6}");
    url.query_pairs_mut()
        .append_pair("where", "attr_IncidentTypeCategory='WF'")
        .append_pair("geometry", &geometry)
        .append_pair("geometryType", "esriGeometryEnvelope")
        .append_pair("inSR", "4326")
        .append_pair("spatialRel", "esriSpatialRelIntersects")
        .append_pair("outSR", "4326")
        .append_pair(
            "outFields",
            "OBJECTID,poly_IncidentName,poly_GISAcres,poly_DateCurrent,poly_PolygonDateTime,attr_PercentContained,attr_IncidentTypeCategory,attr_UniqueFireIdentifier",
        )
        .append_pair("returnGeometry", "true")
        .append_pair("returnZ", "false")
        .append_pair("returnM", "false")
        .append_pair("maxAllowableOffset", "0.001")
        .append_pair("resultRecordCount", &MAX_FEED_FEATURES.to_string())
        .append_pair("orderByFields", "poly_PolygonDateTime DESC")
        .append_pair("f", "geojson");
    Ok(url)
}

fn query_envelope(context: WildfireContext) -> io::Result<(f64, f64, f64, f64)> {
    validate_context(context)?;
    let latitude_delta = f64::from(QUERY_RADIUS_KM) / 111.32;
    let longitude_scale = context.latitude.to_radians().cos().abs().max(0.10);
    let longitude_delta = f64::from(QUERY_RADIUS_KM) / (111.32 * longitude_scale);
    Ok((
        (context.longitude - longitude_delta).max(-180.0),
        (context.latitude - latitude_delta).max(-90.0),
        (context.longitude + longitude_delta).min(180.0),
        (context.latitude + latitude_delta).min(90.0),
    ))
}

fn require_json_type(response: &reqwest::blocking::Response) -> Result<(), ProbeFailure> {
    if response
        .content_length()
        .is_some_and(|length| length > u64::try_from(MAX_BODY_BYTES).unwrap_or(u64::MAX))
    {
        return Err(ProbeFailure::other(format!(
            "WFIGS response exceeds {MAX_BODY_BYTES} byte limit"
        )));
    }
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim();
    if !matches!(content_type, "application/json" | "application/geo+json") {
        return Err(ProbeFailure::other(format!(
            "WFIGS returned unexpected content type {content_type:?}"
        )));
    }
    Ok(())
}

fn retry_after_header(response: &reqwest::blocking::Response) -> Option<Duration> {
    response
        .headers()
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
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

fn read_bounded_string(reader: &mut impl Read, max_bytes: usize) -> Result<String, ProbeFailure> {
    let mut bytes = Vec::with_capacity(max_bytes.min(64 * 1024));
    reader
        .take(
            u64::try_from(max_bytes)
                .unwrap_or(u64::MAX)
                .saturating_add(1),
        )
        .read_to_end(&mut bytes)
        .map_err(ProbeFailure::other)?;
    if bytes.len() > max_bytes {
        return Err(ProbeFailure::other(format!(
            "WFIGS response exceeds {max_bytes} byte limit"
        )));
    }
    String::from_utf8(bytes).map_err(ProbeFailure::other)
}

#[derive(Debug, Deserialize)]
struct ArcGisErrorEnvelope {
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
    let envelope: ArcGisErrorEnvelope = serde_json::from_str(body).ok()?;
    let error = envelope.error?;
    let detail = error
        .details
        .first()
        .map(String::as_str)
        .unwrap_or(error.message.as_str());
    if error.code == 429 {
        let seconds = seconds_after(detail, "Retry after").unwrap_or(RETRY_MIN.as_secs());
        Some(ProbeFailure::rate_limited(
            Duration::from_secs(seconds),
            format!("WFIGS rate limited the query: {detail}"),
        ))
    } else {
        Some(ProbeFailure::other(format!(
            "WFIGS ArcGIS error {}: {}",
            error.code, error.message
        )))
    }
}

fn seconds_after(value: &str, marker: &str) -> Option<u64> {
    let suffix = value.split_once(marker)?.1.trim_start();
    let digits: String = suffix.chars().take_while(char::is_ascii_digit).collect();
    (!digits.is_empty()).then(|| digits.parse().ok()).flatten()
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
    geometry: Option<Geometry>,
}

#[derive(Debug, Default, Deserialize)]
struct Properties {
    #[serde(default, rename = "OBJECTID")]
    object_id: Option<i64>,
    #[serde(default, rename = "poly_IncidentName")]
    incident_name: Option<String>,
    #[serde(default, rename = "poly_GISAcres")]
    acres: Option<f64>,
    #[serde(default, rename = "poly_DateCurrent")]
    date_current: Option<Value>,
    #[serde(default, rename = "poly_PolygonDateTime")]
    polygon_date_time: Option<Value>,
    #[serde(default, rename = "attr_PercentContained")]
    percent_contained: Option<f64>,
    #[serde(default, rename = "attr_IncidentTypeCategory")]
    incident_type: Option<String>,
    #[serde(default, rename = "attr_UniqueFireIdentifier")]
    unique_fire_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Geometry {
    #[serde(rename = "type")]
    kind: String,
    coordinates: Value,
}

fn parse_snapshot(
    host: &str,
    context: WildfireContext,
    body: &str,
    fetched_at_ms: i64,
) -> Result<WildfireSnapshot, ProbeFailure> {
    validate_context(context).map_err(ProbeFailure::other)?;
    if body.len() > MAX_BODY_BYTES {
        return Err(ProbeFailure::other("WFIGS response exceeds byte limit"));
    }
    if let Some(error) = classify_arcgis_error(body) {
        return Err(error);
    }
    let collection: FeatureCollection = serde_json::from_str(body).map_err(ProbeFailure::other)?;
    if collection.kind != "FeatureCollection" {
        return Err(ProbeFailure::other(
            "WFIGS payload is not a FeatureCollection",
        ));
    }
    let feature_count = collection.features.len();
    let mut snapshot = WildfireSnapshot::empty(
        host,
        fetched_at_ms,
        context.latitude,
        context.longitude,
        QUERY_RADIUS_KM,
    );
    if collection.exceeded_transfer_limit || feature_count > MAX_FEED_FEATURES {
        push_gap(
            &mut snapshot.gaps,
            "WFIGS transfer limit reached; the local perimeter set is incomplete".to_string(),
        );
    }
    let mut point_budget = MAX_TOTAL_POINTS;
    for (index, feature) in collection
        .features
        .into_iter()
        .take(MAX_FEED_FEATURES)
        .enumerate()
    {
        match parse_feature(feature, fetched_at_ms, &mut point_budget) {
            Ok(perimeter) => snapshot.perimeters.push(perimeter),
            Err(error) => {
                snapshot.omitted_features = snapshot.omitted_features.saturating_add(1);
                push_gap(
                    &mut snapshot.gaps,
                    format!("WFIGS feature {index} omitted: {error}"),
                );
            }
        }
    }
    snapshot.omitted_features = snapshot.omitted_features.saturating_add(
        u32::try_from(feature_count.saturating_sub(MAX_FEED_FEATURES)).unwrap_or(u32::MAX),
    );
    snapshot.perimeters.sort_by(|a, b| {
        b.perimeter_updated_at_ms
            .cmp(&a.perimeter_updated_at_ms)
            .then_with(|| b.acres.unwrap_or(0.0).total_cmp(&a.acres.unwrap_or(0.0)))
    });
    if snapshot.perimeters.len() > MAX_RETAINED_PERIMETERS {
        let omitted = snapshot.perimeters.len() - MAX_RETAINED_PERIMETERS;
        snapshot.perimeters.truncate(MAX_RETAINED_PERIMETERS);
        snapshot.omitted_features = snapshot
            .omitted_features
            .saturating_add(u32::try_from(omitted).unwrap_or(u32::MAX));
        push_gap(
            &mut snapshot.gaps,
            format!("WFIGS nearby perimeters capped at {MAX_RETAINED_PERIMETERS}"),
        );
    }
    Ok(snapshot)
}

fn parse_feature(
    feature: Feature,
    fetched_at_ms: i64,
    point_budget: &mut usize,
) -> io::Result<WildfirePerimeter> {
    let properties = feature.properties;
    if properties.incident_type.as_deref() != Some("WF") {
        return Err(io::Error::other("incident category is not wildfire"));
    }
    let object_id = properties
        .object_id
        .filter(|id| *id > 0)
        .ok_or_else(|| io::Error::other("missing positive OBJECTID"))?;
    let incident_name = bounded_text(properties.incident_name.as_deref(), MAX_STRING_BYTES)
        .ok_or_else(|| io::Error::other("invalid incident name"))?;
    let unique_fire_id = bounded_text(properties.unique_fire_id.as_deref(), MAX_STRING_BYTES);
    let acres = properties
        .acres
        .filter(|value| value.is_finite() && (0.0..=1_000_000_000.0).contains(value));
    let percent_contained = properties
        .percent_contained
        .filter(|value| value.is_finite() && (0.0..=100.0).contains(value))
        .map(|value| value as f32);
    let perimeter_updated_at_ms = properties
        .polygon_date_time
        .as_ref()
        .and_then(parse_date_value)
        .or_else(|| properties.date_current.as_ref().and_then(parse_date_value))
        .filter(|value| {
            *value >= 946_684_800_000
                && *value <= fetched_at_ms.saturating_add(24 * 60 * 60 * 1_000)
        });
    let geometry = feature
        .geometry
        .ok_or_else(|| io::Error::other("missing geometry"))?;
    let polygons = parse_geometry(&geometry, point_budget)?;
    if polygons.is_empty() {
        return Err(io::Error::other("geometry contains no valid polygon"));
    }
    Ok(WildfirePerimeter {
        id: object_id.to_string(),
        incident_name,
        unique_fire_id,
        acres,
        percent_contained,
        perimeter_updated_at_ms,
        polygons,
    })
}

fn parse_date_value(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number.as_i64(),
        Value::String(value) => value.parse::<i64>().ok().or_else(|| {
            chrono::DateTime::parse_from_rfc3339(value)
                .ok()
                .map(|time| time.timestamp_millis())
        }),
        _ => None,
    }
}

fn parse_geometry(
    geometry: &Geometry,
    point_budget: &mut usize,
) -> io::Result<Vec<WildfirePolygon>> {
    match geometry.kind.as_str() {
        "Polygon" => Ok(vec![parse_polygon(&geometry.coordinates, point_budget)?]),
        "MultiPolygon" => {
            let polygons = geometry
                .coordinates
                .as_array()
                .ok_or_else(|| io::Error::other("MultiPolygon coordinates are not an array"))?;
            if polygons.len() > MAX_POLYGONS_PER_FEATURE {
                return Err(io::Error::other("MultiPolygon exceeds polygon cap"));
            }
            polygons
                .iter()
                .map(|polygon| parse_polygon(polygon, point_budget))
                .collect()
        }
        _ => Err(io::Error::other("geometry is not Polygon/MultiPolygon")),
    }
}

fn parse_polygon(value: &Value, point_budget: &mut usize) -> io::Result<WildfirePolygon> {
    let rings = value
        .as_array()
        .ok_or_else(|| io::Error::other("polygon rings are not an array"))?;
    if rings.is_empty() || rings.len() > MAX_RINGS_PER_POLYGON {
        return Err(io::Error::other("polygon has invalid ring count"));
    }
    let rings = rings
        .iter()
        .map(|ring| parse_ring(ring, point_budget))
        .collect::<io::Result<Vec<_>>>()?;
    Ok(WildfirePolygon { rings })
}

fn parse_ring(value: &Value, point_budget: &mut usize) -> io::Result<Vec<WildfirePoint>> {
    let points = value
        .as_array()
        .ok_or_else(|| io::Error::other("ring is not an array"))?;
    if points.len() < 4 || points.len() > MAX_POINTS_PER_RING || points.len() > *point_budget {
        return Err(io::Error::other("ring point count exceeds bounds"));
    }
    let mut normalized = Vec::with_capacity(points.len());
    for coordinate in points {
        let coordinate = coordinate
            .as_array()
            .ok_or_else(|| io::Error::other("coordinate is not an array"))?;
        let longitude = coordinate.first().and_then(Value::as_f64);
        let latitude = coordinate.get(1).and_then(Value::as_f64);
        let (Some(latitude), Some(longitude)) = (latitude, longitude) else {
            return Err(io::Error::other("coordinate is not numeric"));
        };
        if !latitude.is_finite()
            || !longitude.is_finite()
            || !(-90.0..=90.0).contains(&latitude)
            || !(-180.0..=180.0).contains(&longitude)
        {
            return Err(io::Error::other("coordinate is not finite/in range"));
        }
        normalized.push(WildfirePoint {
            latitude,
            longitude,
        });
    }
    *point_budget -= normalized.len();
    Ok(normalized)
}

fn bounded_text(value: Option<&str>, max_bytes: usize) -> Option<String> {
    let value = value?.trim();
    (!value.is_empty()
        && value.len() <= max_bytes
        && value.chars().all(|character| !character.is_control()))
    .then(|| value.to_string())
}

fn validate_context(context: WildfireContext) -> io::Result<()> {
    if context.latitude.is_finite()
        && context.longitude.is_finite()
        && (17.0..=72.0).contains(&context.latitude)
        && (-180.0..=-60.0).contains(&context.longitude)
    {
        Ok(())
    } else {
        Err(io::Error::other(
            "fresh vehicle point is outside finite United States coverage",
        ))
    }
}

fn points_near(a: WildfireContext, b: WildfireContext) -> bool {
    great_circle_km(a.latitude, a.longitude, b.latitude, b.longitude) <= 10.0
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
    } else if gaps
        .last()
        .is_some_and(|last| last != "additional WFIGS gaps omitted")
    {
        gaps[MAX_GAPS - 1] = "additional WFIGS gaps omitted".to_string();
    }
}

fn io_other(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

enum PreparedResponse {
    Modified(WildfireSnapshot),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ApplyOutcome {
    success: bool,
    retry_after: Option<Duration>,
}

/// Workstation-side keyless WFIGS perimeter adapter.
pub struct WildfireOverlayWorker {
    host: String,
    probe: Option<Arc<dyn WildfireProbe>>,
    bus_root: Option<PathBuf>,
    poll: Duration,
}

impl WildfireOverlayWorker {
    /// Production wiring. Disabled unless explicitly opted in.
    #[must_use]
    pub fn new(host: String) -> Self {
        let probe = if env_truthy(ENABLED_ENV) {
            let endpoint =
                std::env::var(ENDPOINT_ENV).unwrap_or_else(|_| DEFAULT_ENDPOINT.to_string());
            match WfigsHttpProbe::new(endpoint) {
                Ok(probe) => Some(Arc::new(probe) as Arc<dyn WildfireProbe>),
                Err(error) => {
                    tracing::warn!(target: "mackesd::wildfire_overlay", %error, "WFIGS client unavailable; worker idle");
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

    /// Inject a fixture probe.
    #[must_use]
    pub fn with_probe(mut self, probe: Arc<dyn WildfireProbe>) -> Self {
        self.probe = Some(probe);
        self
    }

    /// Override or disable Bus access.
    #[must_use]
    pub fn with_bus_root(mut self, root: Option<PathBuf>) -> Self {
        self.bus_root = root;
        self
    }

    fn current_context(&self) -> Option<WildfireContext> {
        let root = self.bus_root.clone()?;
        let persist = mde_bus::persist::Persist::open(root).ok()?;
        let topic = mackes_mesh_types::vehicle::vehicle_state_topic(&self.host);
        let body = persist.read_latest(&topic).ok().flatten()?.body?;
        let vehicle: mackes_mesh_types::vehicle::VehicleState = serde_json::from_str(&body).ok()?;
        validated_vehicle_context(&vehicle, &self.host, now_ms())
    }

    fn publish(&self, snapshot: &WildfireSnapshot) {
        if let Some(mut persist) = crate::bus_publish::open_bus(self.bus_root.clone()) {
            crate::bus_publish::publish_json(
                &mut persist,
                &wildfire_state_topic(&self.host),
                snapshot,
            );
        }
    }

    fn apply_result(
        &self,
        result: Result<PreparedResponse, ProbeFailure>,
        last_good: &mut Option<WildfireSnapshot>,
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
                        .retain(|gap| !gap.starts_with("NIFC wildfire paused:"));
                    push_gap(&mut snapshot.gaps, format!("NIFC wildfire paused: {error}"));
                    self.publish(snapshot);
                }
                ApplyOutcome {
                    success: false,
                    retry_after: error.retry_after(),
                }
            }
        }
    }

    async fn fetch_async(
        &self,
        probe: Arc<dyn WildfireProbe>,
        context: WildfireContext,
        previous: Option<WildfireSnapshot>,
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
                        ProbeFailure::other("WFIGS returned 304 before a last-good snapshot")
                    })?;
                    if !points_near(
                        WildfireContext {
                            latitude: snapshot.query_latitude,
                            longitude: snapshot.query_longitude,
                        },
                        context,
                    ) {
                        return Err(ProbeFailure::other(
                            "WFIGS 304 query point does not match last-good",
                        ));
                    }
                    snapshot.fetched_at_ms = fetched_at_ms;
                    snapshot.query_latitude = context.latitude;
                    snapshot.query_longitude = context.longitude;
                    snapshot
                        .gaps
                        .retain(|gap| !gap.starts_with("NIFC wildfire paused:"));
                    snapshot
                }
            };
            Ok(PreparedResponse::Modified(snapshot))
        });
        tokio::select! {
            () = shutdown.wait() => None,
            joined = task => Some(match joined {
                Ok(result) => result,
                Err(error) => Err(ProbeFailure::other(format!("WFIGS fetch task failed: {error}"))),
            }),
        }
    }
}

fn validated_vehicle_context(
    vehicle: &mackes_mesh_types::vehicle::VehicleState,
    expected_host: &str,
    now: i64,
) -> Option<WildfireContext> {
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
    let context = WildfireContext {
        latitude: gps.latitude,
        longitude: gps.longitude,
    };
    validate_context(context).ok().map(|()| context)
}

fn effective_retry(outcome: ApplyOutcome, exponential: Duration, poll: Duration) -> Duration {
    if outcome.success {
        poll
    } else {
        outcome
            .retry_after
            .unwrap_or(exponential)
            .max(RETRY_MIN)
            .min(RETRY_MAX)
    }
}

#[async_trait::async_trait]
impl Worker for WildfireOverlayWorker {
    fn name(&self) -> &'static str {
        "wildfire_overlay"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let Some(probe) = self.probe.clone() else {
            shutdown.wait().await;
            return Ok(());
        };
        let mut last_good = None;
        let mut retry = RETRY_MIN;
        let mut no_fix_published = false;
        loop {
            let Some(context) = self.current_context() else {
                if !no_fix_published {
                    self.apply_result(
                        Err(ProbeFailure::other(
                            "fresh same-host US vehicle fix unavailable",
                        )),
                        &mut last_good,
                    );
                    no_fix_published = true;
                }
                tokio::select! {
                    () = shutdown.wait() => break,
                    () = tokio::time::sleep(self.poll) => {}
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
            let delay = effective_retry(outcome, retry, self.poll);
            retry = if outcome.success {
                RETRY_MIN
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
    use serde_json::json;

    use super::*;

    const NOW_MS: i64 = 1_784_757_600_000;

    fn context() -> WildfireContext {
        WildfireContext {
            latitude: 44.0,
            longitude: -120.0,
        }
    }

    fn captured_geojson() -> String {
        json!({
            "type":"FeatureCollection",
            "features":[
                {
                    "type":"Feature",
                    "properties":{
                        "OBJECTID":42,
                        "poly_IncidentName":"Morrill",
                        "poly_GISAcres":642029.0,
                        "poly_DateCurrent":NOW_MS - 60_000,
                        "poly_PolygonDateTime":NOW_MS - 120_000,
                        "attr_PercentContained":100.0,
                        "attr_IncidentTypeCategory":"WF",
                        "attr_UniqueFireIdentifier":"2026-NE-NESUP-000123"
                    },
                    "geometry":{
                        "type":"MultiPolygon",
                        "coordinates":[
                            [[[-120.1,43.9],[-119.9,43.9],[-119.9,44.1],[-120.1,43.9]]],
                            [[[-120.2,44.0],[-120.15,44.0],[-120.15,44.05],[-120.2,44.0]]]
                        ]
                    }
                },
                {
                    "type":"Feature",
                    "properties":{
                        "OBJECTID":43,
                        "poly_IncidentName":"Prescribed",
                        "attr_IncidentTypeCategory":"RX"
                    },
                    "geometry":{"type":"Polygon","coordinates":[]}
                }
            ]
        })
        .to_string()
    }

    #[test]
    fn captured_live_schema_normalizes_multipolygon_and_omits_non_wildfire() {
        let snapshot =
            parse_snapshot("rig-1", context(), &captured_geojson(), NOW_MS).expect("snapshot");
        assert_eq!(snapshot.perimeters.len(), 1);
        assert_eq!(snapshot.omitted_features, 1);
        assert_eq!(snapshot.perimeters[0].incident_name, "Morrill");
        assert_eq!(snapshot.perimeters[0].polygons.len(), 2);
        assert_eq!(snapshot.perimeters[0].percent_contained, Some(100.0));
        assert_eq!(snapshot.license_tier, "public-domain");
    }

    #[test]
    fn endpoint_query_geometry_and_payload_bounds_are_strict() {
        let url = query_url(DEFAULT_ENDPOINT, context()).expect("query");
        assert_eq!(url.host_str(), Some("services3.arcgis.com"));
        assert!(url.query().is_some_and(|query| query.contains("f=geojson")));
        let (west, south, east, north) = query_envelope(context()).expect("envelope");
        assert!(west < context().longitude && east > context().longitude);
        assert!(south < context().latitude && north > context().latitude);
        for hostile in [
            "http://services3.arcgis.com/T4QMspbfLg3qTGWY/arcgis/rest/services/WFIGS_Interagency_Perimeters_Current/FeatureServer/0/query",
            "https://services3.arcgis.com.evil.test/T4QMspbfLg3qTGWY/arcgis/rest/services/WFIGS_Interagency_Perimeters_Current/FeatureServer/0/query",
            "https://services3.arcgis.com:444/T4QMspbfLg3qTGWY/arcgis/rest/services/WFIGS_Interagency_Perimeters_Current/FeatureServer/0/query",
            "https://services3.arcgis.com/T4QMspbfLg3qTGWY/arcgis/rest/services/other/FeatureServer/0/query",
        ] {
            assert!(validate_endpoint(hostile).is_err(), "accepted {hostile}");
        }
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
            latitude: 44.0,
            longitude: -120.0,
            satellites: 8,
            age_s: 1.0,
            ..GpsFix::default()
        };
        assert!(validated_vehicle_context(&vehicle, "rig-1", 110_000).is_some());
        assert!(validated_vehicle_context(&vehicle, "other", 110_000).is_none());
        vehicle.gps.latitude = f64::NAN;
        assert!(validated_vehicle_context(&vehicle, "rig-1", 100_000).is_none());
        vehicle.gps.latitude = 0.0;
        vehicle.gps.longitude = 0.0;
        assert!(validated_vehicle_context(&vehicle, "rig-1", 100_000).is_none());
    }

    #[test]
    fn arcgis_json_429_preserves_retry_after_and_exponential_floor() {
        let body = r#"{"error":{"code":429,"message":"Unable to perform query. Too many requests.","details":["API calls quota exceeded. Retry after 240 sec."]}}"#;
        let error = classify_arcgis_error(body).expect("classified");
        assert_eq!(error.retry_after(), Some(Duration::from_secs(240)));
        let outcome = ApplyOutcome {
            success: false,
            retry_after: error.retry_after(),
        };
        assert_eq!(
            effective_retry(outcome, Duration::from_secs(60), POLL),
            Duration::from_secs(240)
        );
        let oversized = ProbeFailure::rate_limited(Duration::from_secs(3_600), "429");
        assert_eq!(oversized.retry_after(), Some(RETRY_MAX));
    }

    #[test]
    fn rate_limit_retains_original_fetch_time_and_publishes_paused_last_good() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().to_path_buf();
        let worker =
            WildfireOverlayWorker::new("rig-1".to_string()).with_bus_root(Some(root.clone()));
        let original =
            parse_snapshot("rig-1", context(), &captured_geojson(), NOW_MS).expect("snapshot");
        let mut last_good = None;
        let success = worker.apply_result(Ok(PreparedResponse::Modified(original)), &mut last_good);
        assert!(success.success);
        let paused = worker.apply_result(
            Err(ProbeFailure::rate_limited(
                Duration::from_secs(60),
                "ArcGIS HTTP 429",
            )),
            &mut last_good,
        );
        assert!(!paused.success);
        let retained = last_good.as_ref().expect("last good");
        assert_eq!(retained.fetched_at_ms, NOW_MS);
        assert!(retained
            .gaps
            .iter()
            .any(|gap| gap.starts_with("NIFC wildfire paused:")));
        let persist = Persist::open(root).expect("bus");
        assert_eq!(
            persist
                .list_since(&wildfire_state_topic("rig-1"), None)
                .expect("rows")
                .len(),
            2
        );
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
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("client");
        let response = client
            .get(format!("http://{redirect_addr}/query"))
            .send()
            .expect("response");
        assert_eq!(response.status(), reqwest::StatusCode::FOUND);
        redirect_thread.join().expect("redirect join");
        target_thread.join().expect("target join");
        assert!(!contacted.load(Ordering::Relaxed));
    }

    struct SlowProbe;

    impl WildfireProbe for SlowProbe {
        fn fetch(&self, _context: WildfireContext) -> Result<ProbeResponse, ProbeFailure> {
            std::thread::sleep(Duration::from_millis(500));
            Ok(ProbeResponse::Modified(captured_geojson()))
        }
    }

    #[tokio::test]
    async fn shutdown_wins_while_wfigs_query_is_in_flight() {
        let worker = WildfireOverlayWorker::new("rig-1".to_string()).with_bus_root(None);
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
