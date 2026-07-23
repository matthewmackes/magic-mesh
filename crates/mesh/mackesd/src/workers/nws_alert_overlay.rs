//! WL-FUNC-012 / OVERLAY-1 — keyless NWS active-weather-alert adapter.
//!
//! Workstations opt in with `MDE_OVERLAY_NWS_ALERTS=1`. A fresh local vehicle
//! fix drives the point-scoped `/alerts/active?point=lat,lon` query. Inline CAP
//! geometry is normalized directly; alerts with null geometry resolve their
//! `affectedZones` GeoJSON through a bounded static cache. Blocking rustls HTTP
//! runs via `spawn_blocking`, never on a Tokio worker thread.

#![cfg(feature = "async-services")]

use std::collections::HashMap;
use std::io::{self, Read};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mackes_mesh_types::nws_alert::{
    nws_alert_state_topic, AlertPolygon, GeoPoint, GeometrySource, NwsAlert, NwsAlertSnapshot,
    NwsSeverity,
};
use reqwest::blocking::Client;
use reqwest::header::{ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED};
use serde::Deserialize;
use serde_json::Value;

use super::{ShutdownToken, Worker};

/// Explicit opt-in; absent/false is an idle no-op.
pub const ENABLED_ENV: &str = "MDE_OVERLAY_NWS_ALERTS";
/// Optional operator-controlled endpoint override.
pub const ENDPOINT_ENV: &str = "MDE_OVERLAY_NWS_ALERTS_URL";
/// Official point-scoped active-alert endpoint.
pub const DEFAULT_ENDPOINT: &str = "https://api.weather.gov/alerts/active";
/// NWS supports 30–60 second polling; one minute is polite and sufficient.
pub const POLL: Duration = Duration::from_secs(60);
const RETRY_MIN: Duration = Duration::from_secs(5);
const NO_FIX_RETRY: Duration = Duration::from_secs(5);
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const ZONE_HTTP_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_ALERT_BODY_BYTES: usize = 4 * 1024 * 1024;
const MAX_ZONE_BODY_BYTES: usize = 2 * 1024 * 1024;
const MAX_ZONE_CACHE: usize = 16;
const MAX_ZONE_CACHE_BYTES: usize = 16 * 1024 * 1024;
const MAX_FEATURES: usize = 256;
const MAX_ZONES_PER_ALERT: usize = 8;
const MAX_TOTAL_ZONE_FETCHES: usize = 4;
const MAX_POLYGONS_PER_GEOMETRY: usize = 64;
const MAX_RINGS_PER_POLYGON: usize = 16;
const MAX_POINTS_PER_RING: usize = 4_096;
const MAX_TOTAL_POLYGONS: usize = 512;
const MAX_TOTAL_POINTS: usize = 100_000;
const VEHICLE_FIX_MAX_AGE_MS: i64 = 30_000;
/// Permit small clock disagreement, but reject a mirror dated far in the future.
const VEHICLE_MAX_FUTURE_SKEW_MS: i64 = 5_000;
const USER_AGENT: &str =
    "Construct/12 mackesd NWS-alert-overlay (+https://github.com/matthewmackes/magic-mesh)";

/// Result of the conditional active-alert request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeResponse {
    /// HTTP 200 with GeoJSON.
    Modified(String),
    /// HTTP 304; the prior snapshot was revalidated.
    NotModified,
}

/// Fully prepared result returned by the blocking seam. Parsing and any
/// affected-zone HTTP requests happen before this crosses back onto Tokio.
enum PreparedResponse {
    Modified(NwsAlertSnapshot),
    NotModified,
}

/// Injectable raw HTTP seam.
pub trait NwsAlertProbe: Send + Sync {
    /// Fetch or revalidate active alerts for a point.
    fn fetch_alerts(&self, point: GeoPoint) -> io::Result<ProbeResponse>;
    /// Fetch a zone GeoJSON feature used when alert geometry is null.
    fn fetch_zone(&self, url: &str) -> io::Result<String>;
}

#[derive(Debug, Default)]
struct Validators {
    url: String,
    etag: Option<String>,
    last_modified: Option<String>,
}

/// Production rustls probe with required descriptive User-Agent.
pub struct NwsHttpProbe {
    client: Client,
    zone_client: Client,
    endpoint: String,
    validators: Mutex<Validators>,
    zone_cache: Mutex<HashMap<String, String>>,
}

