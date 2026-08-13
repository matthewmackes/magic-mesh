//! OW-15 (target-side, day-2) — the `onboard_apply` worker: the §9-native
//! receiver for the [`BusApply`](crate::onboard::remote_push::BusApply) transport.
//!
//! An issuing node (running an onboard verb whose `LiveX` seam drives
//! [`RemotePush`](crate::onboard::remote_push::RemotePush)) publishes a **signed**
//! [`JobBundle`] on [`ACTION_TOPIC`]. This worker — running on the *target* — is
//! the trust boundary that decides whether to apply it. It reuses the pure core in
//! [`crate::onboard::remote_push`] verbatim (no re-implemented crypto): a bundle is
//! applied only when ALL of these hold, in this order:
//!
//! 1. **Addressed to me** — [`JobBundle::target_node`] equals this node's id (a
//!    node ignores a bundle meant for a different peer).
//! 2. **Leadership-authorized issuer** — the *claimed* issuer node-id resolves, in
//!    the CA `nodes` registry ([`crate::store::list_nodes`]), to a registered
//!    identity public key whose `role` is **leader-eligible** (the Nebula `host`
//!    group = a lighthouse). A workstation cannot push onboard actions (§8
//!    privileged issuer). [`authorize_issuer`] is the pure decision.
//! 3. **Signature + freshness + single-use** — [`process_apply`] verifies the
//!    detached signature against the resolved issuer key, rejects a stale/future
//!    or replayed bundle, then applies the allow-listed [`Action`]s via the
//!    injectable [`Applier`] (production [`LocalApplier`]).
//!
//! A failure at ANY step leaves the target **unchanged** (§7): resolution/verify
//! is fully upstream of apply, so an unauthorized / forged / stale bundle is a
//! clean no-op that publishes a typed rejection on [`EVENT_TOPIC`] — never a fake
//! apply. The observed-state echo carries only the **redacted** action
//! descriptions (§8), never secret material.
//!
//! **The action allow-list is the type system** ([`Action`]): there is no
//! "run arbitrary command" arm, so a forged bundle physically cannot request
//! anything outside it (§9 no-raw-shell for the day-2 path).
//!
//! # Scope (first cut)
//! `PinRole` + `SealSecret` — OW-11's day-2 Music path (pin the Media role + seal
//! `media-spaces` on a target lighthouse) — and `OpenBroker`, which carries the
//! complete session context and lands as the exact `SessionRequest::Open` Bus
//! message through [`LocalApplier`]. `RunEnroll` remains the bootstrap SSH step
//! and is honestly [`RemotePushError::NotWired`] here — never faked.

#![cfg(feature = "async-services")]

use std::path::{Path, PathBuf};
use std::time::Duration;

use ed25519_dalek::VerifyingKey;
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use serde::{Deserialize, Serialize};

use crate::onboard::remote_push::{
    process_apply, Applier, JobBundle, LocalApplier, NonceGuard, RemotePushError,
};

use super::scheduler::Publisher;
use super::{ShutdownToken, Worker};

/// Bus action topic this worker drains — the `action/<domain>/<verb>` convention
/// applied to the onboard family's day-2 apply verb (the [`BusApply`] transport
/// publishes here).
///
/// [`BusApply`]: crate::onboard::remote_push::BusApply
pub const ACTION_TOPIC: &str = "action/onboard/apply";

/// Bus event topic the typed observed-state (or a typed rejection) is published on
/// — the matching `event/<domain>/<verb>` lane the issuer tails for confirmation +
/// the §8 audit log records.
pub const EVENT_TOPIC: &str = "event/onboard/apply";

/// Poll cadence — an apply is a slow, operator-paced event; the 2 s `session_broker`
/// cadence is responsive without spinning.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Lower bound for retrying an unresolved, unopenable, or unsafe Bus.
const MIN_BUS_RETRY_INTERVAL: Duration = Duration::from_millis(10);

/// Upper bound for startup retry backoff.
const MAX_BUS_RETRY_INTERVAL: Duration = Duration::from_secs(2);

// ───────────────────────────── wire contract ─────────────────────────────

/// A signed-apply request drained off [`ACTION_TOPIC`] — the envelope the
/// [`BusApply`](crate::onboard::remote_push::BusApply) transport publishes.
///
/// The `issuer` is the *claimed* issuing node-id. It is deliberately safe outside
/// the signed [`JobBundle`]: the worker resolves the claimed issuer's identity key
/// and verifies `sig_hex` against it, so a forged issuer either fails the
/// signature (the attacker doesn't hold that key) or fails leadership
/// authorization (a workstation issuer is refused) — a bundle cannot be laundered
/// under a false issuer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyAction {
    /// The claimed issuing node's mesh id (resolved to its identity key in the
    /// `nodes` registry for signature + leadership checks).
    pub issuer: String,
    /// The signed bundle of allow-listed actions.
    pub bundle: JobBundle,
    /// Hex of the detached 64-byte Ed25519 signature over [`JobBundle::signing_bytes`].
    pub sig_hex: String,
}

