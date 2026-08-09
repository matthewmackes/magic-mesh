//! WL-FUNC-017 S6 — bounded route and navigation wire contracts.

#![allow(missing_docs, reason = "closed v1 wire fields are self-describing")]

use crate::location::{
    decode_json, validate_id, validate_not_future, validate_point, validate_text,
    WeatherContractError, MAX_WEATHER_ID_BYTES, MAX_WEATHER_LABEL_BYTES,
};
use crate::nws_alert::GeoPoint;
use serde::{Deserialize, Serialize};

pub const NAVIGATION_SCHEMA_VERSION: u16 = 1;
pub const NAVIGATION_ROUTE_ACTION_PREFIX: &str = "action/navigation/route/";
pub const NAVIGATION_CANCEL_ACTION_PREFIX: &str = "action/navigation/cancel/";
pub const NAVIGATION_PROGRESS_ACTION_PREFIX: &str = "action/navigation/progress/";
pub const NAVIGATION_STATE_PREFIX: &str = "state/navigation/";
pub const MAX_NAVIGATION_WIRE_BYTES: usize = 256 * 1024;
pub const MAX_ROUTE_REQUEST_BYTES: usize = 16 * 1024;
pub const MAX_ROUTE_MANEUVERS: usize = 512;
pub const MAX_ROUTE_GEOMETRY_POINTS: usize = 8_192;
pub const MAX_ROUTE_DISTANCE_METRES: u64 = 20_000_000;
pub const MAX_ROUTE_DURATION_SECONDS: u64 = 14 * 24 * 60 * 60;

#[must_use]
pub fn navigation_route_action_topic(host: &str) -> String {
    format!("{NAVIGATION_ROUTE_ACTION_PREFIX}{host}")
}

#[must_use]
pub fn navigation_cancel_action_topic(host: &str) -> String {
    format!("{NAVIGATION_CANCEL_ACTION_PREFIX}{host}")
}

#[must_use]
pub fn navigation_progress_action_topic(host: &str) -> String {
    format!("{NAVIGATION_PROGRESS_ACTION_PREFIX}{host}")
}

