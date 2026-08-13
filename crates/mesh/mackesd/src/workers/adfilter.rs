//! Browser-VM filter-policy replication and allowlist service.
//!
//! The host stores and replicates opaque filter-list policy for Browser VMs. It
//! deliberately contains no URL matcher, cosmetic-rule engine, browser runtime,
//! or bundled host rule set. Request enforcement belongs to the guest.
//!
//! ## What this worker owns
//!
//! * **Per-node store replication** (the same substrate the [`super::bookmarks`]
//!   worker uses). Every node writes ONLY its own
//!   `<share>/adfilter/<node>/store.json` (single-writer → Syncthing never sees a
//!   write conflict) and *reads* every peer's store, folding them through the
//!   store's last-writer-wins merge into one converged store.
//! * **Leader compile** (lock: one compiler mesh-wide). The elected leader
//!   ([`crate::leader`], the shared `.mackesd-leader.lock`) serializes the
//!   converged store into the compiled engine blob at
//!   `<share>/adfilter/compiled/engine.json` — the single blob consumed by the
//!   guest/application policy path — and refreshes the enabled
//!   lists from upstream.
//! * **Airgap-honest refresh** (§7). The leader attempts an upstream refresh of
//!   each enabled list via the injectable [`ListFetcher`]; production reads an
//!   operator-provided local mirror (`<share>/adfilter/mirror/<name>.txt`,
//!   sneakernet-safe, no network) and — on a miss — falls back to the last-synced
//!   / bundled lists, publishing an honest [`Staleness`] indicator. It NEVER
//!   fabricates list text.
//! * **Per-site allowlist synced mesh-wide** (block-on-by-default). Drains
//!   `action/adfilter/{allow,block}` (a typed domain) into the store's allowlist,
//!   which replicates + LWW-merges over the same per-node store path.
//! * **State publish**. Publishes `state/adfilter/<node>` (per-node: enabled +
//!   total source counts, compiled rule counts, allowlist size, blob
//!   staleness/age) via the existing mackesd Bus [`Persist`] mechanism.
//!
//! ## §6 / §7 posture — nothing faked
//!
//! Like [`super::bookmarks`], this worker has no external transport to fake:
//! Syncthing does the replication out of band and the worker's job is real file
//! I/O against the shared dir — it runs unchanged on a headless farm box. The one
//! environmental condition is whether the canonical shared mount is present, the
//! existing [`crate::shared_root_writable`] guard (AUDIT-MESH-15): when it is not,
//! the worker keeps its node-local store and publishes an honest offline status,
//! never a faked converge nor a write into a bare unprovisioned mount. Timestamps
//! are injected (`now_fn`) so the model stays deterministic under test.

#![cfg(feature = "async-services")]

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;

use crate::ipc::action_auth::{ActionAuthorizer, MutationContext};

use super::{ShutdownToken, Worker};

/// Retained-latest topic prefix carrying this node's [`AdfilterStatus`]
/// (`state/adfilter/<node>`).
pub const STATE_PREFIX: &str = "state/adfilter/";

/// The `action/adfilter/` RPC domain prefix this worker drains (`allow`/`block`).
pub const ACTION_PREFIX: &str = "action/adfilter/";

/// The share subdirectory the per-node stores live under (`<root>/adfilter/…`).
pub const ADFILTER_SUBDIR: &str = "adfilter";

/// Each node's replicated store file name (single-writer per node).
pub const STORE_FILE: &str = "store.json";

/// The leader-compiled engine blob subdir + file (`<root>/adfilter/compiled/engine.json`).
pub const COMPILED_SUBDIR: &str = "compiled";
/// The leader-compiled engine blob file name.
pub const COMPILED_FILE: &str = "engine.json";

/// The operator's local list-mirror subdir (`<root>/adfilter/mirror/<name>.txt`) —
/// the airgap-safe upstream the leader refreshes from.
pub const MIRROR_SUBDIR: &str = "mirror";

/// Default poll/flush cadence. Filter lists change slowly (an operator edit or a
/// mirror drop); a 30 s tick keeps convergence prompt without polling storms.
pub const DEFAULT_TICK: Duration = Duration::from_secs(30);

/// Bounds for recovering a Bus that is unresolved, unopenable, or not yet
/// safe to activate. The same worker remains live while backing off.
const MIN_BUS_RETRY_INTERVAL: Duration = Duration::from_millis(10);
const MAX_BUS_RETRY_INTERVAL: Duration = Duration::from_secs(2);

/// Default freshness window: a sync older than this reads as [`Staleness::Stale`]
/// (7 days — EasyList's own refresh cadence).
pub const DEFAULT_FRESHNESS_MS: u64 = 7 * 24 * 60 * 60 * 1000;

/// A wall-clock source (ms since the Unix epoch). Injected so the model stays pure
/// and tests drive a deterministic fake clock.
type NowFn = Arc<dyn Fn() -> u64 + Send + Sync>;

/// Honest freshness state for the replicated Browser-VM policy envelope.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Staleness {
    /// A policy payload was synchronized within the freshness window.
    Fresh,
    /// The newest synchronized policy is older than the freshness window.
    Stale {
        /// Milliseconds since the newest successful policy synchronization.
        age_ms: u64,
    },
    /// No operator policy payload has synchronized yet.
    NeverSynced,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct FilterPolicySource {
    name: String,
    url: Option<String>,
    raw: String,
    enabled: bool,
    updated_ms: u64,
}