/// Parse an [`ApplyAction`] body.
///
/// # Errors
/// A human-readable message on malformed JSON.
pub fn parse_action(body: &str) -> Result<ApplyAction, String> {
    serde_json::from_str(body).map_err(|e| format!("malformed onboard-apply action: {e}"))
}

/// The typed result published on [`EVENT_TOPIC`]: what the target actually did.
///
/// On success `error` is `None` and `applied` echoes the **redacted** (secret-free)
/// action descriptions (§8). On any rejection `applied` is empty and `error` names
/// the typed reason — the two are mutually exclusive so a reader never mistakes a
/// refusal for a partial apply (§7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyResultEvent {
    /// The claimed issuer, echoed for correlation.
    pub issuer: String,
    /// The node that applied (or refused) — this node's id.
    pub target: String,
    /// The signed bundle nonce, echoed so a BusApply caller can correlate the
    /// observed-state reply without trusting arrival order.
    ///
    /// `default` keeps older event records readable while all newly emitted
    /// events carry the nonce from the bundle.
    #[serde(default)]
    pub nonce: String,
    /// The redacted descriptions of the actions that applied, in order (empty on a
    /// rejection).
    #[serde(default)]
    pub applied: Vec<String>,
    /// The typed rejection reason when nothing applied (`None` on success).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ApplyResultEvent {
    fn rejected(action: &ApplyAction, target: &str, why: String) -> Self {
        Self {
            issuer: action.issuer.clone(),
            target: target.to_string(),
            nonce: action.bundle.nonce.clone(),
            applied: Vec::new(),
            error: Some(why),
        }
    }
}

// ─────────────────────── issuer resolution + authorization ───────────────────────

/// The `(pubkey-hex, role)` projection [`authorize_issuer`] needs from the CA
/// `nodes` registry — the identity key the enrollment recorded + the Nebula group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuerRow {
    /// The registered node-id.
    pub node_id: String,
    /// The node's Ed25519 identity public key, hex-encoded (as the `nodes` table
    /// stores it).
    pub public_key_hex: String,
    /// The Nebula group / role (`host` | `peer` | `observer` | `decommissioned`).
    pub role: String,
}

/// Leader-eligible = the Nebula `host` group (a lighthouse / CA-holding node). Only
/// such a node may push onboard actions to a peer (§8 privileged issuer).
#[must_use]
pub fn role_is_leader_eligible(role: &str) -> bool {
    role == "host"
}

/// Pure authorization: resolve the *claimed* `issuer` to its identity verifying key
/// **iff** it is registered AND leader-eligible.
///
/// # Errors
/// [`RemotePushError::BundleRejected`] when the issuer is unknown, not
/// leader-eligible, or has a malformed identity key — the target is left unchanged
/// (this runs before any apply).
pub fn authorize_issuer(rows: &[IssuerRow], issuer: &str) -> Result<VerifyingKey, RemotePushError> {
    let row = rows.iter().find(|r| r.node_id == issuer).ok_or_else(|| {
        RemotePushError::BundleRejected {
            why: format!("unknown issuer `{issuer}` (not in the nodes registry)"),
        }
    })?;
    if !role_is_leader_eligible(&row.role) {
        return Err(RemotePushError::BundleRejected {
            why: format!(
                "issuer `{issuer}` role `{}` is not leader-eligible — only a lighthouse may push \
                 onboard actions",
                row.role
            ),
        });
    }
    let bytes =
        hex_to_array::<32>(&row.public_key_hex).ok_or_else(|| RemotePushError::BundleRejected {
            why: format!("issuer `{issuer}` has a malformed identity public key"),
        })?;
    VerifyingKey::from_bytes(&bytes).map_err(|e| RemotePushError::BundleRejected {
        why: format!("issuer `{issuer}` identity key does not parse: {e}"),
    })
}

