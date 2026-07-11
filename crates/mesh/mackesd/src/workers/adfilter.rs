//! BOOKMARKS-7 — the mackesd **adfilter worker** (the mesh-wide ad-blocker's
//! Syncthing replication + leader compile).
//!
//! Builds on the landed pure [`mde_adblock`] crate — the [`FilterListStore`]
//! (bundled seed + operator custom sources + per-site allowlist + [`Staleness`])
//! and the compiled [`Engine`] — and adds the mesh-side plumbing the pure crate
//! deliberately omits: persistence, the Syncthing-replicated store blob, the
//! leader compile, upstream refresh, and the Bus surface.
//!
//! ## What this worker owns
//!
//! * **Per-node store replication** (the same substrate the [`super::bookmarks`]
//!   worker uses). Every node writes ONLY its own
//!   `<share>/adfilter/<node>/store.json` (single-writer → Syncthing never sees a
//!   write conflict) and *reads* every peer's store, folding them through the
//!   store's last-writer-wins [`FilterListStore::merge`] into one converged store.
//! * **Leader compile** (lock: one compiler mesh-wide). The elected leader
//!   ([`crate::leader`], the shared `.mackesd-leader.lock`) serializes the
//!   converged store into the compiled engine blob at
//!   `<share>/adfilter/compiled/engine.json` — the single blob the mde-web-preview
//!   browser reads + compiles into its [`Engine`] — and refreshes the enabled
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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mde_adblock::{Engine, FilterListStore, Staleness};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;

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

/// Default freshness window: a sync older than this reads as [`Staleness::Stale`]
/// (7 days — EasyList's own refresh cadence).
pub const DEFAULT_FRESHNESS_MS: u64 = 7 * 24 * 60 * 60 * 1000;

/// A wall-clock source (ms since the Unix epoch). Injected so the model stays pure
/// and tests drive a deterministic fake clock.
type NowFn = Arc<dyn Fn() -> u64 + Send + Sync>;

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
fn load_store(path: &Path) -> Option<FilterListStore> {
    let text = std::fs::read_to_string(path).ok()?;
    FilterListStore::from_json(&text).ok()
}

