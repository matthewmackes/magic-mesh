//! WL-FUNC-012 / OVERLAY-5 — keyless Caltrans CWWP2 camera adapter.

#![cfg(feature = "async-services")]

use std::collections::HashSet;
use std::io::{self, Read};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use mackes_mesh_types::caltrans_camera::{
    caltrans_camera_state_topic, CaltransCamera, CaltransCameraSnapshot, CameraThumbnail,
};
use reqwest::blocking::Client;
use reqwest::header::{CONTENT_TYPE, ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED};
use serde::Deserialize;

use super::{ShutdownToken, Worker};

/// Explicit opt-in; unset/false is an idle no-op.
pub const ENABLED_ENV: &str = "MDE_OVERLAY_CALTRANS_CAMERAS";
/// Required Caltrans district number (`1` through `12`).
pub const DISTRICT_ENV: &str = "MDE_OVERLAY_CALTRANS_DISTRICT";
/// Conditional catalog and current-still refresh cadence.
pub const POLL: Duration = Duration::from_secs(60);
const RETRY_MAX: Duration = Duration::from_secs(15 * 60);
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_CATALOG_BODY_BYTES: usize = 2 * 1024 * 1024;
const MAX_THUMBNAIL_BYTES: usize = 128 * 1024;
const MAX_FEED_CAMERAS: usize = 2_000;
const MAX_RETAINED_CAMERAS: usize = 128;
const MAX_THUMBNAILS: usize = 3;
const MAX_GAPS: usize = 128;
const MAX_STRING_BYTES: usize = 128;
const RELEVANCE_RADIUS_NM: f64 = 30.0;
const THUMBNAIL_RADIUS_NM: f32 = 10.0;
const AHEAD_CONE_DEG: f64 = 75.0;
const VEHICLE_FIX_MAX_AGE_MS: i64 = 30_000;
const VEHICLE_MAX_FUTURE_SKEW_MS: i64 = 5_000;
const USER_AGENT: &str =
    "Construct/12 mackesd Caltrans-CWWP2-overlay (+https://github.com/matthewmackes/magic-mesh)";

/// Fresh finite vehicle point and optional trustworthy heading.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraContext {
    /// WGS-84 latitude.
    pub latitude: f64,
    /// WGS-84 longitude.
    pub longitude: f64,
    /// Motion heading when speed/heading are plausible.
    pub heading_deg: Option<f32>,
}

/// Conditional catalog response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogResponse {
    /// Complete HTTP 200 catalog and optional HTTP modification time.
    Modified {
        /// Bounded JSON body.
        body: String,
        /// Parsed HTTP `Last-Modified`, Unix milliseconds.
        modified_at_ms: Option<i64>,
    },
    /// Validator-backed HTTP 304.
    NotModified,
}

/// Bounded official current still.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThumbnailResponse {
    /// JPEG bytes.
    pub jpeg: Vec<u8>,
    /// Server modification time, or local fetch time when absent.
    pub observed_at_ms: i64,
}

/// Injectable catalog/current-still seam.
pub trait CaltransCameraProbe: Send + Sync {
    /// Fetch one district catalog.
    fn fetch_catalog(&self, district: u8, point: CameraContext) -> io::Result<CatalogResponse>;
    /// Fetch one validated official current still.
    fn fetch_thumbnail(&self, district: u8, url: &str) -> io::Result<ThumbnailResponse>;
}

#[derive(Debug, Default)]
struct Validators {
    district: Option<u8>,
    point: Option<CameraContext>,
    etag: Option<String>,
    last_modified: Option<String>,
}

/// Production rustls CWWP2 probe.
pub struct CaltransHttpProbe {
    client: Client,
    validators: Mutex<Validators>,
}

