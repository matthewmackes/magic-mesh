//! OW-11 (Bus half) — the `service_onboard` worker: `onboard service-add`
//! reachable over the Bus.
//!
//! The CLI verb (`mackesd onboard service-add`) and the shell's Services flow
//! must drive ONE engine (§6 glue): this worker makes the existing
//! [`crate::onboard::service_add`] core Bus-reachable, so the egui shell — which
//! deliberately never links the daemon crate — requests a service add by
//! publishing a typed [`ServiceAddAction`] on [`ACTION_TOPIC`] and renders the
//! typed [`ServiceAddEvent`] answer off [`EVENT_TOPIC`].
//!
//! ## Shape (mirrors [`super::session_broker`] / [`super::scheduler`])
//!
//! - The **pure core** is [`resolve`]: one drained action + the gathered
//!   [`ServiceAddFacts`] + the injectable [`ServiceApply`] seam → the one result
//!   event. It REUSES the onboard engine verbatim —
//!   [`plan_service_add`] for the plan and [`execute`] over the seam for a real
//!   run — reimplementing none of the planning logic (§6).
//! - **Dry-run** resolves the plan only (steps + the honest blocked/no-op
//!   outcomes) and never touches the seam — the preview the shell renders.
//! - **Apply** drives [`execute`] over the production [`LiveServiceApply`],
//!   whose typed [`ServiceError::IntegrationGated`] is the honest live answer
//!   today (§7 — a real typed error on the wire, never a fake success).
//! - **Leader-gated** like `scheduler`: the action log is mesh-replicated, so
//!   only the elected node resolves + publishes — an N-node mesh answers each
//!   request once, not N times. The facts come off the replicated peer roster
//!   ([`crate::onboard::service_add::gather`]), so any leader plans identically.
//! - The cursor **primes past the backlog** on start (like `scheduler`, unlike
//!   `session_broker`): a service add is a one-shot verb, not a fold — a restart
//!   must not re-drive historical applies.

#![cfg(feature = "async-services")]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use mde_bus::persist::{Persist, StoredMessage};

use crate::ipc::action_auth::{ActionAuthorizer, MutationContext};
use crate::onboard::service_add::{
    execute, plan_service_add, ServiceAddFacts, ServiceAddRequest, ServiceApply, ServiceError,
    ServiceKind, SipAccount,
};

use super::scheduler::{BusPublisher, Publisher};
use super::{ShutdownToken, Worker};

/// Bus action topic this worker drains — the `action/<domain>/<verb>` convention
/// (`action/vdi/session`, `action/compute/migrate`, …) applied to the onboard
/// family's day-2 Services verb.
pub const ACTION_TOPIC: &str = "action/onboard/service-add";

/// Bus event topic the typed result is published on — the matching
/// `event/<domain>/<verb>` lane the shell's Services flow tails.
pub const EVENT_TOPIC: &str = "event/onboard/service-add";

/// Closed capability verb for non-preview service onboarding actions.
pub const SERVICE_ONBOARD_AUTH_VERB: &str = "service-onboard";

/// Stable capability scope for this leader-coordinated service plane.
pub const SERVICE_ONBOARD_NODE_SCOPE: &str = "service-onboard";

/// Current wire schema for service-add actions.
pub const SERVICE_ACTION_SCHEMA_VERSION: u64 = 1;

/// Bound the caller-controlled correlation identity before it reaches the
/// authenticated action target or durable event echo.
const MAX_SERVICE_ACTION_ID_BYTES: usize = 128;

/// Poll cadence. The bus read is a cheap local log scan and a service add is a
/// slow, operator-paced event, so the 2 s `session_broker` cadence is responsive
/// without spinning.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Lower bound for retrying an unresolved, unopenable, or unsafe-to-activate
/// Bus without turning startup failure into a tight loop.
const MIN_BUS_RETRY_INTERVAL: Duration = Duration::from_millis(10);

/// Upper bound for startup retry backoff. The same worker must recover when the
/// canonical Bus arrives instead of depending on a supervisor restart.
const MAX_BUS_RETRY_INTERVAL: Duration = Duration::from_secs(2);

#[cfg(test)]
type BusOpenFn = dyn Fn(&Path) -> Result<Option<Persist>, String> + Send + Sync;

#[cfg(test)]
type CursorPrimeFn = dyn Fn(&Persist) -> Result<Option<String>, String> + Send + Sync;

#[cfg(test)]
type ActionReadFn =
    dyn Fn(&Persist, Option<&str>) -> Result<Vec<StoredMessage>, String> + Send + Sync;

// ───────────────────────────── wire contract ─────────────────────────────

/// The operator-supplied external-SIP registration parameters (Voice only) — the
/// non-secret half of [`SipAccount`]. The `creds_ref` is deliberately NOT on the
/// wire: the daemon derives it via [`SipAccount::new`] (the single
/// `sip_creds_ref` source of truth), so a front-end can't name an arbitrary
/// secret-store key.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SipParams {
    /// The SIP registrar host (e.g. `sip.provider.net`).
    pub registrar: String,
    /// The SIP address-of-record domain.
    pub domain: String,
    /// The SIP account username.
    pub username: String,
}

