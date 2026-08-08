//! Bounded weather-location preference and effective-location contracts.
//!
//! This module deliberately describes authority state without implementing the
//! daemon resolver or persistence. `Auto` may resolve only from a fresh,
//! same-host live GNSS observation or a previously verified saved place;
//! `Manual` names one verified place. Consumers correlate every weather
//! projection with the effective-location generation.

#![allow(
    missing_docs,
    reason = "public fields and closed variants form the documented v1 wire contract"
)]
#![allow(
    clippy::missing_errors_doc,
    reason = "WeatherContractError is the closed error vocabulary for every admission helper"
)]

use crate::nws_alert::GeoPoint;
use serde::{Deserialize, Serialize};
use std::fmt;

pub const WEATHER_LOCATION_SCHEMA_VERSION: u16 = 1;
pub const WEATHER_SET_LOCATION_TOPIC: &str = "action/weather/set-location";
pub const WEATHER_LOCATION_STATE_PREFIX: &str = "state/weather/location/";
pub const MAX_WEATHER_LOCATION_WIRE_BYTES: usize = 64 * 1024;
pub const MAX_WEATHER_ID_BYTES: usize = 128;
pub const MAX_WEATHER_LABEL_BYTES: usize = 256;
pub const MAX_WEATHER_TIME_ZONE_BYTES: usize = 64;
pub const MAX_WEATHER_REASON_BYTES: usize = 512;
pub const MAX_WEATHER_FUTURE_SKEW_MS: i64 = 5 * 60 * 1_000;
pub const MAX_LIVE_GNSS_AGE_MS: i64 = 5 * 60 * 1_000;

