//! WL-FUNC-017 S4 — daemon-owned nowCOAST atmospheric map authority.
//!
//! Maps may submit one bounded latest-wins Web-Mercator viewport per host; the
//! daemon admits it against the exact effective-location generation and retains
//! a deterministic location-derived fallback. Temperature, wind, and
//! cloud-cover images come from the exact official NOAA nowCOAST NDFD forecast
//! products. Provider and cache work runs on Tokio's blocking pool; Bus handles
//! and writes remain on the worker task side.

#![cfg(feature = "async-services")]

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use mackes_mesh_types::location::{
    weather_location_state_topic, EffectiveLocationSnapshot, EffectiveLocationState,
    EffectiveWeatherLocation,
};
use mackes_mesh_types::nws_alert::GeoPoint;
use mackes_mesh_types::weather::{
    weather_map_state_topic, weather_map_viewport_state_topic, weather_set_map_viewport_topic,
    AtmosphericFieldImage, AtmosphericFieldKind, AtmosphericMapSnapshot, AtmosphericViewport,
    SetWeatherMapViewportRequest, WeatherAttribution, WeatherAvailability,
    WeatherMapViewportSource, WeatherMapViewportState, WeatherProvider, WeatherStaleReason,
    WeatherUnavailableReason, ATMOSPHERIC_FIELD_EDGE, MAX_ATMOSPHERIC_CACHE_AGE_MS,
    MAX_ATMOSPHERIC_FIELD_PNG_BYTES, MAX_ATMOSPHERIC_FRESH_AGE_MS, WEATHER_CONTRACT_SCHEMA_VERSION,
};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use reqwest::blocking::Client;
use reqwest::header::CONTENT_TYPE;
use serde::{Deserialize, Serialize};

use super::{ShutdownToken, Worker};

const POLL: Duration = Duration::from_secs(10 * 60);
const AUTHORITY_POLL: Duration = Duration::from_secs(2);
const RETRY_INITIAL: Duration = Duration::from_secs(30);
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_VIEWPORT_ACTION_AGE_MS: i64 = 5 * 60 * 1_000;
const TILE_ZOOM: u8 = 6;
const CACHE_SCHEMA_VERSION: u16 = 1;
const MAX_CACHE_BYTES: usize = 2 * 1024 * 1024;
const DEFAULT_CACHE_PATH: &str = "/var/lib/mackesd/weather-atmosphere-cache.json";
const CACHE_PATH_ENV: &str = "MDE_WEATHER_ATMOSPHERE_CACHE_PATH";
const OFFICIAL_HOST: &str = "nowcoast.noaa.gov";
const WEB_MERCATOR_LIMIT: f64 = 20_037_508.342_789_244;
const USER_AGENT: &str =
    "Construct/12 mackesd nowCOAST-atmosphere (+https://github.com/matthewmackes/magic-mesh)";
static CACHE_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

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

trait AtmosphericProbe: Send + Sync {
    fn fetch_field(
        &self,
        field: AtmosphericFieldKind,
        viewport: &AtmosphericViewport,
    ) -> io::Result<Vec<u8>>;
}

struct NowCoastHttpProbe {
    client: Client,
}

