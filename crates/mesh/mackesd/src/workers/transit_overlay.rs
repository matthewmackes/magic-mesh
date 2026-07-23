//! WL-FUNC-012 / OVERLAY-9 — keyless MBTA GTFS-Realtime vehicle adapter.

#![cfg(feature = "async-services")]

use std::collections::HashSet;
use std::io::{self, Read};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mackes_mesh_types::transit::{
    transit_state_topic, TransitOccupancy, TransitSnapshot, TransitStopStatus, TransitVehicle,
};
use prost::Message;
use reqwest::blocking::Client;
use reqwest::header::{CONTENT_TYPE, ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED};

use super::{ShutdownToken, Worker};

/// Explicit opt-in; unset/false is an idle no-op.
pub const ENABLED_ENV: &str = "MDE_OVERLAY_MBTA_TRANSIT";
/// Optional operator-controlled feed override.
pub const ENDPOINT_ENV: &str = "MDE_OVERLAY_MBTA_TRANSIT_URL";
/// Official MBTA vehicle-position feed.
pub const DEFAULT_ENDPOINT: &str = "https://cdn.mbta.com/realtime/VehiclePositions.pb";
/// Feed regeneration/poll cadence.
pub const POLL: Duration = Duration::from_secs(20);
const RETRY_MAX: Duration = Duration::from_secs(5 * 60);
const HTTP_TIMEOUT: Duration = Duration::from_secs(12);
const MAX_BODY_BYTES: usize = 512 * 1024;
const MAX_FEED_ENTITIES: usize = 4_096;
const MAX_RETAINED_VEHICLES: usize = 256;
const MAX_GAPS: usize = 128;
const MAX_STRING_BYTES: usize = 64;
const RELEVANCE_RADIUS_NM: f64 = 15.0;
const MAX_POSITION_AGE_MS: i64 = 120_000;
const MAX_FUTURE_SKEW_MS: i64 = 30_000;
const MAX_FEED_CLOCK_SKEW_MS: u64 = 60_000;
const MAX_SPEED_MPS: f32 = 100.0;
const VEHICLE_FIX_MAX_AGE_MS: i64 = 30_000;
const VEHICLE_MAX_FUTURE_SKEW_MS: i64 = 5_000;
const USER_AGENT: &str =
    "Construct/12 mackesd MBTA-GTFS-RT-overlay (+https://github.com/matthewmackes/magic-mesh)";

/// Finite local vehicle point used for relevance filtering.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransitPoint {
    /// Latitude in decimal degrees.
    pub latitude: f64,
    /// Longitude in decimal degrees.
    pub longitude: f64,
}

/// Conditional binary feed result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeResponse {
    /// Complete HTTP 200 protobuf body.
    Modified(Vec<u8>),
    /// Validator-backed HTTP 304.
    NotModified,
}

/// Injectable binary feed seam.
pub trait TransitProbe: Send + Sync {
    /// Fetch the full MBTA vehicle-position snapshot.
    fn fetch(&self, point: TransitPoint) -> io::Result<ProbeResponse>;
}

#[derive(Debug, Default)]
struct Validators {
    point: Option<TransitPoint>,
    etag: Option<String>,
    last_modified: Option<String>,
}

/// Production rustls probe.
pub struct MbtaHttpProbe {
    client: Client,
    endpoint: String,
    validators: Mutex<Validators>,
}

impl MbtaHttpProbe {
    fn new(endpoint: String) -> io::Result<Self> {
        let client = Client::builder()
            .timeout(HTTP_TIMEOUT)
            .user_agent(USER_AGENT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(io_other)?;
        let url = reqwest::Url::parse(&endpoint).map_err(io_other)?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(io::Error::other("MBTA endpoint must use HTTP(S)"));
        }
        Ok(Self {
            client,
            endpoint,
            validators: Mutex::new(Validators::default()),
        })
    }
}

