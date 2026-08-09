//! Bounded current-condition and general forecast contracts.
//!
//! These projections are location-generation scoped and separate from the
//! existing vehicle drive-ahead NWS overlay. Measurements are optional and
//! unit tagged; absence is never represented by a synthetic zero.

#![allow(
    missing_docs,
    reason = "public fields and closed variants form the documented v1 wire contract"
)]
#![allow(
    clippy::missing_errors_doc,
    reason = "WeatherContractError is the closed error vocabulary for every admission helper"
)]

use crate::location::{
    decode_json, validate_id, validate_len, validate_not_future, validate_point, validate_text,
    WeatherContractError, MAX_WEATHER_ID_BYTES, MAX_WEATHER_LABEL_BYTES, MAX_WEATHER_REASON_BYTES,
};
use crate::nws_alert::GeoPoint;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const WEATHER_CONTRACT_SCHEMA_VERSION: u16 = 1;
pub const WEATHER_CURRENT_STATE_PREFIX: &str = "state/weather/current/";
pub const WEATHER_FORECAST_STATE_PREFIX: &str = "state/weather/forecast/";
pub const WEATHER_MAP_STATE_PREFIX: &str = "state/weather/map/";
pub const WEATHER_SET_MAP_VIEWPORT_PREFIX: &str = "action/weather/set-map-viewport/";
pub const WEATHER_MAP_VIEWPORT_STATE_PREFIX: &str = "state/weather/map-viewport/";
pub const MAX_WEATHER_WIRE_BYTES: usize = 512 * 1024;
pub const MAX_WEATHER_VIEWPORT_WIRE_BYTES: usize = 16 * 1024;
pub const MAX_WEATHER_HOURLY_PERIODS: usize = 120;
pub const MAX_WEATHER_DAILY_SUMMARIES: usize = 5;
pub const MAX_WEATHER_GAPS: usize = 16;
pub const MAX_WEATHER_ALERT_REFERENCES: usize = 32;
pub const MAX_WEATHER_ATTRIBUTIONS: usize = 8;
pub const MAX_CURRENT_AGE_MS: i64 = 90 * 60 * 1_000;
pub const MAX_LAST_GOOD_AGE_MS: i64 = 6 * 60 * 60 * 1_000;
pub const MAX_FORECAST_FRESH_AGE_MS: i64 = 90 * 60 * 1_000;
pub const MAX_FORECAST_HORIZON_MS: i64 = 6 * 24 * 60 * 60 * 1_000;
pub const MAX_HOURLY_PERIOD_MS: i64 = 2 * 60 * 60 * 1_000;
pub const MAX_DAILY_SOURCE_PERIODS: u16 = 25;
pub const MAX_ATMOSPHERIC_MAP_WIRE_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_ATMOSPHERIC_FIELD_PNG_BYTES: usize = 256 * 1024;
pub const ATMOSPHERIC_FIELD_EDGE: u16 = 256;
pub const MAX_ATMOSPHERIC_FRESH_AGE_MS: i64 = 20 * 60 * 1_000;
pub const MAX_ATMOSPHERIC_CACHE_AGE_MS: i64 = 2 * 60 * 60 * 1_000;
pub const NOWCOAST_NDFD_TEMPERATURE_PATH: &str = "/geoserver/forecasts/ndfd_temperature/ows";
pub const NOWCOAST_NDFD_TEMPERATURE_LAYER: &str = "air_temperature";
pub const NOWCOAST_NDFD_WIND_PATH: &str = "/geoserver/forecasts/ndfd_wind/ows";
pub const NOWCOAST_NDFD_WIND_LAYER: &str = "wind_velocity";
pub const NOWCOAST_NDFD_SKY_PATH: &str = "/geoserver/forecasts/ndfd_sky/ows";
pub const NOWCOAST_NDFD_SKY_LAYER: &str = "total_sky_cover";

#[must_use]
pub fn weather_current_state_topic(host: &str) -> String {
    format!("{WEATHER_CURRENT_STATE_PREFIX}{host}")
}

#[must_use]
pub fn weather_forecast_state_topic(host: &str) -> String {
    format!("{WEATHER_FORECAST_STATE_PREFIX}{host}")
}

#[must_use]
pub fn weather_map_state_topic(host: &str) -> String {
    format!("{WEATHER_MAP_STATE_PREFIX}{host}")
}

#[must_use]
pub fn weather_set_map_viewport_topic(host: &str) -> String {
    format!("{WEATHER_SET_MAP_VIEWPORT_PREFIX}{host}")
}