impl NowCoastHttpProbe {
    fn new() -> io::Result<Self> {
        let client = Client::builder()
            .timeout(HTTP_TIMEOUT)
            .user_agent(USER_AGENT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(io_other)?;
        Ok(Self { client })
    }
}

impl AtmosphericProbe for NowCoastHttpProbe {
    fn fetch_field(
        &self,
        field: AtmosphericFieldKind,
        viewport: &AtmosphericViewport,
    ) -> io::Result<Vec<u8>> {
        let url = get_map_url(field, viewport)?;
        validate_get_map_url(url.as_str(), field, viewport)?;
        let response = self.client.get(url).send().map_err(io_other)?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(io::Error::other(format!(
                "nowCOAST returned HTTP {} (redirects are disabled)",
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
        if content_type != "image/png" {
            return Err(io::Error::other("nowCOAST returned a non-PNG content type"));
        }
        if response.content_length().is_some_and(|length| {
            length > u64::try_from(MAX_ATMOSPHERIC_FIELD_PNG_BYTES).unwrap_or(u64::MAX)
        }) {
            return Err(io::Error::other("nowCOAST PNG exceeds its byte limit"));
        }
        let mut response = response;
        let png = read_bounded(&mut response, MAX_ATMOSPHERIC_FIELD_PNG_BYTES)?;
        validate_png(&png)?;
        Ok(png)
    }
}

fn get_map_url(
    field: AtmosphericFieldKind,
    viewport: &AtmosphericViewport,
) -> io::Result<reqwest::Url> {
    let (min_x, min_y, max_x, max_y) = viewport_bbox(viewport)?;
    let (service_path, layer_name) = field.nowcoast_product();
    let mut url =
        reqwest::Url::parse(&format!("https://{OFFICIAL_HOST}{service_path}")).map_err(io_other)?;
    url.query_pairs_mut()
        .append_pair("service", "WMS")
        .append_pair("version", "1.3.0")
        .append_pair("request", "GetMap")
        .append_pair("layers", layer_name)
        .append_pair("styles", "")
        .append_pair("crs", "EPSG:3857")
        .append_pair(
            "bbox",
            &format!("{min_x:.3},{min_y:.3},{max_x:.3},{max_y:.3}"),
        )
        .append_pair("width", &viewport.pixel_width.to_string())
        .append_pair("height", &viewport.pixel_height.to_string())
        .append_pair("format", "image/png")
        .append_pair("transparent", "TRUE");
    Ok(url)
}

fn validate_get_map_url(
    value: &str,
    field: AtmosphericFieldKind,
    viewport: &AtmosphericViewport,
) -> io::Result<()> {
    let url = reqwest::Url::parse(value).map_err(io_other)?;
    let (service_path, layer_name) = field.nowcoast_product();
    if url.scheme() != "https"
        || url.host_str() != Some(OFFICIAL_HOST)
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.path() != service_path
    {
        return Err(io::Error::other(
            "nowCOAST URL is outside the strict official HTTPS allowlist",
        ));
    }
    let mut query = BTreeMap::new();
    for (key, value) in url.query_pairs() {
        if query.insert(key.into_owned(), value.into_owned()).is_some() {
            return Err(io::Error::other(
                "nowCOAST URL contains duplicate query keys",
            ));
        }
    }
    if query.len() != 11
        || query.get("service").map(String::as_str) != Some("WMS")
        || query.get("version").map(String::as_str) != Some("1.3.0")
        || query.get("request").map(String::as_str) != Some("GetMap")
        || query.get("layers").map(String::as_str) != Some(layer_name)
        || query.get("styles").map(String::as_str) != Some("")
        || query.get("crs").map(String::as_str) != Some("EPSG:3857")
        || query.get("width").map(String::as_str) != Some("256")
        || query.get("height").map(String::as_str) != Some("256")
        || query.get("format").map(String::as_str) != Some("image/png")
        || query.get("transparent").map(String::as_str) != Some("TRUE")
    {
        return Err(io::Error::other("nowCOAST URL query is not canonical"));
    }
    let expected = viewport_bbox(viewport)?;
    let actual: Vec<f64> = query
        .get("bbox")
        .ok_or_else(|| io::Error::other("nowCOAST URL has no bbox"))?
        .split(',')
        .map(str::parse::<f64>)
        .collect::<Result<_, _>>()
        .map_err(io_other)?;
    if actual.len() != 4
        || actual.iter().any(|value| !value.is_finite())
        || (actual[0] - expected.0).abs() > 0.01
        || (actual[1] - expected.1).abs() > 0.01
        || (actual[2] - expected.2).abs() > 0.01
        || (actual[3] - expected.3).abs() > 0.01
    {
        return Err(io::Error::other(
            "nowCOAST URL bbox is not the admitted viewport",
        ));
    }
    Ok(())
}

fn read_bounded(reader: &mut impl Read, cap: usize) -> io::Result<Vec<u8>> {
    let mut body = Vec::with_capacity(cap.min(64 * 1024));
    reader
        .take(u64::try_from(cap).unwrap_or(u64::MAX).saturating_add(1))
        .read_to_end(&mut body)?;
    if body.len() > cap {
        return Err(io::Error::other("nowCOAST PNG exceeds its byte limit"));
    }
    Ok(body)
}

fn validate_png(png: &[u8]) -> io::Result<()> {
    const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if png.len() < 41
        || png.len() > MAX_ATMOSPHERIC_FIELD_PNG_BYTES
        || !png.starts_with(SIGNATURE)
        || png.get(12..16) != Some(b"IHDR")
        || png.get(16..20).and_then(bytes_u32) != Some(u32::from(ATMOSPHERIC_FIELD_EDGE))
        || png.get(20..24).and_then(bytes_u32) != Some(u32::from(ATMOSPHERIC_FIELD_EDGE))
        || png.get(png.len().saturating_sub(8)..png.len().saturating_sub(4)) != Some(b"IEND")
    {
        return Err(io::Error::other(
            "nowCOAST field is not a complete admitted 256x256 PNG",
        ));
    }
    Ok(())
}

fn bytes_u32(bytes: &[u8]) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.try_into().ok()?))
}

fn fallback_viewport(
    location_generation: u64,
    point: &GeoPoint,
) -> io::Result<AtmosphericViewport> {
    if location_generation == 0 {
        return Err(io::Error::other("effective-location generation is invalid"));
    }
    let (x, y) = tile_xyz(point.latitude, point.longitude, TILE_ZOOM)?;
    Ok(AtmosphericViewport {
        // Deterministic fallback identity when no compatible Maps action has
        // been admitted. Maps viewport generations supersede this value.
        generation: location_generation,
        zoom: TILE_ZOOM,
        x,
        y,
        pixel_width: ATMOSPHERIC_FIELD_EDGE,
        pixel_height: ATMOSPHERIC_FIELD_EDGE,
    })
}

fn tile_xyz(latitude: f64, longitude: f64, zoom: u8) -> io::Result<(u32, u32)> {
    if !latitude.is_finite()
        || !longitude.is_finite()
        || !(-85.051_128_78..=85.051_128_78).contains(&latitude)
        || !(-180.0..180.0).contains(&longitude)
        || zoom > 20
    {
        return Err(io::Error::other(
            "atmospheric point is outside Web-Mercator",
        ));
    }
    let n = f64::from(1_u32 << zoom);
    let latitude = latitude.to_radians();
    let x = ((longitude + 180.0) / 360.0 * n).floor();
    let y = ((1.0 - (latitude.tan() + 1.0 / latitude.cos()).ln() / std::f64::consts::PI) / 2.0 * n)
        .floor();
    if x.is_finite() && y.is_finite() && x >= 0.0 && y >= 0.0 && x < n && y < n {
        Ok((x as u32, y as u32))
    } else {
        Err(io::Error::other("atmospheric viewport does not resolve"))
    }
}

fn viewport_bbox(viewport: &AtmosphericViewport) -> io::Result<(f64, f64, f64, f64)> {
    if viewport.zoom > 20
        || viewport.x >= (1_u32 << viewport.zoom)
        || viewport.y >= (1_u32 << viewport.zoom)
        || viewport.pixel_width != ATMOSPHERIC_FIELD_EDGE
        || viewport.pixel_height != ATMOSPHERIC_FIELD_EDGE
    {
        return Err(io::Error::other("atmospheric viewport is invalid"));
    }
    let tile_span = 2.0 * WEB_MERCATOR_LIMIT / f64::from(1_u32 << viewport.zoom);
    let min_x = -WEB_MERCATOR_LIMIT + f64::from(viewport.x) * tile_span;
    let max_x = min_x + tile_span;
    let max_y = WEB_MERCATOR_LIMIT - f64::from(viewport.y) * tile_span;
    let min_y = max_y - tile_span;
    Ok((min_x, min_y, max_x, max_y))
}

fn attribution() -> WeatherAttribution {
    WeatherAttribution {
        provider: WeatherProvider::NationalWeatherService,
        source_id: "nowcoast-ndfd".into(),
        label: "NOAA nowCOAST / NWS NDFD".into(),
    }
}

fn build_snapshot(
    probe: &dyn AtmosphericProbe,
    host: &str,
    location_generation: u64,
    point: &GeoPoint,
    viewport: &AtmosphericViewport,
    now_ms: i64,
) -> io::Result<AtmosphericMapSnapshot> {
    let mut fields = Vec::with_capacity(3);
    for kind in [
        AtmosphericFieldKind::Temperature,
        AtmosphericFieldKind::Wind,
        AtmosphericFieldKind::CloudCover,
    ] {
        let png = probe.fetch_field(kind, viewport)?;
        validate_png(&png)?;
        let (service_path, layer_name) = kind.nowcoast_product();
        fields.push(AtmosphericFieldImage {
            kind,
            provider_service_path: service_path.into(),
            provider_layer_name: layer_name.into(),
            pixel_width: ATMOSPHERIC_FIELD_EDGE,
            pixel_height: ATMOSPHERIC_FIELD_EDGE,
            png_base64: base64::engine::general_purpose::STANDARD.encode(png),
        });
    }
    let snapshot = AtmosphericMapSnapshot {
        schema_version: WEATHER_CONTRACT_SCHEMA_VERSION,
        host: host.into(),
        location_generation,
        location_point: point.clone(),
        viewport: viewport.clone(),
        // WMS GetMap exposes no data-valid timestamp in the image response. This
        // is named rendered_at, not provider_at, so freshness is not invented.
        rendered_at_ms: now_ms,
        fetched_at_ms: now_ms,
        availability: WeatherAvailability::Fresh,
        fields,
        gaps: vec![],
        attributions: vec![attribution()],
    };
    snapshot.validate_at(now_ms).map_err(io_other)?;
    Ok(snapshot)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AtmosphereCache {
    schema_version: u16,
    host: String,
    location_generation: u64,
    location_point: GeoPoint,
    viewport: AtmosphericViewport,
    snapshot: AtmosphericMapSnapshot,
}

impl AtmosphereCache {
    fn matches(
        &self,
        host: &str,
        location_generation: u64,
        point: &GeoPoint,
        viewport: &AtmosphericViewport,
    ) -> bool {
        self.schema_version == CACHE_SCHEMA_VERSION
            && self.host == host
            && self.location_generation == location_generation
            && self.location_point == *point
            && self.viewport == *viewport
            && self.snapshot.host == host
            && self.snapshot.location_generation == location_generation
            && self.snapshot.location_point == *point
            && self.snapshot.viewport == *viewport
            && self.snapshot.attributions.as_slice() == [attribution()]
    }
}

fn cache_path() -> PathBuf {
    std::env::var_os(CACHE_PATH_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_CACHE_PATH))
}

fn load_cache(path: &Path) -> io::Result<Option<AtmosphereCache>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(io::Error::other("atmospheric cache is not a regular file"));
    }
    if metadata.len() > u64::try_from(MAX_CACHE_BYTES).unwrap_or(u64::MAX) {
        return Err(io::Error::other("atmospheric cache exceeds its byte limit"));
    }
    let file: File = rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::NONBLOCK
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )?
    .into();
    let opened_metadata = file.metadata()?;
    if !opened_metadata.is_file()
        || opened_metadata.dev() != metadata.dev()
        || opened_metadata.ino() != metadata.ino()
    {
        return Err(io::Error::other(
            "atmospheric cache changed during secure open",
        ));
    }
    let mut body = Vec::with_capacity(metadata.len() as usize);
    file.take(u64::try_from(MAX_CACHE_BYTES).unwrap_or(u64::MAX) + 1)
        .read_to_end(&mut body)?;
    if body.len() > MAX_CACHE_BYTES {
        return Err(io::Error::other("atmospheric cache exceeds its byte limit"));
    }
    let text = std::str::from_utf8(&body).map_err(io_other)?;
    mackes_mesh_types::workloads::reject_duplicate_json_keys(text).map_err(io_other)?;
    let cache: AtmosphereCache = serde_json::from_slice(&body).map_err(io_other)?;
    if cache.schema_version != CACHE_SCHEMA_VERSION {
        return Err(io::Error::other("unsupported atmospheric cache schema"));
    }
    cache
        .snapshot
        .validate_at(cache.snapshot.fetched_at_ms)
        .map_err(io_other)?;
    Ok(Some(cache))
}

