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
use std::time::Duration;

use mde_bus::persist::Persist;

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

/// Poll cadence. The bus read is a cheap local log scan and a service add is a
/// slow, operator-paced event, so the 2 s `session_broker` cadence is responsive
/// without spinning.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(2);

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

/// Parse a [`ServiceAddAction`] body.
///
/// # Errors
/// A human-readable message on malformed JSON.
pub fn parse_action(body: &str) -> Result<ServiceAddAction, String> {
    serde_json::from_str(body).map_err(|e| format!("malformed service-add action: {e}"))
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
/// malformed action is dropped honestly with a warn.
fn read_new_actions(bus_root: &Path, cursor: &mut Option<String>) -> Vec<ServiceAddAction> {
    let Ok(persist) = Persist::open(bus_root.to_path_buf()) else {
        return vec![];
    };
    let Ok(msgs) = persist.list_since(ACTION_TOPIC, cursor.as_deref()) else {
        return vec![];
    };
    let mut out = Vec::new();
    for msg in msgs {
        *cursor = Some(msg.ulid.clone());
        let body = msg.body.as_deref().unwrap_or("");
        match parse_action(body) {
            Ok(a) => out.push(a),
            Err(e) => {
                tracing::warn!(ulid = %msg.ulid, error = %e, "service_onboard: bad service-add action");
            }
        }
    }
    out
}

/// Seed the cursor to the newest existing message so a (re)start doesn't
/// re-drive a historical service add. `None` when the topic is empty.
fn prime_cursor(bus_root: &Path) -> Option<String> {
    let persist = Persist::open(bus_root.to_path_buf()).ok()?;
    let msgs = persist.list_since(ACTION_TOPIC, None).ok()?;
    msgs.last().map(|m| m.ulid.clone())
}

fn default_bus_root() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("mde").join("bus"))
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
    /// Bus root override (tests). `None` ⇒ [`default_bus_root`].
    bus_root_override: Option<PathBuf>,
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
            bus_root_override: None,
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

    fn bus_root(&self) -> Option<PathBuf> {
        self.bus_root_override.clone().or_else(default_bus_root)
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
    fn drain_and_publish(&self, bus_root: &Path, cursor: &mut Option<String>) {
        let actions = read_new_actions(bus_root, cursor);
        if actions.is_empty() || !self.is_leader() {
            return;
        }
        // Gather once per tick — the replicated roster is the same for every
        // action in this batch.
        let facts = crate::onboard::service_add::gather(&self.workgroup_root);
        for action in actions {
            let event = resolve(&action, &facts, self.apply.as_ref());
            match serde_json::to_string(&event) {
                Ok(body) => self.publisher.publish(EVENT_TOPIC, &body),
                Err(e) => {
                    tracing::warn!(id = %event.id, error = %e, "service_onboard: event serialize failed");
                }
            }
        }
    }
}

#[async_trait::async_trait]
impl Worker for ServiceOnboardWorker {
    fn name(&self) -> &'static str {
        "service_onboard"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let Some(bus_root) = self.bus_root() else {
            tracing::debug!("service_onboard: no bus root; worker idle");
            return Ok(());
        };
        // Prime past the backlog: a service add is a one-shot verb — a restart
        // must not re-drive historical applies (mirrors `scheduler`).
        let mut cursor = prime_cursor(&bus_root);
        let mut tick = tokio::time::interval(self.poll);
        tick.tick().await; // consume the immediate first tick
        loop {
            tokio::select! {
                _ = tick.tick() => self.drain_and_publish(&bus_root, &mut cursor),
                () = shutdown.wait() => break,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        calls: Mutex<Vec<&'static str>>,
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
            id: "svc-42-voice".to_string(),
            kind: ServiceKind::Voice,
            sip: Some(sip()),
            dry_run: true,
        };
        let json = serde_json::to_string(&a).expect("serialize");
        assert_eq!(
            json,
            r#"{"id":"svc-42-voice","kind":"voice","sip":{"registrar":"sip.provider.net","domain":"provider.net","username":"alice"},"dry_run":true}"#
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
    fn dry_run_music_returns_the_plan_without_touching_the_seam() {
        let apply = FakeApply::default();
        let ev = resolve(
            &action(ServiceKind::Music, None, true),
            &facts_with_media_lighthouse(),
            &apply,
        );
        assert_eq!(ev.id, "svc-test-music");
        assert_eq!(ev.kind, ServiceKind::Music);
        assert!(ev.dry_run);
        // The reused engine's 4 ordered Navidrome provisioning steps.
        assert_eq!(ev.steps.len(), 4);
        assert!(ev.steps[0].contains("DO Spaces"));
        assert!(ev.summary.contains("Navidrome"));
        assert!(ev.summary.contains("music.mesh"));
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
        // Music with no lighthouse → the honest retryable blocked plan.
        let m = resolve(
            &action(ServiceKind::Music, None, true),
            &empty_facts(),
            &apply,
        );
        assert!(m.retry_available);
        assert!(m.steps.is_empty());
        assert!(m.summary.contains("spawn-lighthouse"));
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
    fn apply_music_drives_the_seam_and_reports_the_outcome() {
        let apply = FakeApply::default();
        let ev = resolve(
            &action(ServiceKind::Music, None, false),
            &facts_with_media_lighthouse(),
            &apply,
        );
        assert!(!ev.dry_run);
        assert!(ev.summary.contains("Music provisioned on `lh-media`"));
        assert!(ev.error.is_none());
        assert_eq!(
            *apply.calls.lock().expect("calls mutex"),
            vec!["provision_music"]
        );
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
    fn apply_through_the_live_seam_publishes_the_typed_gated_error() {
        // The production seam is honestly integration-gated (§7) — the event
        // carries the typed error, never a fake success.
        let ev = resolve(
            &action(ServiceKind::Music, None, false),
            &facts_with_media_lighthouse(),
            &LiveServiceApply::default(),
        );
        let Some(WireServiceError::IntegrationGated { step, reason }) = &ev.error else {
            panic!("expected the typed gated error, got {:?}", ev.error);
        };
        assert_eq!(step, "provision-music");
        assert!(reason.contains("media-spaces"), "names the creds ref");
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

    fn seed_bus(actions: &[ServiceAddAction]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mde-so-{}-{}", now_ms(), actions.len()));
        let persist = Persist::open(dir.clone()).expect("open bus");
        for a in actions {
            persist
                .write(
                    ACTION_TOPIC,
                    Priority::Default,
                    None,
                    Some(&serde_json::to_string(a).unwrap()),
                )
                .expect("write action");
        }
        dir
    }

    #[test]
    fn worker_drains_the_request_and_publishes_the_matching_event() {
        // A dry-run Files request drained off a real temp bus and answered on
        // EVENT_TOPIC with the echoed id (a fresh temp workgroup ⇒ this node
        // wins the leader lock).
        let bus = seed_bus(&[ServiceAddAction {
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

        let mut cursor = None;
        w.drain_and_publish(&bus, &mut cursor);

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
        w.drain_and_publish(&bus, &mut cursor);
        assert_eq!(log.lock().expect("recorder mutex").len(), 1);

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