impl FilterPolicySource {
    fn mirror(name: String) -> Self {
        Self {
            url: Some(format!("mirror://{name}")),
            name,
            raw: String::new(),
            enabled: true,
            updated_ms: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct AllowlistEntry {
    allowed: bool,
    added_by: String,
    updated_ms: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct FilterPolicyStore {
    sources: Vec<FilterPolicySource>,
    allowlist: BTreeMap<String, AllowlistEntry>,
    synced_ms: Option<u64>,
}

impl FilterPolicyStore {
    fn sources(&self) -> &[FilterPolicySource] {
        &self.sources
    }

    fn enabled_sources(&self) -> impl Iterator<Item = &FilterPolicySource> {
        self.sources.iter().filter(|source| source.enabled)
    }

    fn add_source(&mut self, source: FilterPolicySource) {
        if self
            .sources
            .iter()
            .all(|current| current.name != source.name)
        {
            self.sources.push(source);
        }
    }

    fn update_source(&mut self, name: &str, raw: String, now_ms: u64) -> bool {
        let Some(source) = self.sources.iter_mut().find(|source| source.name == name) else {
            return false;
        };
        source.raw = raw;
        source.updated_ms = now_ms;
        self.synced_ms = Some(now_ms);
        true
    }

    fn allow_site(&mut self, domain: &str, by: &str, now_ms: u64) {
        self.set_site(domain, true, by, now_ms);
    }

    fn block_site(&mut self, domain: &str, by: &str, now_ms: u64) {
        self.set_site(domain, false, by, now_ms);
    }

    fn set_site(&mut self, domain: &str, allowed: bool, by: &str, now_ms: u64) {
        let domain = domain.to_ascii_lowercase();
        if self
            .allowlist
            .get(&domain)
            .is_none_or(|entry| now_ms >= entry.updated_ms)
        {
            self.allowlist.insert(
                domain,
                AllowlistEntry {
                    allowed,
                    added_by: by.to_string(),
                    updated_ms: now_ms,
                },
            );
        }
    }

    #[cfg(test)]
    fn is_allowed(&self, domain: &str) -> bool {
        self.allowlist
            .get(&domain.to_ascii_lowercase())
            .is_some_and(|entry| entry.allowed)
    }

    fn allowed_count(&self) -> usize {
        self.allowlist
            .values()
            .filter(|entry| entry.allowed)
            .count()
    }

    fn merge(&mut self, other: &Self) {
        for source in &other.sources {
            match self
                .sources
                .iter_mut()
                .find(|mine| mine.name == source.name)
            {
                Some(mine) if source.updated_ms > mine.updated_ms => *mine = source.clone(),
                Some(_) => {}
                None => self.sources.push(source.clone()),
            }
        }
        for (domain, entry) in &other.allowlist {
            if self
                .allowlist
                .get(domain)
                .is_none_or(|mine| entry.updated_ms > mine.updated_ms)
            {
                self.allowlist.insert(domain.clone(), entry.clone());
            }
        }
        self.synced_ms = self.synced_ms.max(other.synced_ms);
    }

    fn synced_ms(&self) -> Option<u64> {
        self.synced_ms
    }

    fn staleness(&self, now_ms: u64, ttl_ms: u64) -> Staleness {
        self.synced_ms.map_or(Staleness::NeverSynced, |synced_ms| {
            let age_ms = now_ms.saturating_sub(synced_ms);
            if age_ms <= ttl_ms {
                Staleness::Fresh
            } else {
                Staleness::Stale { age_ms }
            }
        })
    }

    fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    fn rule_counts(&self) -> (usize, usize) {
        self.enabled_sources()
            .flat_map(|source| source.raw.lines())
            .fold((0, 0), |(network, cosmetic), line| {
                let line = line.trim();
                if line.is_empty() || line.starts_with('!') || line.starts_with('[') {
                    (network, cosmetic)
                } else if line.contains("##") || line.contains("#@#") {
                    (network, cosmetic + 1)
                } else {
                    (network + 1, cosmetic)
                }
            })
    }
}

// ── the upstream-refresh seam ────────────────────────────────────────────────

/// One list's upstream-refresh outcome.
pub enum RefreshOutcome {
    /// Fresh list text was obtained.
    Fetched(String),
    /// Upstream is unavailable (airgapped / no mirror) — keep the last-synced or
    /// bundled copy. NEVER a fabricated body.
    Unavailable,
}

/// The upstream list-refresh seam. Airgap-honest: an implementation returns
/// [`RefreshOutcome::Unavailable`] rather than inventing list text when it can't
/// reach an upstream.
pub trait ListFetcher: Send + Sync {
    /// Attempt to refresh the list named `name` (its upstream `url` is advisory).
    fn fetch(&self, name: &str, url: &str) -> RefreshOutcome;
}

/// The production fetcher: an **airgap-safe local mirror**. The mesh never reaches
/// upstream directly (no `adblock-rust` fetch — the crate is airgap-trivial by
/// design); instead an operator drops a refreshed EasyList body into
/// `<share>/adfilter/mirror/<name>.txt` (sneakernet or a gated mirror job), and the
/// leader picks it up here. A missing mirror file is an honest
/// [`RefreshOutcome::Unavailable`] → the fallback to the last-synced / bundled
/// lists + a [`Staleness`] indicator.
pub struct MirrorFetcher {
    mirror_dir: PathBuf,
}

impl MirrorFetcher {
    /// A fetcher reading list mirrors from `mirror_dir`.
    #[must_use]
    pub const fn new(mirror_dir: PathBuf) -> Self {
        Self { mirror_dir }
    }
}

impl ListFetcher for MirrorFetcher {
    fn fetch(&self, name: &str, _url: &str) -> RefreshOutcome {
        let path = self.mirror_dir.join(format!("{}.txt", sanitize_name(name)));
        match std::fs::read_to_string(&path) {
            Ok(text) if !text.trim().is_empty() => RefreshOutcome::Fetched(text),
            _ => RefreshOutcome::Unavailable,
        }
    }
}

/// Reduce a source name to a safe file stem (no path traversal, no separators).
#[must_use]
fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

// ── the published status ──────────────────────────────────────────────────────

/// The per-node ad-filter status published to `state/adfilter/<node>` — the
/// operator's "N lists, M rules, X days old" indicator (BOOKMARKS-7 §6).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AdfilterStatus {
    /// This node's id.
    pub node: String,
    /// Enabled filter sources (the engine compiles these).
    pub enabled_sources: usize,
    /// Total filter sources (enabled or not).
    pub total_sources: usize,
    /// Network block+allow rules the compiled engine holds.
    pub network_rules: usize,
    /// Cosmetic hide+unhide rules the compiled engine holds.
    pub cosmetic_rules: usize,
    /// Sites currently allowlisted (blocking off) mesh-wide.
    pub allowlisted_sites: usize,
    /// How fresh the lists are (the honest staleness indicator).
    pub staleness: Staleness,
    /// Age (ms) since the last successful upstream sync, if ever synced.
    pub age_ms: Option<u64>,
    /// Wall-clock ms of the last successful upstream sync, if any.
    pub synced_ms: Option<u64>,
    /// How many *other* nodes' stores this node is merging.
    pub peers: usize,
    /// Whether the shared Syncthing folder was present + writable this tick.
    pub share_reachable: bool,
    /// Wall-clock ms of the last flush.
    pub last_flush_ms: u64,
}

// ── the typed action ─────────────────────────────────────────────────────────

/// A typed `action/adfilter/<verb>` request (block-on-by-default per-site opt-out).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdfilterAction {
    /// Allowlist a first-party site (turn blocking off for it).
    Allow {
        /// The first-party domain.
        domain: String,
    },
    /// Re-enable blocking on a first-party site.
    Block {
        /// The first-party domain.
        domain: String,
    },
}

#[derive(serde::Deserialize)]
struct DomainReq {
    domain: String,
}

/// Parse a typed [`AdfilterAction`] from the topic's `<verb>` slot + JSON body.
///
/// # Errors
/// An unknown verb or a body missing `domain` returns a human-readable message.
pub fn parse_action(verb: &str, body: &str) -> Result<AdfilterAction, String> {
    let body = body.trim();
    let json = if body.is_empty() { "{}" } else { body };
    let malformed = |e: serde_json::Error| format!("malformed `{verb}` adfilter request: {e}");
    let domain = |raw: &str| -> Result<String, String> {
        let r: DomainReq = serde_json::from_str(raw).map_err(malformed)?;
        let d = r.domain.trim().to_ascii_lowercase();
        if d.is_empty() {
            Err(format!("empty `domain` in `{verb}` adfilter request"))
        } else {
            Ok(d)
        }
    };
    match verb {
        "allow" => Ok(AdfilterAction::Allow {
            domain: domain(json)?,
        }),
        "block" => Ok(AdfilterAction::Block {
            domain: domain(json)?,
        }),
        other => Err(format!("unknown adfilter action verb `{other}`")),
    }
}

// ── path helpers ─────────────────────────────────────────────────────────────

fn adfilter_dir(root: &Path) -> PathBuf {
    root.join(ADFILTER_SUBDIR)
}
fn node_dir(root: &Path, node: &str) -> PathBuf {
    adfilter_dir(root).join(node)
}
fn store_path(root: &Path, node: &str) -> PathBuf {
    node_dir(root, node).join(STORE_FILE)
}
fn compiled_path(root: &Path) -> PathBuf {
    adfilter_dir(root).join(COMPILED_SUBDIR).join(COMPILED_FILE)
}
fn mirror_dir(root: &Path) -> PathBuf {
    adfilter_dir(root).join(MIRROR_SUBDIR)
}

/// Load a store from `path`, or `None` when absent / corrupt (a peer-supplied file
/// never panics the reader).
fn load_store(path: &Path) -> Option<FilterPolicyStore> {
    let text = std::fs::read_to_string(path).ok()?;
    FilterPolicyStore::from_json(&text).ok()
}

// ── the worker ───────────────────────────────────────────────────────────────

#[cfg(test)]
type BusOpenFn = dyn Fn(&Path) -> Result<Option<Persist>, String> + Send + Sync;

#[cfg(test)]
type CursorPrimeFn =
    dyn Fn(&Persist) -> Result<HashMap<String, Option<String>>, String> + Send + Sync;

#[cfg(test)]
type RequestReadGateFn = dyn Fn(&str, usize) -> Result<(), String> + Send + Sync;

/// BOOKMARKS-7 — the mesh-wide ad-filter worker.
pub struct AdfilterWorker {
    /// This node's id (the store owner + status key).
    node: String,
    /// Node-local durable root (offline-first + restart durability).
    local_root: PathBuf,
    /// The shared Syncthing root: this node mirrors its own store here + reads peers.
    share_root: PathBuf,
    /// The shared leader lock (reused across the leader-gated workers).
    leader_lock: PathBuf,
    /// This node's authoritative own store (bundled seed + local edits/refreshes).
    own: FilterPolicyStore,
    /// The converged store (own ⊕ every peer) — published + compiled.
    converged: FilterPolicyStore,
    /// The injectable upstream-refresh seam.
    fetcher: Arc<dyn ListFetcher>,
    /// Freshness window (ms) for the staleness classification.
    freshness_ms: u64,
    /// Peer count observed on the last rebuild.
    peer_count: usize,
    /// Wall-clock ms of the last flush.
    last_flush_ms: u64,
    /// Poll/flush cadence.
    tick: Duration,
    /// Per-topic action cursors (`action/adfilter/<verb>` → last ULID). A
    /// present `None` is an existing empty topic primed during activation; an
    /// absent topic appeared afterward and drains its first forward message.
    cursors: HashMap<String, Option<String>>,
    /// Injected wall clock.
    now_fn: NowFn,
    /// Test seam forcing the share up/down; `None` → the real writable guard.
    share_gate: Option<Arc<AtomicBool>>,
    /// Bus spool root override (tests point this at a tempdir).
    bus_root_override: Option<PathBuf>,
    /// Dynamic Bus seams for deterministic startup/read-failure tests.
    #[cfg(test)]
    bus_open_override: Option<Arc<BusOpenFn>>,
    #[cfg(test)]
    cursor_prime_override: Option<Arc<CursorPrimeFn>>,
    #[cfg(test)]
    request_read_gate: Option<Arc<RequestReadGateFn>>,
    /// Exact-body capability verifier for cross-UID allowlist mutations.
    authorizer: Arc<ActionAuthorizer>,
}

impl AdfilterWorker {
    /// Construct with production defaults. `local_root` is a node-local durable dir
    /// ([`resolve_local_root`]); `share_root` is the mesh workgroup root.
    #[must_use]
    pub fn new(node: String, local_root: PathBuf, share_root: PathBuf) -> Self {
        let fetcher = Arc::new(MirrorFetcher::new(mirror_dir(&share_root)));
        Self {
            leader_lock: share_root.join(".mackesd-leader.lock"),
            fetcher,
            node,
            local_root,
            share_root,
            own: FilterPolicyStore::default(),
            converged: FilterPolicyStore::default(),
            freshness_ms: DEFAULT_FRESHNESS_MS,
            peer_count: 0,
            last_flush_ms: 0,
            tick: DEFAULT_TICK,
            cursors: HashMap::new(),
            now_fn: Arc::new(default_now),
            share_gate: None,
            bus_root_override: None,
            #[cfg(test)]
            bus_open_override: None,
            #[cfg(test)]
            cursor_prime_override: None,
            #[cfg(test)]
            request_read_gate: None,
            authorizer: Arc::new(ActionAuthorizer::production()),
        }
    }

    /// Inject a deterministic wall clock (tests).
    #[must_use]
    pub fn with_now_fn(mut self, now: NowFn) -> Self {
        self.now_fn = now;
        self
    }

    /// Inject a share-availability gate (offline-first tests).
    #[must_use]
    pub fn with_share_gate(mut self, gate: Arc<AtomicBool>) -> Self {
        self.share_gate = Some(gate);
        self
    }

    /// Override the poll/flush cadence (tests use a short value).
    #[must_use]
    pub const fn with_tick(mut self, d: Duration) -> Self {
        self.tick = d;
        self
    }

    /// Override the Bus spool root (tests).
    #[must_use]
    pub fn with_bus_root(mut self, root: PathBuf) -> Self {
        self.bus_root_override = Some(root);
        self
    }

    #[cfg(test)]
    #[must_use]
    fn with_bus_opener(mut self, open: Arc<BusOpenFn>) -> Self {
        self.bus_open_override = Some(open);
        self
    }

    #[cfg(test)]
    #[must_use]
    fn with_cursor_primer(mut self, prime: Arc<CursorPrimeFn>) -> Self {
        self.cursor_prime_override = Some(prime);
        self
    }

    #[cfg(test)]
    #[must_use]
    fn with_request_read_gate(mut self, gate: Arc<RequestReadGateFn>) -> Self {
        self.request_read_gate = Some(gate);
        self
    }

    /// Inject an isolated verifier and replay ledger for hostile action tests.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_authorizer(mut self, authorizer: Arc<ActionAuthorizer>) -> Self {
        self.authorizer = authorizer;
        self
    }

    /// Inject a custom upstream-refresh fetcher (tests).
    #[must_use]
    pub fn with_fetcher(mut self, fetcher: Arc<dyn ListFetcher>) -> Self {
        self.fetcher = fetcher;
        self
    }

    fn now_ms(&self) -> u64 {
        (self.now_fn)()
    }

    fn open_bus(&self, root: &Path) -> Result<Option<Persist>, String> {
        #[cfg(test)]
        if let Some(open) = self.bus_open_override.as_ref() {
            return open(root);
        }

        Persist::open(root.to_path_buf())
            .map(Some)
            .map_err(|error| error.to_string())
    }

    fn prime_action_cursors(
        &self,
        persist: &Persist,
    ) -> Result<HashMap<String, Option<String>>, String> {
        #[cfg(test)]
        if let Some(prime) = self.cursor_prime_override.as_ref() {
            return prime(persist);
        }

        prime_action_cursors(persist)
    }

    /// Whether the shared folder is present + writable this tick. The test gate
    /// wins when set; otherwise the AUDIT-MESH-15 canonical-mount guard.
    fn share_writable(&self) -> bool {
        self.share_gate.as_ref().map_or_else(
            || crate::shared_root_writable(&self.share_root),
            |g| g.load(Ordering::SeqCst),
        )
    }

    /// Is this node the directory leader (reuses the shared leader lock)? Only the
    /// leader refreshes lists + compiles the shared blob (one compiler mesh-wide).
    fn is_leader(&self) -> bool {
        crate::leader_gate::LeaderGate::from_lock_path(self.leader_lock.clone(), self.node.clone())
            .is_leader()
    }

    /// Restore this node's authoritative own store from `local_root` (offline-
    /// proof), else start with an empty policy envelope, then rebuild the
    /// converged view.
    fn load(&mut self) {
        self.own = load_store(&store_path(&self.local_root, &self.node)).unwrap_or_default();
        self.rebuild_converged();
    }

    /// Apply a typed action to the own store's allowlist (block-on-by-default
    /// opt-out), attributed to this node + stamped now.
    fn apply_action(&mut self, action: AdfilterAction) {
        let now = self.now_ms();
        match action {
            AdfilterAction::Allow { domain } => self.own.allow_site(&domain, &self.node, now),
            AdfilterAction::Block { domain } => self.own.block_site(&domain, &self.node, now),
        }
    }

    /// LEADER-ONLY: attempt an upstream refresh of every enabled list via the
    /// fetcher, updating the own store on a fresh, changed body. Airgap-honest —
    /// an [`RefreshOutcome::Unavailable`] leaves the last-synced copy and
    /// never stamps a sync. Returns whether any list changed.
    fn refresh_lists(&mut self) -> bool {
        let now = self.now_ms();
        if let Ok(entries) = std::fs::read_dir(mirror_dir(&self.share_root)) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("txt") {
                    continue;
                }
                let Some(name) = path.file_stem().and_then(|stem| stem.to_str()) else {
                    continue;
                };
                self.own
                    .add_source(FilterPolicySource::mirror(name.to_string()));
            }
        }
        // Snapshot the (name, url, current-raw) of enabled sources with an upstream.
        let targets: Vec<(String, String, String)> = self
            .own
            .sources()
            .iter()
            .filter(|s| s.enabled)
            .filter_map(|s| {
                s.url
                    .clone()
                    .map(|url| (s.name.clone(), url, s.raw.clone()))
            })
            .collect();
        let mut changed = false;
        for (name, url, current) in targets {
            if let RefreshOutcome::Fetched(text) = self.fetcher.fetch(&name, &url) {
                if text != current {
                    self.own.update_source(&name, text, now);
                    changed = true;
                }
            }
        }
        changed
    }

    /// Persist this node's authoritative own store to `local_root` (restart-proof).
    fn persist_own_local(&self) {
        let dir = node_dir(&self.local_root, &self.node);
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        if let Ok(json) = self.own.to_json() {
            let _ = std::fs::write(store_path(&self.local_root, &self.node), json);
        }
    }

    /// Mirror this node's own store into the shared Syncthing folder so peers can
    /// merge it. A no-op while the share is down (offline). NEVER writes into a bare
    /// unprovisioned canonical mount (AUDIT-MESH-15). Returns whether it mirrored.
    fn mirror_to_share(&self) -> bool {
        if !self.share_writable() {
            return false;
        }
        let dir = node_dir(&self.share_root, &self.node);
        if std::fs::create_dir_all(&dir).is_err() {
            return false;
        }
        let Ok(json) = self.own.to_json() else {
            return false;
        };
        std::fs::write(store_path(&self.share_root, &self.node), json).is_ok()
    }

    /// Rebuild the converged store: own ⊕ every peer's store (LWW-merge). Also
    /// counts the peers merged (for the status).
    fn rebuild_converged(&mut self) {
        let mut converged = self.own.clone();
        let mut peers = 0usize;
        if let Ok(rd) = std::fs::read_dir(adfilter_dir(&self.share_root)) {
            for entry in rd.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let name = entry.file_name();
                let Some(node) = name.to_str() else {
                    continue;
                };
                // Skip self + the non-peer service dirs.
                if node == self.node || node == COMPILED_SUBDIR || node == MIRROR_SUBDIR {
                    continue;
                }
                if let Some(peer) = load_store(&path.join(STORE_FILE)) {
                    converged.merge(&peer);
                    peers += 1;
                }
            }
        }
        self.peer_count = peers;
        self.converged = converged;
    }

    /// LEADER-ONLY: serialize the converged policy envelope for Browser VMs at
    /// `<share>/adfilter/compiled/engine.json`.
    /// A no-op while the share is down. Returns whether it wrote.
    fn compile_blob(&self) -> bool {
        if !self.share_writable() {
            return false;
        }
        let path = compiled_path(&self.share_root);
        if let Some(parent) = path.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                return false;
            }
        }
        let Ok(json) = self.converged.to_json() else {
            return false;
        };
        std::fs::write(path, json).is_ok()
    }

    /// The current published status derived from the converged store.
    #[must_use]
    pub fn status(&self) -> AdfilterStatus {
        let now = self.now_ms();
        let staleness = self.converged.staleness(now, self.freshness_ms);
        let age_ms = self.converged.synced_ms().map(|s| now.saturating_sub(s));
        let (network_rules, cosmetic_rules) = self.converged.rule_counts();
        AdfilterStatus {
            node: self.node.clone(),
            enabled_sources: self.converged.enabled_sources().count(),
            total_sources: self.converged.sources().len(),
            network_rules,
            cosmetic_rules,
            allowlisted_sites: self.converged.allowed_count(),
            staleness,
            age_ms,
            synced_ms: self.converged.synced_ms(),
            peers: self.peer_count,
            share_reachable: self.share_writable(),
            last_flush_ms: self.last_flush_ms,
        }
    }

    /// One convergence pass (no Bus): leader refresh + compile, mirror own out,
    /// merge peers in. Split from [`Self::flush`] so tests drive convergence without
    /// a Bus.
    fn sync(&mut self) {
        let leader = self.is_leader();
        if leader {
            self.refresh_lists();
        }
        self.persist_own_local();
        let _ = self.mirror_to_share();
        self.rebuild_converged();
        if leader {
            let _ = self.compile_blob();
        }
        self.last_flush_ms = self.now_ms();
    }

    /// Publish `state/adfilter/<node>`.
    fn publish_state(&self, persist: &Persist) {
        let topic = format!("{STATE_PREFIX}{}", self.node);
        if let Ok(body) = serde_json::to_string(&self.status()) {
            if let Err(e) = persist.write(&topic, Priority::Default, None, Some(&body)) {
                tracing::warn!(target: "mackesd::adfilter", error = %e, "state publish failed");
            }
        }
    }

    /// A sync pass + publish (the tick body's convergence half).
    fn flush(&mut self, persist: &Persist) {
        self.sync();
        self.publish_state(persist);
    }

    /// Drain net-new `action/adfilter/{allow,block}` requests, applying each to the
    /// own store's allowlist. Publishes immediately when any landed so the surface
    /// reflects the edit without waiting for the flush.
    fn drain_requests(&mut self, persist: &Persist) -> bool {
        let topics = match persist.list_topics() {
            Ok(t) => t,
            Err(e) => {
                tracing::debug!(target: "mackesd::adfilter", error = %e, "list_topics failed");
                return false;
            }
        };
        let mut candidate_cursors = self.cursors.clone();
        let mut requests = Vec::new();
        let topics = topics
            .into_iter()
            .filter(|t| t.starts_with(ACTION_PREFIX) && t.len() > ACTION_PREFIX.len());
        #[cfg(test)]
        let topics = topics.enumerate();
        #[cfg(not(test))]
        let topics = topics.map(|topic| ((), topic));
        for (_index, topic) in topics {
            #[cfg(test)]
            if let Some(gate) = self.request_read_gate.as_ref() {
                if let Err(error) = gate(&topic, _index) {
                    tracing::debug!(target: "mackesd::adfilter", topic, %error, "injected list_since failure");
                    return false;
                }
            }
            let verb = topic[ACTION_PREFIX.len()..].to_string();
            let cursor = self.cursors.get(&topic).and_then(Option::as_deref);
            let messages = match persist.list_since(&topic, cursor) {
                Ok(m) => m,
                Err(e) => {
                    tracing::debug!(target: "mackesd::adfilter", topic, error = %e, "list_since failed");
                    return false;
                }
            };
            if let Some(tail) = messages.last().map(|message| message.ulid.clone()) {
                candidate_cursors.insert(topic, Some(tail));
            }
            requests.push((verb, messages));
        }

        // A failed read means unavailable state, not an empty command set. Only
        // install cursors and execute mutations after the whole sweep succeeds.
        self.cursors = candidate_cursors;
        let mut changed = false;
        for (verb, messages) in requests {
            for msg in messages {
                let body = msg.body.as_deref().unwrap_or_default();
                let action = match parse_action(&verb, body) {
                    Ok(action) => action,
                    Err(e) => {
                        tracing::warn!(target: "mackesd::adfilter", verb = %verb, error = %e, "bad request");
                        continue;
                    }
                };
                let target = match &action {
                    AdfilterAction::Allow { domain } | AdfilterAction::Block { domain } => {
                        domain.as_str()
                    }
                };
                let auth_verb = format!("adfilter-{verb}");
                if let Err(e) = self.authorizer.authorize(
                    body,
                    MutationContext {
                        verb: &auth_verb,
                        node: &self.node,
                        target,
                    },
                ) {
                    tracing::warn!(
                        target: "mackesd::adfilter",
                        verb = %verb,
                        error = %e,
                        "refused unauthorized allowlist mutation"
                    );
                    continue;
                }
                self.apply_action(action);
                changed = true;
            }
        }
        if changed {
            // Persist + mirror the allowlist edit right away, then republish.
            self.persist_own_local();
            let _ = self.mirror_to_share();
            self.rebuild_converged();
            self.publish_state(persist);
        }
        true
    }
}

