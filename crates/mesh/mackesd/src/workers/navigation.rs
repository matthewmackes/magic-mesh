//! WL-FUNC-017 S6 — daemon-owned route/navigation authority.

#![cfg(feature = "async-services")]

use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mackes_mesh_types::navigation::{
    navigation_cancel_action_topic, navigation_progress_action_topic,
    navigation_route_action_topic, navigation_state_topic, CancelNavigationRequest,
    NavigationPhase, NavigationProgress, NavigationProgressRequest, NavigationSnapshot,
    NavigationUnavailableReason, RouteRequest, RouteRequestKind, RouteResult,
    NAVIGATION_SCHEMA_VERSION,
};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use serde::{Deserialize, Serialize};

use super::{ShutdownToken, Worker};

const POLL: Duration = Duration::from_secs(1);
const MAX_ACTION_AGE_MS: i64 = 10 * 60 * 1_000;
const MAX_PERSISTED_BYTES: usize = 512 * 1024;
const MAX_REPLAY_IDS: usize = 32;
const DEFAULT_STATE_PATH: &str = "/var/lib/mackesd/navigation.json";
const STATE_PATH_ENV: &str = "MDE_NAVIGATION_STATE_PATH";
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RouteProviderError {
    NotConfigured,
    Unavailable,
}

trait RouteProvider: Send + Sync {
    fn calculate(
        &self,
        request: &RouteRequest,
        now_ms: i64,
    ) -> Result<RouteResult, RouteProviderError>;
}

/// Production is fail-closed until an approved offline/online routing engine is provisioned.
struct UnavailableRouteProvider;

impl RouteProvider for UnavailableRouteProvider {
    fn calculate(
        &self,
        _request: &RouteRequest,
        _now_ms: i64,
    ) -> Result<RouteResult, RouteProviderError> {
        Err(RouteProviderError::NotConfigured)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistedNavigation {
    schema_version: u16,
    snapshot: NavigationSnapshot,
    route_cursor: Option<String>,
    progress_cursor: Option<String>,
    cancel_cursor: Option<String>,
    seen_request_ids: VecDeque<String>,
}

impl PersistedNavigation {
    fn initial(host: &str, now_ms: i64) -> Self {
        Self {
            schema_version: NAVIGATION_SCHEMA_VERSION,
            snapshot: NavigationSnapshot {
                schema_version: NAVIGATION_SCHEMA_VERSION,
                host: host.to_string(),
                generation: 0,
                produced_at_ms: now_ms,
                phase: NavigationPhase::Idle,
            },
            route_cursor: None,
            progress_cursor: None,
            cancel_cursor: None,
            seen_request_ids: VecDeque::new(),
        }
    }

    fn validate(&self, host: &str, now_ms: i64) -> io::Result<()> {
        if self.schema_version != NAVIGATION_SCHEMA_VERSION
            || self.snapshot.host != host
            || self.seen_request_ids.len() > MAX_REPLAY_IDS
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "navigation authority record mismatch",
            ));
        }
        self.snapshot.validate_at(now_ms).map_err(io_invalid_data)
    }

    fn remember(&mut self, request_id: String) {
        if self.seen_request_ids.len() == MAX_REPLAY_IDS {
            self.seen_request_ids.pop_front();
        }
        self.seen_request_ids.push_back(request_id);
    }

    fn saw(&self, request_id: &str) -> bool {
        self.seen_request_ids.iter().any(|seen| seen == request_id)
    }
}

enum ActionKind {
    Route,
    Progress,
    Cancel,
}

struct PendingAction {
    ulid: String,
    body: String,
    kind: ActionKind,
}

/// One latest-wins navigation authority for this node.
pub struct NavigationWorker {
    host: String,
    state_path: PathBuf,
    bus_root: Option<PathBuf>,
    poll: Duration,
    clock: Arc<dyn Clock>,
    provider: Arc<dyn RouteProvider>,
    authority: Option<PersistedNavigation>,
    published_once: bool,
}

