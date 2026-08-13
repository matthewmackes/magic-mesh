//! WL-FUNC-017 S6 — daemon-owned route/navigation authority.

#![cfg(feature = "async-services")]

use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ed25519_dalek::{Signature, VerifyingKey};
use mackes_mesh_types::navigation::{
    navigation_cancel_action_topic, navigation_progress_action_topic,
    navigation_route_action_topic, navigation_state_topic, CancelNavigationRequest,
    NavigationPhase, NavigationProgress, NavigationProgressRequest, NavigationSnapshot,
    NavigationUnavailableReason, RouteRequest, RouteRequestKind, RouteResult,
    NAVIGATION_SCHEMA_VERSION,
};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::{Persist, StoredMessage};
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, CONTENT_LENGTH, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{ShutdownToken, Worker};

const POLL: Duration = Duration::from_secs(1);
const MIN_BUS_RETRY: Duration = Duration::from_millis(10);
const MAX_BUS_RETRY: Duration = Duration::from_secs(2);
const MAX_ACTION_AGE_MS: i64 = 10 * 60 * 1_000;
const MAX_PERSISTED_BYTES: usize = 512 * 1024;
const MAX_REPLAY_IDS: usize = 32;
const MAX_PROVIDER_AUTHORITY_BYTES: usize = 16 * 1024;
const MAX_PROVIDER_RESPONSE_BYTES: usize = 256 * 1024;
const MIN_PROVIDER_TIMEOUT_MS: u64 = 100;
const MAX_PROVIDER_TIMEOUT_MS: u64 = 3_000;
const DEFAULT_STATE_PATH: &str = "/var/lib/mackesd/navigation.json";
const DEFAULT_PROVIDER_AUTHORITY_PATH: &str = "/etc/mackesd/navigation-provider.json";
const STATE_PATH_ENV: &str = "MDE_NAVIGATION_STATE_PATH";
const PROVIDER_SCHEMA_VERSION: u16 = 1;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
type BusOpenFn = dyn Fn(&Path) -> io::Result<Persist> + Send + Sync;

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

/// Explicitly unavailable provider retained for deterministic failure fixtures.
#[cfg(test)]
struct UnavailableRouteProvider;

#[cfg(test)]
impl RouteProvider for UnavailableRouteProvider {
    fn calculate(
        &self,
        _request: &RouteRequest,
        _now_ms: i64,
    ) -> Result<RouteResult, RouteProviderError> {
        Err(RouteProviderError::NotConfigured)
    }
}

/// Root-governed, vendor-neutral route authority. The endpoint is deliberately
/// restricted to a numeric loopback address: the installed provider must own
/// its map data and offline behavior locally rather than turning mackesd into an
/// implicit Internet routing client.
#[derive(Debug, Clone)]
struct RouteProviderAuthority {
    provider_id: String,
    endpoint: reqwest::Url,
    timeout: Duration,
    verifying_key: VerifyingKey,
    source_dev: u64,
    source_ino: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RouteProviderAuthorityFile {
    schema_version: u16,
    provider_id: String,
    endpoint: String,
    timeout_ms: u64,
    ed25519_public_key: String,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct ProviderRouteRequest<'a> {
    schema_version: u16,
    request_sha256: &'a str,
    request: &'a RouteRequest,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProviderRouteResponse {
    schema_version: u16,
    request_sha256: String,
    route: RouteResult,
    signature: String,
}

struct GovernedHttpRouteProvider {
    authority_path: PathBuf,
    authority_owner_uid: u32,
}

impl GovernedHttpRouteProvider {
    fn production() -> Self {
        Self {
            authority_path: PathBuf::from(DEFAULT_PROVIDER_AUTHORITY_PATH),
            authority_owner_uid: 0,
        }
    }

    #[cfg(test)]
    fn for_test(authority_path: PathBuf, authority_owner_uid: u32) -> Self {
        Self {
            authority_path,
            authority_owner_uid,
        }
    }
}

impl RouteProvider for GovernedHttpRouteProvider {
    fn calculate(
        &self,
        request: &RouteRequest,
        _now_ms: i64,
    ) -> Result<RouteResult, RouteProviderError> {
        let authority = load_provider_authority(&self.authority_path, self.authority_owner_uid)
            .map_err(|_| RouteProviderError::NotConfigured)?
            .ok_or(RouteProviderError::NotConfigured)?;
        let timeout = authority.timeout;
        let request = request.clone();
        let request_authority = authority.clone();
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("navigation-provider-http".into())
            .spawn(move || {
                let _ = send.send(calculate_http_route(&request_authority, &request));
            })
            .map_err(|_| RouteProviderError::Unavailable)?;
        let result = receive
            .recv_timeout(timeout.saturating_add(Duration::from_millis(250)))
            .map_err(|_| RouteProviderError::Unavailable)?;
        let current = load_provider_authority(&self.authority_path, self.authority_owner_uid)
            .map_err(|_| RouteProviderError::NotConfigured)?
            .ok_or(RouteProviderError::NotConfigured)?;
        if !same_provider_authority(&authority, &current) {
            return Err(RouteProviderError::NotConfigured);
        }
        result
    }
}

fn same_provider_authority(
    expected: &RouteProviderAuthority,
    current: &RouteProviderAuthority,
) -> bool {
    expected.provider_id == current.provider_id
        && expected.endpoint == current.endpoint
        && expected.timeout == current.timeout
        && expected.verifying_key.as_bytes() == current.verifying_key.as_bytes()
        && expected.source_dev == current.source_dev
        && expected.source_ino == current.source_ino
}

fn load_provider_authority(
    path: &Path,
    expected_owner_uid: u32,
) -> io::Result<Option<RouteProviderAuthority>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let parent = path
        .parent()
        .filter(|parent| parent.is_absolute())
        .ok_or_else(|| io_invalid_data("navigation provider authority path is not absolute"))?;
    let parent_metadata = fs::symlink_metadata(parent)?;
    if parent_metadata.file_type().is_symlink()
        || !parent_metadata.is_dir()
        || parent_metadata.uid() != expected_owner_uid
        || parent_metadata.permissions().mode() & 0o022 != 0
        || fs::canonicalize(parent)? != parent
    {
        return Err(io_invalid_data(
            "navigation provider authority parent is not securely owned",
        ));
    }
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.uid() != expected_owner_uid
        || metadata.permissions().mode() & 0o022 != 0
        || metadata.len() > MAX_PROVIDER_AUTHORITY_BYTES as u64
    {
        return Err(io_invalid_data(
            "navigation provider authority is not a secure bounded regular file",
        ));
    }
    let mut options = OpenOptions::new();
    options.read(true).custom_flags(0o400_000 | 0o2_000_000); // O_NOFOLLOW | O_CLOEXEC
    let file = options.open(path)?;
    let opened = file.metadata()?;
    if !opened.is_file()
        || opened.dev() != metadata.dev()
        || opened.ino() != metadata.ino()
        || opened.uid() != expected_owner_uid
    {
        return Err(io_invalid_data(
            "navigation provider authority changed during secure open",
        ));
    }
    let mut body = Vec::with_capacity(metadata.len() as usize);
    file.take((MAX_PROVIDER_AUTHORITY_BYTES + 1) as u64)
        .read_to_end(&mut body)?;
    if body.len() > MAX_PROVIDER_AUTHORITY_BYTES {
        return Err(io_invalid_data(
            "navigation provider authority is too large",
        ));
    }
    let text = std::str::from_utf8(&body).map_err(io_invalid_data)?;
    mackes_mesh_types::workloads::reject_duplicate_json_keys(text).map_err(io_invalid_data)?;
    let authority: RouteProviderAuthorityFile =
        serde_json::from_slice(&body).map_err(io_invalid_data)?;
    validate_provider_authority(authority, opened.dev(), opened.ino()).map(Some)
}

