//! WL-FUNC-012 / OVERLAY-9 — keyless MBTA GTFS-Realtime vehicle adapter.
//!
//! Workstations start this keyless MassDOT/MBTA producer by default.
//! Operators can explicitly disable it with `MDE_OVERLAY_MBTA_TRANSIT=0/false/no/off`.
//! Missing same-host vehicle context publishes an honest empty retained mirror
//! instead of leaving the catalog topic absent or replaying stale vehicles.
//! Blocking rustls HTTP and protobuf normalization run away from Tokio worker
//! threads.

#![cfg(feature = "async-services")]

use std::collections::HashSet;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mackes_mesh_types::transit::{
    transit_state_topic, TransitOccupancy, TransitSnapshot, TransitStopStatus, TransitVehicle,
};
use prost::Message;
use reqwest::blocking::Client;
use reqwest::header::{CONTENT_TYPE, ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED};

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;

use super::{ShutdownToken, Worker};

/// Explicit disable for the keyless default-on MassDOT/MBTA producer.
pub const ENABLED_ENV: &str = "MDE_OVERLAY_MBTA_TRANSIT";
/// Optional operator-controlled feed override.
pub const ENDPOINT_ENV: &str = "MDE_OVERLAY_MBTA_TRANSIT_URL";
/// Official MBTA vehicle-position feed.
pub const DEFAULT_ENDPOINT: &str = "https://cdn.mbta.com/realtime/VehiclePositions.pb";
/// Feed regeneration/poll cadence.
pub const POLL: Duration = Duration::from_secs(20);
const RETRY_MAX: Duration = Duration::from_secs(5 * 60);
const NO_FIX_RETRY: Duration = Duration::from_secs(20);
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

    /// Commit any conditional validators staged by the successful fetch.
    /// Fixture probes and probes without validators need no action.
    fn commit(&self, _point: TransitPoint) {}
}

#[derive(Debug, Clone)]
struct PointValidators {
    point: TransitPoint,
    etag: Option<String>,
    last_modified: Option<String>,
}

#[derive(Debug, Default)]
struct Validators {
    committed: Option<PointValidators>,
    staged: Option<PointValidators>,
}

/// Production rustls probe.
pub struct MbtaHttpProbe {
    endpoint: String,
    validators: Mutex<Validators>,
}

impl MbtaHttpProbe {
    fn new(endpoint: String) -> io::Result<Self> {
        let endpoint = validate_endpoint(&endpoint)?.to_string();
        Self::from_endpoint(endpoint)
    }

    fn from_endpoint(endpoint: String) -> io::Result<Self> {
        Ok(Self {
            endpoint,
            validators: Mutex::new(Validators::default()),
        })
    }

    #[cfg(test)]
    fn new_for_test(endpoint: String) -> io::Result<Self> {
        Self::from_endpoint(endpoint)
    }
}