impl NavigationWorker {
    /// Construct the node-scoped production authority with a fail-closed provider.
    #[must_use]
    pub fn new(host: String) -> Self {
        Self {
            host,
            state_path: std::env::var_os(STATE_PATH_ENV)
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(DEFAULT_STATE_PATH)),
            bus_root: crate::bus_publish::default_bus_root(),
            poll: POLL,
            clock: Arc::new(SystemClock),
            provider: Arc::new(UnavailableRouteProvider),
            authority: None,
            published_once: false,
        }
    }

    fn ensure_loaded(&mut self) -> io::Result<()> {
        if self.authority.is_some() {
            return Ok(());
        }
        let now_ms = self.clock.now_ms();
        let mut authority = load_record(&self.state_path)?
            .unwrap_or_else(|| PersistedNavigation::initial(&self.host, now_ms));
        authority.validate(&self.host, now_ms)?;
        if matches!(
            authority.snapshot.phase,
            NavigationPhase::Calculating { .. }
        ) {
            let request_id = match &authority.snapshot.phase {
                NavigationPhase::Calculating { request_id, .. } => Some(request_id.clone()),
                _ => None,
            };
            authority.snapshot.generation = authority
                .snapshot
                .generation
                .checked_add(1)
                .ok_or_else(|| io_invalid_data("navigation generation exhausted"))?;
            authority.snapshot.produced_at_ms = now_ms;
            authority.snapshot.phase = NavigationPhase::Unavailable {
                request_id,
                reason: NavigationUnavailableReason::InterruptedByRestart,
            };
        }
        store_record(&self.state_path, &authority)?;
        self.authority = Some(authority);
        Ok(())
    }

    fn collect_actions(&self, persist: &Persist) -> io::Result<Vec<PendingAction>> {
        let authority = self.authority.as_ref().expect("authority loaded");
        let mut actions = Vec::new();
        for (topic, cursor, kind) in [
            (
                navigation_route_action_topic(&self.host),
                authority.route_cursor.as_deref(),
                ActionKind::Route,
            ),
            (
                navigation_progress_action_topic(&self.host),
                authority.progress_cursor.as_deref(),
                ActionKind::Progress,
            ),
            (
                navigation_cancel_action_topic(&self.host),
                authority.cancel_cursor.as_deref(),
                ActionKind::Cancel,
            ),
        ] {
            for message in persist.list_since(&topic, cursor).map_err(io_other)? {
                actions.push(PendingAction {
                    ulid: message.ulid,
                    body: message.body.unwrap_or_default(),
                    kind: match kind {
                        ActionKind::Route => ActionKind::Route,
                        ActionKind::Progress => ActionKind::Progress,
                        ActionKind::Cancel => ActionKind::Cancel,
                    },
                });
            }
        }
        actions.sort_by(|left, right| left.ulid.cmp(&right.ulid));
        Ok(actions)
    }

    fn advance_cursor(&mut self, kind: &ActionKind, ulid: String) {
        let authority = self.authority.as_mut().expect("authority loaded");
        match kind {
            ActionKind::Route => authority.route_cursor = Some(ulid),
            ActionKind::Progress => authority.progress_cursor = Some(ulid),
            ActionKind::Cancel => authority.cancel_cursor = Some(ulid),
        }
    }

    fn publish(&mut self, persist: &Persist) -> io::Result<()> {
        let snapshot = &self.authority.as_ref().expect("authority loaded").snapshot;
        let body = serde_json::to_string(snapshot).map_err(io_other)?;
        persist
            .write(
                &navigation_state_topic(&self.host),
                Priority::Default,
                None,
                Some(&body),
            )
            .map_err(io_other)?;
        self.published_once = true;
        Ok(())
    }

    fn persist_and_publish(&mut self, persist: &Persist) -> io::Result<()> {
        store_record(
            &self.state_path,
            self.authority.as_ref().expect("authority loaded"),
        )?;
        self.publish(persist)
    }

    fn request_is_current(
        &self,
        host: &str,
        generation: u64,
        request_id: &str,
        issued_at_ms: i64,
        now_ms: i64,
    ) -> bool {
        let authority = self.authority.as_ref().expect("authority loaded");
        host == self.host
            && generation == authority.snapshot.generation
            && !authority.saw(request_id)
            && now_ms.saturating_sub(issued_at_ms) <= MAX_ACTION_AGE_MS
    }

    fn process_route(&mut self, persist: &Persist, body: &[u8], now_ms: i64) -> io::Result<()> {
        let Ok(request) = RouteRequest::from_json_at(body, now_ms) else {
            return Ok(());
        };
        if !self.request_is_current(
            &request.host,
            request.expected_generation,
            &request.request_id,
            request.issued_at_ms,
            now_ms,
        ) {
            return Ok(());
        }
        if request.kind == RouteRequestKind::Reroute {
            let active_route = match &self
                .authority
                .as_ref()
                .expect("authority loaded")
                .snapshot
                .phase
            {
                NavigationPhase::Active { route, .. } => Some(route.route_id.as_str()),
                _ => None,
            };
            if active_route != request.replaces_route_id.as_deref() {
                return Ok(());
            }
        }
        {
            let authority = self.authority.as_mut().expect("authority loaded");
            authority.snapshot.generation = authority
                .snapshot
                .generation
                .checked_add(1)
                .ok_or_else(|| io_invalid_data("navigation generation exhausted"))?;
            authority.snapshot.produced_at_ms = now_ms;
            authority.snapshot.phase = NavigationPhase::Calculating {
                request_id: request.request_id.clone(),
                reroute: request.kind == RouteRequestKind::Reroute,
            };
            authority.remember(request.request_id.clone());
        }
        self.persist_and_publish(persist)?;
        let phase = match self.provider.calculate(&request, now_ms) {
            Ok(route)
                if route.request_id == request.request_id && route.validate_at(now_ms).is_ok() =>
            {
                let progress = NavigationProgress {
                    route_id: route.route_id.clone(),
                    position: request.origin.point.clone(),
                    observed_at_ms: now_ms,
                    maneuver_index: 0,
                    distance_remaining_metres: route.distance_metres,
                    duration_remaining_seconds: route.duration_seconds,
                    off_route: false,
                };
                NavigationPhase::Active { route, progress }
            }
            Err(RouteProviderError::NotConfigured) => NavigationPhase::Unavailable {
                request_id: Some(request.request_id),
                reason: NavigationUnavailableReason::ProviderNotConfigured,
            },
            Err(RouteProviderError::Unavailable) | Ok(_) => NavigationPhase::Unavailable {
                request_id: Some(request.request_id),
                reason: NavigationUnavailableReason::ProviderUnavailable,
            },
        };
        let authority = self.authority.as_mut().expect("authority loaded");
        authority.snapshot.produced_at_ms = now_ms;
        authority.snapshot.phase = phase;
        Ok(())
    }

    fn process_progress(&mut self, body: &[u8], now_ms: i64) {
        let Ok(request) = NavigationProgressRequest::from_json_at(body, now_ms) else {
            return;
        };
        if !self.request_is_current(
            &request.host,
            request.expected_generation,
            &request.request_id,
            request.issued_at_ms,
            now_ms,
        ) {
            return;
        }
        let valid = match &self
            .authority
            .as_ref()
            .expect("authority loaded")
            .snapshot
            .phase
        {
            NavigationPhase::Active { route, progress } => {
                request.route_id == route.route_id
                    && request.progress.observed_at_ms >= progress.observed_at_ms
                    && request.progress.validate_for(route, now_ms).is_ok()
            }
            _ => false,
        };
        if !valid {
            return;
        }
        let authority = self.authority.as_mut().expect("authority loaded");
        if let NavigationPhase::Active { progress, .. } = &mut authority.snapshot.phase {
            *progress = request.progress;
        }
        let Some(generation) = authority.snapshot.generation.checked_add(1) else {
            return;
        };
        authority.snapshot.generation = generation;
        authority.snapshot.produced_at_ms = now_ms;
        authority.remember(request.request_id);
    }

    fn process_cancel(&mut self, body: &[u8], now_ms: i64) {
        let Ok(request) = CancelNavigationRequest::from_json_at(body, now_ms) else {
            return;
        };
        if !self.request_is_current(
            &request.host,
            request.expected_generation,
            &request.request_id,
            request.issued_at_ms,
            now_ms,
        ) {
            return;
        }
        if matches!(
            self.authority
                .as_ref()
                .expect("authority loaded")
                .snapshot
                .phase,
            NavigationPhase::Idle | NavigationPhase::Cancelled { .. }
        ) {
            return;
        }
        let authority = self.authority.as_mut().expect("authority loaded");
        let Some(generation) = authority.snapshot.generation.checked_add(1) else {
            return;
        };
        authority.snapshot.generation = generation;
        authority.snapshot.produced_at_ms = now_ms;
        authority.snapshot.phase = NavigationPhase::Cancelled {
            request_id: request.request_id.clone(),
            cancelled_at_ms: now_ms,
        };
        authority.remember(request.request_id);
    }

    fn tick_once(&mut self) -> io::Result<()> {
        self.ensure_loaded()?;
        let Some(bus_root) = self.bus_root.clone() else {
            return Ok(());
        };
        let persist = Persist::open(bus_root).map_err(io_other)?;
        let actions = self.collect_actions(&persist)?;
        if actions.is_empty() {
            if !self.published_once {
                self.publish(&persist)?;
            }
            return Ok(());
        }
        for action in actions {
            let now_ms = self.clock.now_ms();
            self.advance_cursor(&action.kind, action.ulid);
            match action.kind {
                ActionKind::Route => {
                    self.process_route(&persist, action.body.as_bytes(), now_ms)?
                }
                ActionKind::Progress => self.process_progress(action.body.as_bytes(), now_ms),
                ActionKind::Cancel => self.process_cancel(action.body.as_bytes(), now_ms),
            }
            self.persist_and_publish(&persist)?;
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl Worker for NavigationWorker {
    fn name(&self) -> &'static str {
        "navigation"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        self.tick_once()?;
        let mut tick = tokio::time::interval(self.poll);
        tick.tick().await;
        loop {
            tokio::select! { _ = tick.tick() => self.tick_once()?, () = shutdown.wait() => break }
        }
        Ok(())
    }
}

fn load_record(path: &Path) -> io::Result<Option<PersistedNavigation>> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut body = Vec::new();
    std::io::Read::by_ref(&mut file)
        .take((MAX_PERSISTED_BYTES + 1) as u64)
        .read_to_end(&mut body)?;
    if body.len() > MAX_PERSISTED_BYTES {
        return Err(io_invalid_data("navigation state too large"));
    }
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(io_invalid_data)
}

