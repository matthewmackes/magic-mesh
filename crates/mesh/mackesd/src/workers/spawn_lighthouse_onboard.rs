//! OW-7 (Bus half) — the `spawn_lighthouse_onboard` worker: `onboard
//! spawn-lighthouse` reachable over the Bus.
//!
//! The CLI verb (`mackesd onboard spawn-lighthouse`) and the shell's Spawn
//! Lighthouse flow must drive ONE engine (§6 glue): this worker makes the
//! existing [`crate::onboard::spawn_lighthouse`] core Bus-reachable, so the egui
//! shell — which deliberately never links the daemon crate — requests a
//! lighthouse spawn by publishing a typed [`SpawnLighthouseAction`] on
//! [`ACTION_TOPIC`] and renders the typed [`SpawnLighthouseEvent`] answer off
//! [`EVENT_TOPIC`].
//!
//! ## Shape (mirrors [`super::service_onboard`] exactly)
//!
//! - The **pure core** is [`resolve`]: one drained action + the gathered
//!   [`SpawnFacts`] + the injectable [`Provisioner`] seam → the one result event.
//!   It REUSES the onboard engine verbatim — [`plan_spawn`] for the plan and
//!   [`execute`] over the seam for a real run — reimplementing none of the
//!   planning logic (§6).
//! - **Dry-run** resolves the plan only (the CA-migration steps + the honest
//!   LAN-only outcome) and never touches the seam — the preview the shell renders.
//! - **Apply** drives [`execute`] over the production [`LiveProvisioner`], whose
//!   typed [`ProvisionError::IntegrationGated`] is the honest live answer today
//!   (§7 — a real typed error on the wire, never a fake success). The live
//!   cloud/SSH provision + CA-migrate leg is gated on the operator's cloud token
//!   + prod-SSH, which stays behind that seam.
//! - **Leader-gated** like `service_onboard`: the action log is mesh-replicated,
//!   so only the elected node resolves + publishes — an N-node mesh answers each
//!   request once. The facts come off the founding bundle
//!   ([`crate::onboard::spawn_lighthouse::gather`]) so any leader plans identically.
//! - The cursor **primes past the backlog** on start (like `service_onboard`): a
//!   spawn is a one-shot verb, not a fold — a restart must not re-drive historical
//!   provisions.

#![cfg(feature = "async-services")]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::{Persist, StoredMessage};

use crate::ipc::action_auth::{ActionAuthorizer, MutationContext};
use crate::onboard::spawn_lighthouse::{
    execute, gather, plan_spawn, ProvisionError, Provisioner, SpawnOutcome, SpawnPlan,
    SpawnRequest, SpawnTarget,
};

use super::scheduler::Publisher;
use super::{ShutdownToken, Worker};

/// Bus action topic this worker drains — the `action/<domain>/<verb>` convention
/// applied to the onboard family's promote-to-lighthouse verb (sibling of
/// `action/onboard/service-add`).
pub const ACTION_TOPIC: &str = "action/onboard/spawn-lighthouse";

/// Bus event topic the typed result is published on — the matching
/// `event/<domain>/<verb>` lane the shell's Spawn Lighthouse flow tails.
pub const EVENT_TOPIC: &str = "event/onboard/spawn-lighthouse";

/// Closed capability verb for non-preview lighthouse provisioning actions.
/// Possession of the shared action topic is transport reachability, not IaC
/// authority.
pub const SPAWN_LIGHTHOUSE_AUTH_VERB: &str = "onboard-spawn-lighthouse";

/// Stable capability scope for the leader-coordinated lighthouse plane.
pub const SPAWN_LIGHTHOUSE_NODE_SCOPE: &str = "lighthouse-onboard";

/// Version of the authenticated action envelope accepted by the worker.
pub const ACTION_SCHEMA_VERSION: u64 = 1;

/// Poll cadence. The bus read is a cheap local log scan and a spawn is a slow,
/// operator-paced event, so the 2 s `service_onboard` cadence is responsive
/// without spinning.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(2);

const MIN_BUS_RETRY_INTERVAL: Duration = Duration::from_millis(10);
const MAX_BUS_RETRY_INTERVAL: Duration = Duration::from_secs(2);

// ───────────────────────────── wire contract ─────────────────────────────

/// Where the operator asked to provision. QC-15 retired the old local
/// cloud-hypervisor option, so the wire is cloud-only; the daemon maps the
/// discriminant to the shared `do-lighthouse-join` defaults
/// ([`SpawnTarget::default_cloud`]) — the single source of truth for region/size,
/// so a front-end can't name an off-policy shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpawnTargetKind {
    /// A cloud droplet (`DigitalOcean`, the `zone1-do` `IaC`).
    Cloud,
}

impl SpawnTargetKind {
    /// Map the wire discriminant onto the shared-default [`SpawnTarget`].
    #[must_use]
    pub fn to_target(self) -> SpawnTarget {
        match self {
            Self::Cloud => SpawnTarget::default_cloud(),
        }
    }
}

/// A spawn-lighthouse request drained off [`ACTION_TOPIC`] — the wire verb the
/// shell (or any front-end) publishes. Mirrors the CLI's `onboard
/// spawn-lighthouse` args: the [`SpawnTargetKind`], the `--pair` HA flag, and the
/// `--dry-run` flag, plus a caller-minted `id` the result event echoes for
/// correlation.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SpawnLighthouseAction {
    /// Caller-minted correlation id — echoed on the [`SpawnLighthouseEvent`].
    pub id: String,
    /// Where to provision.
    pub target: SpawnTargetKind,
    /// Provision two lighthouses for quorum/HA (`false` ⇒ a single lighthouse).
    #[serde(default)]
    pub pair: bool,
    /// `true` ⇒ resolve + publish the plan only; the [`Provisioner`] seam is
    /// never touched.
    #[serde(default)]
    pub dry_run: bool,
    /// Authenticated action-envelope schema. Defaults keep older read-only
    /// previews parseable; apply requests must carry the explicit field.
    #[serde(default = "default_action_schema_version")]
    pub schema_version: u64,
}

