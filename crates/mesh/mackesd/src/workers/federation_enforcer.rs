//! WL-SEC-002 — the `federation_enforcer` worker: the RUNTIME consumer that makes
//! cross-mesh federation grants actually enforce.
//!
//! The federation CONFIG lifecycle (`mde-bus federation` mint/accept/revoke/rotate)
//! writes grants into `<bus_root>/federation.yaml`, but nothing read them at runtime
//! — the grants were inert. This worker closes that:
//!
//!  1. **Grant-gated routing / ingress identity check.** Each tick it drains the
//!     cross-mesh ingress spool ([`mde_bus::federation::ingress_dir`]) — where the
//!     live Nebula cross-mesh bridge drops foreign-mesh messages — and runs every
//!     envelope through [`mde_bus::federation::FederationGrants::decide`] (Ingress).
//!     A message is forwarded onto the LOCAL bus IFF its ORIGIN mesh has an accepted,
//!     non-revoked pair AND the topic is granted AND not excluded. Everything else —
//!     an unaccepted mesh, a revoked mesh, an excluded/ungranted topic — is DROPPED
//!     and audited on `federation/refused/<peer>`. **Default DENY.**
//!
//!  2. **GUI-driven accept / revoke.** It drains `action/federation/{accept,revoke,
//!     refuse-mint}` (the shell Federation panel's publishes) and runs the privileged
//!     acts as root: `accept` consumes the single-use mint, writes the pair, and
//!     INSTALLS the cross-mesh Nebula trust cert; `revoke` removes the pair and
//!     DELETES the cert; `refuse-mint` cancels a pending outbound offer.
//!
//!  3. **Status mirror.** It publishes `state/federation/<node>` (accepted pairs +
//!     pending mints) so the shell panel can render pending offers + accepted meshes
//!     without the desktop tier reading the root-owned `federation.yaml` (§6).
//!
//! Universal (rank 0): a Lighthouse RELAYS cross-mesh traffic, so it especially must
//! enforce the boundary; a Workstation enforces its own ingress too. Runs everywhere.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use mde_bus::federation::{
    accept_passcode, ingress_dir, mint_path, read_grants, revoke_pair, trust_dir,
    CrossMeshEnvelope, Decision, Direction, FederationGrants, MintEnvelope,
};
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;

use crate::ipc::action_auth::{ActionAuthorizer, MutationContext};

use super::{ShutdownToken, Worker};

/// The shell Federation panel's accept request lane (`{passcode, label?, peer-ca-pem?}`).
const ACCEPT_TOPIC: &str = "action/federation/accept";
/// The shell Federation panel's revoke request lane (`{peer-mesh-id}`).
const REVOKE_TOPIC: &str = "action/federation/revoke";
/// The shell Federation panel's cancel-a-pending-offer lane (`{ulid}`).
const REFUSE_MINT_TOPIC: &str = "action/federation/refuse-mint";
/// The retained per-node enforcement status mirror the shell reads.
const STATUS_PREFIX: &str = "state/federation/";
/// The audit lane a refused cross-mesh ingress message lands on.
const REFUSED_PREFIX: &str = "federation/refused/";
/// The audit lane an accepted (forwarded) cross-mesh ingress message lands on.
const INGRESS_OK_PREFIX: &str = "federation/ingress-accepted/";

/// Closed capability scope for the federation action consumer. The caller
/// cannot select a node; this worker always mutates this node's federation
/// state and trust directory.
const FEDERATION_ACTION_NODE_SCOPE: &str = "federation";

/// Default enforcement cadence — the ingress spool + action lanes are drained this
/// often. Cheap local reads, so a tight cadence keeps a foreign-mesh message from
/// lingering unenforced.
const DEFAULT_POLL: Duration = Duration::from_secs(2);

/// The WL-SEC-002 federation runtime-enforcement worker.
pub struct FederationEnforcerWorker {
    node_id: String,
    authorizer: Arc<ActionAuthorizer>,
    bus_root_override: Option<PathBuf>,
    trust_dir_override: Option<PathBuf>,
    poll_interval: Duration,
    // ── runtime cursors + last-published status (mutated in `tick_once`) ──
    accept_cursor: Option<String>,
    revoke_cursor: Option<String>,
    refuse_mint_cursor: Option<String>,
    last_status: Option<String>,
}

