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
use std::time::Duration;

use mde_bus::persist::Persist;

use crate::onboard::spawn_lighthouse::{
    execute, gather, plan_spawn, ProvisionError, Provisioner, SpawnOutcome, SpawnPlan,
    SpawnRequest, SpawnTarget,
};

use super::scheduler::{BusPublisher, Publisher};
use super::{ShutdownToken, Worker};

/// Bus action topic this worker drains — the `action/<domain>/<verb>` convention
/// applied to the onboard family's promote-to-lighthouse verb (sibling of
/// `action/onboard/service-add`).
pub const ACTION_TOPIC: &str = "action/onboard/spawn-lighthouse";

/// Bus event topic the typed result is published on — the matching
/// `event/<domain>/<verb>` lane the shell's Spawn Lighthouse flow tails.
pub const EVENT_TOPIC: &str = "event/onboard/spawn-lighthouse";

/// Poll cadence. The bus read is a cheap local log scan and a spawn is a slow,
/// operator-paced event, so the 2 s `service_onboard` cadence is responsive
/// without spinning.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(2);

// ───────────────────────────── wire contract ─────────────────────────────

/// Where the operator asked to provision — the shell's honest cloud-vs-local
/// choice. The wire carries only the discriminant; the daemon maps it to the
/// shared `do-lighthouse-join` defaults ([`SpawnTarget::default_cloud`] /
/// [`SpawnTarget::default_local`]) — the single source of truth for region/size
/// and vCPU/mem, so a front-end can't name an off-policy shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpawnTargetKind {
    /// A cloud droplet (`DigitalOcean`, the `zone1-do` `IaC`).
    Cloud,
    /// A local cloud-hypervisor VM on this host.
    Local,
}

impl SpawnTargetKind {
    /// Map the wire discriminant onto the shared-default [`SpawnTarget`].
    #[must_use]
    pub fn to_target(self) -> SpawnTarget {
        match self {
            Self::Cloud => SpawnTarget::default_cloud(),
            Self::Local => SpawnTarget::default_local(),
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
    /// Where to provision (cloud droplet vs local CH VM).
    pub target: SpawnTargetKind,
    /// Provision two lighthouses for quorum/HA (`false` ⇒ a single lighthouse).
    #[serde(default)]
    pub pair: bool,
    /// `true` ⇒ resolve + publish the plan only; the [`Provisioner`] seam is
    /// never touched.
    #[serde(default)]
    pub dry_run: bool,
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
    /// `true` for the honest retryable LAN-only outcome (no cloud token / no
    /// local virt / not founded) — the mesh keeps running and the operator
    /// retries after clearing the blocker.
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
/// vs local, the rendered spec + enroll bootstrap + ordered CA-migration steps,
/// or the honest LAN-only outcome), and — apply only — [`execute`] drives the
/// seam. A dry-run never touches the seam; a LAN-only plan short-circuits inside
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

/// Read new [`ACTION_TOPIC`] messages since `cursor`, advancing it. A short sync
/// open-read-drop (never crosses an `.await`), mirroring `service_onboard`. A
/// malformed action is dropped honestly with a warn.
fn read_new_actions(bus_root: &Path, cursor: &mut Option<String>) -> Vec<SpawnLighthouseAction> {
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
                tracing::warn!(ulid = %msg.ulid, error = %e, "spawn_lighthouse_onboard: bad spawn-lighthouse action");
            }
        }
    }
    out
}

/// Seed the cursor to the newest existing message so a (re)start doesn't
/// re-drive a historical spawn. `None` when the topic is empty.
fn prime_cursor(bus_root: &Path) -> Option<String> {
    let persist = Persist::open(bus_root.to_path_buf()).ok()?;
    let msgs = persist.list_since(ACTION_TOPIC, None).ok()?;
    msgs.last().map(|m| m.ulid.clone())
}

fn default_bus_root() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("mde").join("bus"))
}

/// The Bus-reachable `onboard spawn-lighthouse` worker. Leader-gated +
/// best-effort.
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
    /// The injectable publish seam (production: the shared [`BusPublisher`]).
    publisher: Box<dyn Publisher + Send + Sync>,
    /// Poll cadence.
    poll: Duration,
    /// Bus root override (tests). `None` ⇒ [`default_bus_root`].
    bus_root_override: Option<PathBuf>,
}

impl SpawnLighthouseOnboardWorker {
    /// Construct with production defaults: the honestly integration-gated
    /// [`LiveProvisioner`], the shared [`BusPublisher`], the shared leader lock
    /// under `workgroup_root`, and the default cadence.
    #[must_use]
    pub fn new(workgroup_root: PathBuf, node_id: String) -> Self {
        let leader_lock = workgroup_root.join(".mackesd-leader.lock");
        Self {
            workgroup_root,
            node_id,
            leader_lock,
            prov: Box::new(crate::onboard::spawn_lighthouse::LiveProvisioner::default()),
            publisher: Box::new(BusPublisher),
            poll: DEFAULT_POLL_INTERVAL,
            bus_root_override: None,
        }
    }

    /// Inject a provision seam (tests). Production uses [`LiveProvisioner`].
    #[must_use]
    pub fn with_provisioner(mut self, prov: Box<dyn Provisioner + Send + Sync>) -> Self {
        self.prov = prov;
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
        matches!(
            crate::leader::try_acquire(&self.leader_lock, &self.node_id),
            Ok(crate::leader::AcquireResult::Acquired)
        )
    }