/// A service-add request drained off [`ACTION_TOPIC`] — the wire verb the shell
/// (or any front-end) publishes. Mirrors the CLI's `onboard service-add` args:
/// the [`ServiceKind`], the Voice SIP account, and the `--dry-run` flag, plus a
/// caller-minted `id` the result event echoes for correlation.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ServiceAddAction {
    /// Version of the action envelope; apply capabilities bind this field.
    #[serde(default = "default_service_action_schema_version")]
    pub schema_version: u64,
    /// Caller-minted correlation id — echoed on the [`ServiceAddEvent`].
    pub id: String,
    /// Which curated service to add (reused [`ServiceKind`] — `music` | `files`
    /// | `voice` on the wire).
    pub kind: ServiceKind,
    /// The external SIP account params (Voice only; absent otherwise — a Voice
    /// request without one resolves to the honest retryable blocked outcome).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sip: Option<SipParams>,
    /// `true` ⇒ resolve + publish the plan only; the [`ServiceApply`] seam is
    /// never touched.
    #[serde(default)]
    pub dry_run: bool,
}

const fn default_service_action_schema_version() -> u64 {
    SERVICE_ACTION_SCHEMA_VERSION
}

/// Parse a [`ServiceAddAction`] body.
///
/// # Errors
/// A human-readable message on malformed JSON.
pub fn parse_action(body: &str) -> Result<ServiceAddAction, String> {
    let action: ServiceAddAction =
        serde_json::from_str(body).map_err(|e| format!("malformed service-add action: {e}"))?;
    if action.schema_version != SERVICE_ACTION_SCHEMA_VERSION {
        return Err(format!(
            "unsupported service-add action schema version {}",
            action.schema_version
        ));
    }
    if action.id.is_empty()
        || action.id.len() > MAX_SERVICE_ACTION_ID_BYTES
        || !action
            .id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:".contains(&byte))
    {
        return Err("service-add action id is invalid".to_string());
    }
    Ok(action)
}

/// The typed [`ServiceError`] on the wire — the same two variants, tagged on
/// `type`, so the shell renders "integration-gated" and "failed" distinctly
/// (never collapsed into a fake success or an untyped string).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireServiceError {
    /// The live path is honestly gated on a real prerequisite — retriable only
    /// once the named integration lands.
    IntegrationGated {
        /// Which seam step (`provision-music` / `register-voice`).
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

impl From<ServiceError> for WireServiceError {
    fn from(e: ServiceError) -> Self {
        match e {
            ServiceError::IntegrationGated { step, reason } => Self::IntegrationGated {
                step: step.to_string(),
                reason,
            },
            ServiceError::Failed { step, reason } => Self::Failed {
                step: step.to_string(),
                reason,
            },
        }
    }
}

/// The typed result published on [`EVENT_TOPIC`]: the request echo (`id` /
/// `kind` / `dry_run`), the resolved plan's ordered step descriptions, the
/// one-line summary ([`plan.human()`](crate::onboard::service_add::ServiceAddPlan::human)
/// for a dry-run, the outcome's for an apply), whether an operator retry is
/// available (the honest blocked outcomes), and the typed error when the apply
/// seam refused.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ServiceAddEvent {
    /// The request's correlation id, echoed.
    pub id: String,
    /// The requested service kind, echoed.
    pub kind: ServiceKind,
    /// Whether this answers a dry-run (plan preview) or an apply.
    pub dry_run: bool,
    /// The one-line human summary (plan for dry-run, outcome/error for apply).
    pub summary: String,
    /// The plan's ordered step descriptions (empty for the blocked / no-op
    /// outcomes — nothing would be spawned).
    #[serde(default)]
    pub steps: Vec<String>,
    /// `true` for the honest retryable blocked outcomes (no lighthouse / no SIP
    /// account) — the mesh keeps running and the operator retries after clearing
    /// the blocker.
    #[serde(default)]
    pub retry_available: bool,
    /// The typed seam error when an apply couldn't run (`None` for a dry-run and
    /// for a completed / honestly-blocked apply).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<WireServiceError>,
}

// ───────────────────────────── pure: resolve ─────────────────────────────

/// Pure orchestration: one drained [`ServiceAddAction`] + the gathered
/// [`ServiceAddFacts`] + the injectable [`ServiceApply`] seam → the one
/// [`ServiceAddEvent`] to publish.
///
/// Reuses the onboard engine verbatim: [`plan_service_add`] resolves the plan
/// (branching per [`ServiceKind`], selecting the media lighthouse / capturing
/// the SIP account / the honest no-op + blocked outcomes), and — apply only —
/// [`execute`] drives the seam. A dry-run never touches the seam; a blocked /
/// no-op plan short-circuits inside `execute` without seam calls either.
#[must_use]
pub fn resolve(
    action: &ServiceAddAction,
    facts: &ServiceAddFacts,
    apply: &dyn ServiceApply,
) -> ServiceAddEvent {
    let request = ServiceAddRequest {
        kind: action.kind,
        sip: action
            .sip
            .as_ref()
            .map(|p| SipAccount::new(&p.registrar, &p.domain, &p.username)),
    };
    let plan = plan_service_add(&request, facts);
    let steps: Vec<String> = plan.steps().iter().map(|s| (*s).to_string()).collect();
    let retry_available = plan.retry_available();
    let (summary, error) = if action.dry_run {
        (plan.human(), None)
    } else {
        match execute(&plan, apply) {
            Ok(outcome) => (outcome.human(), None),
            Err(e) => (e.to_string(), Some(WireServiceError::from(e))),
        }
    };
    ServiceAddEvent {
        id: action.id.clone(),
        kind: action.kind,
        dry_run: action.dry_run,
        summary,
        steps,
        retry_available,
        error,
    }
}