/// Decode a `2*N`-hex string into `[u8; N]`; `None` on a bad length or non-hex
/// digit.
fn hex_to_array<const N: usize>(s: &str) -> Option<[u8; N]> {
    if s.len() != N * 2 {
        return None;
    }
    let mut out = [0u8; N];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

/// The issuer-key resolver seam — production reads the CA `nodes` registry; tests
/// inject a fixed set of [`IssuerRow`]s.
pub trait IssuerResolver {
    /// Resolve `issuer` to its identity key iff registered + leader-eligible.
    ///
    /// # Errors
    /// [`RemotePushError::BundleRejected`] (unknown / not leader-eligible / bad key).
    fn resolve(&self, issuer: &str) -> Result<VerifyingKey, RemotePushError>;
}

/// Production [`IssuerResolver`] — resolves against the mackesd `nodes` registry
/// (mapping each `node_id` to its `public_key` + `role`, populated at enrollment).
pub struct StoreIssuerResolver {
    /// The mackesd store db path (`crate::default_db_path` in production).
    db_path: PathBuf,
}

impl StoreIssuerResolver {
    /// Resolve issuers against the mackesd store at `db_path` (production:
    /// [`crate::default_db_path`]).
    #[must_use]
    pub const fn new(db_path: PathBuf) -> Self {
        Self { db_path }
    }
}

impl IssuerResolver for StoreIssuerResolver {
    fn resolve(&self, issuer: &str) -> Result<VerifyingKey, RemotePushError> {
        let conn =
            crate::store::open(&self.db_path).map_err(|e| RemotePushError::BundleRejected {
                why: format!("nodes registry unavailable: {e}"),
            })?;
        let rows = crate::store::list_nodes(&conn)
            .map_err(|e| RemotePushError::BundleRejected {
                why: format!("nodes registry query failed: {e}"),
            })?
            .into_iter()
            .map(|n| IssuerRow {
                node_id: n.node_id,
                public_key_hex: n.public_key,
                role: n.role,
            })
            .collect::<Vec<_>>();
        authorize_issuer(&rows, issuer)
    }
}

// ───────────────────────────── pure: resolve ─────────────────────────────

/// Pure orchestration of one drained [`ApplyAction`] into the one
/// [`ApplyResultEvent`] to publish.
///
/// Applies the bundle iff it is addressed here, from a leadership-authorized
/// issuer, and validly signed/fresh/single-use.
///
/// The order is security-load-bearing (each earlier gate leaves the target fully
/// unchanged): addressed-to-me → signature decode → issuer resolution + leadership
/// authorization → [`process_apply`] (verify + nonce + allow-listed apply).
#[must_use]
pub fn resolve(
    action: &ApplyAction,
    self_node_id: &str,
    resolver: &dyn IssuerResolver,
    now: i64,
    nonce_guard: &mut NonceGuard,
    applier: &dyn Applier,
) -> ApplyResultEvent {
    if action.bundle.target_node != self_node_id {
        return ApplyResultEvent::rejected(
            action,
            self_node_id,
            format!(
                "bundle targets `{}`, not this node `{self_node_id}`",
                action.bundle.target_node
            ),
        );
    }
    let Some(sig) = hex_to_array::<64>(&action.sig_hex) else {
        return ApplyResultEvent::rejected(
            action,
            self_node_id,
            "signature is not 64 hex-encoded bytes".to_string(),
        );
    };
    let signer = match resolver.resolve(&action.issuer) {
        Ok(vk) => vk,
        Err(e) => return ApplyResultEvent::rejected(action, self_node_id, e.to_string()),
    };
    match process_apply(&action.bundle, &sig, &signer, now, nonce_guard, applier) {
        Ok(outcome) => ApplyResultEvent {
            issuer: action.issuer.clone(),
            target: self_node_id.to_string(),
            nonce: action.bundle.nonce.clone(),
            applied: outcome.applied,
            error: None,
        },
        Err(e) => ApplyResultEvent::rejected(action, self_node_id, e.to_string()),
    }
}

// ─────────────────────────── bus + worker ───────────────────────────

fn read_new_actions(
    persist: &Persist,
    cursor: Option<&str>,
) -> Result<(Option<String>, Vec<ApplyAction>), String> {
    let msgs = persist
        .list_since(ACTION_TOPIC, cursor)
        .map_err(|error| format!("read {ACTION_TOPIC}: {error}"))?;
    let next_cursor = msgs
        .last()
        .map(|msg| msg.ulid.clone())
        .or_else(|| cursor.map(str::to_string));
    let mut out = Vec::new();
    for msg in msgs {
        let body = msg.body.as_deref().unwrap_or("");
        match parse_action(body) {
            Ok(a) => out.push(a),
            Err(e) => {
                tracing::warn!(ulid = %msg.ulid, error = %e, "onboard_apply: bad apply action");
            }
        }
    }
    Ok((next_cursor, out))
}

/// Seed the cursor to the newest existing message so a (re)start doesn't re-drive a
/// historical apply. `None` when the topic is empty.
fn prime_cursor(persist: &Persist) -> Result<Option<String>, String> {
    persist
        .latest_ulid(ACTION_TOPIC)
        .map_err(|error| format!("prime {ACTION_TOPIC}: {error}"))
}

fn onboard_apply_bus_root(override_root: Option<PathBuf>) -> PathBuf {
    onboard_apply_bus_root_or_system(override_root.or_else(mde_bus::default_data_dir))
}

fn onboard_apply_bus_root_or_system(resolved: Option<PathBuf>) -> PathBuf {
    resolved.unwrap_or_else(|| PathBuf::from(mde_bus::SYSTEM_BUS_ROOT))
}

fn next_bus_retry_interval(current: Duration) -> Duration {
    current
        .saturating_mul(2)
        .clamp(MIN_BUS_RETRY_INTERVAL, MAX_BUS_RETRY_INTERVAL)
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0)
}