/// Discover and tail-prime every existing mutation topic as one activation
/// transaction. The caller installs this map only after every tail read succeeds.
fn prime_action_cursors(persist: &Persist) -> Result<HashMap<String, Option<String>>, String> {
    let topics = persist
        .list_topics()
        .map_err(|error| format!("discover adfilter action topics: {error}"))?;
    let mut cursors = HashMap::new();
    for topic in topics
        .into_iter()
        .filter(|topic| topic.starts_with(ACTION_PREFIX) && topic.len() > ACTION_PREFIX.len())
    {
        let tail = persist
            .latest_ulid(&topic)
            .map_err(|error| format!("prime {topic}: {error}"))?;
        cursors.insert(topic, tail);
    }
    Ok(cursors)
}

fn adfilter_bus_root(override_root: Option<PathBuf>) -> PathBuf {
    adfilter_bus_root_or_system(override_root.or_else(mde_bus::default_data_dir))
}

fn adfilter_bus_root_or_system(resolved: Option<PathBuf>) -> PathBuf {
    resolved.unwrap_or_else(|| PathBuf::from(mde_bus::SYSTEM_BUS_ROOT))
}

fn next_bus_retry_interval(current: Duration) -> Duration {
    current
        .saturating_mul(2)
        .clamp(MIN_BUS_RETRY_INTERVAL, MAX_BUS_RETRY_INTERVAL)
}