impl TransitProbe for MbtaHttpProbe {
    fn fetch(&self, point: TransitPoint) -> io::Result<ProbeResponse> {
        validate_point(point)?;
        let mut request = self.client.get(&self.endpoint);
        let mut sent_validator = false;
        {
            let validators = self
                .validators
                .lock()
                .map_err(|_| io::Error::other("MBTA validator lock poisoned"))?;
            if validators
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
                Ok(ProbeResponse::NotModified)
            } else {
                Err(io::Error::other(
                    "MBTA returned 304 although no matching-point validator was sent",
                ))
            };
        }
        if response.status() != reqwest::StatusCode::OK {
            return Err(io::Error::other(format!(
                "MBTA returned unexpected HTTP {} (redirects are disabled)",
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
            "application/x-protobuf" | "application/protobuf" | "application/octet-stream"
        ) {
            return Err(io::Error::other(format!(
                "MBTA returned unexpected content type {content_type:?}"
            )));
        }
        if response
            .content_length()
            .is_some_and(|length| length > u64::try_from(MAX_BODY_BYTES).unwrap_or(u64::MAX))
        {
            return Err(io::Error::other("MBTA protobuf exceeds 524288 byte limit"));
        }
        let etag = header_string(&response, ETAG);
        let last_modified = header_string(&response, LAST_MODIFIED);
        let mut response = response;
        let body = read_bounded_body(&mut response)?;
        *self
            .validators
            .lock()
            .map_err(|_| io::Error::other("MBTA validator lock poisoned"))? = Validators {
            point: Some(point),
            etag,
            last_modified,
        };
        Ok(ProbeResponse::Modified(body))
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

fn read_bounded_body(response: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut body = Vec::with_capacity(96 * 1024);
    response
        .take(u64::try_from(MAX_BODY_BYTES).unwrap_or(u64::MAX) + 1)
        .read_to_end(&mut body)?;
    if body.len() > MAX_BODY_BYTES {
        return Err(io::Error::other("MBTA protobuf exceeds 524288 byte limit"));
    }
    Ok(body)
}

fn io_other(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

// Minimal official GTFS-Realtime proto2 subset. Unknown fields are deliberately
// ignored by prost; semantic validation below is fail-closed.
#[derive(Clone, PartialEq, Message)]
struct FeedMessage {
    #[prost(message, optional, tag = "1")]
    header: Option<FeedHeader>,
    #[prost(message, repeated, tag = "2")]
    entity: Vec<FeedEntity>,
}

#[derive(Clone, PartialEq, Message)]
struct FeedHeader {
    #[prost(string, optional, tag = "1")]
    version: Option<String>,
    #[prost(enumeration = "Incrementality", optional, tag = "2")]
    incrementality: Option<i32>,
    #[prost(uint64, optional, tag = "3")]
    timestamp: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, prost::Enumeration)]
enum Incrementality {
    FullDataset = 0,
    Differential = 1,
}

#[derive(Clone, PartialEq, Message)]
struct FeedEntity {
    #[prost(string, optional, tag = "1")]
    id: Option<String>,
    #[prost(bool, optional, tag = "2")]
    is_deleted: Option<bool>,
    #[prost(message, optional, tag = "4")]
    vehicle: Option<VehiclePosition>,
}

#[derive(Clone, PartialEq, Message)]
struct VehiclePosition {
    #[prost(message, optional, tag = "1")]
    trip: Option<TripDescriptor>,
    #[prost(message, optional, tag = "2")]
    position: Option<Position>,
    #[prost(uint32, optional, tag = "3")]
    current_stop_sequence: Option<u32>,
    #[prost(enumeration = "VehicleStopStatusProto", optional, tag = "4")]
    current_status: Option<i32>,
    #[prost(uint64, optional, tag = "5")]
    timestamp: Option<u64>,
    #[prost(string, optional, tag = "7")]
    stop_id: Option<String>,
    #[prost(message, optional, tag = "8")]
    vehicle: Option<VehicleDescriptor>,
    #[prost(enumeration = "OccupancyProto", optional, tag = "9")]
    occupancy_status: Option<i32>,
    #[prost(uint32, optional, tag = "10")]
    occupancy_percentage: Option<u32>,
}

#[derive(Clone, PartialEq, Message)]
struct TripDescriptor {
    #[prost(string, optional, tag = "5")]
    route_id: Option<String>,
}

#[derive(Clone, PartialEq, Message)]
struct Position {
    #[prost(float, optional, tag = "1")]
    latitude: Option<f32>,
    #[prost(float, optional, tag = "2")]
    longitude: Option<f32>,
    #[prost(float, optional, tag = "3")]
    bearing: Option<f32>,
    #[prost(float, optional, tag = "5")]
    speed: Option<f32>,
}

#[derive(Clone, PartialEq, Message)]
struct VehicleDescriptor {
    #[prost(string, optional, tag = "1")]
    id: Option<String>,
    #[prost(string, optional, tag = "2")]
    label: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, prost::Enumeration)]
enum VehicleStopStatusProto {
    IncomingAt = 0,
    StoppedAt = 1,
    InTransitTo = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, prost::Enumeration)]
enum OccupancyProto {
    Empty = 0,
    ManySeatsAvailable = 1,
    FewSeatsAvailable = 2,
    StandingRoomOnly = 3,
    CrushedStandingRoomOnly = 4,
    Full = 5,
    NotAcceptingPassengers = 6,
    NoDataAvailable = 7,
    NotBoardable = 8,
}

enum Disposition {
    Keep(TransitVehicle, Vec<String>),
    Relevance,
    Quality(String),
    Malformed(String),
}

fn build_snapshot(
    host: &str,
    point: TransitPoint,
    body: &[u8],
    fetched_at_ms: i64,
) -> io::Result<TransitSnapshot> {
    validate_point(point)?;
    if body.len() > MAX_BODY_BYTES {
        return Err(io::Error::other("MBTA protobuf exceeds 524288 byte limit"));
    }
    let feed = FeedMessage::decode(body)
        .map_err(|error| io::Error::other(format!("GTFS-Realtime decode failed: {error}")))?;
    let header = feed
        .header
        .ok_or_else(|| io::Error::other("GTFS-Realtime header missing"))?;
    let version = header
        .version
        .filter(|value| matches!(value.as_str(), "1.0" | "2.0"))
        .ok_or_else(|| io::Error::other("unsupported GTFS-Realtime version"))?;
    if header.incrementality.unwrap_or(0) != Incrementality::FullDataset as i32 {
        return Err(io::Error::other(
            "GTFS-Realtime differential feeds are unsupported",
        ));
    }
    let generated_at_ms = seconds_to_ms(
        header
            .timestamp
            .ok_or_else(|| io::Error::other("GTFS-Realtime header timestamp missing"))?,
    )?;
    if generated_at_ms.abs_diff(fetched_at_ms) > MAX_FEED_CLOCK_SKEW_MS {
        return Err(io::Error::other(
            "GTFS-Realtime header timestamp has implausible clock skew",
        ));
    }
    let mut snapshot = TransitSnapshot::empty(
        host,
        fetched_at_ms,
        generated_at_ms,
        &version,
        point.latitude,
        point.longitude,
    );
    snapshot.feed_total = u32::try_from(feed.entity.len()).unwrap_or(u32::MAX);
    if feed.entity.len() > MAX_FEED_ENTITIES {
        push_gap(
            &mut snapshot.gaps,
            format!(
                "feed contains {} entities; only the first {MAX_FEED_ENTITIES} are processed",
                feed.entity.len()
            ),
        );
    }
    let mut ids = HashSet::new();
    for (index, entity) in feed.entity.into_iter().take(MAX_FEED_ENTITIES).enumerate() {
        let entity_id = entity.id.as_deref().and_then(bounded_string);
        let Some(entity_id) = entity_id else {
            push_gap(&mut snapshot.gaps, format!("entity {index} has invalid id"));
            continue;
        };
        if !ids.insert(entity_id.clone()) {
            push_gap(
                &mut snapshot.gaps,
                format!("duplicate entity id {entity_id:?} omitted"),
            );
            continue;
        }
        if entity.is_deleted.unwrap_or(false) {
            push_gap(
                &mut snapshot.gaps,
                format!("full-dataset entity {entity_id:?} unexpectedly marked deleted"),
            );
            continue;
        }
        let Some(vehicle) = entity.vehicle else {
            continue;
        };
        match parse_vehicle(&entity_id, vehicle, point, fetched_at_ms) {
            Disposition::Keep(vehicle, gaps) => {
                for gap in gaps {
                    push_gap(&mut snapshot.gaps, format!("vehicle {entity_id:?}: {gap}"));
                }
                if snapshot.vehicles.len() < MAX_RETAINED_VEHICLES {
                    snapshot.vehicles.push(vehicle);
                } else if !snapshot
                    .gaps
                    .iter()
                    .any(|gap| gap.contains("retained vehicles capped"))
                {
                    push_gap(
                        &mut snapshot.gaps,
                        format!("retained vehicles capped at {MAX_RETAINED_VEHICLES}"),
                    );
                }
            }
            Disposition::Relevance => {
                snapshot.relevance_filtered = snapshot.relevance_filtered.saturating_add(1);
            }
            Disposition::Quality(reason) => {
                snapshot.quality_filtered = snapshot.quality_filtered.saturating_add(1);
                push_gap(
                    &mut snapshot.gaps,
                    format!("vehicle {entity_id:?} quality-filtered: {reason}"),
                );
            }
            Disposition::Malformed(reason) => push_gap(
                &mut snapshot.gaps,
                format!("vehicle {entity_id:?} malformed: {reason}"),
            ),
        }
    }
    Ok(snapshot)
}

fn parse_vehicle(
    entity_id: &str,
    vehicle: VehiclePosition,
    point: TransitPoint,
    now_ms: i64,
) -> Disposition {
    let Some(position) = vehicle.position else {
        return Disposition::Quality("position missing".to_string());
    };
    let (Some(latitude), Some(longitude)) = (position.latitude, position.longitude) else {
        return Disposition::Quality("coordinates missing".to_string());
    };
    let (latitude, longitude) = (f64::from(latitude), f64::from(longitude));
    if !latitude.is_finite()
        || !longitude.is_finite()
        || !(-90.0..=90.0).contains(&latitude)
        || !(-180.0..=180.0).contains(&longitude)
    {
        return Disposition::Malformed("coordinates are not finite/in range".to_string());
    }
    if great_circle_nm(point.latitude, point.longitude, latitude, longitude) > RELEVANCE_RADIUS_NM {
        return Disposition::Relevance;
    }
    let Some(timestamp) = vehicle.timestamp else {
        return Disposition::Quality("position timestamp missing".to_string());
    };
    let Ok(observed_at_ms) = seconds_to_ms(timestamp) else {
        return Disposition::Malformed("position timestamp overflows".to_string());
    };
    if observed_at_ms.saturating_sub(now_ms) > MAX_FUTURE_SKEW_MS {
        return Disposition::Quality("position timestamp is in the future".to_string());
    }
    if now_ms.saturating_sub(observed_at_ms) > MAX_POSITION_AGE_MS {
        return Disposition::Quality("position is older than 120 seconds".to_string());
    }
    let mut gaps = Vec::new();
    let bearing_deg = match position.bearing {
        Some(value) if value.is_finite() && (0.0..360.0).contains(&value) => Some(value),
        Some(_) => {
            gaps.push("invalid bearing omitted".to_string());
            None
        }
        None => None,
    };
    let speed_mps = match position.speed {
        Some(value) if value.is_finite() && (0.0..=MAX_SPEED_MPS).contains(&value) => Some(value),
        Some(_) => {
            gaps.push("invalid speed omitted".to_string());
            None
        }
        None => None,
    };
    let occupancy = match vehicle.occupancy_status {
        Some(raw) => match OccupancyProto::try_from(raw) {
            Ok(value) => Some(map_occupancy(value)),
            Err(_) => {
                gaps.push(format!("unknown occupancy enum {raw} omitted"));
                None
            }
        },
        None => None,
    };
    let occupancy_percentage = match vehicle.occupancy_percentage {
        Some(value) if value <= 1_000 => Some(value),
        Some(_) => {
            gaps.push("implausible occupancy percentage omitted".to_string());
            None
        }
        None => None,
    };
    let stop_status = match vehicle.current_status {
        Some(raw) => match VehicleStopStatusProto::try_from(raw) {
            Ok(value) => Some(map_stop_status(value)),
            Err(_) => {
                gaps.push(format!("unknown stop-status enum {raw} omitted"));
                None
            }
        },
        None => None,
    };
    let descriptor = vehicle.vehicle.as_ref();
    let id = descriptor
        .and_then(|value| value.id.as_deref())
        .and_then(bounded_string)
        .unwrap_or_else(|| entity_id.to_string());
    let label = descriptor
        .and_then(|value| value.label.as_deref())
        .and_then(bounded_string);
    let route_id = vehicle
        .trip
        .as_ref()
        .and_then(|trip| trip.route_id.as_deref())
        .and_then(bounded_string);
    let stop_id = vehicle.stop_id.as_deref().and_then(bounded_string);
    Disposition::Keep(
        TransitVehicle {
            id,
            label,
            route_id,
            observed_at_ms,
            latitude,
            longitude,
            bearing_deg,
            speed_mps,
            occupancy,
            occupancy_percentage,
            stop_id,
            stop_status,
        },
        gaps,
    )
}

fn map_occupancy(value: OccupancyProto) -> TransitOccupancy {
    match value {
        OccupancyProto::Empty => TransitOccupancy::Empty,
        OccupancyProto::ManySeatsAvailable => TransitOccupancy::ManySeatsAvailable,
        OccupancyProto::FewSeatsAvailable => TransitOccupancy::FewSeatsAvailable,
        OccupancyProto::StandingRoomOnly => TransitOccupancy::StandingRoomOnly,
        OccupancyProto::CrushedStandingRoomOnly => TransitOccupancy::CrushedStandingRoomOnly,
        OccupancyProto::Full => TransitOccupancy::Full,
        OccupancyProto::NotAcceptingPassengers => TransitOccupancy::NotAcceptingPassengers,
        OccupancyProto::NoDataAvailable => TransitOccupancy::NoDataAvailable,
        OccupancyProto::NotBoardable => TransitOccupancy::NotBoardable,
    }
}

fn map_stop_status(value: VehicleStopStatusProto) -> TransitStopStatus {
    match value {
        VehicleStopStatusProto::IncomingAt => TransitStopStatus::IncomingAt,
        VehicleStopStatusProto::StoppedAt => TransitStopStatus::StoppedAt,
        VehicleStopStatusProto::InTransitTo => TransitStopStatus::InTransitTo,
    }
}

fn bounded_string(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()
        && value.len() <= MAX_STRING_BYTES
        && value
            .chars()
            .all(|character| character.is_ascii_graphic() || character == ' '))
    .then(|| value.to_string())
}

fn seconds_to_ms(seconds: u64) -> io::Result<i64> {
    i64::try_from(seconds)
        .ok()
        .and_then(|value| value.checked_mul(1_000))
        .ok_or_else(|| io::Error::other("POSIX timestamp overflows milliseconds"))
}

fn validate_point(point: TransitPoint) -> io::Result<()> {
    if point.latitude.is_finite()
        && point.longitude.is_finite()
        && (-90.0..=90.0).contains(&point.latitude)
        && (-180.0..=180.0).contains(&point.longitude)
    {
        Ok(())
    } else {
        Err(io::Error::other("vehicle point is not finite/in range"))
    }
}

fn point_near(a: TransitPoint, b: TransitPoint) -> bool {
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

fn push_gap(gaps: &mut Vec<String>, gap: String) {
    if gaps.len() < MAX_GAPS {
        gaps.push(gap);
    } else if gaps
        .last()
        .is_some_and(|last| last != "additional normalization gaps omitted")
    {
        gaps[MAX_GAPS - 1] = "additional normalization gaps omitted".to_string();
    }
}

enum PreparedResponse {
    Modified(TransitSnapshot),
    NotModified,
}

/// Workstation-side MBTA transit adapter.
pub struct TransitOverlayWorker {
    host: String,
    probe: Option<Arc<dyn TransitProbe>>,
    bus_root: Option<PathBuf>,
    poll: Duration,
}

impl TransitOverlayWorker {
    /// Production wiring. Disabled unless explicitly opted in.
    #[must_use]
    pub fn new(host: String) -> Self {
        let probe = if env_truthy(ENABLED_ENV) {
            let endpoint = std::env::var(ENDPOINT_ENV)
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_ENDPOINT.to_string());
            match MbtaHttpProbe::new(endpoint) {
                Ok(probe) => Some(Arc::new(probe) as Arc<dyn TransitProbe>),
                Err(error) => {
                    tracing::warn!(target: "mackesd::transit_overlay", %error, "MBTA client unavailable; worker idle");
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
    pub fn with_probe(mut self, probe: Arc<dyn TransitProbe>) -> Self {
        self.probe = Some(probe);
        self
    }

    /// Override or disable Bus access.
    #[must_use]
    pub fn with_bus_root(mut self, root: Option<PathBuf>) -> Self {
        self.bus_root = root;
        self
    }

    fn current_point(&self) -> Option<TransitPoint> {
        let root = self.bus_root.clone()?;
        let persist = mde_bus::persist::Persist::open(root).ok()?;
        let topic = mackes_mesh_types::vehicle::vehicle_state_topic(&self.host);
        let body = persist.read_latest(&topic).ok().flatten()?.body?;
        let vehicle: mackes_mesh_types::vehicle::VehicleState = serde_json::from_str(&body).ok()?;
        validated_vehicle_point(&vehicle, &self.host, now_ms())
    }

    fn publish(&self, snapshot: &TransitSnapshot) {
        if let Some(mut persist) = crate::bus_publish::open_bus(self.bus_root.clone()) {
            crate::bus_publish::publish_json(
                &mut persist,
                &transit_state_topic(&self.host),
                snapshot,
            );
        }
    }

    fn apply_result(
        &self,
        result: io::Result<PreparedResponse>,
        point: TransitPoint,
        last_good: &mut Option<TransitSnapshot>,
    ) -> bool {
        match result {
            Ok(PreparedResponse::Modified(snapshot)) => {
                self.publish(&snapshot);
                *last_good = Some(snapshot);
                true
            }
            Ok(PreparedResponse::NotModified) => {
                if let Some(snapshot) = last_good {
                    if !point_near(
                        TransitPoint {
                            latitude: snapshot.query_latitude,
                            longitude: snapshot.query_longitude,
                        },
                        point,
                    ) {
                        self.publish_failure(last_good, "MBTA 304 point does not match last-good");
                        return false;
                    }
                    snapshot.fetched_at_ms = now_ms();
                    snapshot
                        .gaps
                        .retain(|gap| !gap.starts_with("MBTA refresh failed:"));
                    self.publish(snapshot);
                    true
                } else {
                    false
                }
            }
            Err(error) => {
                self.publish_failure(last_good, &format!("MBTA refresh failed: {error}"));
                false
            }
        }
    }

    fn publish_failure(&self, last_good: &mut Option<TransitSnapshot>, gap: &str) {
        if let Some(snapshot) = last_good {
            snapshot
                .gaps
                .retain(|existing| !existing.starts_with("MBTA refresh failed:"));
            push_gap(&mut snapshot.gaps, gap.to_string());
            self.publish(snapshot);
        }
    }

    async fn fetch_async(
        &self,
        probe: Arc<dyn TransitProbe>,
        point: TransitPoint,
        shutdown: &mut ShutdownToken,
    ) -> Option<io::Result<PreparedResponse>> {
        let host = self.host.clone();
        let task = tokio::task::spawn_blocking(move || match probe.fetch(point)? {
            ProbeResponse::Modified(body) => build_snapshot(&host, point, &body, now_ms())
                .map(PreparedResponse::Modified)
                .map_err(|error| io::Error::other(format!("MBTA payload invalid: {error}"))),
            ProbeResponse::NotModified => Ok(PreparedResponse::NotModified),
        });
        tokio::select! {
            () = shutdown.wait() => None,
            joined = task => Some(match joined {
                Ok(result) => result,
                Err(error) => Err(io::Error::other(format!("MBTA fetch task failed: {error}"))),
            }),
        }
    }
}

fn validated_vehicle_point(
    vehicle: &mackes_mesh_types::vehicle::VehicleState,
    expected_host: &str,
    now: i64,
) -> Option<TransitPoint> {
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
    Some(TransitPoint {
        latitude: gps.latitude,
        longitude: gps.longitude,
    })
}

#[async_trait::async_trait]
impl Worker for TransitOverlayWorker {
    fn name(&self) -> &'static str {
        "transit_overlay"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let Some(probe) = self.probe.clone() else {
            shutdown.wait().await;
            return Ok(());
        };
        let mut last_good = None;
        let mut retry = self.poll;
        loop {
            let Some(point) = self.current_point() else {
                tokio::select! {
                    () = shutdown.wait() => break,
                    () = tokio::time::sleep(self.poll) => {}
                }
                continue;
            };
            let Some(result) = self.fetch_async(probe.clone(), point, &mut shutdown).await else {
                break;
            };
            let success = self.apply_result(result, point, &mut last_good);
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

    use super::*;

    const NOW_MS: i64 = 1_784_755_535_000;

    fn point() -> TransitPoint {
        TransitPoint {
            latitude: 42.3601,
            longitude: -71.0589,
        }
    }

    fn captured_feed() -> Vec<u8> {
        let vehicle = |id: &str, route: &str, lat: f32, lon: f32, timestamp: u64| FeedEntity {
            id: Some(id.to_string()),
            is_deleted: None,
            vehicle: Some(VehiclePosition {
                trip: Some(TripDescriptor {
                    route_id: Some(route.to_string()),
                }),
                position: Some(Position {
                    latitude: Some(lat),
                    longitude: Some(lon),
                    bearing: Some(45.0),
                    speed: Some(22.8),
                }),
                current_stop_sequence: Some(1),
                current_status: Some(VehicleStopStatusProto::StoppedAt as i32),
                timestamp: Some(timestamp),
                stop_id: Some("334".to_string()),
                vehicle: Some(VehicleDescriptor {
                    id: Some(id.to_string()),
                    label: Some("1891".to_string()),
                }),
                occupancy_status: Some(OccupancyProto::Full as i32),
                occupancy_percentage: Some(160),
            }),
        };
        FeedMessage {
            header: Some(FeedHeader {
                version: Some("2.0".to_string()),
                incrementality: Some(Incrementality::FullDataset as i32),
                timestamp: Some(1_784_755_535),
            }),
            entity: vec![
                vehicle("y1891", "22", 42.35, -71.06, 1_784_755_531),
                vehicle("far", "CR", 41.0, -73.0, 1_784_755_531),
            ],
        }
        .encode_to_vec()
    }

    #[test]
    fn captured_live_schema_maps_route_bearing_stop_and_crush_load() {
        let snapshot = build_snapshot("rig-1", point(), &captured_feed(), NOW_MS).expect("decode");
        assert_eq!(snapshot.feed_total, 2);
        assert_eq!(snapshot.vehicles.len(), 1);
        assert_eq!(snapshot.relevance_filtered, 1);
        let vehicle = &snapshot.vehicles[0];
        assert_eq!(vehicle.route_id.as_deref(), Some("22"));
        assert_eq!(vehicle.occupancy, Some(TransitOccupancy::Full));
        assert_eq!(vehicle.occupancy_percentage, Some(160));
        assert_eq!(vehicle.stop_status, Some(TransitStopStatus::StoppedAt));
    }

    #[test]
    fn truncated_overlong_and_differential_payloads_fail_closed() {
        let mut truncated = captured_feed();
        truncated.truncate(truncated.len() / 2);
        assert!(build_snapshot("rig-1", point(), &truncated, NOW_MS).is_err());
        assert!(build_snapshot("rig-1", point(), &vec![0; MAX_BODY_BYTES + 1], NOW_MS).is_err());
        let differential = FeedMessage {
            header: Some(FeedHeader {
                version: Some("2.0".to_string()),
                incrementality: Some(Incrementality::Differential as i32),
                timestamp: Some(1_784_755_535),
            }),
            entity: Vec::new(),
        }
        .encode_to_vec();
        assert!(build_snapshot("rig-1", point(), &differential, NOW_MS).is_err());
    }

    #[test]
    fn duplicate_overlong_nan_unknown_enum_and_future_clock_are_explicit() {
        let mut feed = FeedMessage::decode(captured_feed().as_slice()).expect("fixture");
        feed.entity.push(feed.entity[0].clone());
        let mut hostile = feed.entity[0].clone();
        hostile.id = Some("x".repeat(MAX_STRING_BYTES + 1));
        hostile
            .vehicle
            .as_mut()
            .expect("vehicle")
            .position
            .as_mut()
            .expect("position")
            .latitude = Some(f32::NAN);
        feed.entity.push(hostile);
        let mut nan = feed.entity[0].clone();
        nan.id = Some("nan".to_string());
        nan.vehicle
            .as_mut()
            .expect("vehicle")
            .position
            .as_mut()
            .expect("position")
            .latitude = Some(f32::NAN);
        feed.entity.push(nan);
        let mut unknown = feed.entity[0].clone();
        unknown.id = Some("unknown".to_string());
        unknown.vehicle.as_mut().expect("vehicle").occupancy_status = Some(99);
        feed.entity.push(unknown);
        let snapshot =
            build_snapshot("rig-1", point(), &feed.encode_to_vec(), NOW_MS).expect("fold");
        assert!(snapshot.gaps.iter().any(|gap| gap.contains("duplicate")));
        assert!(snapshot.gaps.iter().any(|gap| gap.contains("invalid id")));
        assert!(snapshot
            .gaps
            .iter()
            .any(|gap| gap.contains("coordinates are not finite")));
        assert!(snapshot
            .gaps
            .iter()
            .any(|gap| gap.contains("unknown occupancy")));

        feed.header.as_mut().expect("header").timestamp = Some(1_784_755_700);
        assert!(build_snapshot("rig-1", point(), &feed.encode_to_vec(), NOW_MS).is_err());
    }

    #[test]
    fn stale_and_future_positions_quality_filter_without_panicking() {
        let mut feed = FeedMessage::decode(captured_feed().as_slice()).expect("fixture");
        feed.entity[0].vehicle.as_mut().expect("vehicle").timestamp = Some(1_784_755_000);
        feed.entity[1].vehicle.as_mut().expect("vehicle").timestamp = Some(1_784_755_600);
        // Bring the second entity nearby so its future timestamp is evaluated.
        let position = feed.entity[1]
            .vehicle
            .as_mut()
            .expect("vehicle")
            .position
            .as_mut()
            .expect("position");
        position.latitude = Some(42.36);
        position.longitude = Some(-71.05);
        let snapshot =
            build_snapshot("rig-1", point(), &feed.encode_to_vec(), NOW_MS).expect("fold");
        assert_eq!(snapshot.quality_filtered, 2);
        assert!(snapshot.vehicles.is_empty());
    }

    #[test]
    fn entity_retention_and_gap_cardinality_are_bounded() {
        let mut feed = FeedMessage::decode(captured_feed().as_slice()).expect("fixture");
        let template = feed.entity[0].clone();
        feed.entity = (0..=MAX_FEED_ENTITIES)
            .map(|index| {
                let mut entity = template.clone();
                let id = format!("vehicle-{index}");
                entity.id = Some(id.clone());
                entity.vehicle.as_mut().expect("vehicle").vehicle = Some(VehicleDescriptor {
                    id: Some(id),
                    label: None,
                });
                entity
            })
            .collect();
        let body = feed.encode_to_vec();
        assert!(
            body.len() <= MAX_BODY_BYTES,
            "fixture remains within body cap"
        );
        let snapshot = build_snapshot("rig-1", point(), &body, NOW_MS).expect("bounded fold");
        assert_eq!(snapshot.feed_total as usize, MAX_FEED_ENTITIES + 1);
        assert_eq!(snapshot.vehicles.len(), MAX_RETAINED_VEHICLES);
        assert!(snapshot.gaps.iter().any(|gap| gap.contains("first 4096")));
        assert!(snapshot
            .gaps
            .iter()
            .any(|gap| gap.contains("capped at 256")));
        assert!(snapshot.gaps.len() <= MAX_GAPS);
    }

    #[test]
    fn vehicle_point_requires_same_host_online_finite_fresh_fix() {
        let mut vehicle = VehicleState::offline("rig-1");
        vehicle.online = true;
        vehicle.published_at_ms = 100_000;
        vehicle.gps = GpsFix {
            fix_type: "gps".to_string(),
            latitude: 42.3601,
            longitude: -71.0589,
            satellites: 8,
            age_s: 1.0,
            ..GpsFix::default()
        };
        assert!(validated_vehicle_point(&vehicle, "rig-1", 110_000).is_some());
        assert!(validated_vehicle_point(&vehicle, "other", 110_000).is_none());
        vehicle.gps.age_s = 25.0;
        assert!(validated_vehicle_point(&vehicle, "rig-1", 110_001).is_none());
        vehicle.gps.age_s = 0.0;
        vehicle.gps.latitude = f64::NAN;
        assert!(validated_vehicle_point(&vehicle, "rig-1", 100_000).is_none());
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
        let probe =
            MbtaHttpProbe::new(format!("http://{redirect_addr}/vehicles.pb")).expect("client");
        let error = probe.fetch(point()).expect_err("redirect rejected");
        assert!(error.to_string().contains("redirects are disabled"));
        redirect_thread.join().expect("redirect join");
        target_thread.join().expect("target join");
        assert!(!contacted.load(Ordering::Relaxed));
    }

    #[test]
    fn failed_refresh_keeps_timestamp_and_publishes_degraded_latest_snapshot() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().to_path_buf();
        let worker =
            TransitOverlayWorker::new("rig-1".to_string()).with_bus_root(Some(root.clone()));
        let original = build_snapshot("rig-1", point(), &captured_feed(), NOW_MS).expect("parse");
        let mut last = None;
        assert!(worker.apply_result(Ok(PreparedResponse::Modified(original)), point(), &mut last));
        assert!(!worker.apply_result(Err(io::Error::other("timeout")), point(), &mut last));
        assert_eq!(last.as_ref().expect("last").fetched_at_ms, NOW_MS);
        assert!(last
            .as_ref()
            .expect("last")
            .gaps
            .iter()
            .any(|gap| gap.contains("timeout")));
        let persist = Persist::open(root).expect("bus");
        let rows = persist
            .list_since(&transit_state_topic("rig-1"), None)
            .expect("read");
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn not_modified_cannot_relabel_a_moved_points_snapshot() {
        let worker = TransitOverlayWorker::new("rig-1".to_string()).with_bus_root(None);
        let original = build_snapshot("rig-1", point(), &captured_feed(), NOW_MS).expect("parse");
        let mut last = Some(original);
        let moved = TransitPoint {
            latitude: point().latitude + 1.0,
            longitude: point().longitude,
        };
        assert!(!worker.apply_result(Ok(PreparedResponse::NotModified), moved, &mut last));
        let retained = last.expect("last-good retained");
        assert_eq!(retained.query_latitude, point().latitude);
        assert_eq!(retained.fetched_at_ms, NOW_MS);
        assert!(retained.gaps.iter().any(|gap| gap.contains("304 point")));
    }

    struct SlowProbe;

    impl TransitProbe for SlowProbe {
        fn fetch(&self, _point: TransitPoint) -> io::Result<ProbeResponse> {
            std::thread::sleep(Duration::from_millis(500));
            Ok(ProbeResponse::Modified(captured_feed()))
        }
    }

    #[tokio::test]
    async fn shutdown_wins_while_blocking_http_is_in_flight() {
        let worker = TransitOverlayWorker::new("rig-1".to_string()).with_bus_root(None);
        let (tx, rx) = tokio::sync::watch::channel(false);
        let mut shutdown = ShutdownToken::from_receiver(rx);
        let sender = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            tx.send(true).expect("shutdown");
        });
        let result = tokio::time::timeout(
            Duration::from_millis(200),
            worker.fetch_async(Arc::new(SlowProbe), point(), &mut shutdown),
        )
        .await
        .expect("runtime remains responsive");
        assert!(result.is_none());
        sender.await.expect("sender");
    }
}