// ─────────────────────────── bus + worker ───────────────────────────

/// Read new [`ACTION_TOPIC`] messages since `cursor`, advancing it. A short sync
/// open-read-drop (never crosses an `.await`), mirroring `session_broker`. A
/// malformed action is dropped honestly with a warn. Raw bodies stay attached
/// until the elected node can authenticate them immediately before resolution.
fn parse_new_actions(
    msgs: Vec<StoredMessage>,
    cursor: &mut Option<String>,
) -> Vec<(String, ServiceAddAction)> {
    let mut out = Vec::new();
    for msg in msgs {
        *cursor = Some(msg.ulid.clone());
        let body = msg.body.as_deref().unwrap_or("");
        match parse_action(body) {
            Ok(a) => out.push((body.to_string(), a)),
            Err(e) => {
                tracing::warn!(ulid = %msg.ulid, error = %e, "service_onboard: bad service-add action");
            }
        }
    }
    out
}

/// Tail-prime the one transient service-add command lane as the activation
/// transaction. A read error is activation failure, never an empty backlog.
fn prime_cursor(persist: &Persist) -> Result<Option<String>, String> {
    persist
        .latest_ulid(ACTION_TOPIC)
        .map_err(|error| format!("prime {ACTION_TOPIC}: {error}"))
}

/// Bind a service-add capability to the semantic service target, not merely the
/// shared action topic. Voice includes its external registrar/account identity;
/// Music and Files are closed service scopes because they do not accept a
/// caller-selected host target.
#[must_use]
fn service_auth_target(action: &ServiceAddAction) -> String {
    match (action.kind, action.sip.as_ref()) {
        (ServiceKind::Voice, Some(sip)) => format!(
            "service:voice:{}@{} via {}",
            sip.username, sip.domain, sip.registrar
        ),
        (kind, _) => format!("service:{}", kind.as_str()),
    }
}

/// Verify the exact original body before [`resolve`] can reach any apply seam.
fn authorize_service_action(
    authorizer: &ActionAuthorizer,
    body: &str,
    action: &ServiceAddAction,
) -> Result<(), String> {
    let target = service_auth_target(action);
    authorizer.authorize(
        body,
        MutationContext {
            verb: SERVICE_ONBOARD_AUTH_VERB,
            node: SERVICE_ONBOARD_NODE_SCOPE,
            target: &target,
        },
    )
}

fn service_onboard_bus_root(override_root: Option<PathBuf>) -> PathBuf {
    service_onboard_bus_root_or_system(override_root.or_else(mde_bus::default_data_dir))
}

fn service_onboard_bus_root_or_system(resolved: Option<PathBuf>) -> PathBuf {
    resolved.unwrap_or_else(|| PathBuf::from(mde_bus::SYSTEM_BUS_ROOT))
}

fn next_bus_retry_interval(current: Duration) -> Duration {
    current
        .saturating_mul(2)
        .clamp(MIN_BUS_RETRY_INTERVAL, MAX_BUS_RETRY_INTERVAL)
}

/// The Bus-reachable `onboard service-add` worker. Leader-gated + best-effort.
pub struct ServiceOnboardWorker {
    /// Shared-storage root — where [`crate::onboard::service_add::gather`] reads
    /// the replicated peer roster and where the leader lock lives.
    workgroup_root: PathBuf,
    /// This node's id — its identity in the leader election.
    node_id: String,
    /// The shared leader lock (the same `.mackesd-leader.lock` `session_broker` /
    /// `dc_auditor` use).
    leader_lock: PathBuf,
    /// The injectable apply seam (production: [`LiveServiceApply`]).
    apply: Box<dyn ServiceApply + Send + Sync>,
    /// The injectable publish seam (production: the shared [`BusPublisher`]).
    publisher: Box<dyn Publisher + Send + Sync>,
    /// Poll cadence.
    poll: Duration,
    /// Exact-body capability verifier for the privileged apply lane.
    /// Missing production credentials fail closed.
    authorizer: Arc<ActionAuthorizer>,
    /// Bus root override (tests). `None` resolves the user Bus, then the
    /// canonical system Bus fallback.
    bus_root_override: Option<PathBuf>,
    /// Dynamic Bus open seam for deterministic startup recovery tests.
    #[cfg(test)]
    bus_open_override: Option<Arc<BusOpenFn>>,
    /// Cursor-prime seam for deterministic fail-closed activation tests.
    #[cfg(test)]
    cursor_prime_override: Option<Arc<CursorPrimeFn>>,
    /// Action read seam for deterministic fail-closed poll tests.
    #[cfg(test)]
    action_read_override: Option<Arc<ActionReadFn>>,
}

impl ServiceOnboardWorker {
    /// Construct with production defaults: the honestly integration-gated
    /// [`LiveServiceApply`], the shared [`BusPublisher`], the shared leader lock
    /// under `workgroup_root`, and the default cadence.
    #[must_use]
    pub fn new(workgroup_root: PathBuf, node_id: String) -> Self {
        let leader_lock = workgroup_root.join(".mackesd-leader.lock");
        Self {
            workgroup_root,
            node_id,
            leader_lock,
            apply: Box::new(crate::onboard::service_add::LiveServiceApply::default()),
            publisher: Box::new(BusPublisher),
            poll: DEFAULT_POLL_INTERVAL,
            authorizer: Arc::new(ActionAuthorizer::production()),
            bus_root_override: None,
            #[cfg(test)]
            bus_open_override: None,
            #[cfg(test)]
            cursor_prime_override: None,
            #[cfg(test)]
            action_read_override: None,
        }
    }

