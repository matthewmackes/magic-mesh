//! WL-FUNC-012 / OVERLAY-6 — credential-gated NASA FIRMS hotspots.
//!
//! FIRMS is a useful context layer, not a safety-of-life feed.  The worker
//! therefore requires an explicit opt-in, a sealed MAP_KEY, and a fresh
//! same-host vehicle fix before it makes a request.  Every response is
//! bounded, validated, and published as one complete latest-wins snapshot.

#![cfg(feature = "async-services")]

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Read};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::NaiveDateTime;
use mackes_mesh_types::firms::{firms_state_topic, FirmsAvailability, FirmsHotspot, FirmsSnapshot};
use reqwest::blocking::Client;

use super::{ShutdownToken, Worker};

/// Explicit overlay opt-in. Unset/false is an idle no-op.
pub const ENABLED_ENV: &str = "MDE_OVERLAY_FIRMS_HOTSPOTS";
/// Optional official FIRMS source selector (for example `VIIRS_NOAA20_NRT`).
pub const SOURCE_ENV: &str = "MDE_OVERLAY_FIRMS_SOURCE";
/// Stable mde-seal secret reference for the free NASA FIRMS MAP_KEY.
pub const API_KEY_SECRET_REF: &str = "firms-api-key";
/// Official NASA FIRMS area CSV endpoint.
pub const DEFAULT_ENDPOINT: &str = "https://firms.modaps.eosdis.nasa.gov/api/area/csv/";
/// FIRMS near-real-time refresh cadence.
pub const POLL: Duration = Duration::from_secs(15 * 60);

const DEFAULT_SOURCE: &str = "VIIRS_NOAA20_NRT";
const RETRY_MIN: Duration = Duration::from_secs(60);
const RETRY_MAX: Duration = Duration::from_secs(15 * 60);
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const QUERY_RADIUS_KM: u16 = 200;
const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;
const MAX_CSV_FIELDS: usize = 64;
const MAX_FEED_RECORDS: usize = 2_000;
const MAX_RETAINED_HOTSPOTS: usize = 512;
const MAX_STRING_BYTES: usize = 160;
const MAX_GAPS: usize = 128;
const VEHICLE_FIX_MAX_AGE_MS: i64 = 30_000;
const VEHICLE_MAX_FUTURE_SKEW_MS: i64 = 5_000;
const USER_AGENT: &str =
    "Construct/12 mackesd NASA-FIRMS-overlay (+https://github.com/matthewmackes/magic-mesh)";

/// Fresh finite vehicle point used for the server-side area envelope.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FirmsContext {
    /// WGS-84 latitude.
    pub latitude: f64,
    /// WGS-84 longitude.
    pub longitude: f64,
}