// ── the worker ───────────────────────────────────────────────────────────────

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
    own: FilterListStore,
    /// The converged store (own ⊕ every peer) — published + compiled.
    converged: FilterListStore,
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
    /// Per-topic action cursors (`action/adfilter/<verb>` → last ULID).
    cursors: HashMap<String, String>,
    /// Injected wall clock.
    now_fn: NowFn,
    /// Test seam forcing the share up/down; `None` → the real writable guard.
    share_gate: Option<Arc<AtomicBool>>,
    /// Bus spool root override (tests point this at a tempdir).
    bus_root_override: Option<PathBuf>,
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
            own: FilterListStore::with_bundled(),
            converged: FilterListStore::with_bundled(),
            freshness_ms: DEFAULT_FRESHNESS_MS,
            peer_count: 0,
            last_flush_ms: 0,
            tick: DEFAULT_TICK,
            cursors: HashMap::new(),
            now_fn: Arc::new(default_now),
            share_gate: None,
            bus_root_override: None,
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

    /// Inject a custom upstream-refresh fetcher (tests).
    #[must_use]
    pub fn with_fetcher(mut self, fetcher: Arc<dyn ListFetcher>) -> Self {
        self.fetcher = fetcher;
        self
    }

    fn now_ms(&self) -> u64 {
        (self.now_fn)()
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
    /// proof), else seed the bundled lists, then rebuild the converged view.
    fn load(&mut self) {
        self.own = load_store(&store_path(&self.local_root, &self.node))
            .unwrap_or_else(FilterListStore::with_bundled);
        self.rebuild_converged();
    }

    /// The compiled engine for the converged store (the same [`Engine`] the browser
    /// builds from the replicated blob) — for the published rule counts.
    fn engine(&self) -> Engine {
        Engine::from_store(&self.converged)
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
    /// an [`RefreshOutcome::Unavailable`] leaves the last-synced / bundled copy and
    /// never stamps a sync. Returns whether any list changed.
    fn refresh_lists(&mut self) -> bool {
        let now = self.now_ms();
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

    /// LEADER-ONLY: compile the converged store into the shared engine blob at
    /// `<share>/adfilter/compiled/engine.json` — the single blob the browser reads.
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
        let engine = self.engine();
        AdfilterStatus {
            node: self.node.clone(),
            enabled_sources: self.converged.enabled_sources().count(),
            total_sources: self.converged.sources().len(),
            network_rules: engine.network_rule_count(),
            cosmetic_rules: engine.cosmetic_rule_count(),
            allowlisted_sites: self.converged.allowlist().domains().count(),
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
    fn drain_requests(&mut self, persist: &Persist) {
        let topics = match persist.list_topics() {
            Ok(t) => t,
            Err(e) => {
                tracing::debug!(target: "mackesd::adfilter", error = %e, "list_topics failed");
                return;
            }
        };
        let mut changed = false;
        for topic in topics
            .into_iter()
            .filter(|t| t.starts_with(ACTION_PREFIX) && t.len() > ACTION_PREFIX.len())
        {
            let verb = topic[ACTION_PREFIX.len()..].to_string();
            let cursor = self.cursors.get(&topic).cloned();
            let msgs = match persist.list_since(&topic, cursor.as_deref()) {
                Ok(m) => m,
                Err(e) => {
                    tracing::debug!(target: "mackesd::adfilter", topic, error = %e, "list_since failed");
                    continue;
                }
            };
            for msg in msgs {
                self.cursors.insert(topic.clone(), msg.ulid.clone());
                match parse_action(&verb, msg.body.as_deref().unwrap_or_default()) {
                    Ok(action) => {
                        self.apply_action(action);
                        changed = true;
                    }
                    Err(e) => {
                        tracing::warn!(target: "mackesd::adfilter", verb = %verb, error = %e, "bad request");
                    }
                }
            }
        }
        if changed {
            // Persist + mirror the allowlist edit right away, then republish.
            self.persist_own_local();
            let _ = self.mirror_to_share();
            self.rebuild_converged();
            self.publish_state(persist);
        }
    }

    /// Seed each action topic's cursor at its tail so a restart doesn't replay +
    /// re-apply already-processed requests (the edits are already in the store).
    fn seed_cursors(&mut self, persist: &Persist) {
        if let Ok(topics) = persist.list_topics() {
            for topic in topics
                .into_iter()
                .filter(|t| t.starts_with(ACTION_PREFIX) && t.len() > ACTION_PREFIX.len())
            {
                if let Ok(Some(ulid)) = persist.latest_ulid(&topic) {
                    self.cursors.insert(topic, ulid);
                }
            }
        }
    }
}

#[async_trait::async_trait]
impl Worker for AdfilterWorker {
    fn name(&self) -> &'static str {
        "adfilter"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let Some(bus_root) = self
            .bus_root_override
            .clone()
            .or_else(mde_bus::default_data_dir)
        else {
            tracing::debug!(target: "mackesd::adfilter", "no bus root; worker idle");
            return Ok(());
        };
        let persist = match Persist::open(bus_root) {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!(target: "mackesd::adfilter", error = %e, "persist open failed; worker idle");
                return Ok(());
            }
        };
        self.load();
        self.seed_cursors(&persist);
        self.flush(&persist); // publish the initial converged state
        let mut tick = tokio::time::interval(self.tick);
        tick.tick().await; // burn the immediate first tick
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    self.drain_requests(&persist);
                    self.flush(&persist);
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

    // ── the crate's serde blob compiles + round-trips into a matching engine ──

    #[test]
    fn compiled_blob_round_trips_and_blocks_a_tracker() {
        let store = FilterListStore::with_bundled();
        let json = store.to_json().expect("serialize");
        let back = FilterListStore::from_json(&json).expect("deserialize");
        assert_eq!(store, back, "the blob round-trips byte-for-byte");
        let engine = Engine::from_store(&back);
        // A bundled tracker rule matches through the recompiled engine.
        assert!(engine
            .match_request(
                "https://doubleclick.net/ad",
                mde_adblock::ResourceType::Script,
                "news.example.com",
            )
            .is_block());
        assert!(engine.network_rule_count() > 0);
    }

    // ── leader refresh + the leader compile fold ──

    #[test]
    fn leader_refresh_updates_a_source_and_stamps_a_sync() {
        let (_c, now) = fake_clock(1_000);
        let local = tempfile::tempdir().unwrap();
        let share = tempfile::tempdir().unwrap();
        // The tmpdir share is always writable + this lone node wins leadership.
        let fresh = "||fresh-tracker.example^\n##.fresh-ad\n";
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
        let engine = Engine::from_store(&compiled);
        assert!(engine
            .match_request(
                "https://fresh-tracker.example/x",
                mde_adblock::ResourceType::Script,
                "site.example",
            )
            .is_block());
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
        assert!(a.converged.allowlist().is_allowed("news.example.com"));
        assert!(b.converged.allowlist().is_allowed("news.example.com"));
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
        // Never synced upstream → the bundled seed with an honest NeverSynced.
        let status = w.status();
        assert_eq!(status.staleness, Staleness::NeverSynced);
        assert_eq!(status.age_ms, None);
        assert!(
            status.enabled_sources >= 3,
            "the bundled seed is still active"
        );
        assert!(status.network_rules > 0, "the bundled rules still compile");
    }

    #[test]
    fn a_stale_sync_is_reported_honestly() {
        let (clock, now) = fake_clock(1_000);
        let local = tempfile::tempdir().unwrap();
        let share = tempfile::tempdir().unwrap();
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
        assert!(w2.converged.allowlist().is_allowed("news.example.com"));

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
        assert!(v["enabled_sources"].as_u64().unwrap() >= 3);
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