impl FederationEnforcerWorker {
    /// Construct the worker for `node_id` (the retained-status mirror key).
    #[must_use]
    pub fn new(node_id: String) -> Self {
        Self {
            node_id,
            authorizer: Arc::new(ActionAuthorizer::production()),
            bus_root_override: None,
            trust_dir_override: None,
            poll_interval: DEFAULT_POLL,
            accept_cursor: None,
            revoke_cursor: None,
            refuse_mint_cursor: None,
            last_status: None,
        }
    }

    /// Override the Bus root (tests point it at a tempdir Persist).
    #[must_use]
    pub fn with_bus_root(mut self, p: PathBuf) -> Self {
        self.bus_root_override = Some(p);
        self
    }

    /// Override the cross-mesh trust-cert dir (tests point it at a tempdir).
    #[must_use]
    pub fn with_trust_dir(mut self, p: PathBuf) -> Self {
        self.trust_dir_override = Some(p);
        self
    }

    /// Inject an isolated verifier and replay ledger for hostile action tests.
    /// Production always uses the systemd-credential-backed authorizer.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_authorizer(mut self, authorizer: Arc<ActionAuthorizer>) -> Self {
        self.authorizer = authorizer;
        self
    }

    /// Override the poll cadence (tests use a short interval).
    #[must_use]
    pub fn with_poll_interval(mut self, d: Duration) -> Self {
        self.poll_interval = d;
        self
    }

    fn resolved_trust_dir(&self) -> PathBuf {
        self.trust_dir_override.clone().unwrap_or_else(trust_dir)
    }

    /// One enforcement pass. Ordered so a just-accepted pair enforces on the SAME
    /// tick's ingress drain:
    ///
    ///  1. drain the GUI accept/revoke/refuse-mint actions (may mutate grants + cert),
    ///  2. reload the grants from disk,
    ///  3. drain the cross-mesh ingress spool through the (reloaded) gate,
    ///  4. publish the status mirror (only on change).
    pub fn tick_once(&mut self, persist: &Persist, bus_root: &Path, trust: &Path) {
        self.drain_actions(persist, bus_root, trust);
        let grants = read_grants(bus_root).unwrap_or_default();
        self.drain_ingress(persist, bus_root, &grants);
        self.publish_status(persist, bus_root, &grants);
    }

    // ── (1) GUI-driven accept / revoke / refuse-mint ───────────────────────────

    fn drain_actions(&mut self, persist: &Persist, bus_root: &Path, trust: &Path) {
        // accept — establish a pair + install the trust cert.
        let accepts = persist
            .list_since(ACCEPT_TOPIC, self.accept_cursor.as_deref())
            .unwrap_or_default();
        for msg in accepts {
            self.accept_cursor = Some(msg.ulid.clone());
            if let Some(body) = msg.body.as_deref() {
                if let Err(error) = self.authorize_action("accept", body) {
                    audit(
                        persist,
                        "federation/accept-error",
                        &serde_json::json!({
                            "event": "authorization-refused",
                            "action": "accept",
                            "error": error,
                        }),
                    );
                    continue;
                }
                self.handle_accept(body, persist, bus_root, trust);
            }
        }
        // revoke — remove a pair + delete the trust cert (default-deny restored).
        let revokes = persist
            .list_since(REVOKE_TOPIC, self.revoke_cursor.as_deref())
            .unwrap_or_default();
        for msg in revokes {
            self.revoke_cursor = Some(msg.ulid.clone());
            if let Some(body) = msg.body.as_deref() {
                if let Err(error) = self.authorize_action("revoke", body) {
                    audit(
                        persist,
                        "federation/revoke-error",
                        &serde_json::json!({
                            "event": "authorization-refused",
                            "action": "revoke",
                            "error": error,
                        }),
                    );
                    continue;
                }
                self.handle_revoke(body, persist, bus_root, trust);
            }
        }
        // refuse-mint — cancel a pending outbound offer.
        let refuses = persist
            .list_since(REFUSE_MINT_TOPIC, self.refuse_mint_cursor.as_deref())
            .unwrap_or_default();
        for msg in refuses {
            self.refuse_mint_cursor = Some(msg.ulid.clone());
            if let Some(body) = msg.body.as_deref() {
                if let Err(error) = self.authorize_action("refuse-mint", body) {
                    audit(
                        persist,
                        "federation/mint-revoke-error",
                        &serde_json::json!({
                            "event": "authorization-refused",
                            "action": "refuse-mint",
                            "error": error,
                        }),
                    );
                    continue;
                }
                self.handle_refuse_mint(body, persist, bus_root);
            }
        }
    }

    /// Authenticate a federation mutation before any grant, mint, or trust
    /// certificate helper is reached. Targets are deterministic and bound to
    /// the requested peer/mint where the body carries one.
    fn authorize_action(&self, action: &str, body: &str) -> Result<(), String> {
        if !crate::ipc::body_within_cap(Some(body)) {
            return Err("request body exceeds the 64 KiB cap".to_string());
        }
        let target = mutation_target(action, body)?;
        let auth_verb = format!("federation-{action}");
        self.authorizer.authorize(
            body,
            MutationContext {
                verb: &auth_verb,
                node: FEDERATION_ACTION_NODE_SCOPE,
                target: &target,
            },
        )
    }

    fn handle_accept(&self, body: &str, persist: &Persist, bus_root: &Path, trust: &Path) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
            return;
        };
        let Some(passcode) = v.get("passcode").and_then(|x| x.as_str()) else {
            return;
        };
        let label = v
            .get("label")
            .and_then(|x| x.as_str())
            .unwrap_or("Remote mesh");
        let peer_ca = v.get("peer-ca-pem").and_then(|x| x.as_str());
        match accept_passcode(bus_root, trust, passcode, label, peer_ca) {
            Ok(outcome) => {
                audit(
                    persist,
                    &format!("federation/pair-established/{}", outcome.peer_mesh_id),
                    &serde_json::json!({
                        "event": "pair-established",
                        "peer-mesh-id": outcome.peer_mesh_id,
                        "peer-mesh-label": outcome.label,
                        "established": outcome.established,
                        "trust-cert-installed": outcome.cert_path.is_some(),
                        "via": "shell",
                    }),
                );
                tracing::info!(
                    peer_mesh_id = %outcome.peer_mesh_id,
                    "federation_enforcer: accepted a pair via the shell (trust cert {})",
                    if outcome.cert_path.is_some() { "installed" } else { "not installed" }
                );
            }
            Err(e) => {
                audit(
                    persist,
                    "federation/accept-error",
                    &serde_json::json!({ "event": "accept-error", "error": e.to_string() }),
                );
                tracing::warn!(error = %e, "federation_enforcer: shell accept refused");
            }
        }
    }

    fn handle_revoke(&self, body: &str, persist: &Persist, bus_root: &Path, trust: &Path) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
            return;
        };
        let Some(peer) = v.get("peer-mesh-id").and_then(|x| x.as_str()) else {
            return;
        };
        match revoke_pair(bus_root, trust, peer) {
            Ok(()) => {
                audit(
                    persist,
                    &format!("federation/pair-revoked/{}", safe_seg(peer)),
                    &serde_json::json!({ "event": "pair-revoked", "peer-mesh-id": peer, "via": "shell" }),
                );
                tracing::info!(peer_mesh_id = %peer, "federation_enforcer: revoked a pair via the shell");
            }
            Err(e) => {
                audit(
                    persist,
                    "federation/revoke-error",
                    &serde_json::json!({ "event": "revoke-error", "peer-mesh-id": peer, "error": e.to_string() }),
                );
            }
        }
    }

    fn handle_refuse_mint(&self, body: &str, persist: &Persist, bus_root: &Path) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
            return;
        };
        let Some(ulid) = v.get("ulid").and_then(|x| x.as_str()) else {
            return;
        };
        if !is_safe_id(ulid) {
            return;
        }
        let path = mint_path(bus_root, ulid);
        if std::fs::remove_file(&path).is_ok() {
            audit(
                persist,
                "federation/mint-revoked/local",
                &serde_json::json!({ "event": "mint-revoked", "ulid": ulid, "via": "shell" }),
            );
            tracing::info!(
                ulid,
                "federation_enforcer: cancelled a pending mint via the shell"
            );
        }
    }

    // ── (2) cross-mesh ingress identity check (default DENY) ────────────────────

    fn drain_ingress(&self, persist: &Persist, bus_root: &Path, grants: &FederationGrants) {
        let dir = ingress_dir(bus_root);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return; // no spool yet — nothing to enforce
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            // Whatever we decide, the spool file is consumed (removed) so a message
            // is enforced exactly once — a DROP must not linger for a retry.
            let decoded = std::fs::read_to_string(&path)
                .ok()
                .and_then(|t| serde_json::from_str::<CrossMeshEnvelope>(&t).ok());
            let _ = std::fs::remove_file(&path);
            let Some(env) = decoded else {
                // Malformed envelope — drop it (fail closed), audit anonymously.
                audit(
                    persist,
                    &format!("{REFUSED_PREFIX}unknown"),
                    &serde_json::json!({ "event": "refused", "reason": "malformed-envelope" }),
                );
                continue;
            };
            match grants.decide(&env.peer_mesh_id, &env.topic, Direction::Ingress) {
                Decision::Allow => {
                    // Forward onto the local bus — the boundary is crossed only here.
                    let _ = persist.write(
                        &env.topic,
                        Priority::Default,
                        env.title.as_deref(),
                        env.body.as_deref(),
                    );
                    audit(
                        persist,
                        &format!("{INGRESS_OK_PREFIX}{}", safe_seg(&env.peer_mesh_id)),
                        &serde_json::json!({
                            "event": "ingress-accepted",
                            "peer-mesh-id": env.peer_mesh_id,
                            "topic": env.topic,
                        }),
                    );
                }
                Decision::Deny(reason) => {
                    audit(
                        persist,
                        &format!("{REFUSED_PREFIX}{}", safe_seg(&env.peer_mesh_id)),
                        &serde_json::json!({
                            "event": "refused",
                            "peer-mesh-id": env.peer_mesh_id,
                            "topic": env.topic,
                            "reason": reason.as_str(),
                        }),
                    );
                    tracing::debug!(
                        peer_mesh_id = %env.peer_mesh_id,
                        topic = %env.topic,
                        reason = reason.as_str(),
                        "federation_enforcer: refused a cross-mesh ingress message (default-deny)"
                    );
                }
            }
        }
    }

    // ── (3) status mirror (only on change) ──────────────────────────────────────

    fn publish_status(&mut self, persist: &Persist, bus_root: &Path, grants: &FederationGrants) {
        let accepted: Vec<serde_json::Value> = grants
            .pairs
            .iter()
            .map(|p| {
                serde_json::json!({
                    "peer-mesh-id": p.peer_mesh_id,
                    "peer-mesh-label": p.peer_mesh_label,
                    "established": p.established,
                    "subscribe-count": p.subscribe_topics.len(),
                    "publish-count": p.publish_topics.len(),
                    "excluded-count": p.excluded_topics.len(),
                })
            })
            .collect();
        let pending: Vec<serde_json::Value> = pending_mints(bus_root)
            .into_iter()
            .map(|m| {
                serde_json::json!({
                    "ulid": m.ulid,
                    "expires-at-unix-ms": m.expires_at_unix_ms,
                })
            })
            .collect();
        // Stable status body (no timestamp) so an unchanged posture is a no-op write
        // (§7: no inert churn) — the change-gate compares this exact string.
        let body = serde_json::json!({
            "node": self.node_id,
            "enforced": true,
            "accepted": accepted,
            "pending-mints": pending,
        })
        .to_string();
        if self.last_status.as_deref() == Some(body.as_str()) {
            return;
        }
        let topic = format!("{STATUS_PREFIX}{}", self.node_id);
        if persist
            .write(&topic, Priority::Min, None, Some(&body))
            .is_ok()
        {
            self.last_status = Some(body);
        }
    }
}