/// Operator-safe fetch failure. It never includes the MAP_KEY or a keyed URL.
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

    fn authentication() -> Self {
        Self {
            message: format!(
                "NASA FIRMS rejected the sealed credential; rotate secret:{API_KEY_SECRET_REF}"
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

/// Injectable FIRMS query seam. Tests use captured CSV and never contact NASA.
pub trait FirmsProbe: Send + Sync {
    /// Query the official area CSV around a fresh vehicle fix.
    fn fetch(&self, context: FirmsContext, fetched_at_ms: i64) -> Result<String, ProbeFailure>;
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

/// Production rustls client for the official FIRMS area CSV service.
pub struct FirmsHttpProbe {
    client: Client,
    api_key: String,
    source: String,
}

impl FirmsHttpProbe {
    fn new(api_key: String, source: String) -> Result<Self, ProbeFailure> {
        validate_endpoint(DEFAULT_ENDPOINT).map_err(ProbeFailure::other)?;
        let api_key = validate_api_key(&api_key).map_err(ProbeFailure::other)?;
        let source = validate_source(&source).map_err(ProbeFailure::other)?;
        let client = Client::builder()
            .timeout(HTTP_TIMEOUT)
            .user_agent(USER_AGENT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| ProbeFailure::other(error.without_url()))?;
        Ok(Self {
            client,
            api_key,
            source,
        })
    }
}

impl FirmsProbe for FirmsHttpProbe {
    fn fetch(&self, context: FirmsContext, fetched_at_ms: i64) -> Result<String, ProbeFailure> {
        let url = query_url(context, &self.source, &self.api_key).map_err(ProbeFailure::other)?;
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
            return Err(ProbeFailure {
                message: "NASA FIRMS rate limited the query".to_string(),
                retry_after: Some(POLL),
                reload_key: false,
            });
        }
        if response.status() != reqwest::StatusCode::OK {
            return Err(ProbeFailure::other(format!(
                "NASA FIRMS returned unexpected HTTP {} (redirects are disabled)",
                response.status()
            )));
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_BODY_BYTES as u64)
        {
            return Err(ProbeFailure::other(
                "NASA FIRMS response exceeds byte limit",
            ));
        }
        let mut response = response;
        let body = read_bounded(&mut response)?;
        let _ = fetched_at_ms;
        Ok(body)
    }
}

fn validate_api_key(value: &str) -> io::Result<String> {
    let value = value.trim();
    if value.len() < 8
        || value.len() > 256
        || value.chars().any(|character| {
            character.is_control() || character.is_whitespace() || character == '/'
        })
    {
        return Err(io::Error::other("sealed NASA FIRMS MAP_KEY is invalid"));
    }
    Ok(value.to_string())
}

fn validate_source(value: &str) -> io::Result<String> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > MAX_STRING_BYTES
        || value.chars().any(|character| {
            character.is_control() || character.is_whitespace() || character == '/'
        })
    {
        return Err(io::Error::other("NASA FIRMS source is invalid"));
    }
    Ok(value.to_string())
}

fn validate_endpoint(value: &str) -> io::Result<reqwest::Url> {
    let url = reqwest::Url::parse(value).map_err(io_other)?;
    if url.scheme() != "https"
        || url.host_str() != Some("firms.modaps.eosdis.nasa.gov")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/api/area/csv/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(io::Error::other(
            "NASA FIRMS endpoint is outside the strict official-service allowlist",
        ));
    }
    Ok(url)
}

fn query_url(context: FirmsContext, source: &str, api_key: &str) -> io::Result<reqwest::Url> {
    validate_context(context)?;
    let source = validate_source(source)?;
    let api_key = validate_api_key(api_key)?;
    let (west, south, east, north) = query_envelope(context)?;
    let mut url = validate_endpoint(DEFAULT_ENDPOINT)?;
    url.set_path(&format!(
        "/api/area/csv/{api_key}/{source}/{west:.6},{south:.6},{east:.6},{north:.6}/1"
    ));
    Ok(url)
}

fn query_envelope(context: FirmsContext) -> io::Result<(f64, f64, f64, f64)> {
    validate_context(context)?;
    let latitude_delta = f64::from(QUERY_RADIUS_KM) / 111.32;
    let longitude_delta =
        f64::from(QUERY_RADIUS_KM) / (111.32 * context.latitude.to_radians().cos().abs().max(0.1));
    Ok((
        (context.longitude - longitude_delta).max(-180.0),
        (context.latitude - latitude_delta).max(-90.0),
        (context.longitude + longitude_delta).min(180.0),
        (context.latitude + latitude_delta).min(90.0),
    ))
}

fn read_bounded(reader: &mut impl Read) -> Result<String, ProbeFailure> {
    let mut bytes = Vec::with_capacity(MAX_BODY_BYTES.min(64 * 1024));
    reader
        .take(MAX_BODY_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(ProbeFailure::other)?;
    if bytes.len() > MAX_BODY_BYTES {
        return Err(ProbeFailure::other(
            "NASA FIRMS response exceeds byte limit",
        ));
    }
    String::from_utf8(bytes).map_err(ProbeFailure::other)
}

fn validate_context(context: FirmsContext) -> io::Result<()> {
    if !context.latitude.is_finite()
        || !context.longitude.is_finite()
        || !(-90.0..=90.0).contains(&context.latitude)
        || !(-180.0..=180.0).contains(&context.longitude)
    {
        return Err(io::Error::other(
            "FIRMS query point is outside WGS-84 bounds",
        ));
    }
    Ok(())
}

fn parse_snapshot(
    host: &str,
    context: FirmsContext,
    body: &str,
    fetched_at_ms: i64,
    source: &str,
) -> Result<FirmsSnapshot, ProbeFailure> {
    validate_context(context).map_err(ProbeFailure::other)?;
    let source = validate_source(source).map_err(ProbeFailure::other)?;
    if body.len() > MAX_BODY_BYTES {
        return Err(ProbeFailure::other(
            "NASA FIRMS response exceeds byte limit",
        ));
    }
    let mut lines = body.lines();
    let header_line = lines
        .next()
        .ok_or_else(|| ProbeFailure::other("NASA FIRMS response has no CSV header"))?;
    let headers = split_csv_line(header_line)?
        .into_iter()
        .map(|value| value.trim().to_ascii_lowercase())
        .collect::<Vec<_>>();
    if headers.iter().any(String::is_empty) {
        return Err(ProbeFailure::other("NASA FIRMS CSV has an empty header"));
    }
    let mut seen_headers = BTreeSet::new();
    if headers
        .iter()
        .any(|header| !seen_headers.insert(header.as_str()))
    {
        return Err(ProbeFailure::other("NASA FIRMS CSV has duplicate headers"));
    }
    let required = ["latitude", "longitude", "acq_date", "acq_time"];
    if required
        .iter()
        .any(|name| !headers.iter().any(|header| header == name))
    {
        return Err(ProbeFailure::other(
            "NASA FIRMS CSV is missing required location/time columns",
        ));
    }
    let index = |name: &str| headers.iter().position(|header| header == name);
    let lat_index = index("latitude").expect("required header checked");
    let lon_index = index("longitude").expect("required header checked");
    let date_index = index("acq_date").expect("required header checked");
    let time_index = index("acq_time").expect("required header checked");
    let mut snapshot = FirmsSnapshot::empty(
        host,
        fetched_at_ms,
        fetched_at_ms,
        &source,
        context.latitude,
        context.longitude,
        QUERY_RADIUS_KM,
    );
    let rows: Vec<&str> = lines.collect();
    let mut latest = BTreeMap::<String, FirmsHotspot>::new();
    for (line_number, line) in rows.iter().copied().take(MAX_FEED_RECORDS).enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let values = match split_csv_line(line) {
            Ok(values) => values,
            Err(error) => {
                snapshot.omitted_records = snapshot.omitted_records.saturating_add(1);
                push_gap(
                    &mut snapshot.gaps,
                    format!("CSV row {} omitted: {error}", line_number + 2),
                );
                continue;
            }
        };
        let value = |name: &str| index(name).and_then(|position| values.get(position));
        let Some(latitude) = value_at(&values, lat_index).and_then(|value| value.parse().ok())
        else {
            snapshot.omitted_records = snapshot.omitted_records.saturating_add(1);
            continue;
        };
        let Some(longitude) = value_at(&values, lon_index).and_then(|value| value.parse().ok())
        else {
            snapshot.omitted_records = snapshot.omitted_records.saturating_add(1);
            continue;
        };
        let point = FirmsContext {
            latitude,
            longitude,
        };
        if validate_context(point).is_err() {
            snapshot.omitted_records = snapshot.omitted_records.saturating_add(1);
            continue;
        }
        let distance_km = haversine_km(context, point);
        if !distance_km.is_finite() || distance_km > f64::from(QUERY_RADIUS_KM) {
            snapshot.omitted_records = snapshot.omitted_records.saturating_add(1);
            continue;
        }
        let Some(observed_at_ms) = parse_acquisition_time(
            value_at(&values, date_index).unwrap_or_default(),
            value_at(&values, time_index).unwrap_or_default(),
        ) else {
            snapshot.omitted_records = snapshot.omitted_records.saturating_add(1);
            continue;
        };
        let id = format!(
            "{}:{}:{latitude:.5}:{longitude:.5}",
            value("satellite")
                .filter(|value| !value.is_empty())
                .unwrap_or(&source),
            observed_at_ms
        );
        let hotspot = FirmsHotspot {
            id: id.clone(),
            latitude,
            longitude,
            brightness_k: parse_finite(value("bright_ti4")),
            frp_mw: parse_finite(value("frp")),
            confidence: bounded_string(value("confidence")),
            satellite: bounded_string(value("satellite")),
            observed_at_ms,
            distance_km: distance_km as f32,
        };
        latest.entry(id).or_insert(hotspot);
    }
    if rows.len() > MAX_FEED_RECORDS {
        push_gap(
            &mut snapshot.gaps,
            format!("NASA FIRMS records capped at {MAX_FEED_RECORDS}"),
        );
        snapshot.omitted_records = snapshot.omitted_records.saturating_add(1);
    }
    snapshot.hotspots = latest.into_values().take(MAX_RETAINED_HOTSPOTS).collect();
    if snapshot.hotspots.len() == MAX_RETAINED_HOTSPOTS {
        push_gap(
            &mut snapshot.gaps,
            format!("NASA FIRMS hotspots capped at {MAX_RETAINED_HOTSPOTS}"),
        );
    }
    Ok(snapshot)
}

fn value_at(values: &[String], index: usize) -> Option<&str> {
    values.get(index).map(String::as_str).map(str::trim)
}

fn parse_finite(value: Option<&String>) -> Option<f32> {
    let value = value?.trim().parse::<f32>().ok()?;
    value.is_finite().then_some(value)
}

fn bounded_string(value: Option<&String>) -> Option<String> {
    let value = value?.trim();
    (!value.is_empty() && value.len() <= MAX_STRING_BYTES).then(|| value.to_string())
}

fn parse_acquisition_time(date: &str, time: &str) -> Option<i64> {
    let time = time.trim().split('.').next().unwrap_or_default();
    let time = match time.len() {
        4 => format!("{time}00"),
        6 => time.to_string(),
        _ => return None,
    };
    NaiveDateTime::parse_from_str(&format!("{} {time}", date.trim()), "%Y-%m-%d %H%M%S")
        .ok()
        .map(|value| value.and_utc().timestamp_millis())
}

fn split_csv_line(line: &str) -> Result<Vec<String>, ProbeFailure> {
    let mut values = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(character) = chars.next() {
        match (quoted, character) {
            (true, '"') if chars.peek() == Some(&'"') => {
                current.push('"');
                chars.next();
            }
            (true, '"') => quoted = false,
            (false, '"') if current.is_empty() => quoted = true,
            (false, ',') => {
                if values.len() >= MAX_CSV_FIELDS {
                    return Err(ProbeFailure::other(format!(
                        "NASA FIRMS CSV field count exceeds limit of {MAX_CSV_FIELDS}"
                    )));
                }
                values.push(std::mem::take(&mut current));
            }
            (_, character) => current.push(character),
        }
        if current.len() > MAX_STRING_BYTES * 4 {
            return Err(ProbeFailure::other(
                "NASA FIRMS CSV field exceeds byte limit",
            ));
        }
    }
    if quoted {
        return Err(ProbeFailure::other(
            "NASA FIRMS CSV row has unterminated quote",
        ));
    }
    if values.len() >= MAX_CSV_FIELDS {
        return Err(ProbeFailure::other(format!(
            "NASA FIRMS CSV field count exceeds limit of {MAX_CSV_FIELDS}"
        )));
    }
    values.push(current);
    Ok(values)
}

fn haversine_km(a: FirmsContext, b: FirmsContext) -> f64 {
    let latitude = (b.latitude - a.latitude).to_radians();
    let longitude = (b.longitude - a.longitude).to_radians();
    let haversine = (latitude / 2.0).sin().powi(2)
        + a.latitude.to_radians().cos()
            * b.latitude.to_radians().cos()
            * (longitude / 2.0).sin().powi(2);
    6_371.008 * 2.0 * haversine.sqrt().asin()
}

fn push_gap(gaps: &mut Vec<String>, gap: String) {
    if gaps.len() < MAX_GAPS {
        gaps.push(gap);
    }
}

fn io_other(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

/// Workstation-side credential-gated NASA FIRMS adapter.
pub struct FirmsOverlayWorker {
    host: String,
    enabled: bool,
    probe: Option<Arc<dyn FirmsProbe>>,
    key_source: Arc<dyn ApiKeySource>,
    bus_root: Option<PathBuf>,
    source: String,
}

impl FirmsOverlayWorker {
    /// Production wiring. Disabled unless explicitly opted in.
    #[must_use]
    pub fn new(host: String) -> Self {
        Self {
            host,
            enabled: env_truthy(ENABLED_ENV),
            probe: None,
            key_source: Arc::new(SealedApiKeySource),
            bus_root: crate::bus_publish::default_bus_root(),
            source: std::env::var(SOURCE_ENV)
                .ok()
                .and_then(|value| validate_source(&value).ok())
                .unwrap_or_else(|| DEFAULT_SOURCE.to_string()),
        }
    }

    /// Inject a fixture probe, bypassing credential resolution in tests.
    #[must_use]
    pub fn with_probe(mut self, probe: Arc<dyn FirmsProbe>) -> Self {
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

    fn current_context(&self) -> Option<FirmsContext> {
        let root = self.bus_root.clone()?;
        let persist = mde_bus::persist::Persist::open(root).ok()?;
        let topic = mackes_mesh_types::vehicle::vehicle_state_topic(&self.host);
        let body = persist.read_latest(&topic).ok().flatten()?.body?;
        let vehicle: mackes_mesh_types::vehicle::VehicleState = serde_json::from_str(&body).ok()?;
        validated_vehicle_context(&vehicle, &self.host, now_ms())
    }

    fn publish(&self, snapshot: &FirmsSnapshot) {
        if let Some(mut persist) = crate::bus_publish::open_bus(self.bus_root.clone()) {
            crate::bus_publish::publish_json(
                &mut persist,
                &firms_state_topic(&self.host),
                snapshot,
            );
        }
    }

    fn status_snapshot(
        &self,
        availability: FirmsAvailability,
        context: Option<FirmsContext>,
        gap: impl Into<String>,
    ) -> FirmsSnapshot {
        FirmsSnapshot::status(
            &self.host,
            now_ms(),
            &self.source,
            availability,
            context.map(|context| (context.latitude, context.longitude)),
            gap,
        )
    }

    fn apply_result(
        &self,
        result: Result<FirmsSnapshot, ProbeFailure>,
        context: FirmsContext,
        last_good: &mut Option<FirmsSnapshot>,
    ) -> (bool, Option<Duration>, bool) {
        match result {
            Ok(snapshot) => {
                self.publish(&snapshot);
                *last_good = Some(snapshot);
                (true, None, false)
            }
            Err(error) => {
                if let Some(snapshot) = last_good {
                    snapshot.published_at_ms = now_ms();
                    snapshot
                        .gaps
                        .retain(|gap| !gap.starts_with("NASA FIRMS paused:"));
                    push_gap(&mut snapshot.gaps, format!("NASA FIRMS paused: {error}"));
                    self.publish(snapshot);
                } else {
                    self.publish(&self.status_snapshot(
                        FirmsAvailability::Ready,
                        Some(context),
                        format!("NASA FIRMS paused: {error}"),
                    ));
                }
                (false, error.retry_after, error.reload_key)
            }
        }
    }

    async fn load_probe(
        &self,
        shutdown: &mut ShutdownToken,
    ) -> Option<Result<Option<Arc<dyn FirmsProbe>>, ProbeFailure>> {
        let source = self.source.clone();
        let key_source = self.key_source.clone();
        let task = tokio::task::spawn_blocking(move || key_source.load());
        tokio::select! {
            () = shutdown.wait() => None,
            joined = task => Some(match joined {
                Ok(Ok(Some(key))) => FirmsHttpProbe::new(key, source)
                    .map(|probe| Some(Arc::new(probe) as Arc<dyn FirmsProbe>)),
                Ok(Ok(None)) => Ok(None),
                Ok(Err(error)) => Err(ProbeFailure::other(format!("NASA FIRMS secret store unavailable: {error}"))),
                Err(error) => Err(ProbeFailure::other(format!("NASA FIRMS secret task failed: {error}"))),
            }),
        }
    }

    async fn fetch_async(
        &self,
        probe: Arc<dyn FirmsProbe>,
        context: FirmsContext,
        shutdown: &mut ShutdownToken,
    ) -> Option<Result<FirmsSnapshot, ProbeFailure>> {
        let host = self.host.clone();
        let source = self.source.clone();
        let task = tokio::task::spawn_blocking(move || {
            let fetched_at_ms = now_ms();
            let body = probe.fetch(context, fetched_at_ms)?;
            parse_snapshot(&host, context, &body, fetched_at_ms, &source)
        });
        tokio::select! {
            () = shutdown.wait() => None,
            joined = task => Some(match joined {
                Ok(result) => result,
                Err(error) => Err(ProbeFailure::other(format!("NASA FIRMS fetch task failed: {error}"))),
            }),
        }
    }
}

fn validated_vehicle_context(
    vehicle: &mackes_mesh_types::vehicle::VehicleState,
    expected_host: &str,
    now: i64,
) -> Option<FirmsContext> {
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
    let context = FirmsContext {
        latitude: gps.latitude,
        longitude: gps.longitude,
    };
    validate_context(context).ok().map(|()| context)
}

#[async_trait::async_trait]
impl Worker for FirmsOverlayWorker {
    fn name(&self) -> &'static str {
        "firms_overlay"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        if !self.enabled {
            shutdown.wait().await;
            return Ok(());
        }
        let mut probe = self.probe.clone();
        let mut last_good: Option<FirmsSnapshot> = None;
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
                            self.publish(&FirmsSnapshot::unconfigured(
                                &self.host,
                                now_ms(),
                                &self.source,
                            ));
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
                            FirmsAvailability::SecretStoreError,
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
                    self.publish(&self.status_snapshot(
                        FirmsAvailability::Ready,
                        None,
                        "fresh same-host vehicle fix unavailable",
                    ));
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
                    probe.clone().expect("probe initialized"),
                    context,
                    &mut shutdown,
                )
                .await
            else {
                break;
            };
            let (success, retry_after, reload_key) =
                self.apply_result(result, context, &mut last_good);
            if reload_key {
                probe = None;
            }
            let delay = if success {
                POLL
            } else {
                retry_after.unwrap_or(retry).max(RETRY_MIN).min(RETRY_MAX)
            };
            retry = if success {
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
    use super::*;

    const CSV: &str = "latitude,longitude,bright_ti4,frp,acq_date,acq_time,satellite,confidence\n35.78,-78.64,331.2,18.4,2026-07-23,123456,N20,nominal\n35.80,-78.60,,,,2026-07-23,124000,N20,low\n";

    #[test]
    fn query_url_is_keyed_path_without_query_leak() {
        let url = query_url(
            FirmsContext {
                latitude: 35.78,
                longitude: -78.64,
            },
            DEFAULT_SOURCE,
            "abcdefghijklmnop",
        )
        .expect("valid query");
        assert!(url.as_str().contains("/abcdefghijklmnop/"));
        assert!(url.query().is_none());
        assert!(validate_endpoint(DEFAULT_ENDPOINT).is_ok());
        assert!(validate_endpoint("http://firms.modaps.eosdis.nasa.gov/api/area/csv/").is_err());
    }

    #[test]
    fn csv_parser_normalizes_valid_hotspots_and_omits_bad_rows() {
        let snapshot = parse_snapshot(
            "rig-1",
            FirmsContext {
                latitude: 35.78,
                longitude: -78.64,
            },
            CSV,
            1_800_000_000_000,
            DEFAULT_SOURCE,
        )
        .expect("parse CSV");
        assert_eq!(snapshot.hotspots.len(), 1);
        assert_eq!(snapshot.hotspots[0].satellite.as_deref(), Some("N20"));
        assert_eq!(snapshot.hotspots[0].confidence.as_deref(), Some("nominal"));
        assert_eq!(snapshot.hotspots[0].brightness_k, Some(331.2));
        assert_eq!(snapshot.hotspots[0].frp_mw, Some(18.4));
    }

    #[test]
    fn csv_quotes_and_required_headers_are_checked() {
        assert_eq!(
            split_csv_line("a,\"b,b\",\"c\"\"d\"").unwrap(),
            ["a", "b,b", "c\"d"]
        );
        assert!(parse_snapshot(
            "rig-1",
            FirmsContext {
                latitude: 35.78,
                longitude: -78.64,
            },
            "latitude,longitude\n35,-78\n",
            1,
            DEFAULT_SOURCE,
        )
        .is_err());
    }

    #[test]
    fn csv_duplicate_or_empty_headers_are_rejected_before_row_parsing() {
        let duplicate = parse_snapshot(
            "rig-1",
            FirmsContext {
                latitude: 35.78,
                longitude: -78.64,
            },
            "latitude,longitude,acq_date,acq_time,latitude\n35.78,-78.64,2026-07-23,123456,\n",
            1,
            DEFAULT_SOURCE,
        )
        .unwrap_err();
        assert!(duplicate.to_string().contains("duplicate headers"));

        let empty = parse_snapshot(
            "rig-1",
            FirmsContext {
                latitude: 35.78,
                longitude: -78.64,
            },
            "latitude,,acq_date,acq_time\n35.78,-78.64,2026-07-23,123456\n",
            1,
            DEFAULT_SOURCE,
        )
        .unwrap_err();
        assert!(empty.to_string().contains("empty header"));
    }

    #[test]
    fn csv_field_count_is_bounded_and_bad_rows_fail_soft() {
        let mut excessive_fields = vec![
            "35.78".to_string(),
            "-78.64".to_string(),
            "331.2".to_string(),
            "18.4".to_string(),
            "2026-07-23".to_string(),
            "123456".to_string(),
            "N20".to_string(),
            "nominal".to_string(),
        ];
        excessive_fields.extend(std::iter::repeat(String::from("unexpected")).take(MAX_CSV_FIELDS));
        let body = format!(
            "latitude,longitude,bright_ti4,frp,acq_date,acq_time,satellite,confidence\n{}\n35.80,-78.60,331.0,17.0,2026-07-23,124000,N20,low\n",
            excessive_fields.join(",")
        );
        let snapshot = parse_snapshot(
            "rig-1",
            FirmsContext {
                latitude: 35.78,
                longitude: -78.64,
            },
            &body,
            1,
            DEFAULT_SOURCE,
        )
        .expect("hostile row should be omitted, not abort the feed");
        assert_eq!(snapshot.hotspots.len(), 1);
        assert_eq!(snapshot.omitted_records, 1);
        assert!(snapshot.gaps.iter().any(|gap| gap.contains("field count")));
    }

    #[test]
    fn official_probe_response_is_bounded_before_csv_parse() {
        let mut response = std::io::Cursor::new(vec![b'x'; MAX_BODY_BYTES + 1]);
        let error = read_bounded(&mut response).unwrap_err();
        assert!(error.to_string().contains("exceeds byte limit"));
    }

    #[test]
    fn stale_or_wrong_host_vehicle_fix_is_rejected() {
        let vehicle = mackes_mesh_types::vehicle::VehicleState::offline("rig-1");
        assert!(validated_vehicle_context(&vehicle, "rig-1", 100).is_none());
    }

    #[test]
    fn bounded_key_and_source_validation_reject_path_injection() {
        assert!(validate_api_key("short").is_err());
        assert!(validate_api_key("abcdefghijklmnop/secret").is_err());
        assert!(validate_source("VIIRS_NOAA20_NRT").is_ok());
        assert!(validate_source("../../secret").is_err());
    }
}
