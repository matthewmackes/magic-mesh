//! WL-FUNC-012 / OVERLAY-4 — keyless NWS hourly drive-ahead forecast adapter.

#![cfg(feature = "async-services")]

use std::io::{self, Read};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use mackes_mesh_types::nws_forecast::{
    nws_forecast_state_topic, ForecastKind, ForecastPeriod, ForecastSample, NwsForecastSnapshot,
};
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use serde::Deserialize;

use super::{ShutdownToken, Worker};

/// Explicit opt-in; unset/false is an idle no-op.
pub const ENABLED_ENV: &str = "MDE_OVERLAY_NWS_FORECAST";
/// NWS forecast refresh cadence.
pub const POLL: Duration = Duration::from_secs(10 * 60);
const FIX_RETRY: Duration = Duration::from_secs(20);
const RETRY_MAX: Duration = Duration::from_secs(10 * 60);
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_POINTS_BODY_BYTES: usize = 128 * 1024;
const MAX_FORECAST_BODY_BYTES: usize = 256 * 1024;
const MAX_FEED_PERIODS: usize = 192;
const MAX_RETAINED_PERIODS: usize = 24;
const MAX_GAPS: usize = 64;
const MAX_SHORT_FORECAST_BYTES: usize = 128;
const MAX_STRING_BYTES: usize = 64;
const MAX_FEED_FUTURE_SKEW_MS: i64 = 5 * 60 * 1_000;
const MAX_FEED_AGE_MS: i64 = 6 * 60 * 60 * 1_000;
const MAX_PERIOD_DURATION_MS: i64 = 3 * 60 * 60 * 1_000;
const VEHICLE_FIX_MAX_AGE_MS: i64 = 30_000;
const VEHICLE_MAX_FUTURE_SKEW_MS: i64 = 5_000;
const MIN_HEADING_SPEED_MPH: f32 = 5.0;
const MAX_SPEED_MPH: f32 = 180.0;
const DRIVE_AHEAD_KM: [f32; 3] = [0.0, 25.0, 50.0];
const USER_AGENT: &str =
    "Construct/12 mackesd NWS-hourly-overlay (+https://github.com/matthewmackes/magic-mesh)";

/// Finite vehicle context used for current/drive-ahead forecast samples.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ForecastContext {
    /// Current WGS-84 latitude.
    pub latitude: f64,
    /// Current WGS-84 longitude.
    pub longitude: f64,
    /// Trustworthy motion heading, when available.
    pub heading_deg: Option<f32>,
    /// Trustworthy motion speed, when available.
    pub speed_mph: Option<f32>,
}

/// Injectable two-step NWS API seam.
pub trait NwsForecastProbe: Send + Sync {
    /// Fetch `/points/{lat},{lon}`.
    fn fetch_points(&self, point: ForecastPoint) -> io::Result<String>;
    /// Fetch the validated `forecastHourly` URL returned by `/points`.
    fn fetch_hourly(&self, url: &str) -> io::Result<String>;
}

/// One bounded current/drive-ahead point passed through the injectable probe.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ForecastPoint {
    /// WGS-84 latitude.
    pub latitude: f64,
    /// WGS-84 longitude.
    pub longitude: f64,
    /// Distance along the current vehicle heading.
    pub distance_ahead_km: f32,
    /// Estimated arrival time, Unix milliseconds.
    pub eta_at_ms: i64,
}

/// Production rustls NWS probe.
pub struct NwsForecastHttpProbe {
    client: Client,
}

impl NwsForecastHttpProbe {
    fn new() -> io::Result<Self> {
        Ok(Self {
            client: build_http_client()?,
        })
    }
}

impl NwsForecastProbe for NwsForecastHttpProbe {
    fn fetch_points(&self, point: ForecastPoint) -> io::Result<String> {
        validate_coordinates(point.latitude, point.longitude)?;
        let url = format!(
            "https://api.weather.gov/points/{:.4},{:.4}",
            point.latitude, point.longitude
        );
        fetch_bounded_json(&self.client, &url, MAX_POINTS_BODY_BYTES)
    }

    fn fetch_hourly(&self, url: &str) -> io::Result<String> {
        let validated = validate_hourly_url(url)?;
        fetch_bounded_json(&self.client, validated.as_str(), MAX_FORECAST_BODY_BYTES)
    }
}