// ── free helpers ────────────────────────────────────────────────────────────────

/// Derive the closed capability target for a federation mutation without
/// touching grants, mints, or the trust-cert directory.
fn mutation_target(action: &str, body: &str) -> Result<String, String> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|_| format!("federation-{action}: request body must be JSON"))?;
    let object = value
        .as_object()
        .ok_or_else(|| format!("federation-{action}: request body must be an object"))?;
    match action {
        // The passcode is a secret and must not become a capability target.
        "accept" => Ok("pair".to_string()),
        "revoke" => object
            .get("peer-mesh-id")
            .and_then(serde_json::Value::as_str)
            .filter(|peer| is_safe_id(peer))
            .map(|peer| format!("peer:{peer}"))
            .ok_or_else(|| "federation-revoke: missing safe peer-mesh-id".to_string()),
        "refuse-mint" => object
            .get("ulid")
            .and_then(serde_json::Value::as_str)
            .filter(|ulid| is_safe_id(ulid))
            .map(|ulid| format!("mint:{ulid}"))
            .ok_or_else(|| "federation-refuse-mint: missing safe ulid".to_string()),
        _ => Err(format!("unknown federation mutation: {action}")),
    }
}

/// Publish a best-effort audit event; a validation/write failure is a silent no-op
/// (the hash-chained persist index is the source of truth, this is a courtesy lane).
fn audit(persist: &Persist, topic: &str, payload: &serde_json::Value) {
    let body = payload.to_string();
    let _ = persist.write(topic, Priority::Min, None, Some(&body));
}

