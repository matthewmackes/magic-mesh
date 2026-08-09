//! WL-FUNC-017 S3 — daemon-owned current conditions and local forecast.
//!
//! The worker consumes the S2 effective-location authority and publishes the
//! general weather projections.  It deliberately does not alter the vehicle
//! drive-ahead overlay: that producer retains its existing topic and motion
//! semantics.  Every network result is rebound to the exact effective-location
//! generation immediately before publication.

#![cfg(feature = "async-services")]

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, FixedOffset};
use mackes_mesh_types::location::{
    weather_location_state_topic, EffectiveLocationSnapshot, EffectiveLocationState,
    EffectiveWeatherLocation, WeatherCoverage,
};
use mackes_mesh_types::nws_alert::GeoPoint;
use mackes_mesh_types::weather::{
    weather_current_state_topic, weather_forecast_state_topic, CurrentConditions,
    CurrentWeatherSnapshot, Distance, DistanceUnit, HourlyForecastPeriod, LocalDaySummary,
    Pressure, PressureUnit, Speed, SpeedUnit, Temperature, TemperatureUnit, WeatherAttribution,
    WeatherAvailability, WeatherConditionKind, WeatherForecastSnapshot, WeatherProvider,
    WeatherUnavailableReason, MAX_WEATHER_DAILY_SUMMARIES, MAX_WEATHER_HOURLY_PERIODS,
    WEATHER_CONTRACT_SCHEMA_VERSION,
};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use serde::{Deserialize, Serialize};

use super::{ShutdownToken, Worker};

const CURRENT_POLL: Duration = Duration::from_secs(5 * 60);
const FORECAST_POLL: Duration = Duration::from_secs(10 * 60);
const AUTHORITY_POLL: Duration = Duration::from_secs(2);
const RETRY_INITIAL: Duration = Duration::from_secs(30);
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_POINTS_BODY_BYTES: usize = 128 * 1024;
const MAX_STATIONS_BODY_BYTES: usize = 128 * 1024;
const MAX_OBSERVATION_BODY_BYTES: usize = 128 * 1024;
const MAX_FORECAST_BODY_BYTES: usize = 512 * 1024;
const MAX_PROVIDER_PERIODS: usize = 192;
const MAX_PROVIDER_TEXT_BYTES: usize = 256;
const MAX_STATION_ID_BYTES: usize = 16;
const MAX_PROVIDER_FUTURE_SKEW_MS: i64 = 5 * 60 * 1_000;
const MAX_PROVIDER_AGE_MS: i64 = 6 * 60 * 60 * 1_000;
const MAX_CURRENT_FRESH_AGE_MS: i64 = 90 * 60 * 1_000;
const MAX_CACHE_AGE_MS: i64 = 6 * 60 * 60 * 1_000;
const CACHE_SCHEMA_VERSION: u16 = 1;
const MAX_CACHE_BYTES: usize = 2 * 1024 * 1024;
const DEFAULT_CACHE_PATH: &str = "/var/lib/mackesd/weather-forecast-cache.json";
const CACHE_PATH_ENV: &str = "MDE_WEATHER_FORECAST_CACHE_PATH";
static CACHE_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const USER_AGENT: &str =
    "Construct/12 mackesd weather-forecast (+https://github.com/matthewmackes/magic-mesh)";

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

/// Injectable NWS transport. Production validates every returned URL before a
/// request; fixtures can exercise parsing and generation races without a live
/// network dependency.
trait WeatherForecastProbe: Send + Sync {
    fn fetch_points(&self, point: &GeoPoint) -> io::Result<String>;
    fn fetch_official(&self, url: &str, max_bytes: usize) -> io::Result<String>;
}

struct NwsHttpProbe {
    client: Client,
}

impl NwsHttpProbe {
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

impl WeatherForecastProbe for NwsHttpProbe {
    fn fetch_points(&self, point: &GeoPoint) -> io::Result<String> {
        validate_point(point)?;
        let url = format!(
            "https://api.weather.gov/points/{:.4},{:.4}",
            point.latitude, point.longitude
        );
        fetch_bounded_json(&self.client, &url, MAX_POINTS_BODY_BYTES)
    }