#[must_use]
pub fn navigation_state_topic(host: &str) -> String {
    format!("{NAVIGATION_STATE_PREFIX}{host}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteProfile {
    Car,
    Bicycle,
    Walking,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteRequestKind {
    Route,
    Reroute,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteEndpoint {
    pub label: String,
    pub point: GeoPoint,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteRequest {
    pub schema_version: u16,
    pub request_id: String,
    pub host: String,
    pub expected_generation: u64,
    pub issued_at_ms: i64,
    pub kind: RouteRequestKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaces_route_id: Option<String>,
    pub profile: RouteProfile,
    pub origin: RouteEndpoint,
    pub destination: RouteEndpoint,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelNavigationRequest {
    pub schema_version: u16,
    pub request_id: String,
    pub host: String,
    pub expected_generation: u64,
    pub issued_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NavigationProgressRequest {
    pub schema_version: u16,
    pub request_id: String,
    pub host: String,
    pub expected_generation: u64,
    pub issued_at_ms: i64,
    pub route_id: String,
    pub progress: NavigationProgress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManeuverKind {
    Depart,
    Continue,
    TurnLeft,
    TurnRight,
    Merge,
    Roundabout,
    Arrive,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteManeuver {
    pub sequence: u16,
    pub kind: ManeuverKind,
    pub instruction: String,
    pub point: GeoPoint,
    pub distance_metres: u32,
    pub duration_seconds: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteAttribution {
    pub provider_id: String,
    pub label: String,
    pub data_revision: String,
    pub offline: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteResult {
    pub route_id: String,
    pub request_id: String,
    pub calculated_at_ms: i64,
    pub distance_metres: u64,
    pub duration_seconds: u64,
    pub geometry: Vec<GeoPoint>,
    pub maneuvers: Vec<RouteManeuver>,
    pub attribution: RouteAttribution,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NavigationProgress {
    pub route_id: String,
    pub position: GeoPoint,
    pub observed_at_ms: i64,
    pub maneuver_index: u16,
    pub distance_remaining_metres: u64,
    pub duration_remaining_seconds: u64,
    pub off_route: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NavigationUnavailableReason {
    ProviderNotConfigured,
    ProviderUnavailable,
    InterruptedByRestart,
    InvalidRequest,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum NavigationPhase {
    Idle,
    Calculating {
        request_id: String,
        reroute: bool,
    },
    Active {
        route: RouteResult,
        progress: NavigationProgress,
    },
    Cancelled {
        request_id: String,
        cancelled_at_ms: i64,
    },
    Unavailable {
        request_id: Option<String>,
        reason: NavigationUnavailableReason,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NavigationSnapshot {
    pub schema_version: u16,
    pub host: String,
    pub generation: u64,
    pub produced_at_ms: i64,
    pub phase: NavigationPhase,
}

impl RouteRequest {
    pub fn from_json_at(bytes: &[u8], now_ms: i64) -> Result<Self, WeatherContractError> {
        let value: Self = decode_json(bytes, MAX_ROUTE_REQUEST_BYTES)?;
        value.validate_at(now_ms)?;
        Ok(value)
    }

    pub fn validate_at(&self, now_ms: i64) -> Result<(), WeatherContractError> {
        validate_version(self.schema_version)?;
        validate_id(&self.request_id, "request_id")?;
        validate_id(&self.host, "host")?;
        validate_not_future(self.issued_at_ms, now_ms, "issued_at_ms")?;
        validate_endpoint(&self.origin, "origin")?;
        validate_endpoint(&self.destination, "destination")?;
        match (self.kind, &self.replaces_route_id) {
            (RouteRequestKind::Route, None) => {}
            (RouteRequestKind::Reroute, Some(route_id)) => {
                validate_id(route_id, "replaces_route_id")?
            }
            _ => return Err(WeatherContractError::InvalidField("replaces_route_id")),
        }
        Ok(())
    }
}

impl CancelNavigationRequest {
    pub fn from_json_at(bytes: &[u8], now_ms: i64) -> Result<Self, WeatherContractError> {
        let value: Self = decode_json(bytes, MAX_ROUTE_REQUEST_BYTES)?;
        value.validate_at(now_ms)?;
        Ok(value)
    }

    pub fn validate_at(&self, now_ms: i64) -> Result<(), WeatherContractError> {
        validate_version(self.schema_version)?;
        validate_id(&self.request_id, "request_id")?;
        validate_id(&self.host, "host")?;
        validate_not_future(self.issued_at_ms, now_ms, "issued_at_ms")
    }
}

impl NavigationProgressRequest {
    pub fn from_json_at(bytes: &[u8], now_ms: i64) -> Result<Self, WeatherContractError> {
        let value: Self = decode_json(bytes, MAX_ROUTE_REQUEST_BYTES)?;
        validate_version(value.schema_version)?;
        validate_id(&value.request_id, "request_id")?;
        validate_id(&value.host, "host")?;
        validate_id(&value.route_id, "route_id")?;
        validate_not_future(value.issued_at_ms, now_ms, "issued_at_ms")?;
        if value.route_id != value.progress.route_id {
            return Err(WeatherContractError::InvalidRelationship(
                "progress.route_id",
            ));
        }
        Ok(value)
    }
}

impl RouteResult {
    pub fn validate_at(&self, now_ms: i64) -> Result<(), WeatherContractError> {
        validate_id(&self.route_id, "route_id")?;
        validate_id(&self.request_id, "request_id")?;
        validate_not_future(self.calculated_at_ms, now_ms, "calculated_at_ms")?;
        if self.distance_metres == 0 || self.distance_metres > MAX_ROUTE_DISTANCE_METRES {
            return Err(WeatherContractError::InvalidField("distance_metres"));
        }
        if self.duration_seconds == 0 || self.duration_seconds > MAX_ROUTE_DURATION_SECONDS {
            return Err(WeatherContractError::InvalidField("duration_seconds"));
        }
        if self.geometry.len() < 2 || self.geometry.len() > MAX_ROUTE_GEOMETRY_POINTS {
            return Err(WeatherContractError::CapacityExceeded {
                field: "geometry",
                max: MAX_ROUTE_GEOMETRY_POINTS,
            });
        }
        if self.maneuvers.is_empty() || self.maneuvers.len() > MAX_ROUTE_MANEUVERS {
            return Err(WeatherContractError::CapacityExceeded {
                field: "maneuvers",
                max: MAX_ROUTE_MANEUVERS,
            });
        }
        for point in &self.geometry {
            validate_point(point)?;
        }
        for (index, maneuver) in self.maneuvers.iter().enumerate() {
            if usize::from(maneuver.sequence) != index {
                return Err(WeatherContractError::InvalidField("maneuver.sequence"));
            }
            validate_text(
                &maneuver.instruction,
                "maneuver.instruction",
                MAX_WEATHER_LABEL_BYTES,
            )?;
            validate_point(&maneuver.point)?;
        }
        validate_id(&self.attribution.provider_id, "attribution.provider_id")?;
        validate_text(
            &self.attribution.label,
            "attribution.label",
            MAX_WEATHER_LABEL_BYTES,
        )?;
        validate_text(
            &self.attribution.data_revision,
            "attribution.data_revision",
            MAX_WEATHER_ID_BYTES,
        )?;
        Ok(())
    }
}

impl NavigationProgress {
    pub fn validate_for(
        &self,
        route: &RouteResult,
        now_ms: i64,
    ) -> Result<(), WeatherContractError> {
        validate_id(&self.route_id, "progress.route_id")?;
        if self.route_id != route.route_id
            || usize::from(self.maneuver_index) >= route.maneuvers.len()
        {
            return Err(WeatherContractError::InvalidField("progress.route"));
        }
        validate_point(&self.position)?;
        validate_not_future(self.observed_at_ms, now_ms, "progress.observed_at_ms")?;
        if self.distance_remaining_metres > route.distance_metres
            || self.duration_remaining_seconds > route.duration_seconds
        {
            return Err(WeatherContractError::InvalidField("progress.remaining"));
        }
        Ok(())
    }
}

impl NavigationSnapshot {
    pub fn validate_at(&self, now_ms: i64) -> Result<(), WeatherContractError> {
        validate_version(self.schema_version)?;
        validate_id(&self.host, "host")?;
        validate_not_future(self.produced_at_ms, now_ms, "produced_at_ms")?;
        match &self.phase {
            NavigationPhase::Idle => {}
            NavigationPhase::Calculating { request_id, .. } => {
                validate_id(request_id, "request_id")?
            }
            NavigationPhase::Cancelled {
                request_id,
                cancelled_at_ms,
            } => {
                validate_id(request_id, "request_id")?;
                validate_not_future(*cancelled_at_ms, now_ms, "cancelled_at_ms")?;
            }
            NavigationPhase::Unavailable { request_id, .. } => {
                if let Some(id) = request_id {
                    validate_id(id, "request_id")?;
                }
            }
            NavigationPhase::Active { route, progress } => {
                route.validate_at(now_ms)?;
                progress.validate_for(route, now_ms)?;
            }
        }
        Ok(())
    }
}

fn validate_version(version: u16) -> Result<(), WeatherContractError> {
    if version == NAVIGATION_SCHEMA_VERSION {
        Ok(())
    } else {
        Err(WeatherContractError::UnsupportedSchema { found: version })
    }
}

fn validate_endpoint(
    endpoint: &RouteEndpoint,
    field: &'static str,
) -> Result<(), WeatherContractError> {
    validate_text(&endpoint.label, field, MAX_WEATHER_LABEL_BYTES)?;
    validate_point(&endpoint.point)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> RouteRequest {
        RouteRequest {
            schema_version: 1,
            request_id: "req-1".into(),
            host: "seat-1".into(),
            expected_generation: 0,
            issued_at_ms: 100,
            kind: RouteRequestKind::Route,
            replaces_route_id: None,
            profile: RouteProfile::Car,
            origin: RouteEndpoint {
                label: "Start".into(),
                point: GeoPoint {
                    latitude: 40.0,
                    longitude: -75.0,
                },
            },
            destination: RouteEndpoint {
                label: "Finish".into(),
                point: GeoPoint {
                    latitude: 40.1,
                    longitude: -75.1,
                },
            },
        }
    }

    #[test]
    fn route_request_rejects_duplicate_unknown_and_non_finite_input() {
        let mut json = serde_json::to_value(request()).unwrap();
        json["origin"]["point"]["latitude"] = serde_json::json!(91.0);
        assert!(RouteRequest::from_json_at(&serde_json::to_vec(&json).unwrap(), 100).is_err());
        let duplicate = br#"{"schema_version":1,"schema_version":1}"#;
        assert!(RouteRequest::from_json_at(duplicate, 100).is_err());
        let mut unknown = serde_json::to_value(request()).unwrap();
        unknown["surprise"] = serde_json::json!(true);
        assert!(RouteRequest::from_json_at(&serde_json::to_vec(&unknown).unwrap(), 100).is_err());
    }

    #[test]
    fn reroute_requires_exact_lineage_and_collections_are_bounded() {
        let mut value = request();
        value.kind = RouteRequestKind::Reroute;
        assert!(value.validate_at(100).is_err());
        value.replaces_route_id = Some("route-1".into());
        assert!(value.validate_at(100).is_ok());
        let mut oversized = serde_json::to_vec(&request()).unwrap();
        oversized.resize(MAX_ROUTE_REQUEST_BYTES + 1, b' ');
        assert!(RouteRequest::from_json_at(&oversized, 100).is_err());
    }
}