fn default_action_schema_version() -> u64 {
    ACTION_SCHEMA_VERSION
}

/// Parse a [`SpawnLighthouseAction`] body.
///
/// # Errors
/// A human-readable message on malformed JSON.
pub fn parse_action(body: &str) -> Result<SpawnLighthouseAction, String> {
    serde_json::from_str(body).map_err(|e| format!("malformed spawn-lighthouse action: {e}"))
}

/// The typed [`ProvisionError`] on the wire — the same two variants, tagged on
/// `type`, so the shell renders "integration-gated" and "failed" distinctly
/// (never collapsed into a fake success or an untyped string).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireProvisionError {
    /// The live path is honestly gated on a real prerequisite — retriable only
    /// once the named integration lands (cloud token / live SSH / the CA signer).
    IntegrationGated {
        /// Which seam step (`provision` / `push-enroll` / `migrate-ca`).
        step: String,
        /// What the live call needs before it can run.
        reason: String,
    },
    /// A step failed for a concrete runtime reason.
    Failed {
        /// Which seam step failed.
        step: String,
        /// The failure detail.
        reason: String,
    },
}

impl From<ProvisionError> for WireProvisionError {
    fn from(e: ProvisionError) -> Self {
        match e {
            ProvisionError::IntegrationGated { step, reason } => Self::IntegrationGated {
                step: step.to_string(),
                reason,
            },
            ProvisionError::Failed { step, reason } => Self::Failed {
                step: step.to_string(),
                reason,
            },
        }
    }
}

/// The typed result published on [`EVENT_TOPIC`]: the request echo (`id` /
/// `target` / `pair` / `dry_run`), the plan's one-line summary
/// ([`SpawnPlan::human`] for a dry-run, the outcome's for an apply), the ordered
/// CA-migration step descriptions, how many lighthouses the plan stands up,
/// whether an operator retry is available (the honest LAN-only outcome) with its
/// fix hint, and the typed error when the apply seam refused.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SpawnLighthouseEvent {
    /// The request's correlation id, echoed.
    pub id: String,
    /// The requested target, echoed.
    pub target: SpawnTargetKind,
    /// Whether a pair (two lighthouses) was requested, echoed.
    pub pair: bool,
    /// Whether this answers a dry-run (plan preview) or an apply.
    pub dry_run: bool,
    /// The one-line human summary (plan for dry-run, outcome/error for apply).
    pub summary: String,
    /// The ordered CA-migration step descriptions (empty for the LAN-only
    /// outcome — nothing would be provisioned).
    #[serde(default)]
    pub steps: Vec<String>,
    /// How many lighthouses this plan stands up (0 for LAN-only, 1, or 2 for a
    /// pair).
    #[serde(default)]
    pub lighthouse_count: usize,
    /// `true` for the honest retryable LAN-only outcome (no cloud token / not
    /// founded) — the mesh keeps running and the operator retries after clearing
    /// the blocker.
    #[serde(default)]
    pub retry_available: bool,
    /// What the operator must fix before a retry succeeds (LAN-only only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lan_only_hint: Option<String>,
    /// The typed seam error when an apply couldn't run (`None` for a dry-run and
    /// for a completed / LAN-only apply).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<WireProvisionError>,
}

// ───────────────────────────── pure: resolve ─────────────────────────────

/// The one-line summary for a completed [`execute`] outcome (the apply path's
/// `Ok`). Mirrors [`SpawnPlan::human`]'s LAN-only phrasing so a dry-run and an
/// apply that both land LAN-only read identically.
fn outcome_summary(outcome: &SpawnOutcome) -> String {
    match outcome {
        SpawnOutcome::Provisioned { endpoint } => {
            format!("lighthouse provisioned + enrolled at {}", endpoint.host)
        }
        SpawnOutcome::LanOnly { reason } => {
            format!(
                "stays LAN-only ({reason}) — retry once you {}",
                reason.hint()
            )
        }
    }
}

/// Pure orchestration: one drained [`SpawnLighthouseAction`] + the gathered
/// [`SpawnFacts`] + the injectable [`Provisioner`] seam → the one
/// [`SpawnLighthouseEvent`] to publish.
///
/// Reuses the onboard engine verbatim: [`plan_spawn`] resolves the plan (cloud
/// cloud rendered spec + enroll bootstrap + ordered CA-migration steps, or the
/// honest LAN-only outcome), and — apply only — [`execute`] drives the seam. A
/// dry-run never touches the seam; a LAN-only plan short-circuits inside
/// `execute` without seam calls either.
///
/// [`SpawnFacts`]: crate::onboard::spawn_lighthouse::SpawnFacts
#[must_use]
pub fn resolve(
    action: &SpawnLighthouseAction,
    facts: &crate::onboard::spawn_lighthouse::SpawnFacts,
    prov: &dyn Provisioner,
) -> SpawnLighthouseEvent {
    let request = SpawnRequest {
        target: action.target.to_target(),
        pair: action.pair,
    };
    let plan = plan_spawn(&request, facts);

    let (steps, lan_only_hint) = match &plan {
        SpawnPlan::Provision { ca_migration, .. } => (
            ca_migration
                .iter()
                .map(|s| s.describe().to_string())
                .collect(),
            None,
        ),
        SpawnPlan::LanOnly { reason } => (Vec::new(), Some(reason.hint().to_string())),
    };
    let lighthouse_count = plan.lighthouse_count();
    let retry_available = plan.retry_available();

    let (summary, error) = if action.dry_run {
        (plan.human(), None)
    } else {
        match execute(&plan, prov) {
            Ok(outcome) => (outcome_summary(&outcome), None),
            Err(e) => (e.to_string(), Some(WireProvisionError::from(e))),
        }
    };

    SpawnLighthouseEvent {
        id: action.id.clone(),
        target: action.target,
        pair: action.pair,
        dry_run: action.dry_run,
        summary,
        steps,
        lighthouse_count,
        retry_available,
        lan_only_hint,
        error,
    }
}

