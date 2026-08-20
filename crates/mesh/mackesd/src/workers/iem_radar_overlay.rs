//! WL-FUNC-012 / OVERLAY-2 — keyless IEM/NWS NEXRAD radar-tile adapter.

#![cfg(feature = "async-services")]

use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use mackes_mesh_types::iem_radar::{
    iem_radar_state_topic, IemRadarFrame, IemRadarSnapshot, IemRadarTile,
};
use reqwest::blocking::Client;
use reqwest::header::CONTENT_TYPE;
use serde::Deserialize;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;

use super::{ShutdownToken, Worker};

/// Explicit disable for the keyless default-on IEM/NWS radar producer.
pub const ENABLED_ENV: &str = "MDE_OVERLAY_IEM_RADAR";
/// Exact IEM metadata clock check cadence.
pub const POLL: Duration = Duration::from_secs(60);
const RETRY_MAX: Duration = Duration::from_secs(15 * 60);
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_METADATA_BYTES: usize = 4 * 1024;
const MAX_TILE_BYTES: usize = 96 * 1024;
const MAX_TOTAL_TILE_BYTES: usize = 512 * 1024;
const MAX_FRAMES: u8 = 6;
const MAX_GAPS: usize = 32;
const TILE_ZOOM: u8 = 6;
const TILE_EDGE: u32 = 256;
const CURRENT_MAX_AGE_MS: i64 = 20 * 60 * 1_000;
const MAX_FUTURE_SKEW_MS: i64 = 5 * 60 * 1_000;
const VEHICLE_FIX_MAX_AGE_MS: i64 = 30_000;
const VEHICLE_MAX_FUTURE_SKEW_MS: i64 = 5_000;
const USER_AGENT: &str =
    "Construct/12 mackesd IEM-NEXRAD-overlay (+https://github.com/matthewmackes/magic-mesh)";

/// Fresh finite vehicle point used to select one regional radar tile.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RadarContext {
    /// WGS-84 latitude.
    pub latitude: f64,
    /// WGS-84 longitude.
    pub longitude: f64,
}

/// Injectable metadata/tile seam.
pub trait IemRadarProbe: Send + Sync {
    /// Fetch one exact IEM composite metadata record (`n0q_{index}.json`).
    fn fetch_metadata(&self, index: u8) -> io::Result<String>;
    /// Fetch one local tile for a relative history lane.
    fn fetch_tile(&self, index: u8, z: u8, x: u32, y: u32) -> io::Result<Vec<u8>>;
}

/// Production rustls IEM courtesy-service probe.
pub struct IemRadarHttpProbe;

impl IemRadarHttpProbe {
    fn new() -> Self {
        Self
    }

    fn client() -> io::Result<Client> {
        let client = Client::builder()
            .timeout(HTTP_TIMEOUT)
            .user_agent(USER_AGENT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(io_other)?;
        Ok(client)
    }

    fn get_bounded(&self, url: &str, content_type: &str, cap: usize) -> io::Result<Vec<u8>> {
        validate_official_url(url)?;
        let response = Self::client()?.get(url).send().map_err(io_other)?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(io::Error::other(format!(
                "IEM returned unexpected HTTP {} (redirects are disabled)",
                response.status()
            )));
        }
        let actual_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .split(';')
            .next()
            .unwrap_or_default()
            .trim();
        if actual_type != content_type {
            return Err(io::Error::other(format!(
                "IEM returned unexpected content type {actual_type:?}"
            )));
        }
        if response
            .content_length()
            .is_some_and(|length| length > u64::try_from(cap).unwrap_or(u64::MAX))
        {
            return Err(io::Error::other(format!(
                "IEM response exceeds {cap} byte limit"
            )));
        }
        let mut response = response;
        read_bounded(&mut response, cap)
    }
}

impl IemRadarProbe for IemRadarHttpProbe {
    fn fetch_metadata(&self, index: u8) -> io::Result<String> {
        if index >= MAX_FRAMES {
            return Err(io::Error::other("IEM radar frame index exceeds cap"));
        }
        String::from_utf8(self.get_bounded(
            &metadata_url(index),
            "application/json",
            MAX_METADATA_BYTES,
        )?)
        .map_err(io_other)
    }

    fn fetch_tile(&self, index: u8, z: u8, x: u32, y: u32) -> io::Result<Vec<u8>> {
        if index >= MAX_FRAMES || z != TILE_ZOOM || x >= (1_u32 << z) || y >= (1_u32 << z) {
            return Err(io::Error::other("IEM radar tile coordinates exceed cap"));
        }
        let png = self.get_bounded(&tile_url(index, z, x, y), "image/png", MAX_TILE_BYTES)?;
        validate_png(&png)?;
        Ok(png)
    }
}

fn metadata_url(index: u8) -> String {
    format!("https://mesonet.agron.iastate.edu/data/gis/images/4326/USCOMP/n0q_{index}.json")
}

fn tile_url(index: u8, z: u8, x: u32, y: u32) -> String {
    let layer = if index == 0 {
        "nexrad-n0q-900913".to_string()
    } else {
        format!("nexrad-n0q-900913-m{:02}m", index.saturating_mul(5))
    };
    format!("https://mesonet.agron.iastate.edu/cache/tile.py/1.0.0/{layer}/{z}/{x}/{y}.png")
}

fn validate_official_url(value: &str) -> io::Result<reqwest::Url> {
    let url = reqwest::Url::parse(value).map_err(io_other)?;
    if url.scheme() != "https"
        || url.host_str() != Some("mesonet.agron.iastate.edu")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path().contains("..")
    {
        return Err(io::Error::other(
            "IEM URL is outside the strict official-host allowlist",
        ));
    }
    let path = url.path();
    let extension = Path::new(path)
        .extension()
        .and_then(std::ffi::OsStr::to_str);
    if !(path.starts_with("/data/gis/images/4326/USCOMP/n0q_") && extension == Some("json")
        || path.starts_with("/cache/tile.py/1.0.0/nexrad-n0q-900913") && extension == Some("png"))
    {
        return Err(io::Error::other("IEM URL path is not canonical"));
    }
    Ok(url)
}