#[must_use]
pub fn weather_map_viewport_state_topic(host: &str) -> String {
    format!("{WEATHER_MAP_VIEWPORT_STATE_PREFIX}{host}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WeatherProvider {
    NationalWeatherService,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WeatherConditionKind {
    ClearDay,
    ClearNight,
    Clouds,
    Rain,
    Wintry,
    Storm,
    Fog,
    Wind,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemperatureUnit {
    Celsius,
    Fahrenheit,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Temperature {
    pub value: f32,
    pub unit: TemperatureUnit,
}

impl Temperature {
    fn validate(self, field: &'static str) -> Result<(), WeatherContractError> {
        let admitted = match self.unit {
            TemperatureUnit::Celsius => (-100.0..=60.0).contains(&self.value),
            TemperatureUnit::Fahrenheit => (-148.0..=140.0).contains(&self.value),
        };
        if !self.value.is_finite() || !admitted {
            return Err(WeatherContractError::InvalidMeasurement(field));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeedUnit {
    MetresPerSecond,
    KilometresPerHour,
    MilesPerHour,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Speed {
    pub value: f32,
    pub unit: SpeedUnit,
}

impl Speed {
    fn validate(self, field: &'static str) -> Result<(), WeatherContractError> {
        let max = match self.unit {
            SpeedUnit::MetresPerSecond => 150.0,
            SpeedUnit::KilometresPerHour => 540.0,
            SpeedUnit::MilesPerHour => 335.0,
        };
        if !self.value.is_finite() || !(0.0..=max).contains(&self.value) {
            return Err(WeatherContractError::InvalidMeasurement(field));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DistanceUnit {
    Metres,
    Kilometres,
    Miles,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Distance {
    pub value: f32,
    pub unit: DistanceUnit,
}

impl Distance {
    fn validate(self, field: &'static str) -> Result<(), WeatherContractError> {
        let max = match self.unit {
            DistanceUnit::Metres => 500_000.0,
            DistanceUnit::Kilometres => 500.0,
            DistanceUnit::Miles => 310.7,
        };
        if !self.value.is_finite() || !(0.0..=max).contains(&self.value) {
            return Err(WeatherContractError::InvalidMeasurement(field));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PressureUnit {
    Pascals,
    Hectopascals,
    InchesMercury,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pressure {
    pub value: f32,
    pub unit: PressureUnit,
}

impl Pressure {
    fn validate(self, field: &'static str) -> Result<(), WeatherContractError> {
        let admitted = match self.unit {
            PressureUnit::Pascals => (80_000.0..=110_000.0).contains(&self.value),
            PressureUnit::Hectopascals => (800.0..=1_100.0).contains(&self.value),
            PressureUnit::InchesMercury => (23.6..=32.5).contains(&self.value),
        };
        if !self.value.is_finite() || !admitted {
            return Err(WeatherContractError::InvalidMeasurement(field));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WeatherAttribution {
    pub provider: WeatherProvider,
    pub source_id: String,
    pub label: String,
}

impl WeatherAttribution {
    fn validate(&self) -> Result<(), WeatherContractError> {
        validate_id(&self.source_id, "source_id")?;
        validate_text(&self.label, "attribution", MAX_WEATHER_LABEL_BYTES)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WeatherUnavailableReason {
    LocationUnavailable,
    UnsupportedCoverage,
    ProviderUnavailable,
    ObservationUnavailable,
    ForecastUnavailable,
    Expired,
    InvalidProviderData,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WeatherStaleReason {
    ProviderBackoff,
    PartialProviderFailure,
    RefreshFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum WeatherAvailability {
    Fresh,
    Stale { reason: WeatherStaleReason },
    Unavailable { reason: WeatherUnavailableReason },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AtmosphericFieldKind {
    Temperature,
    Wind,
    CloudCover,
}

impl AtmosphericFieldKind {
    #[must_use]
    pub const fn nowcoast_product(self) -> (&'static str, &'static str) {
        match self {
            Self::Temperature => (
                NOWCOAST_NDFD_TEMPERATURE_PATH,
                NOWCOAST_NDFD_TEMPERATURE_LAYER,
            ),
            Self::Wind => (NOWCOAST_NDFD_WIND_PATH, NOWCOAST_NDFD_WIND_LAYER),
            Self::CloudCover => (NOWCOAST_NDFD_SKY_PATH, NOWCOAST_NDFD_SKY_LAYER),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AtmosphericViewport {
    pub generation: u64,
    pub zoom: u8,
    pub x: u32,
    pub y: u32,
    pub pixel_width: u16,
    pub pixel_height: u16,
}

impl AtmosphericViewport {
    fn validate(&self) -> Result<(), WeatherContractError> {
        if self.generation == 0
            || !(2..=12).contains(&self.zoom)
            || self.x >= (1_u32 << self.zoom)
            || self.y >= (1_u32 << self.zoom)
            || self.pixel_width != ATMOSPHERIC_FIELD_EDGE
            || self.pixel_height != ATMOSPHERIC_FIELD_EDGE
        {
            return Err(WeatherContractError::InvalidRelationship(
                "atmospheric_viewport",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetWeatherMapViewportRequest {
    pub schema_version: u16,
    pub request_id: String,
    pub target_host: String,
    pub expected_location_generation: u64,
    pub viewport: AtmosphericViewport,
    pub issued_at_ms: i64,
}

impl SetWeatherMapViewportRequest {
    pub fn validate_at(&self, now_ms: i64) -> Result<(), WeatherContractError> {
        validate_weather_schema(self.schema_version)?;
        validate_id(&self.request_id, "request_id")?;
        validate_id(&self.target_host, "target_host")?;
        if self.expected_location_generation == 0 {
            return Err(WeatherContractError::InvalidGeneration);
        }
        self.viewport.validate()?;
        validate_not_future(self.issued_at_ms, now_ms, "issued_at_ms")
    }

    pub fn from_json_at(body: &[u8], now_ms: i64) -> Result<Self, WeatherContractError> {
        let value: Self = decode_json(body, MAX_WEATHER_VIEWPORT_WIRE_BYTES)?;
        value.validate_at(now_ms)?;
        Ok(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WeatherMapViewportSource {
    MapsAction,
    EffectiveLocationFallback,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WeatherMapViewportState {
    pub schema_version: u16,
    pub host: String,
    pub location_generation: u64,
    pub viewport: AtmosphericViewport,
    pub source: WeatherMapViewportSource,
    pub admitted_at_ms: i64,
}

impl WeatherMapViewportState {
    pub fn validate_at(&self, now_ms: i64) -> Result<(), WeatherContractError> {
        validate_weather_schema(self.schema_version)?;
        validate_id(&self.host, "host")?;
        if self.location_generation == 0 {
            return Err(WeatherContractError::InvalidGeneration);
        }
        self.viewport.validate()?;
        validate_not_future(self.admitted_at_ms, now_ms, "admitted_at_ms")
    }

    pub fn from_json_at(body: &[u8], now_ms: i64) -> Result<Self, WeatherContractError> {
        let value: Self = decode_json(body, MAX_WEATHER_VIEWPORT_WIRE_BYTES)?;
        value.validate_at(now_ms)?;
        Ok(value)
    }
}

const fn validate_weather_schema(schema_version: u16) -> Result<(), WeatherContractError> {
    if schema_version != WEATHER_CONTRACT_SCHEMA_VERSION {
        return Err(WeatherContractError::UnsupportedSchema {
            found: schema_version,
        });
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AtmosphericFieldImage {
    pub kind: AtmosphericFieldKind,
    pub provider_service_path: String,
    pub provider_layer_name: String,
    pub pixel_width: u16,
    pub pixel_height: u16,
    pub png_base64: String,
}

impl AtmosphericFieldImage {
    fn validate(&self) -> Result<(), WeatherContractError> {
        let (expected_service, expected_layer) = self.kind.nowcoast_product();
        if self.provider_service_path != expected_service
            || self.provider_layer_name != expected_layer
            || self.pixel_width != ATMOSPHERIC_FIELD_EDGE
            || self.pixel_height != ATMOSPHERIC_FIELD_EDGE
        {
            return Err(WeatherContractError::InvalidRelationship(
                "atmospheric_field_identity",
            ));
        }
        let png = decode_bounded_base64(
            &self.png_base64,
            MAX_ATMOSPHERIC_FIELD_PNG_BYTES,
            "atmospheric_png",
        )?;
        validate_png_dimensions(
            &png,
            u32::from(self.pixel_width),
            u32::from(self.pixel_height),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AtmosphericMapSnapshot {
    pub schema_version: u16,
    pub host: String,
    pub location_generation: u64,
    pub location_point: GeoPoint,
    pub viewport: AtmosphericViewport,
    pub rendered_at_ms: i64,
    pub fetched_at_ms: i64,
    pub availability: WeatherAvailability,
    #[serde(default)]
    pub fields: Vec<AtmosphericFieldImage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gaps: Vec<String>,
    pub attributions: Vec<WeatherAttribution>,
}

impl AtmosphericMapSnapshot {
    pub fn validate_at(&self, now_ms: i64) -> Result<(), WeatherContractError> {
        validate_common(
            self.schema_version,
            &self.host,
            self.location_generation,
            Some(&self.location_point),
            self.rendered_at_ms,
            self.fetched_at_ms,
            &self.gaps,
            &self.attributions,
            now_ms,
        )?;
        self.viewport.validate()?;
        let age = now_ms.saturating_sub(self.rendered_at_ms).max(0);
        match self.availability {
            WeatherAvailability::Fresh => {
                if age > MAX_ATMOSPHERIC_FRESH_AGE_MS {
                    return Err(WeatherContractError::InvalidRelationship(
                        "fresh_atmospheric_age",
                    ));
                }
                self.validate_available_fields()?;
            }
            WeatherAvailability::Stale { .. } => {
                if !(MAX_ATMOSPHERIC_FRESH_AGE_MS < age && age <= MAX_ATMOSPHERIC_CACHE_AGE_MS) {
                    return Err(WeatherContractError::InvalidRelationship(
                        "stale_atmospheric_age",
                    ));
                }
                self.validate_available_fields()?;
            }
            WeatherAvailability::Unavailable { .. } => {
                if !self.fields.is_empty() {
                    return Err(WeatherContractError::InvalidRelationship(
                        "unavailable_atmospheric_payload",
                    ));
                }
            }
        }
        Ok(())
    }

    fn validate_available_fields(&self) -> Result<(), WeatherContractError> {
        if self.fields.len() != 3 {
            return Err(WeatherContractError::InvalidRelationship(
                "complete_atmospheric_fields",
            ));
        }
        let mut kinds = BTreeSet::new();
        for field in &self.fields {
            field.validate()?;
            if !kinds.insert(field.kind) {
                return Err(WeatherContractError::Duplicate("atmospheric_fields"));
            }
        }
        Ok(())
    }

    pub fn from_json_at(body: &[u8], now_ms: i64) -> Result<Self, WeatherContractError> {
        let value: Self = decode_json(body, MAX_ATMOSPHERIC_MAP_WIRE_BYTES)?;
        value.validate_at(now_ms)?;
        Ok(value)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CurrentConditions {
    pub observed_at_ms: i64,
    pub condition: WeatherConditionKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<Temperature>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apparent_temperature: Option<Temperature>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relative_humidity_percent: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub precipitation_probability_percent: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wind_speed: Option<Speed>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wind_direction_degrees: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wind_gust: Option<Speed>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility: Option<Distance>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pressure: Option<Pressure>,
}

impl CurrentConditions {
    fn validate(&self, now_ms: i64) -> Result<(), WeatherContractError> {
        validate_not_future(self.observed_at_ms, now_ms, "observed_at_ms")?;
        if self.condition == WeatherConditionKind::Unavailable {
            return Err(WeatherContractError::InvalidRelationship(
                "available_condition_kind",
            ));
        }
        validate_optional_text(self.provider_text.as_ref(), "provider_text")?;
        validate_optional_temperature(self.temperature, "temperature")?;
        validate_optional_temperature(self.apparent_temperature, "apparent_temperature")?;
        validate_percent(self.relative_humidity_percent, "relative_humidity_percent")?;
        validate_percent(
            self.precipitation_probability_percent,
            "precipitation_probability_percent",
        )?;
        validate_optional_speed(self.wind_speed, "wind_speed")?;
        validate_direction(self.wind_direction_degrees, "wind_direction_degrees")?;
        validate_optional_speed(self.wind_gust, "wind_gust")?;
        if let Some(value) = &self.visibility {
            value.validate("visibility")?;
        }
        if let Some(value) = &self.pressure {
            value.validate("pressure")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CurrentWeatherSnapshot {
    pub schema_version: u16,
    pub host: String,
    pub location_generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location_point: Option<GeoPoint>,
    pub producer_at_ms: i64,
    pub fetched_at_ms: i64,
    pub availability: WeatherAvailability,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conditions: Option<CurrentConditions>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gaps: Vec<String>,
    pub attributions: Vec<WeatherAttribution>,
}

impl CurrentWeatherSnapshot {
    pub fn validate_at(&self, now_ms: i64) -> Result<(), WeatherContractError> {
        validate_common(
            self.schema_version,
            &self.host,
            self.location_generation,
            self.location_point.as_ref(),
            self.producer_at_ms,
            self.fetched_at_ms,
            &self.gaps,
            &self.attributions,
            now_ms,
        )?;
        match (&self.availability, &self.conditions) {
            (WeatherAvailability::Fresh, Some(conditions)) => {
                require_location_point(self.location_point.as_ref())?;
                conditions.validate(now_ms)?;
                let age = now_ms.saturating_sub(conditions.observed_at_ms).max(0);
                if age > MAX_CURRENT_AGE_MS {
                    return Err(WeatherContractError::InvalidRelationship(
                        "fresh_current_age",
                    ));
                }
            }
            (WeatherAvailability::Stale { .. }, Some(conditions)) => {
                require_location_point(self.location_point.as_ref())?;
                conditions.validate(now_ms)?;
                let age = now_ms.saturating_sub(conditions.observed_at_ms).max(0);
                if !(MAX_CURRENT_AGE_MS < age && age <= MAX_LAST_GOOD_AGE_MS) {
                    return Err(WeatherContractError::InvalidRelationship(
                        "stale_current_age",
                    ));
                }
            }
            (WeatherAvailability::Unavailable { .. }, None) => {}
            _ => {
                return Err(WeatherContractError::InvalidRelationship(
                    "current_availability_payload",
                ));
            }
        }
        Ok(())
    }

    pub fn from_json_at(body: &[u8], now_ms: i64) -> Result<Self, WeatherContractError> {
        let value: Self = decode_json(body, MAX_WEATHER_WIRE_BYTES)?;
        value.validate_at(now_ms)?;
        Ok(value)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HourlyForecastPeriod {
    pub sequence: u16,
    pub start_at_ms: i64,
    pub end_at_ms: i64,
    pub local_date: String,
    pub is_daytime: bool,
    pub condition: WeatherConditionKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<Temperature>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub precipitation_probability_percent: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relative_humidity_percent: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wind_speed: Option<Speed>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wind_direction_degrees: Option<f32>,
}

impl HourlyForecastPeriod {
    fn validate(&self, producer_at_ms: i64) -> Result<(), WeatherContractError> {
        if self.end_at_ms <= self.start_at_ms
            || self.end_at_ms.saturating_sub(self.start_at_ms) > MAX_HOURLY_PERIOD_MS
            || self.end_at_ms <= producer_at_ms
            || self.start_at_ms > producer_at_ms.saturating_add(MAX_FORECAST_HORIZON_MS)
        {
            return Err(WeatherContractError::InvalidTimestamp("hourly_period"));
        }
        validate_local_date(&self.local_date)?;
        if self.condition == WeatherConditionKind::Unavailable {
            return Err(WeatherContractError::InvalidRelationship(
                "hourly_condition_kind",
            ));
        }
        validate_optional_text(self.provider_text.as_ref(), "provider_text")?;
        validate_optional_temperature(self.temperature, "hourly_temperature")?;
        validate_percent(
            self.precipitation_probability_percent,
            "hourly_precipitation_probability_percent",
        )?;
        validate_percent(
            self.relative_humidity_percent,
            "hourly_relative_humidity_percent",
        )?;
        validate_optional_speed(self.wind_speed, "hourly_wind_speed")?;
        validate_direction(self.wind_direction_degrees, "hourly_wind_direction_degrees")
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalDaySummary {
    pub local_date: String,
    pub condition: WeatherConditionKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub high_temperature: Option<Temperature>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub low_temperature: Option<Temperature>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub precipitation_probability_percent: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak_wind_speed: Option<Speed>,
    pub source_period_count: u16,
}

impl LocalDaySummary {
    fn validate(&self) -> Result<(), WeatherContractError> {
        validate_local_date(&self.local_date)?;
        if self.condition == WeatherConditionKind::Unavailable
            || self.source_period_count == 0
            || self.source_period_count > MAX_DAILY_SOURCE_PERIODS
        {
            return Err(WeatherContractError::InvalidRelationship("daily_summary"));
        }
        validate_optional_text(self.provider_text.as_ref(), "daily_provider_text")?;
        validate_optional_temperature(self.high_temperature, "daily_high_temperature")?;
        validate_optional_temperature(self.low_temperature, "daily_low_temperature")?;
        if let (Some(high), Some(low)) = (&self.high_temperature, &self.low_temperature) {
            if high.unit != low.unit || high.value < low.value {
                return Err(WeatherContractError::InvalidRelationship(
                    "daily_temperature_range",
                ));
            }
        }
        validate_percent(
            self.precipitation_probability_percent,
            "daily_precipitation_probability_percent",
        )?;
        validate_optional_speed(self.peak_wind_speed, "daily_peak_wind_speed")
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WeatherForecastSnapshot {
    pub schema_version: u16,
    pub host: String,
    pub location_generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location_point: Option<GeoPoint>,
    pub time_zone: String,
    pub producer_at_ms: i64,
    pub fetched_at_ms: i64,
    pub availability: WeatherAvailability,
    #[serde(default)]
    pub hourly: Vec<HourlyForecastPeriod>,
    #[serde(default)]
    pub daily: Vec<LocalDaySummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alert_references: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gaps: Vec<String>,
    pub attributions: Vec<WeatherAttribution>,
}

impl WeatherForecastSnapshot {
    pub fn validate_at(&self, now_ms: i64) -> Result<(), WeatherContractError> {
        validate_common(
            self.schema_version,
            &self.host,
            self.location_generation,
            self.location_point.as_ref(),
            self.producer_at_ms,
            self.fetched_at_ms,
            &self.gaps,
            &self.attributions,
            now_ms,
        )?;
        validate_time_zone(&self.time_zone)?;
        if self.hourly.len() > MAX_WEATHER_HOURLY_PERIODS {
            return Err(WeatherContractError::CapacityExceeded {
                field: "hourly",
                max: MAX_WEATHER_HOURLY_PERIODS,
            });
        }
        if self.daily.len() > MAX_WEATHER_DAILY_SUMMARIES {
            return Err(WeatherContractError::CapacityExceeded {
                field: "daily",
                max: MAX_WEATHER_DAILY_SUMMARIES,
            });
        }
        validate_bounded_unique_texts(
            &self.alert_references,
            "alert_references",
            MAX_WEATHER_ALERT_REFERENCES,
            MAX_WEATHER_ID_BYTES,
        )?;
        match self.availability {
            WeatherAvailability::Unavailable { .. } => {
                if !self.hourly.is_empty() || !self.daily.is_empty() {
                    return Err(WeatherContractError::InvalidRelationship(
                        "unavailable_forecast_payload",
                    ));
                }
            }
            WeatherAvailability::Fresh | WeatherAvailability::Stale { .. } => {
                require_location_point(self.location_point.as_ref())?;
                if self.hourly.is_empty() || self.daily.is_empty() {
                    return Err(WeatherContractError::InvalidRelationship(
                        "available_forecast_payload",
                    ));
                }
            }
        }
        let forecast_age = now_ms.saturating_sub(self.producer_at_ms).max(0);
        match self.availability {
            WeatherAvailability::Fresh if forecast_age > MAX_FORECAST_FRESH_AGE_MS => {
                return Err(WeatherContractError::InvalidRelationship(
                    "fresh_forecast_age",
                ));
            }
            WeatherAvailability::Stale { .. }
                if !(MAX_FORECAST_FRESH_AGE_MS < forecast_age
                    && forecast_age <= MAX_LAST_GOOD_AGE_MS) =>
            {
                return Err(WeatherContractError::InvalidRelationship(
                    "stale_forecast_age",
                ));
            }
            _ => {}
        }
        let mut sequences = BTreeSet::new();
        let mut previous_start = None;
        for period in &self.hourly {
            period.validate(self.producer_at_ms)?;
            if !sequences.insert(period.sequence) {
                return Err(WeatherContractError::Duplicate("hourly_sequence"));
            }
            if previous_start.is_some_and(|previous| period.start_at_ms <= previous) {
                return Err(WeatherContractError::InvalidRelationship("hourly_order"));
            }
            previous_start = Some(period.start_at_ms);
        }
        let mut dates = BTreeSet::new();
        let mut previous_date: Option<&str> = None;
        for day in &self.daily {
            day.validate()?;
            if !dates.insert(day.local_date.as_str()) {
                return Err(WeatherContractError::Duplicate("daily_local_date"));
            }
            if previous_date.is_some_and(|previous| day.local_date.as_str() <= previous) {
                return Err(WeatherContractError::InvalidRelationship("daily_order"));
            }
            previous_date = Some(day.local_date.as_str());
        }
        Ok(())
    }

    pub fn from_json_at(body: &[u8], now_ms: i64) -> Result<Self, WeatherContractError> {
        let value: Self = decode_json(body, MAX_WEATHER_WIRE_BYTES)?;
        value.validate_at(now_ms)?;
        Ok(value)
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "all projection headers share this one internal invariant gate"
)]
fn validate_common(
    schema_version: u16,
    host: &str,
    location_generation: u64,
    location_point: Option<&GeoPoint>,
    producer_at_ms: i64,
    fetched_at_ms: i64,
    gaps: &[String],
    attributions: &[WeatherAttribution],
    now_ms: i64,
) -> Result<(), WeatherContractError> {
    if schema_version != WEATHER_CONTRACT_SCHEMA_VERSION {
        return Err(WeatherContractError::UnsupportedSchema {
            found: schema_version,
        });
    }
    validate_id(host, "host")?;
    if location_generation == 0 {
        return Err(WeatherContractError::InvalidGeneration);
    }
    if let Some(location_point) = location_point {
        validate_point(location_point)?;
    }
    validate_not_future(producer_at_ms, now_ms, "producer_at_ms")?;
    validate_not_future(fetched_at_ms, now_ms, "fetched_at_ms")?;
    if fetched_at_ms < producer_at_ms {
        return Err(WeatherContractError::InvalidTimestamp("fetched_at_ms"));
    }
    validate_bounded_unique_texts(gaps, "gaps", MAX_WEATHER_GAPS, MAX_WEATHER_REASON_BYTES)?;
    if attributions.is_empty() || attributions.len() > MAX_WEATHER_ATTRIBUTIONS {
        return Err(WeatherContractError::CapacityExceeded {
            field: "attributions",
            max: MAX_WEATHER_ATTRIBUTIONS,
        });
    }
    let mut sources = BTreeSet::new();
    for attribution in attributions {
        attribution.validate()?;
        if !sources.insert((attribution.provider, attribution.source_id.as_str())) {
            return Err(WeatherContractError::Duplicate("attributions"));
        }
    }
    Ok(())
}

fn require_location_point(point: Option<&GeoPoint>) -> Result<(), WeatherContractError> {
    point
        .map(|_| ())
        .ok_or(WeatherContractError::InvalidRelationship(
            "available_location_point",
        ))
}

fn validate_bounded_unique_texts(
    values: &[String],
    field: &'static str,
    max_count: usize,
    max_bytes: usize,
) -> Result<(), WeatherContractError> {
    if values.len() > max_count {
        return Err(WeatherContractError::CapacityExceeded {
            field,
            max: max_count,
        });
    }
    let mut seen = BTreeSet::new();
    for value in values {
        validate_text(value, field, max_bytes)?;
        if !seen.insert(value) {
            return Err(WeatherContractError::Duplicate(field));
        }
    }
    Ok(())
}

fn validate_optional_text(
    value: Option<&String>,
    field: &'static str,
) -> Result<(), WeatherContractError> {
    if let Some(value) = value {
        validate_text(value, field, MAX_WEATHER_LABEL_BYTES)?;
    }
    Ok(())
}

fn validate_optional_temperature(
    value: Option<Temperature>,
    field: &'static str,
) -> Result<(), WeatherContractError> {
    if let Some(value) = value {
        value.validate(field)?;
    }
    Ok(())
}

fn validate_optional_speed(
    value: Option<Speed>,
    field: &'static str,
) -> Result<(), WeatherContractError> {
    if let Some(value) = value {
        value.validate(field)?;
    }
    Ok(())
}

fn validate_percent(value: Option<f32>, field: &'static str) -> Result<(), WeatherContractError> {
    if value.is_some_and(|value| !value.is_finite() || !(0.0..=100.0).contains(&value)) {
        return Err(WeatherContractError::InvalidMeasurement(field));
    }
    Ok(())
}

fn validate_direction(value: Option<f32>, field: &'static str) -> Result<(), WeatherContractError> {
    if value.is_some_and(|value| !value.is_finite() || !(0.0..360.0).contains(&value)) {
        return Err(WeatherContractError::InvalidMeasurement(field));
    }
    Ok(())
}

fn validate_local_date(value: &str) -> Result<(), WeatherContractError> {
    let bytes = value.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes
            .iter()
            .enumerate()
            .any(|(index, byte)| !matches!(index, 4 | 7) && !byte.is_ascii_digit())
    {
        return Err(WeatherContractError::InvalidField("local_date"));
    }
    let year = value[0..4].parse::<u16>().unwrap_or(0);
    let month = value[5..7].parse::<u8>().unwrap_or(0);
    let day = value[8..10].parse::<u8>().unwrap_or(0);
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let max_day = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => 0,
    };
    if year < 1970 || day == 0 || day > max_day {
        return Err(WeatherContractError::InvalidField("local_date"));
    }
    Ok(())
}

fn validate_time_zone(value: &str) -> Result<(), WeatherContractError> {
    validate_len(value, "time_zone", 64)?;
    if !value.contains('/')
        || value.starts_with('/')
        || value.ends_with('/')
        || value.bytes().any(|byte| {
            !(byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'_' | b'-' | b'+'))
        })
    {
        return Err(WeatherContractError::InvalidField("time_zone"));
    }
    Ok(())
}

fn decode_bounded_base64(
    value: &str,
    max_bytes: usize,
    field: &'static str,
) -> Result<Vec<u8>, WeatherContractError> {
    let max_encoded = max_bytes.div_ceil(3).saturating_mul(4);
    if value.is_empty() || value.len() > max_encoded || value.len() % 4 != 0 {
        return Err(WeatherContractError::InvalidField(field));
    }
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity((bytes.len() / 4).saturating_mul(3));
    for (index, chunk) in bytes.chunks_exact(4).enumerate() {
        let final_chunk = index + 1 == bytes.len() / 4;
        let a = base64_value(chunk[0]).ok_or(WeatherContractError::InvalidField(field))?;
        let b = base64_value(chunk[1]).ok_or(WeatherContractError::InvalidField(field))?;
        let c_padding = chunk[2] == b'=';
        let d_padding = chunk[3] == b'=';
        if c_padding && !d_padding
            || (!final_chunk && (c_padding || d_padding))
            || (c_padding && b & 0x0f != 0)
        {
            return Err(WeatherContractError::InvalidField(field));
        }
        let c = if c_padding {
            0
        } else {
            base64_value(chunk[2]).ok_or(WeatherContractError::InvalidField(field))?
        };
        if d_padding && c & 0x03 != 0 {
            return Err(WeatherContractError::InvalidField(field));
        }
        let d = if d_padding {
            0
        } else {
            base64_value(chunk[3]).ok_or(WeatherContractError::InvalidField(field))?
        };
        decoded.push((a << 2) | (b >> 4));
        if !c_padding {
            decoded.push((b << 4) | (c >> 2));
        }
        if !d_padding {
            decoded.push((c << 6) | d);
        }
        if decoded.len() > max_bytes {
            return Err(WeatherContractError::CapacityExceeded {
                field,
                max: max_bytes,
            });
        }
    }
    Ok(decoded)
}

fn base64_value(value: u8) -> Option<u8> {
    match value {
        b'A'..=b'Z' => Some(value - b'A'),
        b'a'..=b'z' => Some(value - b'a' + 26),
        b'0'..=b'9' => Some(value - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

fn validate_png_dimensions(
    png: &[u8],
    expected_width: u32,
    expected_height: u32,
) -> Result<(), WeatherContractError> {
    const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    let width = png
        .get(16..20)
        .and_then(|bytes| <[u8; 4]>::try_from(bytes).ok())
        .map(u32::from_be_bytes);
    let height = png
        .get(20..24)
        .and_then(|bytes| <[u8; 4]>::try_from(bytes).ok())
        .map(u32::from_be_bytes);
    if png.len() < 41
        || !png.starts_with(SIGNATURE)
        || png.get(12..16) != Some(b"IHDR")
        || width != Some(expected_width)
        || height != Some(expected_height)
        || png.get(png.len().saturating_sub(8)..png.len().saturating_sub(4)) != Some(b"IEND")
    {
        return Err(WeatherContractError::InvalidField("atmospheric_png"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000_000;

    fn point() -> GeoPoint {
        GeoPoint {
            latitude: 42.3601,
            longitude: -71.0589,
        }
    }

    fn attribution() -> WeatherAttribution {
        WeatherAttribution {
            provider: WeatherProvider::NationalWeatherService,
            source_id: "NWS:BOX:71:101".into(),
            label: "NOAA National Weather Service".into(),
        }
    }

    fn conditions(observed_at_ms: i64) -> CurrentConditions {
        CurrentConditions {
            observed_at_ms,
            condition: WeatherConditionKind::Clouds,
            provider_text: Some("Mostly Cloudy".into()),
            temperature: Some(Temperature {
                value: 71.0,
                unit: TemperatureUnit::Fahrenheit,
            }),
            apparent_temperature: None,
            relative_humidity_percent: Some(64.0),
            precipitation_probability_percent: None,
            wind_speed: Some(Speed {
                value: 8.0,
                unit: SpeedUnit::MilesPerHour,
            }),
            wind_direction_degrees: Some(270.0),
            wind_gust: None,
            visibility: None,
            pressure: None,
        }
    }

    fn current() -> CurrentWeatherSnapshot {
        CurrentWeatherSnapshot {
            schema_version: WEATHER_CONTRACT_SCHEMA_VERSION,
            host: "workstation-1".into(),
            location_generation: 7,
            location_point: Some(point()),
            producer_at_ms: NOW - 120_000,
            fetched_at_ms: NOW - 60_000,
            availability: WeatherAvailability::Fresh,
            conditions: Some(conditions(NOW - 300_000)),
            gaps: vec![],
            attributions: vec![attribution()],
        }
    }

    fn hour(sequence: u16, start_at_ms: i64, date: &str) -> HourlyForecastPeriod {
        HourlyForecastPeriod {
            sequence,
            start_at_ms,
            end_at_ms: start_at_ms + 3_600_000,
            local_date: date.into(),
            is_daytime: true,
            condition: WeatherConditionKind::Rain,
            provider_text: Some("Chance Rain".into()),
            temperature: None,
            precipitation_probability_percent: Some(40.0),
            relative_humidity_percent: None,
            wind_speed: None,
            wind_direction_degrees: None,
        }
    }

    fn day(date: &str) -> LocalDaySummary {
        LocalDaySummary {
            local_date: date.into(),
            condition: WeatherConditionKind::Rain,
            provider_text: None,
            high_temperature: Some(Temperature {
                value: 70.0,
                unit: TemperatureUnit::Fahrenheit,
            }),
            low_temperature: Some(Temperature {
                value: 50.0,
                unit: TemperatureUnit::Fahrenheit,
            }),
            precipitation_probability_percent: Some(40.0),
            peak_wind_speed: None,
            source_period_count: 24,
        }
    }

    fn forecast() -> WeatherForecastSnapshot {
        WeatherForecastSnapshot {
            schema_version: WEATHER_CONTRACT_SCHEMA_VERSION,
            host: "workstation-1".into(),
            location_generation: 7,
            location_point: Some(point()),
            time_zone: "America/New_York".into(),
            producer_at_ms: NOW - 1_000,
            fetched_at_ms: NOW - 500,
            availability: WeatherAvailability::Fresh,
            hourly: vec![hour(1, NOW + 1_000, "2027-01-15")],
            daily: vec![day("2027-01-15")],
            alert_references: vec!["urn:oid:2.49.0.1.840.0.test".into()],
            gaps: vec![],
            attributions: vec![attribution()],
        }
    }

    #[test]
    fn topics_caps_and_vehicle_drive_ahead_topic_are_stable() {
        assert_eq!(
            weather_current_state_topic("rig"),
            "state/weather/current/rig"
        );
        assert_eq!(
            weather_forecast_state_topic("rig"),
            "state/weather/forecast/rig"
        );
        assert_eq!(weather_map_state_topic("rig"), "state/weather/map/rig");
        assert_eq!(MAX_WEATHER_HOURLY_PERIODS, 120);
        assert_eq!(MAX_WEATHER_DAILY_SUMMARIES, 5);
        assert_eq!(
            crate::nws_forecast::NWS_FORECAST_STATE_PREFIX,
            "state/overlay/nws-hourly/"
        );
    }

    #[test]
    fn current_and_forecast_round_trip_without_filling_missing_measurements() {
        let current = current();
        let body = serde_json::to_vec(&current).expect("encode");
        let decoded = CurrentWeatherSnapshot::from_json_at(&body, NOW).expect("admit");
        assert_eq!(decoded, current);
        assert_eq!(decoded.conditions.expect("conditions").visibility, None);

        let forecast = forecast();
        let body = serde_json::to_vec(&forecast).expect("encode");
        assert_eq!(
            WeatherForecastSnapshot::from_json_at(&body, NOW).expect("admit"),
            forecast
        );
    }

    #[test]
    fn fresh_stale_expired_and_unavailable_states_are_typed() {
        let mut snapshot = current();
        snapshot.availability = WeatherAvailability::Stale {
            reason: WeatherStaleReason::RefreshFailed,
        };
        snapshot.conditions = Some(conditions(NOW - MAX_CURRENT_AGE_MS as i64 - 1));
        assert!(snapshot.validate_at(NOW).is_ok());
        snapshot.conditions = Some(conditions(NOW - MAX_LAST_GOOD_AGE_MS as i64 - 1));
        assert!(snapshot.validate_at(NOW).is_err());
        snapshot.availability = WeatherAvailability::Unavailable {
            reason: WeatherUnavailableReason::Expired,
        };
        assert!(
            snapshot.validate_at(NOW).is_err(),
            "expired data cannot remain attached"
        );
        snapshot.conditions = None;
        assert!(snapshot.validate_at(NOW).is_ok());
    }

    #[test]
    fn duplicate_unknown_and_oversized_wire_payloads_fail_closed() {
        let duplicate = r#"{"schema_version":1,"schema_version":1}"#;
        assert_eq!(
            CurrentWeatherSnapshot::from_json_at(duplicate.as_bytes(), NOW),
            Err(WeatherContractError::MalformedWire)
        );
        let mut value = serde_json::to_value(current()).expect("value");
        value["conditions"]["surprise"] = serde_json::json!(1);
        let body = serde_json::to_vec(&value).expect("encode");
        assert_eq!(
            CurrentWeatherSnapshot::from_json_at(&body, NOW),
            Err(WeatherContractError::MalformedWire)
        );
        let nested_duplicate = r#"{"schema_version":1,"host":"workstation-1","location_generation":7,"location_point":{"latitude":42.0,"longitude":-71.0},"producer_at_ms":1799999880000,"fetched_at_ms":1799999940000,"availability":{"state":"fresh"},"conditions":{"observed_at_ms":1799999700000,"condition":"clouds","temperature":{"value":71.0,"value":72.0,"unit":"fahrenheit"}},"attributions":[{"provider":"national_weather_service","source_id":"NWS:BOX","label":"NWS"}]}"#;
        assert_eq!(
            CurrentWeatherSnapshot::from_json_at(nested_duplicate.as_bytes(), NOW),
            Err(WeatherContractError::MalformedWire)
        );
        let mut unknown_unit = serde_json::to_value(current()).expect("value");
        unknown_unit["conditions"]["temperature"]["unit"] = serde_json::json!("kelvin");
        assert_eq!(
            CurrentWeatherSnapshot::from_json_at(
                &serde_json::to_vec(&unknown_unit).expect("encode"),
                NOW
            ),
            Err(WeatherContractError::MalformedWire)
        );
        let oversized = vec![b' '; MAX_WEATHER_WIRE_BYTES + 1];
        assert!(matches!(
            WeatherForecastSnapshot::from_json_at(&oversized, NOW),
            Err(WeatherContractError::BodyTooLarge { .. })
        ));
    }

    #[test]
    fn collection_caps_order_duplicates_and_daily_ranges_are_rejected() {
        let mut snapshot = forecast();
        snapshot.hourly = (0..=MAX_WEATHER_HOURLY_PERIODS)
            .map(|index| {
                hour(
                    index as u16,
                    NOW + 1_000 + index as i64 * 3_600_000,
                    "2027-01-15",
                )
            })
            .collect();
        assert!(matches!(
            snapshot.validate_at(NOW),
            Err(WeatherContractError::CapacityExceeded {
                field: "hourly",
                ..
            })
        ));

        snapshot = forecast();
        snapshot.hourly.push(snapshot.hourly[0].clone());
        assert!(snapshot.validate_at(NOW).is_err());
        snapshot = forecast();
        snapshot.daily.push(snapshot.daily[0].clone());
        assert_eq!(
            snapshot.validate_at(NOW),
            Err(WeatherContractError::Duplicate("daily_local_date"))
        );
        snapshot = forecast();
        let day = snapshot.daily.first_mut().expect("day");
        day.high_temperature.as_mut().expect("high").value = 40.0;
        assert!(snapshot.validate_at(NOW).is_err());
        snapshot = forecast();
        snapshot.hourly[0].start_at_ms = NOW + MAX_FORECAST_HORIZON_MS + 1;
        snapshot.hourly[0].end_at_ms = snapshot.hourly[0].start_at_ms + 3_600_000;
        assert!(snapshot.validate_at(NOW).is_err());
        snapshot = forecast();
        snapshot.daily[0].source_period_count = MAX_DAILY_SOURCE_PERIODS + 1;
        assert!(snapshot.validate_at(NOW).is_err());
    }

    #[test]
    fn hostile_measurements_dates_times_and_generations_fail() {
        for value in [f32::NAN, f32::INFINITY, -0.1, 100.1] {
            let mut snapshot = current();
            snapshot
                .conditions
                .as_mut()
                .expect("conditions")
                .relative_humidity_percent = Some(value);
            assert!(snapshot.validate_at(NOW).is_err(), "percent {value}");
        }
        for value in [-149.0, 141.0, f32::NEG_INFINITY] {
            let mut snapshot = current();
            snapshot
                .conditions
                .as_mut()
                .expect("conditions")
                .temperature
                .as_mut()
                .expect("temperature")
                .value = value;
            assert!(snapshot.validate_at(NOW).is_err(), "temperature {value}");
        }
        for date in ["2027-02-29", "2027-13-01", "2027-01-00", "27-01-01"] {
            let mut snapshot = forecast();
            snapshot.daily[0].local_date = date.into();
            assert!(snapshot.validate_at(NOW).is_err(), "date {date}");
        }
        let mut snapshot = forecast();
        snapshot.location_generation = 0;
        assert_eq!(
            snapshot.validate_at(NOW),
            Err(WeatherContractError::InvalidGeneration)
        );
        snapshot = forecast();
        snapshot.producer_at_ms = NOW + crate::location::MAX_WEATHER_FUTURE_SKEW_MS as i64 + 1;
        assert!(snapshot.validate_at(NOW).is_err());
    }

    #[test]
    fn percentage_and_temperature_ranges_are_property_checked() {
        for whole in 0..=100 {
            assert!(validate_percent(Some(whole as f32), "property").is_ok());
        }
        for tenths in -1000..=600 {
            let value = Temperature {
                value: tenths as f32 / 10.0,
                unit: TemperatureUnit::Celsius,
            };
            assert!(value.validate("property").is_ok());
        }
        assert!(validate_percent(Some(-f32::EPSILON), "property").is_err());
        assert!(validate_percent(Some(100.001), "property").is_err());
    }

    #[test]
    fn unavailable_weather_may_omit_location_but_available_weather_may_not() {
        let mut snapshot = current();
        snapshot.location_point = None;
        assert!(snapshot.validate_at(NOW).is_err());
        snapshot.availability = WeatherAvailability::Unavailable {
            reason: WeatherUnavailableReason::LocationUnavailable,
        };
        snapshot.conditions = None;
        assert!(snapshot.validate_at(NOW).is_ok());

        let mut forecast = forecast();
        forecast.location_point = None;
        assert!(forecast.validate_at(NOW).is_err());
        forecast.availability = WeatherAvailability::Unavailable {
            reason: WeatherUnavailableReason::LocationUnavailable,
        };
        forecast.hourly.clear();
        forecast.daily.clear();
        assert!(forecast.validate_at(NOW).is_ok());
    }

    fn atmospheric_field(kind: AtmosphericFieldKind) -> AtmosphericFieldImage {
        let (service, layer) = kind.nowcoast_product();
        AtmosphericFieldImage {
            kind,
            provider_service_path: service.into(),
            provider_layer_name: layer.into(),
            pixel_width: ATMOSPHERIC_FIELD_EDGE,
            pixel_height: ATMOSPHERIC_FIELD_EDGE,
            png_base64: "iVBORw0KGgoAAAAASUhEUgAAAQAAAAEAAAAAAAAAAAAASUVORAAAAAA=".into(),
        }
    }

    fn atmospheric() -> AtmosphericMapSnapshot {
        AtmosphericMapSnapshot {
            schema_version: WEATHER_CONTRACT_SCHEMA_VERSION,
            host: "workstation-1".into(),
            location_generation: 7,
            location_point: point(),
            viewport: AtmosphericViewport {
                generation: 7,
                zoom: 6,
                x: 19,
                y: 23,
                pixel_width: ATMOSPHERIC_FIELD_EDGE,
                pixel_height: ATMOSPHERIC_FIELD_EDGE,
            },
            rendered_at_ms: NOW - 60_000,
            fetched_at_ms: NOW - 60_000,
            availability: WeatherAvailability::Fresh,
            fields: vec![
                atmospheric_field(AtmosphericFieldKind::Temperature),
                atmospheric_field(AtmosphericFieldKind::Wind),
                atmospheric_field(AtmosphericFieldKind::CloudCover),
            ],
            gaps: vec![],
            attributions: vec![attribution()],
        }
    }

    fn viewport_action() -> SetWeatherMapViewportRequest {
        SetWeatherMapViewportRequest {
            schema_version: WEATHER_CONTRACT_SCHEMA_VERSION,
            request_id: "viewport-8".into(),
            target_host: "workstation-1".into(),
            expected_location_generation: 7,
            viewport: AtmosphericViewport {
                generation: 8,
                zoom: 7,
                x: 38,
                y: 47,
                pixel_width: ATMOSPHERIC_FIELD_EDGE,
                pixel_height: ATMOSPHERIC_FIELD_EDGE,
            },
            issued_at_ms: NOW - 1_000,
        }
    }

    #[test]
    fn viewport_action_and_state_are_bounded_latest_wins_contracts() {
        assert_eq!(
            weather_set_map_viewport_topic("workstation-1"),
            "action/weather/set-map-viewport/workstation-1"
        );
        assert_eq!(
            weather_map_viewport_state_topic("workstation-1"),
            "state/weather/map-viewport/workstation-1"
        );
        let action = viewport_action();
        let body = serde_json::to_vec(&action).expect("encode");
        assert_eq!(
            SetWeatherMapViewportRequest::from_json_at(&body, NOW).expect("admit"),
            action
        );
        let state = WeatherMapViewportState {
            schema_version: WEATHER_CONTRACT_SCHEMA_VERSION,
            host: action.target_host.clone(),
            location_generation: action.expected_location_generation,
            viewport: action.viewport,
            source: WeatherMapViewportSource::MapsAction,
            admitted_at_ms: NOW,
        };
        let body = serde_json::to_vec(&state).expect("encode");
        assert_eq!(
            WeatherMapViewportState::from_json_at(&body, NOW).expect("admit"),
            state
        );
    }

    #[test]
    fn viewport_action_rejects_hostile_zoom_dimensions_bounds_and_generation() {
        let mut action = viewport_action();
        action.viewport.zoom = 13;
        assert!(action.validate_at(NOW).is_err());
        action = viewport_action();
        action.viewport.pixel_width = ATMOSPHERIC_FIELD_EDGE + 1;
        assert!(action.validate_at(NOW).is_err());
        action = viewport_action();
        action.viewport.x = 1_u32 << action.viewport.zoom;
        assert!(action.validate_at(NOW).is_err());
        action = viewport_action();
        action.expected_location_generation = 0;
        assert!(action.validate_at(NOW).is_err());
        let oversized = vec![b' '; MAX_WEATHER_VIEWPORT_WIRE_BYTES + 1];
        assert!(matches!(
            SetWeatherMapViewportRequest::from_json_at(&oversized, NOW),
            Err(WeatherContractError::BodyTooLarge { .. })
        ));
    }

    #[test]
    fn atmospheric_snapshot_admits_exact_fields_and_typed_age_transitions() {
        let snapshot = atmospheric();
        let body = serde_json::to_vec(&snapshot).expect("encode");
        assert_eq!(
            AtmosphericMapSnapshot::from_json_at(&body, NOW).expect("admit"),
            snapshot
        );

        let mut stale = snapshot.clone();
        stale.rendered_at_ms = NOW - MAX_ATMOSPHERIC_FRESH_AGE_MS - 1;
        stale.fetched_at_ms = stale.rendered_at_ms;
        stale.availability = WeatherAvailability::Stale {
            reason: WeatherStaleReason::ProviderBackoff,
        };
        assert!(stale.validate_at(NOW).is_ok());

        stale.rendered_at_ms = NOW - MAX_ATMOSPHERIC_CACHE_AGE_MS - 1;
        stale.fetched_at_ms = stale.rendered_at_ms;
        assert!(stale.validate_at(NOW).is_err());
        stale.availability = WeatherAvailability::Unavailable {
            reason: WeatherUnavailableReason::Expired,
        };
        stale.fields.clear();
        assert!(stale.validate_at(NOW).is_ok());
    }

    #[test]
    fn atmospheric_snapshot_rejects_product_drift_duplicates_png_and_viewport() {
        let mut snapshot = atmospheric();
        snapshot.fields[0].provider_layer_name = NOWCOAST_NDFD_SKY_LAYER.into();
        assert!(snapshot.validate_at(NOW).is_err());
        snapshot = atmospheric();
        snapshot.fields[0].provider_service_path = NOWCOAST_NDFD_SKY_PATH.into();
        assert!(snapshot.validate_at(NOW).is_err());
        snapshot = atmospheric();
        snapshot.fields[1] = snapshot.fields[0].clone();
        assert!(snapshot.validate_at(NOW).is_err());
        snapshot = atmospheric();
        snapshot.fields[0].png_base64 = "bm90IGEgcG5n".into();
        assert!(snapshot.validate_at(NOW).is_err());
        snapshot = atmospheric();
        snapshot.viewport.x = 1_u32 << snapshot.viewport.zoom;
        assert!(snapshot.validate_at(NOW).is_err());

        let oversized = vec![b' '; MAX_ATMOSPHERIC_MAP_WIRE_BYTES + 1];
        assert!(matches!(
            AtmosphericMapSnapshot::from_json_at(&oversized, NOW),
            Err(WeatherContractError::BodyTooLarge { .. })
        ));
    }
}