fn validate_provider_authority(
    authority: RouteProviderAuthorityFile,
    source_dev: u64,
    source_ino: u64,
) -> io::Result<RouteProviderAuthority> {
    if authority.schema_version != PROVIDER_SCHEMA_VERSION
        || authority.provider_id.is_empty()
        || authority.provider_id.len() > 128
        || !authority
            .provider_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
        || !(MIN_PROVIDER_TIMEOUT_MS..=MAX_PROVIDER_TIMEOUT_MS).contains(&authority.timeout_ms)
    {
        return Err(io_invalid_data("invalid navigation provider authority"));
    }
    let endpoint = reqwest::Url::parse(&authority.endpoint).map_err(io_invalid_data)?;
    let loopback = matches!(endpoint.host_str(), Some("127.0.0.1" | "::1" | "[::1]"));
    if endpoint.scheme() != "http"
        || !loopback
        || endpoint.port().is_none()
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
        || endpoint.path() == "/"
    {
        return Err(io_invalid_data(
            "navigation provider endpoint must be an explicit loopback HTTP route",
        ));
    }
    let public_key = decode_hex::<32>(&authority.ed25519_public_key)
        .ok_or_else(|| io_invalid_data("invalid navigation provider Ed25519 public key"))?;
    let verifying_key = VerifyingKey::from_bytes(&public_key).map_err(io_invalid_data)?;
    Ok(RouteProviderAuthority {
        provider_id: authority.provider_id,
        endpoint,
        timeout: Duration::from_millis(authority.timeout_ms),
        verifying_key,
        source_dev,
        source_ino,
    })
}

fn calculate_http_route(
    authority: &RouteProviderAuthority,
    request: &RouteRequest,
) -> Result<RouteResult, RouteProviderError> {
    let canonical_request =
        serde_json::to_vec(request).map_err(|_| RouteProviderError::Unavailable)?;
    let request_sha256 = sha256_hex(&canonical_request);
    let body = serde_json::to_vec(&ProviderRouteRequest {
        schema_version: PROVIDER_SCHEMA_VERSION,
        request_sha256: &request_sha256,
        request,
    })
    .map_err(|_| RouteProviderError::Unavailable)?;
    let client = Client::builder()
        .connect_timeout(authority.timeout)
        .timeout(authority.timeout)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| RouteProviderError::Unavailable)?;
    let mut response = client
        .post(authority.endpoint.clone())
        .header(ACCEPT, "application/json")
        .header(CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .map_err(|_| RouteProviderError::Unavailable)?;
    if response.status() != reqwest::StatusCode::OK
        || !response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.eq_ignore_ascii_case("application/json"))
        || response
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<usize>().ok())
            .is_some_and(|length| length > MAX_PROVIDER_RESPONSE_BYTES)
    {
        return Err(RouteProviderError::Unavailable);
    }
    let mut response_body = Vec::new();
    response
        .by_ref()
        .take((MAX_PROVIDER_RESPONSE_BYTES + 1) as u64)
        .read_to_end(&mut response_body)
        .map_err(|_| RouteProviderError::Unavailable)?;
    if response_body.len() > MAX_PROVIDER_RESPONSE_BYTES {
        return Err(RouteProviderError::Unavailable);
    }
    let text = std::str::from_utf8(&response_body).map_err(|_| RouteProviderError::Unavailable)?;
    mackes_mesh_types::workloads::reject_duplicate_json_keys(text)
        .map_err(|_| RouteProviderError::Unavailable)?;
    let response: ProviderRouteResponse =
        serde_json::from_slice(&response_body).map_err(|_| RouteProviderError::Unavailable)?;
    if response.schema_version != PROVIDER_SCHEMA_VERSION
        || response.request_sha256 != request_sha256
        || response.route.request_id != request.request_id
        || response.route.attribution.provider_id != authority.provider_id
        || !response.route.attribution.offline
    {
        return Err(RouteProviderError::Unavailable);
    }
    let signature_bytes =
        decode_hex::<64>(&response.signature).ok_or(RouteProviderError::Unavailable)?;
    let signed = provider_response_signing_bytes(&request_sha256, &response.route)
        .map_err(|_| RouteProviderError::Unavailable)?;
    authority
        .verifying_key
        .verify_strict(&signed, &Signature::from_bytes(&signature_bytes))
        .map_err(|_| RouteProviderError::Unavailable)?;
    Ok(response.route)
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    encode_hex(&digest)
}