fn read_bounded(reader: &mut impl Read, cap: usize) -> io::Result<Vec<u8>> {
    let mut body = Vec::with_capacity(cap.min(32 * 1024));
    reader
        .take(u64::try_from(cap).unwrap_or(u64::MAX) + 1)
        .read_to_end(&mut body)?;
    if body.len() > cap {
        return Err(io::Error::other(format!(
            "IEM response exceeds {cap} byte limit"
        )));
    }
    Ok(body)
}

fn validate_png(png: &[u8]) -> io::Result<()> {
    const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if png.len() < 41
        || !png.starts_with(SIGNATURE)
        || png.get(12..16) != Some(b"IHDR")
        || png.get(16..20).and_then(bytes_u32) != Some(TILE_EDGE)
        || png.get(20..24).and_then(bytes_u32) != Some(TILE_EDGE)
        || png.get(png.len().saturating_sub(8)..png.len().saturating_sub(4)) != Some(b"IEND")
    {
        return Err(io::Error::other("IEM tile is not a complete 256x256 PNG"));
    }
    Ok(())
}

fn bytes_u32(bytes: &[u8]) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.try_into().ok()?))
}

fn io_other(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

#[derive(Debug, Deserialize)]
struct MetadataDocument {
    meta: Metadata,
}

#[derive(Debug, Deserialize)]
struct Metadata {
    product: String,
    site: String,
    valid: String,
    #[serde(default)]
    radar_quorum: Option<String>,
}

#[derive(Debug, Clone)]
struct ParsedMetadata {
    valid_at_ms: i64,
    radar_quorum: Option<String>,
}

fn parse_metadata(body: &str, now_ms: i64, current: bool) -> io::Result<ParsedMetadata> {
    if body.len() > MAX_METADATA_BYTES {
        return Err(io::Error::other("IEM metadata exceeds byte limit"));
    }
    let document: MetadataDocument = serde_json::from_str(body).map_err(io_other)?;
    if document.meta.product != "N0Q" || document.meta.site != "USCOMP" {
        return Err(io::Error::other("IEM metadata product/site mismatch"));
    }
    let valid_at_ms = chrono::DateTime::parse_from_rfc3339(&document.meta.valid)
        .map_err(io_other)?
        .timestamp_millis();
    if current
        && (valid_at_ms > now_ms.saturating_add(MAX_FUTURE_SKEW_MS)
            || now_ms.saturating_sub(valid_at_ms) > CURRENT_MAX_AGE_MS)
    {
        return Err(io::Error::other(
            "IEM current mosaic valid time is future/stale",
        ));
    }
    let radar_quorum = document.meta.radar_quorum.filter(|value| {
        !value.is_empty()
            && value.len() <= 16
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || byte == b'/')
    });
    Ok(ParsedMetadata {
        valid_at_ms,
        radar_quorum,
    })
}

fn build_snapshot(
    probe: &dyn IemRadarProbe,
    host: &str,
    context: RadarContext,
    current_metadata: ParsedMetadata,
    fetched_at_ms: i64,
) -> io::Result<IemRadarSnapshot> {
    validate_context(context)?;
    if current_metadata.valid_at_ms > fetched_at_ms.saturating_add(MAX_FUTURE_SKEW_MS)
        || fetched_at_ms.saturating_sub(current_metadata.valid_at_ms) > CURRENT_MAX_AGE_MS
    {
        return Err(io::Error::other("IEM current mosaic is future/stale"));
    }
    let (x, y) = tile_xyz(context.latitude, context.longitude, TILE_ZOOM)?;
    let mut snapshot =
        IemRadarSnapshot::empty(host, fetched_at_ms, context.latitude, context.longitude);
    let mut total_bytes = 0_usize;
    for index in 0..MAX_FRAMES {
        let metadata = if index == 0 {
            current_metadata.clone()
        } else {
            match probe
                .fetch_metadata(index)
                .and_then(|body| parse_metadata(&body, fetched_at_ms, false))
            {
                Ok(metadata) => metadata,
                Err(error) => {
                    push_gap(
                        &mut snapshot.gaps,
                        format!("radar frame -{}m metadata omitted: {error}", index * 5),
                    );
                    continue;
                }
            }
        };
        let expected_delta = i64::from(index) * 5 * 60 * 1_000;
        let actual_delta = current_metadata
            .valid_at_ms
            .saturating_sub(metadata.valid_at_ms);
        if (actual_delta - expected_delta).abs() > 2 * 60 * 1_000 {
            push_gap(
                &mut snapshot.gaps,
                format!("radar frame -{}m has inconsistent producer time", index * 5),
            );
            continue;
        }
        let tile = match probe.fetch_tile(index, TILE_ZOOM, x, y) {
            Ok(tile) => tile,
            Err(error) if index == 0 => return Err(error),
            Err(error) => {
                push_gap(
                    &mut snapshot.gaps,
                    format!("radar frame -{}m tile omitted: {error}", index * 5),
                );
                continue;
            }
        };
        validate_png(&tile)?;
        if total_bytes.saturating_add(tile.len()) > MAX_TOTAL_TILE_BYTES {
            push_gap(
                &mut snapshot.gaps,
                "radar animation truncated at aggregate tile-byte cap".to_string(),
            );
            break;
        }
        total_bytes += tile.len();
        snapshot.frames.push(IemRadarFrame {
            valid_at_ms: metadata.valid_at_ms,
            nominal_minutes_ago: index * 5,
            radar_quorum: metadata.radar_quorum,
            tiles: vec![IemRadarTile {
                z: TILE_ZOOM,
                x,
                y,
                png_base64: base64::engine::general_purpose::STANDARD.encode(tile),
            }],
        });
    }
    if snapshot
        .frames
        .first()
        .map(|frame| frame.nominal_minutes_ago)
        != Some(0)
    {
        return Err(io::Error::other("IEM current radar frame is unavailable"));
    }
    Ok(snapshot)
}

fn unchanged_snapshot(
    previous: &IemRadarSnapshot,
    context: RadarContext,
    current: &ParsedMetadata,
) -> bool {
    let Ok((x, y)) = tile_xyz(context.latitude, context.longitude, TILE_ZOOM) else {
        return false;
    };
    previous.frames.first().is_some_and(|frame| {
        frame.valid_at_ms == current.valid_at_ms
            && frame
                .tiles
                .first()
                .is_some_and(|tile| tile.z == TILE_ZOOM && tile.x == x && tile.y == y)
    })
}