impl CaltransHttpProbe {
    fn new() -> io::Result<Self> {
        let client = Client::builder()
            .timeout(HTTP_TIMEOUT)
            .user_agent(USER_AGENT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(io_other)?;
        Ok(Self {
            client,
            validators: Mutex::new(Validators::default()),
        })
    }
}

impl CaltransCameraProbe for CaltransHttpProbe {
    fn fetch_catalog(&self, district: u8, point: CameraContext) -> io::Result<CatalogResponse> {
        validate_district(district)?;
        validate_context(point)?;
        let url = catalog_url(district);
        let mut request = self.client.get(&url);
        let mut sent_validator = false;
        {
            let validators = self
                .validators
                .lock()
                .map_err(|_| io::Error::other("Caltrans validator lock poisoned"))?;
            if validators.district == Some(district)
                && validators
                    .point
                    .is_some_and(|prior| point_near(prior, point))
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
        let response = request.send().map_err(io_other)?;
        if response.status() == reqwest::StatusCode::NOT_MODIFIED {
            return if sent_validator {
                Ok(CatalogResponse::NotModified)
            } else {
                Err(io::Error::other(
                    "Caltrans returned 304 although no matching-point validator was sent",
                ))
            };
        }
        require_status_and_type(&response, &["application/json"], MAX_CATALOG_BODY_BYTES)?;
        let etag = header_string(&response, ETAG);
        let last_modified = header_string(&response, LAST_MODIFIED);
        let modified_at_ms = last_modified.as_deref().and_then(parse_http_time_ms);
        let mut response = response;
        let body = read_bounded_string(&mut response, MAX_CATALOG_BODY_BYTES)?;
        *self
            .validators
            .lock()
            .map_err(|_| io::Error::other("Caltrans validator lock poisoned"))? = Validators {
            district: Some(district),
            point: Some(point),
            etag,
            last_modified,
        };
        Ok(CatalogResponse::Modified {
            body,
            modified_at_ms,
        })
    }

    fn fetch_thumbnail(&self, district: u8, url: &str) -> io::Result<ThumbnailResponse> {
        validate_image_url(district, url)?;
        let response = self.client.get(url).send().map_err(io_other)?;
        require_status_and_type(&response, &["image/jpeg"], MAX_THUMBNAIL_BYTES)?;
        let observed_at_ms = header_string(&response, LAST_MODIFIED)
            .as_deref()
            .and_then(parse_http_time_ms)
            .unwrap_or_else(now_ms);
        let mut response = response;
        let jpeg = read_bounded_bytes(&mut response, MAX_THUMBNAIL_BYTES)?;
        if jpeg.len() < 4 || !jpeg.starts_with(&[0xFF, 0xD8]) || !jpeg.ends_with(&[0xFF, 0xD9]) {
            return Err(io::Error::other(
                "Caltrans thumbnail is not a complete JPEG",
            ));
        }
        Ok(ThumbnailResponse {
            jpeg,
            observed_at_ms,
        })
    }
}

fn catalog_url(district: u8) -> String {
    format!("https://cwwp2.dot.ca.gov/data/d{district}/cctv/cctvStatusD{district:02}.json")
}

fn require_status_and_type(
    response: &reqwest::blocking::Response,
    content_types: &[&str],
    max_bytes: usize,
) -> io::Result<()> {
    if response.status() != reqwest::StatusCode::OK {
        return Err(io::Error::other(format!(
            "Caltrans returned unexpected HTTP {} (redirects are disabled)",
            response.status()
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
    if !content_types.contains(&content_type) {
        return Err(io::Error::other(format!(
            "Caltrans returned unexpected content type {content_type:?}"
        )));
    }
    if response
        .content_length()
        .is_some_and(|length| length > u64::try_from(max_bytes).unwrap_or(u64::MAX))
    {
        return Err(io::Error::other(format!(
            "Caltrans response exceeds {max_bytes} byte limit"
        )));
    }
    Ok(())
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

fn read_bounded_string(reader: &mut impl Read, max_bytes: usize) -> io::Result<String> {
    String::from_utf8(read_bounded_bytes(reader, max_bytes)?).map_err(io_other)
}

fn read_bounded_bytes(reader: &mut impl Read, max_bytes: usize) -> io::Result<Vec<u8>> {
    let mut body = Vec::with_capacity(max_bytes.min(96 * 1024));
    reader
        .take(u64::try_from(max_bytes).unwrap_or(u64::MAX) + 1)
        .read_to_end(&mut body)?;
    if body.len() > max_bytes {
        return Err(io::Error::other(format!(
            "Caltrans response exceeds {max_bytes} byte limit"
        )));
    }
    Ok(body)
}

fn io_other(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

#[derive(Debug, Deserialize)]
struct CatalogDocument {
    #[serde(default)]
    data: Vec<CatalogEntry>,
}

#[derive(Debug, Deserialize)]
struct CatalogEntry {
    cctv: CameraDocument,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CameraDocument {
    index: String,
    #[serde(default)]
    record_timestamp: RecordTimestamp,
    location: LocationDocument,
    in_service: String,
    image_data: ImageDataDocument,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecordTimestamp {
    #[serde(default)]
    record_epoch: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocationDocument {
    district: String,
    location_name: String,
    #[serde(default)]
    nearby_place: String,
    longitude: String,
    latitude: String,
    #[serde(default)]
    direction: String,
    #[serde(default)]
    county: String,
    #[serde(default)]
    route: String,
}

#[derive(Debug, Deserialize)]
struct ImageDataDocument {
    #[serde(rename = "static")]
    still: StaticImageDocument,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StaticImageDocument {
    current_image_update_frequency: String,
    #[serde(rename = "currentImageURL")]
    current_image_url: String,
}

fn build_snapshot(
    probe: &dyn CaltransCameraProbe,
    host: &str,
    district: u8,
    context: CameraContext,
    body: &str,
    modified_at_ms: Option<i64>,
    fetched_at_ms: i64,
) -> io::Result<CaltransCameraSnapshot> {
    validate_district(district)?;
    validate_context(context)?;
    if body.len() > MAX_CATALOG_BODY_BYTES {
        return Err(io::Error::other("Caltrans catalog exceeds byte limit"));
    }
    let document: CatalogDocument = serde_json::from_str(body).map_err(io_other)?;
    let mut snapshot = CaltransCameraSnapshot::empty(
        host,
        district,
        fetched_at_ms,
        context.latitude,
        context.longitude,
    );
    snapshot.catalog_modified_at_ms = modified_at_ms;
    snapshot.query_heading_deg = context.heading_deg;
    snapshot.feed_total = u32::try_from(document.data.len()).unwrap_or(u32::MAX);
    if document.data.len() > MAX_FEED_CAMERAS {
        push_gap(
            &mut snapshot.gaps,
            format!(
                "catalog contains {} cameras; only the first {MAX_FEED_CAMERAS} are processed",
                document.data.len()
            ),
        );
    }
    let mut ids = HashSet::new();
    for (index, entry) in document.data.into_iter().take(MAX_FEED_CAMERAS).enumerate() {
        match parse_camera(entry.cctv, district, context) {
            Ok(Some(camera)) => {
                if ids.insert(camera.id.clone()) {
                    snapshot.cameras.push(camera);
                } else {
                    snapshot.quality_filtered = snapshot.quality_filtered.saturating_add(1);
                    push_gap(
                        &mut snapshot.gaps,
                        format!("camera {index} duplicate id omitted"),
                    );
                }
            }
            Ok(None) => {
                snapshot.relevance_filtered = snapshot.relevance_filtered.saturating_add(1);
            }
            Err(error) => {
                snapshot.quality_filtered = snapshot.quality_filtered.saturating_add(1);
                push_gap(
                    &mut snapshot.gaps,
                    format!("camera {index} omitted: {error}"),
                );
            }
        }
    }
    snapshot
        .cameras
        .sort_by(|a, b| a.distance_nm.total_cmp(&b.distance_nm));
    if snapshot.cameras.len() > MAX_RETAINED_CAMERAS {
        snapshot.cameras.truncate(MAX_RETAINED_CAMERAS);
        push_gap(
            &mut snapshot.gaps,
            format!("nearby cameras capped at {MAX_RETAINED_CAMERAS}"),
        );
    }
    refresh_thumbnails(probe, &mut snapshot, context);
    Ok(snapshot)
}

fn parse_camera(
    camera: CameraDocument,
    district: u8,
    context: CameraContext,
) -> io::Result<Option<CaltransCamera>> {
    if camera.location.district.trim().parse::<u8>().ok() != Some(district) {
        return Err(io::Error::other("district mismatch"));
    }
    let id =
        bounded_ascii(&camera.index, 32).ok_or_else(|| io::Error::other("invalid camera id"))?;
    let name = bounded_ascii(&camera.location.location_name, MAX_STRING_BYTES)
        .ok_or_else(|| io::Error::other("invalid location name"))?;
    let latitude = parse_coordinate(&camera.location.latitude, -90.0, 90.0)?;
    let longitude = parse_coordinate(&camera.location.longitude, -180.0, 180.0)?;
    let distance_nm = great_circle_nm(context.latitude, context.longitude, latitude, longitude);
    if distance_nm > RELEVANCE_RADIUS_NM {
        return Ok(None);
    }
    let in_service = match camera.in_service.trim() {
        "true" => true,
        "false" => false,
        _ => return Err(io::Error::other("invalid inService value")),
    };
    validate_image_url(district, &camera.image_data.still.current_image_url)?;
    let record_at_ms = camera
        .record_timestamp
        .record_epoch
        .trim()
        .parse::<i64>()
        .ok()
        .and_then(|seconds| seconds.checked_mul(1_000));
    let image_update_minutes = camera
        .image_data
        .still
        .current_image_update_frequency
        .trim()
        .parse::<u16>()
        .ok()
        .filter(|minutes| *minutes <= 24 * 60);
    Ok(Some(CaltransCamera {
        id,
        name,
        nearby_place: bounded_optional(&camera.location.nearby_place),
        county: bounded_optional(&camera.location.county),
        route: bounded_optional(&camera.location.route),
        direction: bounded_optional(&camera.location.direction),
        latitude,
        longitude,
        distance_nm: distance_nm as f32,
        in_service,
        record_at_ms,
        image_url: camera.image_data.still.current_image_url,
        image_update_minutes,
        thumbnail: None,
    }))
}

fn refresh_thumbnails(
    probe: &dyn CaltransCameraProbe,
    snapshot: &mut CaltransCameraSnapshot,
    context: CameraContext,
) {
    snapshot.query_heading_deg = context.heading_deg;
    snapshot.gaps.retain(|gap| !gap.starts_with("thumbnail "));
    let selected: Vec<usize> = snapshot
        .cameras
        .iter()
        .enumerate()
        .filter(|(_, camera)| {
            camera.in_service
                && camera.distance_nm <= THUMBNAIL_RADIUS_NM
                && context.heading_deg.is_none_or(|heading| {
                    let bearing = initial_bearing_deg(
                        context.latitude,
                        context.longitude,
                        camera.latitude,
                        camera.longitude,
                    );
                    angle_difference_deg(f64::from(heading), bearing) <= AHEAD_CONE_DEG
                })
        })
        .map(|(index, _)| index)
        .take(MAX_THUMBNAILS)
        .collect();
    let selected_set: HashSet<usize> = selected.iter().copied().collect();
    for (index, camera) in snapshot.cameras.iter_mut().enumerate() {
        if !selected_set.contains(&index) {
            camera.thumbnail = None;
        }
    }
    for index in selected {
        let camera_id = snapshot.cameras[index].id.clone();
        let image_url = snapshot.cameras[index].image_url.clone();
        match probe.fetch_thumbnail(snapshot.district, &image_url) {
            Ok(response) => {
                snapshot.cameras[index].thumbnail = Some(CameraThumbnail {
                    observed_at_ms: response.observed_at_ms,
                    jpeg_base64: base64::engine::general_purpose::STANDARD.encode(response.jpeg),
                });
            }
            Err(error) => push_gap(
                &mut snapshot.gaps,
                format!("thumbnail {camera_id:?} refresh failed: {error}"),
            ),
        }
    }
}

fn bounded_optional(value: &str) -> Option<String> {
    bounded_ascii(value, MAX_STRING_BYTES)
}

fn bounded_ascii(value: &str, max_bytes: usize) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()
        && value.len() <= max_bytes
        && value
            .chars()
            .all(|character| character.is_ascii_graphic() || character == ' '))
    .then(|| value.to_string())
}

fn parse_coordinate(value: &str, min: f64, max: f64) -> io::Result<f64> {
    value
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && (min..=max).contains(value))
        .ok_or_else(|| io::Error::other("coordinate is not finite/in range"))
}

fn validate_district(district: u8) -> io::Result<()> {
    (1..=12)
        .contains(&district)
        .then_some(())
        .ok_or_else(|| io::Error::other("Caltrans district must be 1 through 12"))
}

fn validate_context(context: CameraContext) -> io::Result<()> {
    if context.latitude.is_finite()
        && context.longitude.is_finite()
        && (32.0..=42.2).contains(&context.latitude)
        && (-124.6..=-114.0).contains(&context.longitude)
        && context
            .heading_deg
            .is_none_or(|heading| heading.is_finite() && (0.0..360.0).contains(&heading))
    {
        Ok(())
    } else {
        Err(io::Error::other(
            "fresh vehicle point is outside finite California coverage",
        ))
    }
}

fn validate_image_url(district: u8, value: &str) -> io::Result<reqwest::Url> {
    validate_district(district)?;
    let url = reqwest::Url::parse(value).map_err(io_other)?;
    if url.scheme() != "https"
        || url.host_str() != Some("cwwp2.dot.ca.gov")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(io::Error::other(
            "Caltrans image URL is outside the strict official-host allowlist",
        ));
    }
    let segments: Vec<_> = url
        .path_segments()
        .ok_or_else(|| io::Error::other("Caltrans image URL has no path"))?
        .collect();
    let expected_district = format!("d{district}");
    if segments.len() < 6
        || segments[0] != "data"
        || segments[1] != expected_district
        || segments[2] != "cctv"
        || segments[3] != "image"
        || !segments.last().is_some_and(|name| {
            name.to_ascii_lowercase().ends_with(".jpg")
                || name.to_ascii_lowercase().ends_with(".jpeg")
        })
        || segments.iter().any(|segment| {
            segment.is_empty()
                || segment.len() > MAX_STRING_BYTES
                || !segment.bytes().all(|byte| byte.is_ascii_graphic())
        })
    {
        return Err(io::Error::other("Caltrans image URL path is not canonical"));
    }
    Ok(url)
}

fn parse_http_time_ms(value: &str) -> Option<i64> {
    let normalized = value
        .strip_suffix(" GMT")
        .map_or_else(|| value.to_string(), |prefix| format!("{prefix} +0000"));
    chrono::DateTime::parse_from_rfc2822(&normalized)
        .ok()
        .map(|time| time.timestamp_millis())
}

fn point_near(a: CameraContext, b: CameraContext) -> bool {
    great_circle_nm(a.latitude, a.longitude, b.latitude, b.longitude) <= 0.05
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

fn initial_bearing_deg(lat_a: f64, lon_a: f64, lat_b: f64, lon_b: f64) -> f64 {
    let lat_a = lat_a.to_radians();
    let lat_b = lat_b.to_radians();
    let delta_lon = (lon_b - lon_a).to_radians();
    let y = delta_lon.sin() * lat_b.cos();
    let x = lat_a.cos() * lat_b.sin() - lat_a.sin() * lat_b.cos() * delta_lon.cos();
    (y.atan2(x).to_degrees() + 360.0) % 360.0
}

fn angle_difference_deg(a: f64, b: f64) -> f64 {
    ((a - b + 180.0).rem_euclid(360.0) - 180.0).abs()
}

fn push_gap(gaps: &mut Vec<String>, gap: String) {
    if gaps.len() < MAX_GAPS {
        gaps.push(gap);
    } else if gaps
        .last()
        .is_some_and(|last| last != "additional camera gaps omitted")
    {
        gaps[MAX_GAPS - 1] = "additional camera gaps omitted".to_string();
    }
}

enum PreparedResponse {
    Modified(CaltransCameraSnapshot),
}

/// Workstation-side Caltrans traffic-camera adapter.
pub struct CaltransCameraOverlayWorker {
    host: String,
    district: Option<u8>,
    probe: Option<Arc<dyn CaltransCameraProbe>>,
    bus_root: Option<PathBuf>,
    poll: Duration,
}

impl CaltransCameraOverlayWorker {
    /// Production wiring. Disabled unless explicitly opted in with a district.
    #[must_use]
    pub fn new(host: String) -> Self {
        let district = std::env::var(DISTRICT_ENV)
            .ok()
            .and_then(|value| value.trim().parse::<u8>().ok())
            .filter(|district| (1..=12).contains(district));
        let probe = if env_truthy(ENABLED_ENV) && district.is_some() {
            match CaltransHttpProbe::new() {
                Ok(probe) => Some(Arc::new(probe) as Arc<dyn CaltransCameraProbe>),
                Err(error) => {
                    tracing::warn!(target: "mackesd::caltrans_camera_overlay", %error, "Caltrans client unavailable; worker idle");
                    None
                }
            }
        } else {
            if env_truthy(ENABLED_ENV) && district.is_none() {
                tracing::warn!(target: "mackesd::caltrans_camera_overlay", "Caltrans camera overlay requires MDE_OVERLAY_CALTRANS_DISTRICT=1..12; worker idle");
            }
            None
        };
        Self {
            host,
            district,
            probe,
            bus_root: crate::bus_publish::default_bus_root(),
            poll: POLL,
        }
    }

    /// Inject a fixture probe and district.
    #[must_use]
    pub fn with_probe(mut self, district: u8, probe: Arc<dyn CaltransCameraProbe>) -> Self {
        self.district = Some(district);
        self.probe = Some(probe);
        self
    }

    /// Override or disable Bus access.
    #[must_use]
    pub fn with_bus_root(mut self, root: Option<PathBuf>) -> Self {
        self.bus_root = root;
        self
    }

    fn current_context(&self) -> Option<CameraContext> {
        let root = self.bus_root.clone()?;
        let persist = mde_bus::persist::Persist::open(root).ok()?;
        let topic = mackes_mesh_types::vehicle::vehicle_state_topic(&self.host);
        let body = persist.read_latest(&topic).ok().flatten()?.body?;
        let vehicle: mackes_mesh_types::vehicle::VehicleState = serde_json::from_str(&body).ok()?;
        validated_vehicle_context(&vehicle, &self.host, now_ms())
    }

    fn publish(&self, snapshot: &CaltransCameraSnapshot) {
        if let Some(mut persist) = crate::bus_publish::open_bus(self.bus_root.clone()) {
            crate::bus_publish::publish_json(
                &mut persist,
                &caltrans_camera_state_topic(&self.host),
                snapshot,
            );
        }
    }

    fn apply_result(
        &self,
        result: io::Result<PreparedResponse>,
        last_good: &mut Option<CaltransCameraSnapshot>,
    ) -> bool {
        match result {
            Ok(PreparedResponse::Modified(snapshot)) => {
                self.publish(&snapshot);
                *last_good = Some(snapshot);
                true
            }
            Err(error) => {
                if let Some(snapshot) = last_good {
                    snapshot
                        .gaps
                        .retain(|gap| !gap.starts_with("Caltrans cameras paused:"));
                    push_gap(
                        &mut snapshot.gaps,
                        format!("Caltrans cameras paused: {error}"),
                    );
                    self.publish(snapshot);
                }
                false
            }
        }
    }

    async fn fetch_async(
        &self,
        probe: Arc<dyn CaltransCameraProbe>,
        district: u8,
        context: CameraContext,
        previous: Option<CaltransCameraSnapshot>,
        shutdown: &mut ShutdownToken,
    ) -> Option<io::Result<PreparedResponse>> {
        let host = self.host.clone();
        let task = tokio::task::spawn_blocking(move || {
            let fetched_at_ms = now_ms();
            let snapshot = match probe.fetch_catalog(district, context)? {
                CatalogResponse::Modified {
                    body,
                    modified_at_ms,
                } => build_snapshot(
                    probe.as_ref(),
                    &host,
                    district,
                    context,
                    &body,
                    modified_at_ms,
                    fetched_at_ms,
                )?,
                CatalogResponse::NotModified => {
                    let mut snapshot = previous.ok_or_else(|| {
                        io::Error::other("Caltrans returned 304 before a last-good snapshot")
                    })?;
                    if snapshot.district != district
                        || !point_near(
                            CameraContext {
                                latitude: snapshot.query_latitude,
                                longitude: snapshot.query_longitude,
                                heading_deg: snapshot.query_heading_deg,
                            },
                            context,
                        )
                    {
                        return Err(io::Error::other(
                            "Caltrans 304 district/point does not match last-good",
                        ));
                    }
                    snapshot.fetched_at_ms = fetched_at_ms;
                    snapshot.query_heading_deg = context.heading_deg;
                    snapshot
                        .gaps
                        .retain(|gap| !gap.starts_with("Caltrans cameras paused:"));
                    refresh_thumbnails(probe.as_ref(), &mut snapshot, context);
                    snapshot
                }
            };
            Ok(PreparedResponse::Modified(snapshot))
        });
        tokio::select! {
            () = shutdown.wait() => None,
            joined = task => Some(match joined {
                Ok(result) => result,
                Err(error) => Err(io::Error::other(format!("Caltrans fetch task failed: {error}"))),
            }),
        }
    }
}

fn validated_vehicle_context(
    vehicle: &mackes_mesh_types::vehicle::VehicleState,
    expected_host: &str,
    now: i64,
) -> Option<CameraContext> {
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
    let heading_deg = (gps.speed_mph.is_finite()
        && gps.speed_mph >= 5.0
        && gps.heading_deg.is_finite()
        && (0.0..360.0).contains(&gps.heading_deg))
    .then_some(gps.heading_deg);
    let context = CameraContext {
        latitude: gps.latitude,
        longitude: gps.longitude,
        heading_deg,
    };
    validate_context(context).ok().map(|()| context)
}

#[async_trait::async_trait]
impl Worker for CaltransCameraOverlayWorker {
    fn name(&self) -> &'static str {
        "caltrans_camera_overlay"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let (Some(probe), Some(district)) = (self.probe.clone(), self.district) else {
            shutdown.wait().await;
            return Ok(());
        };
        let mut last_good = None;
        let mut retry = self.poll;
        let mut no_fix_published = false;
        loop {
            let Some(context) = self.current_context() else {
                if !no_fix_published {
                    self.apply_result(
                        Err(io::Error::other(
                            "fresh same-host California vehicle fix unavailable",
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
                .fetch_async(
                    probe.clone(),
                    district,
                    context,
                    last_good.clone(),
                    &mut shutdown,
                )
                .await
            else {
                break;
            };
            let success = self.apply_result(result, &mut last_good);
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
    use serde_json::json;

    use super::*;

    const NOW_MS: i64 = 1_784_757_600_000;
    const IMAGE_URL: &str =
        "https://cwwp2.dot.ca.gov/data/d3/cctv/image/hwy5atpocket/hwy5atpocket.jpg";

    fn context() -> CameraContext {
        CameraContext {
            latitude: 38.481,
            longitude: -121.511,
            heading_deg: None,
        }
    }

    fn camera(index: usize, latitude: &str, longitude: &str) -> serde_json::Value {
        json!({"cctv": {
            "index": index.to_string(),
            "recordTimestamp": {"recordEpoch":"1778637079"},
            "location": {
                "district":"3", "locationName":"Hwy 5 at Pocket",
                "nearbyPlace":"Sacramento", "longitude":longitude,
                "latitude":latitude, "direction":"Median",
                "county":"Sacramento", "route":"I-5"
            },
            "inService":"true",
            "imageData": {"static": {
                "currentImageUpdateFrequency":"1", "currentImageURL":IMAGE_URL
            }}
        }})
    }

    fn catalog(entries: Vec<serde_json::Value>) -> String {
        json!({"data":entries}).to_string()
    }

    struct FakeProbe;

    impl CaltransCameraProbe for FakeProbe {
        fn fetch_catalog(
            &self,
            _district: u8,
            _point: CameraContext,
        ) -> io::Result<CatalogResponse> {
            Ok(CatalogResponse::Modified {
                body: catalog(vec![camera(1, "38.481128", "-121.510528")]),
                modified_at_ms: Some(NOW_MS - 60_000),
            })
        }

        fn fetch_thumbnail(&self, _district: u8, _url: &str) -> io::Result<ThumbnailResponse> {
            Ok(ThumbnailResponse {
                jpeg: vec![0xFF, 0xD8, 0xFF, 0xD9],
                observed_at_ms: NOW_MS - 20_000,
            })
        }
    }

    #[test]
    fn captured_live_schema_filters_and_embeds_bounded_current_still() {
        let body = catalog(vec![
            camera(1, "38.481128", "-121.510528"),
            camera(2, "35.0", "-120.0"),
        ]);
        let snapshot = build_snapshot(
            &FakeProbe,
            "rig-1",
            3,
            context(),
            &body,
            Some(NOW_MS - 60_000),
            NOW_MS,
        )
        .expect("snapshot");
        assert_eq!(snapshot.feed_total, 2);
        assert_eq!(snapshot.cameras.len(), 1);
        assert_eq!(snapshot.relevance_filtered, 1);
        assert!(snapshot.cameras[0].thumbnail.is_some());
        assert_eq!(snapshot.cameras[0].route.as_deref(), Some("I-5"));
    }

    #[test]
    fn image_url_allowlist_rejects_host_port_query_and_district_substitution() {
        assert!(validate_image_url(3, IMAGE_URL).is_ok());
        for hostile in [
            "http://cwwp2.dot.ca.gov/data/d3/cctv/image/x/x.jpg",
            "https://cwwp2.dot.ca.gov.evil.test/data/d3/cctv/image/x/x.jpg",
            "https://cwwp2.dot.ca.gov:444/data/d3/cctv/image/x/x.jpg",
            "https://cwwp2.dot.ca.gov/data/d4/cctv/image/x/x.jpg",
            "https://cwwp2.dot.ca.gov/data/d3/cctv/image/x/x.jpg?q=1",
        ] {
            assert!(
                validate_image_url(3, hostile).is_err(),
                "accepted {hostile}"
            );
        }
    }

    #[test]
    fn body_cardinality_strings_nan_and_retention_are_bounded() {
        assert!(read_bounded_string(
            &mut io::Cursor::new(vec![b'x'; MAX_CATALOG_BODY_BYTES + 1]),
            MAX_CATALOG_BODY_BYTES
        )
        .is_err());
        let mut entries: Vec<_> = (0..=MAX_FEED_CAMERAS)
            .map(|index| camera(index, "38.481128", "-121.510528"))
            .collect();
        entries[0]["cctv"]["location"]["locationName"] = json!("x".repeat(MAX_STRING_BYTES + 1));
        entries[1]["cctv"]["location"]["latitude"] = json!("NaN");
        let body = catalog(entries);
        assert!(body.len() <= MAX_CATALOG_BODY_BYTES);
        let snapshot = build_snapshot(&FakeProbe, "rig-1", 3, context(), &body, None, NOW_MS)
            .expect("bounded");
        assert_eq!(snapshot.cameras.len(), MAX_RETAINED_CAMERAS);
        assert!(snapshot.gaps.iter().any(|gap| gap.contains("first 2000")));
        assert!(snapshot
            .gaps
            .iter()
            .any(|gap| gap.contains("capped at 128")));
        assert!(snapshot.quality_filtered >= 2);
        assert!(snapshot.gaps.len() <= MAX_GAPS);
    }

    #[test]
    fn vehicle_context_requires_fresh_same_host_california_fix() {
        let mut vehicle = VehicleState::offline("rig-1");
        vehicle.online = true;
        vehicle.published_at_ms = 100_000;
        vehicle.gps = GpsFix {
            fix_type: "gps".to_string(),
            latitude: 38.481,
            longitude: -121.511,
            satellites: 8,
            age_s: 1.0,
            ..GpsFix::default()
        };
        assert!(validated_vehicle_context(&vehicle, "rig-1", 110_000).is_some());
        assert!(validated_vehicle_context(&vehicle, "other", 110_000).is_none());
        vehicle.gps.latitude = f64::NAN;
        assert!(validated_vehicle_context(&vehicle, "rig-1", 100_000).is_none());
        vehicle.gps.latitude = 42.3601;
        vehicle.gps.longitude = -71.0589;
        assert!(validated_vehicle_context(&vehicle, "rig-1", 100_000).is_none());
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
            .get(format!("http://{redirect_addr}/catalog"))
            .send()
            .expect("response");
        let error = require_status_and_type(&response, &["application/json"], 1)
            .expect_err("redirect rejected");
        assert!(error.to_string().contains("redirects are disabled"));
        redirect_thread.join().expect("redirect join");
        target_thread.join().expect("target join");
        assert!(!contacted.load(Ordering::Relaxed));
    }

    #[test]
    fn failed_refresh_retains_timestamp_and_publishes_paused_latest_wins_state() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().to_path_buf();
        let worker =
            CaltransCameraOverlayWorker::new("rig-1".to_string()).with_bus_root(Some(root.clone()));
        let original = build_snapshot(
            &FakeProbe,
            "rig-1",
            3,
            context(),
            &catalog(vec![camera(1, "38.481128", "-121.510528")]),
            Some(NOW_MS - 60_000),
            NOW_MS,
        )
        .expect("snapshot");
        let mut last_good = None;
        assert!(worker.apply_result(Ok(PreparedResponse::Modified(original)), &mut last_good,));
        assert!(!worker.apply_result(
            Err(io::Error::other("fresh vehicle fix unavailable")),
            &mut last_good,
        ));
        let retained = last_good.as_ref().expect("retained");
        assert_eq!(retained.fetched_at_ms, NOW_MS);
        assert!(retained
            .gaps
            .iter()
            .any(|gap| gap.starts_with("Caltrans cameras paused:")));
        let persist = Persist::open(root).expect("bus");
        let rows = persist
            .list_since(&caltrans_camera_state_topic("rig-1"), None)
            .expect("rows");
        assert_eq!(rows.len(), 2);
    }

    struct SlowProbe;

    impl CaltransCameraProbe for SlowProbe {
        fn fetch_catalog(
            &self,
            _district: u8,
            _point: CameraContext,
        ) -> io::Result<CatalogResponse> {
            std::thread::sleep(Duration::from_millis(500));
            FakeProbe.fetch_catalog(3, context())
        }

        fn fetch_thumbnail(&self, district: u8, url: &str) -> io::Result<ThumbnailResponse> {
            FakeProbe.fetch_thumbnail(district, url)
        }
    }

    #[tokio::test]
    async fn shutdown_wins_while_catalog_is_in_flight() {
        let worker = CaltransCameraOverlayWorker::new("rig-1".to_string()).with_bus_root(None);
        let (tx, rx) = tokio::sync::watch::channel(false);
        let mut shutdown = ShutdownToken::from_receiver(rx);
        let sender = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            tx.send(true).expect("shutdown");
        });
        let result = tokio::time::timeout(
            Duration::from_millis(200),
            worker.fetch_async(Arc::new(SlowProbe), 3, context(), None, &mut shutdown),
        )
        .await
        .expect("responsive");
        assert!(result.is_none());
        sender.await.expect("sender");
    }
}