impl NwsHttpProbe {
    fn new(endpoint: String) -> io::Result<Self> {
        let client = Client::builder()
            .timeout(HTTP_TIMEOUT)
            .user_agent(USER_AGENT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(io_other)?;
        let zone_client = Client::builder()
            .timeout(ZONE_HTTP_TIMEOUT)
            .user_agent(USER_AGENT)
            // An api.weather.gov affected-zone URL must never redirect this
            // worker to a different origin.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(io_other)?;
        Ok(Self {
            client,
            zone_client,
            endpoint,
            validators: Mutex::new(Validators::default()),
            zone_cache: Mutex::new(HashMap::new()),
        })
    }

    fn point_url(&self, point: GeoPoint) -> io::Result<String> {
        let mut url = reqwest::Url::parse(&self.endpoint).map_err(io_other)?;
        url.query_pairs_mut().append_pair(
            "point",
            &format!("{:.4},{:.4}", point.latitude, point.longitude),
        );
        Ok(url.to_string())
    }
}

impl NwsAlertProbe for NwsHttpProbe {
    fn fetch_alerts(&self, point: GeoPoint) -> io::Result<ProbeResponse> {
        let url = self.point_url(point)?;
        let mut request = self.client.get(&url);
        let mut sent_validator = false;
        {
            let validators = self
                .validators
                .lock()
                .map_err(|_| io::Error::other("NWS validator lock poisoned"))?;
            if validators.url == url {
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
        reject_non_success(&response, "NWS alerts")?;
        let mut response = response.error_for_status().map_err(io_other)?;
        reject_declared_oversize(&response, MAX_ALERT_BODY_BYTES, "NWS alerts")?;
        let etag = header_string(&response, ETAG);
        let last_modified = header_string(&response, LAST_MODIFIED);
        let body = read_bounded_body(&mut response, MAX_ALERT_BODY_BYTES, "NWS alerts")?;
        let mut validators = self
            .validators
            .lock()
            .map_err(|_| io::Error::other("NWS validator lock poisoned"))?;
        *validators = Validators {
            url,
            etag,
            last_modified,
        };
        Ok(ProbeResponse::Modified(body))
    }

    fn fetch_zone(&self, url: &str) -> io::Result<String> {
        validate_zone_url(url)?;
        if let Some(body) = self
            .zone_cache
            .lock()
            .map_err(|_| io::Error::other("NWS zone cache lock poisoned"))?
            .get(url)
            .cloned()
        {
            return Ok(body);
        }
        let response = self.zone_client.get(url).send().map_err(io_other)?;
        reject_non_success(&response, "NWS zone")?;
        let mut response = response.error_for_status().map_err(io_other)?;
        reject_declared_oversize(&response, MAX_ZONE_BODY_BYTES, "NWS zone")?;
        let body = read_bounded_body(&mut response, MAX_ZONE_BODY_BYTES, "NWS zone")?;
        let mut cache = self
            .zone_cache
            .lock()
            .map_err(|_| io::Error::other("NWS zone cache lock poisoned"))?;
        insert_zone_cache(&mut cache, url, &body);
        Ok(body)
    }
}

fn accept_not_modified(sent_validator: bool) -> io::Result<ProbeResponse> {
    if sent_validator {
        Ok(ProbeResponse::NotModified)
    } else {
        Err(io::Error::other(
            "NWS returned 304 although this point request sent no validator",
        ))
    }
}

fn insert_zone_cache(cache: &mut HashMap<String, String>, url: &str, body: &str) {
    let retained_bytes = cache
        .values()
        .map(String::len)
        .fold(0_usize, usize::saturating_add);
    if cache.len() >= MAX_ZONE_CACHE
        || retained_bytes.saturating_add(body.len()) > MAX_ZONE_CACHE_BYTES
    {
        cache.clear();
    }
    if body.len() <= MAX_ZONE_CACHE_BYTES {
        cache.insert(url.to_string(), body.to_string());
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

fn validate_zone_url(raw: &str) -> io::Result<()> {
    let url = reqwest::Url::parse(raw).map_err(io_other)?;
    if url.scheme() != "https" || url.host_str() != Some("api.weather.gov") {
        return Err(io::Error::other("refusing non-NWS affected-zone URL"));
    }
    Ok(())
}

fn reject_declared_oversize(
    response: &reqwest::blocking::Response,
    max_bytes: usize,
    label: &str,
) -> io::Result<()> {
    if response
        .content_length()
        .is_some_and(|length| length > u64::try_from(max_bytes).unwrap_or(u64::MAX))
    {
        return Err(io::Error::other(format!(
            "{label} response exceeds {max_bytes} byte limit"
        )));
    }
    Ok(())
}

fn reject_non_success(response: &reqwest::blocking::Response, label: &str) -> io::Result<()> {
    if !response.status().is_success() {
        return Err(io::Error::other(format!(
            "{label} returned unexpected HTTP {} (redirects are disabled)",
            response.status()
        )));
    }
    Ok(())
}

fn read_bounded_body(
    response: &mut impl Read,
    max_bytes: usize,
    label: &str,
) -> io::Result<String> {
    let limit = u64::try_from(max_bytes)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let mut bytes = Vec::with_capacity(max_bytes.min(64 * 1024));
    response.take(limit).read_to_end(&mut bytes)?;
    if bytes.len() > max_bytes {
        return Err(io::Error::other(format!(
            "{label} response exceeds {max_bytes} byte limit"
        )));
    }
    String::from_utf8(bytes).map_err(io_other)
}

fn io_other(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

#[derive(Debug, Deserialize)]
struct Feed {
    updated: Option<String>,
    #[serde(default)]
    features: Vec<Feature>,
}

#[derive(Debug, Deserialize)]
struct Feature {
    #[serde(default)]
    id: String,
    geometry: Option<Geometry>,
    #[serde(default)]
    properties: Properties,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Properties {
    #[serde(default)]
    event: String,
    #[serde(default)]
    headline: String,
    #[serde(default)]
    area_desc: String,
    #[serde(default)]
    severity: String,
    #[serde(default)]
    urgency: String,
    #[serde(default)]
    certainty: String,
    sent: Option<String>,
    expires: Option<String>,
    #[serde(default)]
    affected_zones: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Geometry {
    #[serde(rename = "type")]
    kind: String,
    coordinates: Value,
}

struct ParsedAlert {
    alert: NwsAlert,
    affected_zones: Vec<String>,
}

fn build_snapshot(
    probe: &dyn NwsAlertProbe,
    host: &str,
    point: GeoPoint,
    body: &str,
    fetched_at_ms: i64,
) -> io::Result<NwsAlertSnapshot> {
    if body.len() > MAX_ALERT_BODY_BYTES {
        return Err(io::Error::other(format!(
            "NWS alerts response exceeds {MAX_ALERT_BODY_BYTES} byte limit"
        )));
    }
    let feed: Feed = serde_json::from_str(body).map_err(io_other)?;
    let mut snapshot = NwsAlertSnapshot::empty(host, fetched_at_ms);
    snapshot.feed_updated_at_ms = feed.updated.as_deref().and_then(parse_time_ms);
    snapshot.query_point = Some(point);
    if feed.features.len() > MAX_FEATURES {
        snapshot.gaps.push(format!(
            "feed contains {} features; only the first {MAX_FEATURES} are processed",
            feed.features.len()
        ));
    }
    let mut zone_results: HashMap<String, Result<Vec<AlertPolygon>, String>> = HashMap::new();
    let mut zone_fetches = 0_usize;
    let mut total_polygons = 0_usize;
    let mut total_points = 0_usize;

    for (index, feature) in feed.features.into_iter().take(MAX_FEATURES).enumerate() {
        let Some(mut parsed) = parse_feature(feature, index, &mut snapshot.gaps) else {
            continue;
        };
        if parsed.alert.polygons.is_empty() {
            let mut resolved = Vec::new();
            if parsed.affected_zones.len() > MAX_ZONES_PER_ALERT {
                snapshot.gaps.push(format!(
                    "alert {} names {} affected zones; only the first {MAX_ZONES_PER_ALERT} are considered",
                    parsed.alert.id,
                    parsed.affected_zones.len()
                ));
            }
            for zone_url in parsed.affected_zones.iter().take(MAX_ZONES_PER_ALERT) {
                let zone_url = zone_url.trim();
                if zone_url.is_empty() {
                    continue;
                }
                if !zone_results.contains_key(zone_url) {
                    if zone_fetches >= MAX_TOTAL_ZONE_FETCHES {
                        snapshot.gaps.push(format!(
                            "alert {} affected-zone resolution capped at {MAX_TOTAL_ZONE_FETCHES} unique requests",
                            parsed.alert.id
                        ));
                        break;
                    }
                    zone_fetches += 1;
                    let result = probe
                        .fetch_zone(zone_url)
                        .and_then(|zone_body| parse_zone_geometry(&zone_body))
                        .map_err(|error| error.to_string());
                    zone_results.insert(zone_url.to_string(), result);
                }
                match zone_results.get(zone_url) {
                    Some(Ok(polygons)) => resolved.extend(polygons.iter().cloned()),
                    Some(Err(error)) => snapshot.gaps.push(format!(
                        "alert {} affected zone unavailable or invalid: {error}",
                        parsed.alert.id
                    )),
                    None => {}
                }
            }
            if !resolved.is_empty() {
                parsed.alert.polygons = resolved;
                parsed.alert.geometry_source = Some(GeometrySource::AffectedZone);
            }
        }
        let (alert_polygons, alert_points) = geometry_counts(&parsed.alert.polygons);
        if total_polygons.saturating_add(alert_polygons) > MAX_TOTAL_POLYGONS
            || total_points.saturating_add(alert_points) > MAX_TOTAL_POINTS
        {
            snapshot.gaps.push(format!(
                "alert {} geometry omitted: snapshot geometry budget exceeded",
                parsed.alert.id
            ));
            parsed.alert.polygons.clear();
            parsed.alert.geometry_source = None;
        } else {
            total_polygons += alert_polygons;
            total_points += alert_points;
        }
        if parsed.alert.polygons.is_empty() {
            snapshot.gaps.push(format!(
                "alert {} has no resolvable geometry",
                parsed.alert.id
            ));
        }
        snapshot.alerts.push(parsed.alert);
    }
    Ok(snapshot)
}

fn geometry_counts(polygons: &[AlertPolygon]) -> (usize, usize) {
    let points = polygons
        .iter()
        .flat_map(|polygon| &polygon.rings)
        .map(Vec::len)
        .fold(0_usize, usize::saturating_add);
    (polygons.len(), points)
}

fn parse_feature(feature: Feature, index: usize, gaps: &mut Vec<String>) -> Option<ParsedAlert> {
    if feature.id.trim().is_empty() {
        gaps.push(format!("feature {index} omitted: missing id"));
        return None;
    }
    let polygons = match feature.geometry {
        Some(geometry) => match geometry_polygons(&geometry) {
            Ok(polygons) => polygons,
            Err(error) => {
                gaps.push(format!(
                    "alert {} inline geometry invalid: {error}",
                    feature.id
                ));
                Vec::new()
            }
        },
        None => Vec::new(),
    };
    let geometry_source = (!polygons.is_empty()).then_some(GeometrySource::Inline);
    let severity = match feature.properties.severity.as_str() {
        "Extreme" => NwsSeverity::Extreme,
        "Severe" => NwsSeverity::Severe,
        "Moderate" => NwsSeverity::Moderate,
        "Minor" => NwsSeverity::Minor,
        _ => NwsSeverity::Unknown,
    };
    Some(ParsedAlert {
        alert: NwsAlert {
            id: feature.id,
            event: feature.properties.event,
            headline: feature.properties.headline,
            area_desc: feature.properties.area_desc,
            severity,
            urgency: feature.properties.urgency,
            certainty: feature.properties.certainty,
            sent_at_ms: feature.properties.sent.as_deref().and_then(parse_time_ms),
            expires_at_ms: feature
                .properties
                .expires
                .as_deref()
                .and_then(parse_time_ms),
            polygons,
            geometry_source,
        },
        affected_zones: feature.properties.affected_zones,
    })
}

fn parse_zone_geometry(body: &str) -> io::Result<Vec<AlertPolygon>> {
    if body.len() > MAX_ZONE_BODY_BYTES {
        return Err(io::Error::other(format!(
            "NWS zone response exceeds {MAX_ZONE_BODY_BYTES} byte limit"
        )));
    }
    #[derive(Deserialize)]
    struct ZoneFeature {
        geometry: Geometry,
    }
    let zone: ZoneFeature = serde_json::from_str(body).map_err(io_other)?;
    geometry_polygons(&zone.geometry)
}

fn geometry_polygons(geometry: &Geometry) -> io::Result<Vec<AlertPolygon>> {
    match geometry.kind.as_str() {
        "Polygon" => Ok(vec![AlertPolygon {
            rings: parse_rings(&geometry.coordinates)?,
        }]),
        "MultiPolygon" => {
            let polygons = geometry
                .coordinates
                .as_array()
                .ok_or_else(|| io::Error::other("MultiPolygon coordinates are not an array"))?;
            if polygons.len() > MAX_POLYGONS_PER_GEOMETRY {
                return Err(io::Error::other(format!(
                    "geometry has more than {MAX_POLYGONS_PER_GEOMETRY} polygons"
                )));
            }
            polygons
                .iter()
                .map(|polygon| parse_rings(polygon).map(|rings| AlertPolygon { rings }))
                .collect()
        }
        other => Err(io::Error::other(format!(
            "unsupported geometry type `{other}`"
        ))),
    }
}

fn parse_rings(value: &Value) -> io::Result<Vec<Vec<GeoPoint>>> {
    let rings = value
        .as_array()
        .ok_or_else(|| io::Error::other("polygon rings are not an array"))?;
    if rings.len() > MAX_RINGS_PER_POLYGON {
        return Err(io::Error::other(format!(
            "polygon has more than {MAX_RINGS_PER_POLYGON} rings"
        )));
    }
    rings
        .iter()
        .map(|ring| {
            let coords = ring
                .as_array()
                .ok_or_else(|| io::Error::other("polygon ring is not an array"))?;
            if coords.len() > MAX_POINTS_PER_RING {
                return Err(io::Error::other(format!(
                    "polygon ring has more than {MAX_POINTS_PER_RING} points"
                )));
            }
            let points: Vec<GeoPoint> = coords
                .iter()
                .filter_map(|coord| {
                    let values = coord.as_array()?;
                    let longitude = values.first()?.as_f64()?;
                    let latitude = values.get(1)?.as_f64()?;
                    (latitude.is_finite()
                        && longitude.is_finite()
                        && (-90.0..=90.0).contains(&latitude)
                        && (-180.0..=180.0).contains(&longitude))
                    .then_some(GeoPoint {
                        latitude,
                        longitude,
                    })
                })
                .collect();
            if points.len() < 3 {
                return Err(io::Error::other("polygon ring has fewer than 3 points"));
            }
            Ok(points)
        })
        .collect()
}

fn parse_time_ms(raw: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|value| value.timestamp_millis())
}

/// Workstation-side NWS adapter.
pub struct NwsAlertOverlayWorker {
    host: String,
    probe: Option<Arc<dyn NwsAlertProbe>>,
    bus_root: Option<PathBuf>,
    poll: Duration,
}

impl NwsAlertOverlayWorker {
    /// Production wiring. Disabled unless explicitly opted in.
    #[must_use]
    pub fn new(host: String) -> Self {
        let probe = if env_truthy(ENABLED_ENV) {
            let endpoint = std::env::var(ENDPOINT_ENV)
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_ENDPOINT.to_string());
            match NwsHttpProbe::new(endpoint) {
                Ok(probe) => Some(Arc::new(probe) as Arc<dyn NwsAlertProbe>),
                Err(error) => {
                    tracing::warn!(target: "mackesd::nws_alert_overlay", %error, "NWS client unavailable; worker idle");
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
    pub fn with_probe(mut self, probe: Arc<dyn NwsAlertProbe>) -> Self {
        self.probe = Some(probe);
        self
    }

    /// Override/disable Bus access.
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

    fn current_vehicle_point(&self) -> Option<GeoPoint> {
        let root = self.bus_root.clone()?;
        let persist = mde_bus::persist::Persist::open(root).ok()?;
        let topic = mackes_mesh_types::vehicle::vehicle_state_topic(&self.host);
        let body = persist.read_latest(&topic).ok().flatten()?.body?;
        let vehicle: mackes_mesh_types::vehicle::VehicleState = serde_json::from_str(&body).ok()?;
        validated_vehicle_point(&vehicle, &self.host, now_ms())
    }

    fn publish(&self, snapshot: &NwsAlertSnapshot) {
        if let Some(mut persist) = crate::bus_publish::open_bus(self.bus_root.clone()) {
            crate::bus_publish::publish_json(
                &mut persist,
                &nws_alert_state_topic(&self.host),
                snapshot,
            );
        }
    }

    fn apply_result(
        &self,
        result: io::Result<PreparedResponse>,
        point: GeoPoint,
        last_good: &mut Option<NwsAlertSnapshot>,
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
                    .is_some_and(|snapshot| snapshot.query_point != Some(point))
                {
                    self.publish_failure(
                        last_good,
                        "NWS refresh failed: 304 point does not match last-good snapshot",
                    );
                    return false;
                }
                if let Some(snapshot) = last_good {
                    snapshot.fetched_at_ms = now_ms();
                    snapshot.query_point = Some(point);
                    snapshot
                        .gaps
                        .retain(|gap| !gap.starts_with("NWS refresh failed:"));
                    self.publish(snapshot);
                    true
                } else {
                    false
                }
            }
            Err(error) => {
                self.publish_failure(last_good, &format!("NWS refresh failed: {error}"));
                false
            }
        }
    }

    fn publish_failure(&self, last_good: &mut Option<NwsAlertSnapshot>, gap: &str) {
        tracing::warn!(target: "mackesd::nws_alert_overlay", host = %self.host, error = gap, "NWS refresh failed; retaining last-good snapshot");
        if let Some(snapshot) = last_good {
            snapshot
                .gaps
                .retain(|existing| !existing.starts_with("NWS refresh failed:"));
            snapshot.gaps.push(gap.to_string());
            self.publish(snapshot);
        }
    }

    async fn fetch_async(
        &self,
        probe: Arc<dyn NwsAlertProbe>,
        point: GeoPoint,
        shutdown: &mut ShutdownToken,
    ) -> Option<io::Result<PreparedResponse>> {
        let host = self.host.clone();
        let task = tokio::task::spawn_blocking(move || match probe.fetch_alerts(point)? {
            ProbeResponse::Modified(body) => {
                build_snapshot(probe.as_ref(), &host, point, &body, now_ms())
                    .map(PreparedResponse::Modified)
                    .map_err(|error| io::Error::other(format!("NWS payload invalid: {error}")))
            }
            ProbeResponse::NotModified => Ok(PreparedResponse::NotModified),
        });
        tokio::select! {
            () = shutdown.wait() => None,
            joined = task => Some(match joined {
                Ok(result) => result,
                Err(error) => Err(io::Error::other(format!("NWS fetch task failed: {error}"))),
            }),
        }
    }
}

fn validated_vehicle_point(
    vehicle: &mackes_mesh_types::vehicle::VehicleState,
    expected_host: &str,
    now: i64,
) -> Option<GeoPoint> {
    let mirror_age = now.saturating_sub(vehicle.published_at_ms).max(0);
    let future_skew = vehicle.published_at_ms.saturating_sub(now).max(0);
    if vehicle.host != expected_host
        || !vehicle.online
        || !vehicle.gps.has_fix()
        || !vehicle.gps.latitude.is_finite()
        || !vehicle.gps.longitude.is_finite()
        || !(-90.0..=90.0).contains(&vehicle.gps.latitude)
        || !(-180.0..=180.0).contains(&vehicle.gps.longitude)
        || !vehicle.gps.age_s.is_finite()
        || vehicle.gps.age_s < 0.0
        || future_skew > VEHICLE_MAX_FUTURE_SKEW_MS
        || mirror_age as f64 + f64::from(vehicle.gps.age_s) * 1_000.0
            > VEHICLE_FIX_MAX_AGE_MS as f64
    {
        return None;
    }
    Some(GeoPoint {
        latitude: vehicle.gps.latitude,
        longitude: vehicle.gps.longitude,
    })
}

#[async_trait::async_trait]
impl Worker for NwsAlertOverlayWorker {
    fn name(&self) -> &'static str {
        "nws_alert_overlay"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let Some(probe) = self.probe.clone() else {
            tracing::info!(target: "mackesd::nws_alert_overlay", env = ENABLED_ENV, "NWS alert overlay not configured; worker idle");
            shutdown.wait().await;
            return Ok(());
        };
        let mut last_good = None;
        let mut retry = RETRY_MIN.min(self.poll);
        loop {
            let Some(point) = self.current_vehicle_point() else {
                tokio::select! {
                    () = shutdown.wait() => break,
                    () = tokio::time::sleep(NO_FIX_RETRY.min(self.poll)) => {}
                }
                continue;
            };
            let Some(result) = self.fetch_async(probe.clone(), point, &mut shutdown).await else {
                break;
            };
            let success = self.apply_result(result, point, &mut last_good);
            let delay = if success { self.poll } else { retry };
            retry = if success {
                RETRY_MIN.min(self.poll)
            } else {
                retry.saturating_mul(2).min(self.poll)
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
    use std::collections::VecDeque;
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use mackes_mesh_types::vehicle::{GpsFix, VehicleState};
    use mde_bus::hooks::config::Priority;
    use mde_bus::persist::Persist;

    use super::*;

    const CAPTURED_ALERTS: &str = r#"{
      "type":"FeatureCollection","updated":"2026-07-22T10:09:00+00:00",
      "features":[
        {"id":"urn:oid:warning-inline","geometry":{"type":"Polygon","coordinates":[[[-95.2,32.0],[-94.8,32.0],[-94.8,32.4],[-95.2,32.4],[-95.2,32.0]]]},"properties":{"event":"Severe Thunderstorm Warning","headline":"Severe Thunderstorm Warning issued","areaDesc":"Test County","severity":"Severe","urgency":"Immediate","certainty":"Observed","sent":"2026-07-22T10:00:00+00:00","expires":"2026-07-22T10:30:00+00:00","affectedZones":[]}},
        {"id":"urn:oid:warning-zone","geometry":null,"properties":{"event":"Flood Warning","headline":"Flood Warning issued","areaDesc":"River County","severity":"Moderate","urgency":"Expected","certainty":"Likely","sent":"2026-07-22T10:01:00+00:00","expires":"2026-07-22T12:00:00+00:00","affectedZones":["https://api.weather.gov/zones/forecast/TXZ001"]}}
      ]
    }"#;

    const CAPTURED_ZONE: &str = r#"{"type":"Feature","geometry":{"type":"MultiPolygon","coordinates":[[[[-96.0,31.0],[-95.5,31.0],[-95.5,31.5],[-96.0,31.5],[-96.0,31.0]]]]},"properties":{"id":"TXZ001"}}"#;

    struct FakeProbe {
        responses: Mutex<VecDeque<Result<ProbeResponse, String>>>,
        zone: Result<String, String>,
    }

    impl FakeProbe {
        fn captured() -> Self {
            Self {
                responses: Mutex::new(
                    vec![Ok(ProbeResponse::Modified(CAPTURED_ALERTS.to_string()))].into(),
                ),
                zone: Ok(CAPTURED_ZONE.to_string()),
            }
        }
    }

    impl NwsAlertProbe for FakeProbe {
        fn fetch_alerts(&self, _point: GeoPoint) -> io::Result<ProbeResponse> {
            self.responses
                .lock()
                .map_err(|_| io::Error::other("fake lock"))?
                .pop_front()
                .unwrap_or_else(|| Err("no response".to_string()))
                .map_err(io::Error::other)
        }

        fn fetch_zone(&self, _url: &str) -> io::Result<String> {
            self.zone.clone().map_err(io::Error::other)
        }
    }

    fn point() -> GeoPoint {
        GeoPoint {
            latitude: 32.2,
            longitude: -95.0,
        }
    }

    #[test]
    fn captured_fixture_parses_inline_and_affected_zone_geometry() {
        let probe = FakeProbe::captured();
        let snapshot = build_snapshot(&probe, "rig-1", point(), CAPTURED_ALERTS, 100)
            .expect("captured NWS parse");
        assert_eq!(snapshot.alerts.len(), 2);
        assert_eq!(snapshot.alerts[0].severity, NwsSeverity::Severe);
        assert_eq!(
            snapshot.alerts[0].geometry_source,
            Some(GeometrySource::Inline)
        );
        assert_eq!(
            snapshot.alerts[1].geometry_source,
            Some(GeometrySource::AffectedZone)
        );
        assert_eq!(snapshot.alerts[1].polygons[0].rings[0][0].longitude, -96.0);
        assert_eq!(snapshot.license_tier, "public-domain");
    }

    #[test]
    fn malformed_or_oversized_payload_fails_honestly() {
        let probe = FakeProbe::captured();
        assert!(build_snapshot(&probe, "rig-1", point(), "not json", 0).is_err());
        let oversized = " ".repeat(MAX_ALERT_BODY_BYTES + 1);
        let error =
            build_snapshot(&probe, "rig-1", point(), &oversized, 0).expect_err("oversize rejected");
        assert!(error.to_string().contains("exceeds"));
    }

    struct CountingZoneProbe {
        calls: AtomicUsize,
    }

    impl NwsAlertProbe for CountingZoneProbe {
        fn fetch_alerts(&self, _point: GeoPoint) -> io::Result<ProbeResponse> {
            unreachable!("snapshot parser test does not fetch the alert feed")
        }

        fn fetch_zone(&self, _url: &str) -> io::Result<String> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(CAPTURED_ZONE.to_string())
        }
    }

    #[test]
    fn hostile_cardinality_is_capped_and_zone_urls_are_deduplicated() {
        let zones: Vec<String> = (0..20)
            .map(|index| format!("https://api.weather.gov/zones/forecast/TXZ{index:03}"))
            .collect();
        let features: Vec<Value> = (0..300)
            .map(|index| {
                serde_json::json!({
                    "id": format!("urn:test:{index}"),
                    "geometry": null,
                    "properties": {
                        "event": "Flood Warning",
                        "severity": "Moderate",
                        "affectedZones": zones,
                    }
                })
            })
            .collect();
        let body = serde_json::json!({
            "type": "FeatureCollection",
            "features": features,
        })
        .to_string();
        let probe = CountingZoneProbe {
            calls: AtomicUsize::new(0),
        };
        let snapshot = build_snapshot(&probe, "rig-1", point(), &body, 10).expect("bounded");
        assert_eq!(snapshot.alerts.len(), MAX_FEATURES);
        assert_eq!(probe.calls.load(Ordering::Relaxed), MAX_TOTAL_ZONE_FETCHES);
        assert!(snapshot.gaps.iter().any(|gap| gap.contains("first 256")));
        assert!(snapshot.gaps.iter().any(|gap| gap.contains("first 8")));
        assert!(snapshot.gaps.iter().any(|gap| gap.contains("capped at 4")));
        assert!(
            snapshot
                .gaps
                .iter()
                .any(|gap| gap.contains("geometry budget exceeded")),
            "truncation must remain visible as a degraded snapshot"
        );

        let too_many_points: Vec<[f64; 2]> = (0..=MAX_POINTS_PER_RING)
            .map(|index| [index as f64 / 10_000.0, 0.0])
            .collect();
        let geometry = Geometry {
            kind: "Polygon".to_string(),
            coordinates: serde_json::json!([too_many_points]),
        };
        let error = geometry_polygons(&geometry).expect_err("point budget");
        assert!(error.to_string().contains("more than 4096 points"));
    }

    #[test]
    fn affected_zone_url_is_pinned_to_official_https_host() {
        assert!(validate_zone_url("https://api.weather.gov/zones/forecast/TXZ001").is_ok());
        assert!(validate_zone_url("http://api.weather.gov/zones/forecast/TXZ001").is_err());
        assert!(validate_zone_url("https://example.test/steal").is_err());
    }

    #[test]
    fn zone_cache_enforces_count_and_aggregate_byte_budgets() {
        let mut cache = HashMap::new();
        let body = "x".repeat(MAX_ZONE_BODY_BYTES);
        for index in 0..(MAX_ZONE_CACHE + 4) {
            insert_zone_cache(&mut cache, &format!("zone-{index}"), &body);
            assert!(cache.len() <= MAX_ZONE_CACHE);
            assert!(cache.values().map(String::len).sum::<usize>() <= MAX_ZONE_CACHE_BYTES);
        }
    }

    #[test]
    fn spurious_not_modified_without_a_sent_validator_is_rejected() {
        assert!(accept_not_modified(false).is_err());
        assert_eq!(
            accept_not_modified(true).expect("validated 304"),
            ProbeResponse::NotModified
        );
    }

    #[test]
    fn not_modified_cannot_relabel_a_prior_points_snapshot() {
        let worker = NwsAlertOverlayWorker::new("rig-1".to_string()).with_bus_root(None);
        let mut old = NwsAlertSnapshot::empty("rig-1", 10);
        old.query_point = Some(GeoPoint {
            latitude: 32.0,
            longitude: -95.0,
        });
        let mut last_good = Some(old);
        let new_point = GeoPoint {
            latitude: 33.0,
            longitude: -96.0,
        };
        assert!(!worker.apply_result(Ok(PreparedResponse::NotModified), new_point, &mut last_good));
        let retained = last_good.expect("old snapshot retained");
        assert_ne!(retained.query_point, Some(new_point));
        assert_eq!(retained.fetched_at_ms, 10);
        assert!(retained.gaps.iter().any(|gap| gap.contains("304 point")));
    }

    #[test]
    fn http_client_refuses_redirects_before_contacting_the_target() {
        let target = TcpListener::bind("127.0.0.1:0").expect("target listener");
        target.set_nonblocking(true).expect("nonblocking");
        let target_addr = target.local_addr().expect("target addr");
        let contacted = Arc::new(AtomicBool::new(false));
        let contacted_thread = contacted.clone();
        let target_thread = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_millis(400);
            while std::time::Instant::now() < deadline {
                match target.accept() {
                    Ok((_stream, _)) => {
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

        let redirect = TcpListener::bind("127.0.0.1:0").expect("redirect listener");
        let redirect_addr = redirect.local_addr().expect("redirect addr");
        let redirect_thread = std::thread::spawn(move || {
            let (mut stream, _) = redirect.accept().expect("redirect request");
            let mut request = [0_u8; 2048];
            let _ = stream.read(&mut request);
            write!(
                stream,
                "HTTP/1.1 302 Found\r\nLocation: http://{target_addr}/escaped\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .expect("redirect response");
        });

        let probe =
            NwsHttpProbe::new(format!("http://{redirect_addr}/alerts")).expect("probe client");
        let error = probe.fetch_alerts(point()).expect_err("redirect rejected");
        assert!(error.to_string().contains("redirects are disabled"));
        redirect_thread.join().expect("redirect thread");
        target_thread.join().expect("target thread");
        assert!(!contacted.load(Ordering::Relaxed));
    }

    #[test]
    fn fresh_vehicle_fix_is_required_for_query_point() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path().to_path_buf();
        let persist = Persist::open(root.clone()).expect("bus");
        let mut vehicle = VehicleState::offline("rig-1");
        vehicle.online = true;
        vehicle.gps = GpsFix {
            fix_type: "gps".to_string(),
            latitude: 32.2,
            longitude: -95.0,
            satellites: 8,
            ..GpsFix::default()
        };
        vehicle.published_at_ms = now_ms();
        persist
            .write(
                &mackes_mesh_types::vehicle::vehicle_state_topic("rig-1"),
                Priority::Default,
                None,
                Some(&serde_json::to_string(&vehicle).expect("json")),
            )
            .expect("write");
        let worker =
            NwsAlertOverlayWorker::new("rig-1".to_string()).with_bus_root(Some(root.clone()));
        assert_eq!(worker.current_vehicle_point(), Some(point()));

        vehicle.published_at_ms = now_ms() - VEHICLE_FIX_MAX_AGE_MS - 1;
        persist
            .write(
                &mackes_mesh_types::vehicle::vehicle_state_topic("rig-1"),
                Priority::Default,
                None,
                Some(&serde_json::to_string(&vehicle).expect("json")),
            )
            .expect("write stale");
        assert!(worker.current_vehicle_point().is_none());
    }

    #[test]
    fn hostile_vehicle_coordinates_age_host_and_future_time_are_rejected() {
        let now = 1_000_000;
        let mut vehicle = VehicleState::offline("rig-1");
        vehicle.online = true;
        vehicle.gps = GpsFix {
            fix_type: "gps".to_string(),
            latitude: 32.2,
            longitude: -95.0,
            satellites: 8,
            ..GpsFix::default()
        };
        vehicle.published_at_ms = now;
        assert_eq!(
            validated_vehicle_point(&vehicle, "rig-1", now),
            Some(point())
        );

        vehicle.gps.latitude = f64::NAN;
        assert!(validated_vehicle_point(&vehicle, "rig-1", now).is_none());
        vehicle.gps.latitude = 91.0;
        assert!(validated_vehicle_point(&vehicle, "rig-1", now).is_none());
        vehicle.gps.latitude = 32.2;
        vehicle.gps.longitude = f64::INFINITY;
        assert!(validated_vehicle_point(&vehicle, "rig-1", now).is_none());
        vehicle.gps.longitude = -95.0;
        vehicle.gps.age_s = -1.0;
        assert!(validated_vehicle_point(&vehicle, "rig-1", now).is_none());
        vehicle.gps.age_s = f32::NAN;
        assert!(validated_vehicle_point(&vehicle, "rig-1", now).is_none());
        vehicle.gps.age_s = 0.0;
        vehicle.host = "other-rig".to_string();
        assert!(validated_vehicle_point(&vehicle, "rig-1", now).is_none());
        vehicle.host = "rig-1".to_string();
        vehicle.published_at_ms = now + VEHICLE_MAX_FUTURE_SKEW_MS + 1;
        assert!(validated_vehicle_point(&vehicle, "rig-1", now).is_none());
        vehicle.published_at_ms = now - 20_000;
        vehicle.gps.age_s = 20.0;
        assert!(
            validated_vehicle_point(&vehicle, "rig-1", now).is_none(),
            "mirror age and upstream fix age are cumulative"
        );
    }

    struct SlowProbe;

    impl NwsAlertProbe for SlowProbe {
        fn fetch_alerts(&self, _point: GeoPoint) -> io::Result<ProbeResponse> {
            std::thread::sleep(Duration::from_millis(500));
            Ok(ProbeResponse::Modified(CAPTURED_ALERTS.to_string()))
        }

        fn fetch_zone(&self, _url: &str) -> io::Result<String> {
            Ok(CAPTURED_ZONE.to_string())
        }
    }

    #[tokio::test]
    async fn blocking_fetch_does_not_delay_shutdown() {
        let worker = NwsAlertOverlayWorker::new("rig-1".to_string()).with_bus_root(None);
        let (tx, rx) = tokio::sync::watch::channel(false);
        let mut token = ShutdownToken::from_receiver(rx);
        let task = tokio::spawn(async move {
            worker
                .fetch_async(Arc::new(SlowProbe), point(), &mut token)
                .await
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        tx.send(true).expect("shutdown");
        let joined = tokio::time::timeout(Duration::from_millis(200), task).await;
        assert!(joined.is_ok(), "shutdown wins over blocking reqwest seam");
    }
}