fn tile_xyz(latitude: f64, longitude: f64, z: u8) -> io::Result<(u32, u32)> {
    if !latitude.is_finite()
        || !longitude.is_finite()
        || !(-85.051_128_78..=85.051_128_78).contains(&latitude)
        || !(-180.0..180.0).contains(&longitude)
        || z > 20
    {
        return Err(io::Error::other("radar tile point is not finite/in range"));
    }
    let n = f64::from(1_u32 << z);
    let latitude_rad = latitude.to_radians();
    let x = ((longitude + 180.0) / 360.0 * n).floor();
    let y = ((1.0 - (latitude_rad.tan() + 1.0 / latitude_rad.cos()).ln() / std::f64::consts::PI)
        / 2.0
        * n)
        .floor();
    if x.is_finite() && y.is_finite() && x >= 0.0 && y >= 0.0 && x < n && y < n {
        Ok((x as u32, y as u32))
    } else {
        Err(io::Error::other("radar tile point does not resolve"))
    }
}

fn validate_context(context: RadarContext) -> io::Result<()> {
    if context.latitude.is_finite()
        && context.longitude.is_finite()
        && (17.0..=72.0).contains(&context.latitude)
        && (-180.0..=-64.0).contains(&context.longitude)
    {
        Ok(())
    } else {
        Err(io::Error::other(
            "fresh vehicle point is outside finite US radar coverage",
        ))
    }
}

fn push_gap(gaps: &mut Vec<String>, gap: String) {
    if gaps.len() < MAX_GAPS {
        gaps.push(gap);
    }
}

enum PreparedResponse {
    Modified(IemRadarSnapshot),
}

trait RadarBus {
    fn read_latest_body(&mut self, topic: &str) -> io::Result<Option<String>>;
    fn publish_snapshot(&mut self, topic: &str, snapshot: &IemRadarSnapshot) -> io::Result<()>;
}

impl RadarBus for Persist {
    fn read_latest_body(&mut self, topic: &str) -> io::Result<Option<String>> {
        self.reopen_if_index_changed();
        self.read_latest(topic)
            .map_err(io_other)?
            .map(|message| {
                message
                    .body
                    .ok_or_else(|| io::Error::other("vehicle Bus row has no body"))
            })
            .transpose()
    }