#[must_use]
pub fn weather_location_state_topic(host: &str) -> String {
    format!("{WEATHER_LOCATION_STATE_PREFIX}{host}")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WeatherContractError {
    BodyTooLarge { bytes: usize, max: usize },
    MalformedWire,
    UnsupportedSchema { found: u16 },
    InvalidField(&'static str),
    FieldTooLong { field: &'static str, max: usize },
    CapacityExceeded { field: &'static str, max: usize },
    InvalidCoordinate,
    InvalidGeneration,
    InvalidTimestamp(&'static str),
    FutureTimestamp(&'static str),
    InvalidRelationship(&'static str),
    InvalidMeasurement(&'static str),
    Duplicate(&'static str),
}

impl fmt::Display for WeatherContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BodyTooLarge { bytes, max } => {
                write!(formatter, "weather body is {bytes} bytes; maximum is {max}")
            }
            Self::MalformedWire => formatter.write_str("malformed weather contract body"),
            Self::UnsupportedSchema { found } => {
                write!(formatter, "unsupported weather schema version {found}")
            }
            Self::InvalidField(field) => write!(formatter, "invalid weather field {field}"),
            Self::FieldTooLong { field, max } => {
                write!(formatter, "weather field {field} exceeds {max} bytes")
            }
            Self::CapacityExceeded { field, max } => {
                write!(formatter, "weather collection {field} exceeds {max}")
            }
            Self::InvalidCoordinate => formatter.write_str("invalid weather coordinate"),
            Self::InvalidGeneration => formatter.write_str("invalid weather generation"),
            Self::InvalidTimestamp(field) => {
                write!(formatter, "invalid weather timestamp {field}")
            }
            Self::FutureTimestamp(field) => {
                write!(formatter, "future weather timestamp {field}")
            }
            Self::InvalidRelationship(field) => {
                write!(formatter, "invalid weather relationship {field}")
            }
            Self::InvalidMeasurement(field) => {
                write!(formatter, "invalid weather measurement {field}")
            }
            Self::Duplicate(field) => write!(formatter, "duplicate weather value in {field}"),
        }
    }
}

impl std::error::Error for WeatherContractError {}

pub(crate) fn decode_json<T>(body: &[u8], max: usize) -> Result<T, WeatherContractError>
where
    T: serde::de::DeserializeOwned,
{
    if body.len() > max {
        return Err(WeatherContractError::BodyTooLarge {
            bytes: body.len(),
            max,
        });
    }
    let text = std::str::from_utf8(body).map_err(|_| WeatherContractError::MalformedWire)?;
    crate::workloads::reject_duplicate_json_keys(text)
        .map_err(|_| WeatherContractError::MalformedWire)?;
    serde_json::from_str(text).map_err(|_| WeatherContractError::MalformedWire)
}

pub(crate) const fn validate_schema(schema_version: u16) -> Result<(), WeatherContractError> {
    if schema_version != WEATHER_LOCATION_SCHEMA_VERSION {
        return Err(WeatherContractError::UnsupportedSchema {
            found: schema_version,
        });
    }
    Ok(())
}

pub(crate) fn validate_id(value: &str, field: &'static str) -> Result<(), WeatherContractError> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return Err(WeatherContractError::InvalidField(field));
    }
    validate_len(value, field, MAX_WEATHER_ID_BYTES)
}

pub(crate) fn validate_text(
    value: &str,
    field: &'static str,
    max: usize,
) -> Result<(), WeatherContractError> {
    if value.trim().is_empty() || value.chars().any(char::is_control) {
        return Err(WeatherContractError::InvalidField(field));
    }
    validate_len(value, field, max)
}

pub(crate) const fn validate_len(
    value: &str,
    field: &'static str,
    max: usize,
) -> Result<(), WeatherContractError> {
    if value.len() > max {
        return Err(WeatherContractError::FieldTooLong { field, max });
    }
    Ok(())
}

pub(crate) fn validate_point(point: &GeoPoint) -> Result<(), WeatherContractError> {
    if !point.latitude.is_finite()
        || !(-90.0..=90.0).contains(&point.latitude)
        || !point.longitude.is_finite()
        || !(-180.0..=180.0).contains(&point.longitude)
    {
        return Err(WeatherContractError::InvalidCoordinate);
    }
    Ok(())
}

pub(crate) const fn validate_not_future(
    timestamp_ms: i64,
    now_ms: i64,
    field: &'static str,
) -> Result<(), WeatherContractError> {
    if timestamp_ms <= 0 || now_ms <= 0 {
        return Err(WeatherContractError::InvalidTimestamp(field));
    }
    let future_limit = now_ms.saturating_add(MAX_WEATHER_FUTURE_SKEW_MS);
    if timestamp_ms > future_limit {
        return Err(WeatherContractError::FutureTimestamp(field));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WeatherLocationMode {
    Auto,
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WeatherCoverage {
    NwsUnitedStates,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifiedPlace {
    pub place_id: String,
    pub label: String,
    pub point: GeoPoint,
    pub time_zone: String,
    pub coverage: WeatherCoverage,
    pub verified_at_ms: i64,
}

impl VerifiedPlace {
    pub fn validate_at(&self, now_ms: i64) -> Result<(), WeatherContractError> {
        validate_id(&self.place_id, "place_id")?;
        validate_text(&self.label, "place_label", MAX_WEATHER_LABEL_BYTES)?;
        validate_point(&self.point)?;
        validate_time_zone(&self.time_zone)?;
        validate_not_future(self.verified_at_ms, now_ms, "verified_at_ms")
    }
}

fn validate_time_zone(value: &str) -> Result<(), WeatherContractError> {
    validate_len(value, "time_zone", MAX_WEATHER_TIME_ZONE_BYTES)?;
    let mut parts = value.split('/');
    let first = parts.next().unwrap_or_default();
    let second = parts.next().unwrap_or_default();
    if first.is_empty()
        || second.is_empty()
        || parts.any(str::is_empty)
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'_' | b'-' | b'+'))
    {
        return Err(WeatherContractError::InvalidField("time_zone"));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WeatherLocationPreference {
    pub schema_version: u16,
    pub generation: u64,
    pub mode: WeatherLocationMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_place: Option<VerifiedPlace>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved_auto_place: Option<VerifiedPlace>,
    pub updated_at_ms: i64,
}

impl WeatherLocationPreference {
    pub fn validate_at(&self, now_ms: i64) -> Result<(), WeatherContractError> {
        validate_schema(self.schema_version)?;
        if self.generation == 0 {
            return Err(WeatherContractError::InvalidGeneration);
        }
        validate_not_future(self.updated_at_ms, now_ms, "updated_at_ms")?;
        match (self.mode, &self.manual_place) {
            (WeatherLocationMode::Auto, None) | (WeatherLocationMode::Manual, Some(_)) => {}
            _ => {
                return Err(WeatherContractError::InvalidRelationship(
                    "mode_manual_place",
                ));
            }
        }
        if let Some(place) = &self.manual_place {
            place.validate_at(now_ms)?;
        }
        if let Some(place) = &self.saved_auto_place {
            place.validate_at(now_ms)?;
        }
        Ok(())
    }

    pub fn from_json_at(body: &[u8], now_ms: i64) -> Result<Self, WeatherContractError> {
        let value: Self = decode_json(body, MAX_WEATHER_LOCATION_WIRE_BYTES)?;
        value.validate_at(now_ms)?;
        Ok(value)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetWeatherLocationRequest {
    pub schema_version: u16,
    pub request_id: String,
    pub expected_generation: u64,
    pub issued_at_ms: i64,
    pub mode: WeatherLocationMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_place: Option<VerifiedPlace>,
}

impl SetWeatherLocationRequest {
    pub fn validate_at(&self, now_ms: i64) -> Result<(), WeatherContractError> {
        validate_schema(self.schema_version)?;
        validate_id(&self.request_id, "request_id")?;
        if self.expected_generation == u64::MAX {
            return Err(WeatherContractError::InvalidGeneration);
        }
        validate_not_future(self.issued_at_ms, now_ms, "issued_at_ms")?;
        match (self.mode, &self.manual_place) {
            (WeatherLocationMode::Auto, None) => Ok(()),
            (WeatherLocationMode::Manual, Some(place)) => place.validate_at(now_ms),
            _ => Err(WeatherContractError::InvalidRelationship(
                "mode_manual_place",
            )),
        }
    }

    pub fn from_json_at(body: &[u8], now_ms: i64) -> Result<Self, WeatherContractError> {
        let value: Self = decode_json(body, MAX_WEATHER_LOCATION_WIRE_BYTES)?;
        value.validate_at(now_ms)?;
        Ok(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EffectiveLocationProvenance {
    LiveGnss {
        source_host: String,
        source_id: String,
    },
    SavedVerifiedPlace {
        place_id: String,
    },
    ManualVerifiedPlace {
        place_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectiveWeatherLocation {
    pub label: String,
    pub point: GeoPoint,
    pub time_zone: String,
    pub coverage: WeatherCoverage,
    pub provenance: EffectiveLocationProvenance,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_observed_at_ms: Option<i64>,
}

impl EffectiveWeatherLocation {
    fn validate_at(
        &self,
        projection_host: &str,
        produced_at_ms: i64,
        now_ms: i64,
        allow_stale_live_fix: bool,
    ) -> Result<(), WeatherContractError> {
        validate_text(&self.label, "location_label", MAX_WEATHER_LABEL_BYTES)?;
        validate_point(&self.point)?;
        validate_time_zone(&self.time_zone)?;
        match (&self.provenance, self.source_observed_at_ms) {
            (
                EffectiveLocationProvenance::LiveGnss {
                    source_host,
                    source_id,
                },
                Some(observed_at_ms),
            ) => {
                validate_id(source_host, "source_host")?;
                validate_id(source_id, "source_id")?;
                if source_host != projection_host {
                    return Err(WeatherContractError::InvalidRelationship(
                        "same_host_live_gnss",
                    ));
                }
                validate_not_future(observed_at_ms, now_ms, "source_observed_at_ms")?;
                if observed_at_ms > produced_at_ms {
                    return Err(WeatherContractError::InvalidTimestamp(
                        "source_observed_at_ms",
                    ));
                }
                let age = produced_at_ms.saturating_sub(observed_at_ms);
                if !allow_stale_live_fix && age > MAX_LIVE_GNSS_AGE_MS {
                    return Err(WeatherContractError::InvalidRelationship("stale_live_gnss"));
                }
            }
            (EffectiveLocationProvenance::LiveGnss { .. }, None) => {
                return Err(WeatherContractError::InvalidRelationship(
                    "live_gnss_observed_at",
                ));
            }
            (
                EffectiveLocationProvenance::SavedVerifiedPlace { place_id }
                | EffectiveLocationProvenance::ManualVerifiedPlace { place_id },
                None,
            ) => {
                validate_id(place_id, "place_id")?;
            }
            (_, Some(_)) => {
                return Err(WeatherContractError::InvalidRelationship(
                    "static_place_observed_at",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocationUnavailableReason {
    NoFreshLocalFix,
    NoVerifiedFallback,
    UnsupportedCoverage,
    SourceFailed,
    PreferenceInvalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocationStaleReason {
    FixExpired,
    SourcePaused,
    SourceFailed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum EffectiveLocationState {
    Available {
        location: EffectiveWeatherLocation,
    },
    Stale {
        location: EffectiveWeatherLocation,
        stale_since_ms: i64,
        reason: LocationStaleReason,
    },
    Unavailable {
        reason: LocationUnavailableReason,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectiveLocationSnapshot {
    pub schema_version: u16,
    pub host: String,
    pub generation: u64,
    pub mode: WeatherLocationMode,
    pub produced_at_ms: i64,
    pub state: EffectiveLocationState,
}

impl EffectiveLocationSnapshot {
    pub fn validate_at(&self, now_ms: i64) -> Result<(), WeatherContractError> {
        validate_schema(self.schema_version)?;
        validate_id(&self.host, "host")?;
        if self.generation == 0 {
            return Err(WeatherContractError::InvalidGeneration);
        }
        validate_not_future(self.produced_at_ms, now_ms, "produced_at_ms")?;
        match &self.state {
            EffectiveLocationState::Available { location } => {
                location.validate_at(&self.host, self.produced_at_ms, now_ms, false)?;
                validate_mode_provenance(self.mode, &location.provenance)
            }
            EffectiveLocationState::Stale {
                location,
                stale_since_ms,
                ..
            } => {
                validate_not_future(*stale_since_ms, now_ms, "stale_since_ms")?;
                if *stale_since_ms > self.produced_at_ms {
                    return Err(WeatherContractError::InvalidTimestamp("stale_since_ms"));
                }
                location.validate_at(&self.host, self.produced_at_ms, now_ms, true)?;
                if location
                    .source_observed_at_ms
                    .is_some_and(|observed_at_ms| *stale_since_ms < observed_at_ms)
                {
                    return Err(WeatherContractError::InvalidTimestamp("stale_since_ms"));
                }
                validate_mode_provenance(self.mode, &location.provenance)
            }
            EffectiveLocationState::Unavailable { .. } => Ok(()),
        }
    }

    pub fn from_json_at(body: &[u8], now_ms: i64) -> Result<Self, WeatherContractError> {
        let value: Self = decode_json(body, MAX_WEATHER_LOCATION_WIRE_BYTES)?;
        value.validate_at(now_ms)?;
        Ok(value)
    }
}

fn validate_mode_provenance(
    mode: WeatherLocationMode,
    provenance: &EffectiveLocationProvenance,
) -> Result<(), WeatherContractError> {
    let admitted = matches!(
        (mode, provenance),
        (
            WeatherLocationMode::Auto,
            EffectiveLocationProvenance::LiveGnss { .. }
                | EffectiveLocationProvenance::SavedVerifiedPlace { .. }
        ) | (
            WeatherLocationMode::Manual,
            EffectiveLocationProvenance::ManualVerifiedPlace { .. }
        )
    );
    admitted
        .then_some(())
        .ok_or(WeatherContractError::InvalidRelationship("mode_provenance"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000_000;

    fn place() -> VerifiedPlace {
        VerifiedPlace {
            place_id: "gazetteer:us:bos".into(),
            label: "Boston, Massachusetts".into(),
            point: GeoPoint {
                latitude: 42.3601,
                longitude: -71.0589,
            },
            time_zone: "America/New_York".into(),
            coverage: WeatherCoverage::NwsUnitedStates,
            verified_at_ms: NOW - 1_000,
        }
    }

    fn manual_request() -> SetWeatherLocationRequest {
        SetWeatherLocationRequest {
            schema_version: WEATHER_LOCATION_SCHEMA_VERSION,
            request_id: "01JTESTREQUEST".into(),
            expected_generation: 4,
            issued_at_ms: NOW - 100,
            mode: WeatherLocationMode::Manual,
            manual_place: Some(place()),
        }
    }

    fn live_snapshot() -> EffectiveLocationSnapshot {
        EffectiveLocationSnapshot {
            schema_version: WEATHER_LOCATION_SCHEMA_VERSION,
            host: "workstation-1".into(),
            generation: 5,
            mode: WeatherLocationMode::Auto,
            produced_at_ms: NOW - 10,
            state: EffectiveLocationState::Available {
                location: EffectiveWeatherLocation {
                    label: "Current location".into(),
                    point: place().point,
                    time_zone: "America/New_York".into(),
                    coverage: WeatherCoverage::NwsUnitedStates,
                    provenance: EffectiveLocationProvenance::LiveGnss {
                        source_host: "workstation-1".into(),
                        source_id: "mg90:ABC123:gnss".into(),
                    },
                    source_observed_at_ms: Some(NOW - 1_000),
                },
            },
        }
    }

    #[test]
    fn topics_and_schema_are_stable_without_changing_vehicle_forecast() {
        assert_eq!(WEATHER_SET_LOCATION_TOPIC, "action/weather/set-location");
        assert_eq!(
            weather_location_state_topic("rig-1"),
            "state/weather/location/rig-1"
        );
        assert_eq!(
            crate::nws_forecast::nws_forecast_state_topic("rig-1"),
            "state/overlay/nws-hourly/rig-1"
        );
        assert_eq!(WEATHER_LOCATION_SCHEMA_VERSION, 1);
    }

    #[test]
    fn preference_action_and_effective_location_round_trip() {
        let request = manual_request();
        let body = serde_json::to_vec(&request).expect("encode");
        assert_eq!(
            SetWeatherLocationRequest::from_json_at(&body, NOW).expect("admit"),
            request
        );
        let snapshot = live_snapshot();
        let body = serde_json::to_vec(&snapshot).expect("encode");
        assert_eq!(
            EffectiveLocationSnapshot::from_json_at(&body, NOW).expect("admit"),
            snapshot
        );
    }

    #[test]
    fn duplicate_and_unknown_keys_fail_closed_at_every_depth() {
        let mut value = serde_json::to_value(manual_request()).expect("value");
        value["unknown"] = serde_json::json!(true);
        assert_eq!(
            SetWeatherLocationRequest::from_json_at(
                serde_json::to_string(&value).expect("encode").as_bytes(),
                NOW
            ),
            Err(WeatherContractError::MalformedWire)
        );

        let duplicate_top = r#"{"schema_version":1,"schema_version":1,"request_id":"r","expected_generation":0,"issued_at_ms":1799999999999,"mode":"auto"}"#;
        assert_eq!(
            SetWeatherLocationRequest::from_json_at(duplicate_top.as_bytes(), NOW),
            Err(WeatherContractError::MalformedWire)
        );
        let duplicate_nested = r#"{"schema_version":1,"request_id":"r","expected_generation":0,"issued_at_ms":1799999999999,"mode":"manual","manual_place":{"place_id":"p","label":"P","point":{"latitude":1.0,"latitude":2.0,"longitude":3.0},"time_zone":"America/New_York","coverage":"nws_united_states","verified_at_ms":1799999999999}}"#;
        assert_eq!(
            SetWeatherLocationRequest::from_json_at(duplicate_nested.as_bytes(), NOW),
            Err(WeatherContractError::MalformedWire)
        );
    }

    #[test]
    fn mode_place_and_mode_provenance_relationships_are_closed() {
        let mut request = manual_request();
        request.mode = WeatherLocationMode::Auto;
        assert_eq!(
            request.validate_at(NOW),
            Err(WeatherContractError::InvalidRelationship(
                "mode_manual_place"
            ))
        );
        let mut snapshot = live_snapshot();
        snapshot.mode = WeatherLocationMode::Manual;
        assert_eq!(
            snapshot.validate_at(NOW),
            Err(WeatherContractError::InvalidRelationship("mode_provenance"))
        );
    }

    #[test]
    fn wrong_host_stale_and_future_live_fixes_are_rejected() {
        let cases = [
            ("wrong_host", NOW - 1_000),
            ("workstation-1", NOW - MAX_LIVE_GNSS_AGE_MS - 20),
            ("workstation-1", NOW + MAX_WEATHER_FUTURE_SKEW_MS + 1),
        ];
        for (host, observed_at) in cases {
            let mut snapshot = live_snapshot();
            let EffectiveLocationState::Available { location } = &mut snapshot.state else {
                unreachable!();
            };
            let EffectiveLocationProvenance::LiveGnss { source_host, .. } =
                &mut location.provenance
            else {
                unreachable!();
            };
            *source_host = host.into();
            location.source_observed_at_ms = Some(observed_at);
            assert!(
                snapshot.validate_at(NOW).is_err(),
                "host={host} time={observed_at}"
            );
        }
    }

    #[test]
    fn coordinate_label_timezone_version_generation_and_wire_bounds_are_hostile() {
        for latitude in [f64::NAN, f64::INFINITY, -90.01, 90.01] {
            let mut request = manual_request();
            request.manual_place.as_mut().expect("place").point.latitude = latitude;
            assert_eq!(
                request.validate_at(NOW),
                Err(WeatherContractError::InvalidCoordinate)
            );
        }
        for longitude in [f64::NEG_INFINITY, -180.01, 180.01] {
            let mut request = manual_request();
            request
                .manual_place
                .as_mut()
                .expect("place")
                .point
                .longitude = longitude;
            assert_eq!(
                request.validate_at(NOW),
                Err(WeatherContractError::InvalidCoordinate)
            );
        }
        let mut request = manual_request();
        request.schema_version = 99;
        assert!(matches!(
            request.validate_at(NOW),
            Err(WeatherContractError::UnsupportedSchema { found: 99 })
        ));
        request = manual_request();
        request.manual_place.as_mut().expect("place").label = "bad\nlabel".into();
        assert!(request.validate_at(NOW).is_err());
        request = manual_request();
        request.manual_place.as_mut().expect("place").time_zone = "UTC".into();
        assert!(request.validate_at(NOW).is_err());
        request = manual_request();
        request.expected_generation = u64::MAX;
        assert_eq!(
            request.validate_at(NOW),
            Err(WeatherContractError::InvalidGeneration)
        );
        let oversized = vec![b' '; MAX_WEATHER_LOCATION_WIRE_BYTES + 1];
        assert!(matches!(
            SetWeatherLocationRequest::from_json_at(&oversized, NOW),
            Err(WeatherContractError::BodyTooLarge { .. })
        ));
    }

    #[test]
    fn coordinate_boundary_grid_is_property_checked() {
        for lat_step in -180..=180 {
            let latitude = f64::from(lat_step) / 2.0;
            for lon_step in (-360..=360).step_by(9) {
                let point = GeoPoint {
                    latitude,
                    longitude: f64::from(lon_step) / 2.0,
                };
                assert!(
                    validate_point(&point).is_ok(),
                    "{latitude},{}",
                    point.longitude
                );
            }
        }
    }
}