fn build_http_client() -> io::Result<Client> {
    Client::builder()
        .timeout(HTTP_TIMEOUT)
        .user_agent(USER_AGENT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(io_other)
}

fn fetch_bounded_json(client: &Client, url: &str, max_bytes: usize) -> io::Result<String> {
    let response = client
        .get(url)
        .header(ACCEPT, "application/geo+json")
        .send()
        .map_err(io_other)?;
    if response.status() != reqwest::StatusCode::OK {
        return Err(io::Error::other(format!(
            "NWS returned unexpected HTTP {} (redirects are disabled)",
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
    if !matches!(
        content_type,
        "application/geo+json" | "application/ld+json" | "application/json"
    ) {
        return Err(io::Error::other(format!(
            "NWS returned unexpected content type {content_type:?}"
        )));
    }
    if response
        .content_length()
        .is_some_and(|length| length > u64::try_from(max_bytes).unwrap_or(u64::MAX))
    {
        return Err(io::Error::other(format!(
            "NWS response exceeds {max_bytes} byte limit"
        )));
    }
    let mut response = response;
    read_bounded_string(&mut response, max_bytes)
}

fn read_bounded_string(reader: &mut impl Read, max_bytes: usize) -> io::Result<String> {
    let mut body = Vec::with_capacity(max_bytes.min(96 * 1024));
    reader
        .take(u64::try_from(max_bytes).unwrap_or(u64::MAX) + 1)
        .read_to_end(&mut body)?;
    if body.len() > max_bytes {
        return Err(io::Error::other(format!(
            "NWS response exceeds {max_bytes} byte limit"
        )));
    }
    String::from_utf8(body).map_err(io_other)
}

fn io_other(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

#[derive(Debug, Deserialize)]
struct PointsDocument {
    properties: PointsProperties,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PointsProperties {
    grid_id: String,
    grid_x: i32,
    grid_y: i32,
    forecast_hourly: String,
}

#[derive(Debug, Clone)]
struct GridEndpoint {
    grid_id: String,
    grid_x: i32,
    grid_y: i32,
    url: String,
}

fn parse_points_document(body: &str) -> io::Result<GridEndpoint> {
    if body.len() > MAX_POINTS_BODY_BYTES {
        return Err(io::Error::other("NWS points response exceeds byte limit"));
    }
    let document: PointsDocument = serde_json::from_str(body).map_err(io_other)?;
    let grid_id = document.properties.grid_id.trim();
    if grid_id.len() != 3 || !grid_id.bytes().all(|byte| byte.is_ascii_uppercase()) {
        return Err(io::Error::other("NWS points gridId is invalid"));
    }
    if !(0..=10_000).contains(&document.properties.grid_x)
        || !(0..=10_000).contains(&document.properties.grid_y)
    {
        return Err(io::Error::other("NWS points grid coordinate is invalid"));
    }
    let validated = validate_hourly_url(&document.properties.forecast_hourly)?;
    let expected_path = format!(
        "/gridpoints/{grid_id}/{},{}/forecast/hourly",
        document.properties.grid_x, document.properties.grid_y
    );
    if validated.path() != expected_path {
        return Err(io::Error::other(
            "NWS forecastHourly URL does not match declared grid",
        ));
    }
    Ok(GridEndpoint {
        grid_id: grid_id.to_string(),
        grid_x: document.properties.grid_x,
        grid_y: document.properties.grid_y,
        url: validated.to_string(),
    })
}

fn validate_hourly_url(value: &str) -> io::Result<reqwest::Url> {
    let url = reqwest::Url::parse(value).map_err(io_other)?;
    if url.scheme() != "https"
        || url.host_str() != Some("api.weather.gov")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(io::Error::other(
            "NWS forecastHourly URL is outside the strict official-host allowlist",
        ));
    }
    let segments: Vec<_> = url
        .path_segments()
        .ok_or_else(|| io::Error::other("NWS forecastHourly URL has no path"))?
        .collect();
    if segments.len() != 5
        || segments[0] != "gridpoints"
        || segments[1].len() != 3
        || !segments[1].bytes().all(|byte| byte.is_ascii_uppercase())
        || segments[3] != "forecast"
        || segments[4] != "hourly"
    {
        return Err(io::Error::other(
            "NWS forecastHourly URL path is not canonical",
        ));
    }
    let (x, y) = segments[2]
        .split_once(',')
        .ok_or_else(|| io::Error::other("NWS forecastHourly grid coordinate is invalid"))?;
    let x: i32 = x
        .parse()
        .map_err(|_| io::Error::other("NWS forecastHourly grid X is invalid"))?;
    let y: i32 = y
        .parse()
        .map_err(|_| io::Error::other("NWS forecastHourly grid Y is invalid"))?;
    if !(0..=10_000).contains(&x) || !(0..=10_000).contains(&y) {
        return Err(io::Error::other(
            "NWS forecastHourly grid coordinate is out of range",
        ));
    }
    Ok(url)
}

#[derive(Debug, Deserialize)]
struct ForecastDocument {
    properties: ForecastProperties,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ForecastProperties {
    generated_at: String,
    #[serde(default)]
    periods: Vec<PeriodDocument>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PeriodDocument {
    number: u32,
    start_time: String,
    end_time: String,
    is_daytime: bool,
    temperature: i64,
    temperature_unit: String,
    #[serde(default)]
    probability_of_precipitation: QuantitativeValue,
    #[serde(default)]
    relative_humidity: QuantitativeValue,
    wind_speed: String,
    wind_direction: String,
    short_forecast: String,
}

#[derive(Debug, Default, Deserialize)]
struct QuantitativeValue {
    value: Option<f64>,
}

fn parse_forecast_document(
    body: &str,
    endpoint: &GridEndpoint,
    point: ForecastPoint,
    fetched_at_ms: i64,
    gaps: &mut Vec<String>,
) -> io::Result<(ForecastSample, i64)> {
    if body.len() > MAX_FORECAST_BODY_BYTES {
        return Err(io::Error::other("NWS hourly response exceeds byte limit"));
    }
    let document: ForecastDocument = serde_json::from_str(body).map_err(io_other)?;
    let generated_at_ms = parse_time_ms(&document.properties.generated_at)?;
    if generated_at_ms.saturating_sub(fetched_at_ms) > MAX_FEED_FUTURE_SKEW_MS {
        return Err(io::Error::other(
            "NWS generatedAt timestamp is implausibly in the future",
        ));
    }
    if fetched_at_ms.saturating_sub(generated_at_ms) > MAX_FEED_AGE_MS {
        return Err(io::Error::other(
            "NWS generatedAt timestamp is more than six hours old",
        ));
    }
    if document.properties.periods.len() > MAX_FEED_PERIODS {
        push_gap(
            gaps,
            format!(
                "grid {} contains {} periods; only the first {MAX_FEED_PERIODS} are processed",
                endpoint.grid_id,
                document.properties.periods.len()
            ),
        );
    }
    let mut periods = Vec::new();
    for (index, period) in document
        .properties
        .periods
        .into_iter()
        .take(MAX_FEED_PERIODS)
        .enumerate()
    {
        match parse_period(period, fetched_at_ms) {
            Ok(Some(period)) if periods.len() < MAX_RETAINED_PERIODS => periods.push(period),
            Ok(Some(_)) => {
                if !gaps
                    .iter()
                    .any(|gap| gap.contains("retained periods capped"))
                {
                    push_gap(
                        gaps,
                        format!("retained periods capped at {MAX_RETAINED_PERIODS} per grid"),
                    );
                }
            }
            Ok(None) => {}
            Err(error) => push_gap(
                gaps,
                format!("grid {} period {index} omitted: {error}", endpoint.grid_id),
            ),
        }
    }
    if periods.is_empty() {
        return Err(io::Error::other(
            "NWS hourly response has no current periods",
        ));
    }
    Ok((
        ForecastSample {
            distance_ahead_km: point.distance_ahead_km,
            eta_at_ms: point.eta_at_ms,
            latitude: point.latitude,
            longitude: point.longitude,
            grid_id: endpoint.grid_id.clone(),
            grid_x: endpoint.grid_x,
            grid_y: endpoint.grid_y,
            periods,
        },
        generated_at_ms,
    ))
}

fn parse_period(period: PeriodDocument, now_ms: i64) -> io::Result<Option<ForecastPeriod>> {
    let start_at_ms = parse_time_ms(&period.start_time)?;
    let end_at_ms = parse_time_ms(&period.end_time)?;
    if end_at_ms <= start_at_ms || end_at_ms.saturating_sub(start_at_ms) > MAX_PERIOD_DURATION_MS {
        return Err(io::Error::other("invalid period interval"));
    }
    if end_at_ms <= now_ms {
        return Ok(None);
    }
    let number =
        u16::try_from(period.number).map_err(|_| io::Error::other("period number exceeds u16"))?;
    let temperature = i16::try_from(period.temperature)
        .ok()
        .filter(|value| (-150..=150).contains(value))
        .ok_or_else(|| io::Error::other("temperature is implausible"))?;
    let temperature_unit = bounded_ascii(&period.temperature_unit, 1)
        .filter(|value| matches!(value.as_str(), "F" | "C"))
        .ok_or_else(|| io::Error::other("temperature unit is invalid"))?;
    let wind_speed = bounded_ascii(&period.wind_speed, MAX_STRING_BYTES)
        .ok_or_else(|| io::Error::other("wind speed text is invalid"))?;
    let wind_direction = bounded_ascii(&period.wind_direction, 8)
        .ok_or_else(|| io::Error::other("wind direction is invalid"))?;
    let short_forecast = bounded_ascii(&period.short_forecast, MAX_SHORT_FORECAST_BYTES)
        .ok_or_else(|| io::Error::other("short forecast is invalid"))?;
    Ok(Some(ForecastPeriod {
        number,
        start_at_ms,
        end_at_ms,
        is_daytime: period.is_daytime,
        temperature,
        temperature_unit,
        precipitation_percent: bounded_percent(period.probability_of_precipitation.value),
        humidity_percent: bounded_percent(period.relative_humidity.value),
        wind_speed,
        wind_direction,
        kind: classify_forecast(&short_forecast),
        short_forecast,
    }))
}

fn bounded_percent(value: Option<f64>) -> Option<u8> {
    value
        .filter(|value| value.is_finite() && (0.0..=100.0).contains(value))
        .map(|value| value.round() as u8)
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

fn classify_forecast(value: &str) -> ForecastKind {
    let value = value.to_ascii_lowercase();
    if value.contains("thunder") {
        ForecastKind::Thunderstorm
    } else if ["snow", "sleet", "freezing", "wintry", "ice"]
        .iter()
        .any(|needle| value.contains(needle))
    {
        ForecastKind::Wintry
    } else if ["rain", "shower", "drizzle"]
        .iter()
        .any(|needle| value.contains(needle))
    {
        ForecastKind::Rain
    } else if ["fog", "smoke", "haze"]
        .iter()
        .any(|needle| value.contains(needle))
    {
        ForecastKind::LowVisibility
    } else if ["wind", "breezy", "gust"]
        .iter()
        .any(|needle| value.contains(needle))
    {
        ForecastKind::Wind
    } else if value.contains("clear") || value.contains("sunny") {
        ForecastKind::Clear
    } else if value.contains("cloud") || value.contains("overcast") {
        ForecastKind::Cloudy
    } else {
        ForecastKind::Unknown
    }
}

fn parse_time_ms(value: &str) -> io::Result<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|timestamp| timestamp.timestamp_millis())
        .map_err(|error| io::Error::other(format!("invalid ISO-8601 timestamp: {error}")))
}

fn build_snapshot(
    probe: &dyn NwsForecastProbe,
    host: &str,
    context: ForecastContext,
    fetched_at_ms: i64,
) -> io::Result<NwsForecastSnapshot> {
    validate_coordinates(context.latitude, context.longitude)?;
    let points = drive_ahead_points(context, fetched_at_ms);
    let mut snapshot =
        NwsForecastSnapshot::empty(host, fetched_at_ms, context.latitude, context.longitude);
    snapshot.heading_deg = context.heading_deg;
    for point in points {
        let result = (|| {
            let points_body = probe.fetch_points(point)?;
            let endpoint = parse_points_document(&points_body)?;
            let hourly_body = probe.fetch_hourly(&endpoint.url)?;
            parse_forecast_document(
                &hourly_body,
                &endpoint,
                point,
                fetched_at_ms,
                &mut snapshot.gaps,
            )
        })();
        match result {
            Ok((sample, generated_at_ms)) => {
                snapshot.feed_generated_at_ms = Some(
                    snapshot
                        .feed_generated_at_ms
                        .map_or(generated_at_ms, |prior| prior.min(generated_at_ms)),
                );
                snapshot.samples.push(sample);
            }
            Err(error) => push_gap(
                &mut snapshot.gaps,
                format!(
                    "forecast {:.0} km ahead unavailable: {error}",
                    point.distance_ahead_km
                ),
            ),
        }
    }
    if snapshot.samples.is_empty() {
        let detail = snapshot
            .gaps
            .first()
            .map(String::as_str)
            .unwrap_or("no sample diagnostics");
        return Err(io::Error::other(format!(
            "all NWS current/drive-ahead forecast samples failed: {detail}"
        )));
    }
    Ok(snapshot)
}

fn drive_ahead_points(context: ForecastContext, now_ms: i64) -> Vec<ForecastPoint> {
    let Some(heading) = context.heading_deg else {
        return vec![ForecastPoint {
            latitude: context.latitude,
            longitude: context.longitude,
            distance_ahead_km: 0.0,
            eta_at_ms: now_ms,
        }];
    };
    let speed_mph = context.speed_mph.unwrap_or(MIN_HEADING_SPEED_MPH);
    DRIVE_AHEAD_KM
        .into_iter()
        .map(|distance_ahead_km| {
            let (latitude, longitude) = destination(
                context.latitude,
                context.longitude,
                f64::from(heading),
                f64::from(distance_ahead_km),
            );
            let travel_ms = if distance_ahead_km == 0.0 {
                0
            } else {
                ((f64::from(distance_ahead_km) / (f64::from(speed_mph) * 1.609_344)) * 3_600_000.0)
                    .round()
                    .clamp(0.0, 6.0 * 3_600_000.0) as i64
            };
            ForecastPoint {
                latitude,
                longitude,
                distance_ahead_km,
                eta_at_ms: now_ms.saturating_add(travel_ms),
            }
        })
        .collect()
}

fn destination(latitude: f64, longitude: f64, bearing_deg: f64, distance_km: f64) -> (f64, f64) {
    let angular = distance_km / 6_371.0;
    let bearing = bearing_deg.to_radians();
    let lat = latitude.to_radians();
    let lon = longitude.to_radians();
    let destination_lat =
        (lat.sin() * angular.cos() + lat.cos() * angular.sin() * bearing.cos()).asin();
    let destination_lon = lon
        + (bearing.sin() * angular.sin() * lat.cos())
            .atan2(angular.cos() - lat.sin() * destination_lat.sin());
    (
        destination_lat.to_degrees(),
        ((destination_lon.to_degrees() + 540.0) % 360.0) - 180.0,
    )
}

fn validate_coordinates(latitude: f64, longitude: f64) -> io::Result<()> {
    if latitude.is_finite()
        && longitude.is_finite()
        && (-90.0..=90.0).contains(&latitude)
        && (-180.0..=180.0).contains(&longitude)
    {
        Ok(())
    } else {
        Err(io::Error::other("forecast point is not finite/in range"))
    }
}

fn push_gap(gaps: &mut Vec<String>, gap: String) {
    if gaps.len() < MAX_GAPS {
        gaps.push(gap);
    } else if gaps
        .last()
        .is_some_and(|last| last != "additional forecast gaps omitted")
    {
        gaps[MAX_GAPS - 1] = "additional forecast gaps omitted".to_string();
    }
}

/// Workstation-side NWS hourly forecast adapter.
pub struct NwsForecastOverlayWorker {
    host: String,
    probe: Option<Arc<dyn NwsForecastProbe>>,
    bus_root: Option<PathBuf>,
    poll: Duration,
}

impl NwsForecastOverlayWorker {
    /// Production wiring. Disabled unless explicitly opted in.
    #[must_use]
    pub fn new(host: String) -> Self {
        let probe = if env_truthy(ENABLED_ENV) {
            match NwsForecastHttpProbe::new() {
                Ok(probe) => Some(Arc::new(probe) as Arc<dyn NwsForecastProbe>),
                Err(error) => {
                    tracing::warn!(target: "mackesd::nws_forecast_overlay", %error, "NWS forecast client unavailable; worker idle");
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
    pub fn with_probe(mut self, probe: Arc<dyn NwsForecastProbe>) -> Self {
        self.probe = Some(probe);
        self
    }

    /// Override or disable Bus access.
    #[must_use]
    pub fn with_bus_root(mut self, root: Option<PathBuf>) -> Self {
        self.bus_root = root;
        self
    }

    fn current_context(&self) -> Result<ForecastContext, String> {
        let root = self
            .bus_root
            .clone()
            .ok_or_else(|| "Bus spool unavailable".to_string())?;
        let persist = mde_bus::persist::Persist::open(root)
            .map_err(|error| format!("vehicle mirror unavailable: {error}"))?;
        let topic = mackes_mesh_types::vehicle::vehicle_state_topic(&self.host);
        let body = persist
            .read_latest(&topic)
            .map_err(|error| format!("vehicle mirror read failed: {error}"))?
            .and_then(|message| message.body)
            .ok_or_else(|| "fresh same-host MG90 fix unavailable".to_string())?;
        let vehicle: mackes_mesh_types::vehicle::VehicleState =
            serde_json::from_str(&body).map_err(|_| "vehicle mirror is malformed".to_string())?;
        validated_vehicle_context(&vehicle, &self.host, now_ms())
            .ok_or_else(|| "fresh same-host MG90 fix unavailable".to_string())
    }

    fn publish(&self, snapshot: &NwsForecastSnapshot) {
        if let Some(mut persist) = crate::bus_publish::open_bus(self.bus_root.clone()) {
            crate::bus_publish::publish_json(
                &mut persist,
                &nws_forecast_state_topic(&self.host),
                snapshot,
            );
        }
    }

    fn publish_unavailable(&self, last_good: &mut Option<NwsForecastSnapshot>, reason: &str) {
        if let Some(snapshot) = last_good {
            snapshot
                .gaps
                .retain(|gap| !gap.starts_with("NWS forecast paused:"));
            push_gap(&mut snapshot.gaps, format!("NWS forecast paused: {reason}"));
            self.publish(snapshot);
        } else {
            self.publish(&NwsForecastSnapshot::unavailable(&self.host, reason));
        }
    }

    fn apply_result(
        &self,
        result: io::Result<NwsForecastSnapshot>,
        last_good: &mut Option<NwsForecastSnapshot>,
    ) -> bool {
        match result {
            Ok(snapshot) => {
                self.publish(&snapshot);
                *last_good = Some(snapshot);
                true
            }
            Err(error) => {
                self.publish_unavailable(last_good, &format!("refresh failed: {error}"));
                false
            }
        }
    }

    async fn fetch_async(
        &self,
        probe: Arc<dyn NwsForecastProbe>,
        context: ForecastContext,
        shutdown: &mut ShutdownToken,
    ) -> Option<io::Result<NwsForecastSnapshot>> {
        let host = self.host.clone();
        let task = tokio::task::spawn_blocking(move || {
            build_snapshot(probe.as_ref(), &host, context, now_ms())
        });
        tokio::select! {
            () = shutdown.wait() => None,
            joined = task => Some(match joined {
                Ok(result) => result,
                Err(error) => Err(io::Error::other(format!("NWS forecast task failed: {error}"))),
            }),
        }
    }
}

fn validated_vehicle_context(
    vehicle: &mackes_mesh_types::vehicle::VehicleState,
    expected_host: &str,
    now: i64,
) -> Option<ForecastContext> {
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
        || !gps.age_s.is_finite()
        || gps.age_s < 0.0
        || future_skew > VEHICLE_MAX_FUTURE_SKEW_MS
        || mirror_age as f64 + f64::from(gps.age_s) * 1_000.0 > VEHICLE_FIX_MAX_AGE_MS as f64
    {
        return None;
    }
    let motion_valid = gps.speed_mph.is_finite()
        && (MIN_HEADING_SPEED_MPH..=MAX_SPEED_MPH).contains(&gps.speed_mph)
        && gps.heading_deg.is_finite()
        && (0.0..360.0).contains(&gps.heading_deg);
    Some(ForecastContext {
        latitude: gps.latitude,
        longitude: gps.longitude,
        heading_deg: motion_valid.then_some(gps.heading_deg),
        speed_mph: motion_valid.then_some(gps.speed_mph),
    })
}

#[async_trait::async_trait]
impl Worker for NwsForecastOverlayWorker {
    fn name(&self) -> &'static str {
        "nws_forecast_overlay"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let Some(probe) = self.probe.clone() else {
            shutdown.wait().await;
            return Ok(());
        };
        let mut last_good = None;
        let mut retry = FIX_RETRY;
        let mut no_fix_published = false;
        loop {
            let context = match self.current_context() {
                Ok(context) => {
                    no_fix_published = false;
                    context
                }
                Err(reason) => {
                    if !no_fix_published {
                        self.publish_unavailable(&mut last_good, &reason);
                        no_fix_published = true;
                    }
                    tokio::select! {
                        () = shutdown.wait() => break,
                        () = tokio::time::sleep(FIX_RETRY) => {}
                    }
                    continue;
                }
            };
            let Some(result) = self
                .fetch_async(probe.clone(), context, &mut shutdown)
                .await
            else {
                break;
            };
            let success = self.apply_result(result, &mut last_good);
            let delay = if success { self.poll } else { retry };
            retry = if success {
                FIX_RETRY
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
    use std::collections::HashMap;
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    use mackes_mesh_types::vehicle::{GpsFix, VehicleState};
    use mde_bus::persist::Persist;
    use serde_json::json;

    use super::*;

    const NOW_MS: i64 = 1_784_755_535_000;
    const GENERATED: &str = "2026-07-22T21:08:16+00:00";

    fn context() -> ForecastContext {
        ForecastContext {
            latitude: 42.3601,
            longitude: -71.0589,
            heading_deg: None,
            speed_mph: None,
        }
    }

    fn points_body(url: &str) -> String {
        json!({
            "properties": {
                "gridId": "BOX",
                "gridX": 71,
                "gridY": 101,
                "forecastHourly": url
            }
        })
        .to_string()
    }

    // Field shapes captured from BOX/71,101 on the official live endpoint on
    // 2026-07-22; fixtures vary only values and hostile cases.
    fn period(number: usize, start_offset_hours: i64) -> serde_json::Value {
        let start = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(
            NOW_MS + start_offset_hours * 3_600_000,
        )
        .expect("time");
        let end = start + chrono::Duration::hours(1);
        json!({
            "number": number,
            "startTime": start.to_rfc3339(),
            "endTime": end.to_rfc3339(),
            "isDaytime": true,
            "temperature": 83,
            "temperatureUnit": "F",
            "probabilityOfPrecipitation": {"unitCode":"wmoUnit:percent","value":27},
            "relativeHumidity": {"unitCode":"wmoUnit:percent","value":65},
            "windSpeed": "9 mph",
            "windDirection": "W",
            "shortForecast": "Chance Showers And Thunderstorms"
        })
    }

    fn hourly_body(periods: Vec<serde_json::Value>) -> String {
        json!({
            "properties": {
                "generatedAt": GENERATED,
                "periods": periods
            }
        })
        .to_string()
    }

    struct FakeProbe {
        points: String,
        hourly: String,
        calls: Mutex<HashMap<&'static str, usize>>,
    }

    impl FakeProbe {
        fn captured() -> Self {
            Self {
                points: points_body(
                    "https://api.weather.gov/gridpoints/BOX/71,101/forecast/hourly",
                ),
                hourly: hourly_body(vec![period(1, -1), period(2, 0), period(3, 1)]),
                calls: Mutex::new(HashMap::new()),
            }
        }
    }

    impl NwsForecastProbe for FakeProbe {
        fn fetch_points(&self, _point: ForecastPoint) -> io::Result<String> {
            *self
                .calls
                .lock()
                .expect("calls")
                .entry("points")
                .or_default() += 1;
            Ok(self.points.clone())
        }

        fn fetch_hourly(&self, _url: &str) -> io::Result<String> {
            *self
                .calls
                .lock()
                .expect("calls")
                .entry("hourly")
                .or_default() += 1;
            Ok(self.hourly.clone())
        }
    }

    #[test]
    fn captured_two_step_schema_filters_expired_period_and_classifies_storm() {
        let probe = FakeProbe::captured();
        let snapshot = build_snapshot(&probe, "rig-1", context(), NOW_MS).expect("snapshot");
        assert_eq!(snapshot.samples.len(), 1);
        assert_eq!(snapshot.samples[0].periods.len(), 2);
        assert_eq!(
            snapshot.samples[0].periods[0].kind,
            ForecastKind::Thunderstorm
        );
        assert_eq!(
            snapshot.samples[0].periods[0].precipitation_percent,
            Some(27)
        );
        assert_eq!(snapshot.feed_generated_at_ms, Some(1_784_754_496_000));
        let calls = probe.calls.lock().expect("calls");
        assert_eq!(calls.get("points"), Some(&1));
        assert_eq!(calls.get("hourly"), Some(&1));
    }

    #[test]
    fn forecast_hourly_url_rejects_hostile_substitution() {
        assert!(validate_hourly_url(
            "https://api.weather.gov/gridpoints/BOX/71,101/forecast/hourly"
        )
        .is_ok());
        for hostile in [
            "http://api.weather.gov/gridpoints/BOX/71,101/forecast/hourly",
            "https://api.weather.gov.evil.test/gridpoints/BOX/71,101/forecast/hourly",
            "https://api.weather.gov@evil.test/gridpoints/BOX/71,101/forecast/hourly",
            "https://api.weather.gov:444/gridpoints/BOX/71,101/forecast/hourly",
            "https://api.weather.gov/gridpoints/BOX/71,101/forecast/hourly?next=evil",
            "https://api.weather.gov/gridpoints/BOX/71,101/forecast/../evil",
        ] {
            assert!(validate_hourly_url(hostile).is_err(), "accepted {hostile}");
        }
        let mismatch = points_body("https://api.weather.gov/gridpoints/BOX/72,101/forecast/hourly");
        assert!(parse_points_document(&mismatch).is_err());
    }

    #[test]
    fn truncation_oversize_future_time_and_period_retention_fail_honestly() {
        let mut truncated = hourly_body(vec![period(1, 0)]);
        truncated.truncate(truncated.len() / 2);
        let endpoint = parse_points_document(&points_body(
            "https://api.weather.gov/gridpoints/BOX/71,101/forecast/hourly",
        ))
        .expect("endpoint");
        let point = drive_ahead_points(context(), NOW_MS)[0];
        assert!(
            parse_forecast_document(&truncated, &endpoint, point, NOW_MS, &mut Vec::new()).is_err()
        );
        assert!(read_bounded_string(
            &mut io::Cursor::new(vec![b'x'; MAX_FORECAST_BODY_BYTES + 1]),
            MAX_FORECAST_BODY_BYTES
        )
        .is_err());

        let future = json!({
            "properties": {
                "generatedAt": "2026-07-23T03:08:16+00:00",
                "periods": [period(1, 0)]
            }
        })
        .to_string();
        assert!(
            parse_forecast_document(&future, &endpoint, point, NOW_MS, &mut Vec::new()).is_err()
        );

        let mut overlong_period = period(1, 0);
        overlong_period["shortForecast"] = json!("x".repeat(MAX_SHORT_FORECAST_BYTES + 1));
        let mut overlong_gaps = Vec::new();
        assert!(parse_forecast_document(
            &hourly_body(vec![overlong_period]),
            &endpoint,
            point,
            NOW_MS,
            &mut overlong_gaps,
        )
        .is_err());
        assert!(overlong_gaps
            .iter()
            .any(|gap| gap.contains("short forecast is invalid")));

        let periods = (0..=MAX_FEED_PERIODS)
            .map(|index| period(index + 1, index as i64))
            .collect();
        let mut gaps = Vec::new();
        let (sample, _) =
            parse_forecast_document(&hourly_body(periods), &endpoint, point, NOW_MS, &mut gaps)
                .expect("bounded");
        assert_eq!(sample.periods.len(), MAX_RETAINED_PERIODS);
        assert!(gaps.iter().any(|gap| gap.contains("first 192")));
        assert!(gaps.iter().any(|gap| gap.contains("capped at 24")));
    }

    #[test]
    fn vehicle_context_rejects_nan_stale_future_and_wrong_host() {
        let mut vehicle = VehicleState::offline("rig-1");
        vehicle.online = true;
        vehicle.published_at_ms = 100_000;
        vehicle.gps = GpsFix {
            fix_type: "gps".to_string(),
            latitude: 42.3601,
            longitude: -71.0589,
            satellites: 8,
            speed_mph: 55.0,
            heading_deg: 90.0,
            age_s: 1.0,
            ..GpsFix::default()
        };
        let valid = validated_vehicle_context(&vehicle, "rig-1", 110_000).expect("valid");
        assert_eq!(valid.heading_deg, Some(90.0));
        assert!(validated_vehicle_context(&vehicle, "wrong", 110_000).is_none());
        vehicle.gps.latitude = f64::NAN;
        assert!(validated_vehicle_context(&vehicle, "rig-1", 100_000).is_none());
        vehicle.gps.latitude = 42.3601;
        vehicle.gps.age_s = 31.0;
        assert!(validated_vehicle_context(&vehicle, "rig-1", 100_000).is_none());
        vehicle.gps.age_s = 0.0;
        vehicle.published_at_ms = 106_000;
        assert!(validated_vehicle_context(&vehicle, "rig-1", 100_000).is_none());
    }

    #[test]
    fn heading_adds_bounded_forward_points_and_eta() {
        let mut moving = context();
        moving.heading_deg = Some(90.0);
        moving.speed_mph = Some(60.0);
        let points = drive_ahead_points(moving, NOW_MS);
        assert_eq!(points.len(), 3);
        assert_eq!(points[0].distance_ahead_km, 0.0);
        assert_eq!(points[2].distance_ahead_km, 50.0);
        assert!(points[2].longitude > context().longitude);
        assert!(points[2].eta_at_ms > NOW_MS);
    }

    #[test]
    fn http_client_refuses_redirects() {
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
        let client = build_http_client().expect("client");
        let error = fetch_bounded_json(
            &client,
            &format!("http://{redirect_addr}/points"),
            MAX_POINTS_BODY_BYTES,
        )
        .expect_err("redirect rejected");
        assert!(error.to_string().contains("redirects are disabled"));
        redirect_thread.join().expect("redirect join");
        target_thread.join().expect("target join");
        assert!(!contacted.load(Ordering::Relaxed));
    }

    #[test]
    fn failed_refresh_and_no_fix_keep_last_good_timestamp_on_bus() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().to_path_buf();
        let worker =
            NwsForecastOverlayWorker::new("rig-1".to_string()).with_bus_root(Some(root.clone()));
        let original =
            build_snapshot(&FakeProbe::captured(), "rig-1", context(), NOW_MS).expect("snapshot");
        let mut last = None;
        assert!(worker.apply_result(Ok(original), &mut last));
        assert!(!worker.apply_result(Err(io::Error::other("timeout")), &mut last));
        worker.publish_unavailable(&mut last, "fresh same-host MG90 fix unavailable");
        assert_eq!(last.as_ref().expect("last").fetched_at_ms, NOW_MS);
        assert!(last
            .as_ref()
            .expect("last")
            .gaps
            .iter()
            .any(|gap| gap.contains("fresh same-host")));
        let persist = Persist::open(root).expect("bus");
        let rows = persist
            .list_since(&nws_forecast_state_topic("rig-1"), None)
            .expect("read");
        assert_eq!(rows.len(), 3);
    }

    #[test]
    fn no_fix_before_first_fetch_publishes_explicit_unavailable_state() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().to_path_buf();
        let worker =
            NwsForecastOverlayWorker::new("rig-1".to_string()).with_bus_root(Some(root.clone()));
        let mut last = None;
        worker.publish_unavailable(&mut last, "fresh same-host MG90 fix unavailable");
        assert!(last.is_none());
        let persist = Persist::open(root).expect("bus");
        let body = persist
            .read_latest(&nws_forecast_state_topic("rig-1"))
            .expect("read")
            .expect("message")
            .body
            .expect("body");
        let snapshot: NwsForecastSnapshot = serde_json::from_str(&body).expect("snapshot");
        assert_eq!(snapshot.fetched_at_ms, 0);
        assert!(snapshot.samples.is_empty());
        assert!(snapshot
            .gaps
            .iter()
            .any(|gap| gap.contains("fresh same-host")));
    }

    struct SlowProbe;

    impl NwsForecastProbe for SlowProbe {
        fn fetch_points(&self, _point: ForecastPoint) -> io::Result<String> {
            std::thread::sleep(Duration::from_millis(500));
            Ok(FakeProbe::captured().points)
        }

        fn fetch_hourly(&self, _url: &str) -> io::Result<String> {
            Ok(FakeProbe::captured().hourly)
        }
    }

    #[tokio::test]
    async fn shutdown_wins_while_blocking_http_is_in_flight() {
        let worker = NwsForecastOverlayWorker::new("rig-1".to_string()).with_bus_root(None);
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
        .expect("runtime remains responsive");
        assert!(result.is_none());
        sender.await.expect("sender");
    }
}