    fn fetch_official(&self, url: &str, max_bytes: usize) -> io::Result<String> {
        validate_official_url(url)?;
        fetch_bounded_json(&self.client, url, max_bytes)
    }
}

fn fetch_bounded_json(client: &Client, url: &str, max_bytes: usize) -> io::Result<String> {
    let response = client
        .get(url)
        .header(ACCEPT, "application/geo+json")
        .send()
        .map_err(io_other)?;
    if response.status() != reqwest::StatusCode::OK {
        return Err(io::Error::other(format!(
            "NWS returned HTTP {} (redirects are disabled)",
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
        return Err(io::Error::other("NWS returned a non-JSON content type"));
    }
    if response
        .content_length()
        .is_some_and(|length| length > u64::try_from(max_bytes).unwrap_or(u64::MAX))
    {
        return Err(io::Error::other("NWS response exceeds its byte limit"));
    }
    let mut response = response;
    read_bounded(&mut response, max_bytes)
}

fn read_bounded(reader: &mut impl Read, max_bytes: usize) -> io::Result<String> {
    let mut body = Vec::with_capacity(max_bytes.min(96 * 1024));
    reader
        .take(
            u64::try_from(max_bytes)
                .unwrap_or(u64::MAX)
                .saturating_add(1),
        )
        .read_to_end(&mut body)?;
    if body.len() > max_bytes {
        return Err(io::Error::other("NWS response exceeds its byte limit"));
    }
    String::from_utf8(body).map_err(io_other)
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
    observation_stations: String,
    time_zone: String,
}

#[derive(Debug)]
struct Endpoints {
    source_id: String,
    hourly: String,
    stations: String,
    time_zone: String,
}

fn parse_points(body: &str) -> io::Result<Endpoints> {
    enforce_body_bound(body, MAX_POINTS_BODY_BYTES, "points")?;
    let document: PointsDocument = serde_json::from_str(body).map_err(io_other)?;
    let properties = document.properties;
    let grid_id = properties.grid_id.trim();
    if grid_id.len() != 3 || !grid_id.bytes().all(|byte| byte.is_ascii_uppercase()) {
        return Err(io::Error::other("NWS gridId is invalid"));
    }
    if !(0..=10_000).contains(&properties.grid_x) || !(0..=10_000).contains(&properties.grid_y) {
        return Err(io::Error::other("NWS grid coordinate is invalid"));
    }
    validate_grid_url(
        &properties.forecast_hourly,
        grid_id,
        properties.grid_x,
        properties.grid_y,
        "forecast/hourly",
    )?;
    validate_grid_url(
        &properties.observation_stations,
        grid_id,
        properties.grid_x,
        properties.grid_y,
        "stations",
    )?;
    validate_time_zone(&properties.time_zone)?;
    Ok(Endpoints {
        source_id: format!("NWS:{grid_id}:{}:{}", properties.grid_x, properties.grid_y),
        hourly: properties.forecast_hourly,
        stations: properties.observation_stations,
        time_zone: properties.time_zone,
    })
}

fn validate_grid_url(
    value: &str,
    grid_id: &str,
    grid_x: i32,
    grid_y: i32,
    suffix: &str,
) -> io::Result<()> {
    let url = validate_official_url(value)?;
    let expected = format!("/gridpoints/{grid_id}/{grid_x},{grid_y}/{suffix}");
    if url.path() != expected {
        return Err(io::Error::other(
            "NWS endpoint does not match its declared grid",
        ));
    }
    Ok(())
}

fn validate_official_url(value: &str) -> io::Result<reqwest::Url> {
    let url = reqwest::Url::parse(value).map_err(io_other)?;
    if url.scheme() != "https"
        || url.host_str() != Some("api.weather.gov")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path_segments().is_none()
        || url.path().contains("..")
        || url.path().contains("//")
    {
        return Err(io::Error::other(
            "URL is outside the strict official NWS HTTPS allowlist",
        ));
    }
    Ok(url)
}

#[derive(Debug, Deserialize)]
struct StationCollection {
    #[serde(default)]
    features: Vec<StationFeature>,
}

#[derive(Debug, Deserialize)]
struct StationFeature {
    id: String,
}

fn parse_station(body: &str) -> io::Result<(String, String)> {
    enforce_body_bound(body, MAX_STATIONS_BODY_BYTES, "stations")?;
    let document: StationCollection = serde_json::from_str(body).map_err(io_other)?;
    let station_url = document
        .features
        .first()
        .ok_or_else(|| io::Error::other("NWS returned no observation station"))?
        .id
        .as_str();
    let url = validate_official_url(station_url)?;
    let segments: Vec<_> = url
        .path_segments()
        .ok_or_else(|| io::Error::other("station URL has no path"))?
        .collect();
    if segments.len() != 2 || segments[0] != "stations" || !valid_station_id(segments[1]) {
        return Err(io::Error::other("NWS station URL is not canonical"));
    }
    let station_id = segments[1].to_string();
    Ok((
        station_id.clone(),
        format!("https://api.weather.gov/stations/{station_id}/observations/latest"),
    ))
}

fn valid_station_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_STATION_ID_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'-')
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Quantity {
    #[serde(default)]
    unit_code: Option<String>,
    #[serde(default)]
    value: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct ObservationDocument {
    properties: ObservationProperties,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ObservationProperties {
    timestamp: String,
    #[serde(default)]
    text_description: String,
    #[serde(default)]
    temperature: Quantity,
    #[serde(default)]
    heat_index: Quantity,
    #[serde(default)]
    wind_chill: Quantity,
    #[serde(default)]
    relative_humidity: Quantity,
    #[serde(default)]
    wind_speed: Quantity,
    #[serde(default)]
    wind_direction: Quantity,
    #[serde(default)]
    wind_gust: Quantity,
    #[serde(default)]
    visibility: Quantity,
    #[serde(default)]
    barometric_pressure: Quantity,
}

fn parse_current(
    body: &str,
    host: &str,
    generation: u64,
    point: &GeoPoint,
    source_id: &str,
    station_id: &str,
    fetched_at_ms: i64,
) -> io::Result<CurrentWeatherSnapshot> {
    enforce_body_bound(body, MAX_OBSERVATION_BODY_BYTES, "observation")?;
    let document: ObservationDocument = serde_json::from_str(body).map_err(io_other)?;
    let properties = document.properties;
    let observed_at_ms = parse_time_ms(&properties.timestamp)?.0;
    validate_provider_timestamp(observed_at_ms, fetched_at_ms)?;
    let provider_text = bounded_text(&properties.text_description);
    let conditions = CurrentConditions {
        observed_at_ms,
        condition: classify_condition(provider_text.as_deref().unwrap_or(""), true),
        provider_text,
        temperature: quantity_temperature(&properties.temperature),
        apparent_temperature: quantity_temperature(&properties.heat_index)
            .or_else(|| quantity_temperature(&properties.wind_chill)),
        relative_humidity_percent: quantity_percent(&properties.relative_humidity),
        precipitation_probability_percent: None,
        wind_speed: quantity_speed(&properties.wind_speed),
        wind_direction_degrees: quantity_direction(&properties.wind_direction),
        wind_gust: quantity_speed(&properties.wind_gust),
        visibility: quantity_distance(&properties.visibility),
        pressure: quantity_pressure(&properties.barometric_pressure),
    };
    let snapshot = CurrentWeatherSnapshot {
        schema_version: WEATHER_CONTRACT_SCHEMA_VERSION,
        host: host.to_string(),
        location_generation: generation,
        location_point: Some(point.clone()),
        producer_at_ms: observed_at_ms,
        fetched_at_ms,
        availability: WeatherAvailability::Fresh,
        conditions: Some(conditions),
        gaps: Vec::new(),
        attributions: vec![attribution(&format!("{source_id}:{station_id}"))],
    };
    snapshot.validate_at(fetched_at_ms).map_err(io_other)?;
    Ok(snapshot)
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
    periods: Vec<ForecastPeriodDocument>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ForecastPeriodDocument {
    number: u32,
    start_time: String,
    end_time: String,
    is_daytime: bool,
    temperature: f64,
    temperature_unit: String,
    #[serde(default)]
    probability_of_precipitation: Quantity,
    #[serde(default)]
    relative_humidity: Quantity,
    #[serde(default)]
    wind_speed: String,
    #[serde(default)]
    wind_direction: String,
    short_forecast: String,
}

fn parse_forecast(
    body: &str,
    host: &str,
    generation: u64,
    point: &GeoPoint,
    expected_time_zone: &str,
    endpoints: &Endpoints,
    fetched_at_ms: i64,
) -> io::Result<WeatherForecastSnapshot> {
    enforce_body_bound(body, MAX_FORECAST_BODY_BYTES, "hourly forecast")?;
    if endpoints.time_zone != expected_time_zone {
        return Err(io::Error::other(
            "NWS timezone does not match the effective-location authority",
        ));
    }
    let document: ForecastDocument = serde_json::from_str(body).map_err(io_other)?;
    let generated_at_ms = parse_time_ms(&document.properties.generated_at)?.0;
    validate_provider_timestamp(generated_at_ms, fetched_at_ms)?;
    let mut gaps = Vec::new();
    if document.properties.periods.len() > MAX_PROVIDER_PERIODS {
        gaps.push(format!(
            "provider periods capped at {MAX_PROVIDER_PERIODS} before normalization"
        ));
    }
    let mut hourly = Vec::new();
    for period in document
        .properties
        .periods
        .into_iter()
        .take(MAX_PROVIDER_PERIODS)
    {
        match normalize_hour(period, generated_at_ms) {
            Ok(Some(period)) if hourly.len() < MAX_WEATHER_HOURLY_PERIODS => hourly.push(period),
            Ok(Some(_)) => {
                if !gaps.iter().any(|gap| gap.contains("120-hour")) {
                    gaps.push("hourly forecast capped at the 120-hour contract horizon".into());
                }
            }
            Ok(None) => {}
            Err(error) => {
                if gaps.len() < 16 {
                    gaps.push(format!("provider period omitted: {error}"));
                }
            }
        }
    }
    hourly.sort_by_key(|period| period.start_at_ms);
    hourly.dedup_by_key(|period| period.start_at_ms);
    for (index, period) in hourly.iter_mut().enumerate() {
        period.sequence = u16::try_from(index + 1).unwrap_or(u16::MAX);
    }
    let daily = aggregate_days(&hourly);
    if hourly.is_empty() || daily.is_empty() {
        return Err(io::Error::other(
            "NWS forecast has no usable future periods",
        ));
    }
    let snapshot = WeatherForecastSnapshot {
        schema_version: WEATHER_CONTRACT_SCHEMA_VERSION,
        host: host.to_string(),
        location_generation: generation,
        location_point: Some(point.clone()),
        time_zone: expected_time_zone.to_string(),
        producer_at_ms: generated_at_ms,
        fetched_at_ms,
        availability: WeatherAvailability::Fresh,
        hourly,
        daily,
        alert_references: Vec::new(),
        gaps,
        attributions: vec![attribution(&endpoints.source_id)],
    };
    snapshot.validate_at(fetched_at_ms).map_err(io_other)?;
    Ok(snapshot)
}

fn normalize_hour(
    period: ForecastPeriodDocument,
    producer_at_ms: i64,
) -> io::Result<Option<HourlyForecastPeriod>> {
    let (start_at_ms, start_offset) = parse_time_ms(&period.start_time)?;
    let (end_at_ms, _) = parse_time_ms(&period.end_time)?;
    if end_at_ms <= producer_at_ms {
        return Ok(None);
    }
    if end_at_ms <= start_at_ms || end_at_ms.saturating_sub(start_at_ms) > 2 * 60 * 60 * 1_000 {
        return Err(io::Error::other("invalid hourly interval"));
    }
    let local_date = DateTime::from_timestamp_millis(start_at_ms)
        .ok_or_else(|| io::Error::other("hour timestamp is out of range"))?
        .with_timezone(&start_offset)
        .format("%Y-%m-%d")
        .to_string();
    let provider_text = bounded_text(&period.short_forecast)
        .ok_or_else(|| io::Error::other("short forecast is empty or oversized"))?;
    let temperature = temperature(period.temperature, &period.temperature_unit);
    let wind_speed = parse_wind_speed(&period.wind_speed);
    let wind_direction_degrees = parse_compass(&period.wind_direction);
    Ok(Some(HourlyForecastPeriod {
        sequence: u16::try_from(period.number).unwrap_or(u16::MAX),
        start_at_ms,
        end_at_ms,
        local_date,
        is_daytime: period.is_daytime,
        condition: classify_condition(&provider_text, period.is_daytime),
        provider_text: Some(provider_text),
        temperature,
        precipitation_probability_percent: quantity_percent(&period.probability_of_precipitation),
        relative_humidity_percent: quantity_percent(&period.relative_humidity),
        wind_speed,
        wind_direction_degrees,
    }))
}

fn aggregate_days(hourly: &[HourlyForecastPeriod]) -> Vec<LocalDaySummary> {
    let mut days: BTreeMap<&str, Vec<&HourlyForecastPeriod>> = BTreeMap::new();
    for period in hourly {
        if days.len() >= MAX_WEATHER_DAILY_SUMMARIES
            && !days.contains_key(period.local_date.as_str())
        {
            continue;
        }
        days.entry(&period.local_date).or_default().push(period);
    }
    days.into_iter()
        .take(MAX_WEATHER_DAILY_SUMMARIES)
        .map(|(date, periods)| {
            let representative = periods
                .iter()
                .max_by_key(|period| condition_rank(period.condition))
                .copied()
                .expect("day has at least one period");
            let unit = periods
                .iter()
                .find_map(|period| period.temperature.map(|value| value.unit));
            let temperatures: Vec<_> = periods
                .iter()
                .filter_map(|period| period.temperature)
                .filter(|value| Some(value.unit) == unit)
                .collect();
            let high_temperature = temperatures
                .iter()
                .copied()
                .max_by(|left, right| left.value.total_cmp(&right.value));
            let low_temperature = temperatures
                .iter()
                .copied()
                .min_by(|left, right| left.value.total_cmp(&right.value));
            let precipitation_probability_percent = periods
                .iter()
                .filter_map(|period| period.precipitation_probability_percent)
                .max_by(f32::total_cmp);
            let peak_wind_speed = periods
                .iter()
                .filter_map(|period| period.wind_speed)
                .max_by(|left, right| left.value.total_cmp(&right.value));
            LocalDaySummary {
                local_date: date.to_string(),
                condition: representative.condition,
                provider_text: representative.provider_text.clone(),
                high_temperature,
                low_temperature,
                precipitation_probability_percent,
                peak_wind_speed,
                source_period_count: u16::try_from(periods.len()).unwrap_or(u16::MAX),
            }
        })
        .collect()
}

fn condition_rank(kind: WeatherConditionKind) -> u8 {
    match kind {
        WeatherConditionKind::Storm => 8,
        WeatherConditionKind::Wintry => 7,
        WeatherConditionKind::Rain => 6,
        WeatherConditionKind::Fog => 5,
        WeatherConditionKind::Wind => 4,
        WeatherConditionKind::Clouds => 3,
        WeatherConditionKind::ClearNight | WeatherConditionKind::ClearDay => 2,
        WeatherConditionKind::Unavailable => 0,
    }
}

fn classify_condition(value: &str, daytime: bool) -> WeatherConditionKind {
    let value = value.to_ascii_lowercase();
    if value.contains("thunder") {
        WeatherConditionKind::Storm
    } else if ["snow", "sleet", "freezing", "wintry", "ice"]
        .iter()
        .any(|needle| value.contains(needle))
    {
        WeatherConditionKind::Wintry
    } else if ["rain", "shower", "drizzle"]
        .iter()
        .any(|needle| value.contains(needle))
    {
        WeatherConditionKind::Rain
    } else if ["fog", "smoke", "haze"]
        .iter()
        .any(|needle| value.contains(needle))
    {
        WeatherConditionKind::Fog
    } else if ["wind", "breezy", "gust"]
        .iter()
        .any(|needle| value.contains(needle))
    {
        WeatherConditionKind::Wind
    } else if value.contains("cloud") || value.contains("overcast") {
        WeatherConditionKind::Clouds
    } else if daytime {
        WeatherConditionKind::ClearDay
    } else {
        WeatherConditionKind::ClearNight
    }
}

fn bounded_text(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()
        && value.len() <= MAX_PROVIDER_TEXT_BYTES
        && !value.chars().any(char::is_control))
    .then(|| value.to_string())
}

fn quantity_temperature(value: &Quantity) -> Option<Temperature> {
    let unit = match value.unit_code.as_deref()? {
        "wmoUnit:degC" => TemperatureUnit::Celsius,
        "wmoUnit:degF" => TemperatureUnit::Fahrenheit,
        _ => return None,
    };
    temperature(
        value.value?,
        match unit {
            TemperatureUnit::Celsius => "C",
            TemperatureUnit::Fahrenheit => "F",
        },
    )
}

fn temperature(value: f64, unit: &str) -> Option<Temperature> {
    let unit = match unit {
        "C" => TemperatureUnit::Celsius,
        "F" => TemperatureUnit::Fahrenheit,
        _ => return None,
    };
    let value = value as f32;
    (value.is_finite()
        && match unit {
            TemperatureUnit::Celsius => (-100.0..=60.0).contains(&value),
            TemperatureUnit::Fahrenheit => (-148.0..=140.0).contains(&value),
        })
    .then_some(Temperature { value, unit })
}

fn quantity_percent(value: &Quantity) -> Option<f32> {
    if value.unit_code.as_deref() != Some("wmoUnit:percent") {
        return None;
    }
    let value = value.value? as f32;
    (value.is_finite() && (0.0..=100.0).contains(&value)).then_some(value)
}

fn quantity_speed(value: &Quantity) -> Option<Speed> {
    let unit = match value.unit_code.as_deref()? {
        "wmoUnit:m_s-1" => SpeedUnit::MetresPerSecond,
        "wmoUnit:km_h-1" => SpeedUnit::KilometresPerHour,
        _ => return None,
    };
    let value = value.value? as f32;
    (value.is_finite() && value >= 0.0).then_some(Speed { value, unit })
}

fn quantity_direction(value: &Quantity) -> Option<f32> {
    let value = value.value? as f32;
    (value.is_finite() && (0.0..360.0).contains(&value)).then_some(value)
}

fn quantity_distance(value: &Quantity) -> Option<Distance> {
    (value.unit_code.as_deref()? == "wmoUnit:m").then_some(())?;
    let value = value.value? as f32;
    (value.is_finite() && (0.0..=500_000.0).contains(&value)).then_some(Distance {
        value,
        unit: DistanceUnit::Metres,
    })
}

fn quantity_pressure(value: &Quantity) -> Option<Pressure> {
    (value.unit_code.as_deref()? == "wmoUnit:Pa").then_some(())?;
    let value = value.value? as f32;
    (value.is_finite() && (80_000.0..=110_000.0).contains(&value)).then_some(Pressure {
        value,
        unit: PressureUnit::Pascals,
    })
}

fn parse_wind_speed(value: &str) -> Option<Speed> {
    let lower = value.trim().to_ascii_lowercase();
    let unit = if lower.ends_with(" mph") {
        SpeedUnit::MilesPerHour
    } else if lower.ends_with(" km/h") {
        SpeedUnit::KilometresPerHour
    } else {
        return None;
    };
    let maximum = lower
        .split(|character: char| !character.is_ascii_digit() && character != '.')
        .filter_map(|token| token.parse::<f32>().ok())
        .filter(|value| value.is_finite() && *value >= 0.0)
        .max_by(f32::total_cmp)?;
    Some(Speed {
        value: maximum,
        unit,
    })
}

fn parse_compass(value: &str) -> Option<f32> {
    Some(match value.trim().to_ascii_uppercase().as_str() {
        "N" => 0.0,
        "NNE" => 22.5,
        "NE" => 45.0,
        "ENE" => 67.5,
        "E" => 90.0,
        "ESE" => 112.5,
        "SE" => 135.0,
        "SSE" => 157.5,
        "S" => 180.0,
        "SSW" => 202.5,
        "SW" => 225.0,
        "WSW" => 247.5,
        "W" => 270.0,
        "WNW" => 292.5,
        "NW" => 315.0,
        "NNW" => 337.5,
        _ => return None,
    })
}

fn parse_time_ms(value: &str) -> io::Result<(i64, FixedOffset)> {
    let timestamp = DateTime::parse_from_rfc3339(value).map_err(io_other)?;
    Ok((timestamp.timestamp_millis(), *timestamp.offset()))
}

fn validate_provider_timestamp(provider_at_ms: i64, fetched_at_ms: i64) -> io::Result<()> {
    if provider_at_ms <= 0
        || provider_at_ms.saturating_sub(fetched_at_ms) > MAX_PROVIDER_FUTURE_SKEW_MS
        || fetched_at_ms.saturating_sub(provider_at_ms) > MAX_PROVIDER_AGE_MS
    {
        return Err(io::Error::other(
            "NWS provider timestamp is stale or implausible",
        ));
    }
    Ok(())
}

fn validate_point(point: &GeoPoint) -> io::Result<()> {
    if point.latitude.is_finite()
        && point.longitude.is_finite()
        && (-90.0..=90.0).contains(&point.latitude)
        && (-180.0..=180.0).contains(&point.longitude)
    {
        Ok(())
    } else {
        Err(io::Error::other("effective weather point is invalid"))
    }
}

fn validate_time_zone(value: &str) -> io::Result<()> {
    if value.len() <= 64
        && value.contains('/')
        && !value.starts_with('/')
        && !value.ends_with('/')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'_' | b'-' | b'+'))
    {
        Ok(())
    } else {
        Err(io::Error::other("NWS timezone is invalid"))
    }
}

fn enforce_body_bound(body: &str, max_bytes: usize, kind: &str) -> io::Result<()> {
    if body.len() > max_bytes {
        Err(io::Error::other(format!(
            "NWS {kind} body exceeds its byte limit"
        )))
    } else {
        Ok(())
    }
}

fn attribution(source_id: &str) -> WeatherAttribution {
    WeatherAttribution {
        provider: WeatherProvider::NationalWeatherService,
        source_id: source_id.to_string(),
        label: "NOAA National Weather Service".to_string(),
    }
}

fn effective_location(snapshot: &EffectiveLocationSnapshot) -> Option<&EffectiveWeatherLocation> {
    match &snapshot.state {
        EffectiveLocationState::Available { location }
        | EffectiveLocationState::Stale { location, .. } => Some(location),
        EffectiveLocationState::Unavailable { .. } => None,
    }
}

fn read_effective_location(
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

fn same_effective_location(
    left: &EffectiveLocationSnapshot,
    right: &EffectiveLocationSnapshot,
) -> bool {
    left.host == right.host
        && left.generation == right.generation
        && effective_location(left)
            .zip(effective_location(right))
            .is_some_and(|(left, right)| {
                left.point == right.point
                    && left.time_zone == right.time_zone
                    && left.coverage == right.coverage
            })
}

fn unavailable_current(
    host: &str,
    generation: u64,
    point: Option<GeoPoint>,
    now_ms: i64,
    reason: WeatherUnavailableReason,
    gap: String,
) -> CurrentWeatherSnapshot {
    CurrentWeatherSnapshot {
        schema_version: WEATHER_CONTRACT_SCHEMA_VERSION,
        host: host.to_string(),
        location_generation: generation,
        location_point: point,
        producer_at_ms: now_ms,
        fetched_at_ms: now_ms,
        availability: WeatherAvailability::Unavailable { reason },
        conditions: None,
        gaps: vec![gap],
        attributions: vec![attribution("nws")],
    }
}

fn unavailable_forecast(
    host: &str,
    generation: u64,
    point: Option<GeoPoint>,
    time_zone: &str,
    now_ms: i64,
    reason: WeatherUnavailableReason,
    gap: String,
) -> WeatherForecastSnapshot {
    WeatherForecastSnapshot {
        schema_version: WEATHER_CONTRACT_SCHEMA_VERSION,
        host: host.to_string(),
        location_generation: generation,
        location_point: point,
        time_zone: time_zone.to_string(),
        producer_at_ms: now_ms,
        fetched_at_ms: now_ms,
        availability: WeatherAvailability::Unavailable { reason },
        hourly: Vec::new(),
        daily: Vec::new(),
        alert_references: Vec::new(),
        gaps: vec![gap],
        attributions: vec![attribution("nws")],
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WeatherCache {
    schema_version: u16,
    host: String,
    generation: u64,
    point: GeoPoint,
    time_zone: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    current: Option<CurrentWeatherSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    forecast: Option<WeatherForecastSnapshot>,
}

impl WeatherCache {
    fn empty(host: &str, generation: u64, location: &EffectiveWeatherLocation) -> Self {
        Self {
            schema_version: CACHE_SCHEMA_VERSION,
            host: host.to_string(),
            generation,
            point: location.point.clone(),
            time_zone: location.time_zone.clone(),
            current: None,
            forecast: None,
        }
    }

    fn matches(&self, host: &str, generation: u64, location: &EffectiveWeatherLocation) -> bool {
        self.schema_version == CACHE_SCHEMA_VERSION
            && self.host == host
            && self.generation == generation
            && self.point == location.point
            && self.time_zone == location.time_zone
    }
}

fn cache_path() -> PathBuf {
    std::env::var_os(CACHE_PATH_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_CACHE_PATH))
}

fn load_cache(path: &Path) -> io::Result<Option<WeatherCache>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(io::Error::other(
            "weather cache must be a regular non-symlink file",
        ));
    }
    if metadata.len() > u64::try_from(MAX_CACHE_BYTES).unwrap_or(u64::MAX) {
        return Err(io::Error::other("weather cache exceeds its byte limit"));
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
        return Err(io::Error::other("weather cache changed during secure open"));
    }
    let mut body = Vec::with_capacity(metadata.len() as usize);
    file.take(u64::try_from(MAX_CACHE_BYTES).unwrap_or(u64::MAX) + 1)
        .read_to_end(&mut body)?;
    if body.len() > MAX_CACHE_BYTES {
        return Err(io::Error::other("weather cache exceeds its byte limit"));
    }
    let text = std::str::from_utf8(&body).map_err(io_other)?;
    mackes_mesh_types::workloads::reject_duplicate_json_keys(text).map_err(io_other)?;
    let cache: WeatherCache = serde_json::from_slice(&body).map_err(io_other)?;
    if cache.schema_version != CACHE_SCHEMA_VERSION {
        return Err(io::Error::other("unsupported weather cache schema"));
    }
    Ok(Some(cache))
}

fn store_cache(path: &Path, cache: &WeatherCache) -> io::Result<()> {
    let body = serde_json::to_vec(cache).map_err(io_other)?;
    if body.len() > MAX_CACHE_BYTES {
        return Err(io::Error::other("weather cache exceeds its byte limit"));
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| io::Error::other("weather cache path has no parent"))?;
    fs::create_dir_all(parent)?;
    let parent_metadata = fs::symlink_metadata(parent)?;
    if parent_metadata.file_type().is_symlink() || !parent_metadata.is_dir() {
        return Err(io::Error::other(
            "weather cache parent is not a regular directory",
        ));
    }
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(io::Error::other("weather cache path must not be a symlink"));
    }
    let sequence = CACHE_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temp = parent.join(format!(
        ".weather-forecast-{}-{sequence}.tmp",
        std::process::id()
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)?;
        file.write_all(&body)?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn cached_current(
    cache: &WeatherCache,
    host: &str,
    generation: u64,
    location: &EffectiveWeatherLocation,
    now_ms: i64,
) -> Option<CurrentWeatherSnapshot> {
    if !cache.matches(host, generation, location) {
        return None;
    }
    let mut snapshot = cache.current.clone()?;
    if snapshot.host != host
        || snapshot.location_generation != generation
        || snapshot.location_point.as_ref() != Some(&location.point)
    {
        return None;
    }
    let observed_at_ms = snapshot.conditions.as_ref()?.observed_at_ms;
    let age = now_ms.saturating_sub(observed_at_ms).max(0);
    if age > MAX_CACHE_AGE_MS {
        return None;
    }
    snapshot.availability = if age <= MAX_CURRENT_FRESH_AGE_MS {
        WeatherAvailability::Fresh
    } else {
        WeatherAvailability::Stale {
            reason: mackes_mesh_types::weather::WeatherStaleReason::RefreshFailed,
        }
    };
    snapshot.validate_at(now_ms).ok()?;
    Some(snapshot)
}

fn cached_forecast(
    cache: &WeatherCache,
    host: &str,
    generation: u64,
    location: &EffectiveWeatherLocation,
    now_ms: i64,
) -> Option<WeatherForecastSnapshot> {
    if !cache.matches(host, generation, location) {
        return None;
    }
    let mut snapshot = cache.forecast.clone()?;
    if snapshot.host != host
        || snapshot.location_generation != generation
        || snapshot.location_point.as_ref() != Some(&location.point)
        || snapshot.time_zone != location.time_zone
    {
        return None;
    }
    let age = now_ms.saturating_sub(snapshot.producer_at_ms).max(0);
    if age > MAX_CACHE_AGE_MS {
        return None;
    }
    snapshot.availability = if age <= MAX_CURRENT_FRESH_AGE_MS {
        WeatherAvailability::Fresh
    } else {
        WeatherAvailability::Stale {
            reason: mackes_mesh_types::weather::WeatherStaleReason::RefreshFailed,
        }
    };
    snapshot.validate_at(now_ms).ok()?;
    Some(snapshot)
}

#[derive(Debug)]
struct ProviderResult {
    current: Option<Result<CurrentWeatherSnapshot, String>>,
    forecast: Option<Result<WeatherForecastSnapshot, String>>,
    cache: Option<WeatherCache>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct RefreshOutcome {
    current_fresh: Option<bool>,
    forecast_fresh: Option<bool>,
}

#[derive(Debug)]
struct RefreshSchedule {
    generation: Option<u64>,
    current_due_ms: i64,
    forecast_due_ms: i64,
    current_retry: Duration,
    forecast_retry: Duration,
}

impl RefreshSchedule {
    fn new(now_ms: i64) -> Self {
        Self {
            generation: None,
            current_due_ms: now_ms,
            forecast_due_ms: now_ms,
            current_retry: RETRY_INITIAL,
            forecast_retry: RETRY_INITIAL,
        }
    }

    fn due(&mut self, now_ms: i64, generation: u64) -> (bool, bool) {
        if self.generation != Some(generation) {
            self.generation = Some(generation);
            self.current_due_ms = now_ms;
            self.forecast_due_ms = now_ms;
        }
        (
            now_ms >= self.current_due_ms,
            now_ms >= self.forecast_due_ms,
        )
    }

    fn record(&mut self, now_ms: i64, outcome: RefreshOutcome) {
        if let Some(fresh) = outcome.current_fresh {
            if fresh {
                self.current_retry = RETRY_INITIAL;
                self.current_due_ms = now_ms.saturating_add(CURRENT_POLL.as_millis() as i64);
            } else {
                self.current_due_ms = now_ms.saturating_add(self.current_retry.as_millis() as i64);
                self.current_retry = self.current_retry.saturating_mul(2).min(CURRENT_POLL);
            }
        }
        if let Some(fresh) = outcome.forecast_fresh {
            if fresh {
                self.forecast_retry = RETRY_INITIAL;
                self.forecast_due_ms = now_ms.saturating_add(FORECAST_POLL.as_millis() as i64);
            } else {
                self.forecast_due_ms =
                    now_ms.saturating_add(self.forecast_retry.as_millis() as i64);
                self.forecast_retry = self.forecast_retry.saturating_mul(2).min(FORECAST_POLL);
            }
        }
    }
}

/// Workstation-side S3 producer for effective-location weather projections.
pub struct WeatherForecastWorker {
    host: String,
    probe: Option<Arc<dyn WeatherForecastProbe>>,
    clock: Arc<dyn Clock>,
    bus_root: Option<PathBuf>,
    cache_path: PathBuf,
}

impl WeatherForecastWorker {
    /// Construct the default-on official NWS producer for one local host.
    #[must_use]
    pub fn new(host: String) -> Self {
        let probe = NwsHttpProbe::new()
            .map(|probe| Arc::new(probe) as Arc<dyn WeatherForecastProbe>)
            .map_err(|error| {
                tracing::warn!(target: "mackesd::weather_forecast", %error, "NWS client unavailable");
                error
            })
            .ok();
        Self {
            host,
            probe,
            clock: Arc::new(SystemClock),
            bus_root: crate::bus_publish::default_bus_root(),
            cache_path: cache_path(),
        }
    }

    fn read_location(&self) -> io::Result<EffectiveLocationSnapshot> {
        let root = self
            .bus_root
            .as_ref()
            .ok_or_else(|| io::Error::other("Bus spool unavailable"))?;
        let persist = Persist::open(root.clone()).map_err(io_other)?;
        read_effective_location(&persist, &self.host, self.clock.now_ms())
    }

    async fn refresh_once(
        &self,
        location_snapshot: EffectiveLocationSnapshot,
        fetch_current: bool,
        fetch_forecast: bool,
    ) -> io::Result<RefreshOutcome> {
        let Some(location) = effective_location(&location_snapshot).cloned() else {
            return Ok(RefreshOutcome::default());
        };
        let probe = self.probe.clone();
        let host = self.host.clone();
        let cache_path = self.cache_path.clone();
        let now_ms = self.clock.now_ms();
        let generation = location_snapshot.generation;
        let provider = tokio::task::spawn_blocking(move || {
            blocking_provider_refresh(
                probe,
                &host,
                generation,
                &location,
                now_ms,
                fetch_current,
                fetch_forecast,
                &cache_path,
            )
        })
        .await
        .map_err(|error| io::Error::other(format!("weather provider task failed: {error}")))??;

        // Persist never crosses the spawn_blocking boundary. Reopen it on the
        // worker side, then enforce the S2 authority immediately before writes.
        let root = self
            .bus_root
            .as_ref()
            .ok_or_else(|| io::Error::other("Bus spool unavailable"))?;
        let persist = Persist::open(root.clone()).map_err(io_other)?;
        let latest = read_effective_location(&persist, &self.host, self.clock.now_ms())?;
        if !same_effective_location(&location_snapshot, &latest) {
            tracing::info!(
                target: "mackesd::weather_forecast",
                expected_generation = location_snapshot.generation,
                latest_generation = latest.generation,
                "discarding provider response after effective-location change"
            );
            return Ok(RefreshOutcome {
                current_fresh: fetch_current.then_some(false),
                forecast_fresh: fetch_forecast.then_some(false),
            });
        }
        let location = effective_location(&location_snapshot).expect("location retained");
        let mut cache = provider
            .cache
            .filter(|cache| cache.matches(&self.host, location_snapshot.generation, location))
            .unwrap_or_else(|| {
                WeatherCache::empty(&self.host, location_snapshot.generation, location)
            });
        let mut outcome = RefreshOutcome::default();
        if let Some(result) = provider.current {
            let (snapshot, fresh) = match result {
                Ok(snapshot) => {
                    cache.current = Some(snapshot.clone());
                    (snapshot, true)
                }
                Err(error) => cached_current(
                    &cache,
                    &self.host,
                    location_snapshot.generation,
                    location,
                    self.clock.now_ms(),
                )
                .map_or_else(
                    || {
                        (
                            unavailable_current(
                                &self.host,
                                location_snapshot.generation,
                                Some(location.point.clone()),
                                self.clock.now_ms(),
                                WeatherUnavailableReason::ObservationUnavailable,
                                format!("NWS current conditions unavailable: {error}"),
                            ),
                            false,
                        )
                    },
                    |snapshot| (snapshot, false),
                ),
            };
            snapshot
                .validate_at(self.clock.now_ms())
                .map_err(io_other)?;
            publish_json(
                &persist,
                &weather_current_state_topic(&self.host),
                &snapshot,
            )?;
            outcome.current_fresh = Some(fresh);
        }
        if let Some(result) = provider.forecast {
            let (snapshot, fresh) = match result {
                Ok(snapshot) => {
                    cache.forecast = Some(snapshot.clone());
                    (snapshot, true)
                }
                Err(error) => cached_forecast(
                    &cache,
                    &self.host,
                    location_snapshot.generation,
                    location,
                    self.clock.now_ms(),
                )
                .map_or_else(
                    || {
                        (
                            unavailable_forecast(
                                &self.host,
                                location_snapshot.generation,
                                Some(location.point.clone()),
                                &location.time_zone,
                                self.clock.now_ms(),
                                WeatherUnavailableReason::ForecastUnavailable,
                                format!("NWS forecast unavailable: {error}"),
                            ),
                            false,
                        )
                    },
                    |snapshot| (snapshot, false),
                ),
            };
            snapshot
                .validate_at(self.clock.now_ms())
                .map_err(io_other)?;
            publish_json(
                &persist,
                &weather_forecast_state_topic(&self.host),
                &snapshot,
            )?;
            outcome.forecast_fresh = Some(fresh);
        }
        if outcome.current_fresh == Some(true) || outcome.forecast_fresh == Some(true) {
            let cache_path = self.cache_path.clone();
            tokio::task::spawn_blocking(move || store_cache(&cache_path, &cache))
                .await
                .map_err(|error| {
                    io::Error::other(format!("weather cache task failed: {error}"))
                })??;
        }
        Ok(outcome)
    }
}

fn blocking_provider_refresh(
    probe: Option<Arc<dyn WeatherForecastProbe>>,
    host: &str,
    generation: u64,
    location: &EffectiveWeatherLocation,
    now_ms: i64,
    fetch_current: bool,
    fetch_forecast: bool,
    cache_path: &Path,
) -> io::Result<ProviderResult> {
    let cache = load_cache(cache_path)?;
    if location.coverage != WeatherCoverage::NwsUnitedStates {
        return Ok(ProviderResult {
            current: fetch_current
                .then(|| Err("effective location is outside NWS coverage".to_string())),
            forecast: fetch_forecast
                .then(|| Err("effective location is outside NWS coverage".to_string())),
            cache,
        });
    }
    let Some(probe) = probe else {
        return Ok(ProviderResult {
            current: fetch_current.then(|| Err("NWS probe unavailable".to_string())),
            forecast: fetch_forecast.then(|| Err("NWS probe unavailable".to_string())),
            cache,
        });
    };
    let endpoints = match probe
        .fetch_points(&location.point)
        .and_then(|body| parse_points(&body))
    {
        Ok(endpoints) => endpoints,
        Err(error) => {
            let error = error.to_string();
            return Ok(ProviderResult {
                current: fetch_current.then(|| Err(error.clone())),
                forecast: fetch_forecast.then(|| Err(error)),
                cache,
            });
        }
    };
    let current = fetch_current.then(|| {
        (|| {
            let stations = probe.fetch_official(&endpoints.stations, MAX_STATIONS_BODY_BYTES)?;
            let (station_id, observation_url) = parse_station(&stations)?;
            let observation = probe.fetch_official(&observation_url, MAX_OBSERVATION_BODY_BYTES)?;
            parse_current(
                &observation,
                host,
                generation,
                &location.point,
                &endpoints.source_id,
                &station_id,
                now_ms,
            )
        })()
        .map_err(|error| error.to_string())
    });
    let forecast = fetch_forecast.then(|| {
        (|| {
            let body = probe.fetch_official(&endpoints.hourly, MAX_FORECAST_BODY_BYTES)?;
            parse_forecast(
                &body,
                host,
                generation,
                &location.point,
                &location.time_zone,
                &endpoints,
                now_ms,
            )
        })()
        .map_err(|error| error.to_string())
    });
    Ok(ProviderResult {
        current,
        forecast,
        cache,
    })
}

fn publish_json<T: serde::Serialize>(persist: &Persist, topic: &str, value: &T) -> io::Result<()> {
    let body = serde_json::to_string(value).map_err(io_other)?;
    persist
        .write(topic, Priority::Default, None, Some(&body))
        .map_err(io_other)?;
    Ok(())
}

fn io_other(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

#[async_trait::async_trait]
impl Worker for WeatherForecastWorker {
    fn name(&self) -> &'static str {
        "weather_forecast"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let mut schedule = RefreshSchedule::new(self.clock.now_ms());
        loop {
            let location = match self.read_location() {
                Ok(location) => location,
                Err(error) => {
                    tracing::warn!(target: "mackesd::weather_forecast", %error, "effective location unavailable");
                    tokio::select! {
                        () = shutdown.wait() => break,
                        () = tokio::time::sleep(AUTHORITY_POLL) => continue,
                    }
                }
            };
            let now_ms = self.clock.now_ms();
            let (current_due, forecast_due) = schedule.due(now_ms, location.generation);
            if current_due || forecast_due {
                let refresh = self.refresh_once(location, current_due, forecast_due);
                let outcome = tokio::select! {
                    () = shutdown.wait() => break,
                    outcome = refresh => outcome,
                };
                match outcome {
                    Ok(outcome) => schedule.record(self.clock.now_ms(), outcome),
                    Err(error) => {
                        tracing::warn!(target: "mackesd::weather_forecast", %error, "weather refresh failed");
                        schedule.record(
                            self.clock.now_ms(),
                            RefreshOutcome {
                                current_fresh: current_due.then_some(false),
                                forecast_fresh: forecast_due.then_some(false),
                            },
                        );
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
    use mackes_mesh_types::location::{
        EffectiveLocationProvenance, WeatherLocationMode, WEATHER_LOCATION_SCHEMA_VERSION,
    };
    use std::sync::atomic::{AtomicI64, Ordering};
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
        points: String,
        stations: String,
        observation: String,
        forecast: String,
        requested: Mutex<Vec<String>>,
        threads: Mutex<Vec<std::thread::ThreadId>>,
        on_hourly: Option<Box<dyn Fn() + Send + Sync>>,
    }

    impl WeatherForecastProbe for FixtureProbe {
        fn fetch_points(&self, _point: &GeoPoint) -> io::Result<String> {
            self.threads
                .lock()
                .expect("threads")
                .push(std::thread::current().id());
            Ok(self.points.clone())
        }

        fn fetch_official(&self, url: &str, _max_bytes: usize) -> io::Result<String> {
            validate_official_url(url)?;
            self.requested
                .lock()
                .expect("requests")
                .push(url.to_string());
            self.threads
                .lock()
                .expect("threads")
                .push(std::thread::current().id());
            if url.ends_with("/stations") {
                Ok(self.stations.clone())
            } else if url.ends_with("/observations/latest") {
                Ok(self.observation.clone())
            } else if url.ends_with("/forecast/hourly") {
                if let Some(callback) = &self.on_hourly {
                    callback();
                }
                Ok(self.forecast.clone())
            } else {
                Err(io::Error::other("unexpected fixture URL"))
            }
        }
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

    fn points_json(time_zone: &str) -> String {
        format!(
            r#"{{"properties":{{"gridId":"BOX","gridX":71,"gridY":101,"forecastHourly":"https://api.weather.gov/gridpoints/BOX/71,101/forecast/hourly","observationStations":"https://api.weather.gov/gridpoints/BOX/71,101/stations","timeZone":"{time_zone}"}}}}"#
        )
    }

    fn observation_json() -> String {
        r#"{"properties":{"timestamp":"2027-01-15T02:35:00-05:00","textDescription":"Mostly Cloudy","temperature":{"unitCode":"wmoUnit:degC","value":null},"heatIndex":{"unitCode":"wmoUnit:degC","value":null},"windChill":{"unitCode":"wmoUnit:degC","value":null},"relativeHumidity":{"unitCode":"wmoUnit:percent","value":64.0},"windSpeed":{"unitCode":"wmoUnit:m_s-1","value":4.2},"windDirection":{"unitCode":"wmoUnit:degree_(angle)","value":270.0},"windGust":{"unitCode":"wmoUnit:m_s-1","value":null},"visibility":{"unitCode":"wmoUnit:m","value":16000.0},"barometricPressure":{"unitCode":"wmoUnit:Pa","value":101325.0}}}"#.into()
    }

    fn forecast_json(period_count: usize) -> String {
        let generated = DateTime::from_timestamp_millis(NOW - 60_000).expect("generated");
        let periods: Vec<_> = (0..period_count)
            .map(|index| {
                let start = generated + chrono::Duration::hours(i64::try_from(index + 1).unwrap());
                let end = start + chrono::Duration::hours(1);
                serde_json::json!({
                    "number": index + 1,
                    "startTime": start.to_rfc3339(),
                    "endTime": end.to_rfc3339(),
                    "isDaytime": index % 24 < 12,
                    "temperature": 40 + (index % 20),
                    "temperatureUnit": "F",
                    "probabilityOfPrecipitation": {"unitCode":"wmoUnit:percent","value": if index % 3 == 0 { Some(40.0) } else { None }},
                    "relativeHumidity": {"unitCode":"wmoUnit:percent","value": null},
                    "windSpeed": "5 to 12 mph",
                    "windDirection": "NW",
                    "shortForecast": if index % 17 == 0 { "Rain Showers" } else { "Mostly Cloudy" }
                })
            })
            .collect();
        serde_json::json!({
            "properties": {"generatedAt": generated.to_rfc3339(), "periods": periods}
        })
        .to_string()
    }

    fn fixture_probe(period_count: usize) -> Arc<FixtureProbe> {
        Arc::new(FixtureProbe {
            points: points_json("America/New_York"),
            stations: r#"{"features":[{"id":"https://api.weather.gov/stations/KBOS"}]}"#.into(),
            observation: observation_json(),
            forecast: forecast_json(period_count),
            requested: Mutex::new(Vec::new()),
            threads: Mutex::new(Vec::new()),
            on_hourly: None,
        })
    }

    fn write_location(root: &Path, snapshot: &EffectiveLocationSnapshot) {
        let persist = Persist::open(root.to_path_buf()).expect("open Bus");
        publish_json(
            &persist,
            &weather_location_state_topic("workstation-1"),
            snapshot,
        )
        .expect("publish location");
    }

    fn worker_at(
        temp: &TempDir,
        probe: Arc<dyn WeatherForecastProbe>,
        now_ms: i64,
    ) -> WeatherForecastWorker {
        WeatherForecastWorker {
            host: "workstation-1".into(),
            probe: Some(probe),
            clock: Arc::new(TestClock(AtomicI64::new(now_ms))),
            bus_root: Some(temp.path().join("bus")),
            cache_path: temp.path().join("weather-cache.json"),
        }
    }

    fn worker(temp: &TempDir, probe: Arc<dyn WeatherForecastProbe>) -> WeatherForecastWorker {
        worker_at(temp, probe, NOW)
    }

    fn latest<T: serde::de::DeserializeOwned>(root: &Path, topic: &str) -> T {
        let persist = Persist::open(root.to_path_buf()).expect("open Bus");
        let body = persist
            .read_latest(topic)
            .expect("read")
            .expect("message")
            .body
            .expect("body");
        serde_json::from_str(&body).expect("decode")
    }

    #[tokio::test(flavor = "current_thread")]
    async fn publishes_provider_timestamped_current_and_bounded_local_forecast_off_runtime() {
        let temp = TempDir::new().expect("temp");
        let root = temp.path().join("bus");
        write_location(&root, &location(7, -71.0589));
        let probe = fixture_probe(140);
        let worker = worker(&temp, probe.clone());
        let runtime_thread = std::thread::current().id();
        let outcome = worker
            .refresh_once(location(7, -71.0589), true, true)
            .await
            .expect("refresh");
        assert_eq!(outcome.current_fresh, Some(true));
        assert_eq!(outcome.forecast_fresh, Some(true));
        assert!(probe
            .threads
            .lock()
            .expect("threads")
            .iter()
            .all(|thread| *thread != runtime_thread));
        let current: CurrentWeatherSnapshot =
            latest(&root, &weather_current_state_topic("workstation-1"));
        let forecast: WeatherForecastSnapshot =
            latest(&root, &weather_forecast_state_topic("workstation-1"));
        assert_eq!(current.location_generation, 7);
        assert_eq!(forecast.location_generation, 7);
        assert_eq!(
            current.producer_at_ms,
            current.conditions.as_ref().unwrap().observed_at_ms
        );
        assert_ne!(current.producer_at_ms, current.fetched_at_ms);
        assert_eq!(forecast.hourly.len(), 120);
        assert_eq!(forecast.daily.len(), 5);
        assert!(current.conditions.unwrap().temperature.is_none());
        assert!(forecast
            .hourly
            .iter()
            .any(|hour| hour.relative_humidity_percent.is_none()));
        assert!(forecast.daily.iter().all(|day| day.source_period_count > 0));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn effective_generation_change_during_fetch_discards_both_projections() {
        let temp = TempDir::new().expect("temp");
        let root = temp.path().join("bus");
        write_location(&root, &location(7, -71.0589));
        let changed_root = root.clone();
        let mut probe = Arc::try_unwrap(fixture_probe(24))
            .unwrap_or_else(|_| panic!("fixture unexpectedly shared"));
        probe.on_hourly = Some(Box::new(move || {
            write_location(&changed_root, &location(8, -71.2));
        }));
        let worker = worker(&temp, Arc::new(probe));
        let outcome = worker
            .refresh_once(location(7, -71.0589), true, true)
            .await
            .expect("discard");
        assert_eq!(outcome.current_fresh, Some(false));
        assert_eq!(outcome.forecast_fresh, Some(false));
        let persist = Persist::open(root).expect("open Bus");
        assert!(persist
            .read_latest(&weather_current_state_topic("workstation-1"))
            .expect("read")
            .is_none());
        assert!(persist
            .read_latest(&weather_forecast_state_topic("workstation-1"))
            .expect("read")
            .is_none());
    }

    #[test]
    fn hostile_provider_urls_and_oversized_bodies_are_refused() {
        for url in [
            "http://api.weather.gov/gridpoints/BOX/71,101/forecast/hourly",
            "https://api.weather.gov.evil.test/gridpoints/BOX/71,101/forecast/hourly",
            "https://api.weather.gov@evil.test/gridpoints/BOX/71,101/forecast/hourly",
            "https://api.weather.gov:444/gridpoints/BOX/71,101/forecast/hourly",
            "https://api.weather.gov/gridpoints/BOX/71,101/forecast/hourly?next=evil",
        ] {
            assert!(validate_official_url(url).is_err(), "accepted {url}");
        }
        let oversized = "x".repeat(MAX_POINTS_BODY_BYTES + 1);
        assert!(parse_points(&oversized).is_err());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn timezone_mismatch_is_an_honest_typed_forecast_gap() {
        let temp = TempDir::new().expect("temp");
        let root = temp.path().join("bus");
        write_location(&root, &location(7, -71.0589));
        let mut probe = Arc::try_unwrap(fixture_probe(24))
            .unwrap_or_else(|_| panic!("fixture unexpectedly shared"));
        probe.points = points_json("America/Chicago");
        let worker = worker(&temp, Arc::new(probe));
        let outcome = worker
            .refresh_once(location(7, -71.0589), true, true)
            .await
            .expect("partial refresh");
        assert_eq!(outcome.current_fresh, Some(true));
        assert_eq!(outcome.forecast_fresh, Some(false));
        let forecast: WeatherForecastSnapshot =
            latest(&root, &weather_forecast_state_topic("workstation-1"));
        assert!(matches!(
            forecast.availability,
            WeatherAvailability::Unavailable {
                reason: WeatherUnavailableReason::ForecastUnavailable
            }
        ));
        assert!(forecast.hourly.is_empty() && forecast.daily.is_empty());
        assert!(forecast.gaps[0].contains("timezone"));
    }

    #[test]
    fn scheduler_separates_cadences_and_generation_change_is_immediately_due() {
        let mut schedule = RefreshSchedule::new(NOW);
        assert_eq!(schedule.due(NOW, 7), (true, true));
        schedule.record(
            NOW,
            RefreshOutcome {
                current_fresh: Some(true),
                forecast_fresh: Some(true),
            },
        );
        assert_eq!(schedule.due(NOW + 5 * 60 * 1_000 - 1, 7), (false, false));
        assert_eq!(schedule.due(NOW + 5 * 60 * 1_000, 7), (true, false));
        schedule.record(
            NOW + 5 * 60 * 1_000,
            RefreshOutcome {
                current_fresh: Some(true),
                forecast_fresh: None,
            },
        );
        assert_eq!(schedule.due(NOW + 10 * 60 * 1_000, 7), (true, true));
        schedule.record(
            NOW + 10 * 60 * 1_000,
            RefreshOutcome {
                current_fresh: Some(true),
                forecast_fresh: Some(true),
            },
        );
        assert_eq!(schedule.due(NOW + 10 * 60 * 1_000 + 1, 8), (true, true));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn restart_cache_stales_at_ninety_minutes_expires_at_six_hours_and_never_crosses_generation(
    ) {
        let temp = TempDir::new().expect("temp");
        let root = temp.path().join("bus");
        write_location(&root, &location(7, -71.0589));
        let initial = worker(&temp, fixture_probe(24));
        let initial_outcome = initial
            .refresh_once(location(7, -71.0589), true, true)
            .await
            .expect("initial refresh");
        assert_eq!(initial_outcome.current_fresh, Some(true));
        assert!(initial.cache_path.is_file());

        let stale_now = NOW + 91 * 60 * 1_000;
        let restarted = worker_at(&temp, fixture_probe(24), stale_now);
        let stale_outcome = restarted
            .refresh_once(location(7, -71.0589), true, true)
            .await
            .expect("stale cache recovery");
        assert_eq!(stale_outcome.current_fresh, Some(false));
        assert_eq!(stale_outcome.forecast_fresh, Some(false));
        let stale_current: CurrentWeatherSnapshot =
            latest(&root, &weather_current_state_topic("workstation-1"));
        let stale_forecast: WeatherForecastSnapshot =
            latest(&root, &weather_forecast_state_topic("workstation-1"));
        assert!(matches!(
            stale_current.availability,
            WeatherAvailability::Stale { .. }
        ));
        assert!(matches!(
            stale_forecast.availability,
            WeatherAvailability::Stale { .. }
        ));

        write_location(&root, &location(8, -71.2));
        let wrong_generation = worker_at(&temp, fixture_probe(24), stale_now);
        wrong_generation
            .refresh_once(location(8, -71.2), true, true)
            .await
            .expect("mismatched cache refused");
        let current: CurrentWeatherSnapshot =
            latest(&root, &weather_current_state_topic("workstation-1"));
        assert_eq!(current.location_generation, 8);
        assert!(matches!(
            current.availability,
            WeatherAvailability::Unavailable { .. }
        ));

        write_location(&root, &location(7, -71.0589));
        let expired_now = NOW + 6 * 60 * 60 * 1_000 + 31 * 60 * 1_000;
        let expired = worker_at(&temp, fixture_probe(24), expired_now);
        expired
            .refresh_once(location(7, -71.0589), true, true)
            .await
            .expect("expired cache refused");
        let expired_current: CurrentWeatherSnapshot =
            latest(&root, &weather_current_state_topic("workstation-1"));
        let expired_forecast: WeatherForecastSnapshot =
            latest(&root, &weather_forecast_state_topic("workstation-1"));
        assert!(matches!(
            expired_current.availability,
            WeatherAvailability::Unavailable { .. }
        ));
        assert!(matches!(
            expired_forecast.availability,
            WeatherAvailability::Unavailable { .. }
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn restart_refuses_hostile_nested_cache_identity_even_when_envelope_matches() {
        let temp = TempDir::new().expect("temp");
        let root = temp.path().join("bus");
        let authority = location(7, -71.0589);
        write_location(&root, &authority);

        let initial = worker(&temp, fixture_probe(24));
        initial
            .refresh_once(authority.clone(), true, true)
            .await
            .expect("initial refresh");
        let mut cache = load_cache(&initial.cache_path)
            .expect("load cache")
            .expect("cache exists");
        cache
            .current
            .as_mut()
            .expect("cached current")
            .location_generation = 6;
        cache.forecast.as_mut().expect("cached forecast").time_zone = "America/Chicago".into();
        store_cache(&initial.cache_path, &cache).expect("store hostile cache");

        // At this age the fixture provider is intentionally stale, forcing the
        // restarted worker through cache recovery without crossing authority.
        let restarted = worker_at(&temp, fixture_probe(24), NOW + 91 * 60 * 1_000);
        let outcome = restarted
            .refresh_once(authority, true, true)
            .await
            .expect("hostile cache is rejected without stopping refresh");
        assert_eq!(outcome.current_fresh, Some(false));
        assert_eq!(outcome.forecast_fresh, Some(false));

        let current: CurrentWeatherSnapshot =
            latest(&root, &weather_current_state_topic("workstation-1"));
        let forecast: WeatherForecastSnapshot =
            latest(&root, &weather_forecast_state_topic("workstation-1"));
        assert_eq!(current.location_generation, 7);
        assert_eq!(forecast.location_generation, 7);
        assert!(matches!(
            current.availability,
            WeatherAvailability::Unavailable { .. }
        ));
        assert!(matches!(
            forecast.availability,
            WeatherAvailability::Unavailable { .. }
        ));
        assert!(current.conditions.is_none());
        assert!(forecast.hourly.is_empty() && forecast.daily.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn cache_loader_refuses_symlink_and_oversize_records() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("temp");
        let target = temp.path().join("target");
        fs::write(&target, b"{}").expect("target");
        let link = temp.path().join("cache-link");
        symlink(&target, &link).expect("symlink");
        assert!(load_cache(&link).is_err());

        let oversized = temp.path().join("oversized");
        let file = File::create(&oversized).expect("oversized");
        file.set_len(u64::try_from(MAX_CACHE_BYTES + 1).unwrap())
            .expect("set len");
        assert!(load_cache(&oversized).is_err());
    }

    #[test]
    fn provider_offset_controls_local_day_at_dst_boundary() {
        let generated = NOW - 60_000;
        let first = ForecastPeriodDocument {
            number: 1,
            start_time: "2027-11-07T01:00:00-04:00".into(),
            end_time: "2027-11-07T01:00:00-05:00".into(),
            is_daytime: false,
            temperature: 50.0,
            temperature_unit: "F".into(),
            probability_of_precipitation: Quantity::default(),
            relative_humidity: Quantity::default(),
            wind_speed: "5 mph".into(),
            wind_direction: "N".into(),
            short_forecast: "Clear".into(),
        };
        let hour = normalize_hour(first, generated)
            .expect("normalize")
            .expect("future");
        assert_eq!(hour.local_date, "2027-11-07");
        assert_eq!(hour.end_at_ms - hour.start_at_ms, 3_600_000);
    }
}
