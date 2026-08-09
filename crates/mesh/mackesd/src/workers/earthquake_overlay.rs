//! WL-FUNC-012 / OVERLAY-10 — keyless USGS earthquake overlay adapter.
//!
//! Workstation-tier nodes start this keyless, low-bandwidth producer by default.
//! The worker polls the official USGS all-hour GeoJSON feed once per minute,
//! normalizes it into the shared wire contract, and publishes a latest-wins
//! `state/overlay/usgs-earthquakes/<node>` snapshot. Operators can explicitly
//! disable the producer with `MDE_OVERLAY_USGS_EARTHQUAKES=0/false/no/off`.
//! Fetch failures retain the last-good snapshot and its original `fetched_at_ms`,
//! adding an honest gap so the Maps layer ages it to stale.

#![cfg(feature = "async-services")]

use std::io::{self, Read};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mackes_mesh_types::earthquake::{
    earthquake_state_topic, EarthquakeEvent, EarthquakeSnapshot, PagerAlert,
};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use reqwest::blocking::Client;
use reqwest::header::{ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED};
use serde::Deserialize;

use super::{ShutdownToken, Worker};

/// Explicit disable for the keyless default-on USGS producer.
pub const ENABLED_ENV: &str = "MDE_OVERLAY_USGS_EARTHQUAKES";
/// Optional endpoint override, primarily for an operator-controlled mirror.
pub const ENDPOINT_ENV: &str = "MDE_OVERLAY_USGS_EARTHQUAKES_URL";
/// Official keyless USGS all-earthquakes, past-hour GeoJSON summary.
pub const DEFAULT_ENDPOINT: &str =
    "https://earthquake.usgs.gov/earthquakes/feed/v1.0/summary/all_hour.geojson";
/// Feed cache cadence documented by USGS (`Cache-Control: max-age=60`).
pub const POLL: Duration = Duration::from_secs(60);
const RETRY_MIN: Duration = Duration::from_secs(5);
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
/// The official all-hour payload is normally only a few KiB. Two MiB leaves
/// ample incident-spike headroom while refusing an unbounded hostile response.
const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
const USER_AGENT: &str =
    "Construct/12 mackesd USGS-earthquake-overlay (+https://github.com/matthewmackes/magic-mesh)";

/// Result of one conditional probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeResponse {
    /// HTTP 200 with a complete GeoJSON body.
    Modified(String),
    /// HTTP 304; the worker should refresh validation time on its last snapshot.
    NotModified,
}

/// Injectable HTTP seam. Tests use captured payloads and never hit the network.
pub trait EarthquakeProbe: Send + Sync {
    /// Fetch or conditionally validate the current USGS feed.
    fn fetch(&self) -> io::Result<ProbeResponse>;

    /// Forget conditional validators unless a fresh response transaction was
    /// published. The next request must fetch a complete body so a 304 cannot
    /// strand rejected or unpublished data outside last-good authority.
    fn publication_deferred(&self) {}
}

#[derive(Debug, Default)]
struct Validators {
    etag: Option<String>,
    last_modified: Option<String>,
}

/// Production rustls HTTP probe with ETag/Last-Modified conditional requests.
pub struct UsgsHttpProbe {
    endpoint: String,
    validators: Mutex<Validators>,
}

impl UsgsHttpProbe {
    fn new(endpoint: String) -> io::Result<Self> {
        Ok(Self {
            endpoint,
            validators: Mutex::new(Validators::default()),
        })
    }
}