#[async_trait::async_trait]
impl Worker for AdfilterWorker {
    fn name(&self) -> &'static str {
        "adfilter"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        // Durable policy is filesystem-backed and survives a Bus outage; restore
        // it before waiting for the transient command transport to activate.
        self.load();
        let bus_root = adfilter_bus_root(self.bus_root_override.clone());
        let mut retry_interval = MIN_BUS_RETRY_INTERVAL;
        let persist = loop {
            match self.open_bus(&bus_root) {
                Ok(Some(persist)) => match self.prime_action_cursors(&persist) {
                    Ok(cursors) => {
                        self.cursors = cursors;
                        break persist;
                    }
                    Err(error) => tracing::warn!(
                        target: "mackesd::adfilter",
                        %error,
                        "action-topic activation failed; adfilter startup will retry"
                    ),
                },
                Ok(None) => tracing::debug!(
                    target: "mackesd::adfilter",
                    "Bus root unavailable; adfilter startup will retry"
                ),
                Err(error) => tracing::warn!(
                    target: "mackesd::adfilter",
                    %error,
                    "Persist open failed; adfilter startup will retry"
                ),
            }
            tokio::select! {
                () = shutdown.wait() => return Ok(()),
                () = tokio::time::sleep(retry_interval) => {}
            }
            retry_interval = next_bus_retry_interval(retry_interval);
        };
        self.flush(&persist); // publish the initial converged state
        let mut tick = tokio::time::interval(self.tick);
        tick.tick().await; // burn the immediate first tick
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    if self.drain_requests(&persist) {
                        self.flush(&persist);
                    }
                }
                () = shutdown.wait() => break,
            }
        }
        // Clean shutdown: persist + a final mirror so a restart resumes exactly.
        self.persist_own_local();
        let _ = self.mirror_to_share();
        Ok(())
    }
}

