//! WL-FUNC-012 / OVERLAY-6 — credential-gated NASA FIRMS hotspots.
//!
//! FIRMS is a useful context layer, not a safety-of-life feed.  The worker
//! therefore publishes an honest retained status by default, but requires a
//! sealed MAP_KEY and a fresh same-host vehicle fix before it makes a request.
//! Every response is bounded, validated, and published as one complete
//! latest-wins snapshot.

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

/// Optional overlay opt-out. Unset/unknown truthy values keep the retained
/// status mirror present; `0|false|no|off` disable it entirely.
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

#[derive(Debug, Clone, Copy)]
struct ApplyOutcome {
    success: bool,
    publication_committed: bool,
    retry_after: Option<Duration>,
    reload_key: bool,
}

/// Workstation-side credential-gated NASA FIRMS adapter.
pub struct FirmsOverlayWorker {
    host: String,
    enabled: bool,
    probe: Option<Arc<dyn FirmsProbe>>,
    key_source: Arc<dyn ApiKeySource>,
    /// Explicit override. Without one, each transaction resolves the current
    /// user Bus root and falls back to the canonical system spool.
    bus_root_override: Option<PathBuf>,
    source: String,
}

impl FirmsOverlayWorker {
    /// Production wiring. Present by default; set
    /// `MDE_OVERLAY_FIRMS_HOTSPOTS=0` to suppress this optional external-feed
    /// topic entirely.
    #[must_use]
    pub fn new(host: String) -> Self {
        Self {
            host,
            enabled: env_default_enabled(ENABLED_ENV),
            probe: None,
            key_source: Arc::new(SealedApiKeySource),
            bus_root_override: None,
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

    /// Override Bus access. `None` restores per-transaction production
    /// resolution; it no longer freezes this worker in a disabled state.
    #[must_use]
    pub fn with_bus_root(mut self, root: Option<PathBuf>) -> Self {
        self.bus_root_override = root;
        self
    }

    fn bus_root(&self) -> PathBuf {
        firms_bus_root(self.bus_root_override.clone())
    }

    /// `Ok(None)` is reserved for a successfully read, genuinely absent or
    /// stale vehicle fix. Bus/open/read/decode failures defer without an empty
    /// status that could masquerade as valid context loss.
    fn current_context(&self) -> io::Result<Option<FirmsContext>> {
        let persist = mde_bus::persist::Persist::open(self.bus_root()).map_err(io_other)?;
        let topic = mackes_mesh_types::vehicle::vehicle_state_topic(&self.host);
        let Some(message) = persist.read_latest(&topic).map_err(io_other)? else {
            return Ok(None);
        };
        let body = message
            .body
            .ok_or_else(|| io::Error::other("vehicle context message has no body"))?;
        let vehicle: mackes_mesh_types::vehicle::VehicleState =
            serde_json::from_str(&body).map_err(io_other)?;
        Ok(validated_vehicle_context(&vehicle, &self.host, now_ms()))
    }

    fn publish(&self, snapshot: &FirmsSnapshot) -> io::Result<()> {
        let body = serde_json::to_string(snapshot).map_err(io_other)?;
        let persist = mde_bus::persist::Persist::open(self.bus_root()).map_err(io_other)?;
        self.publish_to(&persist, &body)
    }

    fn publish_to(&self, persist: &mde_bus::persist::Persist, body: &str) -> io::Result<()> {
        persist
            .write(
                &firms_state_topic(&self.host),
                mde_bus::hooks::config::Priority::Default,
                None,
                Some(body),
            )
            .map_err(io_other)?;
        Ok(())
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
    ) -> ApplyOutcome {
        match result {
            Ok(snapshot) => match self.publish(&snapshot) {
                Ok(()) => {
                    *last_good = Some(snapshot);
                    ApplyOutcome {
                        success: true,
                        publication_committed: true,
                        retry_after: None,
                        reload_key: false,
                    }
                }
                Err(error) => {
                    tracing::warn!(target: "mackesd::firms_overlay", host = %self.host, %error, "FIRMS snapshot publication failed; refresh remains uncommitted");
                    ApplyOutcome {
                        success: false,
                        publication_committed: false,
                        retry_after: None,
                        reload_key: false,
                    }
                }
            },
            Err(error) => {
                let previous_context = last_good.as_ref().and_then(|snapshot| {
                    Some(FirmsContext {
                        latitude: snapshot.query_latitude?,
                        longitude: snapshot.query_longitude?,
                    })
                });
                let reason = if previous_context.is_some_and(|previous| previous != context) {
                    format!(
                        "NASA FIRMS paused: prior-location hotspots withheld after refresh failure: {error}"
                    )
                } else {
                    format!("NASA FIRMS paused: refresh unavailable; hotspots withheld: {error}")
                };
                match self.publish(&self.status_snapshot(
                    FirmsAvailability::Ready,
                    Some(context),
                    reason,
                )) {
                    Ok(()) => {
                        *last_good = None;
                        ApplyOutcome {
                            success: false,
                            publication_committed: true,
                            retry_after: error.retry_after,
                            reload_key: error.reload_key,
                        }
                    }
                    Err(publish_error) => {
                        tracing::warn!(target: "mackesd::firms_overlay", host = %self.host, error = %publish_error, "FIRMS degraded publication failed; provider outcome remains uncommitted");
                        ApplyOutcome {
                            success: false,
                            publication_committed: false,
                            retry_after: None,
                            reload_key: false,
                        }
                    }
                }
            }
        }
    }

    /// A process-local suppression flag is only a hint to inspect the current
    /// Bus. Suppress only while this exact status (apart from its publication
    /// time) is retained in the current index; a cleared or replaced index
    /// must receive its own row.
    fn ensure_status_published(
        &self,
        snapshot: &FirmsSnapshot,
        last_good: &mut Option<FirmsSnapshot>,
        published: &mut bool,
    ) -> io::Result<()> {
        let persist = mde_bus::persist::Persist::open(self.bus_root()).map_err(io_other)?;
        if *published {
            let current = persist
                .read_latest(&firms_state_topic(&self.host))
                .map_err(io_other)?
                .and_then(|message| message.body)
                .map(|body| serde_json::from_str::<FirmsSnapshot>(&body).map_err(io_other))
                .transpose()?;
            if current.is_some_and(|current| same_status_except_published_at(&current, snapshot)) {
                return Ok(());
            }
        }

        let body = serde_json::to_string(snapshot).map_err(io_other)?;
        self.publish_to(&persist, &body)?;
        *last_good = None;
        *published = true;
        Ok(())
    }

    fn ensure_no_context_published(
        &self,
        last_good: &mut Option<FirmsSnapshot>,
        no_fix_published: &mut bool,
    ) -> io::Result<()> {
        self.ensure_status_published(
            &self.status_snapshot(
                FirmsAvailability::Ready,
                None,
                "NASA FIRMS paused: fresh same-host vehicle fix unavailable",
            ),
            last_good,
            no_fix_published,
        )
    }

    fn ensure_unconfigured_published(
        &self,
        last_good: &mut Option<FirmsSnapshot>,
        unconfigured_published: &mut bool,
    ) -> io::Result<()> {
        self.ensure_status_published(
            &FirmsSnapshot::unconfigured(&self.host, now_ms(), &self.source),
            last_good,
            unconfigured_published,
        )
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

fn firms_bus_root(override_root: Option<PathBuf>) -> PathBuf {
    firms_bus_root_or_system(override_root.or_else(mde_bus::default_data_dir))
}

fn firms_bus_root_or_system(resolved: Option<PathBuf>) -> PathBuf {
    resolved.unwrap_or_else(|| PathBuf::from(mde_bus::SYSTEM_BUS_ROOT))
}

fn same_status_except_published_at(current: &FirmsSnapshot, expected: &FirmsSnapshot) -> bool {
    let mut current = current.clone();
    current.published_at_ms = expected.published_at_ms;
    current == *expected
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
                        let current_bus_has_unconfigured = match self.ensure_unconfigured_published(
                            &mut last_good,
                            &mut unconfigured_published,
                        ) {
                            Ok(()) => {
                                retry = POLL;
                                true
                            }
                            Err(error) => {
                                tracing::warn!(target: "mackesd::firms_overlay", host = %self.host, %error, "FIRMS unconfigured status transaction failed");
                                false
                            }
                        };
                        let delay = if current_bus_has_unconfigured {
                            POLL
                        } else {
                            retry
                        };
                        tokio::select! {
                            () = shutdown.wait() => break,
                            () = tokio::time::sleep(delay) => {}
                        }
                        continue;
                    }
                    Err(error) => {
                        let published = self
                            .publish(&self.status_snapshot(
                                FirmsAvailability::SecretStoreError,
                                None,
                                error.to_string(),
                            ))
                            .is_ok();
                        if !published {
                            tracing::warn!(target: "mackesd::firms_overlay", host = %self.host, "FIRMS secret-store status publication failed");
                        }
                        let delay = if published { POLL } else { retry };
                        tokio::select! {
                            () = shutdown.wait() => break,
                            () = tokio::time::sleep(delay) => {}
                        }
                        continue;
                    }
                }
            }
            let context = match self.current_context() {
                Ok(Some(context)) => context,
                Ok(None) => {
                    let current_bus_has_empty = match self
                        .ensure_no_context_published(&mut last_good, &mut no_fix_published)
                    {
                        Ok(()) => {
                            retry = POLL;
                            true
                        }
                        Err(error) => {
                            tracing::warn!(target: "mackesd::firms_overlay", host = %self.host, %error, "FIRMS no-fix status transaction failed");
                            false
                        }
                    };
                    let delay = if current_bus_has_empty { POLL } else { retry };
                    tokio::select! {
                        () = shutdown.wait() => break,
                        () = tokio::time::sleep(delay) => {}
                    }
                    continue;
                }
                Err(error) => {
                    tracing::warn!(target: "mackesd::firms_overlay", host = %self.host, %error, "vehicle context read failed; FIRMS pass deferred");
                    tokio::select! {
                        () = shutdown.wait() => break,
                        () = tokio::time::sleep(retry) => {}
                    }
                    continue;
                }
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
            // The blocking FIRMS result is staged. Fresh-open and decode the
            // exact context again before any write so movement, loss, Bus
            // replacement, or read faults cannot admit stale-location data.
            let outcome = match self.current_context() {
                Ok(Some(current)) if current == context => {
                    self.apply_result(result, current, &mut last_good)
                }
                Ok(Some(current)) => self.apply_result(
                    Err(ProbeFailure::other(
                        "vehicle context changed while FIRMS query was in flight",
                    )),
                    current,
                    &mut last_good,
                ),
                Ok(None) => {
                    match self.ensure_no_context_published(&mut last_good, &mut no_fix_published) {
                        Ok(()) => ApplyOutcome {
                            success: false,
                            publication_committed: true,
                            retry_after: None,
                            reload_key: false,
                        },
                        Err(error) => {
                            tracing::warn!(target: "mackesd::firms_overlay", host = %self.host, %error, "post-fetch FIRMS no-fix status publication failed");
                            ApplyOutcome {
                                success: false,
                                publication_committed: false,
                                retry_after: None,
                                reload_key: false,
                            }
                        }
                    }
                }
                Err(error) => {
                    tracing::warn!(target: "mackesd::firms_overlay", host = %self.host, %error, "post-fetch vehicle context read failed; FIRMS result discarded");
                    ApplyOutcome {
                        success: false,
                        publication_committed: false,
                        retry_after: None,
                        reload_key: false,
                    }
                }
            };
            if outcome.publication_committed {
                unconfigured_published = false;
            }
            if outcome.reload_key && self.probe.is_none() {
                probe = None;
            }
            let delay = if outcome.success {
                POLL
            } else {
                outcome
                    .retry_after
                    .unwrap_or(retry)
                    .max(RETRY_MIN)
                    .min(RETRY_MAX)
            };
            retry = if outcome.success {
                POLL
            } else if outcome.publication_committed {
                retry.saturating_mul(2).min(RETRY_MAX)
            } else {
                retry
            };
            tokio::select! {
                () = shutdown.wait() => break,
                () = tokio::time::sleep(delay) => {}
            }
        }
        Ok(())
    }
}