    /// Inject an apply seam (tests). Production uses [`LiveServiceApply`].
    #[must_use]
    pub fn with_apply(mut self, apply: Box<dyn ServiceApply + Send + Sync>) -> Self {
        self.apply = apply;
        self
    }

    /// Inject a publisher (tests). Production uses [`BusPublisher`].
    #[must_use]
    pub fn with_publisher(mut self, publisher: Box<dyn Publisher + Send + Sync>) -> Self {
        self.publisher = publisher;
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

    /// Override Bus opening without changing production retry semantics.
    #[cfg(test)]
    #[must_use]
    fn with_bus_opener(mut self, open: Arc<BusOpenFn>) -> Self {
        self.bus_open_override = Some(open);
        self
    }

    /// Override command-tail priming for deterministic activation failures.
    #[cfg(test)]
    #[must_use]
    fn with_cursor_primer(mut self, prime: Arc<CursorPrimeFn>) -> Self {
        self.cursor_prime_override = Some(prime);
        self
    }

    /// Override action reads for deterministic unavailable-state tests.
    #[cfg(test)]
    #[must_use]
    fn with_action_reader(mut self, read: Arc<ActionReadFn>) -> Self {
        self.action_read_override = Some(read);
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

    fn open_bus(&self, root: &Path) -> Result<Option<Persist>, String> {
        #[cfg(test)]
        if let Some(open) = self.bus_open_override.as_ref() {
            return open(root);
        }

        Persist::open(root.to_path_buf())
            .map(Some)
            .map_err(|error| error.to_string())
    }

    fn prime_cursor(&self, persist: &Persist) -> Result<Option<String>, String> {
        #[cfg(test)]
        if let Some(prime) = self.cursor_prime_override.as_ref() {
            return prime(persist);
        }

        prime_cursor(persist)
    }

    fn read_action_messages(
        &self,
        persist: &Persist,
        cursor: Option<&str>,
    ) -> Result<Vec<StoredMessage>, String> {
        #[cfg(test)]
        if let Some(read) = self.action_read_override.as_ref() {
            return read(persist, cursor);
        }

        persist
            .list_since(ACTION_TOPIC, cursor)
            .map_err(|error| format!("read {ACTION_TOPIC}: {error}"))
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

    /// Drain new actions (advancing `cursor`) and — leader only — resolve each
    /// through the reused onboard engine and publish its typed result event.
    /// Non-leaders still advance their cursor (the `scheduler` convention), so
    /// a node that later wins the election answers new requests, not backlog.
    fn drain_and_publish(
        &self,
        persist: &Persist,
        cursor: &mut Option<String>,
    ) -> Result<(), String> {
        // Read the complete fixed command lane before moving the cursor or
        // gathering durable workgroup facts. A failed read defers every effect;
        // it is never interpreted as an empty command view.
        let messages = self.read_action_messages(persist, cursor.as_deref())?;
        let actions = parse_new_actions(messages, cursor);
        if actions.is_empty() || !self.is_leader() {
            return Ok(());
        }
        // Gather once per tick — the replicated roster is the same for every
        // action in this batch.
        let facts = crate::onboard::service_add::gather(&self.workgroup_root);
        for (body, action) in actions {
            // Dry-run is a read-only plan preview and remains usable without an
            // arm token. Every apply, including currently retired/no-op branches,
            // is authenticated before `resolve` can reach `execute` or a future
            // ServiceApply backend.
            if !action.dry_run {
                if let Err(error) =
                    authorize_service_action(self.authorizer.as_ref(), &body, &action)
                {
                    tracing::warn!(
                        id = %action.id,
                        error = %error,
                        "service_onboard: unauthorized service-add apply refused"
                    );
                    continue;
                }
            }
            let event = resolve(&action, &facts, self.apply.as_ref());
            match serde_json::to_string(&event) {
                Ok(body) => self.publisher.publish(EVENT_TOPIC, &body),
                Err(e) => {
                    tracing::warn!(id = %event.id, error = %e, "service_onboard: event serialize failed");
                }
            }
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl Worker for ServiceOnboardWorker {
    fn name(&self) -> &'static str {
        "service_onboard"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let bus_root = service_onboard_bus_root(self.bus_root_override.clone());
        let mut retry_interval = MIN_BUS_RETRY_INTERVAL;
        let (persist, mut cursor) = loop {
            match self.open_bus(&bus_root) {
                Ok(Some(persist)) => match self.prime_cursor(&persist) {
                    Ok(cursor) => break (persist, cursor),
                    Err(error) => tracing::warn!(
                        %error,
                        "service_onboard: command activation failed; startup will retry"
                    ),
                },
                Ok(None) => {
                    tracing::debug!("service_onboard: Bus root unavailable; startup will retry")
                }
                Err(error) => tracing::warn!(
                    %error,
                    "service_onboard: Bus open failed; startup will retry"
                ),
            }

            tokio::select! {
                () = shutdown.wait() => return Ok(()),
                () = tokio::time::sleep(retry_interval) => {}
            }
            retry_interval = next_bus_retry_interval(retry_interval);
        };
        // Prime past the backlog: service-add is a one-shot command, not a
        // durable fold. Replicated workgroup facts are gathered only for a
        // successfully read forward command after activation.
        let mut tick = tokio::time::interval(self.poll);
        tick.tick().await; // consume the immediate first tick
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    if let Err(error) = self.drain_and_publish(&persist, &mut cursor) {
                        tracing::warn!(
                            %error,
                            "service_onboard: Bus read failed; command effects deferred"
                        );
                    }
                }
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
    use crate::onboard::service_add::{
        LighthouseFact, LiveServiceApply, MediaLighthouseTarget, MusicEndpoint,
    };
    use mde_bus::hooks::config::Priority;
    use std::sync::{Arc, Mutex};

    fn facts_with_media_lighthouse() -> ServiceAddFacts {
        ServiceAddFacts {
            lighthouses: vec![LighthouseFact {
                hostname: "lh-media".to_string(),
                overlay_ip: Some("10.42.0.2".to_string()),
                media: true,
            }],
        }
    }

    fn empty_facts() -> ServiceAddFacts {
        ServiceAddFacts {
            lighthouses: vec![],
        }
    }

    fn action(kind: ServiceKind, sip: Option<SipParams>, dry_run: bool) -> ServiceAddAction {
        ServiceAddAction {
            schema_version: SERVICE_ACTION_SCHEMA_VERSION,
            id: format!("svc-test-{}", kind.as_str()),
            kind,
            sip,
            dry_run,
        }
    }

    fn sip() -> SipParams {
        SipParams {
            registrar: "sip.provider.net".to_string(),
            domain: "provider.net".to_string(),
            username: "alice".to_string(),
        }
    }

    /// Recording [`ServiceApply`] fake — pins that dry-runs and no-op plans
    /// never touch the seam, and that applies drive it.
    #[derive(Default)]
    struct FakeApply {
        calls: Arc<Mutex<Vec<&'static str>>>,
    }

    impl ServiceApply for FakeApply {
        fn provision_music(
            &self,
            target: &MediaLighthouseTarget,
            _creds_ref: &str,
            server_url: &str,
        ) -> Result<MusicEndpoint, ServiceError> {
            self.calls
                .lock()
                .expect("calls mutex")
                .push("provision_music");
            Ok(MusicEndpoint {
                host: target.hostname.clone(),
                server_url: server_url.to_string(),
            })
        }
        fn register_voice(&self, _account: &SipAccount) -> Result<(), ServiceError> {
            self.calls
                .lock()
                .expect("calls mutex")
                .push("register_voice");
            Ok(())
        }
    }

    // ── topics follow the action/event convention ──

    #[test]
    fn topics_follow_the_action_event_convention() {
        assert_eq!(ACTION_TOPIC, "action/onboard/service-add");
        assert!(ACTION_TOPIC.starts_with("action/"));
        assert_eq!(EVENT_TOPIC, "event/onboard/service-add");
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
        let a = ServiceAddAction {
            schema_version: SERVICE_ACTION_SCHEMA_VERSION,
            id: "svc-42-voice".to_string(),
            kind: ServiceKind::Voice,
            sip: Some(sip()),
            dry_run: true,
        };
        let json = serde_json::to_string(&a).expect("serialize");
        assert_eq!(
            json,
            r#"{"schema_version":1,"id":"svc-42-voice","kind":"voice","sip":{"registrar":"sip.provider.net","domain":"provider.net","username":"alice"},"dry_run":true}"#
        );
        assert_eq!(parse_action(&json).expect("parse"), a);
        // Music/Files omit `sip`; `dry_run` defaults false when absent.
        let m: ServiceAddAction =
            parse_action(r#"{"id":"svc-1","kind":"music"}"#).expect("minimal parse");
        assert_eq!(m.kind, ServiceKind::Music);
        assert!(m.sip.is_none());
        assert!(!m.dry_run);
        assert!(parse_action("not json").is_err());
    }

    #[test]
    fn action_parser_rejects_future_schema_and_unbounded_correlation_ids() {
        assert!(parse_action(r#"{"schema_version":2,"id":"svc-1","kind":"files"}"#).is_err());
        assert!(parse_action(r#"{"schema_version":1,"id":"","kind":"files"}"#).is_err());
        assert!(parse_action(&format!(
            r#"{{"schema_version":1,"id":"{}","kind":"files"}}"#,
            "x".repeat(MAX_SERVICE_ACTION_ID_BYTES + 1)
        ))
        .is_err());
        assert!(parse_action(r#"{"schema_version":1,"id":"svc/escape","kind":"files"}"#).is_err());
    }

    #[test]
    fn the_event_round_trips_including_the_typed_error() {
        let ev = ServiceAddEvent {
            id: "svc-1".to_string(),
            kind: ServiceKind::Music,
            dry_run: false,
            summary: "gated".to_string(),
            steps: vec!["step one".to_string()],
            retry_available: false,
            error: Some(WireServiceError::IntegrationGated {
                step: "provision-music".to_string(),
                reason: "needs the live provisioner".to_string(),
            }),
        };
        let json = serde_json::to_string(&ev).expect("serialize");
        // The error stays TYPED on the wire — tagged, distinguishing gated
        // from failed (§7: the shell must render the distinction honestly).
        assert!(json.contains(r#""type":"integration_gated""#));
        let back: ServiceAddEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, ev);
    }

    // ── request → plan-event round-trip, each kind, dry-run ──

    #[test]
    fn dry_run_music_reports_the_retired_lighthouse_path_without_touching_the_seam() {
        let apply = FakeApply::default();
        let ev = resolve(
            &action(ServiceKind::Music, None, true),
            &facts_with_media_lighthouse(),
            &apply,
        );
        assert_eq!(ev.id, "svc-test-music");
        assert_eq!(ev.kind, ServiceKind::Music);
        assert!(ev.dry_run);
        assert!(ev.steps.is_empty());
        assert!(ev.summary.contains("retired"));
        assert!(!ev.retry_available);
        assert!(ev.error.is_none());
        assert!(
            apply.calls.lock().expect("calls mutex").is_empty(),
            "a dry-run never touches the apply seam"
        );
    }

    #[test]
    fn dry_run_files_is_the_honest_p2p_no_op_plan() {
        let apply = FakeApply::default();
        let ev = resolve(
            &action(ServiceKind::Files, None, true),
            &empty_facts(),
            &apply,
        );
        // Nothing to provision — no steps, and the summary says so (#37/#20).
        assert!(ev.steps.is_empty());
        assert!(ev.summary.contains("peer-to-peer"));
        assert!(ev.summary.contains("nothing to provision"));
        assert!(!ev.retry_available);
        assert!(ev.error.is_none());
        assert!(apply.calls.lock().expect("calls mutex").is_empty());
    }

    #[test]
    fn dry_run_voice_with_an_account_returns_the_registration_plan() {
        let apply = FakeApply::default();
        let ev = resolve(
            &action(ServiceKind::Voice, Some(sip()), true),
            &empty_facts(),
            &apply,
        );
        // The 3 ordered external-SIP registration steps; the daemon derived the
        // creds ref itself (the wire carries no secret-store key).
        assert_eq!(ev.steps.len(), 3);
        assert!(ev.summary.contains("external SIP registrar"));
        assert!(ev.summary.contains("alice@provider.net"));
        assert!(!ev.retry_available);
        assert!(ev.error.is_none());
        assert!(apply.calls.lock().expect("calls mutex").is_empty());
    }

    #[test]
    fn dry_run_blocked_outcomes_are_retryable_not_errors() {
        let apply = FakeApply::default();
        // Music is retired regardless of roster facts.
        let m = resolve(
            &action(ServiceKind::Music, None, true),
            &empty_facts(),
            &apply,
        );
        assert!(!m.retry_available);
        assert!(m.steps.is_empty());
        assert!(m.summary.contains("retired"));
        assert!(m.error.is_none(), "blocked is an outcome, not an error");
        // Voice with no account → the honest retryable blocked plan.
        let v = resolve(
            &action(ServiceKind::Voice, None, true),
            &empty_facts(),
            &apply,
        );
        assert!(v.retry_available);
        assert!(v.summary.contains("--sip-registrar"));
        assert!(v.error.is_none());
        assert!(apply.calls.lock().expect("calls mutex").is_empty());
    }

    // ── apply drives the seam / surfaces the typed error ──

    #[test]
    fn apply_music_refuses_the_retired_path_without_driving_the_seam() {
        let apply = FakeApply::default();
        let ev = resolve(
            &action(ServiceKind::Music, None, false),
            &facts_with_media_lighthouse(),
            &apply,
        );
        assert!(!ev.dry_run);
        assert!(ev.summary.contains("retired"));
        assert!(ev.error.is_none());
        assert!(apply.calls.lock().expect("calls mutex").is_empty());
    }

    #[test]
    fn apply_files_never_touches_the_seam() {
        let apply = FakeApply::default();
        let ev = resolve(
            &action(ServiceKind::Files, None, false),
            &empty_facts(),
            &apply,
        );
        assert!(ev.summary.contains("peer-to-peer"));
        assert!(ev.error.is_none());
        assert!(
            apply.calls.lock().expect("calls mutex").is_empty(),
            "a P2P Files add never touches live infra"
        );
    }

    #[test]
    fn apply_through_the_live_seam_preserves_the_retired_no_op() {
        let ev = resolve(
            &action(ServiceKind::Music, None, false),
            &facts_with_media_lighthouse(),
            &LiveServiceApply::default(),
        );
        assert!(ev.error.is_none());
        assert!(ev.summary.contains("retired"));
    }

    #[test]
    fn apply_capability_rejects_unsigned_tampered_and_replayed_bodies() {
        const KEY: &[u8] = b"service-onboard-action-auth-key";
        const NOW: i64 = 1_700_000_000_000;
        let unsigned = r#"{"schema_version":1,"id":"svc-auth-voice","kind":"voice","sip":{"registrar":"sip.provider.net","domain":"provider.net","username":"alice"},"dry_run":false}"#;
        let action = parse_action(unsigned).expect("valid service action");
        let target = service_auth_target(&action);
        let armed = authorize_test_body(
            KEY,
            unsigned,
            MutationContext {
                verb: SERVICE_ONBOARD_AUTH_VERB,
                node: SERVICE_ONBOARD_NODE_SCOPE,
                target: &target,
            },
            "service-onboard-once",
            NOW + 30_000,
        );
        let tampered = armed.replace("alice", "mallory");
        let tampered_action = parse_action(&tampered).expect("tamper remains parseable");
        let auth_root = tempfile::tempdir().expect("auth root");
        let authorizer = ActionAuthorizer::for_test(KEY, auth_root.path().to_path_buf(), NOW);

        assert!(authorize_service_action(&authorizer, unsigned, &action).is_err());
        assert!(authorize_service_action(&authorizer, &tampered, &tampered_action).is_err());
        assert!(authorize_service_action(&authorizer, &armed, &action).is_ok());
        assert!(authorize_service_action(&authorizer, &armed, &action).is_err());
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

    fn seed_bus(actions: &[ServiceAddAction]) -> PathBuf {
        let bodies = actions
            .iter()
            .map(|action| serde_json::to_string(action).unwrap())
            .collect::<Vec<_>>();
        seed_bus_bodies(&bodies)
    }

    fn seed_bus_bodies(bodies: &[String]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mde-so-{}-{}", now_ms(), bodies.len()));
        let persist = Persist::open(dir.clone()).expect("open bus");
        for body in bodies {
            persist
                .write(ACTION_TOPIC, Priority::Default, None, Some(body))
                .expect("write action");
        }
        dir
    }

    #[test]
    fn service_bus_root_falls_back_to_the_canonical_system_spool() {
        assert_eq!(
            service_onboard_bus_root_or_system(None),
            PathBuf::from(mde_bus::SYSTEM_BUS_ROOT)
        );
        assert_eq!(
            service_onboard_bus_root_or_system(Some(PathBuf::from(
                "/tmp/service-onboard-explicit-bus",
            ))),
            PathBuf::from("/tmp/service-onboard-explicit-bus")
        );
    }

    #[test]
    fn bus_read_failure_defers_effects_and_retains_the_command_cursor() {
        let root = tempfile::tempdir().expect("temp root");
        let bus_root = root.path().join("bus");
        let persist = Persist::open(bus_root).expect("open Bus");
        let forward = ServiceAddAction {
            schema_version: SERVICE_ACTION_SCHEMA_VERSION,
            id: "svc-read-recovery".into(),
            kind: ServiceKind::Files,
            sip: None,
            dry_run: true,
        };
        persist
            .write(
                ACTION_TOPIC,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&forward).unwrap()),
            )
            .expect("write forward command");

        let fail_read = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let fail_read_for_worker = Arc::clone(&fail_read);
        let publisher = RecordingPublisher::default();
        let sent = Arc::clone(&publisher.sent);
        let worker = ServiceOnboardWorker::new(root.path().join("workgroup"), "peer:a".into())
            .with_publisher(Box::new(publisher))
            .with_action_reader(Arc::new(move |persist, cursor| {
                if fail_read_for_worker.load(std::sync::atomic::Ordering::SeqCst) {
                    return Err("injected service-onboard Bus read failure".into());
                }
                persist
                    .list_since(ACTION_TOPIC, cursor)
                    .map_err(|error| error.to_string())
            }));
        std::fs::create_dir_all(root.path().join("workgroup")).unwrap();

        let mut cursor = None;
        assert!(worker.drain_and_publish(&persist, &mut cursor).is_err());
        assert!(cursor.is_none(), "failed read must not move the cursor");
        assert!(sent.lock().unwrap().is_empty(), "failed read has no effect");

        fail_read.store(false, std::sync::atomic::Ordering::SeqCst);
        worker
            .drain_and_publish(&persist, &mut cursor)
            .expect("recovered read");
        assert!(cursor.is_some());
        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        let event: ServiceAddEvent = serde_json::from_str(&sent[0].1).unwrap();
        assert_eq!(event.id, "svc-read-recovery");
    }

    #[tokio::test]
    async fn late_bus_retries_activation_skips_history_and_executes_forward_messages() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let root = tempfile::tempdir().expect("temp root");
        let bus_root = root.path().join("bus");
        let persist = Persist::open(bus_root.clone()).expect("prepare delayed Bus");
        let stale = ServiceAddAction {
            schema_version: SERVICE_ACTION_SCHEMA_VERSION,
            id: "svc-startup-stale".into(),
            kind: ServiceKind::Files,
            sip: None,
            dry_run: true,
        };
        persist
            .write(
                ACTION_TOPIC,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&stale).unwrap()),
            )
            .expect("write retained startup command");

        let workgroup = root.path().join("workgroup");
        std::fs::create_dir_all(&workgroup).unwrap();
        let publisher = RecordingPublisher::default();
        let sent = Arc::clone(&publisher.sent);
        let open_attempts = Arc::new(AtomicUsize::new(0));
        let open_attempts_for_worker = Arc::clone(&open_attempts);
        let bus_root_for_worker = bus_root.clone();
        let prime_attempts = Arc::new(AtomicUsize::new(0));
        let prime_attempts_for_worker = Arc::clone(&prime_attempts);
        let mut worker = ServiceOnboardWorker::new(workgroup, "peer:a".into())
            .with_publisher(Box::new(publisher))
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
                    return Err("injected service-onboard tail read failure".into());
                }
                prime_cursor(persist)
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
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(
            sent.lock().unwrap().is_empty(),
            "retained startup command must not replay"
        );

        for (id, expected_len) in [("svc-forward-one", 1), ("svc-forward-two", 2)] {
            let forward = ServiceAddAction {
                schema_version: SERVICE_ACTION_SCHEMA_VERSION,
                id: id.into(),
                kind: ServiceKind::Files,
                sip: None,
                dry_run: true,
            };
            persist
                .write(
                    ACTION_TOPIC,
                    Priority::Default,
                    None,
                    Some(&serde_json::to_string(&forward).unwrap()),
                )
                .expect("write forward service command");
            tokio::time::timeout(Duration::from_secs(3), async {
                while sent.lock().unwrap().len() < expected_len {
                    assert!(!task.is_finished(), "worker exited before forward command");
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .expect("forward command must execute");
        }

        let ids = {
            let sent_guard = sent.lock().unwrap();
            assert_eq!(sent_guard.len(), 2, "each forward command executes once");
            sent_guard
                .iter()
                .map(|(_, body)| serde_json::from_str::<ServiceAddEvent>(body).unwrap().id)
                .collect::<Vec<_>>()
        };
        assert_eq!(ids, ["svc-forward-one", "svc-forward-two"]);

        shutdown_tx.send(true).expect("request shutdown");
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .expect("shutdown must interrupt worker promptly")
            .expect("worker task must join")
            .expect("worker must exit cleanly");
    }

    #[test]
    fn worker_drains_the_request_and_publishes_the_matching_event() {
        // A dry-run Files request drained off a real temp bus and answered on
        // EVENT_TOPIC with the echoed id (a fresh temp workgroup ⇒ this node
        // wins the leader lock).
        let bus = seed_bus(&[ServiceAddAction {
            schema_version: SERVICE_ACTION_SCHEMA_VERSION,
            id: "svc-77-files".to_string(),
            kind: ServiceKind::Files,
            sip: None,
            dry_run: true,
        }]);
        let wg = std::env::temp_dir().join(format!("mde-so-wg-{}", now_ms()));
        std::fs::create_dir_all(&wg).expect("mk workgroup");
        let rec = RecordingPublisher::default();
        let log = rec.sent.clone();
        let w = ServiceOnboardWorker::new(wg.clone(), "peer:a".to_string())
            .with_publisher(Box::new(rec))
            .with_bus_root(bus.clone());
        let persist = Persist::open(bus.clone()).expect("reopen bus");

        let mut cursor = None;
        w.drain_and_publish(&persist, &mut cursor).unwrap();

        let sent = log.lock().expect("recorder mutex");
        assert_eq!(sent.len(), 1, "one request ⇒ one event");
        assert_eq!(sent[0].0, EVENT_TOPIC);
        let ev: ServiceAddEvent = serde_json::from_str(&sent[0].1).expect("event parses");
        assert_eq!(ev.id, "svc-77-files", "the correlation id is echoed");
        assert_eq!(ev.kind, ServiceKind::Files);
        assert!(ev.dry_run);
        assert!(ev.summary.contains("peer-to-peer"));
        drop(sent);

        // The cursor advanced — a second drain re-answers nothing.
        w.drain_and_publish(&persist, &mut cursor).unwrap();
        assert_eq!(log.lock().expect("recorder mutex").len(), 1);

        let _ = std::fs::remove_dir_all(&bus);
        let _ = std::fs::remove_dir_all(&wg);
    }

    #[test]
    fn worker_authenticates_apply_before_the_backend_and_consumes_replay_once() {
        const KEY: &[u8] = b"service-onboard-worker-auth-key";
        const NOW: i64 = 1_700_000_000_000;
        let unsigned = r#"{"schema_version":1,"id":"svc-worker-voice","kind":"voice","sip":{"registrar":"sip.provider.net","domain":"provider.net","username":"alice"},"dry_run":false}"#;
        let action = parse_action(unsigned).expect("valid service action");
        let target = service_auth_target(&action);
        let armed = authorize_test_body(
            KEY,
            unsigned,
            MutationContext {
                verb: SERVICE_ONBOARD_AUTH_VERB,
                node: SERVICE_ONBOARD_NODE_SCOPE,
                target: &target,
            },
            "service-onboard-worker-once",
            NOW + 30_000,
        );
        let bus = seed_bus_bodies(&[unsigned.to_string(), armed.clone(), armed]);
        let wg = std::env::temp_dir().join(format!("mde-so-auth-wg-{}", now_ms()));
        std::fs::create_dir_all(&wg).expect("mk workgroup");
        let auth_root = tempfile::tempdir().expect("auth root");
        let apply = FakeApply::default();
        let calls = apply.calls.clone();
        let rec = RecordingPublisher::default();
        let sent = rec.sent.clone();
        let authorizer = Arc::new(ActionAuthorizer::for_test(
            KEY,
            auth_root.path().to_path_buf(),
            NOW,
        ));
        let worker = ServiceOnboardWorker::new(wg.clone(), "peer:auth".to_string())
            .with_apply(Box::new(apply))
            .with_publisher(Box::new(rec))
            .with_authorizer(authorizer)
            .with_bus_root(bus.clone());
        let persist = Persist::open(bus.clone()).expect("reopen bus");

        let mut cursor = None;
        worker.drain_and_publish(&persist, &mut cursor).unwrap();

        assert_eq!(
            calls.lock().expect("calls mutex").as_slice(),
            ["register_voice"],
            "unsigned and replayed requests must not reach the apply seam"
        );
        assert_eq!(sent.lock().expect("sent mutex").len(), 1);
        let _ = std::fs::remove_dir_all(&bus);
        let _ = std::fs::remove_dir_all(&wg);
    }

    #[tokio::test]
    async fn run_loop_exits_promptly_on_shutdown() {
        let bus = std::env::temp_dir().join(format!("mde-so-run-{}", now_ms()));
        let wg = std::env::temp_dir().join(format!("mde-so-runwg-{}", now_ms()));
        std::fs::create_dir_all(&wg).expect("mk workgroup");
        let (tx, rx) = tokio::sync::watch::channel(false);
        let mut w = ServiceOnboardWorker::new(wg.clone(), "peer:a".to_string())
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