// ─────────────────────────── bus + worker ───────────────────────────

/// Read one complete forward command batch without changing publication state.
fn read_new_actions(
    persist: &mut Persist,
    cursor: Option<&str>,
) -> Result<Vec<StoredMessage>, String> {
    // The retired path fresh-opened on every tick and therefore always followed
    // an atomically replaced index.sqlite. Preserve that visibility while
    // retaining one long-lived handle.
    persist.reopen_if_index_changed();
    persist
        .list_since(ACTION_TOPIC, cursor)
        .map_err(|error| format!("read {ACTION_TOPIC}: {error}"))
}

/// Seed the cursor to the newest existing message so a (re)start doesn't
/// re-drive a historical spawn. `None` when the topic is empty.
fn prime_cursor(persist: &Persist) -> Result<Option<String>, String> {
    persist
        .latest_ulid(ACTION_TOPIC)
        .map_err(|error| format!("prime {ACTION_TOPIC}: {error}"))
}

fn spawn_lighthouse_bus_root(override_root: Option<PathBuf>) -> PathBuf {
    spawn_lighthouse_bus_root_or_system(override_root.or_else(mde_bus::default_data_dir))
}

fn spawn_lighthouse_bus_root_or_system(resolved: Option<PathBuf>) -> PathBuf {
    resolved.unwrap_or_else(|| PathBuf::from(mde_bus::SYSTEM_BUS_ROOT))
}

fn next_bus_retry_interval(current: Duration) -> Duration {
    current
        .saturating_mul(2)
        .clamp(MIN_BUS_RETRY_INTERVAL, MAX_BUS_RETRY_INTERVAL)
}

/// Bind the capability to the requested lighthouse identity and HA shape. The
/// exact body is also authenticated, so this scope is defense in depth against
/// a capability minted for another onboarding request.
#[must_use]
pub fn spawn_auth_target(action: &SpawnLighthouseAction) -> String {
    let target = match action.target {
        SpawnTargetKind::Cloud => "cloud",
    };
    let shape = if action.pair { "pair" } else { "single" };
    format!("lighthouse:{target}:{}:{shape}", action.id,)
}

/// Verify an exact apply body before the onboarding engine can reach its
/// provider, enrollment, CA, or secret-minting seams.
fn authorize_spawn_action(
    authorizer: &ActionAuthorizer,
    body: &str,
    action: &SpawnLighthouseAction,
) -> Result<(), String> {
    let target = spawn_auth_target(action);
    authorizer.authorize(
        body,
        MutationContext {
            verb: SPAWN_LIGHTHOUSE_AUTH_VERB,
            node: SPAWN_LIGHTHOUSE_NODE_SCOPE,
            target: &target,
        },
    )
}

#[cfg(test)]
type BusOpenFn = dyn Fn(&Path) -> Result<Option<Persist>, String> + Send + Sync;
#[cfg(test)]
type CursorPrimeFn = dyn Fn(&Persist) -> Result<Option<String>, String> + Send + Sync;
#[cfg(test)]
type ActionReadGateFn = dyn Fn() -> Result<(), String> + Send + Sync;
#[cfg(test)]
type EventPublishGateFn = dyn Fn() -> Result<(), String> + Send + Sync;

/// The Bus-reachable `onboard spawn-lighthouse` worker. Leader-gated.
pub struct SpawnLighthouseOnboardWorker {
    /// Shared-storage root — where [`gather`] reads the founding bundle
    /// (mesh-id + CA-holder overlay IP) and where the leader lock lives.
    workgroup_root: PathBuf,
    /// This node's id — its identity in the leader election AND the node whose
    /// founding bundle [`gather`] reads.
    node_id: String,
    /// The shared leader lock (the same `.mackesd-leader.lock` `session_broker` /
    /// `service_onboard` use).
    leader_lock: PathBuf,
    /// The injectable provision seam (production: [`LiveProvisioner`]).
    prov: Box<dyn Provisioner + Send + Sync>,
    /// Optional injected publisher. Production writes through the recovered Bus.
    publisher: Option<Box<dyn Publisher + Send + Sync>>,
    /// Exact-body capability verifier for the privileged apply lane. Missing
    /// production credentials fail closed.
    authorizer: Arc<ActionAuthorizer>,
    /// Deterministic live-facts seam for the authorization test; production
    /// always gathers from the workgroup and host environment.
    #[cfg(test)]
    facts_override: Option<crate::onboard::spawn_lighthouse::SpawnFacts>,
    /// Poll cadence.
    poll: Duration,
    /// Bus root override (tests). Otherwise user Bus then canonical system Bus.
    bus_root_override: Option<PathBuf>,
    /// A resolved event whose mutation completed but durable publication did not.
    /// It is retried before reading more commands, preventing effect replay.
    pending_publication: Option<(String, String)>,
    #[cfg(test)]
    bus_open_override: Option<Arc<BusOpenFn>>,
    #[cfg(test)]
    cursor_prime_override: Option<Arc<CursorPrimeFn>>,
    #[cfg(test)]
    action_read_gate: Option<Arc<ActionReadGateFn>>,
    #[cfg(test)]
    event_publish_gate: Option<Arc<EventPublishGateFn>>,
}

impl SpawnLighthouseOnboardWorker {
    /// Construct with production defaults: the honestly integration-gated
    /// [`LiveProvisioner`], the recovered Bus, the shared leader lock
    /// under `workgroup_root`, and the default cadence.
    #[must_use]
    pub fn new(workgroup_root: PathBuf, node_id: String) -> Self {
        let leader_lock = workgroup_root.join(".mackesd-leader.lock");
        Self {
            workgroup_root,
            node_id,
            leader_lock,
            prov: Box::new(crate::onboard::spawn_lighthouse::LiveProvisioner::default()),
            publisher: None,
            authorizer: Arc::new(ActionAuthorizer::production()),
            #[cfg(test)]
            facts_override: None,
            poll: DEFAULT_POLL_INTERVAL,
            bus_root_override: None,
            pending_publication: None,
            #[cfg(test)]
            bus_open_override: None,
            #[cfg(test)]
            cursor_prime_override: None,
            #[cfg(test)]
            action_read_gate: None,
            #[cfg(test)]
            event_publish_gate: None,
        }
    }