    /// Drain new actions (advancing `cursor`) and — leader only — resolve each
    /// through the reused onboard engine and publish its typed result event.
    /// Non-leaders still advance their cursor (the `service_onboard`
    /// convention), so a node that later wins the election answers new requests,
    /// not backlog.
    fn drain_and_publish(&self, bus_root: &Path, cursor: &mut Option<String>) {
        let actions = read_new_actions(bus_root, cursor);
        if actions.is_empty() || !self.is_leader() {
            return;
        }
        // Gather once per tick — the founding bundle is the same for every
        // action in this batch.
        let facts = gather(&self.workgroup_root, &self.node_id);
        for action in actions {
            let event = resolve(&action, &facts, self.prov.as_ref());
            match serde_json::to_string(&event) {
                Ok(body) => self.publisher.publish(EVENT_TOPIC, &body),
                Err(e) => {
                    tracing::warn!(id = %event.id, error = %e, "spawn_lighthouse_onboard: event serialize failed");
                }
            }
        }
    }
}

#[async_trait::async_trait]
impl Worker for SpawnLighthouseOnboardWorker {
    fn name(&self) -> &'static str {
        "spawn_lighthouse_onboard"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let Some(bus_root) = self.bus_root() else {
            tracing::debug!("spawn_lighthouse_onboard: no bus root; worker idle");
            return Ok(());
        };
        // Prime past the backlog: a spawn is a one-shot verb — a restart must
        // not re-drive historical provisions (mirrors `service_onboard`).
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
    use crate::onboard::spawn_lighthouse::{
        CaMigrationStep, Endpoint, EnrollBootstrap, LiveProvisioner, ProvisionSpec,
    };
    use mde_bus::hooks::config::Priority;
    use std::sync::{Arc, Mutex};

    fn facts(
        cloud_token: bool,
        local_virt: bool,
        founded: bool,
    ) -> crate::onboard::spawn_lighthouse::SpawnFacts {
        crate::onboard::spawn_lighthouse::SpawnFacts {
            mesh_id: "home-deadbeef".to_string(),
            cloud_token_present: cloud_token,
            local_virt_ready: local_virt,
            ca_holder_overlay_ip: founded.then(|| "10.42.0.1".to_string()),
        }
    }

    fn action(target: SpawnTargetKind, pair: bool, dry_run: bool) -> SpawnLighthouseAction {
        SpawnLighthouseAction {
            id: "lh-test".to_string(),
            target,
            pair,
            dry_run,
        }
    }

    /// Recording [`Provisioner`] fake — pins that dry-runs and LAN-only plans
    /// never touch the seam, and that applies drive it in order.
    #[derive(Default)]
    struct FakeProvisioner {
        calls: Mutex<Vec<&'static str>>,
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
        };
        let json = serde_json::to_string(&a).expect("serialize");
        assert_eq!(
            json,
            r#"{"id":"lh-42-cloud","target":"cloud","pair":false,"dry_run":true}"#
        );
        assert_eq!(parse_action(&json).expect("parse"), a);
        // A minimal body omitting pair/dry_run defaults both to false.
        let m: SpawnLighthouseAction =
            parse_action(r#"{"id":"lh-1","target":"local"}"#).expect("minimal parse");
        assert_eq!(m.target, SpawnTargetKind::Local);
        assert!(!m.pair);
        assert!(!m.dry_run);
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
            &facts(true, false, true),
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
            &facts(true, false, true),
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
            &facts(false, false, true),
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

    #[test]
    fn dry_run_local_without_virt_is_lan_only() {
        let prov = FakeProvisioner::default();
        let ev = resolve(
            &action(SpawnTargetKind::Local, false, true),
            &facts(false, false, true),
            &prov,
        );
        assert!(ev.retry_available);
        assert!(ev.lan_only_hint.expect("hint").contains("cloud-hypervisor"));
    }

    // ── apply drives the seam / surfaces the typed error ──

    #[test]
    fn apply_drives_the_seam_in_order_and_reports_the_outcome() {
        let prov = FakeProvisioner::default();
        let ev = resolve(
            &action(SpawnTargetKind::Cloud, false, false),
            &facts(true, false, true),
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
            &facts(false, false, true),
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
            &facts(true, false, true),
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
        let dir = std::env::temp_dir().join(format!("mde-slo-{}-{}", now_ms(), actions.len()));
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
        // A dry-run cloud request drained off a real temp bus and answered on
        // EVENT_TOPIC with the echoed id (a fresh temp workgroup ⇒ this node wins
        // the leader lock; an un-founded workgroup ⇒ the honest LAN-only plan).
        let bus = seed_bus(&[SpawnLighthouseAction {
            id: "lh-77-cloud".to_string(),
            target: SpawnTargetKind::Cloud,
            pair: false,
            dry_run: true,
        }]);
        let wg = std::env::temp_dir().join(format!("mde-slo-wg-{}", now_ms()));
        std::fs::create_dir_all(&wg).expect("mk workgroup");
        let rec = RecordingPublisher::default();
        let log = rec.sent.clone();
        let w = SpawnLighthouseOnboardWorker::new(wg.clone(), "peer:a".to_string())
            .with_publisher(Box::new(rec))
            .with_bus_root(bus.clone());

        let mut cursor = None;
        w.drain_and_publish(&bus, &mut cursor);

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
        w.drain_and_publish(&bus, &mut cursor);
        assert_eq!(log.lock().expect("recorder mutex").len(), 1);

        let _ = std::fs::remove_dir_all(&bus);
        let _ = std::fs::remove_dir_all(&wg);
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