#[cfg(test)]
type BusOpenFn = dyn Fn(&Path) -> Result<Option<Persist>, String> + Send + Sync;

#[cfg(test)]
type CursorPrimeFn = dyn Fn(&Persist) -> Result<Option<String>, String> + Send + Sync;

#[cfg(test)]
type ActionReadGateFn = dyn Fn() -> Result<(), String> + Send + Sync;

/// The Bus-reachable target-side apply worker.
///
/// Runs on EVERY node (rank-0 default): any enrolled peer can be the target of a
/// day-2 push, and each node only applies bundles addressed to it. Best-effort +
/// honestly gated where the effect isn't node-local.
pub struct OnboardApplyWorker {
    /// This node's id — the target filter + the observed-state `target`.
    node_id: String,
    /// The injectable issuer resolver (production: [`StoreIssuerResolver`]).
    resolver: Box<dyn IssuerResolver + Send + Sync>,
    /// The injectable node-local applier (production: [`LocalApplier`]).
    applier: Box<dyn Applier + Send + Sync>,
    /// Optional injected publish seam. Production writes event history through
    /// the already-open recovered [`Persist`] handle.
    publisher: Option<Box<dyn Publisher + Send + Sync>>,
    /// The single-use nonce guard, held across ticks (replay defense).
    nonce_guard: NonceGuard,
    /// Poll cadence.
    poll: Duration,
    /// Bus root override (tests). `None` resolves the user Bus, then system spool.
    bus_root_override: Option<PathBuf>,
    #[cfg(test)]
    bus_open_override: Option<std::sync::Arc<BusOpenFn>>,
    #[cfg(test)]
    cursor_prime_override: Option<std::sync::Arc<CursorPrimeFn>>,
    #[cfg(test)]
    action_read_gate: Option<std::sync::Arc<ActionReadGateFn>>,
}

impl OnboardApplyWorker {
    /// Construct with production defaults: the store-backed issuer resolver, the
    /// [`LocalApplier`] resolved from the deployed repo + `workgroup_root`, the
    /// recovered Bus handle for event history, and the default cadence.
    #[must_use]
    pub fn new(workgroup_root: &Path, node_id: String) -> Self {
        let applier = LocalApplier::resolve(&crate::ipc::secret_store::repo_root(), workgroup_root);
        Self {
            node_id,
            resolver: Box::new(StoreIssuerResolver::new(crate::default_db_path())),
            applier: Box::new(applier),
            publisher: None,
            nonce_guard: NonceGuard::new(),
            poll: DEFAULT_POLL_INTERVAL,
            bus_root_override: None,
            #[cfg(test)]
            bus_open_override: None,
            #[cfg(test)]
            cursor_prime_override: None,
            #[cfg(test)]
            action_read_gate: None,
        }
    }

    /// Inject an issuer resolver (tests).
    #[must_use]
    pub fn with_resolver(mut self, resolver: Box<dyn IssuerResolver + Send + Sync>) -> Self {
        self.resolver = resolver;
        self
    }

    /// Inject an applier (tests).
    #[must_use]
    pub fn with_applier(mut self, applier: Box<dyn Applier + Send + Sync>) -> Self {
        self.applier = applier;
        self
    }

    /// Inject a publisher (tests).
    #[must_use]
    pub fn with_publisher(mut self, publisher: Box<dyn Publisher + Send + Sync>) -> Self {
        self.publisher = Some(publisher);
        self
    }

    /// Override the poll cadence (tests).
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
    fn with_bus_opener(mut self, open: std::sync::Arc<BusOpenFn>) -> Self {
        self.bus_open_override = Some(open);
        self
    }