/// Resolve the node-local durable adfilter root
/// (`<XDG_DATA_HOME>/mde/adfilter`, or `/var/lib/mde/adfilter` headless).
#[must_use]
pub fn resolve_local_root() -> PathBuf {
    dirs::data_dir().map_or_else(
        || PathBuf::from("/var/lib/mde/adfilter"),
        |d| d.join("mde").join("adfilter"),
    )
}

/// Wall-clock epoch millis (the production [`NowFn`]).
fn default_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::action_auth::{authorize_test_body, ActionAuthorizer, MutationContext};
    use std::sync::atomic::AtomicU64;

    fn fake_clock(start: u64) -> (Arc<AtomicU64>, NowFn) {
        let cell = Arc::new(AtomicU64::new(start));
        let reader = cell.clone();
        let now: NowFn = Arc::new(move || reader.load(Ordering::SeqCst));
        (cell, now)
    }

    fn worker(node: &str, local: &Path, share: &Path, now: NowFn) -> AdfilterWorker {
        AdfilterWorker::new(node.to_string(), local.to_path_buf(), share.to_path_buf())
            .with_now_fn(now)
    }

    /// A fetcher that always hands back a fixed body (a "fresh upstream").
    struct StaticFetcher(String);
    impl ListFetcher for StaticFetcher {
        fn fetch(&self, _name: &str, _url: &str) -> RefreshOutcome {
            RefreshOutcome::Fetched(self.0.clone())
        }
    }

    /// A fetcher that is always unavailable (airgapped, no mirror).
    struct DeadFetcher;
    impl ListFetcher for DeadFetcher {
        fn fetch(&self, _name: &str, _url: &str) -> RefreshOutcome {
            RefreshOutcome::Unavailable
        }
    }

    // ── the guest policy envelope round-trips without a host matcher ──

    #[test]
    fn policy_blob_round_trips_opaque_filter_text() {
        let mut store = FilterPolicyStore::default();
        store.add_source(FilterPolicySource {
            name: "operator-list".into(),
            url: Some("mirror://operator-list".into()),
            raw: "||tracker.example^\n##.advert\n".into(),
            enabled: true,
            updated_ms: 7,
        });
        let json = store.to_json().expect("serialize");
        let back = FilterPolicyStore::from_json(&json).expect("deserialize");
        assert_eq!(store, back, "the blob round-trips byte-for-byte");
        assert_eq!(back.rule_counts(), (1, 1));
    }

    // ── leader refresh + the leader compile fold ──

    #[test]
    fn leader_refresh_updates_a_source_and_stamps_a_sync() {
        let (_c, now) = fake_clock(1_000);
        let local = tempfile::tempdir().unwrap();
        let share = tempfile::tempdir().unwrap();
        // The tmpdir share is always writable + this lone node wins leadership.
        let fresh = "||fresh-tracker.example^\n##.fresh-ad\n";
        std::fs::create_dir_all(mirror_dir(share.path())).unwrap();
        std::fs::write(mirror_dir(share.path()).join("operator.txt"), fresh).unwrap();
        let mut w = worker("solo", local.path(), share.path(), now)
            .with_fetcher(Arc::new(StaticFetcher(fresh.to_string())));
        w.load();
        assert!(
            w.own.synced_ms().is_none(),
            "no sync before the first refresh"
        );
        w.sync();
        // The leader refreshed every enabled source from the fetcher + stamped it.
        assert!(
            w.own.synced_ms().is_some(),
            "a successful refresh stamps a sync"
        );
        // The compiled blob landed in the share for the browser to read.
        let blob = compiled_path(share.path());
        assert!(blob.exists(), "the leader compiled the shared engine blob");
        let compiled = load_store(&blob).expect("compiled blob parses");
        assert_eq!(compiled.rule_counts(), (1, 1));
    }

    #[test]
    fn two_nodes_converge_their_allowlist_after_replay_merge() {
        let (_c, now) = fake_clock(2_000);
        let share = tempfile::tempdir().unwrap();
        let la = tempfile::tempdir().unwrap();
        let lb = tempfile::tempdir().unwrap();
        // Two nodes over one shared Syncthing folder; deny both the fetcher so the
        // test exercises the merge, not refresh.
        let mut a =
            worker("A", la.path(), share.path(), now.clone()).with_fetcher(Arc::new(DeadFetcher));
        let mut b = worker("B", lb.path(), share.path(), now).with_fetcher(Arc::new(DeadFetcher));
        a.load();
        b.load();
        // A allowlists a site; B blocks a different one.
        a.apply_action(AdfilterAction::Allow {
            domain: "news.example.com".into(),
        });
        b.apply_action(AdfilterAction::Block {
            domain: "tracker.example.com".into(),
        });
        // Converge (idempotent — a couple of interleaved passes settle it).
        a.sync();
        b.sync();
        a.sync();
        b.sync();
        // Both nodes' converged allowlist carries A's opt-out.
        assert!(a.converged.is_allowed("news.example.com"));
        assert!(b.converged.is_allowed("news.example.com"));
        assert_eq!(a.status().peers, 1, "A merged B's store");
        assert_eq!(b.status().peers, 1, "B merged A's store");
    }

    // ── airgap-honest staleness fallback ──

    #[test]
    fn unavailable_upstream_falls_back_with_honest_staleness() {
        let (_c, now) = fake_clock(5_000);
        let local = tempfile::tempdir().unwrap();
        let share = tempfile::tempdir().unwrap();
        let mut w =
            worker("solo", local.path(), share.path(), now).with_fetcher(Arc::new(DeadFetcher));
        w.load();
        w.sync();
        // Never synced upstream → no fabricated host rules and honest status.
        let status = w.status();
        assert_eq!(status.staleness, Staleness::NeverSynced);
        assert_eq!(status.age_ms, None);
        assert_eq!(status.enabled_sources, 0);
        assert_eq!(status.network_rules, 0);
    }

    #[test]
    fn a_stale_sync_is_reported_honestly() {
        let (clock, now) = fake_clock(1_000);
        let local = tempfile::tempdir().unwrap();
        let share = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(mirror_dir(share.path())).unwrap();
        std::fs::write(
            mirror_dir(share.path()).join("operator.txt"),
            "||x.example^\n",
        )
        .unwrap();
        let mut w = worker("solo", local.path(), share.path(), now)
            .with_fetcher(Arc::new(StaticFetcher("||x.example^\n".to_string())));
        w.load();
        w.sync(); // synced at t=1000
        assert!(matches!(w.status().staleness, Staleness::Fresh));
        // Jump past the freshness window → honest Stale with a real age.
        clock.store(1_000 + DEFAULT_FRESHNESS_MS + 5, Ordering::SeqCst);
        let st = w.status().staleness;
        assert!(
            matches!(st, Staleness::Stale { .. }),
            "expected Stale, got {st:?}"
        );
        if let Staleness::Stale { age_ms } = st {
            assert!(age_ms >= DEFAULT_FRESHNESS_MS);
        }
    }

    // ── offline-first: a down share never fakes a converge ──

    #[test]
    fn offline_share_is_never_written_and_stays_local() {
        let (_c, now) = fake_clock(1_000);
        let local = tempfile::tempdir().unwrap();
        let share = tempfile::tempdir().unwrap();
        let gate = Arc::new(AtomicBool::new(false)); // share DOWN
        let mut w = worker("solo", local.path(), share.path(), now.clone())
            .with_fetcher(Arc::new(DeadFetcher))
            .with_share_gate(gate.clone());
        w.load();
        w.apply_action(AdfilterAction::Allow {
            domain: "news.example.com".into(),
        });
        w.sync();
        assert!(!w.status().share_reachable);
        // The edit is durable node-local...
        assert!(store_path(local.path(), "solo").exists());
        // ...but nothing was mirrored into the down share.
        assert!(!store_path(share.path(), "solo").exists());
        assert!(!compiled_path(share.path()).exists());

        // Restart replays the local store (the opt-out survives).
        let mut w2 = worker("solo", local.path(), share.path(), now)
            .with_fetcher(Arc::new(DeadFetcher))
            .with_share_gate(gate.clone());
        w2.load();
        assert!(w2.converged.is_allowed("news.example.com"));

        // Share reappears → the next sync mirrors the backlog out.
        gate.store(true, Ordering::SeqCst);
        w2.sync();
        assert!(store_path(share.path(), "solo").exists());
    }

    // ── the production mirror fetcher is real (no network, no fabrication) ──

    #[test]
    fn mirror_fetcher_reads_a_dropped_list_else_unavailable() {
        let dir = tempfile::tempdir().unwrap();
        let f = MirrorFetcher::new(dir.path().to_path_buf());
        // Absent mirror → honest Unavailable (the fallback trigger).
        assert!(matches!(
            f.fetch("EasyList", "https://easylist.to/x"),
            RefreshOutcome::Unavailable
        ));
        // A dropped mirror body → Fetched, keyed by the sanitized name.
        std::fs::write(dir.path().join("EasyList.txt"), "||dropped.example^\n").unwrap();
        let out = f.fetch("EasyList", "https://easylist.to/x");
        assert!(
            matches!(&out, RefreshOutcome::Fetched(_)),
            "mirror body should have been read"
        );
        if let RefreshOutcome::Fetched(text) = out {
            assert!(text.contains("dropped.example"));
        }
        // An empty file is not a fabricated list — still Unavailable.
        std::fs::write(dir.path().join("Empty.txt"), "  \n").unwrap();
        assert!(matches!(f.fetch("Empty", ""), RefreshOutcome::Unavailable));
    }

    // ── typed action parsing ──

    #[test]
    fn parse_action_covers_allow_block_and_rejects_bad_input() {
        assert_eq!(
            parse_action("allow", r#"{"domain":"News.Example.com"}"#).unwrap(),
            AdfilterAction::Allow {
                domain: "news.example.com".into()
            },
        );
        assert_eq!(
            parse_action("block", r#"{"domain":"x.com"}"#).unwrap(),
            AdfilterAction::Block {
                domain: "x.com".into()
            },
        );
        assert!(parse_action("frobnicate", r#"{"domain":"x"}"#).is_err());
        assert!(
            parse_action("allow", "{}").is_err(),
            "missing domain is a typed error"
        );
        assert!(
            parse_action("allow", r#"{"domain":"  "}"#).is_err(),
            "empty domain rejected"
        );
    }

    // ── privileged action boundary ──────────────────────────────────────────

    const AUTH_KEY: &[u8] = b"adfilter-action-auth-test-key";
    const AUTH_NOW: i64 = 1_700_000_000_000;

    fn signed_allow_body(node: &str, domain: &str, nonce: &str) -> String {
        signed_action_body("allow", node, domain, nonce)
    }

    fn signed_action_body(verb: &str, node: &str, domain: &str, nonce: &str) -> String {
        let unsigned = serde_json::json!({
            "domain": domain,
            "schema_version": 1,
        })
        .to_string();
        let target = domain.trim().to_ascii_lowercase();
        authorize_test_body(
            AUTH_KEY,
            &unsigned,
            MutationContext {
                verb: &format!("adfilter-{verb}"),
                node,
                target: &target,
            },
            nonce,
            AUTH_NOW + 30_000,
        )
    }

    #[test]
    fn service_bus_root_falls_back_to_the_shared_system_spool() {
        assert_eq!(
            adfilter_bus_root_or_system(None),
            PathBuf::from(mde_bus::SYSTEM_BUS_ROOT)
        );
        assert_eq!(
            adfilter_bus_root_or_system(Some(PathBuf::from("/tmp/adfilter-explicit-bus"))),
            PathBuf::from("/tmp/adfilter-explicit-bus")
        );
    }

    #[tokio::test]
    async fn late_bus_recovers_without_replay_and_defers_failed_reads() {
        let root = tempfile::tempdir().unwrap();
        let local = root.path().join("local");
        let share = root.path().join("share");
        let bus_root = root.path().join("bus");
        let persist = Persist::open(bus_root.clone()).unwrap();

        // Durable policy is independent of the transient Bus and must be loaded
        // while startup is waiting for that Bus to become usable.
        let mut durable = FilterPolicyStore::default();
        durable.allow_site("durable.example", "local-host", 500);
        std::fs::create_dir_all(node_dir(&local, "local-host")).unwrap();
        std::fs::write(store_path(&local, "local-host"), durable.to_json().unwrap()).unwrap();

        // This retained destructive command predates activation and must never
        // replay after the same worker eventually opens the Bus.
        let stale = signed_action_body("block", "local-host", "stale.example", "startup-stale");
        persist
            .write(
                &format!("{ACTION_PREFIX}block"),
                Priority::Default,
                None,
                Some(&stale),
            )
            .unwrap();

        let authorizer = Arc::new(ActionAuthorizer::for_test(
            AUTH_KEY,
            root.path().join("auth"),
            AUTH_NOW,
        ));
        let open_attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let open_attempts_for_worker = Arc::clone(&open_attempts);
        let bus_root_for_worker = bus_root.clone();
        let prime_attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let prime_attempts_for_worker = Arc::clone(&prime_attempts);
        let fail_reads = Arc::new(AtomicBool::new(false));
        let fail_reads_for_worker = Arc::clone(&fail_reads);
        let (_clock, now) = fake_clock(1_000);
        let mut worker = worker("local-host", &local, &share, now)
            .with_authorizer(authorizer)
            .with_fetcher(Arc::new(DeadFetcher))
            .with_bus_root(bus_root.clone())
            .with_tick(Duration::from_millis(5))
            .with_bus_opener(Arc::new(move |_| {
                match open_attempts_for_worker.fetch_add(1, Ordering::SeqCst) {
                    0 => Ok(None),
                    1 => Err("injected unopenable Bus".into()),
                    _ => Persist::open(bus_root_for_worker.clone())
                        .map(Some)
                        .map_err(|error| error.to_string()),
                }
            }))
            .with_cursor_primer(Arc::new(move |persist| {
                if prime_attempts_for_worker.fetch_add(1, Ordering::SeqCst) == 0 {
                    return Err("injected action-tail read failure".into());
                }
                prime_action_cursors(persist)
            }))
            .with_request_read_gate(Arc::new(move |_, _| {
                if fail_reads_for_worker.load(Ordering::SeqCst) {
                    Err("injected runtime Bus read failure".into())
                } else {
                    Ok(())
                }
            }));

        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let task =
            tokio::spawn(
                async move { worker.run(ShutdownToken::from_receiver(shutdown_rx)).await },
            );
        tokio::time::timeout(Duration::from_secs(3), async {
            while prime_attempts.load(Ordering::SeqCst) < 2 {
                assert!(!task.is_finished(), "worker exited during Bus recovery");
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("same worker must recover and activate");
        assert!(open_attempts.load(Ordering::SeqCst) >= 4);

        tokio::time::sleep(Duration::from_millis(25)).await;
        let restored = load_store(&store_path(&local, "local-host")).unwrap();
        assert!(restored.is_allowed("durable.example"));
        assert!(
            !restored.allowlist.contains_key("stale.example"),
            "retained startup mutation must be tail-primed, not replayed"
        );

        // `allow` appears only after activation. Its first message is forward
        // work, but a failed Bus read must defer both that effect and the tick's
        // convergence/status publish until a complete sweep succeeds.
        fail_reads.store(true, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(20)).await;
        let state_topic = format!("{STATE_PREFIX}local-host");
        let states_before = persist.list_since(&state_topic, None).unwrap().len();
        let forward = signed_allow_body("local-host", "forward.example", "forward-first");
        persist
            .write(
                &format!("{ACTION_PREFIX}allow"),
                Priority::Default,
                None,
                Some(&forward),
            )
            .unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(
            persist.list_since(&state_topic, None).unwrap().len(),
            states_before,
            "failed reads must defer convergence and publishing"
        );
        assert!(
            !load_store(&store_path(&local, "local-host"))
                .unwrap()
                .is_allowed("forward.example"),
            "failed read must not masquerade as an empty successful sweep"
        );

        fail_reads.store(false, Ordering::SeqCst);
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if load_store(&store_path(&local, "local-host"))
                    .is_some_and(|store| store.is_allowed("forward.example"))
                {
                    break;
                }
                assert!(!task.is_finished(), "worker exited before forward mutation");
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("new post-activation topic's first message must execute");

        shutdown_tx.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .expect("worker shutdown timed out")
            .expect("worker task panicked")
            .expect("worker returned an error");
    }

    #[test]
    fn adfilter_mutations_fail_closed_and_authorized_body_applies_once() {
        let local = tempfile::tempdir().unwrap();
        let share = tempfile::tempdir().unwrap();
        let bus = tempfile::tempdir().unwrap();
        let persist = Persist::open(bus.path().to_path_buf()).unwrap();
        let authorizer = Arc::new(ActionAuthorizer::for_test(
            AUTH_KEY,
            bus.path().join("auth"),
            AUTH_NOW,
        ));
        let (_clock, now) = fake_clock(1_000);
        let mut worker =
            worker("local-host", local.path(), share.path(), now).with_authorizer(authorizer);
        let topic = format!("{ACTION_PREFIX}allow");
        let write = |body: &str| {
            persist
                .write(&topic, Priority::Default, None, Some(body))
                .unwrap();
        };

        let unsigned = serde_json::json!({
            "domain": "unsigned.example",
            "schema_version": 1,
        })
        .to_string();
        write(&unsigned);
        worker.drain_requests(&persist);

        let valid = signed_allow_body("local-host", "authorized.example", "once");
        let tampered = valid.replace("authorized.example", "tampered.example");
        write(&tampered);
        worker.drain_requests(&persist);

        let future_unsigned = serde_json::json!({
            "domain": "future.example",
            "schema_version": 2,
        })
        .to_string();
        let future = authorize_test_body(
            AUTH_KEY,
            &future_unsigned,
            MutationContext {
                verb: "adfilter-allow",
                node: "local-host",
                target: "future.example",
            },
            "future-schema",
            AUTH_NOW + 30_000,
        );
        write(&future);
        worker.drain_requests(&persist);

        let oversized = serde_json::json!({
            "domain": "oversized.example",
            "padding": "x".repeat(crate::ipc::MAX_RPC_BODY_BYTES),
            "schema_version": 1,
        })
        .to_string();
        write(&oversized);
        worker.drain_requests(&persist);

        for domain in [
            "unsigned.example",
            "tampered.example",
            "future.example",
            "oversized.example",
        ] {
            assert!(
                !worker.own.is_allowed(domain),
                "unauthorized request changed {domain}"
            );
        }
        assert!(
            !store_path(local.path(), "local-host").exists(),
            "unauthorized requests must not persist the local store"
        );
        assert!(
            !store_path(share.path(), "local-host").exists(),
            "unauthorized requests must not mirror the shared store"
        );

        write(&valid);
        worker.drain_requests(&persist);
        assert!(worker.own.is_allowed("authorized.example"));
        let persisted = std::fs::read_to_string(store_path(local.path(), "local-host"))
            .expect("authorized request persists the local store");
        assert!(store_path(share.path(), "local-host").exists());

        // The exact same capability is replayed under a new Bus message. The
        // authorizer consumes its nonce on the first application, so this pass
        // must leave both the model and persisted bytes unchanged.
        write(&valid);
        worker.drain_requests(&persist);
        assert_eq!(
            std::fs::read_to_string(store_path(local.path(), "local-host")).unwrap(),
            persisted,
            "replayed capability must not apply or persist a second edit"
        );
        assert_eq!(worker.own.allowed_count(), 1);
    }

    // ── the published status shape ──

    #[test]
    fn status_shape_serializes_the_documented_fields() {
        let (_c, now) = fake_clock(1_000);
        let local = tempfile::tempdir().unwrap();
        let share = tempfile::tempdir().unwrap();
        let mut w = worker("peer:eagle", local.path(), share.path(), now)
            .with_fetcher(Arc::new(DeadFetcher));
        w.load();
        w.sync();
        let status = w.status();
        let json = serde_json::to_string(&status).expect("serialize status");
        let back: AdfilterStatus = serde_json::from_str(&json).expect("round-trip status");
        assert_eq!(back, status);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["node"], "peer:eagle");
        assert_eq!(v["enabled_sources"], 0);
        assert!(v.get("total_sources").is_some());
        assert!(v.get("network_rules").is_some());
        assert!(v.get("staleness").is_some());
        assert!(v.get("last_flush_ms").is_some());
    }

    #[test]
    fn worker_name_is_locked() {
        let local = tempfile::tempdir().unwrap();
        let share = tempfile::tempdir().unwrap();
        let (_c, now) = fake_clock(0);
        let w = worker("n1", local.path(), share.path(), now);
        assert_eq!(w.name(), "adfilter");
    }
}