fn env_default_enabled(name: &str) -> bool {
    overlay_enabled_from_env(std::env::var(name).ok().as_deref())
}

fn overlay_enabled_from_env(value: Option<&str>) -> bool {
    !matches!(
        value.map(|value| value.trim().to_ascii_lowercase()),
        Some(value) if matches!(value.as_str(), "0" | "false" | "no" | "off")
    )
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use mackes_mesh_types::vehicle::{GpsFix, VehicleState};
    use mde_bus::persist::Persist;

    use super::*;

    const CSV: &str = "latitude,longitude,bright_ti4,frp,acq_date,acq_time,satellite,confidence\n35.78,-78.64,331.2,18.4,2026-07-23,123456,N20,nominal\n35.80,-78.60,,,,2026-07-23,124000,N20,low\n";

    fn context() -> FirmsContext {
        FirmsContext {
            latitude: 35.78,
            longitude: -78.64,
        }
    }

    fn vehicle_state(context: FirmsContext) -> VehicleState {
        let mut vehicle = VehicleState::offline("rig-1");
        vehicle.online = true;
        vehicle.published_at_ms = now_ms();
        vehicle.gps = GpsFix {
            fix_type: "gps".to_string(),
            latitude: context.latitude,
            longitude: context.longitude,
            satellites: 8,
            age_s: 0.0,
            ..GpsFix::default()
        };
        vehicle
    }

    fn publish_vehicle(root: PathBuf, vehicle: &VehicleState) {
        let body = serde_json::to_string(vehicle).expect("vehicle json");
        Persist::open(root)
            .expect("bus")
            .write(
                &mackes_mesh_types::vehicle::vehicle_state_topic("rig-1"),
                mde_bus::hooks::config::Priority::Default,
                None,
                Some(&body),
            )
            .expect("vehicle write");
    }

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
        excessive_fields.extend(std::iter::repeat_n(
            String::from("unexpected"),
            MAX_CSV_FIELDS,
        ));
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

    #[test]
    fn keyed_firms_producer_defaults_on_with_explicit_false_opt_out() {
        assert!(overlay_enabled_from_env(None));
        assert!(overlay_enabled_from_env(Some("")));
        assert!(overlay_enabled_from_env(Some("1")));
        assert!(overlay_enabled_from_env(Some("true")));
        assert!(overlay_enabled_from_env(Some("yes")));
        assert!(overlay_enabled_from_env(Some("on")));
        assert!(overlay_enabled_from_env(Some("unexpected")));
        assert!(!overlay_enabled_from_env(Some("0")));
        assert!(!overlay_enabled_from_env(Some("false")));
        assert!(!overlay_enabled_from_env(Some("NO")));
        assert!(!overlay_enabled_from_env(Some(" off ")));
    }

    #[test]
    fn late_and_replaced_bus_are_reopened_per_transaction() {
        assert_eq!(
            firms_bus_root_or_system(None),
            PathBuf::from(mde_bus::SYSTEM_BUS_ROOT)
        );
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().join("bus");
        std::fs::write(&root, "not a bus directory").expect("blocking file");
        let worker = FirmsOverlayWorker::new("rig-1".to_string()).with_bus_root(Some(root.clone()));
        assert!(worker.current_context().is_err());

        std::fs::remove_file(&root).expect("remove blocking file");
        publish_vehicle(root.clone(), &vehicle_state(context()));
        assert_eq!(worker.current_context().expect("late bus"), Some(context()));

        let retired = temp.path().join("retired-bus");
        std::fs::rename(&root, &retired).expect("replace bus");
        let moved = FirmsContext {
            latitude: 36.10,
            longitude: -79.00,
        };
        publish_vehicle(root, &vehicle_state(moved));
        assert_eq!(
            worker.current_context().expect("replacement bus"),
            Some(moved)
        );
    }

    #[test]
    fn repeated_no_fix_publishes_once_and_replacement_index_gets_one_row() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().join("bus");
        std::fs::write(&root, "not a bus directory").expect("blocking file");
        let worker = FirmsOverlayWorker::new("rig-1".to_string()).with_bus_root(Some(root.clone()));
        let original = parse_snapshot("rig-1", context(), CSV, 1_800_000_000_000, DEFAULT_SOURCE)
            .expect("snapshot");
        let mut last_good = Some(original.clone());
        let mut no_fix_published = false;

        assert!(worker
            .ensure_no_context_published(&mut last_good, &mut no_fix_published)
            .is_err());
        assert!(!no_fix_published, "failed write cannot set suppression");
        assert_eq!(
            last_good
                .as_ref()
                .expect("last-good retained")
                .hotspots
                .len(),
            1,
            "failed write cannot clear last-good"
        );

        std::fs::remove_file(&root).expect("recover bus");
        worker
            .ensure_no_context_published(&mut last_good, &mut no_fix_published)
            .expect("first no-fix publication");
        assert!(no_fix_published);
        assert!(last_good.is_none());
        let first_bus = Persist::open(root.clone()).expect("first bus");
        worker
            .ensure_no_context_published(&mut last_good, &mut no_fix_published)
            .expect("repeated no-fix check");
        assert_eq!(
            first_bus
                .list_since(&firms_state_topic("rig-1"), None)
                .expect("first bus rows")
                .len(),
            1,
            "repeated no-fix checks must not append rows"
        );

        drop(first_bus);
        for suffix in ["", "-wal", "-shm"] {
            let path = PathBuf::from(format!("{}{suffix}", root.join("index.sqlite").display()));
            if let Err(error) = std::fs::remove_file(path) {
                assert_eq!(error.kind(), io::ErrorKind::NotFound);
            }
        }
        let replacement_bus = Persist::open(root.clone()).expect("replacement bus");
        assert!(replacement_bus
            .read_latest(&firms_state_topic("rig-1"))
            .expect("replacement read")
            .is_none());
        last_good = Some(original);
        worker
            .ensure_no_context_published(&mut last_good, &mut no_fix_published)
            .expect("replacement no-fix publication");
        assert!(last_good.is_none());
        worker
            .ensure_no_context_published(&mut last_good, &mut no_fix_published)
            .expect("replacement repeated check");
        let rows = replacement_bus
            .list_since(&firms_state_topic("rig-1"), None)
            .expect("replacement rows");
        assert_eq!(rows.len(), 1, "replacement index receives exactly one row");
        let snapshot: FirmsSnapshot =
            serde_json::from_str(rows[0].body.as_deref().expect("replacement body"))
                .expect("replacement snapshot");
        assert_eq!(snapshot.host, "rig-1");
        assert!(snapshot.hotspots.is_empty());
        assert_eq!(snapshot.query_latitude, None);
        assert_eq!(snapshot.query_longitude, None);
    }

    #[test]
    fn repeated_unconfigured_publishes_once_and_replacement_index_gets_one_row() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().join("bus");
        let worker = FirmsOverlayWorker::new("rig-1".to_string()).with_bus_root(Some(root.clone()));
        let original = parse_snapshot("rig-1", context(), CSV, 1_800_000_000_000, DEFAULT_SOURCE)
            .expect("snapshot");
        let mut last_good = Some(original.clone());
        let mut unconfigured_published = false;

        worker
            .ensure_unconfigured_published(&mut last_good, &mut unconfigured_published)
            .expect("first unconfigured publication");
        assert!(unconfigured_published);
        assert!(last_good.is_none());
        let first_bus = Persist::open(root.clone()).expect("first bus");
        worker
            .ensure_unconfigured_published(&mut last_good, &mut unconfigured_published)
            .expect("repeated unconfigured check");
        assert_eq!(
            first_bus
                .list_since(&firms_state_topic("rig-1"), None)
                .expect("first bus rows")
                .len(),
            1,
            "repeated unconfigured checks must not append rows"
        );

        drop(first_bus);
        for suffix in ["", "-wal", "-shm"] {
            let path = PathBuf::from(format!("{}{suffix}", root.join("index.sqlite").display()));
            if let Err(error) = std::fs::remove_file(path) {
                assert_eq!(error.kind(), io::ErrorKind::NotFound);
            }
        }
        let replacement_bus = Persist::open(root.clone()).expect("replacement bus");
        assert!(replacement_bus
            .read_latest(&firms_state_topic("rig-1"))
            .expect("replacement read")
            .is_none());
        last_good = Some(original);
        worker
            .ensure_unconfigured_published(&mut last_good, &mut unconfigured_published)
            .expect("replacement unconfigured publication");
        assert!(last_good.is_none());
        worker
            .ensure_unconfigured_published(&mut last_good, &mut unconfigured_published)
            .expect("replacement repeated check");
        let rows = replacement_bus
            .list_since(&firms_state_topic("rig-1"), None)
            .expect("replacement rows");
        assert_eq!(rows.len(), 1, "replacement index receives exactly one row");
        let snapshot: FirmsSnapshot =
            serde_json::from_str(rows[0].body.as_deref().expect("replacement body"))
                .expect("replacement snapshot");
        assert_eq!(snapshot.host, "rig-1");
        assert_eq!(snapshot.availability, FirmsAvailability::Unconfigured);
        assert!(snapshot.hotspots.is_empty());
        assert_eq!(snapshot.fetched_at_ms, None);
        assert_eq!(snapshot.query_latitude, None);
        assert_eq!(snapshot.query_longitude, None);
    }

    struct CountingProbe(Arc<AtomicUsize>);

    impl FirmsProbe for CountingProbe {
        fn fetch(
            &self,
            _context: FirmsContext,
            _fetched_at_ms: i64,
        ) -> Result<String, ProbeFailure> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(CSV.to_string())
        }
    }

    #[tokio::test]
    async fn failed_context_read_defers_without_fetch_or_publication() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().to_path_buf();
        Persist::open(root.clone())
            .expect("bus")
            .write(
                &mackes_mesh_types::vehicle::vehicle_state_topic("rig-1"),
                mde_bus::hooks::config::Priority::Default,
                None,
                Some("{malformed"),
            )
            .expect("malformed context write");
        let calls = Arc::new(AtomicUsize::new(0));
        let mut worker = FirmsOverlayWorker::new("rig-1".to_string())
            .with_bus_root(Some(root.clone()))
            .with_probe(Arc::new(CountingProbe(calls.clone())));
        let (tx, rx) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(async move { worker.run(ShutdownToken::from_receiver(rx)).await });

        tokio::time::sleep(Duration::from_millis(30)).await;
        tx.send(true).expect("shutdown");
        task.await.expect("join").expect("worker");

        assert_eq!(calls.load(Ordering::Relaxed), 0);
        assert!(Persist::open(root)
            .expect("bus")
            .read_latest(&firms_state_topic("rig-1"))
            .expect("FIRMS read")
            .is_none());
    }

    struct BlockingProbe {
        started: Arc<AtomicBool>,
        release: Arc<AtomicBool>,
    }

    impl FirmsProbe for BlockingProbe {
        fn fetch(
            &self,
            _context: FirmsContext,
            _fetched_at_ms: i64,
        ) -> Result<String, ProbeFailure> {
            self.started.store(true, Ordering::Release);
            while !self.release.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(1));
            }
            Ok(CSV.to_string())
        }
    }

    #[tokio::test]
    async fn post_fetch_context_change_withholds_stale_hotspot_result() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().to_path_buf();
        publish_vehicle(root.clone(), &vehicle_state(context()));
        let started = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(false));
        let probe = BlockingProbe {
            started: started.clone(),
            release: release.clone(),
        };
        let mut worker = FirmsOverlayWorker::new("rig-1".to_string())
            .with_bus_root(Some(root.clone()))
            .with_probe(Arc::new(probe));
        let (tx, rx) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(async move { worker.run(ShutdownToken::from_receiver(rx)).await });
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !started.load(Ordering::Acquire) {
            assert!(std::time::Instant::now() < deadline, "fetch did not start");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        let moved = FirmsContext {
            latitude: 36.10,
            longitude: -79.00,
        };
        publish_vehicle(root.clone(), &vehicle_state(moved));
        release.store(true, Ordering::Release);

        let snapshot = loop {
            if let Some(body) = Persist::open(root.clone())
                .expect("bus")
                .read_latest(&firms_state_topic("rig-1"))
                .expect("read")
                .and_then(|message| message.body)
            {
                break serde_json::from_str::<FirmsSnapshot>(&body).expect("snapshot");
            }
            assert!(
                std::time::Instant::now() < deadline,
                "post-fetch context result was not published"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        tx.send(true).expect("shutdown");
        task.await.expect("join").expect("worker");

        assert!(snapshot.hotspots.is_empty());
        assert_eq!(snapshot.fetched_at_ms, None);
        assert_eq!(snapshot.query_latitude, Some(moved.latitude));
        assert_eq!(snapshot.query_longitude, Some(moved.longitude));
        assert!(snapshot
            .gaps
            .iter()
            .any(|gap| gap.contains("context changed")));
    }

    #[test]
    fn write_failure_defers_hotspot_clear_and_key_reload_then_corrects_forward() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().join("bus");
        std::fs::write(&root, "not a bus directory").expect("blocking file");
        let worker = FirmsOverlayWorker::new("rig-1".to_string()).with_bus_root(Some(root.clone()));
        let snapshot = parse_snapshot("rig-1", context(), CSV, 1_800_000_000_000, DEFAULT_SOURCE)
            .expect("snapshot");
        let mut last_good = Some(snapshot.clone());

        let failed = worker.apply_result(
            Err(ProbeFailure::authentication()),
            context(),
            &mut last_good,
        );
        assert!(!failed.success);
        assert!(!failed.publication_committed);
        assert_eq!(failed.retry_after, None);
        assert!(!failed.reload_key);
        assert_eq!(last_good.as_ref().expect("private state").hotspots.len(), 1);

        std::fs::remove_file(&root).expect("recover bus");
        let corrected = worker.apply_result(
            Err(ProbeFailure::authentication()),
            context(),
            &mut last_good,
        );
        assert!(!corrected.success);
        assert!(corrected.publication_committed);
        assert_eq!(corrected.retry_after, Some(POLL));
        assert!(corrected.reload_key);
        assert!(last_good.is_none());
        let rows = Persist::open(root)
            .expect("bus")
            .list_since(&firms_state_topic("rig-1"), None)
            .expect("rows");
        assert_eq!(rows.len(), 1);
        let corrected: FirmsSnapshot =
            serde_json::from_str(rows[0].body.as_deref().expect("corrected-forward body"))
                .expect("corrected-forward snapshot");
        assert!(corrected.hotspots.is_empty());
    }

    #[test]
    fn failed_refresh_publishes_empty_degraded_snapshot_without_replaying_hotspots() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().to_path_buf();
        let worker = FirmsOverlayWorker::new("rig-1".to_string()).with_bus_root(Some(root.clone()));
        let original = parse_snapshot(
            "rig-1",
            FirmsContext {
                latitude: 35.78,
                longitude: -78.64,
            },
            CSV,
            1_800_000_000_000,
            DEFAULT_SOURCE,
        )
        .expect("snapshot");
        assert_eq!(original.hotspots.len(), 1);
        let moved = FirmsContext {
            latitude: 36.10,
            longitude: -79.00,
        };
        let mut last_good = Some(original);

        let outcome =
            worker.apply_result(Err(ProbeFailure::other("timeout")), moved, &mut last_good);

        assert!(!outcome.success);
        assert!(outcome.publication_committed);
        assert_eq!(outcome.retry_after, None);
        assert!(!outcome.reload_key);
        assert!(
            last_good.is_none(),
            "old vehicle-scoped FIRMS hotspot cache must not survive refresh failure"
        );
        let body = Persist::open(root)
            .expect("bus")
            .read_latest(&firms_state_topic("rig-1"))
            .expect("read")
            .expect("message")
            .body
            .expect("body");
        let snapshot: FirmsSnapshot = serde_json::from_str(&body).expect("snapshot");
        assert_eq!(snapshot.availability, FirmsAvailability::Ready);
        assert_eq!(snapshot.fetched_at_ms, None);
        assert_eq!(snapshot.query_latitude, Some(moved.latitude));
        assert_eq!(snapshot.query_longitude, Some(moved.longitude));
        assert!(snapshot.hotspots.is_empty());
        assert!(snapshot.gaps.iter().any(|gap| {
            gap.contains("prior-location hotspots withheld") && gap.contains("timeout")
        }));
    }

    #[test]
    fn no_vehicle_fix_degraded_snapshot_retracts_prior_hotspots_and_query_origin() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().to_path_buf();
        let worker = FirmsOverlayWorker::new("rig-1".to_string()).with_bus_root(Some(root.clone()));
        let original = parse_snapshot(
            "rig-1",
            FirmsContext {
                latitude: 35.78,
                longitude: -78.64,
            },
            CSV,
            1_800_000_000_000,
            DEFAULT_SOURCE,
        )
        .expect("snapshot");
        assert!(!original.hotspots.is_empty());
        let mut last_good = Some(original);
        let mut no_fix_published = false;

        worker
            .ensure_no_context_published(&mut last_good, &mut no_fix_published)
            .expect("publish retraction");

        assert!(
            no_fix_published,
            "no-fix status must be recorded as published"
        );
        assert!(
            last_good.is_none(),
            "old vehicle-scoped FIRMS hotspot cache must not survive fix loss"
        );
        let body = Persist::open(root)
            .expect("bus")
            .read_latest(&firms_state_topic("rig-1"))
            .expect("read")
            .expect("message")
            .body
            .expect("body");
        let snapshot: FirmsSnapshot = serde_json::from_str(&body).expect("snapshot");
        assert_eq!(snapshot.availability, FirmsAvailability::Ready);
        assert_eq!(snapshot.fetched_at_ms, None);
        assert_eq!(snapshot.query_latitude, None);
        assert_eq!(snapshot.query_longitude, None);
        assert!(snapshot.hotspots.is_empty());
        assert!(snapshot
            .gaps
            .iter()
            .any(|gap| gap.contains("vehicle fix unavailable")));
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
            FirmsOverlayWorker::new("rig-1".to_string()).with_bus_root(Some(root.clone()));
        worker.enabled = true;
        worker.key_source = Arc::new(MissingKey);
        let (tx, rx) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(async move { worker.run(ShutdownToken::from_receiver(rx)).await });
        let topic = firms_state_topic("rig-1");
        let mut decoded = None;
        for _ in 0..20 {
            if let Some(body) = Persist::open(root.clone())
                .ok()
                .and_then(|persist| persist.read_latest(&topic).ok().flatten())
                .and_then(|event| event.body)
            {
                decoded = serde_json::from_str::<FirmsSnapshot>(&body).ok();
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        tx.send(true).expect("shutdown");
        task.await.expect("join").expect("worker");
        let snapshot = decoded.expect("unconfigured snapshot");
        assert_eq!(snapshot.availability, FirmsAvailability::Unconfigured);
        assert_eq!(snapshot.fetched_at_ms, None);
        assert_eq!(snapshot.query_latitude, None);
        assert_eq!(snapshot.query_longitude, None);
        assert!(snapshot.hotspots.is_empty());
        assert!(snapshot.gaps[0].contains("firms-api-key"));
    }
}