/// The unconsumed, unexpired pending mints under `bus_root` (the outbound offers the
/// shell shows with a Cancel action).
fn pending_mints(bus_root: &Path) -> Vec<MintEnvelope> {
    let dir = mde_bus::federation::mints_dir(bus_root);
    let now = mde_bus::federation::now_unix_ms();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Ok(env) = std::fs::read_to_string(&path)
            .map_err(|_| ())
            .and_then(|t| serde_json::from_str::<MintEnvelope>(&t).map_err(|_| ()))
        {
            if !env.used && env.expires_at_unix_ms > now {
                out.push(env);
            }
        }
    }
    out.sort_by(|a, b| a.ulid.cmp(&b.ulid));
    out
}

/// A bus-topic-safe segment derived from an (untrusted) mesh id: bare `[A-Za-z0-9_-]`,
/// else `unknown`. Keeps a hostile origin id from breaking the audit topic.
fn safe_seg(id: &str) -> String {
    if is_safe_id(id) {
        id.to_string()
    } else {
        "unknown".to_string()
    }
}

/// A bare `[A-Za-z0-9_-]` id (the ULID shape) — never a path/topic-traversal string.
fn is_safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn default_bus_root() -> Option<PathBuf> {
    if let Some(root) = std::env::var_os("MDE_BUS_ROOT").filter(|r| !r.is_empty()) {
        return Some(PathBuf::from(root));
    }
    Some(dirs::data_dir()?.join("mde").join("bus"))
}