fn provider_response_signing_bytes(
    request_sha256: &str,
    route: &RouteResult,
) -> io::Result<Vec<u8>> {
    let route = serde_json::to_vec(route).map_err(io_other)?;
    let mut bytes = b"magic-mesh:navigation-provider-response:v1\0".to_vec();
    bytes.extend_from_slice(request_sha256.as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(&route);
    Ok(bytes)
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn decode_hex<const N: usize>(value: &str) -> Option<[u8; N]> {
    if value.len() != N * 2 || !value.is_ascii() {
        return None;
    }
    let mut decoded = [0_u8; N];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let high = char::from(pair[0]).to_digit(16)?;
        let low = char::from(pair[1]).to_digit(16)?;
        decoded[index] = u8::try_from((high << 4) | low).ok()?;
    }
    Some(decoded)
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

    fn forget(&mut self, request_id: &str) -> bool {
        let before = self.seen_request_ids.len();
        self.seen_request_ids.retain(|seen| seen != request_id);
        self.seen_request_ids.len() != before
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
    bus_root: PathBuf,
    poll: Duration,
    clock: Arc<dyn Clock>,
    provider: Arc<dyn RouteProvider>,
    authority: Option<PersistedNavigation>,
    published_once: bool,
    #[cfg(test)]
    bus_open_override: Option<Arc<BusOpenFn>>,
}

impl NavigationWorker {
    /// Construct the node-scoped production authority. A provider is admitted
    /// only through the secure local authority file; absent or invalid authority
    /// retains the explicit provider-not-configured state.
    #[must_use]
    pub fn new(host: String) -> Self {
        Self {
            host,
            state_path: std::env::var_os(STATE_PATH_ENV)
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(DEFAULT_STATE_PATH)),
            bus_root: navigation_bus_root(crate::bus_publish::default_bus_root()),
            poll: POLL,
            clock: Arc::new(SystemClock),
            provider: Arc::new(GovernedHttpRouteProvider::production()),
            authority: None,
            published_once: false,
            #[cfg(test)]
            bus_open_override: None,
        }
    }

    fn open_bus(&self) -> io::Result<Persist> {
        #[cfg(test)]
        if let Some(open) = self.bus_open_override.as_ref() {
            return open(&self.bus_root);
        }
        Persist::open(self.bus_root.clone()).map_err(io_other)
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
                NavigationPhase::Calculating { request_id, .. } => request_id.clone(),
                _ => return Err(io_invalid_data("navigation phase changed during recovery")),
            };
            let previous_generation = authority
                .snapshot
                .generation
                .checked_sub(1)
                .ok_or_else(|| io_invalid_data("calculating navigation has no prior generation"))?;
            if !authority.forget(&request_id) {
                return Err(io_invalid_data(
                    "calculating navigation is missing its replay reservation",
                ));
            }
            // A calculating snapshot is only an in-flight reservation. If the
            // worker stopped before the final state and cursor committed, roll
            // that reservation back so the still-unacknowledged Bus action can
            // be retried instead of being stranded behind a synthetic restart
            // generation.
            authority.snapshot.generation = previous_generation;
            authority.snapshot.produced_at_ms = now_ms;
            authority.snapshot.phase = NavigationPhase::Unavailable {
                request_id: Some(request_id),
                reason: NavigationUnavailableReason::InterruptedByRestart,
            };
        }
        store_record(&self.state_path, &authority)?;
        self.authority = Some(authority);
        Ok(())
    }

    fn collect_actions(&self, persist: &Persist) -> io::Result<Vec<PendingAction>> {
        self.collect_actions_with(|topic, cursor| {
            persist.list_since(topic, cursor).map_err(io_other)
        })
    }

    /// Stage every action lane before returning work. A failure on any lane
    /// rejects the complete candidate set, so callers cannot apply effects from
    /// a partial Bus view.
    fn collect_actions_with<F>(&self, mut read: F) -> io::Result<Vec<PendingAction>>
    where
        F: FnMut(&str, Option<&str>) -> io::Result<Vec<StoredMessage>>,
    {
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
            for message in read(&topic, cursor)? {
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

    fn commit_cursor(&mut self, kind: &ActionKind, ulid: String) -> io::Result<()> {
        let before = self.authority.as_ref().expect("authority loaded").clone();
        self.advance_cursor(kind, ulid);
        if let Err(error) = store_record(
            &self.state_path,
            self.authority.as_ref().expect("authority loaded"),
        ) {
            self.authority = Some(before);
            return Err(error);
        }
        Ok(())
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
        let replaced_route_id = match &self
            .authority
            .as_ref()
            .expect("authority loaded")
            .snapshot
            .phase
        {
            NavigationPhase::Active { route, .. } => Some(route.route_id.clone()),
            _ => None,
        };
        let pre_action = self.authority.as_ref().expect("authority loaded").clone();
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
        if let Err(error) = store_record(
            &self.state_path,
            self.authority.as_ref().expect("authority loaded"),
        ) {
            self.authority = Some(pre_action);
            return Err(error);
        }
        if let Err(publication_error) = self.publish(persist) {
            let rollback = store_record(&self.state_path, &pre_action);
            self.authority = Some(pre_action);
            return match rollback {
                Ok(()) => Err(publication_error),
                Err(rollback_error) => Err(io::Error::other(format!(
                    "publish calculating navigation: {publication_error}; restore pre-action authority: {rollback_error}"
                ))),
            };
        }
        let provider_result = self.provider.calculate(&request, now_ms);
        let completed_at_ms = self.clock.now_ms();
        let phase = match provider_result {
            Ok(route)
                if route_matches_request(
                    &route,
                    &request,
                    completed_at_ms,
                    replaced_route_id.as_deref(),
                ) =>
            {
                let progress = NavigationProgress {
                    route_id: route.route_id.clone(),
                    position: request.origin.point.clone(),
                    observed_at_ms: completed_at_ms,
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
        authority.snapshot.produced_at_ms = completed_at_ms;
        authority.snapshot.phase = phase;
        store_record(&self.state_path, authority)
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
                    && now_ms.saturating_sub(request.progress.observed_at_ms) <= MAX_ACTION_AGE_MS
                    && request.progress.validate_for(route, now_ms).is_ok()
            }
            _ => false,
        };
        if !valid {
            return;
        }
        let authority = self.authority.as_mut().expect("authority loaded");
        let Some(generation) = authority.snapshot.generation.checked_add(1) else {
            return;
        };
        if let NavigationPhase::Active { progress, .. } = &mut authority.snapshot.phase {
            *progress = request.progress;
        }
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

    fn tick_with_persist(&mut self, persist: &mut Persist) -> io::Result<()> {
        self.ensure_loaded()?;
        persist.reopen_if_index_changed();
        let actions = self.collect_actions(persist)?;
        if actions.is_empty() {
            if !self.published_once {
                self.publish(persist)?;
            }
            return Ok(());
        }
        for action in actions {
            let now_ms = self.clock.now_ms();
            match action.kind {
                ActionKind::Route => self.process_route(persist, action.body.as_bytes(), now_ms)?,
                ActionKind::Progress => self.process_progress(action.body.as_bytes(), now_ms),
                ActionKind::Cancel => self.process_cancel(action.body.as_bytes(), now_ms),
            }
            // Persist final authority without acknowledging the action. If
            // publication fails, the next pass republishes this checkpoint and
            // does not repeat an already-completed provider calculation.
            store_record(
                &self.state_path,
                self.authority.as_ref().expect("authority loaded"),
            )?;
            self.publish(persist)?;
            // The cursor is the acknowledgement boundary. It advances only
            // after all governed state and Bus effects complete successfully.
            self.commit_cursor(&action.kind, action.ulid)?;
        }
        Ok(())
    }

    #[cfg(test)]
    fn tick_once(&mut self) -> io::Result<()> {
        let mut persist = self.open_bus()?;
        self.tick_with_persist(&mut persist)
    }
}

#[async_trait::async_trait]
impl Worker for NavigationWorker {
    fn name(&self) -> &'static str {
        "navigation"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        self.ensure_loaded()?;
        let mut retry = MIN_BUS_RETRY;
        let mut persist = loop {
            match self.open_bus() {
                Ok(persist) => break persist,
                Err(error) => tracing::warn!(
                    %error,
                    bus_root = %self.bus_root.display(),
                    "navigation: Bus open failed; startup will retry"
                ),
            }
            tokio::select! {
                () = shutdown.wait() => return Ok(()),
                () = tokio::time::sleep(retry) => {}
            }
            retry = next_bus_retry(retry);
        };
        retry = MIN_BUS_RETRY;
        loop {
            let delay = match self.tick_with_persist(&mut persist) {
                Ok(()) => {
                    retry = MIN_BUS_RETRY;
                    self.poll
                }
                Err(error) => {
                    tracing::warn!(%error, "navigation: incomplete Bus transaction deferred");
                    let delay = retry;
                    retry = next_bus_retry(retry);
                    delay
                }
            };
            tokio::select! {
                () = shutdown.wait() => return Ok(()),
                () = tokio::time::sleep(delay) => {}
            }
        }
    }
}

fn navigation_bus_root(resolved: Option<PathBuf>) -> PathBuf {
    resolved.unwrap_or_else(|| PathBuf::from(mde_bus::SYSTEM_BUS_ROOT))
}

fn route_matches_request(
    route: &RouteResult,
    request: &RouteRequest,
    now_ms: i64,
    replaced_route_id: Option<&str>,
) -> bool {
    route.request_id == request.request_id
        && replaced_route_id != Some(route.route_id.as_str())
        && route.calculated_at_ms >= request.issued_at_ms
        && route.validate_at(now_ms).is_ok()
        && route.geometry.first() == Some(&request.origin.point)
        && route.geometry.last() == Some(&request.destination.point)
        && route.maneuvers.first().map(|maneuver| &maneuver.point) == Some(&request.origin.point)
        && route.maneuvers.last().map(|maneuver| &maneuver.point)
            == Some(&request.destination.point)
}

fn next_bus_retry(current: Duration) -> Duration {
    current
        .saturating_mul(2)
        .clamp(MIN_BUS_RETRY, MAX_BUS_RETRY)
}

fn load_record(path: &Path) -> io::Result<Option<PersistedNavigation>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.nlink() != 1
        || metadata.len() > MAX_PERSISTED_BYTES as u64
    {
        return Err(io_invalid_data(
            "navigation state is not a bounded unaliased regular file",
        ));
    }
    let mut options = OpenOptions::new();
    options.read(true).custom_flags(0o400_000 | 0o2_000_000); // O_NOFOLLOW | O_CLOEXEC
    let mut file = options.open(path)?;
    let opened = file.metadata()?;
    if !opened.is_file()
        || opened.nlink() != 1
        || opened.dev() != metadata.dev()
        || opened.ino() != metadata.ino()
    {
        return Err(io_invalid_data(
            "navigation state changed during secure open",
        ));
    }
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
    use ed25519_dalek::{Signer as _, SigningKey};
    use mackes_mesh_types::navigation::{
        ManeuverKind, RouteAttribution, RouteEndpoint, RouteManeuver, RouteProfile,
    };
    use mackes_mesh_types::nws_alert::GeoPoint;
    use std::io::BufRead as _;
    use std::net::{TcpListener, TcpStream};
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

    struct CountingProvider {
        calls: Arc<std::sync::atomic::AtomicUsize>,
        sabotage_state_topic: Option<PathBuf>,
    }

    impl RouteProvider for CountingProvider {
        fn calculate(
            &self,
            request: &RouteRequest,
            now_ms: i64,
        ) -> Result<RouteResult, RouteProviderError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(path) = &self.sabotage_state_topic {
                if path.is_dir() {
                    fs::remove_dir_all(path).unwrap();
                }
                fs::write(path, b"hostile non-directory").unwrap();
            }
            FixtureProvider.calculate(request, now_ms)
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

    struct StaleProvider;
    impl RouteProvider for StaleProvider {
        fn calculate(
            &self,
            request: &RouteRequest,
            now_ms: i64,
        ) -> Result<RouteResult, RouteProviderError> {
            let mut route = FixtureProvider.calculate(request, now_ms)?;
            route.calculated_at_ms = request.issued_at_ms - 1;
            Ok(route)
        }
    }

    struct ReusedRouteIdProvider;
    impl RouteProvider for ReusedRouteIdProvider {
        fn calculate(
            &self,
            request: &RouteRequest,
            now_ms: i64,
        ) -> Result<RouteResult, RouteProviderError> {
            let mut route = FixtureProvider.calculate(request, now_ms)?;
            route.route_id = "recycled-route-identity".into();
            Ok(route)
        }
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct OwnedProviderRouteRequest {
        schema_version: u16,
        request_sha256: String,
        request: RouteRequest,
    }

    fn write_provider_authority(
        path: &Path,
        endpoint: String,
        timeout_ms: u64,
        verifying_key: &VerifyingKey,
    ) -> u32 {
        let body = serde_json::to_vec(&RouteProviderAuthorityFile {
            schema_version: PROVIDER_SCHEMA_VERSION,
            provider_id: "fixture".into(),
            endpoint,
            timeout_ms,
            ed25519_public_key: encode_hex(verifying_key.as_bytes()),
        })
        .unwrap();
        fs::write(path, body).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::metadata(path).unwrap().uid()
    }

    fn read_provider_request(stream: &mut TcpStream) -> OwnedProviderRouteRequest {
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut reader = io::BufReader::new(stream);
        let mut request_line = String::new();
        reader.read_line(&mut request_line).unwrap();
        assert_eq!(request_line.trim_end(), "POST /v1/route HTTP/1.1");
        let mut content_length = None;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
            if let Some((name, value)) = line.split_once(':') {
                if name.eq_ignore_ascii_case("content-length") {
                    content_length = Some(value.trim().parse::<usize>().unwrap());
                }
            }
        }
        let length = content_length.expect("provider request Content-Length");
        assert!(length <= MAX_PROVIDER_AUTHORITY_BYTES);
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    fn serve_provider_once(
        listener: TcpListener,
        signing_key: SigningKey,
    ) -> std::thread::JoinHandle<()> {
        serve_provider_once_with(listener, signing_key, || {})
    }

    fn serve_provider_once_with<F>(
        listener: TcpListener,
        signing_key: SigningKey,
        before_reply: F,
    ) -> std::thread::JoinHandle<()>
    where
        F: FnOnce() + Send + 'static,
    {
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let provider_request = read_provider_request(&mut stream);
            assert_eq!(provider_request.schema_version, PROVIDER_SCHEMA_VERSION);
            assert_eq!(
                provider_request.request_sha256,
                sha256_hex(&serde_json::to_vec(&provider_request.request).unwrap())
            );
            let route = FixtureProvider
                .calculate(&provider_request.request, NOW)
                .unwrap();
            let signature = signing_key.sign(
                &provider_response_signing_bytes(&provider_request.request_sha256, &route).unwrap(),
            );
            let body = serde_json::to_vec(&ProviderRouteResponse {
                schema_version: PROVIDER_SCHEMA_VERSION,
                request_sha256: provider_request.request_sha256,
                route,
                signature: encode_hex(&signature.to_bytes()),
            })
            .unwrap();
            before_reply();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(&body).unwrap();
        })
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
                bus_root: bus.clone(),
                poll: POLL,
                clock: Arc::new(FixedClock),
                provider,
                authority: None,
                published_once: false,
                bus_open_override: None,
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
    fn navigation_bus_root_falls_back_to_canonical_system_spool() {
        assert_eq!(
            navigation_bus_root(None),
            PathBuf::from(mde_bus::SYSTEM_BUS_ROOT)
        );
        assert_eq!(
            navigation_bus_root(Some(PathBuf::from("/tmp/navigation-explicit-bus"))),
            PathBuf::from("/tmp/navigation-explicit-bus")
        );
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
    fn stale_progress_cannot_advance_an_active_route() {
        let mut fixture = Fixture::new(Arc::new(FixtureProvider));
        fixture.publish(
            &navigation_route_action_topic("seat-1"),
            &request(0, "req-stale-progress"),
        );
        fixture.worker.tick_once().unwrap();
        let active = fixture.worker.authority.as_ref().unwrap().snapshot.clone();
        let (route, progress) = match active.phase {
            NavigationPhase::Active { route, progress } => (route, progress),
            phase => panic!("unexpected {phase:?}"),
        };
        let mut stale = progress;
        stale.observed_at_ms = NOW - MAX_ACTION_AGE_MS - 1;
        fixture.publish(
            &navigation_progress_action_topic("seat-1"),
            &NavigationProgressRequest {
                schema_version: 1,
                request_id: "stale-progress".into(),
                host: "seat-1".into(),
                expected_generation: 1,
                issued_at_ms: NOW,
                route_id: route.route_id,
                progress: stale,
            },
        );
        fixture.worker.tick_once().unwrap();
        assert_eq!(
            fixture
                .worker
                .authority
                .as_ref()
                .unwrap()
                .snapshot
                .generation,
            1,
            "stale progress must not advance navigation authority"
        );
    }

    #[test]
    fn exhausted_generation_preserves_last_good_progress_atomically() {
        let mut fixture = Fixture::new(Arc::new(FixtureProvider));
        fixture.publish(
            &navigation_route_action_topic("seat-1"),
            &request(0, "req-1"),
        );
        fixture.worker.tick_once().unwrap();
        let active = fixture.worker.authority.as_ref().unwrap().snapshot.clone();
        let (route, mut progress) = match &active.phase {
            NavigationPhase::Active { route, progress } => (route.clone(), progress.clone()),
            phase => panic!("unexpected {phase:?}"),
        };
        fixture
            .worker
            .authority
            .as_mut()
            .unwrap()
            .snapshot
            .generation = u64::MAX;
        let exhausted = fixture.worker.authority.as_ref().unwrap().snapshot.clone();
        progress.position = GeoPoint {
            latitude: 40.05,
            longitude: -75.05,
        };
        progress.distance_remaining_metres = 500;
        progress.duration_remaining_seconds = 60;
        fixture.publish(
            &navigation_progress_action_topic("seat-1"),
            &NavigationProgressRequest {
                schema_version: 1,
                request_id: "progress-generation-exhausted".into(),
                host: "seat-1".into(),
                expected_generation: u64::MAX,
                issued_at_ms: NOW,
                route_id: route.route_id.clone(),
                progress,
            },
        );
        fixture.worker.tick_once().unwrap();
        assert_eq!(
            fixture.worker.authority.as_ref().unwrap().snapshot,
            exhausted,
            "generation exhaustion partially mutated progress"
        );
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
            authority.snapshot.generation = 1;
            authority.snapshot.phase = NavigationPhase::Calculating {
                request_id: "req-crash".into(),
                reroute: false,
            };
            authority.remember("req-crash".into());
            store_record(&fixture.worker.state_path, authority).unwrap();
        }
        fixture.worker.authority = None;
        fixture.worker.ensure_loaded().unwrap();
        assert_eq!(
            fixture
                .worker
                .authority
                .as_ref()
                .unwrap()
                .snapshot
                .generation,
            0
        );
        assert!(matches!(
            fixture.worker.authority.as_ref().unwrap().snapshot.phase,
            NavigationPhase::Unavailable {
                reason: NavigationUnavailableReason::InterruptedByRestart,
                ..
            }
        ));
    }

    #[test]
    fn restart_rejects_aliased_navigation_checkpoint() {
        let mut fixture = Fixture::new(Arc::new(FixtureProvider));
        fixture.worker.ensure_loaded().unwrap();
        fixture.worker.authority = None;
        let checkpoint = fixture.worker.state_path.clone();
        let parent = checkpoint.parent().unwrap();

        let symlink = parent.join("navigation-symlink.json");
        std::os::unix::fs::symlink(&checkpoint, &symlink).unwrap();
        fixture.worker.state_path = symlink;
        assert!(fixture.worker.ensure_loaded().is_err());
        assert!(fixture.worker.authority.is_none());

        let hardlink = parent.join("navigation-hardlink.json");
        fs::hard_link(checkpoint, &hardlink).unwrap();
        fixture.worker.state_path = hardlink;
        assert!(fixture.worker.ensure_loaded().is_err());
        assert!(fixture.worker.authority.is_none());
    }

    #[test]
    fn failed_calculating_publication_recovers_in_the_same_worker() {
        let mut fixture = Fixture::new(Arc::new(FixtureProvider));
        fixture.publish(
            &navigation_route_action_topic("seat-1"),
            &request(0, "req-retry"),
        );

        let state_topic_path = fixture.bus.join(navigation_state_topic("seat-1"));
        fs::create_dir_all(state_topic_path.parent().unwrap()).unwrap();
        fs::write(&state_topic_path, b"hostile non-directory").unwrap();
        assert!(fixture.worker.tick_once().is_err());
        let interrupted = load_record(&fixture.worker.state_path).unwrap().unwrap();
        assert_eq!(interrupted.route_cursor, None);
        assert!(matches!(interrupted.snapshot.phase, NavigationPhase::Idle));
        assert!(!interrupted.saw("req-retry"));

        fs::remove_file(state_topic_path).unwrap();
        fixture.worker.tick_once().unwrap();

        let recovered = fixture.worker.authority.as_ref().unwrap();
        assert_eq!(recovered.snapshot.generation, 1);
        assert!(recovered.route_cursor.is_some());
        assert!(matches!(
            recovered.snapshot.phase,
            NavigationPhase::Active { .. }
        ));
    }

    #[test]
    fn failed_final_publication_republishes_without_repeating_provider_effect() {
        let temp = tempfile::tempdir().unwrap();
        let bus = temp.path().join("bus");
        fs::create_dir_all(&bus).unwrap();
        let state_topic_path = bus.join(navigation_state_topic("seat-1"));
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let provider = Arc::new(CountingProvider {
            calls: Arc::clone(&calls),
            sabotage_state_topic: Some(state_topic_path.clone()),
        });
        let mut worker = NavigationWorker {
            host: "seat-1".into(),
            state_path: temp.path().join("navigation.json"),
            bus_root: bus.clone(),
            poll: POLL,
            clock: Arc::new(FixedClock),
            provider,
            authority: None,
            published_once: false,
            bus_open_override: None,
        };
        let writer = Persist::open(bus).unwrap();
        writer
            .write(
                &navigation_route_action_topic("seat-1"),
                Priority::Default,
                None,
                Some(&serde_json::to_string(&request(0, "req-final-retry")).unwrap()),
            )
            .unwrap();

        assert!(worker.tick_once().is_err());
        let checkpoint = load_record(&worker.state_path).unwrap().unwrap();
        assert_eq!(checkpoint.route_cursor, None);
        assert!(matches!(
            checkpoint.snapshot.phase,
            NavigationPhase::Active { .. }
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        fs::remove_file(state_topic_path).unwrap();
        worker.tick_once().unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(worker.authority.as_ref().unwrap().route_cursor.is_some());
    }

    #[test]
    fn incomplete_action_lane_read_defers_all_navigation_effects() {
        let mut fixture = Fixture::new(Arc::new(FixtureProvider));
        fixture.worker.ensure_loaded().unwrap();
        fixture.publish(
            &navigation_route_action_topic("seat-1"),
            &request(0, "req-partial"),
        );
        let before = fixture.worker.authority.as_ref().unwrap().clone();
        let reader = Persist::open(fixture.bus.clone()).unwrap();
        let reads = std::cell::Cell::new(0usize);

        let result = fixture.worker.collect_actions_with(|topic, cursor| {
            let index = reads.get();
            reads.set(index + 1);
            if index == 1 {
                return Err(io::Error::other("injected progress-lane read failure"));
            }
            reader.list_since(topic, cursor).map_err(io_other)
        });

        assert!(result.is_err());
        assert_eq!(reads.get(), 2);
        let after = fixture.worker.authority.as_ref().unwrap();
        assert_eq!(after.snapshot, before.snapshot);
        assert_eq!(after.route_cursor, before.route_cursor);
        assert_eq!(after.progress_cursor, before.progress_cursor);
        assert_eq!(after.cancel_cursor, before.cancel_cursor);
        assert_eq!(after.seen_request_ids, before.seen_request_ids);
        assert!(load_record(&fixture.worker.state_path)
            .unwrap()
            .unwrap()
            .route_cursor
            .is_none());
    }

    #[tokio::test]
    async fn late_bus_recovers_and_observes_external_forward_write_until_shutdown() {
        use std::sync::atomic::AtomicUsize;

        let temp = tempfile::tempdir().unwrap();
        let bus_root = temp.path().join("bus");
        let external_bus = Persist::open(bus_root.clone()).unwrap();
        external_bus
            .write(
                &navigation_route_action_topic("seat-1"),
                Priority::Default,
                None,
                Some(&serde_json::to_string(&request(0, "req-retained")).unwrap()),
            )
            .unwrap();

        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_for_open = Arc::clone(&attempts);
        let bus_for_open = bus_root.clone();
        let mut worker = NavigationWorker {
            host: "seat-1".into(),
            state_path: temp.path().join("navigation.json"),
            bus_root,
            poll: Duration::from_millis(5),
            clock: Arc::new(FixedClock),
            provider: Arc::new(FixtureProvider),
            authority: None,
            published_once: false,
            bus_open_override: Some(Arc::new(move |_| {
                match attempts_for_open.fetch_add(1, Ordering::SeqCst) {
                    0 => Err(io::Error::new(io::ErrorKind::NotFound, "late Bus")),
                    1 => Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "unopenable Bus",
                    )),
                    _ => Persist::open(bus_for_open.clone()).map_err(io_other),
                }
            })),
        };
        let state_topic = navigation_state_topic("seat-1");
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let task =
            tokio::spawn(
                async move { worker.run(ShutdownToken::from_receiver(shutdown_rx)).await },
            );

        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                assert!(
                    !task.is_finished(),
                    "worker exited during late-Bus recovery"
                );
                let reached_retained = external_bus
                    .list_since(&state_topic, None)
                    .unwrap()
                    .iter()
                    .filter_map(|message| message.body.as_deref())
                    .filter_map(|body| serde_json::from_str::<NavigationSnapshot>(body).ok())
                    .any(|snapshot| {
                        matches!(
                            snapshot.phase,
                            NavigationPhase::Active { ref route, .. }
                                if route.request_id == "req-retained"
                        )
                    });
                if reached_retained {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("retained route must project after late Bus recovery");
        assert!(attempts.load(Ordering::SeqCst) >= 3);

        external_bus
            .write(
                &navigation_route_action_topic("seat-1"),
                Priority::Default,
                None,
                Some(&serde_json::to_string(&request(1, "req-forward")).unwrap()),
            )
            .unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                assert!(!task.is_finished(), "worker exited before forward route");
                let reached_forward = external_bus
                    .list_since(&state_topic, None)
                    .unwrap()
                    .iter()
                    .filter_map(|message| message.body.as_deref())
                    .filter_map(|body| serde_json::from_str::<NavigationSnapshot>(body).ok())
                    .any(|snapshot| {
                        matches!(
                            snapshot.phase,
                            NavigationPhase::Active { ref route, .. }
                                if route.request_id == "req-forward"
                        )
                    });
                if reached_forward {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("external post-activation route must be observed");

        shutdown_tx.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .expect("shutdown must interrupt navigation worker")
            .expect("navigation task must join")
            .expect("navigation worker must stop cleanly");
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

    #[test]
    fn provider_result_older_than_request_is_refused() {
        let mut fixture = Fixture::new(Arc::new(StaleProvider));
        fixture.publish(
            &navigation_route_action_topic("seat-1"),
            &request(0, "req-stale-provider-result"),
        );

        fixture.worker.tick_once().unwrap();

        assert!(matches!(
            fixture.worker.authority.as_ref().unwrap().snapshot.phase,
            NavigationPhase::Unavailable {
                request_id: Some(ref request_id),
                reason: NavigationUnavailableReason::ProviderUnavailable,
            } if request_id == "req-stale-provider-result"
        ));
    }

    #[test]
    fn replacement_route_cannot_reuse_active_route_identity() {
        let mut fixture = Fixture::new(Arc::new(ReusedRouteIdProvider));
        fixture.publish(
            &navigation_route_action_topic("seat-1"),
            &request(0, "req-original-route"),
        );
        fixture.worker.tick_once().unwrap();
        assert!(matches!(
            fixture.worker.authority.as_ref().unwrap().snapshot.phase,
            NavigationPhase::Active { ref route, .. }
                if route.route_id == "recycled-route-identity"
        ));

        fixture.publish(
            &navigation_route_action_topic("seat-1"),
            &request(1, "req-replacement-route"),
        );
        fixture.worker.tick_once().unwrap();

        assert!(matches!(
            fixture.worker.authority.as_ref().unwrap().snapshot.phase,
            NavigationPhase::Unavailable {
                request_id: Some(ref request_id),
                reason: NavigationUnavailableReason::ProviderUnavailable,
            } if request_id == "req-replacement-route"
        ));
    }

    #[test]
    fn governed_production_provider_is_bounded_and_request_bound() {
        let authority_dir = tempfile::tempdir().unwrap();
        let authority_path = authority_dir.path().join("navigation-provider.json");
        let trusted_signer = SigningKey::from_bytes(&[17_u8; 32]);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/v1/route", listener.local_addr().unwrap());
        let owner_uid = write_provider_authority(
            &authority_path,
            endpoint,
            500,
            &trusted_signer.verifying_key(),
        );
        let server = serve_provider_once(listener, trusted_signer.clone());
        let mut fixture = Fixture::new(Arc::new(GovernedHttpRouteProvider::for_test(
            authority_path.clone(),
            owner_uid,
        )));
        fixture.publish(
            &navigation_route_action_topic("seat-1"),
            &request(0, "req-governed-provider"),
        );

        fixture.worker.tick_once().unwrap();
        server.join().unwrap();

        assert!(matches!(
            fixture.worker.authority.as_ref().unwrap().snapshot.phase,
            NavigationPhase::Active { ref route, .. }
                if route.request_id == "req-governed-provider"
                    && route.attribution.provider_id == "fixture"
                    && route.attribution.offline
        ));

        let impersonator = TcpListener::bind("127.0.0.1:0").unwrap();
        let impersonator_endpoint =
            format!("http://{}/v1/route", impersonator.local_addr().unwrap());
        write_provider_authority(
            &authority_path,
            impersonator_endpoint,
            500,
            &trusted_signer.verifying_key(),
        );
        let impersonator_server =
            serve_provider_once(impersonator, SigningKey::from_bytes(&[19_u8; 32]));
        let mut forged = Fixture::new(Arc::new(GovernedHttpRouteProvider::for_test(
            authority_path.clone(),
            owner_uid,
        )));
        forged.publish(
            &navigation_route_action_topic("seat-1"),
            &request(0, "req-forged-local-provider"),
        );
        forged.worker.tick_once().unwrap();
        impersonator_server.join().unwrap();
        assert!(matches!(
            forged.worker.authority.as_ref().unwrap().snapshot.phase,
            NavigationPhase::Unavailable {
                reason: NavigationUnavailableReason::ProviderUnavailable,
                ..
            }
        ));

        write_provider_authority(
            &authority_path,
            "http://198.51.100.9:8080/v1/route".into(),
            500,
            &trusted_signer.verifying_key(),
        );
        let mut remote = Fixture::new(Arc::new(GovernedHttpRouteProvider::for_test(
            authority_path.clone(),
            owner_uid,
        )));
        remote.publish(
            &navigation_route_action_topic("seat-1"),
            &request(0, "req-remote-refused"),
        );
        remote.worker.tick_once().unwrap();
        assert!(matches!(
            remote.worker.authority.as_ref().unwrap().snapshot.phase,
            NavigationPhase::Unavailable {
                reason: NavigationUnavailableReason::ProviderNotConfigured,
                ..
            }
        ));

        let stalled_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let stalled_endpoint =
            format!("http://{}/v1/route", stalled_listener.local_addr().unwrap());
        write_provider_authority(
            &authority_path,
            stalled_endpoint,
            MIN_PROVIDER_TIMEOUT_MS,
            &trusted_signer.verifying_key(),
        );
        let stalled_server = std::thread::spawn(move || {
            let (_stream, _) = stalled_listener.accept().unwrap();
            std::thread::sleep(Duration::from_millis(500));
        });
        let mut stalled = Fixture::new(Arc::new(GovernedHttpRouteProvider::for_test(
            authority_path,
            owner_uid,
        )));
        stalled.publish(
            &navigation_route_action_topic("seat-1"),
            &request(0, "req-timeout"),
        );
        let begun = std::time::Instant::now();
        stalled.worker.tick_once().unwrap();
        assert!(
            begun.elapsed() < Duration::from_millis(400),
            "provider I/O exceeded its governed timeout"
        );
        assert!(matches!(
            stalled.worker.authority.as_ref().unwrap().snapshot.phase,
            NavigationPhase::Unavailable {
                reason: NavigationUnavailableReason::ProviderUnavailable,
                ..
            }
        ));
        stalled_server.join().unwrap();
    }

    #[test]
    fn provider_authority_replacement_during_calculation_revokes_result() {
        let authority_dir = tempfile::tempdir().unwrap();
        let authority_path = authority_dir.path().join("navigation-provider.json");
        let trusted_signer = SigningKey::from_bytes(&[23_u8; 32]);
        let replacement_signer = SigningKey::from_bytes(&[29_u8; 32]);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/v1/route", listener.local_addr().unwrap());
        let owner_uid = write_provider_authority(
            &authority_path,
            endpoint.clone(),
            500,
            &trusted_signer.verifying_key(),
        );
        let replacement_path = authority_path.clone();
        let replacement_endpoint = endpoint;
        let server = serve_provider_once_with(listener, trusted_signer, move || {
            write_provider_authority(
                &replacement_path,
                replacement_endpoint,
                500,
                &replacement_signer.verifying_key(),
            );
        });
        let mut fixture = Fixture::new(Arc::new(GovernedHttpRouteProvider::for_test(
            authority_path,
            owner_uid,
        )));
        fixture.publish(
            &navigation_route_action_topic("seat-1"),
            &request(0, "req-revoked-provider"),
        );

        fixture.worker.tick_once().unwrap();
        server.join().unwrap();

        assert!(matches!(
            fixture.worker.authority.as_ref().unwrap().snapshot.phase,
            NavigationPhase::Unavailable {
                request_id: Some(ref request_id),
                reason: NavigationUnavailableReason::ProviderNotConfigured,
            } if request_id == "req-revoked-provider"
        ));
    }

    #[test]
    fn byte_identical_authority_replacement_cannot_authorize_in_flight_route() {
        let authority_dir = tempfile::tempdir().unwrap();
        let authority_path = authority_dir.path().join("navigation-provider.json");
        let trusted_signer = SigningKey::from_bytes(&[31_u8; 32]);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/v1/route", listener.local_addr().unwrap());
        let owner_uid = write_provider_authority(
            &authority_path,
            endpoint,
            500,
            &trusted_signer.verifying_key(),
        );
        let replacement_path = authority_path.clone();
        let server = serve_provider_once_with(listener, trusted_signer, move || {
            let replacement = replacement_path.with_extension("replacement");
            fs::copy(&replacement_path, &replacement).unwrap();
            fs::set_permissions(&replacement, fs::Permissions::from_mode(0o600)).unwrap();
            fs::rename(replacement, &replacement_path).unwrap();
        });
        let mut fixture = Fixture::new(Arc::new(GovernedHttpRouteProvider::for_test(
            authority_path,
            owner_uid,
        )));
        fixture.publish(
            &navigation_route_action_topic("seat-1"),
            &request(0, "req-byte-identical-authority-replacement"),
        );

        fixture.worker.tick_once().unwrap();
        server.join().unwrap();

        assert!(matches!(
            fixture.worker.authority.as_ref().unwrap().snapshot.phase,
            NavigationPhase::Unavailable {
                request_id: Some(ref request_id),
                reason: NavigationUnavailableReason::ProviderNotConfigured,
            } if request_id == "req-byte-identical-authority-replacement"
        ));
    }
}