impl TransitProbe for MbtaHttpProbe {
    fn fetch(&self, point: TransitPoint) -> io::Result<ProbeResponse> {
        validate_point(point)?;
        // `reqwest::blocking::Client` owns an internal runtime. Construct and
        // drop it inside this blocking fetch call; `fetch_async` runs this
        // method via `spawn_blocking`, while sync tests call it outside a Tokio
        // runtime.
        let client = mbta_http_client()?;
        let mut request = client.get(&self.endpoint);
        let mut sent_validator = false;
        {
            let validators = self
                .validators
                .lock()
                .map_err(|_| io::Error::other("MBTA validator lock poisoned"))?;
            if let Some(committed) = validators
                .committed
                .as_ref()
                .filter(|validators| point_near(validators.point, point))
            {
                if let Some(value) = &committed.etag {
                    request = request.header(IF_NONE_MATCH, value);
                    sent_validator = true;
                }
                if let Some(value) = &committed.last_modified {
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
        self.validators
            .lock()
            .map_err(|_| io::Error::other("MBTA validator lock poisoned"))?
            .staged = Some(PointValidators {
            point,
            etag,
            last_modified,
        });
        Ok(ProbeResponse::Modified(body))
    }

    fn commit(&self, point: TransitPoint) {
        let mut validators = self
            .validators
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if validators
            .staged
            .as_ref()
            .is_some_and(|staged| staged.point == point)
        {
            validators.committed = validators.staged.take();
        }
    }
}

fn mbta_http_client() -> io::Result<Client> {
    Client::builder()
        .timeout(HTTP_TIMEOUT)
        .user_agent(USER_AGENT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(io_other)
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

fn validate_endpoint(value: &str) -> io::Result<reqwest::Url> {
    let url = reqwest::Url::parse(value).map_err(io_other)?;
    if url.scheme() != "https"
        || url.host_str() != Some("cdn.mbta.com")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/realtime/VehiclePositions.pb"
    {
        return Err(io::Error::other(
            "MBTA endpoint is outside the strict official-feed allowlist",
        ));
    }
    Ok(url)
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

#[derive(Clone)]
enum PreparedResponse {
    Modified(TransitSnapshot),
    NotModified,
}

enum RefreshCommit {
    Applied {
        success: bool,
        commit_validators: bool,
    },
    PointChanged,
    NoPoint,
}

trait TransitBus {
    fn read_latest_body(&mut self, topic: &str) -> io::Result<Option<String>>;
    fn publish_snapshot(&mut self, topic: &str, snapshot: &TransitSnapshot) -> io::Result<()>;
}

impl TransitBus for Persist {
    fn read_latest_body(&mut self, topic: &str) -> io::Result<Option<String>> {
        self.reopen_if_index_changed();
        self.read_latest(topic)
            .map_err(io_other)?
            .map(|message| {
                message
                    .body
                    .ok_or_else(|| io::Error::other("transit Bus row has no body"))
            })
            .transpose()
    }

    fn publish_snapshot(&mut self, topic: &str, snapshot: &TransitSnapshot) -> io::Result<()> {
        let body = serde_json::to_string(snapshot).map_err(|error| {
            crate::metrics::record_bus_publish_error();
            io_other(error)
        })?;
        self.reopen_if_index_changed();
        self.write(topic, Priority::Default, None, Some(&body))
            .map(|_| ())
            .map_err(|error| {
                crate::metrics::record_bus_publish_error();
                io_other(error)
            })
    }
}

/// Workstation-side MBTA transit adapter.
pub struct TransitOverlayWorker {
    host: String,
    probe: Option<Arc<dyn TransitProbe>>,
    bus_root_override: Option<PathBuf>,
    poll: Duration,
}

impl TransitOverlayWorker {
    /// Production wiring. The keyless MassDOT/MBTA adapter is enabled by
    /// default on spawned Workstation-tier nodes; an explicit false-y env
    /// disables it. Missing vehicle context publishes an honest empty mirror
    /// instead of leaving the catalog topic absent.
    #[must_use]
    pub fn new(host: String) -> Self {
        let probe = if env_default_enabled(ENABLED_ENV) {
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
            bus_root_override: None,
            poll: POLL,
        }
    }

    /// Inject a fixture probe.
    #[must_use]
    pub fn with_probe(mut self, probe: Arc<dyn TransitProbe>) -> Self {
        self.probe = Some(probe);
        self
    }

    /// Override the Bus root. Production resolves the current user/system root
    /// for every transaction when no override is configured.
    #[must_use]
    pub fn with_bus_root(mut self, root: PathBuf) -> Self {
        self.bus_root_override = Some(root);
        self
    }

    /// Override cadence for tests.
    #[must_use]
    pub const fn with_poll(mut self, poll: Duration) -> Self {
        self.poll = poll;
        self
    }

    fn open_bus(&self) -> io::Result<Persist> {
        let root = transit_bus_root(
            self.bus_root_override.as_deref(),
            crate::bus_publish::default_bus_root(),
        );
        let mut persist = Persist::open(root).map_err(io_other)?;
        persist.reopen_if_index_changed();
        Ok(persist)
    }

    fn current_point(&self, bus: &mut impl TransitBus) -> io::Result<Option<TransitPoint>> {
        let topic = mackes_mesh_types::vehicle::vehicle_state_topic(&self.host);
        let Some(body) = bus.read_latest_body(&topic)? else {
            return Ok(None);
        };
        let vehicle: mackes_mesh_types::vehicle::VehicleState =
            serde_json::from_str(&body).map_err(io_other)?;
        validated_vehicle_point(&vehicle, &self.host, now_ms())
    }

    fn publish(&self, bus: &mut impl TransitBus, snapshot: &TransitSnapshot) -> io::Result<()> {
        bus.publish_snapshot(&transit_state_topic(&self.host), snapshot)
    }

    fn no_context_snapshot(&self, reason: &str) -> TransitSnapshot {
        let mut snapshot = TransitSnapshot::empty(&self.host, now_ms(), 0, "2.0", 0.0, 0.0);
        push_gap(&mut snapshot.gaps, reason.to_string());
        snapshot
    }

    fn apply_result(
        &self,
        bus: &mut impl TransitBus,
        result: Result<PreparedResponse, String>,
        point: TransitPoint,
        last_good: &mut Option<TransitSnapshot>,
    ) -> io::Result<bool> {
        match result {
            Ok(PreparedResponse::Modified(snapshot)) => {
                self.publish(bus, &snapshot)?;
                *last_good = Some(snapshot);
                Ok(true)
            }
            Ok(PreparedResponse::NotModified) => {
                if let Some(snapshot) = last_good.as_ref() {
                    if !point_near(
                        TransitPoint {
                            latitude: snapshot.query_latitude,
                            longitude: snapshot.query_longitude,
                        },
                        point,
                    ) {
                        self.publish_failure(
                            bus,
                            last_good,
                            point,
                            "MBTA 304 point does not match last-good",
                        )?;
                        return Ok(false);
                    }
                    let mut refreshed = snapshot.clone();
                    refreshed.fetched_at_ms = now_ms();
                    refreshed
                        .gaps
                        .retain(|gap| !gap.starts_with("MBTA refresh failed:"));
                    self.publish(bus, &refreshed)?;
                    *last_good = Some(refreshed);
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
            Err(error) => {
                self.publish_failure(
                    bus,
                    last_good,
                    point,
                    &format!("MBTA refresh failed: {error}"),
                )?;
                Ok(false)
            }
        }
    }

    fn publish_failure(
        &self,
        bus: &mut impl TransitBus,
        last_good: &mut Option<TransitSnapshot>,
        point: TransitPoint,
        gap: &str,
    ) -> io::Result<()> {
        // A failed refresh after the vehicle has moved must not republish the
        // old nearby set under the new query context. Publish an empty,
        // degraded shell to retract the retained markers from the Bus, then
        // drop the in-memory last-good so a later 304 cannot revive it.
        if last_good.as_ref().is_some_and(|snapshot| {
            !point_near(
                TransitPoint {
                    latitude: snapshot.query_latitude,
                    longitude: snapshot.query_longitude,
                },
                point,
            )
        }) {
            let previous = last_good.as_ref().expect("last-good checked above");
            let mut cleared = TransitSnapshot::empty(
                &self.host,
                now_ms(),
                previous.feed_generated_at_ms,
                &previous.feed_version,
                point.latitude,
                point.longitude,
            );
            push_gap(
                &mut cleared.gaps,
                format!("MBTA retained snapshot cleared after query point changed: {gap}"),
            );
            self.publish(bus, &cleared)?;
            *last_good = None;
            return Ok(());
        }
        if let Some(snapshot) = last_good.as_ref() {
            let mut degraded = snapshot.clone();
            degraded
                .gaps
                .retain(|existing| !existing.starts_with("MBTA refresh failed:"));
            push_gap(&mut degraded.gaps, gap.to_string());
            self.publish(bus, &degraded)?;
            *last_good = Some(degraded);
        }
        Ok(())
    }

    fn publish_no_context_degraded(
        &self,
        bus: &mut impl TransitBus,
        last_good: &mut Option<TransitSnapshot>,
        reason: &str,
    ) -> io::Result<()> {
        tracing::warn!(target: "mackesd::transit_overlay", host = %self.host, error = reason, "MBTA refresh has no fresh same-host vehicle context; publishing empty degraded snapshot");
        let snapshot = self.no_context_snapshot(reason);
        self.publish(bus, &snapshot)?;
        // Vehicle-scoped transit rows are invalid once the same-host fix
        // disappears. Keep the retained Bus topic present and licensed, but do
        // not let a later failure or 304 replay vehicles from the stale point.
        *last_good = None;
        Ok(())
    }

    fn ensure_no_context_published(
        &self,
        bus: &mut impl TransitBus,
        last_good: &mut Option<TransitSnapshot>,
        no_context_published: &mut bool,
    ) -> io::Result<()> {
        if *no_context_published {
            let current = bus
                .read_latest_body(&transit_state_topic(&self.host))?
                .map(|body| serde_json::from_str::<TransitSnapshot>(&body).map_err(io_other))
                .transpose()?;
            if current.is_some_and(|snapshot| {
                snapshot.host == self.host
                    && snapshot.feed_generated_at_ms == 0
                    && snapshot.query_latitude == 0.0
                    && snapshot.query_longitude == 0.0
                    && snapshot.vehicles.is_empty()
                    && snapshot
                        .gaps
                        .iter()
                        .any(|gap| gap.contains("vehicle fix unavailable"))
            }) {
                return Ok(());
            }
        }
        self.publish_no_context_degraded(
            bus,
            last_good,
            "MBTA refresh failed: fresh same-host MG90 vehicle fix unavailable",
        )?;
        *no_context_published = true;
        Ok(())
    }

    fn current_point_or_clear(
        &self,
        bus: &mut impl TransitBus,
        last_good: &mut Option<TransitSnapshot>,
        no_context_published: &mut bool,
    ) -> io::Result<Option<TransitPoint>> {
        let point = self.current_point(bus)?;
        if point.is_some() {
            *no_context_published = false;
        } else {
            self.ensure_no_context_published(bus, last_good, no_context_published)?;
        }
        Ok(point)
    }

    async fn fetch_async(
        &self,
        probe: Arc<dyn TransitProbe>,
        point: TransitPoint,
        shutdown: &mut ShutdownToken,
    ) -> Option<Result<PreparedResponse, String>> {
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
                Ok(result) => result.map_err(|error| error.to_string()),
                Err(error) => Err(format!("MBTA fetch task failed: {error}")),
            }),
        }
    }
}

fn transit_bus_root(explicit: Option<&Path>, current: Option<PathBuf>) -> PathBuf {
    explicit
        .map(Path::to_path_buf)
        .or(current)
        .unwrap_or_else(|| PathBuf::from(mde_bus::SYSTEM_BUS_ROOT))
}

fn validated_vehicle_point(
    vehicle: &mackes_mesh_types::vehicle::VehicleState,
    expected_host: &str,
    now: i64,
) -> io::Result<Option<TransitPoint>> {
    let mirror_age = now.saturating_sub(vehicle.published_at_ms).max(0);
    let future_skew = vehicle.published_at_ms.saturating_sub(now).max(0);
    let gps = &vehicle.gps;
    if vehicle.host != expected_host {
        return Err(io::Error::other(
            "vehicle Bus row host does not match topic",
        ));
    }
    if !vehicle.online || !gps.has_fix() {
        return Ok(None);
    }
    if !gps.latitude.is_finite()
        || !gps.longitude.is_finite()
        || !(-90.0..=90.0).contains(&gps.latitude)
        || !(-180.0..=180.0).contains(&gps.longitude)
        || !gps.age_s.is_finite()
        || gps.age_s < 0.0
    {
        return Err(io::Error::other("vehicle Bus row contains an invalid fix"));
    }
    if future_skew > VEHICLE_MAX_FUTURE_SKEW_MS
        || mirror_age as f64 + f64::from(gps.age_s) * 1_000.0 > VEHICLE_FIX_MAX_AGE_MS as f64
    {
        return Ok(None);
    }
    Ok(Some(TransitPoint {
        latitude: gps.latitude,
        longitude: gps.longitude,
    }))
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
        let mut no_fix_published = false;
        let mut pending: Option<(TransitPoint, Result<PreparedResponse, String>)> = None;
        loop {
            let point = match self.open_bus().and_then(|mut bus| {
                self.current_point_or_clear(&mut bus, &mut last_good, &mut no_fix_published)
            }) {
                Ok(Some(point)) => point,
                Ok(None) => {
                    pending = None;
                    retry = self.poll;
                    tokio::select! {
                        () = shutdown.wait() => break,
                        () = tokio::time::sleep(NO_FIX_RETRY.min(self.poll)) => {}
                    }
                    continue;
                }
                Err(error) => {
                    tracing::warn!(
                        target: "mackesd::transit_overlay",
                        host = %self.host,
                        %error,
                        "MBTA vehicle context transaction deferred"
                    );
                    tokio::select! {
                        () = shutdown.wait() => break,
                        () = tokio::time::sleep(retry.min(self.poll)) => {}
                    }
                    continue;
                }
            };
            if pending
                .as_ref()
                .is_some_and(|(pending_point, _)| *pending_point != point)
            {
                pending = None;
            }
            if pending.is_none() {
                let Some(result) = self.fetch_async(probe.clone(), point, &mut shutdown).await
                else {
                    break;
                };
                pending = Some((point, result));
            }

            // MBTA I/O and protobuf normalization run off-thread without a Bus
            // handle. Re-open and re-read the exact vehicle point before any
            // projection or private-state commit. Keep the prepared result
            // across storage faults so retry does not duplicate provider work
            // or advance conditional validators before publication succeeds.
            let commit = self.open_bus().and_then(|mut bus| {
                let latest = self.current_point(&mut bus)?;
                let Some(latest) = latest else {
                    self.ensure_no_context_published(
                        &mut bus,
                        &mut last_good,
                        &mut no_fix_published,
                    )?;
                    return Ok(RefreshCommit::NoPoint);
                };
                no_fix_published = false;
                if latest != point {
                    self.publish_failure(
                        &mut bus,
                        &mut last_good,
                        latest,
                        "MBTA vehicle point changed during refresh",
                    )?;
                    return Ok(RefreshCommit::PointChanged);
                }
                let result = pending.as_ref().expect("prepared MBTA result").1.clone();
                let commit_validators = matches!(result, Ok(PreparedResponse::Modified(_)));
                self.apply_result(&mut bus, result, point, &mut last_good)
                    .map(|success| RefreshCommit::Applied {
                        success,
                        commit_validators,
                    })
            });
            let success = match commit {
                Ok(RefreshCommit::Applied {
                    success,
                    commit_validators,
                }) => {
                    if commit_validators {
                        probe.commit(point);
                    }
                    pending = None;
                    success
                }
                Ok(RefreshCommit::PointChanged) => {
                    pending = None;
                    tokio::select! {
                        () = shutdown.wait() => break,
                        () = tokio::time::sleep(retry.min(self.poll)) => {}
                    }
                    continue;
                }
                Ok(RefreshCommit::NoPoint) => {
                    pending = None;
                    retry = self.poll;
                    tokio::select! {
                        () = shutdown.wait() => break,
                        () = tokio::time::sleep(NO_FIX_RETRY.min(self.poll)) => {}
                    }
                    continue;
                }
                Err(error) => {
                    tracing::warn!(
                        target: "mackesd::transit_overlay",
                        host = %self.host,
                        %error,
                        "MBTA refresh transaction deferred"
                    );
                    tokio::select! {
                        () = shutdown.wait() => break,
                        () = tokio::time::sleep(retry.min(self.poll)) => {}
                    }
                    continue;
                }
            };
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
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc;

    use mackes_mesh_types::vehicle::{GpsFix, VehicleState};

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

    fn live_feed() -> Vec<u8> {
        let mut feed = FeedMessage::decode(captured_feed().as_slice()).expect("captured feed");
        let seconds = u64::try_from(now_ms() / 1_000).expect("current seconds");
        feed.header.as_mut().expect("header").timestamp = Some(seconds);
        for entity in &mut feed.entity {
            entity.vehicle.as_mut().expect("vehicle").timestamp = Some(seconds);
        }
        feed.encode_to_vec()
    }

    fn vehicle(point: TransitPoint) -> VehicleState {
        let mut vehicle = VehicleState::offline("rig-1");
        vehicle.online = true;
        vehicle.published_at_ms = now_ms();
        vehicle.gps = GpsFix {
            fix_type: "gps".to_string(),
            latitude: point.latitude,
            longitude: point.longitude,
            satellites: 8,
            age_s: 0.0,
            ..GpsFix::default()
        };
        vehicle
    }

    fn write_vehicle(persist: &Persist, vehicle: &VehicleState) {
        persist
            .write(
                &mackes_mesh_types::vehicle::vehicle_state_topic("rig-1"),
                Priority::Default,
                None,
                Some(&serde_json::to_string(vehicle).expect("vehicle JSON")),
            )
            .expect("vehicle publication");
    }

    #[derive(Default)]
    struct FixtureBus {
        vehicle_body: Option<String>,
        output_body: Option<String>,
        read_fails: bool,
        failed_writes_remaining: usize,
        published: Vec<TransitSnapshot>,
    }

    impl TransitBus for FixtureBus {
        fn read_latest_body(&mut self, topic: &str) -> io::Result<Option<String>> {
            if self.read_fails {
                Err(io::Error::other("injected transit Bus read failure"))
            } else if topic == transit_state_topic("rig-1") {
                Ok(self.output_body.clone())
            } else {
                Ok(self.vehicle_body.clone())
            }
        }

        fn publish_snapshot(&mut self, _topic: &str, snapshot: &TransitSnapshot) -> io::Result<()> {
            if self.failed_writes_remaining > 0 {
                self.failed_writes_remaining -= 1;
                return Err(io::Error::other("injected transit Bus write failure"));
            }
            self.output_body = Some(serde_json::to_string(snapshot).expect("snapshot JSON"));
            self.published.push(snapshot.clone());
            Ok(())
        }
    }

    #[derive(Default)]
    struct LiveProbe {
        fetches: AtomicUsize,
        commits: AtomicUsize,
    }

    impl TransitProbe for LiveProbe {
        fn fetch(&self, _point: TransitPoint) -> io::Result<ProbeResponse> {
            self.fetches.fetch_add(1, Ordering::SeqCst);
            Ok(ProbeResponse::Modified(live_feed()))
        }

        fn commit(&self, _point: TransitPoint) {
            self.commits.fetch_add(1, Ordering::SeqCst);
        }
    }

    struct RaceProbe {
        started: Mutex<Option<mpsc::Sender<()>>>,
        release: Mutex<Option<mpsc::Receiver<()>>>,
        fetches: AtomicUsize,
        commits: AtomicUsize,
    }

    impl TransitProbe for RaceProbe {
        fn fetch(&self, _point: TransitPoint) -> io::Result<ProbeResponse> {
            let call = self.fetches.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                if let Some(started) = self.started.lock().unwrap().take() {
                    started
                        .send(())
                        .map_err(|_| io::Error::other("race observer dropped"))?;
                    self.release
                        .lock()
                        .unwrap()
                        .take()
                        .ok_or_else(|| io::Error::other("race release missing"))?
                        .recv()
                        .map_err(|_| io::Error::other("race release dropped"))?;
                }
            }
            Ok(ProbeResponse::Modified(live_feed()))
        }

        fn commit(&self, _point: TransitPoint) {
            self.commits.fetch_add(1, Ordering::SeqCst);
        }
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
        assert!(validated_vehicle_point(&vehicle, "rig-1", 110_000)
            .unwrap()
            .is_some());
        assert!(validated_vehicle_point(&vehicle, "other", 110_000).is_err());
        vehicle.gps.age_s = 25.0;
        assert!(validated_vehicle_point(&vehicle, "rig-1", 110_001)
            .unwrap()
            .is_none());
        vehicle.gps.age_s = 0.0;
        vehicle.gps.latitude = f64::NAN;
        assert!(validated_vehicle_point(&vehicle, "rig-1", 100_000).is_err());
    }

    #[test]
    fn context_read_or_decode_fault_is_effect_free() {
        let worker = TransitOverlayWorker::new("rig-1".to_string());
        let original =
            build_snapshot("rig-1", point(), &captured_feed(), NOW_MS).expect("last-good snapshot");
        for mut bus in [
            FixtureBus {
                read_fails: true,
                ..FixtureBus::default()
            },
            FixtureBus {
                vehicle_body: Some("{not vehicle JSON".to_string()),
                ..FixtureBus::default()
            },
        ] {
            let mut last_good = Some(original.clone());
            let mut no_context_published = false;
            assert!(worker
                .current_point_or_clear(&mut bus, &mut last_good, &mut no_context_published,)
                .is_err());
            assert_eq!(last_good.as_ref().unwrap().vehicles, original.vehicles);
            assert!(!no_context_published);
            assert!(bus.published.is_empty());
        }
    }

    #[test]
    fn failed_write_retries_prepared_result_without_refetch_or_early_validator_commit() {
        let worker = TransitOverlayWorker::new("rig-1".to_string());
        let probe = LiveProbe::default();
        let response = probe.fetch(point()).expect("single provider fetch");
        let prepared = match response {
            ProbeResponse::Modified(body) => Ok(PreparedResponse::Modified(
                build_snapshot("rig-1", point(), &body, now_ms()).expect("prepared snapshot"),
            )),
            ProbeResponse::NotModified => panic!("fixture unexpectedly returned 304"),
        };
        let mut bus = FixtureBus {
            failed_writes_remaining: 1,
            ..FixtureBus::default()
        };
        let mut last_good = None;

        assert!(worker
            .apply_result(&mut bus, prepared.clone(), point(), &mut last_good)
            .is_err());
        assert!(last_good.is_none());
        assert_eq!(probe.fetches.load(Ordering::SeqCst), 1);
        assert_eq!(probe.commits.load(Ordering::SeqCst), 0);
        assert!(bus.published.is_empty());

        assert!(worker
            .apply_result(&mut bus, prepared, point(), &mut last_good)
            .expect("corrected-forward publication"));
        probe.commit(point());
        assert_eq!(probe.fetches.load(Ordering::SeqCst), 1);
        assert_eq!(probe.commits.load(Ordering::SeqCst), 1);
        assert!(last_good.is_some());
        assert_eq!(bus.published.len(), 1);
        assert!(bus.published[0].vehicles.len() <= MAX_RETAINED_VEHICLES);

        let mut no_context_published = false;
        bus.failed_writes_remaining = 1;
        assert!(worker
            .current_point_or_clear(&mut bus, &mut last_good, &mut no_context_published,)
            .is_err());
        assert!(last_good.is_some());
        assert!(!no_context_published);
        assert_eq!(bus.published.len(), 1);

        assert!(worker
            .current_point_or_clear(&mut bus, &mut last_good, &mut no_context_published,)
            .expect("corrected-forward no-context publication")
            .is_none());
        assert!(last_good.is_none());
        assert!(no_context_published);
        assert_eq!(bus.published.len(), 2);
        assert!(bus.published.last().unwrap().vehicles.is_empty());

        assert!(worker
            .current_point_or_clear(&mut bus, &mut last_good, &mut no_context_published,)
            .expect("suppressed repeated no-context publication")
            .is_none());
        assert_eq!(bus.published.len(), 2);
        bus.output_body = None;
        assert!(worker
            .current_point_or_clear(&mut bus, &mut last_good, &mut no_context_published,)
            .expect("replacement output publication")
            .is_none());
        assert_eq!(bus.published.len(), 3);
    }

    #[tokio::test]
    async fn late_and_replaced_bus_recovers_in_the_same_worker() {
        assert_eq!(
            transit_bus_root(None, None),
            PathBuf::from(mde_bus::SYSTEM_BUS_ROOT)
        );
        let root = tempfile::tempdir().expect("root");
        let bus_root = root.path().join("bus");
        std::fs::write(&bus_root, b"blocks Persist::open").expect("late Bus blocker");
        let probe = Arc::new(LiveProbe::default());
        let mut worker = TransitOverlayWorker::new("rig-1".to_string())
            .with_probe(probe.clone())
            .with_bus_root(bus_root.clone())
            .with_poll(Duration::from_millis(10));
        let (tx, rx) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(async move { worker.run(ShutdownToken::from_receiver(rx)).await });

        tokio::time::sleep(Duration::from_millis(35)).await;
        assert!(!task.is_finished(), "late Bus terminated the worker");
        std::fs::remove_file(&bus_root).expect("unblock Bus root");
        let first_bus = Persist::open(bus_root.clone()).expect("activate Bus");
        write_vehicle(&first_bus, &vehicle(point()));
        let topic = transit_state_topic("rig-1");
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Ok(Some(message)) = first_bus.read_latest(&topic) {
                    let snapshot: TransitSnapshot =
                        serde_json::from_str(message.body.as_deref().unwrap_or_default()).unwrap();
                    if snapshot.query_latitude == point().latitude {
                        assert!(snapshot.vehicles.len() <= MAX_RETAINED_VEHICLES);
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("late Bus publication");

        drop(first_bus);
        for suffix in ["", "-wal", "-shm"] {
            let path = PathBuf::from(format!(
                "{}{suffix}",
                bus_root.join("index.sqlite").display()
            ));
            if let Err(error) = std::fs::remove_file(path) {
                assert_eq!(error.kind(), io::ErrorKind::NotFound);
            }
        }
        let replacement_bus = Persist::open(bus_root.clone()).expect("replacement Bus");
        let moved = TransitPoint {
            latitude: 42.4,
            longitude: -71.2,
        };
        write_vehicle(&replacement_bus, &vehicle(moved));
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Ok(Some(message)) = replacement_bus.read_latest(&topic) {
                    let snapshot: TransitSnapshot =
                        serde_json::from_str(message.body.as_deref().unwrap_or_default()).unwrap();
                    if snapshot.query_latitude == moved.latitude {
                        assert!(snapshot.vehicles.len() <= MAX_RETAINED_VEHICLES);
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("replacement Bus publication");

        assert!(probe.fetches.load(Ordering::SeqCst) >= 2);
        assert!(probe.commits.load(Ordering::SeqCst) >= 2);
        tx.send(true).expect("shutdown");
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("prompt shutdown")
            .expect("worker join")
            .expect("worker result");
    }

    #[tokio::test]
    async fn post_fetch_point_race_discards_old_feed_before_publication() {
        let bus = tempfile::tempdir().expect("bus");
        let persist = Persist::open(bus.path().to_path_buf()).expect("persist");
        let old_point = point();
        write_vehicle(&persist, &vehicle(old_point));
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let probe = Arc::new(RaceProbe {
            started: Mutex::new(Some(started_tx)),
            release: Mutex::new(Some(release_rx)),
            fetches: AtomicUsize::new(0),
            commits: AtomicUsize::new(0),
        });
        let mut worker = TransitOverlayWorker::new("rig-1".to_string())
            .with_probe(probe.clone())
            .with_bus_root(bus.path().to_path_buf())
            .with_poll(Duration::from_millis(5));
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let task =
            tokio::spawn(
                async move { worker.run(ShutdownToken::from_receiver(shutdown_rx)).await },
            );

        tokio::task::spawn_blocking(move || started_rx.recv_timeout(Duration::from_secs(2)))
            .await
            .expect("race observer join")
            .expect("provider did not start");
        let moved = TransitPoint {
            latitude: 42.4,
            longitude: -71.2,
        };
        write_vehicle(&persist, &vehicle(moved));
        release_tx.send(()).expect("release provider");

        let topic = transit_state_topic("rig-1");
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Ok(Some(message)) = persist.read_latest(&topic) {
                    let snapshot: TransitSnapshot =
                        serde_json::from_str(message.body.as_deref().unwrap_or_default()).unwrap();
                    if snapshot.query_latitude == moved.latitude {
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("moved-point publication");
        for row in persist.list_since(&topic, None).expect("transit rows") {
            let snapshot: TransitSnapshot =
                serde_json::from_str(row.body.as_deref().unwrap_or_default()).unwrap();
            assert_ne!(snapshot.query_latitude, old_point.latitude);
            assert!(snapshot.vehicles.len() <= MAX_RETAINED_VEHICLES);
        }
        let fetches = probe.fetches.load(Ordering::SeqCst);
        let commits = probe.commits.load(Ordering::SeqCst);
        assert!(fetches >= 2);
        assert!(fetches > commits, "discarded old-point fetch was committed");

        shutdown_tx.send(true).expect("shutdown");
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("prompt shutdown")
            .expect("worker join")
            .expect("worker result");
    }

    #[test]
    fn keyless_mbta_producer_defaults_on_with_explicit_false_opt_out() {
        assert!(overlay_enabled_from_env(None));
        assert!(overlay_enabled_from_env(Some("")));
        assert!(overlay_enabled_from_env(Some("1")));
        assert!(overlay_enabled_from_env(Some("true")));
        assert!(overlay_enabled_from_env(Some("yes")));
        assert!(overlay_enabled_from_env(Some("on")));
        assert!(overlay_enabled_from_env(Some("sure")));
        assert!(!overlay_enabled_from_env(Some("0")));
        assert!(!overlay_enabled_from_env(Some("false")));
        assert!(!overlay_enabled_from_env(Some("NO")));
        assert!(!overlay_enabled_from_env(Some(" off ")));
    }

    #[test]
    fn endpoint_requires_the_canonical_https_mbta_feed() {
        assert!(validate_endpoint(DEFAULT_ENDPOINT).is_ok());
        for hostile in [
            "http://cdn.mbta.com/realtime/VehiclePositions.pb",
            "https://cdn.mbta.com.evil.test/realtime/VehiclePositions.pb",
            "https://user:password@cdn.mbta.com/realtime/VehiclePositions.pb",
            "https://cdn.mbta.com:8443/realtime/VehiclePositions.pb",
            "https://cdn.mbta.com/realtime/Other.pb",
            "https://cdn.mbta.com/realtime/VehiclePositions.pb?token=secret",
        ] {
            assert!(validate_endpoint(hostile).is_err(), "accepted {hostile}");
        }
    }

    #[test]
    fn http_validators_remain_staged_until_publication_commit() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let body = captured_feed();
        let body_len = body.len();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("request");
            let mut request = [0_u8; 2048];
            let _ = stream.read(&mut request);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/x-protobuf\r\nETag: \"feed-v1\"\r\nLast-Modified: Sun, 09 Aug 2026 12:00:00 GMT\r\nContent-Length: {body_len}\r\nConnection: close\r\n\r\n"
            )
            .expect("headers");
            stream.write_all(&body).expect("body");
        });
        let probe =
            MbtaHttpProbe::new_for_test(format!("http://{address}/vehicles.pb")).expect("probe");

        assert!(matches!(
            probe.fetch(point()).expect("fetch"),
            ProbeResponse::Modified(_)
        ));
        {
            let validators = probe.validators.lock().expect("validators");
            assert!(validators.committed.is_none());
            let staged = validators.staged.as_ref().expect("staged validators");
            assert_eq!(staged.etag.as_deref(), Some("\"feed-v1\""));
            assert_eq!(staged.point, point());
        }
        probe.commit(point());
        {
            let validators = probe.validators.lock().expect("validators");
            assert!(validators.staged.is_none());
            let committed = validators.committed.as_ref().expect("committed validators");
            assert_eq!(committed.etag.as_deref(), Some("\"feed-v1\""));
            assert_eq!(committed.point, point());
        }
        server.join().expect("server join");
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
        let probe = MbtaHttpProbe::new_for_test(format!("http://{redirect_addr}/vehicles.pb"))
            .expect("client");
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
        let worker = TransitOverlayWorker::new("rig-1".to_string()).with_bus_root(root.clone());
        let original = build_snapshot("rig-1", point(), &captured_feed(), NOW_MS).expect("parse");
        let mut last = None;
        let mut bus = Persist::open(root.clone()).expect("bus");
        assert!(worker
            .apply_result(
                &mut bus,
                Ok(PreparedResponse::Modified(original)),
                point(),
                &mut last,
            )
            .expect("fresh publication"));
        assert!(!worker
            .apply_result(&mut bus, Err("timeout".to_string()), point(), &mut last,)
            .expect("degraded publication"));
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
        let worker = TransitOverlayWorker::new("rig-1".to_string());
        let original = build_snapshot("rig-1", point(), &captured_feed(), NOW_MS).expect("parse");
        let mut last = Some(original);
        let temp = tempfile::tempdir().expect("bus");
        let mut bus = Persist::open(temp.path().to_path_buf()).expect("persist");
        let moved = TransitPoint {
            latitude: point().latitude + 1.0,
            longitude: point().longitude,
        };
        assert!(!worker
            .apply_result(
                &mut bus,
                Ok(PreparedResponse::NotModified),
                moved,
                &mut last,
            )
            .expect("retraction"));
        assert!(
            last.is_none(),
            "moved query must clear the retained snapshot"
        );
    }

    #[test]
    fn failed_refresh_retracts_retained_vehicles_after_query_point_moves() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().to_path_buf();
        let worker = TransitOverlayWorker::new("rig-1".to_string()).with_bus_root(root.clone());
        let original = build_snapshot("rig-1", point(), &captured_feed(), NOW_MS).expect("parse");
        let mut last = None;
        let mut bus = Persist::open(root.clone()).expect("bus");
        assert!(worker
            .apply_result(
                &mut bus,
                Ok(PreparedResponse::Modified(original)),
                point(),
                &mut last,
            )
            .expect("fresh publication"));
        let moved = TransitPoint {
            latitude: point().latitude + 1.0,
            longitude: point().longitude,
        };

        assert!(!worker
            .apply_result(&mut bus, Err("timeout".to_string()), moved, &mut last,)
            .expect("retraction publication"));
        assert!(last.is_none(), "the old snapshot must not remain in memory");

        let row = Persist::open(root)
            .expect("bus")
            .read_latest(&transit_state_topic("rig-1"))
            .expect("read")
            .expect("clearing snapshot");
        let body = row.body.expect("snapshot body");
        let cleared: TransitSnapshot = serde_json::from_str(&body).expect("decode snapshot");
        assert!(cleared.vehicles.is_empty());
        assert_eq!(cleared.query_latitude, moved.latitude);
        assert_eq!(cleared.query_longitude, moved.longitude);
        assert_eq!(cleared.feed_generated_at_ms, NOW_MS);
        assert!(cleared
            .gaps
            .iter()
            .any(|gap| gap.contains("retained snapshot cleared") && gap.contains("timeout")));
    }

    #[test]
    fn no_fresh_vehicle_fix_publishes_empty_state_before_first_fetch() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().to_path_buf();
        let worker = TransitOverlayWorker::new("rig-1".to_string()).with_bus_root(root.clone());
        let mut private_cache = None;
        let mut bus = Persist::open(root.clone()).expect("bus");

        worker
            .publish_no_context_degraded(
                &mut bus,
                &mut private_cache,
                "MBTA refresh failed: fresh same-host MG90 vehicle fix unavailable",
            )
            .expect("empty publication");

        assert!(private_cache.is_none());
        let snapshot: TransitSnapshot = serde_json::from_str(
            Persist::open(root)
                .expect("bus")
                .read_latest(&transit_state_topic("rig-1"))
                .expect("read")
                .expect("message")
                .body
                .as_deref()
                .expect("body"),
        )
        .expect("degraded snapshot");
        assert_eq!(snapshot.host, "rig-1");
        assert!(snapshot.fetched_at_ms > 0);
        assert_eq!(snapshot.feed_generated_at_ms, 0);
        assert_eq!(snapshot.feed_version, "2.0");
        assert!(snapshot.vehicles.is_empty());
        assert_eq!(snapshot.query_latitude, 0.0);
        assert_eq!(snapshot.query_longitude, 0.0);
        assert!(snapshot
            .gaps
            .iter()
            .any(|gap| gap.contains("vehicle fix unavailable")));
        assert_eq!(snapshot.license_tier, "open-data-attribution");
        assert_eq!(snapshot.attribution, "MassDOT · MBTA");
    }

    #[test]
    fn no_vehicle_fix_degraded_snapshot_clears_stale_bus_row_and_private_cache() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().to_path_buf();
        let seed_worker =
            TransitOverlayWorker::new("rig-1".to_string()).with_bus_root(root.clone());
        let original = build_snapshot("rig-1", point(), &captured_feed(), NOW_MS).expect("parse");
        assert!(!original.vehicles.is_empty());
        let mut seed_cache = None;
        let mut bus = Persist::open(root.clone()).expect("bus");
        assert!(seed_worker
            .apply_result(
                &mut bus,
                Ok(PreparedResponse::Modified(original.clone())),
                point(),
                &mut seed_cache,
            )
            .expect("seed publication"));

        let restarted = TransitOverlayWorker::new("rig-1".to_string()).with_bus_root(root.clone());
        let mut private_cache = Some(original);
        restarted
            .publish_no_context_degraded(
                &mut bus,
                &mut private_cache,
                "MBTA refresh failed: no fresh vehicle fix after restart",
            )
            .expect("empty publication");

        assert!(
            private_cache.is_none(),
            "old vehicle-scoped transit cache must not survive fix loss"
        );
        let snapshot: TransitSnapshot = serde_json::from_str(
            Persist::open(root)
                .expect("bus")
                .read_latest(&transit_state_topic("rig-1"))
                .expect("read")
                .expect("message")
                .body
                .as_deref()
                .expect("body"),
        )
        .expect("degraded snapshot");
        assert!(snapshot.vehicles.is_empty());
        assert_eq!(snapshot.query_latitude, 0.0);
        assert_eq!(snapshot.query_longitude, 0.0);
        assert_eq!(snapshot.feed_generated_at_ms, 0);
        assert!(snapshot
            .gaps
            .iter()
            .any(|gap| gap.contains("after restart")));
        assert_eq!(snapshot.license_tier, "open-data-attribution");
        assert_eq!(snapshot.attribution, "MassDOT · MBTA");
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
        let worker = TransitOverlayWorker::new("rig-1".to_string());
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

    #[tokio::test]
    async fn repeated_no_context_polls_publish_once_and_replacement_retries() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().to_path_buf();
        let mut worker = TransitOverlayWorker::new("rig-1".to_string())
            .with_probe(Arc::new(SlowProbe))
            .with_bus_root(root.clone())
            .with_poll(Duration::from_millis(10));
        let (tx, rx) = tokio::sync::watch::channel(false);
        let token = ShutdownToken::from_receiver(rx);
        let handle = tokio::spawn(async move { worker.run(token).await });

        let topic = transit_state_topic("rig-1");
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let first_bus = Persist::open(root.clone()).expect("bus");
        let body = loop {
            if let Some(row) = first_bus.read_latest(&topic).expect("read") {
                break row.body.expect("body");
            }
            assert!(
                std::time::Instant::now() < deadline,
                "default-on MBTA worker did not publish a degraded no-fix snapshot"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        };

        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(
            first_bus
                .list_since(&topic, None)
                .expect("first Bus rows")
                .len(),
            1,
            "repeated no-context polls appended duplicate snapshots"
        );

        drop(first_bus);
        for suffix in ["", "-wal", "-shm"] {
            let path = PathBuf::from(format!("{}{suffix}", root.join("index.sqlite").display()));
            if let Err(error) = std::fs::remove_file(path) {
                assert_eq!(error.kind(), io::ErrorKind::NotFound);
            }
        }
        let replacement_bus = Persist::open(root.clone()).expect("replacement Bus");
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if replacement_bus
                    .read_latest(&topic)
                    .expect("replacement read")
                    .is_some()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("replacement no-context publication");
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(
            replacement_bus
                .list_since(&topic, None)
                .expect("replacement rows")
                .len(),
            1,
            "replacement Bus received duplicate no-context snapshots"
        );

        tx.send(true).expect("shutdown");
        let joined = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(joined.is_ok(), "worker exits promptly after proof");
        joined
            .expect("join timeout")
            .expect("join")
            .expect("worker ok");

        let snapshot: TransitSnapshot = serde_json::from_str(&body).expect("snapshot");
        assert_eq!(snapshot.host, "rig-1");
        assert!(snapshot.fetched_at_ms > 0);
        assert_eq!(snapshot.feed_generated_at_ms, 0);
        assert!(snapshot.vehicles.is_empty());
        assert_eq!(snapshot.query_latitude, 0.0);
        assert_eq!(snapshot.query_longitude, 0.0);
        assert!(snapshot
            .gaps
            .iter()
            .any(|gap| gap.contains("vehicle fix unavailable")));
        assert_eq!(snapshot.license_tier, "open-data-attribution");
        assert_eq!(snapshot.attribution, "MassDOT · MBTA");
    }
}