#[async_trait::async_trait]
impl Worker for FederationEnforcerWorker {
    fn name(&self) -> &'static str {
        "federation_enforcer"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let Some(bus_root) = self.bus_root_override.clone().or_else(default_bus_root) else {
            tracing::debug!(target: "mackesd::federation_enforcer", "no bus root; worker idle");
            return Ok(());
        };
        let persist = match Persist::open(bus_root.clone()) {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!(target: "mackesd::federation_enforcer", error = %e, "persist open failed; worker idle");
                return Ok(());
            }
        };
        let trust = self.resolved_trust_dir();
        let mut tick = tokio::time::interval(self.poll_interval);
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    self.tick_once(&persist, &bus_root, &trust);
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
    use crate::ipc::action_auth::{authorize_test_body, ActionAuthorizer};
    use mde_bus::federation::{
        read_grants, write_mint_envelope, CrossMeshEnvelope, FederationGrants,
    };
    use std::sync::Arc;

    const AUTH_KEY: &[u8] = b"federation-action-auth-test-key";
    const AUTH_NOW: i64 = 1_700_000_000_000;

    fn persist_at(dir: &Path) -> Persist {
        Persist::open(dir.to_path_buf()).expect("open persist")
    }

    /// A unique bare `[a-z0-9]` token — a self-contained stand-in for a ULID so the
    /// tests don't pull the `ulid` crate into `mackesd`.
    fn uniq() -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!("mesh{t}x{n}")
    }

    /// A trust dir whose parent exists so cert install lands in the tempdir.
    fn trusts(dir: &Path) -> PathBuf {
        let parent = dir.join("nebula");
        std::fs::create_dir_all(&parent).unwrap();
        parent.join("federation-trusts")
    }

    /// Mint a single-use passcode into `bus_root`, returning its mnemonic.
    fn mint(bus_root: &Path) -> String {
        let id = uniq();
        let env = MintEnvelope {
            ulid: id,
            mnemonic: "mesh node link mint mode myth".to_string(),
            expires_at_unix_ms: mde_bus::federation::now_unix_ms() + 86_400_000,
            used: false,
        };
        write_mint_envelope(bus_root, &env).unwrap();
        env.mnemonic
    }

    /// Drop a cross-mesh ingress envelope into the spool.
    fn drop_ingress(bus_root: &Path, peer: &str, topic: &str, body: &str) {
        let dir = ingress_dir(bus_root);
        std::fs::create_dir_all(&dir).unwrap();
        let env = CrossMeshEnvelope {
            peer_mesh_id: peer.to_string(),
            topic: topic.to_string(),
            body: Some(body.to_string()),
            title: None,
        };
        let path = dir.join(format!("{}.json", uniq()));
        std::fs::write(path, serde_json::to_string(&env).unwrap()).unwrap();
    }

    fn worker(bus_root: &Path) -> FederationEnforcerWorker {
        FederationEnforcerWorker::new("peer:test".into())
            .with_bus_root(bus_root.to_path_buf())
            .with_trust_dir(trusts(bus_root))
            .with_authorizer(Arc::new(ActionAuthorizer::for_test(
                AUTH_KEY,
                bus_root.join("auth"),
                AUTH_NOW,
            )))
    }

    /// Arm an action body exactly as the root shell does. The worker's test
    /// authorizer uses the same semantic context and durable replay directory.
    fn signed_action(action: &str, unsigned_body: &str) -> String {
        let target = mutation_target(action, unsigned_body).unwrap();
        let verb = format!("federation-{action}");
        authorize_test_body(
            AUTH_KEY,
            unsigned_body,
            MutationContext {
                verb: &verb,
                node: FEDERATION_ACTION_NODE_SCOPE,
                target: &target,
            },
            &format!("federation-{}", uniq()),
            AUTH_NOW + 30_000,
        )
    }

    // ── the DEFINING two-identity acceptance: unaccepted-deny / granted-allow /
    //    revoked-deny, with cert install on accept + removal on revoke ───────────

    #[test]
    fn two_identity_flow_unaccepted_denied_then_granted_then_revoked_denied() {
        // "mesh B" is THIS node's bus; "MESH-A" is the foreign mesh trying to route.
        let dir = tempfile::tempdir().unwrap();
        let bus = dir.path();
        let trust = trusts(bus);
        let persist = persist_at(bus);
        let mut w = worker(bus);

        // (a) UNACCEPTED — a foreign-mesh ingress before any accept is DROPPED, never
        //     forwarded onto the local bus.
        drop_ingress(bus, "MESH-A", "portal/greeting", "hi from A");
        w.tick_once(&persist, bus, &trust);
        assert!(
            persist
                .list_since("portal/greeting", None)
                .unwrap()
                .is_empty(),
            "an unaccepted foreign mesh must not route onto the local bus"
        );
        let refused = persist
            .list_topics()
            .unwrap()
            .into_iter()
            .filter(|t| t.starts_with(REFUSED_PREFIX))
            .count();
        assert!(refused >= 1, "the refusal is audited");

        // (b) ACCEPT via the shell action lane — establishes the pair + installs cert.
        let mnemonic = mint(bus);
        let accept_body = signed_action(
            "accept",
            &serde_json::json!({
                "schema_version": 1,
                "passcode": mnemonic,
                "label": "Mesh A"
            })
            .to_string(),
        );
        persist
            .write(ACCEPT_TOPIC, Priority::Default, None, Some(&accept_body))
            .unwrap();
        w.tick_once(&persist, bus, &trust);
        let grants = read_grants(bus).unwrap();
        assert_eq!(grants.pairs.len(), 1, "accept established exactly one pair");
        let peer_id = grants.pairs[0].peer_mesh_id.clone();
        assert!(
            mde_bus::federation::trust_cert_path(&trust, &peer_id).exists(),
            "accept installed the cross-mesh trust cert"
        );

        // (c) GRANTED — with subscribe `#`, a non-excluded topic from the ACCEPTED
        //     mesh now routes onto the local bus.
        drop_ingress(bus, &peer_id, "portal/greeting", "hi again");
        w.tick_once(&persist, bus, &trust);
        assert_eq!(
            persist
                .list_since("portal/greeting", None)
                .unwrap()
                .last()
                .and_then(|m| m.body.clone())
                .as_deref(),
            Some("hi again"),
            "an accepted+granted topic routes onto the local bus"
        );

        // (c') EXCLUDED — an excluded lane never crosses even for the accepted mesh.
        drop_ingress(bus, &peer_id, "passcode/steal", "gimme");
        w.tick_once(&persist, bus, &trust);
        assert!(
            persist
                .list_since("passcode/steal", None)
                .unwrap()
                .is_empty(),
            "an excluded topic never crosses, even for an accepted mesh"
        );

        // (d) REVOKE via the shell action lane — removes the pair + deletes the cert.
        let revoke_body = signed_action(
            "revoke",
            &serde_json::json!({
                "schema_version": 1,
                "peer-mesh-id": peer_id
            })
            .to_string(),
        );
        persist
            .write(REVOKE_TOPIC, Priority::Default, None, Some(&revoke_body))
            .unwrap();
        w.tick_once(&persist, bus, &trust);
        assert!(
            read_grants(bus).unwrap().pairs.is_empty(),
            "revoke removed the pair"
        );
        assert!(
            !mde_bus::federation::trust_cert_path(&trust, &peer_id).exists(),
            "revoke removed the trust cert"
        );

        // (d') REVOKED — a foreign-mesh ingress is refused AGAIN (default-deny).
        drop_ingress(bus, &peer_id, "portal/greeting", "still here?");
        let before = persist.list_since("portal/greeting", None).unwrap().len();
        w.tick_once(&persist, bus, &trust);
        assert_eq!(
            persist.list_since("portal/greeting", None).unwrap().len(),
            before,
            "a revoked mesh must be refused again — no new message routes"
        );
    }

    #[test]
    fn hostile_unsigned_federation_actions_are_refused_before_state_or_cert_io() {
        let dir = tempfile::tempdir().unwrap();
        let bus = dir.path();
        let trust = trusts(bus);
        let persist = persist_at(bus);
        let mut w = worker(bus);

        // An unsigned accept must not consume the mint, write grants, or install
        // a trust certificate before the shared authorizer has passed.
        let mnemonic = mint(bus);
        let unsigned_accept = serde_json::json!({
            "schema_version": 1,
            "passcode": mnemonic,
            "label": "Unsigned mesh"
        })
        .to_string();
        persist
            .write(
                ACCEPT_TOPIC,
                Priority::Default,
                None,
                Some(&unsigned_accept),
            )
            .unwrap();
        w.tick_once(&persist, bus, &trust);
        assert!(read_grants(bus).unwrap().pairs.is_empty());
        assert!(
            pending_mints(bus).len() == 1,
            "unsigned accept consumed a mint"
        );
        assert!(
            std::fs::read_dir(&trust)
                .map(|mut entries| entries.next().is_none())
                .unwrap_or(true),
            "unsigned accept installed a trust certificate"
        );

        // Establish a real pair so an unsigned revoke can prove it leaves both
        // the grant and installed trust certificate untouched.
        let accept_body = signed_action(
            "accept",
            &serde_json::json!({
                "schema_version": 1,
                "passcode": mint(bus),
                "label": "Mesh A"
            })
            .to_string(),
        );
        persist
            .write(ACCEPT_TOPIC, Priority::Default, None, Some(&accept_body))
            .unwrap();
        w.tick_once(&persist, bus, &trust);
        let peer_id = read_grants(bus).unwrap().pairs[0].peer_mesh_id.clone();
        let cert = mde_bus::federation::trust_cert_path(&trust, &peer_id);
        assert!(cert.exists());

        let unsigned_revoke = serde_json::json!({
            "schema_version": 1,
            "peer-mesh-id": peer_id
        })
        .to_string();
        persist
            .write(
                REVOKE_TOPIC,
                Priority::Default,
                None,
                Some(&unsigned_revoke),
            )
            .unwrap();
        w.tick_once(&persist, bus, &trust);
        assert_eq!(read_grants(bus).unwrap().pairs.len(), 1);
        assert!(cert.exists(), "unsigned revoke removed the trust cert");

        // The pending mint cancel lane is also a mutation and remains intact.
        let pending_id = pending_mints(bus).last().unwrap().ulid.clone();
        let unsigned_refuse = serde_json::json!({
            "schema_version": 1,
            "ulid": pending_id
        })
        .to_string();
        persist
            .write(
                REFUSE_MINT_TOPIC,
                Priority::Default,
                None,
                Some(&unsigned_refuse),
            )
            .unwrap();
        w.tick_once(&persist, bus, &trust);
        assert!(
            mint_path(bus, &pending_mints(bus).last().unwrap().ulid).exists(),
            "unsigned refuse-mint removed the pending offer"
        );
    }

    #[test]
    fn federation_authority_is_exact_body_bound_and_single_use() {
        let dir = tempfile::tempdir().unwrap();
        let bus = dir.path();
        let w = worker(bus);
        let unsigned = serde_json::json!({
            "schema_version": 1,
            "passcode": "mesh node link mint mode myth",
            "label": "Mesh A"
        })
        .to_string();
        let armed = signed_action("accept", &unsigned);
        assert!(w.authorize_action("accept", &armed).is_ok());
        assert!(w
            .authorize_action("accept", &armed)
            .unwrap_err()
            .contains("already used"));

        let tampered = armed.replace("Mesh A", "Mesh B");
        assert!(w.authorize_action("accept", &tampered).is_err());
        assert!(mutation_target(
            "revoke",
            r#"{"schema_version":1,"peer-mesh-id":"../../outside"}"#
        )
        .is_err());
    }

    #[test]
    fn status_mirror_lists_accepted_pairs_and_is_change_gated() {
        let dir = tempfile::tempdir().unwrap();
        let bus = dir.path();
        let trust = trusts(bus);
        let persist = persist_at(bus);
        let mut w = worker(bus);

        // First tick with nothing accepted → an "empty" enforced mirror is published.
        w.tick_once(&persist, bus, &trust);
        let topic = format!("{STATUS_PREFIX}peer:test");
        let first = persist.list_since(&topic, None).unwrap();
        assert_eq!(first.len(), 1, "a status mirror is published");

        // A second tick with NO change must not re-publish (change-gated, §7).
        w.tick_once(&persist, bus, &trust);
        assert_eq!(
            persist.list_since(&topic, None).unwrap().len(),
            1,
            "an unchanged posture is not re-published"
        );

        // Accept a pair → the mirror changes → a new row is published listing it.
        let mnemonic = mint(bus);
        let accept_body = signed_action(
            "accept",
            &serde_json::json!({
                "schema_version": 1,
                "passcode": mnemonic,
                "label": "Mesh A"
            })
            .to_string(),
        );
        persist
            .write(ACCEPT_TOPIC, Priority::Default, None, Some(&accept_body))
            .unwrap();
        w.tick_once(&persist, bus, &trust);
        let rows = persist.list_since(&topic, None).unwrap();
        assert!(rows.len() >= 2, "accepting a pair republishes the mirror");
        let latest = rows.last().unwrap().body.clone().unwrap();
        let v: serde_json::Value = serde_json::from_str(&latest).unwrap();
        assert_eq!(v["accepted"].as_array().unwrap().len(), 1);
        assert_eq!(v["enforced"], true);
    }

    #[test]
    fn malformed_ingress_is_dropped_not_forwarded() {
        let dir = tempfile::tempdir().unwrap();
        let bus = dir.path();
        let trust = trusts(bus);
        let persist = persist_at(bus);
        let mut w = worker(bus);
        let d = ingress_dir(bus);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("junk.json"), "not json").unwrap();
        w.tick_once(&persist, bus, &trust);
        // The junk file is consumed and a refusal audited.
        assert!(
            !d.join("junk.json").exists(),
            "a malformed envelope is consumed"
        );
        assert!(persist
            .list_since(&format!("{REFUSED_PREFIX}unknown"), None)
            .unwrap()
            .iter()
            .any(|m| m.body.as_deref().unwrap_or("").contains("malformed")));
    }

    #[test]
    fn decide_is_default_deny_for_unknown_mesh() {
        // Pure gate check independent of the worker plumbing.
        let grants = FederationGrants::default();
        assert!(!grants
            .decide("anyone", "portal/x", Direction::Ingress)
            .is_allow());
    }
}