    /// Inject a provision seam (tests). Production uses [`LiveProvisioner`].
    #[must_use]
    pub fn with_provisioner(mut self, prov: Box<dyn Provisioner + Send + Sync>) -> Self {
        self.prov = prov;
        self
    }

    /// Inject a publisher (tests). Production uses the recovered Bus handle.
    #[must_use]
    pub fn with_publisher(mut self, publisher: Box<dyn Publisher + Send + Sync>) -> Self {
        self.publisher = Some(publisher);
        self
    }

    /// Inject an isolated verifier and replay ledger for focused action tests.
    /// Production always uses the root-only systemd-credential-backed verifier.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_authorizer(mut self, authorizer: Arc<ActionAuthorizer>) -> Self {
        self.authorizer = authorizer;
        self
    }

    /// Inject live facts so the authorization test can prove that exactly one
    /// authorized body reaches the provider seam without host credentials.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_facts(
        mut self,
        facts: crate::onboard::spawn_lighthouse::SpawnFacts,
    ) -> Self {
        self.facts_override = Some(facts);
        self
    }

    /// Override the poll cadence (tests, to avoid multi-second waits).
    #[must_use]
    pub const fn with_poll(mut self, poll: Duration) -> Self {
        self.poll = poll;
        self
    }

    /// Override the Bus root (tests).
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
    fn with_action_read_gate(mut self, gate: Arc<ActionReadGateFn>) -> Self {
        self.action_read_gate = Some(gate);
        self
    }

    #[cfg(test)]
    #[must_use]
    fn with_event_publish_gate(mut self, gate: Arc<EventPublishGateFn>) -> Self {
        self.event_publish_gate = Some(gate);
        self
    }

    fn bus_root(&self) -> PathBuf {
        spawn_lighthouse_bus_root(self.bus_root_override.clone())
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

    fn prime_action_cursor(&self, persist: &Persist) -> Result<Option<String>, String> {
        #[cfg(test)]
        if let Some(prime) = self.cursor_prime_override.as_ref() {
            return prime(persist);
        }
        prime_cursor(persist)
    }

    fn publish_event(&self, persist: &Persist, body: &str) -> bool {
        #[cfg(test)]
        if let Some(gate) = self.event_publish_gate.as_ref() {
            if let Err(error) = gate() {
                tracing::debug!(target: "mackesd::spawn_lighthouse_onboard", %error, "injected event publication failure");
                return false;
            }
        }
        if let Some(publisher) = self.publisher.as_ref() {
            publisher.publish(EVENT_TOPIC, body);
            true
        } else {
            persist
                .write(EVENT_TOPIC, Priority::Default, None, Some(body))
                .map(|_| true)
                .unwrap_or_else(|error| {
                    tracing::warn!(target: "mackesd::spawn_lighthouse_onboard", %error, "durable event publication failed");
                    false
                })
        }
    }

    /// Only the elected node answers (no-fixed-center: any eligible node can be
    /// it; the mesh-replicated request is answered once, not N times). Reuses
    /// the shared lock.
    fn is_leader(&self) -> bool {
        crate::leader_gate::LeaderGate::from_lock_path(
            self.leader_lock.clone(),
            self.node_id.clone(),
        )
        .is_leader()
    }

    fn live_facts(&self) -> crate::onboard::spawn_lighthouse::SpawnFacts {
        #[cfg(test)]
        if let Some(facts) = self.facts_override.clone() {
            return facts;
        }
        gather(&self.workgroup_root, &self.node_id)
    }

    /// Read one complete command batch, then mutate and durably publish in order.
    /// A read failure changes neither cursor nor effects. A publication failure
    /// retains the resolved event and retries it before any subsequent command.
    fn drain_and_publish(&mut self, persist: &mut Persist, cursor: &mut Option<String>) -> bool {
        if let Some((ulid, body)) = self.pending_publication.clone() {
            if !self.publish_event(persist, &body) {
                return false;
            }
            *cursor = Some(ulid);
            self.pending_publication = None;
        }

        #[cfg(test)]
        if let Some(gate) = self.action_read_gate.as_ref() {
            if let Err(error) = gate() {
                tracing::debug!(target: "mackesd::spawn_lighthouse_onboard", %error, "injected action read failure");
                return false;
            }
        }
        let messages = match read_new_actions(persist, cursor.as_deref()) {
            Ok(messages) => messages,
            Err(error) => {
                tracing::debug!(target: "mackesd::spawn_lighthouse_onboard", %error, "action read failed; mutation sweep deferred");
                return false;
            }
        };
        if messages.is_empty() {
            return true;
        }
        if !self.is_leader() {
            *cursor = messages.last().map(|message| message.ulid.clone());
            return true;
        }

        let mut facts = None;
        for message in messages {
            let body = message.body.as_deref().unwrap_or_default();
            let action = match parse_action(body) {
                Ok(action) => action,
                Err(error) => {
                    tracing::warn!(ulid = %message.ulid, %error, "spawn_lighthouse_onboard: bad spawn-lighthouse action");
                    *cursor = Some(message.ulid);
                    continue;
                }
            };
            // Authenticate every apply before gathering reaches any provider or
            // secret-adjacent work. Dry-run is an unsigned read-only preview.
            if !action.dry_run {
                if let Err(error) = authorize_spawn_action(self.authorizer.as_ref(), body, &action)
                {
                    tracing::warn!(id = %action.id, %error, "spawn_lighthouse_onboard: unauthorized apply refused");
                    *cursor = Some(message.ulid);
                    continue;
                }
            }
            let facts = facts.get_or_insert_with(|| self.live_facts());
            let event = resolve(&action, facts, self.prov.as_ref());
            let event_body = match serde_json::to_string(&event) {
                Ok(body) => body,
                Err(error) => {
                    tracing::warn!(id = %event.id, %error, "spawn_lighthouse_onboard: event serialize failed");
                    return false;
                }
            };
            if !self.publish_event(persist, &event_body) {
                self.pending_publication = Some((message.ulid, event_body));
                return false;
            }
            *cursor = Some(message.ulid);
        }
        true
    }
}