fn store_record(path: &Path, record: &PersistedNavigation) -> io::Result<()> {
    let body = serde_json::to_vec(record).map_err(io_other)?;
    if body.len() > MAX_PERSISTED_BYTES {
        return Err(io_invalid_data("navigation state too large"));
    }
    let parent = path
        .parent()
        .ok_or_else(|| io_invalid_data("navigation state has no parent"))?;
    fs::create_dir_all(parent)?;
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temp = parent.join(format!(
        ".navigation.{}.{}.tmp",
        std::process::id(),
        sequence
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temp)?;
    file.write_all(&body)?;
    file.sync_all()?;
    fs::rename(&temp, path)?;
    File::open(parent)?.sync_all()
}

fn io_invalid_data(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}
fn io_other(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::navigation::{
        ManeuverKind, RouteAttribution, RouteEndpoint, RouteManeuver, RouteProfile,
    };
    use mackes_mesh_types::nws_alert::GeoPoint;
    use tempfile::TempDir;

    const NOW: i64 = 1_800_000_000_000;

    struct FixedClock;
    impl Clock for FixedClock {
        fn now_ms(&self) -> i64 {
            NOW
        }
    }

    struct FixtureProvider;
    impl RouteProvider for FixtureProvider {
        fn calculate(
            &self,
            request: &RouteRequest,
            now_ms: i64,
        ) -> Result<RouteResult, RouteProviderError> {
            Ok(RouteResult {
                route_id: format!("route-{}", request.request_id),
                request_id: request.request_id.clone(),
                calculated_at_ms: now_ms,
                distance_metres: 1000,
                duration_seconds: 120,
                geometry: vec![
                    request.origin.point.clone(),
                    request.destination.point.clone(),
                ],
                maneuvers: vec![
                    RouteManeuver {
                        sequence: 0,
                        kind: ManeuverKind::Depart,
                        instruction: "Depart".into(),
                        point: request.origin.point.clone(),
                        distance_metres: 1000,
                        duration_seconds: 120,
                    },
                    RouteManeuver {
                        sequence: 1,
                        kind: ManeuverKind::Arrive,
                        instruction: "Arrive".into(),
                        point: request.destination.point.clone(),
                        distance_metres: 0,
                        duration_seconds: 0,
                    },
                ],
                attribution: RouteAttribution {
                    provider_id: "fixture".into(),
                    label: "Deterministic offline fixture".into(),
                    data_revision: "fixture-v1".into(),
                    offline: true,
                },
            })
        }
    }

    struct OutageProvider;
    impl RouteProvider for OutageProvider {
        fn calculate(
            &self,
            _request: &RouteRequest,
            _now_ms: i64,
        ) -> Result<RouteResult, RouteProviderError> {
            Err(RouteProviderError::Unavailable)
        }
    }

    struct Fixture {
        _temp: TempDir,
        bus: PathBuf,
        worker: NavigationWorker,
    }
    impl Fixture {
        fn new(provider: Arc<dyn RouteProvider>) -> Self {
            let temp = tempfile::tempdir().unwrap();
            let bus = temp.path().join("bus");
            fs::create_dir_all(&bus).unwrap();
            let worker = NavigationWorker {
                host: "seat-1".into(),
                state_path: temp.path().join("navigation.json"),
                bus_root: Some(bus.clone()),
                poll: POLL,
                clock: Arc::new(FixedClock),
                provider,
                authority: None,
                published_once: false,
            };
            Self {
                _temp: temp,
                bus,
                worker,
            }
        }
        fn publish<T: Serialize>(&self, topic: &str, value: &T) {
            let persist = Persist::open(self.bus.clone()).unwrap();
            persist
                .write(
                    topic,
                    Priority::Default,
                    None,
                    Some(&serde_json::to_string(value).unwrap()),
                )
                .unwrap();
        }
    }

    fn request(generation: u64, id: &str) -> RouteRequest {
        RouteRequest {
            schema_version: 1,
            request_id: id.into(),
            host: "seat-1".into(),
            expected_generation: generation,
            issued_at_ms: NOW,
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
    fn deterministic_route_progress_cancel_and_replay_are_generation_bound() {
        let mut fixture = Fixture::new(Arc::new(FixtureProvider));
        fixture.publish(
            &navigation_route_action_topic("seat-1"),
            &request(0, "req-1"),
        );
        fixture.worker.tick_once().unwrap();
        let active = fixture.worker.authority.as_ref().unwrap().snapshot.clone();
        let (route, mut progress) = match active.phase {
            NavigationPhase::Active { route, progress } => (route, progress),
            phase => panic!("unexpected {phase:?}"),
        };
        assert_eq!(active.generation, 1);
        progress.position = GeoPoint {
            latitude: 40.05,
            longitude: -75.05,
        };
        progress.observed_at_ms = NOW;
        progress.distance_remaining_metres = 500;
        progress.duration_remaining_seconds = 60;
        let update = NavigationProgressRequest {
            schema_version: 1,
            request_id: "progress-1".into(),
            host: "seat-1".into(),
            expected_generation: 1,
            issued_at_ms: NOW,
            route_id: route.route_id.clone(),
            progress,
        };
        fixture.publish(&navigation_progress_action_topic("seat-1"), &update);
        fixture.worker.tick_once().unwrap();
        assert_eq!(
            fixture
                .worker
                .authority
                .as_ref()
                .unwrap()
                .snapshot
                .generation,
            2
        );
        fixture.publish(&navigation_progress_action_topic("seat-1"), &update);
        fixture.worker.tick_once().unwrap();
        assert_eq!(
            fixture
                .worker
                .authority
                .as_ref()
                .unwrap()
                .snapshot
                .generation,
            2
        );
        let cancel = CancelNavigationRequest {
            schema_version: 1,
            request_id: "cancel-1".into(),
            host: "seat-1".into(),
            expected_generation: 2,
            issued_at_ms: NOW,
        };
        fixture.publish(&navigation_cancel_action_topic("seat-1"), &cancel);
        fixture.worker.tick_once().unwrap();
        assert!(matches!(
            fixture.worker.authority.as_ref().unwrap().snapshot.phase,
            NavigationPhase::Cancelled { .. }
        ));
    }

    #[test]
    fn stale_reroute_is_refused_and_restart_never_revives_calculating_route() {
        let mut fixture = Fixture::new(Arc::new(FixtureProvider));
        fixture.worker.ensure_loaded().unwrap();
        let mut reroute = request(0, "reroute-1");
        reroute.kind = RouteRequestKind::Reroute;
        reroute.replaces_route_id = Some("not-active".into());
        fixture.publish(&navigation_route_action_topic("seat-1"), &reroute);
        fixture.worker.tick_once().unwrap();
        assert!(matches!(
            fixture.worker.authority.as_ref().unwrap().snapshot.phase,
            NavigationPhase::Idle
        ));
        {
            let authority = fixture.worker.authority.as_mut().unwrap();
            authority.snapshot.phase = NavigationPhase::Calculating {
                request_id: "req-crash".into(),
                reroute: false,
            };
            store_record(&fixture.worker.state_path, authority).unwrap();
        }
        fixture.worker.authority = None;
        fixture.worker.ensure_loaded().unwrap();
        assert!(matches!(
            fixture.worker.authority.as_ref().unwrap().snapshot.phase,
            NavigationPhase::Unavailable {
                reason: NavigationUnavailableReason::InterruptedByRestart,
                ..
            }
        ));
    }

    #[test]
    fn production_provider_reports_unavailable_without_fabricating_route() {
        let mut fixture = Fixture::new(Arc::new(UnavailableRouteProvider));
        fixture.publish(
            &navigation_route_action_topic("seat-1"),
            &request(0, "req-unavailable"),
        );
        fixture.worker.tick_once().unwrap();
        assert!(matches!(
            fixture.worker.authority.as_ref().unwrap().snapshot.phase,
            NavigationPhase::Unavailable {
                reason: NavigationUnavailableReason::ProviderNotConfigured,
                ..
            }
        ));

        let mut outage = Fixture::new(Arc::new(OutageProvider));
        outage.publish(
            &navigation_route_action_topic("seat-1"),
            &request(0, "req-outage"),
        );
        outage.worker.tick_once().unwrap();
        assert!(matches!(
            outage.worker.authority.as_ref().unwrap().snapshot.phase,
            NavigationPhase::Unavailable {
                reason: NavigationUnavailableReason::ProviderUnavailable,
                ..
            }
        ));
    }
}