    #[cfg(test)]
    #[must_use]
    fn with_cursor_primer(mut self, prime: std::sync::Arc<CursorPrimeFn>) -> Self {
        self.cursor_prime_override = Some(prime);
        self
    }

    #[cfg(test)]
    #[must_use]
    fn with_action_read_gate(mut self, gate: std::sync::Arc<ActionReadGateFn>) -> Self {
        self.action_read_gate = Some(gate);
        self
    }

    fn bus_root(&self) -> PathBuf {
        onboard_apply_bus_root(self.bus_root_override.clone())
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

    /// Drain new apply actions (advancing `cursor`), resolve each through the pure
    /// core, and publish its typed result event. NOT leader-gated: the target
    /// filter is `target_node == self.node_id`, so exactly the addressed node
    /// answers.
    fn drain_and_publish(&mut self, persist: &Persist, cursor: &mut Option<String>) -> bool {
        #[cfg(test)]
        if let Some(gate) = self.action_read_gate.as_ref() {
            if let Err(error) = gate() {
                tracing::debug!(target: "mackesd::onboard_apply", %error, "injected action read failure");
                return false;
            }
        }
        let (next_cursor, actions) = match read_new_actions(persist, cursor.as_deref()) {
            Ok(read) => read,
            Err(error) => {
                tracing::debug!(target: "mackesd::onboard_apply", %error, "action read failed; effect sweep deferred");
                return false;
            }
        };
        // Install the staged cursor only after the complete command-lane read.
        *cursor = next_cursor;
        for action in actions {
            let now = now_unix();
            let event = resolve(
                &action,
                &self.node_id,
                self.resolver.as_ref(),
                now,
                &mut self.nonce_guard,
                self.applier.as_ref(),
            );
            match serde_json::to_string(&event) {
                Ok(body) => {
                    if let Some(publisher) = self.publisher.as_ref() {
                        publisher.publish(EVENT_TOPIC, &body);
                    } else if let Err(error) =
                        persist.write(EVENT_TOPIC, Priority::Default, None, Some(&body))
                    {
                        tracing::warn!(target: "mackesd::onboard_apply", %error, "event history publish failed");
                    }
                }
                Err(e) => {
                    tracing::warn!(issuer = %event.issuer, error = %e, "onboard_apply: event serialize failed");
                }
            }
        }
        true
    }
}

#[async_trait::async_trait]
impl Worker for OnboardApplyWorker {
    fn name(&self) -> &'static str {
        "onboard_apply"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let bus_root = self.bus_root();
        let mut retry_interval = MIN_BUS_RETRY_INTERVAL;
        let (persist, mut cursor) = loop {
            match self.open_bus(&bus_root) {
                Ok(Some(persist)) => match self.prime_action_cursor(&persist) {
                    Ok(cursor) => break (persist, cursor),
                    Err(error) => tracing::warn!(
                        target: "mackesd::onboard_apply",
                        %error,
                        "action-tail activation failed; startup will retry"
                    ),
                },
                Ok(None) => tracing::debug!(
                    target: "mackesd::onboard_apply",
                    "Bus root unavailable; startup will retry"
                ),
                Err(error) => tracing::warn!(
                    target: "mackesd::onboard_apply",
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
                _ = tick.tick() => { self.drain_and_publish(&persist, &mut cursor); }
                () = shutdown.wait() => break,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::onboard::remote_push::Action;
    use ed25519_dalek::SigningKey;
    use mde_bus::hooks::config::Priority;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    fn key() -> SigningKey {
        SigningKey::from_bytes(&[9_u8; 32])
    }

    fn to_hex(bytes: &[u8]) -> String {
        use std::fmt::Write as _;
        bytes.iter().fold(String::new(), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
    }

    fn pubkey_hex(k: &SigningKey) -> String {
        to_hex(k.verifying_key().as_bytes())
    }

    fn sig_hex(k: &SigningKey, bundle: &JobBundle) -> String {
        to_hex(&bundle.sign(k))
    }

    fn bundle(now: i64) -> JobBundle {
        JobBundle {
            target_node: "peer:me".into(),
            actions: vec![
                Action::PinRole {
                    role: "lighthouse".into(),
                    media: false,
                },
                Action::SealSecret {
                    name: "node-config".into(),
                    secret: "s3-creds".into(),
                },
            ],
            issued_at: now,
            nonce: "nonce-1".into(),
        }
    }

    /// A resolver that returns a fixed key for a fixed leader-eligible issuer, and
    /// rejects everything else (the production path's shape without a DB).
    struct FixedResolver {
        issuer: String,
        vk: VerifyingKey,
    }
    impl IssuerResolver for FixedResolver {
        fn resolve(&self, issuer: &str) -> Result<VerifyingKey, RemotePushError> {
            if issuer == self.issuer {
                Ok(self.vk)
            } else {
                Err(RemotePushError::BundleRejected {
                    why: format!("unknown issuer `{issuer}`"),
                })
            }
        }
    }

    /// A recording applier — proves what landed without a real node-local effect.
    #[derive(Default)]
    struct RecordingApplier {
        applied: Mutex<Vec<String>>,
    }
    impl Applier for RecordingApplier {
        fn apply_one(&self, action: &Action) -> Result<(), RemotePushError> {
            self.applied
                .lock()
                .expect("applied mutex")
                .push(action.redacted());
            Ok(())
        }
    }

    // ── authorize_issuer: the leadership gate ──

    #[test]
    fn authorize_accepts_a_leader_eligible_issuer() {
        let k = key();
        let rows = vec![IssuerRow {
            node_id: "peer:lh".into(),
            public_key_hex: pubkey_hex(&k),
            role: "host".into(),
        }];
        let vk = authorize_issuer(&rows, "peer:lh").expect("host issuer authorized");
        assert_eq!(vk.as_bytes(), k.verifying_key().as_bytes());
    }

    #[test]
    fn authorize_refuses_a_non_leader_role() {
        // A workstation (`peer`) cannot push onboard actions (§8).
        let k = key();
        let rows = vec![IssuerRow {
            node_id: "peer:ws".into(),
            public_key_hex: pubkey_hex(&k),
            role: "peer".into(),
        }];
        assert!(matches!(
            authorize_issuer(&rows, "peer:ws"),
            Err(RemotePushError::BundleRejected { .. })
        ));
    }

    #[test]
    fn authorize_refuses_an_unknown_issuer() {
        assert!(matches!(
            authorize_issuer(&[], "peer:ghost"),
            Err(RemotePushError::BundleRejected { .. })
        ));
    }

    #[test]
    fn authorize_refuses_a_malformed_pubkey() {
        let rows = vec![IssuerRow {
            node_id: "peer:lh".into(),
            public_key_hex: "not-hex".into(),
            role: "host".into(),
        }];
        assert!(matches!(
            authorize_issuer(&rows, "peer:lh"),
            Err(RemotePushError::BundleRejected { .. })
        ));
    }

    // ── resolve: the full validate → apply core ──

    fn resolver(k: &SigningKey) -> FixedResolver {
        FixedResolver {
            issuer: "peer:lh".into(),
            vk: k.verifying_key(),
        }
    }

    #[test]
    fn resolve_applies_a_valid_authorized_signed_bundle() {
        let k = key();
        let now = 1_800_000_000;
        let b = bundle(now);
        let action = ApplyAction {
            issuer: "peer:lh".into(),
            bundle: b.clone(),
            sig_hex: sig_hex(&k, &b),
        };
        let app = RecordingApplier::default();
        let mut guard = NonceGuard::new();
        let ev = resolve(&action, "peer:me", &resolver(&k), now, &mut guard, &app);
        assert!(
            ev.error.is_none(),
            "authorized+valid bundle applies: {ev:?}"
        );
        assert_eq!(ev.nonce, b.nonce, "success echoes the signed bundle nonce");
        assert_eq!(ev.applied.len(), 2);
        assert!(ev.applied[0].contains("pin-role lighthouse"));
        // The secret material never appears in the observed-state echo (§8).
        assert!(!ev.applied.iter().any(|a| a.contains("s3-creds")));
    }

    #[test]
    fn resolve_refuses_a_bundle_for_a_different_target() {
        let k = key();
        let now = 1_800_000_000;
        let b = bundle(now); // target_node = peer:me
        let action = ApplyAction {
            issuer: "peer:lh".into(),
            bundle: b.clone(),
            sig_hex: sig_hex(&k, &b),
        };
        let app = RecordingApplier::default();
        let mut guard = NonceGuard::new();
        // this node is a DIFFERENT peer ⇒ ignored, nothing applied
        let ev = resolve(&action, "peer:other", &resolver(&k), now, &mut guard, &app);
        assert!(ev.error.is_some());
        assert_eq!(
            ev.nonce, b.nonce,
            "rejection echoes the signed bundle nonce"
        );
        assert!(app.applied.lock().expect("mutex").is_empty());
    }

    #[test]
    fn resolve_refuses_an_unauthorized_issuer_and_applies_nothing() {
        // A signature that verifies against the CLAIMED issuer's key, but the
        // resolver refuses the issuer (unknown / not leader-eligible) ⇒ no apply.
        let k = key();
        let now = 1_800_000_000;
        let b = bundle(now);
        let action = ApplyAction {
            issuer: "peer:rogue".into(),
            bundle: b.clone(),
            sig_hex: sig_hex(&k, &b),
        };
        let app = RecordingApplier::default();
        let mut guard = NonceGuard::new();
        let ev = resolve(&action, "peer:me", &resolver(&k), now, &mut guard, &app);
        assert!(ev.error.is_some());
        assert!(
            app.applied.lock().expect("mutex").is_empty(),
            "an unauthorized issuer leaves the target unchanged"
        );
    }

    #[test]
    fn resolve_refuses_a_forged_signature_and_applies_nothing() {
        // Correct authorized issuer id, but the bundle is signed by a DIFFERENT
        // key (forgery) ⇒ signature fails ⇒ no apply, target unchanged.
        let k = key();
        let wrong = SigningKey::from_bytes(&[1_u8; 32]);
        let now = 1_800_000_000;
        let b = bundle(now);
        let action = ApplyAction {
            issuer: "peer:lh".into(),
            bundle: b.clone(),
            sig_hex: sig_hex(&wrong, &b),
        };
        let app = RecordingApplier::default();
        let mut guard = NonceGuard::new();
        let ev = resolve(&action, "peer:me", &resolver(&k), now, &mut guard, &app);
        assert!(ev.error.is_some());
        assert!(app.applied.lock().expect("mutex").is_empty());
    }

    // ── the worker: drain → resolve → publish ──

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

    fn seed_bus(actions: &[ApplyAction]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mde-oa-{}-{}", now_ms(), actions.len()));
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
    fn worker_drains_an_apply_and_publishes_the_observed_state() {
        let k = key();
        let now = now_unix();
        let b = JobBundle {
            issued_at: now,
            ..bundle(now)
        };
        let action = ApplyAction {
            issuer: "peer:lh".into(),
            bundle: b.clone(),
            sig_hex: sig_hex(&k, &b),
        };
        let bus = seed_bus(&[action]);
        let rec = RecordingPublisher::default();
        let log = rec.sent.clone();
        let mut w = OnboardApplyWorker::new(&std::env::temp_dir(), "peer:me".to_string())
            .with_resolver(Box::new(resolver(&k)))
            .with_applier(Box::new(RecordingApplier::default()))
            .with_publisher(Box::new(rec))
            .with_bus_root(bus.clone());

        let persist = Persist::open(bus.clone()).expect("reopen bus");
        let mut cursor = None;
        assert!(w.drain_and_publish(&persist, &mut cursor));

        let sent = log.lock().expect("recorder mutex");
        assert_eq!(sent.len(), 1, "one request ⇒ one event");
        assert_eq!(sent[0].0, EVENT_TOPIC);
        let ev: ApplyResultEvent = serde_json::from_str(&sent[0].1).expect("event parses");
        assert_eq!(ev.target, "peer:me");
        assert_eq!(ev.nonce, b.nonce, "worker event carries the bundle nonce");
        assert!(ev.error.is_none(), "authorized valid apply: {ev:?}");
        assert_eq!(ev.applied.len(), 2);
        drop(sent);

        // The cursor advanced — a second drain re-answers nothing.
        assert!(w.drain_and_publish(&persist, &mut cursor));
        assert_eq!(log.lock().expect("recorder mutex").len(), 1);
        let _ = std::fs::remove_dir_all(&bus);
    }

    #[test]
    fn result_event_reads_legacy_payload_without_a_nonce() {
        let event: ApplyResultEvent = serde_json::from_str(
            r#"{"issuer":"peer:lh","target":"peer:me","applied":[],"error":null}"#,
        )
        .expect("legacy event parses");
        assert!(
            event.nonce.is_empty(),
            "missing legacy nonce uses the default"
        );
    }

    #[test]
    fn service_bus_root_honors_override_and_falls_back_to_system_spool() {
        let root = tempfile::tempdir().expect("temporary bus root");
        let expected = root.path().to_path_buf();
        let previous = std::env::var_os("MDE_BUS_ROOT");
        let got = {
            std::env::set_var("MDE_BUS_ROOT", &expected);
            let got = (
                onboard_apply_bus_root(None),
                OnboardApplyWorker::new(&std::env::temp_dir(), "peer:me".to_string()).bus_root(),
            );
            match previous {
                Some(value) => std::env::set_var("MDE_BUS_ROOT", value),
                None => std::env::remove_var("MDE_BUS_ROOT"),
            }
            got
        };

        assert_eq!(got.0, expected.clone());
        assert_eq!(got.1, expected);
        assert_eq!(
            onboard_apply_bus_root_or_system(None),
            PathBuf::from(mde_bus::SYSTEM_BUS_ROOT)
        );
    }

    #[tokio::test]
    async fn late_bus_recovers_without_replay_and_defers_failed_reads() {
        struct CountingApplier(Arc<AtomicUsize>);
        impl Applier for CountingApplier {
            fn apply_one(&self, _action: &Action) -> Result<(), RemotePushError> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }

        let root = tempfile::tempdir().expect("temporary root");
        let bus_root = root.path().join("bus");
        let persist = Persist::open(bus_root.clone()).expect("prepare delayed Bus");
        let signing_key = key();
        let now = now_unix();

        let mut stale_bundle = bundle(now);
        stale_bundle.nonce = "onboard-apply-startup-stale".into();
        let stale = ApplyAction {
            issuer: "peer:lh".into(),
            sig_hex: sig_hex(&signing_key, &stale_bundle),
            bundle: stale_bundle,
        };
        persist
            .write(
                ACTION_TOPIC,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&stale).unwrap()),
            )
            .expect("write retained apply command");

        let applied = Arc::new(AtomicUsize::new(0));
        let recorder = RecordingPublisher::default();
        let published = Arc::clone(&recorder.sent);
        let open_attempts = Arc::new(AtomicUsize::new(0));
        let open_attempts_for_worker = Arc::clone(&open_attempts);
        let bus_root_for_worker = bus_root.clone();
        let prime_attempts = Arc::new(AtomicUsize::new(0));
        let prime_attempts_for_worker = Arc::clone(&prime_attempts);
        let fail_reads = Arc::new(AtomicBool::new(false));
        let fail_reads_for_worker = Arc::clone(&fail_reads);
        let mut worker = OnboardApplyWorker::new(root.path(), "peer:me".into())
            .with_resolver(Box::new(resolver(&signing_key)))
            .with_applier(Box::new(CountingApplier(Arc::clone(&applied))))
            .with_publisher(Box::new(recorder))
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
                    return Err("injected action-tail read failure".into());
                }
                prime_cursor(persist)
            }))
            .with_action_read_gate(Arc::new(move || {
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
        assert_eq!(applied.load(Ordering::SeqCst), 0);
        assert!(published.lock().unwrap().is_empty());

        fail_reads.store(true, Ordering::SeqCst);
        let mut forward_bundle = bundle(now);
        forward_bundle.nonce = "onboard-apply-forward".into();
        let forward = ApplyAction {
            issuer: "peer:lh".into(),
            sig_hex: sig_hex(&signing_key, &forward_bundle),
            bundle: forward_bundle,
        };
        persist
            .write(
                ACTION_TOPIC,
                Priority::Default,
                None,
                Some(&serde_json::to_string(&forward).unwrap()),
            )
            .expect("write post-activation command");
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(
            applied.load(Ordering::SeqCst),
            0,
            "failed command read must defer the complete effect sweep"
        );
        assert!(published.lock().unwrap().is_empty());

        fail_reads.store(false, Ordering::SeqCst);
        tokio::time::timeout(Duration::from_secs(3), async {
            while applied.load(Ordering::SeqCst) < 2 {
                assert!(!task.is_finished(), "worker exited before forward apply");
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("post-activation command must execute after a complete read");
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(applied.load(Ordering::SeqCst), 2);
        let event: ApplyResultEvent = {
            let events = published.lock().unwrap();
            assert_eq!(
                events.len(),
                1,
                "outage-delayed command emits one history event"
            );
            serde_json::from_str(&events[0].1).unwrap()
        };
        assert_eq!(event.nonce, "onboard-apply-forward");

        shutdown_tx.send(true).expect("request shutdown");
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .expect("shutdown timed out")
            .expect("worker task panicked")
            .expect("worker returned an error");
    }

    #[tokio::test]
    async fn run_loop_exits_promptly_on_shutdown() {
        let bus = std::env::temp_dir().join(format!("mde-oa-run-{}", now_ms()));
        let k = key();
        let (tx, rx) = tokio::sync::watch::channel(false);
        let mut w = OnboardApplyWorker::new(&std::env::temp_dir(), "peer:me".to_string())
            .with_resolver(Box::new(resolver(&k)))
            .with_bus_root(bus.clone())
            .with_poll(Duration::from_millis(10));
        let token = ShutdownToken::from_receiver(rx);
        let handle = tokio::spawn(async move { w.run(token).await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        tx.send(true).expect("signal shutdown");
        let joined = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(joined.is_ok(), "worker must exit promptly on shutdown");
        assert!(joined.unwrap().expect("join").is_ok());
    }
}