#[async_trait::async_trait]
impl Worker for SpawnLighthouseOnboardWorker {
    fn name(&self) -> &'static str {
        "spawn_lighthouse_onboard"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let bus_root = self.bus_root();
        let mut retry_interval = MIN_BUS_RETRY_INTERVAL;
        let (mut persist, mut cursor) = loop {
            match self.open_bus(&bus_root) {
                Ok(Some(persist)) => match self.prime_action_cursor(&persist) {
                    Ok(cursor) => break (persist, cursor),
                    Err(error) => tracing::warn!(
                        target: "mackesd::spawn_lighthouse_onboard",
                        %error,
                        "action-tail activation failed; startup will retry"
                    ),
                },
                Ok(None) => tracing::debug!(
                    target: "mackesd::spawn_lighthouse_onboard",
                    "Bus root unavailable; startup will retry"
                ),
                Err(error) => tracing::warn!(
                    target: "mackesd::spawn_lighthouse_onboard",
                    %error,
                    "Persist open failed; startup will retry"
                ),
            }
            tokio::select! {
                () = shutdown.wait() => return Ok(()),
                () = tokio::time::sleep(retry_interval) => {}
            }
            retry_interval = next_bus_retry_interval(retry_interval);
        };
        let mut tick = tokio::time::interval(self.poll);
        tick.tick().await; // consume the immediate first tick
        loop {
            tokio::select! {
                _ = tick.tick() => { self.drain_and_publish(&mut persist, &mut cursor); }
                () = shutdown.wait() => break,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::action_auth::authorize_test_body;
    use crate::onboard::spawn_lighthouse::{
        CaMigrationStep, Endpoint, EnrollBootstrap, LiveProvisioner, ProvisionSpec,
    };
    use mde_bus::hooks::config::Priority;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    fn facts(cloud_token: bool, founded: bool) -> crate::onboard::spawn_lighthouse::SpawnFacts {
        crate::onboard::spawn_lighthouse::SpawnFacts {
            mesh_id: "home-deadbeef".to_string(),
            cloud_token_present: cloud_token,
            ca_holder_overlay_ip: founded.then(|| "10.42.0.1".to_string()),
        }
    }

    fn action(target: SpawnTargetKind, pair: bool, dry_run: bool) -> SpawnLighthouseAction {
        SpawnLighthouseAction {
            id: "lh-test".to_string(),
            target,
            pair,
            dry_run,
            schema_version: ACTION_SCHEMA_VERSION,
        }
    }

    /// Recording [`Provisioner`] fake — pins that dry-runs and LAN-only plans
    /// never touch the seam, and that applies drive it in order.
    #[derive(Default)]
    struct FakeProvisioner {
        calls: Arc<Mutex<Vec<&'static str>>>,
    }

    impl Provisioner for FakeProvisioner {
        fn provision(&self, _spec: &ProvisionSpec) -> Result<Endpoint, ProvisionError> {
            self.calls.lock().expect("calls mutex").push("provision");
            Ok(Endpoint {
                host: "203.0.113.7".to_string(),
                overlay_ip: None,
            })
        }
        fn push_enroll(
            &self,
            _endpoint: &Endpoint,
            _enroll: &EnrollBootstrap,
        ) -> Result<(), ProvisionError> {
            self.calls.lock().expect("calls mutex").push("push_enroll");
            Ok(())
        }
        fn migrate_ca(
            &self,
            _endpoint: &Endpoint,
            _steps: &[CaMigrationStep],
        ) -> Result<(), ProvisionError> {
            self.calls.lock().expect("calls mutex").push("migrate_ca");
            Ok(())
        }
    }

    // ── topics follow the action/event convention ──

    #[test]
    fn topics_follow_the_action_event_convention() {
        assert_eq!(ACTION_TOPIC, "action/onboard/spawn-lighthouse");
        assert!(ACTION_TOPIC.starts_with("action/"));
        assert_eq!(EVENT_TOPIC, "event/onboard/spawn-lighthouse");
        assert!(EVENT_TOPIC.starts_with("event/"));
        // The two lanes name the same verb — a reader can pair them by suffix.
        assert_eq!(
            ACTION_TOPIC.trim_start_matches("action/"),
            EVENT_TOPIC.trim_start_matches("event/")
        );
    }

    // ── wire contract ──

    #[test]
    fn the_action_wire_shape_is_pinned_and_round_trips() {
        // Pin the exact bytes the shell's mirror serialises (its own test pins
        // the identical string) so the two sides can't silently drift.
        let a = SpawnLighthouseAction {
            id: "lh-42-cloud".to_string(),
            target: SpawnTargetKind::Cloud,
            pair: false,
            dry_run: true,
            schema_version: ACTION_SCHEMA_VERSION,
        };
        let json = serde_json::to_string(&a).expect("serialize");
        assert_eq!(
            json,
            r#"{"id":"lh-42-cloud","target":"cloud","pair":false,"dry_run":true,"schema_version":1}"#
        );
        assert_eq!(parse_action(&json).expect("parse"), a);
        // A minimal body omitting pair/dry_run defaults both to false.
        let m: SpawnLighthouseAction =
            parse_action(r#"{"id":"lh-1","target":"cloud"}"#).expect("minimal parse");
        assert_eq!(m.target, SpawnTargetKind::Cloud);
        assert!(!m.pair);
        assert!(!m.dry_run);
        assert!(parse_action(r#"{"id":"lh-old","target":"local"}"#).is_err());
        assert!(parse_action("not json").is_err());
    }

    #[test]
    fn the_event_round_trips_including_the_typed_error() {
        let ev = SpawnLighthouseEvent {
            id: "lh-1".to_string(),
            target: SpawnTargetKind::Cloud,
            pair: false,
            dry_run: false,
            summary: "gated".to_string(),
            steps: vec!["mint the token".to_string()],
            lighthouse_count: 1,
            retry_available: false,
            lan_only_hint: None,
            error: Some(WireProvisionError::IntegrationGated {
                step: "provision".to_string(),
                reason: "needs a cloud token".to_string(),
            }),
        };
        let json = serde_json::to_string(&ev).expect("serialize");
        // The error stays TYPED on the wire — tagged, distinguishing gated from
        // failed (§7: the shell must render the distinction honestly).
        assert!(json.contains(r#""type":"integration_gated""#));
        let back: SpawnLighthouseEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, ev);
    }

    // ── request → plan-event round-trip, dry-run ──

    #[test]
    fn dry_run_cloud_with_token_returns_the_provision_plan() {
        let prov = FakeProvisioner::default();
        let ev = resolve(
            &action(SpawnTargetKind::Cloud, false, true),
            &facts(true, true),
            &prov,
        );
        assert_eq!(ev.id, "lh-test");
        assert_eq!(ev.target, SpawnTargetKind::Cloud);
        assert!(ev.dry_run);
        // The reused engine's 5 ordered CA-migration steps.
        assert_eq!(ev.steps.len(), 5);
        assert!(ev.steps[0].contains("lighthouse-scoped join token"));
        assert_eq!(ev.lighthouse_count, 1);
        assert!(!ev.retry_available);
        assert!(ev.lan_only_hint.is_none());
        assert!(ev.error.is_none());
        assert!(ev.summary.contains("spawn a lighthouse"));
        assert!(
            prov.calls.lock().expect("calls mutex").is_empty(),
            "a dry-run never touches the provision seam"
        );
    }

    #[test]
    fn dry_run_pair_provisions_two_lighthouses() {
        let prov = FakeProvisioner::default();
        let ev = resolve(
            &action(SpawnTargetKind::Cloud, true, true),
            &facts(true, true),
            &prov,
        );
        assert!(ev.pair);
        assert_eq!(ev.lighthouse_count, 2);
        assert!(ev.summary.contains("pair of lighthouses"));
    }

    #[test]
    fn dry_run_no_cloud_token_is_the_honest_lan_only_outcome() {
        // The headline no-cloud-token → LAN-only + retry branch (a real path).
        let prov = FakeProvisioner::default();
        let ev = resolve(
            &action(SpawnTargetKind::Cloud, false, true),
            &facts(false, true),
            &prov,
        );
        assert!(
            ev.retry_available,
            "the operator can retry once a token exists"
        );
        assert!(ev.steps.is_empty(), "nothing to provision ⇒ no steps");
        assert_eq!(ev.lighthouse_count, 0);
        assert!(ev.summary.contains("LAN-only"));
        let hint = ev.lan_only_hint.expect("a LAN-only outcome names the fix");
        assert!(hint.contains("cloud token"));
        assert!(ev.error.is_none(), "LAN-only is an outcome, not an error");
        assert!(prov.calls.lock().expect("calls mutex").is_empty());
    }

    // ── apply drives the seam / surfaces the typed error ──

    #[test]
    fn apply_drives_the_seam_in_order_and_reports_the_outcome() {
        let prov = FakeProvisioner::default();
        let ev = resolve(
            &action(SpawnTargetKind::Cloud, false, false),
            &facts(true, true),
            &prov,
        );
        assert!(!ev.dry_run);
        assert!(ev.summary.contains("provisioned"));
        assert!(ev.error.is_none());
        assert_eq!(
            *prov.calls.lock().expect("calls mutex"),
            vec!["provision", "push_enroll", "migrate_ca"],
            "apply drives the seam provision → push_enroll → migrate_ca"
        );
    }

    #[test]
    fn apply_lan_only_never_touches_the_seam() {
        let prov = FakeProvisioner::default();
        let ev = resolve(
            &action(SpawnTargetKind::Cloud, false, false),
            &facts(false, true),
            &prov,
        );
        assert!(ev.retry_available);
        assert!(ev.error.is_none());
        assert!(
            prov.calls.lock().expect("calls mutex").is_empty(),
            "a LAN-only apply short-circuits with no seam calls"
        );
    }

    #[test]
    fn apply_through_the_live_seam_publishes_the_typed_gated_error() {
        // The production seam is honestly integration-gated (§7) — the event
        // carries the typed error, never a fake success.
        let ev = resolve(
            &action(SpawnTargetKind::Cloud, false, false),
            &facts(true, true),
            &LiveProvisioner::default(),
        );
        let Some(WireProvisionError::IntegrationGated { step, reason }) = &ev.error else {
            panic!("expected the typed gated error, got {:?}", ev.error);
        };
        assert_eq!(step, "provision");
        assert!(reason.contains("cloud token"), "names the missing prereq");
        assert!(ev.summary.contains("integration-gated"));
    }

    // ── the worker: drain → resolve → publish ──

    /// A [`Publisher`] recorder (the `scheduler` test seam re-typed locally —
    /// the trait is shared, the recorder is per-module).
    #[derive(Clone, Default)]
    struct RecordingPublisher {
        sent: Arc<Mutex<Vec<(String, String)>>>,
    }

    impl Publisher for RecordingPublisher {
        fn publish(&self, topic: &str, body: &str) {
            self.sent
                .lock()
                .expect("recorder mutex")
                .push((topic.to_string(), body.to_string()));
        }
    }

    fn now_ms() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    }

    fn seed_bus(actions: &[SpawnLighthouseAction]) -> PathBuf {
        let bodies = actions
            .iter()
            .map(|action| serde_json::to_string(action).unwrap())
            .collect::<Vec<_>>();
        seed_bus_bodies(&bodies)
    }

    fn seed_bus_bodies(bodies: &[String]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mde-slo-{}-{}", now_ms(), bodies.len()));
        let persist = Persist::open(dir.clone()).expect("open bus");
        for body in bodies {
            persist
                .write(ACTION_TOPIC, Priority::Default, None, Some(body))
                .expect("write action");
        }
        dir
    }

    #[test]
    fn worker_refuses_unsigned_tampered_and_replayed_applies_before_provisioning() {
        const KEY: &[u8] = b"spawn-lighthouse-action-auth-key";
        const NOW: i64 = 1_700_000_000_000;
        let unsigned =
            r#"{"id":"lh-auth","target":"cloud","pair":false,"dry_run":false,"schema_version":1}"#;
        let action = parse_action(unsigned).expect("valid spawn action");
        let target = spawn_auth_target(&action);
        let armed = authorize_test_body(
            KEY,
            unsigned,
            MutationContext {
                verb: SPAWN_LIGHTHOUSE_AUTH_VERB,
                node: SPAWN_LIGHTHOUSE_NODE_SCOPE,
                target: &target,
            },
            "spawn-lighthouse-once",
            NOW + 30_000,
        );
        // This remains parseable, but its exact body and semantic target no
        // longer match the capability.
        let tampered = armed.replace("\"id\":\"lh-auth\"", "\"id\":\"lh-tampered\"");
        let bus = seed_bus_bodies(&[unsigned.to_string(), tampered, armed.clone(), armed]);
        let wg = std::env::temp_dir().join(format!("mde-slo-auth-wg-{}", now_ms()));
        std::fs::create_dir_all(&wg).expect("mk workgroup");
        let auth_root = tempfile::tempdir().expect("auth root");
        let authorizer = Arc::new(ActionAuthorizer::for_test(
            KEY,
            auth_root.path().to_path_buf(),
            NOW,
        ));
        let provisioner = FakeProvisioner::default();
        let calls = provisioner.calls.clone();
        let publisher = RecordingPublisher::default();
        let sent = publisher.sent.clone();
        let mut worker = SpawnLighthouseOnboardWorker::new(wg.clone(), "peer:auth".to_string())
            .with_provisioner(Box::new(provisioner))
            .with_publisher(Box::new(publisher))
            .with_authorizer(authorizer)
            .with_facts(facts(true, true))
            .with_bus_root(bus.clone());

        let mut persist = Persist::open(bus.clone()).expect("reopen bus");
        let mut cursor = None;
        assert!(worker.drain_and_publish(&mut persist, &mut cursor));
        let calls = calls.lock().unwrap().clone();

        assert_eq!(
            calls.as_slice(),
            ["provision", "push_enroll", "migrate_ca"],
            "only the one authorized body reaches the provider seam; unsigned, tampered, and replayed bodies have no effect"
        );
        assert_eq!(sent.lock().expect("sent mutex").len(), 1);
        let _ = std::fs::remove_dir_all(&bus);
        let _ = std::fs::remove_dir_all(&wg);
    }

    #[test]
    fn worker_drains_the_request_and_publishes_the_matching_event() {
        // A dry-run cloud request drained off a real temp bus and answered on
        // EVENT_TOPIC with the echoed id (a fresh temp workgroup ⇒ this node wins
        // the leader lock; an un-founded workgroup ⇒ the honest LAN-only plan).
        let bus = seed_bus(&[SpawnLighthouseAction {
            id: "lh-77-cloud".to_string(),
            target: SpawnTargetKind::Cloud,
            pair: false,
            dry_run: true,
            schema_version: ACTION_SCHEMA_VERSION,
        }]);
        let wg = std::env::temp_dir().join(format!("mde-slo-wg-{}", now_ms()));
        std::fs::create_dir_all(&wg).expect("mk workgroup");
        let rec = RecordingPublisher::default();
        let log = rec.sent.clone();
        let mut w = SpawnLighthouseOnboardWorker::new(wg.clone(), "peer:a".to_string())
            .with_publisher(Box::new(rec))
            .with_bus_root(bus.clone());

        let mut persist = Persist::open(bus.clone()).expect("reopen bus");
        let mut cursor = None;
        assert!(w.drain_and_publish(&mut persist, &mut cursor));

        let sent = log.lock().expect("recorder mutex");
        assert_eq!(sent.len(), 1, "one request ⇒ one event");
        assert_eq!(sent[0].0, EVENT_TOPIC);
        let ev: SpawnLighthouseEvent = serde_json::from_str(&sent[0].1).expect("event parses");
        assert_eq!(ev.id, "lh-77-cloud", "the correlation id is echoed");
        assert_eq!(ev.target, SpawnTargetKind::Cloud);
        assert!(ev.dry_run);
        // The temp workgroup has no founding bundle ⇒ the honest NotFounded
        // LAN-only outcome (retryable, no error).
        assert!(ev.retry_available);
        assert!(ev.error.is_none());
        drop(sent);

        // The cursor advanced — a second drain re-answers nothing.
        assert!(w.drain_and_publish(&mut persist, &mut cursor));
        assert_eq!(log.lock().expect("recorder mutex").len(), 1);

        let _ = std::fs::remove_dir_all(&bus);
        let _ = std::fs::remove_dir_all(&wg);
    }

    #[test]
    fn service_bus_root_honors_override_and_falls_back_to_system_spool() {
        assert_eq!(
            spawn_lighthouse_bus_root_or_system(None),
            PathBuf::from(mde_bus::SYSTEM_BUS_ROOT)
        );
        assert_eq!(
            spawn_lighthouse_bus_root_or_system(Some(PathBuf::from(
                "/tmp/spawn-lighthouse-explicit-bus",
            ))),
            PathBuf::from("/tmp/spawn-lighthouse-explicit-bus")
        );
    }

    #[tokio::test]
    async fn late_bus_recovers_without_replay_and_defers_reads_and_publication() {
        const KEY: &[u8] = b"spawn-lighthouse-late-bus-key";
        const AUTH_NOW: i64 = 1_700_000_000_000;

        fn signed_apply_body(nonce: &str) -> String {
            let unsigned = serde_json::to_string(&SpawnLighthouseAction {
                id: "lh-late-bus".into(),
                target: SpawnTargetKind::Cloud,
                pair: false,
                dry_run: false,
                schema_version: ACTION_SCHEMA_VERSION,
            })
            .unwrap();
            let action = parse_action(&unsigned).unwrap();
            authorize_test_body(
                KEY,
                &unsigned,
                MutationContext {
                    verb: SPAWN_LIGHTHOUSE_AUTH_VERB,
                    node: SPAWN_LIGHTHOUSE_NODE_SCOPE,
                    target: &spawn_auth_target(&action),
                },
                nonce,
                AUTH_NOW + 30_000,
            )
        }

        let root = tempfile::tempdir().expect("temporary root");
        let bus_root = root.path().join("bus");
        let workgroup = root.path().join("workgroup");
        std::fs::create_dir_all(&workgroup).unwrap();
        let persist = Persist::open(bus_root.clone()).expect("prepare delayed Bus");
        persist
            .write(
                ACTION_TOPIC,
                Priority::Default,
                None,
                Some(&signed_apply_body("spawn-startup-stale")),
            )
            .expect("write retained spawn command");

        let provisioner = FakeProvisioner::default();
        let calls = Arc::clone(&provisioner.calls);
        let authorizer = Arc::new(ActionAuthorizer::for_test(
            KEY,
            root.path().join("auth"),
            AUTH_NOW,
        ));
        let open_attempts = Arc::new(AtomicUsize::new(0));
        let open_attempts_for_worker = Arc::clone(&open_attempts);
        let bus_root_for_worker = bus_root.clone();
        let prime_attempts = Arc::new(AtomicUsize::new(0));
        let prime_attempts_for_worker = Arc::clone(&prime_attempts);
        let fail_reads = Arc::new(AtomicBool::new(false));
        let fail_reads_for_worker = Arc::clone(&fail_reads);
        let fail_publication = Arc::new(AtomicBool::new(false));
        let fail_publication_for_worker = Arc::clone(&fail_publication);
        let mut worker = SpawnLighthouseOnboardWorker::new(workgroup, "peer:leader".into())
            .with_provisioner(Box::new(provisioner))
            .with_authorizer(authorizer)
            .with_facts(facts(true, true))
            .with_bus_root(bus_root.clone())
            .with_poll(Duration::from_millis(5))
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
                    return Err("injected action-tail failure".into());
                }
                prime_cursor(persist)
            }))
            .with_action_read_gate(Arc::new(move || {
                if fail_reads_for_worker.load(Ordering::SeqCst) {
                    Err("injected command read failure".into())
                } else {
                    Ok(())
                }
            }))
            .with_event_publish_gate(Arc::new(move || {
                if fail_publication_for_worker.load(Ordering::SeqCst) {
                    Err("injected durable event failure".into())
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
        assert!(calls.lock().unwrap().is_empty(), "retained spawn replayed");
        assert!(persist.list_since(EVENT_TOPIC, None).unwrap().is_empty());

        fail_reads.store(true, Ordering::SeqCst);
        fail_publication.store(true, Ordering::SeqCst);
        // This handle is intentionally separate from the worker's long-held
        // Persist. The forward write proves runtime refresh + list_since sees
        // external publishers after activation.
        persist
            .write(
                ACTION_TOPIC,
                Priority::Default,
                None,
                Some(&signed_apply_body("spawn-forward")),
            )
            .expect("write forward spawn command");
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(
            calls.lock().unwrap().is_empty(),
            "read failure allowed mutation"
        );

        fail_reads.store(false, Ordering::SeqCst);
        tokio::time::timeout(Duration::from_secs(3), async {
            while calls.lock().unwrap().len() < 3 {
                assert!(!task.is_finished(), "worker exited before forward mutation");
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("forward command must mutate after read recovery");
        assert!(
            persist.list_since(EVENT_TOPIC, None).unwrap().is_empty(),
            "publication failure must not claim durable success"
        );

        fail_publication.store(false, Ordering::SeqCst);
        tokio::time::timeout(Duration::from_secs(3), async {
            while persist.list_since(EVENT_TOPIC, None).unwrap().len() < 1 {
                assert!(
                    !task.is_finished(),
                    "worker exited before durable publication"
                );
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("pending event must publish after Bus recovery");
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            ["provision", "push_enroll", "migrate_ca"],
            "publication retry must not replay the completed mutation"
        );
        assert_eq!(persist.list_since(EVENT_TOPIC, None).unwrap().len(), 1);

        shutdown_tx.send(true).expect("request shutdown");
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .expect("shutdown timed out")
            .expect("worker task panicked")
            .expect("worker returned an error");
    }

    #[tokio::test]
    async fn run_loop_exits_promptly_on_shutdown() {
        let bus = std::env::temp_dir().join(format!("mde-slo-run-{}", now_ms()));
        let wg = std::env::temp_dir().join(format!("mde-slo-runwg-{}", now_ms()));
        std::fs::create_dir_all(&wg).expect("mk workgroup");
        let (tx, rx) = tokio::sync::watch::channel(false);
        let mut w = SpawnLighthouseOnboardWorker::new(wg.clone(), "peer:a".to_string())
            .with_bus_root(bus.clone())
            .with_poll(Duration::from_millis(10));
        let token = ShutdownToken::from_receiver(rx);
        let handle = tokio::spawn(async move { w.run(token).await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        tx.send(true).expect("signal shutdown");
        let joined = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(joined.is_ok(), "worker must exit promptly on shutdown");
        assert!(joined.unwrap().expect("join").is_ok());
        let _ = std::fs::remove_dir_all(&wg);
    }
}