    fn publish_snapshot(&mut self, topic: &str, snapshot: &IemRadarSnapshot) -> io::Result<()> {
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

/// Workstation-side IEM radar adapter.
pub struct IemRadarOverlayWorker {
    host: String,
    probe: Option<Arc<dyn IemRadarProbe>>,
    bus_root_override: Option<PathBuf>,
    poll: Duration,
}

impl IemRadarOverlayWorker {
    /// Production wiring. The keyless IEM/NWS radar adapter is enabled by
    /// default on spawned Workstation-tier nodes; an explicit false-y env
    /// disables it. Missing vehicle context publishes an honest empty mirror
    /// instead of leaving the catalog topic absent.
    #[must_use]
    pub fn new(host: String) -> Self {
        let probe = if env_default_enabled(ENABLED_ENV) {
            Some(Arc::new(IemRadarHttpProbe::new()) as Arc<dyn IemRadarProbe>)
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
    pub fn with_probe(mut self, probe: Arc<dyn IemRadarProbe>) -> Self {
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

    /// Override cadence for focused recovery tests.
    #[must_use]
    pub const fn with_poll(mut self, poll: Duration) -> Self {
        self.poll = poll;
        self
    }

    fn open_bus(&self) -> io::Result<Persist> {
        let root = iem_radar_bus_root(
            self.bus_root_override.as_deref(),
            crate::bus_publish::default_bus_root(),
        );
        let mut persist = Persist::open(root).map_err(io_other)?;
        persist.reopen_if_index_changed();
        Ok(persist)
    }

    fn current_context(&self, bus: &mut impl RadarBus) -> io::Result<Option<RadarContext>> {
        let topic = mackes_mesh_types::vehicle::vehicle_state_topic(&self.host);
        let Some(body) = bus.read_latest_body(&topic)? else {
            return Ok(None);
        };
        let vehicle: mackes_mesh_types::vehicle::VehicleState =
            serde_json::from_str(&body).map_err(io_other)?;
        validated_vehicle_context(&vehicle, &self.host, now_ms())
    }

    fn publish(&self, bus: &mut impl RadarBus, snapshot: &IemRadarSnapshot) -> io::Result<()> {
        bus.publish_snapshot(&iem_radar_state_topic(&self.host), snapshot)
    }

    fn no_context_snapshot(&self, reason: &str) -> IemRadarSnapshot {
        let mut snapshot = IemRadarSnapshot::empty(&self.host, now_ms(), 0.0, 0.0);
        push_gap(&mut snapshot.gaps, format!("IEM radar paused: {reason}"));
        snapshot
    }

    fn publish_no_context_degraded(
        &self,
        bus: &mut impl RadarBus,
        reason: &str,
        last_good: &mut Option<IemRadarSnapshot>,
    ) -> io::Result<()> {
        let snapshot = self.no_context_snapshot(reason);
        self.publish(bus, &snapshot)?;
        // A missing same-host fix invalidates the previous tile origin. Keep
        // the Bus mirror present and licensed, but do not let later failures
        // replay a tile fetched for an old point as if it still described the
        // vehicle's surroundings.
        *last_good = None;
        Ok(())
    }

    fn ensure_no_context_published(
        &self,
        bus: &mut impl RadarBus,
        last_good: &mut Option<IemRadarSnapshot>,
        no_context_published: &mut bool,
    ) -> io::Result<()> {
        if *no_context_published {
            let topic = iem_radar_state_topic(&self.host);
            let current = bus
                .read_latest_body(&topic)?
                .map(|body| serde_json::from_str::<IemRadarSnapshot>(&body).map_err(io_other))
                .transpose()?;
            if current
                .is_some_and(|snapshot| snapshot.host == self.host && snapshot.frames.is_empty())
            {
                return Ok(());
            }
        }
        self.publish_no_context_degraded(
            bus,
            "fresh same-host US vehicle fix unavailable",
            last_good,
        )?;
        *no_context_published = true;
        Ok(())
    }

    fn current_context_or_clear(
        &self,
        bus: &mut impl RadarBus,
        last_good: &mut Option<IemRadarSnapshot>,
        no_context_published: &mut bool,
    ) -> io::Result<Option<RadarContext>> {
        let context = self.current_context(bus)?;
        if context.is_some() {
            *no_context_published = false;
        } else {
            self.ensure_no_context_published(bus, last_good, no_context_published)?;
        }
        Ok(context)
    }

    fn apply_result(
        &self,
        bus: &mut impl RadarBus,
        result: io::Result<PreparedResponse>,
        last_good: &mut Option<IemRadarSnapshot>,
    ) -> io::Result<bool> {
        match result {
            Ok(PreparedResponse::Modified(snapshot)) => {
                self.publish(bus, &snapshot)?;
                *last_good = Some(snapshot);
                Ok(true)
            }
            Err(error) => {
                if let Some(snapshot) = last_good {
                    let mut paused = snapshot.clone();
                    paused
                        .gaps
                        .retain(|gap| !gap.starts_with("IEM radar paused:"));
                    push_gap(&mut paused.gaps, format!("IEM radar paused: {error}"));
                    self.publish(bus, &paused)?;
                    *snapshot = paused;
                }
                Ok(false)
            }
        }
    }

    async fn fetch_async(
        &self,
        probe: Arc<dyn IemRadarProbe>,
        context: RadarContext,
        previous: Option<IemRadarSnapshot>,
        shutdown: &mut ShutdownToken,
    ) -> Option<io::Result<PreparedResponse>> {
        let host = self.host.clone();
        let task = tokio::task::spawn_blocking(move || {
            let fetched_at_ms = now_ms();
            validate_context(context)?;
            let current_body = probe.fetch_metadata(0)?;
            let current = parse_metadata(&current_body, fetched_at_ms, true)?;
            let snapshot = if previous
                .as_ref()
                .is_some_and(|snapshot| unchanged_snapshot(snapshot, context, &current))
            {
                let mut snapshot = previous.expect("checked above");
                snapshot.fetched_at_ms = fetched_at_ms;
                snapshot.query_latitude = context.latitude;
                snapshot.query_longitude = context.longitude;
                snapshot
                    .gaps
                    .retain(|gap| !gap.starts_with("IEM radar paused:"));
                snapshot
            } else {
                build_snapshot(probe.as_ref(), &host, context, current, fetched_at_ms)?
            };
            Ok(PreparedResponse::Modified(snapshot))
        });
        tokio::select! {
            () = shutdown.wait() => None,
            joined = task => Some(match joined {
                Ok(result) => result,
                Err(error) => Err(io::Error::other(format!("IEM radar fetch task failed: {error}"))),
            }),
        }
    }
}

fn iem_radar_bus_root(explicit: Option<&Path>, current: Option<PathBuf>) -> PathBuf {
    explicit
        .map(Path::to_path_buf)
        .or(current)
        .unwrap_or_else(|| PathBuf::from(mde_bus::SYSTEM_BUS_ROOT))
}

fn validated_vehicle_context(
    vehicle: &mackes_mesh_types::vehicle::VehicleState,
    expected_host: &str,
    now: i64,
) -> io::Result<Option<RadarContext>> {
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
        || !gps.age_s.is_finite()
        || gps.age_s < 0.0
        || !(-90.0..=90.0).contains(&gps.latitude)
        || !(-180.0..=180.0).contains(&gps.longitude)
    {
        return Err(io::Error::other("vehicle Bus row contains an invalid fix"));
    }
    if future_skew > VEHICLE_MAX_FUTURE_SKEW_MS
        || mirror_age as f64 + f64::from(gps.age_s) * 1_000.0 > VEHICLE_FIX_MAX_AGE_MS as f64
    {
        return Ok(None);
    }
    let context = RadarContext {
        latitude: gps.latitude,
        longitude: gps.longitude,
    };
    if !(17.0..=72.0).contains(&context.latitude) || !(-180.0..=-64.0).contains(&context.longitude)
    {
        return Ok(None);
    }
    validate_context(context)?;
    Ok(Some(context))
}

#[async_trait::async_trait]
impl Worker for IemRadarOverlayWorker {
    fn name(&self) -> &'static str {
        "iem_radar_overlay"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let Some(probe) = self.probe.clone() else {
            shutdown.wait().await;
            return Ok(());
        };
        let mut last_good = None;
        let mut no_context_published = false;
        let mut retry = self.poll;
        loop {
            let context = match self.open_bus().and_then(|mut bus| {
                self.current_context_or_clear(&mut bus, &mut last_good, &mut no_context_published)
            }) {
                Ok(Some(context)) => context,
                Ok(None) => {
                    retry = self.poll;
                    tokio::select! {
                        () = shutdown.wait() => break,
                        () = tokio::time::sleep(self.poll) => {}
                    }
                    continue;
                }
                Err(error) => {
                    tracing::warn!(
                        target: "mackesd::iem_radar_overlay",
                        %error,
                        "IEM radar vehicle context transaction deferred"
                    );
                    let delay = retry;
                    retry = retry.saturating_mul(2).min(RETRY_MAX);
                    tokio::select! {
                        () = shutdown.wait() => break,
                        () = tokio::time::sleep(delay) => {}
                    }
                    continue;
                }
            };
            let Some(result) = self
                .fetch_async(probe.clone(), context, last_good.clone(), &mut shutdown)
                .await
            else {
                break;
            };
            // Metadata/tile I/O runs off-thread without holding the Bus. Re-open
            // and re-read the exact vehicle authority before publication so
            // movement, fix loss, or index replacement cannot admit old tiles.
            let success = match self.open_bus().and_then(|mut bus| {
                let latest = self.current_context_or_clear(
                    &mut bus,
                    &mut last_good,
                    &mut no_context_published,
                )?;
                if latest != Some(context) {
                    return Ok(false);
                }
                self.apply_result(&mut bus, result, &mut last_good)
            }) {
                Ok(success) => success,
                Err(error) => {
                    tracing::warn!(
                        target: "mackesd::iem_radar_overlay",
                        %error,
                        "IEM radar refresh transaction deferred"
                    );
                    false
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
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{mpsc, Mutex};

    use mackes_mesh_types::vehicle::{GpsFix, VehicleState};
    use mde_bus::persist::Persist;

    use super::*;

    const NOW_MS: i64 = 1_784_757_600_000;
    const CURRENT_METADATA: &str = r#"{"meta":{"vcp":null,"product":"N0Q","site":"USCOMP","valid":"2026-07-22T22:00:00Z","processing_time_secs":101,"radar_quorum":"143/147"}}"#;

    fn context() -> RadarContext {
        RadarContext {
            latitude: 42.3601,
            longitude: -71.0589,
        }
    }

    fn metadata(index: u8) -> String {
        let valid = chrono::DateTime::from_timestamp_millis(NOW_MS - i64::from(index) * 300_000)
            .expect("time")
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        format!(
            r#"{{"meta":{{"product":"N0Q","site":"USCOMP","valid":"{valid}","radar_quorum":"143/147"}}}}"#
        )
    }

    fn live_metadata(index: u8) -> String {
        let valid = chrono::DateTime::from_timestamp_millis(
            now_ms().saturating_sub(i64::from(index) * 300_000),
        )
        .expect("time")
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        format!(
            r#"{{"meta":{{"product":"N0Q","site":"USCOMP","valid":"{valid}","radar_quorum":"143/147"}}}}"#
        )
    }

    fn png() -> Vec<u8> {
        let mut png = vec![0_u8; 41];
        png[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        png[12..16].copy_from_slice(b"IHDR");
        png[16..20].copy_from_slice(&TILE_EDGE.to_be_bytes());
        png[20..24].copy_from_slice(&TILE_EDGE.to_be_bytes());
        png[33..37].copy_from_slice(b"IEND");
        png
    }

    #[derive(Default)]
    struct FakeProbe {
        tile_calls: AtomicUsize,
    }

    impl IemRadarProbe for FakeProbe {
        fn fetch_metadata(&self, index: u8) -> io::Result<String> {
            Ok(metadata(index))
        }

        fn fetch_tile(&self, _index: u8, _z: u8, _x: u32, _y: u32) -> io::Result<Vec<u8>> {
            self.tile_calls.fetch_add(1, Ordering::Relaxed);
            Ok(png())
        }
    }

    struct LiveProbe;

    impl IemRadarProbe for LiveProbe {
        fn fetch_metadata(&self, index: u8) -> io::Result<String> {
            Ok(live_metadata(index))
        }

        fn fetch_tile(&self, _index: u8, _z: u8, _x: u32, _y: u32) -> io::Result<Vec<u8>> {
            Ok(png())
        }
    }

    struct RaceProbe {
        started: Mutex<Option<mpsc::Sender<()>>>,
        release: Mutex<Option<mpsc::Receiver<()>>>,
    }

    impl IemRadarProbe for RaceProbe {
        fn fetch_metadata(&self, index: u8) -> io::Result<String> {
            if index == 0 {
                let started = self.started.lock().unwrap().take();
                if let Some(started) = started {
                    started
                        .send(())
                        .map_err(|_| io::Error::other("race observer dropped"))?;
                    let release = {
                        let mut release = self.release.lock().unwrap();
                        release
                            .take()
                            .ok_or_else(|| io::Error::other("race release missing"))?
                    };
                    release
                        .recv()
                        .map_err(|_| io::Error::other("race release dropped"))?;
                }
            }
            Ok(live_metadata(index))
        }

        fn fetch_tile(&self, _index: u8, _z: u8, _x: u32, _y: u32) -> io::Result<Vec<u8>> {
            Ok(png())
        }
    }

    fn vehicle(latitude: f64, longitude: f64) -> VehicleState {
        let mut vehicle = VehicleState::offline("rig-1");
        vehicle.online = true;
        vehicle.published_at_ms = now_ms();
        vehicle.gps = GpsFix {
            fix_type: "gps".to_string(),
            latitude,
            longitude,
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
                Some(&serde_json::to_string(vehicle).unwrap()),
            )
            .expect("vehicle publication");
    }

    #[derive(Default)]
    struct FixtureBus {
        body: Option<String>,
        output_body: Option<String>,
        read_fails: bool,
        failed_writes_remaining: usize,
        published: Vec<IemRadarSnapshot>,
    }

    impl RadarBus for FixtureBus {
        fn read_latest_body(&mut self, topic: &str) -> io::Result<Option<String>> {
            if self.read_fails {
                Err(io::Error::other("injected vehicle read failure"))
            } else if topic == iem_radar_state_topic("rig-1") {
                Ok(self.output_body.clone())
            } else {
                Ok(self.body.clone())
            }
        }

        fn publish_snapshot(
            &mut self,
            _topic: &str,
            snapshot: &IemRadarSnapshot,
        ) -> io::Result<()> {
            if self.failed_writes_remaining > 0 {
                self.failed_writes_remaining -= 1;
                return Err(io::Error::other("injected radar write failure"));
            }
            self.output_body = Some(serde_json::to_string(snapshot).expect("fixture snapshot"));
            self.published.push(snapshot.clone());
            Ok(())
        }
    }

    #[test]
    fn captured_live_metadata_builds_six_exact_bounded_frames() {
        let parsed = parse_metadata(CURRENT_METADATA, NOW_MS, true).expect("metadata");
        assert_eq!(parsed.radar_quorum.as_deref(), Some("143/147"));
        let probe = FakeProbe::default();
        let snapshot =
            build_snapshot(&probe, "rig-1", context(), parsed, NOW_MS).expect("snapshot");
        assert_eq!(snapshot.frames.len(), usize::from(MAX_FRAMES));
        assert_eq!(snapshot.frames[0].nominal_minutes_ago, 0);
        assert_eq!(snapshot.frames[5].nominal_minutes_ago, 25);
        assert_eq!(probe.tile_calls.load(Ordering::Relaxed), 6);
        assert!(snapshot.frames.iter().all(|frame| frame.tiles.len() == 1));
    }

    #[test]
    fn metadata_png_coordinates_and_urls_are_strictly_bounded() {
        assert!(parse_metadata(&"x".repeat(MAX_METADATA_BYTES + 1), NOW_MS, true).is_err());
        assert!(parse_metadata(
            r#"{"meta":{"product":"N0Q","site":"evil","valid":"2026-07-22T22:00:00Z"}}"#,
            NOW_MS,
            true
        )
        .is_err());
        assert!(parse_metadata(CURRENT_METADATA, NOW_MS + CURRENT_MAX_AGE_MS + 1, true).is_err());
        assert!(validate_png(&png()).is_ok());
        let mut wrong = png();
        wrong[16..20].copy_from_slice(&512_u32.to_be_bytes());
        assert!(validate_png(&wrong).is_err());
        assert_eq!(tile_xyz(42.3601, -71.0589, 6).expect("tile"), (19, 23));
        assert!(tile_xyz(f64::NAN, -71.0, 6).is_err());
        assert!(validate_official_url(&metadata_url(0)).is_ok());
        assert!(validate_official_url(&tile_url(0, 6, 19, 23)).is_ok());
        for hostile in [
            "http://mesonet.agron.iastate.edu/data/gis/images/4326/USCOMP/n0q_0.json",
            "https://mesonet.agron.iastate.edu.evil.test/data/gis/images/4326/USCOMP/n0q_0.json",
            "https://mesonet.agron.iastate.edu:444/data/gis/images/4326/USCOMP/n0q_0.json",
            "https://mesonet.agron.iastate.edu/cache/tile.py/1.0.0/other/6/19/23.png",
        ] {
            assert!(
                validate_official_url(hostile).is_err(),
                "accepted {hostile}"
            );
        }
    }

    #[test]
    fn vehicle_context_requires_fresh_same_host_us_fix() {
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
        assert!(validated_vehicle_context(&vehicle, "rig-1", 110_000)
            .unwrap()
            .is_some());
        assert!(validated_vehicle_context(&vehicle, "other", 110_000).is_err());
        vehicle.gps.latitude = f64::NAN;
        assert!(validated_vehicle_context(&vehicle, "rig-1", 100_000).is_err());
        vehicle.gps.latitude = 0.0;
        vehicle.gps.longitude = 0.0;
        assert!(validated_vehicle_context(&vehicle, "rig-1", 100_000)
            .unwrap()
            .is_none());
    }

    #[test]
    fn keyless_iem_radar_producer_defaults_on_with_explicit_false_opt_out() {
        assert!(overlay_enabled_from_env(None));
        assert!(overlay_enabled_from_env(Some("")));
        assert!(overlay_enabled_from_env(Some("1")));
        assert!(overlay_enabled_from_env(Some("true")));
        assert!(overlay_enabled_from_env(Some("yes")));
        assert!(overlay_enabled_from_env(Some("on")));
        assert!(!overlay_enabled_from_env(Some("0")));
        assert!(!overlay_enabled_from_env(Some("false")));
        assert!(!overlay_enabled_from_env(Some("NO")));
        assert!(!overlay_enabled_from_env(Some(" off ")));
    }

    #[test]
    fn unchanged_producer_frame_reuses_tiles_without_refetching() {
        let probe = FakeProbe::default();
        let current = parse_metadata(&metadata(0), NOW_MS, true).expect("metadata");
        let snapshot =
            build_snapshot(&probe, "rig-1", context(), current.clone(), NOW_MS).expect("snapshot");
        assert_eq!(probe.tile_calls.load(Ordering::Relaxed), 6);
        assert!(unchanged_snapshot(&snapshot, context(), &current));
        let moved = RadarContext {
            latitude: 42.3601,
            longitude: -120.0,
        };
        assert!(!unchanged_snapshot(&snapshot, moved, &current));
    }

    #[test]
    fn failed_refresh_retains_producer_time_and_publishes_paused_state() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().to_path_buf();
        let worker = IemRadarOverlayWorker::new("rig-1".to_string()).with_bus_root(root.clone());
        let probe = FakeProbe::default();
        let current = parse_metadata(&metadata(0), NOW_MS, true).expect("metadata");
        let original =
            build_snapshot(&probe, "rig-1", context(), current, NOW_MS).expect("snapshot");
        let valid = original.frames[0].valid_at_ms;
        let mut last_good = None;
        let mut bus = Persist::open(root.clone()).expect("bus");
        assert!(worker
            .apply_result(
                &mut bus,
                Ok(PreparedResponse::Modified(original)),
                &mut last_good,
            )
            .expect("fresh publication"));
        assert!(!worker
            .apply_result(&mut bus, Err(io::Error::other("timeout")), &mut last_good,)
            .expect("degraded publication"));
        let retained = last_good.as_ref().expect("retained");
        assert_eq!(retained.frames[0].valid_at_ms, valid);
        assert!(retained
            .gaps
            .iter()
            .any(|gap| gap.starts_with("IEM radar paused:")));
        let persist = Persist::open(root).expect("bus");
        assert_eq!(
            persist
                .list_since(&iem_radar_state_topic("rig-1"), None)
                .expect("rows")
                .len(),
            2
        );
    }

    #[test]
    fn no_vehicle_fix_degraded_snapshot_is_present_and_retracts_prior_frames() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().to_path_buf();
        let worker = IemRadarOverlayWorker::new("rig-1".to_string()).with_bus_root(root.clone());
        let probe = FakeProbe::default();
        let current = parse_metadata(&metadata(0), NOW_MS, true).expect("metadata");
        let original =
            build_snapshot(&probe, "rig-1", context(), current, NOW_MS).expect("snapshot");
        assert!(!original.frames.is_empty());
        let mut last_good = Some(original);

        let mut bus = Persist::open(root.clone()).expect("bus");
        worker
            .publish_no_context_degraded(
                &mut bus,
                "fresh same-host US vehicle fix unavailable",
                &mut last_good,
            )
            .expect("durable empty publication");

        assert!(
            last_good.is_none(),
            "old vehicle-scoped radar tiles must not survive fix loss"
        );
        let body = Persist::open(root)
            .expect("bus")
            .read_latest(&iem_radar_state_topic("rig-1"))
            .expect("read")
            .expect("row")
            .body
            .expect("body");
        let snapshot: IemRadarSnapshot = serde_json::from_str(&body).expect("snapshot");
        assert_eq!(snapshot.host, "rig-1");
        assert!(snapshot.fetched_at_ms > 0);
        assert!(snapshot.frames.is_empty());
        assert_eq!(snapshot.query_latitude, 0.0);
        assert_eq!(snapshot.query_longitude, 0.0);
        assert!(snapshot
            .gaps
            .iter()
            .any(|gap| gap.contains("vehicle fix unavailable")));
        assert_eq!(snapshot.license_tier, "public-domain-courtesy-attribution");
        assert_eq!(snapshot.attribution, "IEM / NOAA NWS NEXRAD");
    }

    #[test]
    fn vehicle_read_or_decode_failure_is_effect_free() {
        let worker = IemRadarOverlayWorker::new("rig-1".to_string());
        let probe = FakeProbe::default();
        let current = parse_metadata(&metadata(0), NOW_MS, true).expect("metadata");
        let original = build_snapshot(&probe, "rig-1", context(), current, NOW_MS)
            .expect("last-good snapshot");

        for mut bus in [
            FixtureBus {
                read_fails: true,
                ..FixtureBus::default()
            },
            FixtureBus {
                body: Some("{not vehicle json".to_string()),
                ..FixtureBus::default()
            },
        ] {
            let mut last_good = Some(original.clone());
            let mut no_context_published = false;
            assert!(worker
                .current_context_or_clear(&mut bus, &mut last_good, &mut no_context_published,)
                .is_err());
            assert_eq!(last_good.as_ref(), Some(&original));
            assert!(!no_context_published);
            assert!(bus.published.is_empty());
        }
    }

    #[test]
    fn failed_publication_retains_state_and_corrects_forward() {
        let worker = IemRadarOverlayWorker::new("rig-1".to_string());
        let previous = IemRadarSnapshot::empty("rig-1", NOW_MS - 1, 40.0, -72.0);
        let probe = FakeProbe::default();
        let current = parse_metadata(&metadata(0), NOW_MS, true).expect("metadata");
        let replacement = build_snapshot(&probe, "rig-1", context(), current, NOW_MS)
            .expect("replacement snapshot");
        assert_eq!(replacement.frames.len(), usize::from(MAX_FRAMES));
        let mut last_good = Some(previous.clone());
        let mut bus = FixtureBus {
            failed_writes_remaining: 1,
            ..FixtureBus::default()
        };

        assert!(worker
            .apply_result(
                &mut bus,
                Ok(PreparedResponse::Modified(replacement.clone())),
                &mut last_good,
            )
            .is_err());
        assert_eq!(last_good.as_ref(), Some(&previous));
        assert!(bus.published.is_empty());

        assert!(worker
            .apply_result(
                &mut bus,
                Ok(PreparedResponse::Modified(replacement.clone())),
                &mut last_good,
            )
            .expect("corrected-forward publication"));
        assert_eq!(last_good.as_ref(), Some(&replacement));
        assert_eq!(bus.published, vec![replacement.clone()]);

        bus.body = None;
        bus.failed_writes_remaining = 1;
        let mut no_context_published = false;
        assert!(worker
            .current_context_or_clear(&mut bus, &mut last_good, &mut no_context_published,)
            .is_err());
        assert_eq!(last_good.as_ref(), Some(&replacement));
        assert!(!no_context_published);
        assert_eq!(bus.published.len(), 1);
        assert!(worker
            .current_context_or_clear(&mut bus, &mut last_good, &mut no_context_published,)
            .expect("corrected-forward empty publication")
            .is_none());
        assert!(last_good.is_none());
        assert!(no_context_published);
        assert!(bus.published.last().unwrap().frames.is_empty());

        let rows_after_transition = bus.published.len();
        assert!(worker
            .current_context_or_clear(&mut bus, &mut last_good, &mut no_context_published,)
            .expect("suppressed repeated no-context poll")
            .is_none());
        assert_eq!(bus.published.len(), rows_after_transition);

        bus.output_body = None;
        assert!(worker
            .current_context_or_clear(&mut bus, &mut last_good, &mut no_context_published,)
            .expect("replacement Bus empty publication")
            .is_none());
        assert_eq!(bus.published.len(), rows_after_transition + 1);
    }

    #[tokio::test]
    async fn late_and_replaced_bus_recovers_in_the_same_worker() {
        assert_eq!(
            iem_radar_bus_root(None, None),
            PathBuf::from(mde_bus::SYSTEM_BUS_ROOT)
        );
        let root = tempfile::tempdir().expect("root");
        let bus_root = root.path().join("bus");
        std::fs::write(&bus_root, b"blocks Persist::open").expect("late Bus blocker");
        let mut worker = IemRadarOverlayWorker::new("rig-1".to_string())
            .with_probe(Arc::new(LiveProbe))
            .with_bus_root(bus_root.clone())
            .with_poll(Duration::from_millis(10));
        let (tx, rx) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(async move { worker.run(ShutdownToken::from_receiver(rx)).await });

        tokio::time::sleep(Duration::from_millis(35)).await;
        assert!(!task.is_finished(), "late Bus terminated the worker");
        std::fs::remove_file(&bus_root).expect("unblock Bus root");
        let first_bus = Persist::open(bus_root.clone()).expect("activate Bus");
        write_vehicle(&first_bus, &vehicle(42.3601, -71.0589));
        let topic = iem_radar_state_topic("rig-1");
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Ok(Some(message)) = first_bus.read_latest(&topic) {
                    let snapshot: IemRadarSnapshot =
                        serde_json::from_str(message.body.as_deref().unwrap_or_default()).unwrap();
                    if snapshot.query_latitude == 42.3601 {
                        assert!(snapshot.frames.len() <= usize::from(MAX_FRAMES));
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
        write_vehicle(&replacement_bus, &vehicle(41.0, -72.0));
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Ok(Some(message)) = replacement_bus.read_latest(&topic) {
                    let snapshot: IemRadarSnapshot =
                        serde_json::from_str(message.body.as_deref().unwrap_or_default()).unwrap();
                    if snapshot.query_latitude == 41.0 {
                        assert!(snapshot.frames.len() <= usize::from(MAX_FRAMES));
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("replacement Bus publication");

        tx.send(true).expect("shutdown");
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("prompt shutdown")
            .expect("worker join")
            .expect("worker result");
    }

    #[tokio::test]
    async fn post_fetch_context_race_discards_old_tiles_and_retracts_on_loss() {
        let bus = tempfile::tempdir().expect("bus");
        let persist = Persist::open(bus.path().to_path_buf()).expect("persist");
        let old_context = vehicle(42.3601, -71.0589);
        write_vehicle(&persist, &old_context);
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let probe = Arc::new(RaceProbe {
            started: Mutex::new(Some(started_tx)),
            release: Mutex::new(Some(release_rx)),
        });
        let mut worker = IemRadarOverlayWorker::new("rig-1".to_string())
            .with_probe(probe)
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
        write_vehicle(&persist, &vehicle(41.0, -72.0));
        release_tx.send(()).expect("release provider");

        let topic = iem_radar_state_topic("rig-1");
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Ok(Some(message)) = persist.read_latest(&topic) {
                    let snapshot: IemRadarSnapshot =
                        serde_json::from_str(message.body.as_deref().unwrap_or_default()).unwrap();
                    if snapshot.query_latitude == 41.0 && !snapshot.frames.is_empty() {
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("moved-context publication");
        let rows = persist.list_since(&topic, None).expect("radar rows");
        assert!(!rows.is_empty());
        for row in rows {
            let snapshot: IemRadarSnapshot =
                serde_json::from_str(row.body.as_deref().unwrap_or_default()).unwrap();
            assert_ne!(snapshot.query_latitude, old_context.gps.latitude);
            assert!(snapshot.frames.len() <= usize::from(MAX_FRAMES));
        }

        let mut lost = VehicleState::offline("rig-1");
        lost.published_at_ms = now_ms();
        write_vehicle(&persist, &lost);
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Ok(Some(message)) = persist.read_latest(&topic) {
                    let snapshot: IemRadarSnapshot =
                        serde_json::from_str(message.body.as_deref().unwrap_or_default()).unwrap();
                    if snapshot.frames.is_empty() {
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("lost-context retraction");

        shutdown_tx.send(true).expect("shutdown");
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("prompt shutdown")
            .expect("worker join")
            .expect("worker result");
    }

    #[test]
    fn http_client_refuses_redirects_before_contacting_target() {
        let target = TcpListener::bind("127.0.0.1:0").expect("target");
        target.set_nonblocking(true).expect("nonblocking");
        let target_addr = target.local_addr().expect("target addr");
        let contacted = Arc::new(AtomicUsize::new(0));
        let contacted_thread = contacted.clone();
        let target_thread = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_millis(400);
            while std::time::Instant::now() < deadline {
                match target.accept() {
                    Ok(_) => {
                        contacted_thread.store(1, Ordering::Relaxed);
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
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("client");
        let response = client
            .get(format!("http://{redirect_addr}/tile"))
            .send()
            .expect("response");
        assert_eq!(response.status(), reqwest::StatusCode::FOUND);
        redirect_thread.join().expect("redirect join");
        target_thread.join().expect("target join");
        assert_eq!(contacted.load(Ordering::Relaxed), 0);
    }

    struct SlowProbe;

    impl IemRadarProbe for SlowProbe {
        fn fetch_metadata(&self, _index: u8) -> io::Result<String> {
            std::thread::sleep(Duration::from_millis(500));
            Ok(metadata(0))
        }

        fn fetch_tile(&self, _index: u8, _z: u8, _x: u32, _y: u32) -> io::Result<Vec<u8>> {
            Ok(png())
        }
    }

    #[tokio::test]
    async fn shutdown_wins_while_metadata_is_in_flight() {
        let worker = IemRadarOverlayWorker::new("rig-1".to_string());
        let (tx, rx) = tokio::sync::watch::channel(false);
        let mut shutdown = ShutdownToken::from_receiver(rx);
        let sender = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            tx.send(true).expect("shutdown");
        });
        let result = tokio::time::timeout(
            Duration::from_millis(200),
            worker.fetch_async(Arc::new(SlowProbe), context(), None, &mut shutdown),
        )
        .await
        .expect("responsive");
        assert!(result.is_none());
        sender.await.expect("sender");
    }

    #[tokio::test]
    async fn repeated_no_fix_polls_publish_once_and_replacement_retries() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().to_path_buf();
        let mut worker = IemRadarOverlayWorker::new("rig-1".to_string())
            .with_probe(Arc::new(FakeProbe::default()))
            .with_bus_root(root.clone())
            .with_poll(Duration::from_millis(10));
        let (tx, rx) = tokio::sync::watch::channel(false);
        let token = ShutdownToken::from_receiver(rx);
        let handle = tokio::spawn(async move { worker.run(token).await });

        let topic = iem_radar_state_topic("rig-1");
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let first_bus = Persist::open(root.clone()).expect("bus");
        let body = loop {
            if let Some(row) = first_bus.read_latest(&topic).expect("read") {
                break row.body.expect("body");
            }
            assert!(
                std::time::Instant::now() < deadline,
                "default-on IEM radar worker did not publish a degraded no-fix snapshot"
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
            "repeated no-fix polls appended duplicate empty snapshots"
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
        .expect("replacement Bus no-context publication");
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(
            replacement_bus
                .list_since(&topic, None)
                .expect("replacement rows")
                .len(),
            1,
            "replacement Bus received repeated empty snapshots"
        );

        tx.send(true).expect("shutdown");
        let joined = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(joined.is_ok(), "worker exits promptly after proof");
        joined
            .expect("join timeout")
            .expect("join")
            .expect("worker ok");

        let snapshot: IemRadarSnapshot = serde_json::from_str(&body).expect("snapshot");
        assert_eq!(snapshot.host, "rig-1");
        assert!(snapshot.fetched_at_ms > 0);
        assert!(snapshot.frames.is_empty());
        assert!(snapshot
            .gaps
            .iter()
            .any(|gap| gap.contains("vehicle fix unavailable")));
        assert_eq!(snapshot.license_tier, "public-domain-courtesy-attribution");
        assert_eq!(snapshot.attribution, "IEM / NOAA NWS NEXRAD");
    }
}