fn store_cache(path: &Path, cache: &AtmosphereCache) -> io::Result<()> {
    let body = serde_json::to_vec(cache).map_err(io_other)?;
    if body.len() > MAX_CACHE_BYTES {
        return Err(io::Error::other("atmospheric cache exceeds its byte limit"));
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| io::Error::other("atmospheric cache path has no parent"))?;
    fs::create_dir_all(parent)?;
    let metadata = fs::symlink_metadata(parent)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(io::Error::other("atmospheric cache parent is invalid"));
    }
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(io::Error::other("atmospheric cache path is a symlink"));
    }
    let sequence = CACHE_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(
        ".weather-atmosphere-{}-{sequence}.tmp",
        std::process::id()
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(&body)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn cached_projection(
    cache: &AtmosphereCache,
    host: &str,
    location_generation: u64,
    point: &GeoPoint,
    viewport: &AtmosphericViewport,
    now_ms: i64,
) -> Option<AtmosphericMapSnapshot> {
    if !cache.matches(host, location_generation, point, viewport) {
        return None;
    }
    let mut snapshot = cache.snapshot.clone();
    let age = now_ms.saturating_sub(snapshot.rendered_at_ms).max(0);
    if age > MAX_ATMOSPHERIC_CACHE_AGE_MS {
        return None;
    }
    snapshot.availability = if age <= MAX_ATMOSPHERIC_FRESH_AGE_MS {
        WeatherAvailability::Fresh
    } else {
        WeatherAvailability::Stale {
            reason: WeatherStaleReason::ProviderBackoff,
        }
    };
    snapshot.validate_at(now_ms).ok()?;
    Some(snapshot)
}

fn unavailable_snapshot(
    host: &str,
    location_generation: u64,
    point: &GeoPoint,
    viewport: &AtmosphericViewport,
    now_ms: i64,
    reason: WeatherUnavailableReason,
    gap: String,
) -> AtmosphericMapSnapshot {
    AtmosphericMapSnapshot {
        schema_version: WEATHER_CONTRACT_SCHEMA_VERSION,
        host: host.into(),
        location_generation,
        location_point: point.clone(),
        viewport: viewport.clone(),
        rendered_at_ms: now_ms,
        fetched_at_ms: now_ms,
        availability: WeatherAvailability::Unavailable { reason },
        fields: vec![],
        gaps: vec![gap],
        attributions: vec![attribution()],
    }
}

#[derive(Debug)]
struct BlockingResult {
    fresh: Result<AtmosphericMapSnapshot, String>,
    cache: Option<AtmosphereCache>,
    cache_error: Option<String>,
}

fn blocking_refresh(
    probe: Option<Arc<dyn AtmosphericProbe>>,
    host: &str,
    location_generation: u64,
    point: &GeoPoint,
    viewport: &AtmosphericViewport,
    now_ms: i64,
    cache_path: &Path,
) -> BlockingResult {
    let (cache, cache_error) = match load_cache_for_refresh(cache_path) {
        Ok(cache) => (cache, None),
        Err(error) => (None, Some(error.to_string())),
    };
    let fresh = probe
        .ok_or_else(|| "nowCOAST probe unavailable".to_string())
        .and_then(|probe| {
            build_snapshot(
                probe.as_ref(),
                host,
                location_generation,
                point,
                viewport,
                now_ms,
            )
            .map_err(|error| error.to_string())
        });
    BlockingResult {
        fresh,
        cache,
        cache_error,
    }
}

/// A malformed regular cache is disposable acceleration state. Quarantine it
/// before an outage fallback so the next retry does not repeatedly parse the
/// same corrupt bytes. Symlinks and other non-regular paths are left in place
/// and remain fail-closed for the later cache write.
fn load_cache_for_refresh(path: &Path) -> io::Result<Option<AtmosphereCache>> {
    match load_cache(path) {
        Ok(cache) => Ok(cache),
        Err(error) if quarantine_regular_cache(path)? => {
            tracing::warn!(
                target: "mackesd::weather_atmosphere",
                %error,
                path = %path.display(),
                "quarantined unusable atmospheric cache; continuing without cache"
            );
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

fn quarantine_regular_cache(path: &Path) -> io::Result<bool> {
    static NONCE: AtomicU64 = AtomicU64::new(0);
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(true),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Ok(false);
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let quarantine = parent.join(format!(
        ".corrupt-weather-atmosphere-cache-{}-{}",
        std::process::id(),
        NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    match fs::rename(path, &quarantine) {
        Ok(()) => {
            File::open(parent)?.sync_all()?;
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(_) => Ok(false),
    }
}

fn effective_location(snapshot: &EffectiveLocationSnapshot) -> Option<&EffectiveWeatherLocation> {
    match &snapshot.state {
        EffectiveLocationState::Available { location }
        | EffectiveLocationState::Stale { location, .. } => Some(location),
        EffectiveLocationState::Unavailable { .. } => None,
    }
}

fn read_location(
    persist: &Persist,
    host: &str,
    now_ms: i64,
) -> io::Result<EffectiveLocationSnapshot> {
    let body = persist
        .read_latest(&weather_location_state_topic(host))
        .map_err(io_other)?
        .and_then(|message| message.body)
        .ok_or_else(|| io::Error::other("effective weather location is unavailable"))?;
    EffectiveLocationSnapshot::from_json_at(body.as_bytes(), now_ms).map_err(io_other)
}

#[derive(Debug, Clone)]
struct AtmosphericAuthority {
    location: EffectiveLocationSnapshot,
    viewport: WeatherMapViewportState,
}

#[derive(Debug, Serialize)]
struct AtmosphericMapReset<'a> {
    schema_version: u16,
    host: &'a str,
    location_generation: u64,
    location_point: &'a GeoPoint,
    cleared_at_ms: i64,
    state: &'static str,
}

fn publish_viewport_state(
    persist: &Persist,
    host: &str,
    state: &WeatherMapViewportState,
) -> io::Result<()> {
    let body = serde_json::to_string(state).map_err(io_other)?;
    persist
        .write(
            &weather_map_viewport_state_topic(host),
            Priority::Default,
            None,
            Some(&body),
        )
        .map_err(io_other)?;
    Ok(())
}

fn resolve_viewport(
    persist: &Persist,
    host: &str,
    location: &EffectiveLocationSnapshot,
    point: &GeoPoint,
    now_ms: i64,
) -> io::Result<WeatherMapViewportState> {
    let fallback = WeatherMapViewportState {
        schema_version: WEATHER_CONTRACT_SCHEMA_VERSION,
        host: host.into(),
        location_generation: location.generation,
        viewport: fallback_viewport(location.generation, point)?,
        source: WeatherMapViewportSource::EffectiveLocationFallback,
        admitted_at_ms: now_ms,
    };
    let current = persist
        .read_latest(&weather_map_viewport_state_topic(host))
        .map_err(io_other)?
        .and_then(|message| message.body)
        .and_then(|body| WeatherMapViewportState::from_json_at(body.as_bytes(), now_ms).ok())
        .filter(|state| {
            state.host == host
                && state.location_generation == location.generation
                && match state.source {
                    WeatherMapViewportSource::EffectiveLocationFallback => {
                        state.viewport == fallback.viewport
                    }
                    WeatherMapViewportSource::MapsAction => {
                        state.viewport.generation > fallback.viewport.generation
                    }
                }
        });
    let action = persist
        .read_latest(&weather_set_map_viewport_topic(host))
        .map_err(io_other)?
        .and_then(|message| message.body)
        .and_then(|body| SetWeatherMapViewportRequest::from_json_at(body.as_bytes(), now_ms).ok())
        .filter(|request| {
            request.target_host == host
                && request.expected_location_generation == location.generation
                && now_ms.saturating_sub(request.issued_at_ms) <= MAX_VIEWPORT_ACTION_AGE_MS
        });
    let minimum_generation = current
        .as_ref()
        .map_or(fallback.viewport.generation, |state| {
            state.viewport.generation
        });
    let admitted = action
        .filter(|request| request.viewport.generation > minimum_generation)
        .map(|request| WeatherMapViewportState {
            schema_version: WEATHER_CONTRACT_SCHEMA_VERSION,
            host: host.into(),
            location_generation: location.generation,
            viewport: request.viewport,
            source: WeatherMapViewportSource::MapsAction,
            admitted_at_ms: now_ms,
        })
        .or(current)
        .unwrap_or(fallback);
    admitted.validate_at(now_ms).map_err(io_other)?;
    let unchanged = persist
        .read_latest(&weather_map_viewport_state_topic(host))
        .map_err(io_other)?
        .and_then(|message| message.body)
        .and_then(|body| WeatherMapViewportState::from_json_at(body.as_bytes(), now_ms).ok())
        .is_some_and(|state| state == admitted);
    if !unchanged {
        publish_viewport_state(persist, host, &admitted)?;
    }
    Ok(admitted)
}

fn same_authority(
    expected: &EffectiveLocationSnapshot,
    latest: &EffectiveLocationSnapshot,
) -> bool {
    expected.host == latest.host
        && expected.generation == latest.generation
        && expected.mode == latest.mode
        && effective_location(expected)
            .zip(effective_location(latest))
            .is_some_and(|(expected, latest)| expected == latest)
}

fn same_atmospheric_authority(
    expected: &AtmosphericAuthority,
    latest: &AtmosphericAuthority,
) -> bool {
    same_authority(&expected.location, &latest.location)
        && expected.viewport == latest.viewport
        && expected.viewport.location_generation == expected.location.generation
}

fn publish(persist: &Persist, host: &str, snapshot: &AtmosphericMapSnapshot) -> io::Result<()> {
    let body = serde_json::to_string(snapshot).map_err(io_other)?;
    persist
        .write(
            &weather_map_state_topic(host),
            Priority::Default,
            None,
            Some(&body),
        )
        .map_err(io_other)?;
    Ok(())
}

#[derive(Debug)]
struct RefreshSchedule {
    generations: Option<(u64, u64)>,
    due_ms: i64,
    retry: Duration,
}

impl RefreshSchedule {
    fn new(now_ms: i64) -> Self {
        Self {
            generations: None,
            due_ms: now_ms,
            retry: RETRY_INITIAL,
        }
    }

    fn due(&mut self, now_ms: i64, location_generation: u64, viewport_generation: u64) -> bool {
        let generations = (location_generation, viewport_generation);
        if self.generations != Some(generations) {
            self.generations = Some(generations);
            self.due_ms = now_ms;
        }
        now_ms >= self.due_ms
    }

    fn record(&mut self, now_ms: i64, fresh: bool) {
        if fresh {
            self.retry = RETRY_INITIAL;
            self.due_ms = now_ms.saturating_add(POLL.as_millis() as i64);
        } else {
            self.due_ms = now_ms.saturating_add(self.retry.as_millis() as i64);
            self.retry = self.retry.saturating_mul(2).min(POLL);
        }
    }
}

/// Daemon-owned NOAA nowCOAST atmospheric map authority.
pub struct WeatherAtmosphereWorker {
    host: String,
    probe: Option<Arc<dyn AtmosphericProbe>>,
    clock: Arc<dyn Clock>,
    bus_root_override: Option<PathBuf>,
    cache_path: PathBuf,
}

impl WeatherAtmosphereWorker {
    /// Construct the default official NOAA producer for one local host.
    #[must_use]
    pub fn new(host: String) -> Self {
        let probe = NowCoastHttpProbe::new()
            .map(|probe| Arc::new(probe) as Arc<dyn AtmosphericProbe>)
            .map_err(|error| {
                tracing::warn!(target: "mackesd::weather_atmosphere", %error, "nowCOAST client unavailable");
                error
            })
            .ok();
        Self {
            host,
            probe,
            clock: Arc::new(SystemClock),
            bus_root_override: None,
            cache_path: cache_path(),
        }
    }

    fn open_bus(&self) -> io::Result<Persist> {
        let root = weather_atmosphere_bus_root(
            self.bus_root_override.as_deref(),
            crate::bus_publish::default_bus_root(),
        );
        let mut persist = Persist::open(root).map_err(io_other)?;
        persist.reopen_if_index_changed();
        Ok(persist)
    }

    fn read_authority(&self) -> io::Result<AtmosphericAuthority> {
        let persist = self.open_bus()?;
        let now_ms = self.clock.now_ms();
        let location = read_location(&persist, &self.host, now_ms)?;
        let point = effective_location(&location)
            .map(|location| &location.point)
            .ok_or_else(|| io::Error::other("effective weather location is unavailable"))?;
        let viewport = resolve_viewport(&persist, &self.host, &location, point, now_ms)?;
        Ok(AtmosphericAuthority { location, viewport })
    }

    /// Revoke a previously published atmospheric image when its location
    /// authority can no longer be admitted. The reset is written only while a
    /// typed atmospheric snapshot is still the latest record, so the two-second
    /// recovery poll cannot flood the Bus and cannot overwrite another
    /// authority's newer reset.
    fn revoke_projection_after_authority_loss(&self) -> io::Result<bool> {
        let persist = self.open_bus()?;
        let Some(body) = persist
            .read_latest(&weather_map_state_topic(&self.host))
            .map_err(io_other)?
            .and_then(|message| message.body)
        else {
            return Ok(false);
        };
        let Ok(snapshot) = serde_json::from_str::<AtmosphericMapSnapshot>(&body) else {
            return Ok(false);
        };
        if snapshot.host != self.host {
            return Ok(false);
        }
        let reset = AtmosphericMapReset {
            schema_version: WEATHER_CONTRACT_SCHEMA_VERSION,
            host: &self.host,
            location_generation: snapshot.location_generation,
            location_point: &snapshot.location_point,
            cleared_at_ms: self.clock.now_ms(),
            state: "location_authority_unavailable",
        };
        let body = serde_json::to_string(&reset).map_err(io_other)?;
        persist
            .write(
                &weather_map_state_topic(&self.host),
                Priority::Default,
                None,
                Some(&body),
            )
            .map_err(io_other)?;
        Ok(true)
    }

    async fn refresh_once(&self, expected: AtmosphericAuthority) -> io::Result<bool> {
        let Some(location) = effective_location(&expected.location).cloned() else {
            return Ok(false);
        };
        let viewport = expected.viewport.viewport.clone();
        let probe = self.probe.clone();
        let host = self.host.clone();
        let point = location.point.clone();
        let blocking_viewport = viewport.clone();
        let cache_path = self.cache_path.clone();
        let generation = expected.location.generation;
        let now_ms = self.clock.now_ms();
        let result = tokio::task::spawn_blocking(move || {
            blocking_refresh(
                probe,
                &host,
                generation,
                &point,
                &blocking_viewport,
                now_ms,
                &cache_path,
            )
        })
        .await
        .map_err(|error| io::Error::other(format!("nowCOAST task failed: {error}")))?;

        let persist = self.open_bus()?;
        let now_ms = self.clock.now_ms();
        let latest_location = read_location(&persist, &self.host, now_ms)?;
        let latest_point = effective_location(&latest_location)
            .map(|location| &location.point)
            .ok_or_else(|| io::Error::other("effective weather location is unavailable"))?;
        let latest_viewport =
            resolve_viewport(&persist, &self.host, &latest_location, latest_point, now_ms)?;
        let latest = AtmosphericAuthority {
            location: latest_location,
            viewport: latest_viewport,
        };
        if !same_atmospheric_authority(&expected, &latest) {
            tracing::info!(
                target: "mackesd::weather_atmosphere",
                expected_location_generation = expected.location.generation,
                latest_location_generation = latest.location.generation,
                expected_viewport_generation = expected.viewport.viewport.generation,
                latest_viewport_generation = latest.viewport.viewport.generation,
                "discarding nowCOAST response after authority change"
            );
            return Ok(false);
        }
        let now_ms = self.clock.now_ms();
        let (snapshot, fresh) = match result.fresh {
            Ok(snapshot) => (snapshot, true),
            Err(error) => {
                let cached = result.cache.as_ref().and_then(|cache| {
                    cached_projection(
                        cache,
                        &self.host,
                        expected.location.generation,
                        &location.point,
                        &viewport,
                        now_ms,
                    )
                });
                cached.map_or_else(
                    || {
                        let cache_gap = result
                            .cache_error
                            .as_deref()
                            .map(|cache_error| format!("; cache refused: {cache_error}"))
                            .unwrap_or_default();
                        (
                            unavailable_snapshot(
                                &self.host,
                                expected.location.generation,
                                &location.point,
                                &viewport,
                                now_ms,
                                WeatherUnavailableReason::ProviderUnavailable,
                                format!("nowCOAST unavailable: {error}{cache_gap}"),
                            ),
                            false,
                        )
                    },
                    |snapshot| (snapshot, false),
                )
            }
        };
        snapshot.validate_at(now_ms).map_err(io_other)?;
        if fresh {
            let cache = AtmosphereCache {
                schema_version: CACHE_SCHEMA_VERSION,
                host: self.host.clone(),
                location_generation: expected.location.generation,
                location_point: location.point,
                viewport,
                snapshot: snapshot.clone(),
            };
            let cache_path = self.cache_path.clone();
            tokio::task::spawn_blocking(move || store_cache(&cache_path, &cache))
                .await
                .map_err(|error| io::Error::other(format!("cache task failed: {error}")))??;
        }
        publish(&persist, &self.host, &snapshot)?;
        Ok(fresh)
    }
}

fn weather_atmosphere_bus_root(explicit: Option<&Path>, current: Option<PathBuf>) -> PathBuf {
    explicit
        .map(Path::to_path_buf)
        .or(current)
        .unwrap_or_else(|| PathBuf::from(mde_bus::SYSTEM_BUS_ROOT))
}

fn io_other(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

#[async_trait::async_trait]
impl Worker for WeatherAtmosphereWorker {
    fn name(&self) -> &'static str {
        "weather_atmosphere"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let mut schedule = RefreshSchedule::new(self.clock.now_ms());
        loop {
            let authority = match self.read_authority() {
                Ok(authority) => authority,
                Err(error) => {
                    tracing::warn!(target: "mackesd::weather_atmosphere", %error, "effective location unavailable");
                    if let Err(revoke_error) = self.revoke_projection_after_authority_loss() {
                        tracing::warn!(
                            target: "mackesd::weather_atmosphere",
                            %revoke_error,
                            "failed to revoke atmospheric projection after authority loss"
                        );
                    }
                    tokio::select! {
                        () = shutdown.wait() => break,
                        () = tokio::time::sleep(AUTHORITY_POLL) => continue,
                    }
                }
            };
            if schedule.due(
                self.clock.now_ms(),
                authority.location.generation,
                authority.viewport.viewport.generation,
            ) {
                let refresh = self.refresh_once(authority);
                let result = tokio::select! {
                    () = shutdown.wait() => break,
                    result = refresh => result,
                };
                match result {
                    Ok(fresh) => schedule.record(self.clock.now_ms(), fresh),
                    Err(error) => {
                        tracing::warn!(target: "mackesd::weather_atmosphere", %error, "atmospheric refresh failed");
                        schedule.record(self.clock.now_ms(), false);
                    }
                }
            }
            tokio::select! {
                () = shutdown.wait() => break,
                () = tokio::time::sleep(AUTHORITY_POLL) => {}
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::location::WeatherCoverage;
    use mackes_mesh_types::location::{
        EffectiveLocationProvenance, LocationUnavailableReason, WeatherLocationMode,
        WEATHER_LOCATION_SCHEMA_VERSION,
    };
    use std::sync::atomic::{AtomicBool, AtomicI64};
    use std::sync::Mutex;
    use tempfile::TempDir;

    const NOW: i64 = 1_800_000_000_000;

    struct TestClock(AtomicI64);

    impl Clock for TestClock {
        fn now_ms(&self) -> i64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    struct FixtureProbe {
        fail: AtomicBool,
        threads: Mutex<Vec<std::thread::ThreadId>>,
        on_cloud: Option<Box<dyn Fn() + Send + Sync>>,
    }

    impl AtmosphericProbe for FixtureProbe {
        fn fetch_field(
            &self,
            field: AtmosphericFieldKind,
            _viewport: &AtmosphericViewport,
        ) -> io::Result<Vec<u8>> {
            self.threads
                .lock()
                .expect("threads")
                .push(std::thread::current().id());
            if field == AtmosphericFieldKind::CloudCover {
                if let Some(callback) = &self.on_cloud {
                    callback();
                }
            }
            if self.fail.load(Ordering::SeqCst) {
                Err(io::Error::other("fixture provider unavailable"))
            } else {
                Ok(png())
            }
        }
    }

    fn fixture_probe() -> Arc<FixtureProbe> {
        Arc::new(FixtureProbe {
            fail: AtomicBool::new(false),
            threads: Mutex::new(vec![]),
            on_cloud: None,
        })
    }

    fn png() -> Vec<u8> {
        let mut png = vec![0_u8; 41];
        png[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        png[12..16].copy_from_slice(b"IHDR");
        png[16..20].copy_from_slice(&u32::from(ATMOSPHERIC_FIELD_EDGE).to_be_bytes());
        png[20..24].copy_from_slice(&u32::from(ATMOSPHERIC_FIELD_EDGE).to_be_bytes());
        png[33..37].copy_from_slice(b"IEND");
        png
    }

    fn location(generation: u64, longitude: f64) -> EffectiveLocationSnapshot {
        EffectiveLocationSnapshot {
            schema_version: WEATHER_LOCATION_SCHEMA_VERSION,
            host: "workstation-1".into(),
            generation,
            mode: WeatherLocationMode::Manual,
            produced_at_ms: NOW - 1_000,
            state: EffectiveLocationState::Available {
                location: EffectiveWeatherLocation {
                    label: "Boston".into(),
                    point: GeoPoint {
                        latitude: 42.3601,
                        longitude,
                    },
                    time_zone: "America/New_York".into(),
                    coverage: WeatherCoverage::NwsUnitedStates,
                    provenance: EffectiveLocationProvenance::ManualVerifiedPlace {
                        place_id: format!("boston-{generation}"),
                    },
                    source_observed_at_ms: None,
                },
            },
        }
    }

    fn write_location(root: &Path, location: &EffectiveLocationSnapshot) {
        let persist = Persist::open(root.to_path_buf()).expect("open Bus");
        let body = serde_json::to_string(location).expect("encode");
        persist
            .write(
                &weather_location_state_topic("workstation-1"),
                Priority::Default,
                None,
                Some(&body),
            )
            .expect("publish location");
    }

    fn worker_at(
        temp: &TempDir,
        probe: Arc<dyn AtmosphericProbe>,
        now_ms: i64,
    ) -> WeatherAtmosphereWorker {
        WeatherAtmosphereWorker {
            host: "workstation-1".into(),
            probe: Some(probe),
            clock: Arc::new(TestClock(AtomicI64::new(now_ms))),
            bus_root_override: Some(temp.path().join("bus")),
            cache_path: temp.path().join("atmosphere-cache.json"),
        }
    }

    fn latest(root: &Path) -> AtmosphericMapSnapshot {
        let persist = Persist::open(root.to_path_buf()).expect("open Bus");
        let body = persist
            .read_latest(&weather_map_state_topic("workstation-1"))
            .expect("read")
            .expect("message")
            .body
            .expect("body");
        serde_json::from_str(&body).expect("decode")
    }

    fn authority(worker: &WeatherAtmosphereWorker) -> AtmosphericAuthority {
        worker.read_authority().expect("resolve authority")
    }

    fn write_viewport_action(
        root: &Path,
        location_generation: u64,
        viewport_generation: u64,
        zoom: u8,
        x: u32,
        y: u32,
        issued_at_ms: i64,
    ) {
        let request = SetWeatherMapViewportRequest {
            schema_version: WEATHER_CONTRACT_SCHEMA_VERSION,
            request_id: format!("viewport-{viewport_generation}"),
            target_host: "workstation-1".into(),
            expected_location_generation: location_generation,
            viewport: AtmosphericViewport {
                generation: viewport_generation,
                zoom,
                x,
                y,
                pixel_width: ATMOSPHERIC_FIELD_EDGE,
                pixel_height: ATMOSPHERIC_FIELD_EDGE,
            },
            issued_at_ms,
        };
        let persist = Persist::open(root.to_path_buf()).expect("open Bus");
        persist
            .write(
                &weather_set_map_viewport_topic("workstation-1"),
                Priority::Default,
                None,
                Some(&serde_json::to_string(&request).expect("encode")),
            )
            .expect("publish viewport action");
    }

    fn write_viewport_state(root: &Path, state: &WeatherMapViewportState) {
        let persist = Persist::open(root.to_path_buf()).expect("open Bus");
        persist
            .write(
                &weather_map_viewport_state_topic("workstation-1"),
                Priority::Default,
                None,
                Some(&serde_json::to_string(state).expect("encode")),
            )
            .expect("publish viewport state");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn publishes_three_exact_fields_with_provider_work_off_runtime() {
        let temp = TempDir::new().expect("temp");
        let root = temp.path().join("bus");
        write_location(&root, &location(7, -71.0589));
        let probe = fixture_probe();
        let worker = worker_at(&temp, probe.clone(), NOW);
        let runtime_thread = std::thread::current().id();
        let expected = authority(&worker);
        assert!(worker.refresh_once(expected).await.expect("refresh"));
        let snapshot = latest(&root);
        assert_eq!(snapshot.location_generation, 7);
        assert_eq!(snapshot.viewport.generation, 7);
        assert_eq!(snapshot.fields.len(), 3);
        assert!(probe
            .threads
            .lock()
            .expect("threads")
            .iter()
            .all(|thread| *thread != runtime_thread));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn location_authority_loss_revokes_old_imagery_and_recovery_republishes() {
        let temp = TempDir::new().expect("temp");
        let root = temp.path().join("bus");
        write_location(&root, &location(7, -71.0589));
        let worker = worker_at(&temp, fixture_probe(), NOW);
        assert!(worker
            .refresh_once(authority(&worker))
            .await
            .expect("seed atmospheric projection"));

        let mut unavailable = location(8, -72.0);
        unavailable.state = EffectiveLocationState::Unavailable {
            reason: LocationUnavailableReason::NoVerifiedFallback,
        };
        write_location(&root, &unavailable);
        assert!(worker
            .read_authority()
            .expect_err("unavailable authority must fail closed")
            .to_string()
            .contains("unavailable"));
        assert!(worker
            .revoke_projection_after_authority_loss()
            .expect("revoke stale imagery"));
        assert!(
            !worker
                .revoke_projection_after_authority_loss()
                .expect("repeated poll remains idempotent"),
            "the authority recovery poll must not flood the Bus with resets"
        );

        let persist = Persist::open(root.clone()).expect("open Bus");
        let reset = persist
            .read_latest(&weather_map_state_topic("workstation-1"))
            .expect("read reset")
            .and_then(|message| message.body)
            .and_then(|body| serde_json::from_str::<serde_json::Value>(&body).ok())
            .expect("decode reset");
        assert_eq!(reset["state"], "location_authority_unavailable");
        assert_eq!(reset["location_generation"], 7);
        assert!(serde_json::from_value::<AtmosphericMapSnapshot>(reset).is_err());

        write_location(&root, &location(8, -72.0));
        assert!(worker
            .refresh_once(authority(&worker))
            .await
            .expect("provider recovery"));
        let recovered = latest(&root);
        assert_eq!(recovered.location_generation, 8);
        assert!(matches!(recovered.availability, WeatherAvailability::Fresh));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn late_and_replaced_bus_recovers_external_authority_and_shutdown() {
        assert_eq!(
            weather_atmosphere_bus_root(Some(Path::new("/explicit")), Some("/current".into())),
            PathBuf::from("/explicit")
        );
        assert_eq!(
            weather_atmosphere_bus_root(None, Some("/current".into())),
            PathBuf::from("/current")
        );
        assert_eq!(
            weather_atmosphere_bus_root(None, None),
            PathBuf::from(mde_bus::SYSTEM_BUS_ROOT)
        );

        let temp = TempDir::new().expect("temp");
        let root = temp.path().join("late-bus");
        fs::write(&root, b"temporarily unopenable Bus root").expect("block Bus root");
        let mut worker = worker_at(&temp, fixture_probe(), NOW);
        worker.bus_root_override = Some(root.clone());
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let task =
            tokio::spawn(
                async move { worker.run(ShutdownToken::from_receiver(shutdown_rx)).await },
            );

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            !task.is_finished(),
            "late Bus must not terminate the worker"
        );
        fs::remove_file(&root).expect("unblock Bus root");
        write_location(&root, &location(7, -71.0589));
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let persist = Persist::open(root.clone()).expect("open recovered Bus");
                let generation = persist
                    .read_latest(&weather_map_state_topic("workstation-1"))
                    .expect("read recovered projection")
                    .and_then(|message| message.body)
                    .and_then(|body| serde_json::from_str::<AtmosphericMapSnapshot>(&body).ok())
                    .map(|snapshot| snapshot.location_generation);
                if generation == Some(7) {
                    break;
                }
                assert!(!task.is_finished(), "worker exited before Bus recovery");
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("late Bus projection");

        let index = root.join("index.sqlite");
        for suffix in ["", "-wal", "-shm"] {
            let _ = fs::remove_file(format!("{}{suffix}", index.display()));
        }
        write_location(&root, &location(8, -72.0));
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let persist = Persist::open(root.clone()).expect("open replacement Bus");
                let generation = persist
                    .read_latest(&weather_map_state_topic("workstation-1"))
                    .expect("read replacement projection")
                    .and_then(|message| message.body)
                    .and_then(|body| serde_json::from_str::<AtmosphericMapSnapshot>(&body).ok())
                    .map(|snapshot| snapshot.location_generation);
                if generation == Some(8) {
                    break;
                }
                assert!(!task.is_finished(), "worker exited after Bus replacement");
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("replacement Bus projection");

        shutdown_tx.send(true).expect("request shutdown");
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("shutdown timeout")
            .expect("worker task")
            .expect("worker result");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fresh_cache_precedes_publication_and_failures_remain_retryable() {
        let temp = TempDir::new().expect("temp");
        let root = temp.path().join("bus");
        write_location(&root, &location(7, -71.0589));
        let worker = worker_at(&temp, fixture_probe(), NOW);

        fs::create_dir(&worker.cache_path).expect("block cache file");
        let expected = authority(&worker);
        assert!(worker.refresh_once(expected).await.is_err());
        let persist = Persist::open(root.clone()).expect("open Bus");
        assert!(persist
            .read_latest(&weather_map_state_topic("workstation-1"))
            .expect("read after cache failure")
            .is_none());
        fs::remove_dir(&worker.cache_path).expect("unblock cache file");

        let map_path = root.join(weather_map_state_topic("workstation-1"));
        fs::create_dir_all(map_path.parent().expect("map parent")).expect("create map parent");
        fs::write(&map_path, b"block map topic directory").expect("block publication");
        let expected = authority(&worker);
        let expected_point = effective_location(&expected.location)
            .expect("effective location")
            .point
            .clone();
        let expected_viewport = expected.viewport.viewport.clone();
        assert!(worker.refresh_once(expected).await.is_err());
        let cache = load_cache(&worker.cache_path)
            .expect("read durable cache")
            .expect("fresh cache");
        assert!(cache.matches("workstation-1", 7, &expected_point, &expected_viewport));

        fs::remove_file(&map_path).expect("unblock publication");
        let expected = authority(&worker);
        assert!(worker.refresh_once(expected).await.expect("retry refresh"));
        assert_eq!(latest(&root).location_generation, 7);
    }

    #[test]
    fn malformed_regular_cache_is_quarantined_before_provider_outage_fallback() {
        let temp = TempDir::new().expect("temp");
        let cache_path = temp.path().join("atmosphere-cache.json");
        fs::write(&cache_path, b"not-json").expect("corrupt cache");
        let location = location(7, -71.0589);
        let point = location_point(&location);
        let viewport = fallback_viewport(7, &point).expect("fallback viewport");
        let probe = fixture_probe();
        probe.fail.store(true, Ordering::SeqCst);

        let result = blocking_refresh(
            Some(probe),
            "workstation-1",
            7,
            &point,
            &viewport,
            NOW,
            &cache_path,
        );

        assert!(
            result.fresh.is_err(),
            "provider outage must remain explicit"
        );
        assert!(result.cache.is_none(), "corrupt cache must not be admitted");
        assert!(
            result.cache_error.is_none(),
            "quarantine should recover the read"
        );
        assert!(
            !cache_path.exists(),
            "corrupt cache must leave the authority path"
        );
        assert!(
            fs::read_dir(temp.path())
                .expect("cache directory")
                .flatten()
                .any(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".corrupt-weather-atmosphere-cache-")),
            "corrupt cache must remain recoverable as quarantined evidence"
        );
    }

    #[test]
    fn official_wms_products_url_png_and_dimensions_are_strict() {
        let viewport =
            fallback_viewport(7, &location_point(&location(7, -71.0589))).expect("viewport");
        for field in [
            AtmosphericFieldKind::Temperature,
            AtmosphericFieldKind::Wind,
            AtmosphericFieldKind::CloudCover,
        ] {
            let url = get_map_url(field, &viewport).expect("url");
            let (service_path, layer_name) = field.nowcoast_product();
            assert_eq!(url.path(), service_path);
            assert_eq!(
                url.query_pairs()
                    .find(|(key, _)| key == "layers")
                    .map(|(_, value)| value.into_owned())
                    .as_deref(),
                Some(layer_name)
            );
            assert!(validate_get_map_url(url.as_str(), field, &viewport).is_ok());
        }
        let url = get_map_url(AtmosphericFieldKind::Wind, &viewport).expect("url");
        let (wind_path, _) = AtmosphericFieldKind::Wind.nowcoast_product();
        for hostile in [
            url.as_str().replace("https://", "http://"),
            url.as_str()
                .replace(OFFICIAL_HOST, "nowcoast.noaa.gov.evil.test"),
            format!("https://evil@{OFFICIAL_HOST}{wind_path}?service=WMS"),
            format!("https://{OFFICIAL_HOST}:444{wind_path}?service=WMS"),
            url.as_str().replace("wind_velocity", "wind_speed"),
            url.as_str().replace("ndfd_wind", "ndfd_temperature"),
        ] {
            assert!(
                validate_get_map_url(&hostile, AtmosphericFieldKind::Wind, &viewport).is_err(),
                "accepted {hostile}"
            );
        }
        assert!(validate_png(&png()).is_ok());
        let mut wrong = png();
        wrong[16..20].copy_from_slice(&512_u32.to_be_bytes());
        assert!(validate_png(&wrong).is_err());
        assert!(validate_png(&vec![0; MAX_ATMOSPHERIC_FIELD_PNG_BYTES + 1]).is_err());
    }

    #[test]
    fn daemon_admits_latest_viewport_and_keeps_deterministic_fallback() {
        let temp = TempDir::new().expect("temp");
        let root = temp.path().join("bus");
        write_location(&root, &location(7, -71.0589));
        let worker = worker_at(&temp, fixture_probe(), NOW);
        let fallback = authority(&worker);
        assert_eq!(
            fallback.viewport.source,
            WeatherMapViewportSource::EffectiveLocationFallback
        );
        assert_eq!(fallback.viewport.viewport.generation, 7);

        write_viewport_action(&root, 7, 8, 7, 38, 47, NOW);
        let admitted = authority(&worker);
        assert_eq!(
            admitted.viewport.source,
            WeatherMapViewportSource::MapsAction
        );
        assert_eq!(admitted.viewport.viewport.generation, 8);
        assert_eq!(admitted.viewport.viewport.zoom, 7);

        write_viewport_action(&root, 7, 7, 6, 19, 23, NOW);
        assert_eq!(authority(&worker).viewport, admitted.viewport);
        write_viewport_action(&root, 6, 9, 7, 39, 47, NOW);
        assert_eq!(authority(&worker).viewport, admitted.viewport);
    }

    #[test]
    fn restart_rejects_retained_viewport_without_source_identity() {
        let temp = TempDir::new().expect("temp");
        let root = temp.path().join("bus");
        let location = location(7, -71.0589);
        write_location(&root, &location);
        let expected = fallback_viewport(7, &location_point(&location)).expect("fallback");

        let mut forged_fallback = expected.clone();
        forged_fallback.x = (forged_fallback.x + 1) % (1_u32 << forged_fallback.zoom);
        write_viewport_state(
            &root,
            &WeatherMapViewportState {
                schema_version: WEATHER_CONTRACT_SCHEMA_VERSION,
                host: "workstation-1".into(),
                location_generation: 7,
                viewport: forged_fallback,
                source: WeatherMapViewportSource::EffectiveLocationFallback,
                admitted_at_ms: NOW - 1,
            },
        );

        let worker = worker_at(&temp, fixture_probe(), NOW);
        let recovered = authority(&worker).viewport;
        assert_eq!(
            recovered.source,
            WeatherMapViewportSource::EffectiveLocationFallback
        );
        assert_eq!(recovered.viewport, expected);

        let mut impossible_action = expected.clone();
        impossible_action.x = (impossible_action.x + 1) % (1_u32 << impossible_action.zoom);
        write_viewport_state(
            &root,
            &WeatherMapViewportState {
                schema_version: WEATHER_CONTRACT_SCHEMA_VERSION,
                host: "workstation-1".into(),
                location_generation: 7,
                viewport: impossible_action,
                source: WeatherMapViewportSource::MapsAction,
                admitted_at_ms: NOW - 1,
            },
        );

        let recovered = authority(&worker).viewport;
        assert_eq!(
            recovered.source,
            WeatherMapViewportSource::EffectiveLocationFallback
        );
        assert_eq!(recovered.viewport, expected);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn viewport_change_during_fetch_is_admitted_and_stale_result_is_discarded() {
        let temp = TempDir::new().expect("temp");
        let root = temp.path().join("bus");
        write_location(&root, &location(7, -71.0589));
        let changed_root = root.clone();
        let mut probe = Arc::try_unwrap(fixture_probe())
            .unwrap_or_else(|_| panic!("fixture unexpectedly shared"));
        probe.on_cloud = Some(Box::new(move || {
            write_viewport_action(&changed_root, 7, 8, 7, 38, 47, NOW);
        }));
        let worker = worker_at(&temp, Arc::new(probe), NOW);
        let expected = authority(&worker);
        assert!(!worker
            .refresh_once(expected)
            .await
            .expect("discard stale viewport"));
        let persist = Persist::open(root).expect("open Bus");
        assert!(persist
            .read_latest(&weather_map_state_topic("workstation-1"))
            .expect("read")
            .is_none());
        let state = persist
            .read_latest(&weather_map_viewport_state_topic("workstation-1"))
            .expect("read")
            .and_then(|message| message.body)
            .and_then(|body| serde_json::from_str::<WeatherMapViewportState>(&body).ok())
            .expect("admitted state");
        assert_eq!(state.source, WeatherMapViewportSource::MapsAction);
        assert_eq!(state.viewport.generation, 8);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn generation_change_during_fetch_discards_snapshot() {
        let temp = TempDir::new().expect("temp");
        let root = temp.path().join("bus");
        write_location(&root, &location(7, -71.0589));
        let changed_root = root.clone();
        let mut probe = Arc::try_unwrap(fixture_probe())
            .unwrap_or_else(|_| panic!("fixture unexpectedly shared"));
        probe.on_cloud = Some(Box::new(move || {
            write_location(&changed_root, &location(8, -72.0));
        }));
        let worker = worker_at(&temp, Arc::new(probe), NOW);
        let expected = authority(&worker);
        assert!(!worker.refresh_once(expected).await.expect("discard"));
        let persist = Persist::open(root).expect("open Bus");
        assert!(persist
            .read_latest(&weather_map_state_topic("workstation-1"))
            .expect("read")
            .is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn same_generation_location_provenance_substitution_discards_snapshot() {
        let temp = TempDir::new().expect("temp");
        let root = temp.path().join("bus");
        write_location(&root, &location(7, -71.0589));
        let changed_root = root.clone();
        let mut probe = Arc::try_unwrap(fixture_probe())
            .unwrap_or_else(|_| panic!("fixture unexpectedly shared"));
        probe.on_cloud = Some(Box::new(move || {
            let mut substituted = location(7, -71.0589);
            let EffectiveLocationState::Available { location } = &mut substituted.state else {
                panic!("fixture location must be available");
            };
            location.provenance = EffectiveLocationProvenance::ManualVerifiedPlace {
                place_id: "substituted-place".into(),
            };
            write_location(&changed_root, &substituted);
        }));
        let worker = worker_at(&temp, Arc::new(probe), NOW);
        let expected = authority(&worker);

        assert!(!worker
            .refresh_once(expected)
            .await
            .expect("discard substituted location authority"));
        let persist = Persist::open(root).expect("open Bus");
        assert!(persist
            .read_latest(&weather_map_state_topic("workstation-1"))
            .expect("read")
            .is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn restart_cache_is_identity_bound_stale_then_expired_and_corruption_is_refused() {
        let temp = TempDir::new().expect("temp");
        let root = temp.path().join("bus");
        write_location(&root, &location(7, -71.0589));
        let initial_probe = fixture_probe();
        let initial = worker_at(&temp, initial_probe, NOW);
        let initial_authority = authority(&initial);
        assert!(initial
            .refresh_once(initial_authority)
            .await
            .expect("initial"));

        let stale_probe = fixture_probe();
        stale_probe.fail.store(true, Ordering::SeqCst);
        let stale_now = NOW + MAX_ATMOSPHERIC_FRESH_AGE_MS + 1;
        let restarted = worker_at(&temp, stale_probe, stale_now);
        let restarted_authority = authority(&restarted);
        assert!(!restarted
            .refresh_once(restarted_authority)
            .await
            .expect("stale"));
        assert!(matches!(
            latest(&root).availability,
            WeatherAvailability::Stale { .. }
        ));

        write_viewport_action(&root, 7, 8, 7, 38, 47, stale_now);
        let viewport_probe = fixture_probe();
        viewport_probe.fail.store(true, Ordering::SeqCst);
        let changed_viewport = worker_at(&temp, viewport_probe, stale_now);
        let changed_viewport_authority = authority(&changed_viewport);
        assert!(!changed_viewport
            .refresh_once(changed_viewport_authority)
            .await
            .expect("viewport mismatch"));
        assert!(matches!(
            latest(&root).availability,
            WeatherAvailability::Unavailable { .. }
        ));

        write_location(&root, &location(8, -72.0));
        let mismatch_probe = fixture_probe();
        mismatch_probe.fail.store(true, Ordering::SeqCst);
        let mismatch = worker_at(&temp, mismatch_probe, stale_now);
        let mismatch_authority = authority(&mismatch);
        mismatch
            .refresh_once(mismatch_authority)
            .await
            .expect("mismatch");
        let mismatch_snapshot = latest(&root);
        assert_eq!(mismatch_snapshot.location_generation, 8);
        assert!(matches!(
            mismatch_snapshot.availability,
            WeatherAvailability::Unavailable { .. }
        ));

        write_location(&root, &location(7, -71.0589));
        let expired_probe = fixture_probe();
        expired_probe.fail.store(true, Ordering::SeqCst);
        let expired = worker_at(&temp, expired_probe, NOW + MAX_ATMOSPHERIC_CACHE_AGE_MS + 1);
        let expired_authority = authority(&expired);
        expired
            .refresh_once(expired_authority)
            .await
            .expect("expired");
        assert!(matches!(
            latest(&root).availability,
            WeatherAvailability::Unavailable { .. }
        ));

        fs::write(&initial.cache_path, b"not-json").expect("corrupt cache");
        let recovery = worker_at(&temp, fixture_probe(), NOW);
        let recovery_authority = authority(&recovery);
        assert!(recovery
            .refresh_once(recovery_authority)
            .await
            .expect("recover"));
        assert!(matches!(
            latest(&root).availability,
            WeatherAvailability::Fresh
        ));
        assert!(load_cache(&recovery.cache_path)
            .expect("load recovered")
            .is_some());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn restart_cache_cannot_relabel_atmospheric_source_identity() {
        let temp = TempDir::new().expect("temp");
        let root = temp.path().join("bus");
        write_location(&root, &location(7, -71.0589));
        let initial = worker_at(&temp, fixture_probe(), NOW);
        let initial_authority = authority(&initial);
        assert!(initial
            .refresh_once(initial_authority)
            .await
            .expect("seed canonical cache"));

        let mut substituted = load_cache(&initial.cache_path)
            .expect("read cache")
            .expect("seeded cache");
        substituted.snapshot.attributions = vec![WeatherAttribution {
            provider: WeatherProvider::NationalWeatherService,
            source_id: "substituted-source".into(),
            label: "Substituted atmospheric source".into(),
        }];
        store_cache(&initial.cache_path, &substituted).expect("replace cache source identity");

        let failed_probe = fixture_probe();
        failed_probe.fail.store(true, Ordering::SeqCst);
        let restarted = worker_at(&temp, failed_probe, NOW + MAX_ATMOSPHERIC_FRESH_AGE_MS + 1);
        let restarted_authority = authority(&restarted);
        assert!(!restarted
            .refresh_once(restarted_authority)
            .await
            .expect("reject relabeled restart cache"));

        let published = latest(&root);
        assert!(matches!(
            published.availability,
            WeatherAvailability::Unavailable { .. }
        ));
        assert_eq!(published.attributions, vec![attribution()]);
    }

    #[test]
    fn ten_minute_scheduler_backs_off_and_generation_change_is_due() {
        let mut schedule = RefreshSchedule::new(NOW);
        assert!(schedule.due(NOW, 7, 7));
        schedule.record(NOW, true);
        assert!(!schedule.due(NOW + POLL.as_millis() as i64 - 1, 7, 7));
        assert!(schedule.due(NOW + POLL.as_millis() as i64, 7, 7));
        schedule.record(NOW + POLL.as_millis() as i64, false);
        assert!(!schedule.due(NOW + POLL.as_millis() as i64 + 29_999, 7, 7));
        assert!(schedule.due(NOW + POLL.as_millis() as i64 + 30_000, 7, 7));
        schedule.record(NOW + POLL.as_millis() as i64 + 30_000, true);
        assert!(schedule.due(NOW + POLL.as_millis() as i64 + 30_001, 8, 7));
        schedule.record(NOW + POLL.as_millis() as i64 + 30_001, true);
        assert!(schedule.due(NOW + POLL.as_millis() as i64 + 30_002, 8, 9));
    }

    fn location_point(snapshot: &EffectiveLocationSnapshot) -> GeoPoint {
        effective_location(snapshot)
            .expect("effective location")
            .point
            .clone()
    }
}