impl EarthquakeProbe for UsgsHttpProbe {
    fn fetch(&self) -> io::Result<ProbeResponse> {
        // `reqwest::blocking::Client` owns an internal runtime that must be
        // dropped outside Tokio async contexts. Build and drop it inside this
        // blocking fetch call; `poll_once_async` already runs `fetch()` via
        // `spawn_blocking`, while sync tests call it outside an async runtime.
        let client = Client::builder()
            .timeout(HTTP_TIMEOUT)
            .user_agent(USER_AGENT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(io_other)?;
        let mut request = client.get(&self.endpoint);
        let mut sent_validator = false;
        {
            let validators = self
                .validators
                .lock()
                .map_err(|_| io::Error::other("USGS validator lock poisoned"))?;
            if let Some(value) = &validators.etag {
                request = request.header(IF_NONE_MATCH, value);
                sent_validator = true;
            }
            if let Some(value) = &validators.last_modified {
                request = request.header(IF_MODIFIED_SINCE, value);
                sent_validator = true;
            }
        }

        let response = request.send().map_err(io_other)?;
        if response.status() == reqwest::StatusCode::NOT_MODIFIED {
            return accept_not_modified(sent_validator);
        }
        if !response.status().is_success() {
            return Err(io::Error::other(format!(
                "USGS returned unexpected HTTP {} (redirects are disabled)",
                response.status()
            )));
        }
        let mut response = response.error_for_status().map_err(io_other)?;
        let etag = response
            .headers()
            .get(ETAG)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let last_modified = response
            .headers()
            .get(LAST_MODIFIED)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let body = read_bounded_body(&mut response, MAX_BODY_BYTES)?;
        let mut validators = self
            .validators
            .lock()
            .map_err(|_| io::Error::other("USGS validator lock poisoned"))?;
        validators.etag = etag;
        validators.last_modified = last_modified;
        Ok(ProbeResponse::Modified(body))
    }

    fn publication_deferred(&self) {
        match self.validators.lock() {
            Ok(mut validators) => *validators = Validators::default(),
            Err(error) => tracing::warn!(
                target: "mackesd::earthquake_overlay",
                %error,
                "USGS validator reset failed after deferred publication"
            ),
        }
    }
}

fn accept_not_modified(sent_validator: bool) -> io::Result<ProbeResponse> {
    if sent_validator {
        Ok(ProbeResponse::NotModified)
    } else {
        Err(io::Error::other(
            "USGS returned 304 although the request sent no validator",
        ))
    }
}

fn read_bounded_body(response: &mut impl Read, max_bytes: usize) -> io::Result<String> {
    let limit = u64::try_from(max_bytes)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let mut bytes = Vec::with_capacity(max_bytes.min(64 * 1024));
    response.take(limit).read_to_end(&mut bytes)?;
    if bytes.len() > max_bytes {
        return Err(io::Error::other(format!(
            "USGS response exceeds {max_bytes} byte limit"
        )));
    }
    String::from_utf8(bytes).map_err(io_other)
}

fn io_other(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

#[derive(Debug, Deserialize)]
struct Feed {
    #[serde(default)]
    metadata: Metadata,
    #[serde(default)]
    features: Vec<Feature>,
}

#[derive(Debug, Default, Deserialize)]
struct Metadata {
    generated: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct Feature {
    #[serde(default)]
    id: String,
    #[serde(default)]
    properties: Properties,
    #[serde(default)]
    geometry: Geometry,
}

#[derive(Debug, Default, Deserialize)]
struct Properties {
    mag: Option<f32>,
    #[serde(default)]
    place: String,
    #[serde(default)]
    time: i64,
    #[serde(default)]
    updated: i64,
    alert: Option<String>,
    url: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct Geometry {
    #[serde(default)]
    coordinates: Vec<f64>,
}

/// Parse and normalize one complete USGS GeoJSON response.
fn parse_snapshot(host: &str, body: &str, fetched_at_ms: i64) -> io::Result<EarthquakeSnapshot> {
    if body.len() > MAX_BODY_BYTES {
        return Err(io::Error::other(format!(
            "USGS response exceeds {MAX_BODY_BYTES} byte limit"
        )));
    }
    let feed: Feed = serde_json::from_str(body).map_err(io_other)?;
    let mut snapshot = EarthquakeSnapshot::empty(host, fetched_at_ms);
    snapshot.feed_generated_at_ms = feed.metadata.generated;

    for (index, feature) in feed.features.into_iter().enumerate() {
        let Some((&longitude, rest)) = feature.geometry.coordinates.split_first() else {
            snapshot
                .gaps
                .push(format!("feature {index} omitted: missing longitude"));
            continue;
        };
        let Some((&latitude, rest)) = rest.split_first() else {
            snapshot
                .gaps
                .push(format!("feature {index} omitted: missing latitude"));
            continue;
        };
        let Some(&depth_km) = rest.first() else {
            snapshot
                .gaps
                .push(format!("feature {index} omitted: missing depth"));
            continue;
        };
        if feature.id.trim().is_empty()
            || !latitude.is_finite()
            || !longitude.is_finite()
            || !depth_km.is_finite()
            || !(-90.0..=90.0).contains(&latitude)
            || !(-180.0..=180.0).contains(&longitude)
        {
            snapshot.gaps.push(format!(
                "feature {index} omitted: invalid id or coordinates"
            ));
            continue;
        }
        let pager_alert = match feature.properties.alert.as_deref() {
            None => None,
            Some("green") => Some(PagerAlert::Green),
            Some("yellow") => Some(PagerAlert::Yellow),
            Some("orange") => Some(PagerAlert::Orange),
            Some("red") => Some(PagerAlert::Red),
            Some(other) => {
                snapshot.gaps.push(format!(
                    "event {} has unknown PAGER alert `{other}`",
                    feature.id
                ));
                None
            }
        };
        snapshot.events.push(EarthquakeEvent {
            id: feature.id,
            occurred_at_ms: feature.properties.time,
            updated_at_ms: feature.properties.updated,
            latitude,
            longitude,
            depth_km: depth_km as f32,
            magnitude: feature.properties.mag.filter(|value| value.is_finite()),
            place: feature.properties.place,
            pager_alert,
            detail_url: feature.properties.url,
        });
    }
    Ok(snapshot)
}

/// Workstation-side USGS overlay worker.
pub struct EarthquakeOverlayWorker {
    host: String,
    probe: Option<Arc<dyn EarthquakeProbe>>,
    bus_root: Option<PathBuf>,
    bus_disabled: bool,
    poll: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PollOutcome {
    Fresh,
    Degraded,
    Deferred,
}

impl EarthquakeOverlayWorker {
    /// Build production wiring. The keyless USGS adapter is enabled by default
    /// on spawned Workstation-tier nodes; an explicit false-y env disables it.
    /// Client construction failure also degrades to idle.
    #[must_use]
    pub fn new(host: String) -> Self {
        let probe = if env_default_enabled(ENABLED_ENV) {
            let endpoint = std::env::var(ENDPOINT_ENV)
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_ENDPOINT.to_string());
            match UsgsHttpProbe::new(endpoint) {
                Ok(probe) => Some(Arc::new(probe) as Arc<dyn EarthquakeProbe>),
                Err(error) => {
                    tracing::warn!(
                        target: "mackesd::earthquake_overlay",
                        %error,
                        "USGS client unavailable; earthquake overlay worker will idle"
                    );
                    None
                }
            }
        } else {
            None
        };
        Self {
            host,
            probe,
            bus_root: None,
            bus_disabled: false,
            poll: POLL,
        }
    }

    /// Inject a captured-fixture probe.
    #[must_use]
    pub fn with_probe(mut self, probe: Arc<dyn EarthquakeProbe>) -> Self {
        self.probe = Some(probe);
        self
    }

    /// Override or explicitly disable Bus publishing.
    #[must_use]
    pub fn with_bus_root(mut self, root: Option<PathBuf>) -> Self {
        self.bus_disabled = root.is_none();
        self.bus_root = root;
        self
    }

    /// Override the poll cadence for tests.
    #[must_use]
    pub const fn with_poll(mut self, poll: Duration) -> Self {
        self.poll = poll;
        self
    }

    fn bus_root(&self) -> PathBuf {
        resolve_bus_root(self.bus_root.clone(), mde_bus::default_data_dir())
    }

    fn publish(&self, snapshot: &EarthquakeSnapshot) -> io::Result<()> {
        if self.bus_disabled {
            return Ok(());
        }
        let body = serde_json::to_string(snapshot).map_err(io_other)?;
        let persist = Persist::open(self.bus_root()).map_err(io_other)?;
        #[cfg(test)]
        if FAIL_NEXT_PUBLICATION.with(|fail| fail.replace(false)) {
            return Err(io::Error::other("injected earthquake publication failure"));
        }
        persist
            .write(
                &earthquake_state_topic(&self.host),
                Priority::Default,
                None,
                Some(&body),
            )
            .map_err(io_other)?;
        Ok(())
    }

    fn poll_once(
        &self,
        probe: &dyn EarthquakeProbe,
        last_good: &mut Option<EarthquakeSnapshot>,
    ) -> PollOutcome {
        let outcome = self.handle_probe_result(probe.fetch(), last_good);
        if outcome != PollOutcome::Fresh {
            probe.publication_deferred();
        }
        outcome
    }

    fn handle_probe_result(
        &self,
        result: io::Result<ProbeResponse>,
        last_good: &mut Option<EarthquakeSnapshot>,
    ) -> PollOutcome {
        match result {
            Ok(ProbeResponse::Modified(body)) => {
                match parse_snapshot(&self.host, &body, now_ms()) {
                    Ok(snapshot) => match self.publish(&snapshot) {
                        Ok(()) => {
                            *last_good = Some(snapshot);
                            PollOutcome::Fresh
                        }
                        Err(error) => {
                            tracing::warn!(target: "mackesd::earthquake_overlay", host = %self.host, %error, "earthquake publication failed; refresh remains uncommitted");
                            PollOutcome::Deferred
                        }
                    },
                    Err(error) => {
                        self.publish_failure(last_good, &format!("USGS payload invalid: {error}"))
                    }
                }
            }
            Ok(ProbeResponse::NotModified) => {
                if let Some(snapshot) = last_good.as_ref() {
                    let mut refreshed = snapshot.clone();
                    refreshed.fetched_at_ms = now_ms();
                    refreshed
                        .gaps
                        .retain(|gap| !gap.starts_with("USGS refresh failed:"));
                    match self.publish(&refreshed) {
                        Ok(()) => {
                            *last_good = Some(refreshed);
                            PollOutcome::Fresh
                        }
                        Err(error) => {
                            tracing::warn!(target: "mackesd::earthquake_overlay", host = %self.host, %error, "earthquake 304 publication failed; refresh remains uncommitted");
                            PollOutcome::Deferred
                        }
                    }
                } else {
                    tracing::warn!(
                        target: "mackesd::earthquake_overlay",
                        host = %self.host,
                        "USGS returned 304 before this process had a last-good snapshot"
                    );
                    PollOutcome::Degraded
                }
            }
            Err(error) => self.publish_failure(last_good, &format!("USGS refresh failed: {error}")),
        }
    }

    /// Run the blocking reqwest probe away from Tokio's worker threads. Shutdown
    /// wins the select immediately; the detached blocking call remains bounded by
    /// the client's 15-second timeout and cannot stall worker supervision.
    async fn poll_once_async(
        &self,
        probe: Arc<dyn EarthquakeProbe>,
        last_good: &mut Option<EarthquakeSnapshot>,
        shutdown: &mut ShutdownToken,
    ) -> Option<PollOutcome> {
        let fetch_probe = probe.clone();
        let task = tokio::task::spawn_blocking(move || fetch_probe.fetch());
        tokio::select! {
            () = shutdown.wait() => None,
            joined = task => {
                let result = match joined {
                    Ok(result) => result,
                    Err(error) => Err(io::Error::other(format!("USGS fetch task failed: {error}"))),
                };
                let outcome = self.handle_probe_result(result, last_good);
                if outcome != PollOutcome::Fresh {
                    probe.publication_deferred();
                }
                Some(outcome)
            }
        }
    }

    fn publish_failure(
        &self,
        last_good: &mut Option<EarthquakeSnapshot>,
        gap: &str,
    ) -> PollOutcome {
        tracing::warn!(
            target: "mackesd::earthquake_overlay",
            host = %self.host,
            error = gap,
            "USGS earthquake refresh failed; retaining last-good snapshot"
        );
        if let Some(snapshot) = last_good.as_ref() {
            let mut degraded = snapshot.clone();
            degraded
                .gaps
                .retain(|existing| !existing.starts_with("USGS refresh failed:"));
            degraded.gaps.push(gap.to_string());
            match self.publish(&degraded) {
                Ok(()) => {
                    *last_good = Some(degraded);
                    PollOutcome::Degraded
                }
                Err(error) => {
                    tracing::warn!(target: "mackesd::earthquake_overlay", host = %self.host, %error, "earthquake degraded publication failed; gap remains uncommitted");
                    PollOutcome::Deferred
                }
            }
        } else {
            PollOutcome::Degraded
        }
    }
}

fn resolve_bus_root(configured: Option<PathBuf>, user: Option<PathBuf>) -> PathBuf {
    configured
        .or(user)
        .unwrap_or_else(|| PathBuf::from(mde_bus::SYSTEM_BUS_ROOT))
}

fn cadence_after(outcome: PollOutcome, retry: Duration, poll: Duration) -> (Duration, Duration) {
    match outcome {
        PollOutcome::Fresh => (poll, RETRY_MIN.min(poll)),
        PollOutcome::Degraded => (retry, retry.saturating_mul(2).min(poll)),
        PollOutcome::Deferred => (retry, retry),
    }
}

#[cfg(test)]
std::thread_local! {
    static FAIL_NEXT_PUBLICATION: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[async_trait::async_trait]
impl Worker for EarthquakeOverlayWorker {
    fn name(&self) -> &'static str {
        "earthquake_overlay"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let Some(probe) = self.probe.clone() else {
            tracing::info!(
                target: "mackesd::earthquake_overlay",
                env = ENABLED_ENV,
                "earthquake overlay disabled or unavailable; worker idle"
            );
            shutdown.wait().await;
            return Ok(());
        };

        let mut last_good = None;
        let mut retry = RETRY_MIN.min(self.poll);
        loop {
            let Some(outcome) = self
                .poll_once_async(probe.clone(), &mut last_good, &mut shutdown)
                .await
            else {
                break;
            };
            let (delay, next_retry) = cadence_after(outcome, retry, self.poll);
            retry = next_retry;
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                () = shutdown.wait() => break,
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
    use std::collections::VecDeque;
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, Ordering};

    use mde_bus::persist::Persist;

    use super::*;

    // Captured from the official all_hour.geojson response on 2026-07-22 and
    // reduced to two complete features. The first feature's third coordinate
    // (2.98) is deliberately asserted as depth rather than altitude.
    const CAPTURED_USGS: &str = r#"{
      "type":"FeatureCollection",
      "metadata":{"generated":1784750329000,"count":2},
      "features":[
        {"type":"Feature","properties":{"mag":0.53,"place":"4 km WNW of Little Lake, CA","time":1784749958810,"updated":1784750161693,"alert":null,"url":"https://earthquake.usgs.gov/earthquakes/eventpage/ci40659474"},"geometry":{"type":"Point","coordinates":[-117.95,35.956,2.98]},"id":"ci40659474"},
        {"type":"Feature","properties":{"mag":1.8,"place":"17 km W of Susitna, Alaska","time":1784749779323,"updated":1784749868970,"alert":"red","url":"https://earthquake.usgs.gov/earthquakes/eventpage/aka2026okqcho"},"geometry":{"type":"Point","coordinates":[-150.843,61.525,64.6]},"id":"aka2026okqcho"}
      ]
    }"#;

    struct FakeProbe {
        responses: Mutex<VecDeque<Result<ProbeResponse, String>>>,
    }

    impl FakeProbe {
        fn new(responses: Vec<Result<ProbeResponse, String>>) -> Self {
            Self {
                responses: Mutex::new(responses.into()),
            }
        }
    }

    impl EarthquakeProbe for FakeProbe {
        fn fetch(&self) -> io::Result<ProbeResponse> {
            self.responses
                .lock()
                .map_err(|_| io::Error::other("fake lock"))?
                .pop_front()
                .unwrap_or_else(|| Err("no scripted response".to_string()))
                .map_err(io::Error::other)
        }
    }

    fn worker() -> EarthquakeOverlayWorker {
        EarthquakeOverlayWorker::new("rig-1".to_string()).with_bus_root(None)
    }

    #[test]
    fn captured_fixture_normalizes_coordinates_depth_revision_and_pager() {
        let snapshot = parse_snapshot("rig-1", CAPTURED_USGS, 1784750330000).expect("parse");
        assert_eq!(snapshot.feed_generated_at_ms, Some(1784750329000));
        assert_eq!(snapshot.events.len(), 2);
        let first = &snapshot.events[0];
        assert_eq!(first.id, "ci40659474");
        assert!((first.longitude + 117.95).abs() < f64::EPSILON);
        assert!((first.latitude - 35.956).abs() < f64::EPSILON);
        assert!((first.depth_km - 2.98).abs() < f32::EPSILON);
        assert_eq!(first.updated_at_ms, 1784750161693);
        assert_eq!(snapshot.events[1].pager_alert, Some(PagerAlert::Red));
        assert_eq!(snapshot.license_tier, "public-domain");
    }

    #[test]
    fn malformed_features_become_gaps_and_null_magnitude_stays_absent() {
        let body = r#"{"metadata":{},"features":[
          {"id":"null-mag","properties":{"mag":null},"geometry":{"coordinates":[-1.0,2.0,3.0]}},
          {"id":"bad","properties":{},"geometry":{"coordinates":[1.0]}}
        ]}"#;
        let snapshot = parse_snapshot("rig-1", body, 10).expect("parse");
        assert_eq!(snapshot.events.len(), 1);
        assert_eq!(snapshot.events[0].magnitude, None);
        assert_eq!(snapshot.gaps.len(), 1);
        assert!(snapshot.gaps[0].contains("missing latitude"));
    }

    #[test]
    fn oversized_fixture_is_refused_before_json_allocation() {
        let body = " ".repeat(MAX_BODY_BYTES + 1);
        let error = parse_snapshot("rig-1", &body, 10).expect_err("oversize must fail");
        assert!(error.to_string().contains("exceeds"));
    }

    #[test]
    fn spurious_not_modified_without_a_sent_validator_is_rejected() {
        assert!(accept_not_modified(false).is_err());
        assert_eq!(
            accept_not_modified(true).expect("validated 304"),
            ProbeResponse::NotModified
        );
    }

    #[test]
    fn keyless_usgs_producer_defaults_on_with_explicit_false_opt_out() {
        assert!(overlay_enabled_from_env(None));
        assert!(overlay_enabled_from_env(Some("")));
        assert!(overlay_enabled_from_env(Some("1")));
        assert!(overlay_enabled_from_env(Some("true")));
        assert!(!overlay_enabled_from_env(Some("0")));
        assert!(!overlay_enabled_from_env(Some("false")));
        assert!(!overlay_enabled_from_env(Some("NO")));
        assert!(!overlay_enabled_from_env(Some(" off ")));
    }

    #[test]
    fn http_client_refuses_redirects_before_contacting_the_target() {
        let target = TcpListener::bind("127.0.0.1:0").expect("target listener");
        target.set_nonblocking(true).expect("nonblocking");
        let target_addr = target.local_addr().expect("target addr");
        let contacted = Arc::new(AtomicBool::new(false));
        let contacted_thread = contacted.clone();
        let target_thread = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_millis(400);
            while std::time::Instant::now() < deadline {
                match target.accept() {
                    Ok((_stream, _)) => {
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
        let redirect = TcpListener::bind("127.0.0.1:0").expect("redirect listener");
        let redirect_addr = redirect.local_addr().expect("redirect addr");
        let redirect_thread = std::thread::spawn(move || {
            let (mut stream, _) = redirect.accept().expect("redirect request");
            let mut request = [0_u8; 2048];
            let _ = stream.read(&mut request);
            write!(
                stream,
                "HTTP/1.1 302 Found\r\nLocation: http://{target_addr}/escaped\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .expect("redirect response");
        });
        let probe =
            UsgsHttpProbe::new(format!("http://{redirect_addr}/feed")).expect("probe client");
        let error = probe.fetch().expect_err("redirect rejected");
        assert!(error.to_string().contains("redirects are disabled"));
        redirect_thread.join().expect("redirect thread");
        target_thread.join().expect("target thread");
        assert!(!contacted.load(Ordering::Relaxed));
    }

    #[test]
    fn failed_refresh_retains_original_fetch_timestamp_and_publishes_gap() {
        let tmp = tempfile::tempdir().expect("temp");
        let root = tmp.path().to_path_buf();
        let fake = Arc::new(FakeProbe::new(vec![
            Ok(ProbeResponse::Modified(CAPTURED_USGS.to_string())),
            Err("timeout".to_string()),
        ]));
        let w = worker().with_probe(fake).with_bus_root(Some(root.clone()));
        let mut last = None;
        assert_eq!(
            w.poll_once(w.probe.as_deref().expect("probe"), &mut last),
            PollOutcome::Fresh
        );
        let fetched = last.as_ref().expect("last").fetched_at_ms;
        assert_eq!(
            w.poll_once(w.probe.as_deref().expect("probe"), &mut last),
            PollOutcome::Degraded
        );
        assert_eq!(last.as_ref().expect("last").fetched_at_ms, fetched);
        assert!(last
            .as_ref()
            .expect("last")
            .gaps
            .iter()
            .any(|gap| gap.contains("timeout")));

        let persist = Persist::open(root).expect("bus");
        let rows = persist
            .list_since(&earthquake_state_topic("rig-1"), None)
            .expect("read");
        assert_eq!(rows.len(), 2, "success plus honest retained failure");
    }

    #[test]
    fn conditional_validation_refreshes_age_without_reparsing() {
        let fake = Arc::new(FakeProbe::new(vec![
            Ok(ProbeResponse::Modified(CAPTURED_USGS.to_string())),
            Ok(ProbeResponse::NotModified),
        ]));
        let w = worker().with_probe(fake);
        let mut last = None;
        assert_eq!(
            w.poll_once(w.probe.as_deref().expect("probe"), &mut last),
            PollOutcome::Fresh
        );
        let before = last.as_ref().expect("last").events.clone();
        assert_eq!(
            w.poll_once(w.probe.as_deref().expect("probe"), &mut last),
            PollOutcome::Fresh
        );
        assert_eq!(last.as_ref().expect("last").events, before);
    }

    #[test]
    fn same_worker_recovers_late_and_replaced_bus_with_latest_wins_state() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().join("late-bus");
        std::fs::write(&root, "temporarily unavailable").expect("blocking file");
        let fake = Arc::new(FakeProbe::new(vec![
            Ok(ProbeResponse::Modified(CAPTURED_USGS.to_string())),
            Ok(ProbeResponse::Modified(CAPTURED_USGS.to_string())),
            Ok(ProbeResponse::NotModified),
        ]));
        let worker = worker().with_probe(fake).with_bus_root(Some(root.clone()));
        let mut last_good = None;

        assert_eq!(
            worker.poll_once(worker.probe.as_deref().expect("probe"), &mut last_good),
            PollOutcome::Deferred
        );
        assert!(last_good.is_none(), "late Bus must not commit last-good");

        std::fs::remove_file(&root).expect("remove blocking file");
        assert_eq!(
            worker.poll_once(worker.probe.as_deref().expect("probe"), &mut last_good),
            PollOutcome::Fresh
        );
        assert!(last_good.is_some());

        let replacement = temp.path().join("replacement-bus");
        drop(Persist::open(replacement.clone()).expect("replacement Bus"));
        std::fs::rename(replacement.join("index.sqlite"), root.join("index.sqlite"))
            .expect("replace Bus index");
        assert_eq!(
            worker.poll_once(worker.probe.as_deref().expect("probe"), &mut last_good),
            PollOutcome::Fresh
        );

        let rows = Persist::open(root)
            .expect("replacement reader")
            .list_since(&earthquake_state_topic("rig-1"), None)
            .expect("replacement rows");
        assert_eq!(rows.len(), 1, "state topic remains latest-wins per index");
        assert_eq!(
            resolve_bus_root(None, None),
            PathBuf::from(mde_bus::SYSTEM_BUS_ROOT)
        );
    }

    #[test]
    fn failed_publication_corrects_forward_without_state_or_cadence_advance() {
        let temp = tempfile::tempdir().expect("temp");
        let root = temp.path().join("bus");
        let fake = Arc::new(FakeProbe::new(vec![
            Ok(ProbeResponse::Modified(CAPTURED_USGS.to_string())),
            Ok(ProbeResponse::Modified(CAPTURED_USGS.to_string())),
            Ok(ProbeResponse::NotModified),
            Ok(ProbeResponse::NotModified),
        ]));
        let worker = worker().with_probe(fake).with_bus_root(Some(root.clone()));
        let mut last_good = None;
        let retry = Duration::from_secs(5);
        let poll = Duration::from_secs(60);

        FAIL_NEXT_PUBLICATION.with(|fail| fail.set(true));
        let first = worker.poll_once(worker.probe.as_deref().expect("probe"), &mut last_good);
        assert_eq!(first, PollOutcome::Deferred);
        assert!(last_good.is_none());
        assert_eq!(cadence_after(first, retry, poll), (retry, retry));

        assert_eq!(
            worker.poll_once(worker.probe.as_deref().expect("probe"), &mut last_good),
            PollOutcome::Fresh
        );
        let committed = last_good.clone().expect("committed snapshot");

        std::thread::sleep(Duration::from_millis(2));
        FAIL_NEXT_PUBLICATION.with(|fail| fail.set(true));
        let refresh = worker.poll_once(worker.probe.as_deref().expect("probe"), &mut last_good);
        assert_eq!(refresh, PollOutcome::Deferred);
        assert_eq!(last_good.as_ref(), Some(&committed));
        assert_eq!(cadence_after(refresh, retry, poll), (retry, retry));

        std::thread::sleep(Duration::from_millis(2));
        assert_eq!(
            worker.poll_once(worker.probe.as_deref().expect("probe"), &mut last_good),
            PollOutcome::Fresh
        );
        assert!(last_good.as_ref().expect("refreshed").fetched_at_ms > committed.fetched_at_ms);
        assert_eq!(
            Persist::open(root)
                .expect("bus")
                .list_since(&earthquake_state_topic("rig-1"), None)
                .expect("rows")
                .len(),
            2,
            "only successful modified and corrected-forward refresh publish"
        );
    }

    #[tokio::test]
    async fn unconfigured_worker_idles_and_exits_on_shutdown() {
        let mut w = worker();
        w.probe = None;
        let (tx, rx) = tokio::sync::watch::channel(false);
        let token = ShutdownToken::from_receiver(rx);
        let handle = tokio::spawn(async move { w.run(token).await });
        tokio::time::sleep(Duration::from_millis(10)).await;
        tx.send(true).expect("shutdown");
        let joined = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(joined.is_ok(), "idle worker exits promptly");
    }

    struct SlowProbe;

    impl EarthquakeProbe for SlowProbe {
        fn fetch(&self) -> io::Result<ProbeResponse> {
            std::thread::sleep(Duration::from_millis(500));
            Ok(ProbeResponse::Modified(CAPTURED_USGS.to_string()))
        }
    }

    #[tokio::test]
    async fn blocking_http_probe_does_not_delay_worker_shutdown() {
        let mut w = worker()
            .with_probe(Arc::new(SlowProbe))
            .with_poll(Duration::from_secs(60));
        let (tx, rx) = tokio::sync::watch::channel(false);
        let token = ShutdownToken::from_receiver(rx);
        let handle = tokio::spawn(async move { w.run(token).await });
        tokio::time::sleep(Duration::from_millis(20)).await;
        tx.send(true).expect("shutdown");
        let joined = tokio::time::timeout(Duration::from_millis(200), handle).await;
        assert!(
            joined.is_ok(),
            "shutdown must win while the bounded blocking fetch is still in flight"
        );
    }
}
