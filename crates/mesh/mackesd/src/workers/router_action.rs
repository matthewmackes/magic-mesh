//! WL-RUN-006 — the **router firewall-edit executor**: the privileged node-side
//! seam that makes the router-control read slice's mutation stage real.
//!
//! The Device-Manager surface (`mde-shell-egui`, where a `HostKind::Router`
//! already renders) holds no privileged-exec seam — it composes a typed
//! [`RouterActionRequest`] and writes it into this node's replicated
//! `<workgroup_root>/action/router/<self>/` dir (Syncthing carries a remote seat's
//! request to the node that sits behind the router, §9 — no push-SSH). This worker
//! drains its own dir and, for each request, applies the **three load-bearing
//! safety layers** before any live mutation:
//!
//! 1. **Typed-confirm gate** ([`RouterActionRequest::typed_confirm_ok`]): the
//!    request must echo the appliance id (the gateway MAC); a mismatch is refused
//!    before any mutation (the release-rollback `--confirm` idiom, bound to the
//!    *specific* appliance).
//! 2. **Vyatta `commit-confirm`** ([`FirewallEdit::to_vyatta_commands`] applied
//!    inside a `commit-confirm <min>` window): an edit that locks us out of the
//!    router **auto-reverts** — the worker only `confirm`s (makes permanent) when
//!    it can still re-reach the box after the change.
//! 3. **Hash-chain audit** ([`append_audit`] / [`verify_audit`], reusing
//!    [`crate::audit`]): every edit — applied, refused, reverted, or staged —
//!    appends a tamper-evident [`AuditRecord`] to
//!    `<workgroup_root>/<host>/router-audit.jsonl`.
//!
//! The **live mutation is operator-gated** (a real farm-gateway firewall change):
//! the worker resolves + records the edit but only shells out to the router when
//! `MDE_ROUTER_ACTION_LIVE=1` is set — otherwise the edit is *staged* (recorded,
//! honestly not applied, §7). Rank-0 / universal like `device_control`; a request
//! is drained ONLY by the node whose `<self>` dir it lands in AND whose *own*
//! primary gateway matches the request's appliance id, so a node can never mutate
//! a router it does not sit behind.

#![cfg(feature = "async-services")]

use std::path::{Path, PathBuf};
use std::time::Duration;

// The shared contract.
use mackes_mesh_types::router_action::{
    audit_log_path, clamp_confirm_min, read_audit, take_requests, write_result, AuditRecord,
    RouterActionRequest, RouterActionResult,
};

use super::{ShutdownToken, Worker};
use crate::audit::{self, AuditRow, VerifyOutcome};
use crate::ipc::secret_store::{self, SecretStore};
use crate::router_discovery::{self, RouterCandidate};

/// Request poll cadence — an edit lands within ~3 s of replication (as `device_control`).
pub const POLL: Duration = Duration::from_secs(3);

/// The env flag that arms the LIVE router mutation. Absent/`!= "1"` → every edit is
/// *staged* (recorded + audited, but the box is never touched). This is the parked,
/// operator-gated live-smoke seam.
pub const LIVE_ENV: &str = "MDE_ROUTER_ACTION_LIVE";

/// The resolved plan handed to the (parked) live applier — everything the Vyatta
/// commit-confirm dance needs, all typed (§9 — no command string reaches here).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyPlan {
    /// The appliance management IP (this node's own resolved primary gateway).
    pub ip: String,
    /// The SSH user (from the sealed `router/<mac>` cred).
    pub user: String,
    /// The SSH password (from the sealed cred; fed via `SSHPASS`, never argv).
    pub pass: String,
    /// The FIXED `set/delete firewall name …` lines to apply.
    pub commands: Vec<String>,
    /// The commit-confirm auto-revert window (minutes), already clamped.
    pub confirm_min: u32,
}

/// The outcome of applying a plan inside a commit-confirm window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// Applied AND still reachable AND confirmed + saved (permanent).
    Confirmed,
    /// Applied but reachability was LOST — the commit-confirm timer auto-reverts;
    /// the worker deliberately never `confirm`s (SAFETY 2).
    Reverted,
    /// The apply itself failed (a bad set line, an unreachable box, …).
    Error(String),
}

/// The pre-apply decision — the pure gate over a request BEFORE any cred/IP
/// resolution or mutation. Unit-tested in isolation (the SAFETY-1 fixtures).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreApply {
    /// Refused — the typed-confirm token did not match, or the edit is malformed /
    /// injection-unsafe. Carries the honest reason + the audit outcome token.
    Refused {
        /// Human-readable reason for the [`RouterActionResult::failed`].
        reason: String,
        /// The audit `outcome` token (`refused-typed-confirm` / `refused-invalid`).
        outcome: &'static str,
    },
    /// Recorded but not applied — the live seam is operator-gated (parked).
    Staged,
    /// The gates passed + the live seam is armed — resolve cred/IP + apply.
    Proceed,
}

/// SAFETY 1 (+ validity + the live gate) — the pure pre-apply decision. Refuses a
/// missing/wrong typed-confirm token or a malformed edit BEFORE anything else; when
/// the gates pass, stages the edit unless the live seam is armed. No IO, so the
/// gate logic is tested without a router / secret store.
#[must_use]
pub fn pre_apply_decision(req: &RouterActionRequest, live_enabled: bool) -> PreApply {
    if !req.typed_confirm_ok() {
        return PreApply::Refused {
            reason: format!(
                "typed-confirm token did not match appliance {} — refused (SAFETY 1)",
                req.appliance_id
            ),
            outcome: "refused-typed-confirm",
        };
    }
    if !req.edit.is_valid() {
        return PreApply::Refused {
            reason: "firewall edit is malformed or not injection-safe — refused".to_string(),
            outcome: "refused-invalid",
        };
    }
    if live_enabled {
        PreApply::Proceed
    } else {
        PreApply::Staged
    }
}

/// Map a live [`ApplyOutcome`] onto the typed result + the audit outcome token
/// (pure — the commit-confirm auto-revert fixture drives this).
#[must_use]
pub fn map_outcome(
    id: &str,
    summary: &str,
    outcome: &ApplyOutcome,
) -> (RouterActionResult, &'static str) {
    match outcome {
        ApplyOutcome::Confirmed => (
            RouterActionResult::ok(id, format!("{summary}; confirmed + saved")),
            "applied",
        ),
        ApplyOutcome::Reverted => (
            RouterActionResult::reverted(
                id,
                format!("{summary}; lost reachability — commit-confirm auto-reverted"),
            ),
            "auto-reverted",
        ),
        ApplyOutcome::Error(e) => (RouterActionResult::failed(id, e.clone()), "error"),
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn encode_hex(bytes: &[u8; 32]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::with_capacity(64), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn decode_hex32(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
        out[i] = u8::from_str_radix(std::str::from_utf8(chunk).ok()?, 16).ok()?;
    }
    Some(out)
}

/// SAFETY 3 — append one tamper-evident row to the router audit chain at
/// `<root>/<host>/router-audit.jsonl` and return it. Reads the current head to
/// chain on it (genesis = 32 zero bytes) via [`crate::audit::next_hash`], the same
/// algorithm the `events` table uses. Best-effort atomicity: the whole chain is
/// rewritten with the new line appended (a single-writer file — this node's own
/// worker), tmp+rename so a reader never sees a half-written chain.
///
/// # Errors
/// IO / serialization failures (the caller logs + swallows — an audit hiccup never
/// wedges the op lane).
pub fn append_audit(
    root: &Path,
    host: &str,
    appliance_id: &str,
    action: &str,
    summary: &str,
    outcome: &str,
    from: &str,
    timestamp_ms: i64,
) -> std::io::Result<AuditRecord> {
    let path = audit_log_path(root, host);
    let existing = read_audit(&path);
    let prev_hash = existing
        .last()
        .and_then(|r| decode_hex32(&r.hash))
        .unwrap_or([0u8; 32]);
    let seq = existing.last().map_or(1, |r| r.seq + 1);
    let mut rec = AuditRecord {
        seq,
        timestamp_ms,
        appliance_id: appliance_id.to_string(),
        action: action.to_string(),
        summary: summary.to_string(),
        outcome: outcome.to_string(),
        from: from.to_string(),
        prev_hash: encode_hex(&prev_hash),
        hash: String::new(),
    };
    let payload = rec
        .hash_payload()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let hash = audit::next_hash(&prev_hash, &payload, timestamp_ms);
    rec.hash = encode_hex(&hash);

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut body = String::new();
    for r in &existing {
        body.push_str(&serde_json::to_string(r).map_err(std::io::Error::other)?);
        body.push('\n');
    }
    body.push_str(&serde_json::to_string(&rec).map_err(std::io::Error::other)?);
    body.push('\n');
    let tmp = path.with_extension("jsonl.tmp");
    std::fs::write(&tmp, body.as_bytes())?;
    std::fs::rename(&tmp, &path)?;
    Ok(rec)
}

/// SAFETY 3 — walk the router audit chain at `path` and report the first tampered
/// row (or `Intact`). Reuses [`crate::audit::verify`] over rows reconstructed from
/// each line's canonical payload — a rewritten field breaks the chain from there.
#[must_use]
pub fn verify_audit(path: &Path) -> VerifyOutcome {
    let records = read_audit(path);
    let mut rows = Vec::with_capacity(records.len());
    for rec in &records {
        let Ok(payload) = rec.hash_payload() else {
            continue;
        };
        let hash = decode_hex32(&rec.hash).unwrap_or([0u8; 32]);
        rows.push(AuditRow {
            event_id: rec.seq,
            payload,
            timestamp_ms: rec.timestamp_ms,
            hash,
        });
    }
    audit::verify(&rows)
}

/// The router firewall-edit executor (per-node, rank-0 universal).
pub struct RouterActionWorker {
    /// Replicated workgroup root (the `action/router/<self>/` dir + the audit chain).
    workgroup_root: PathBuf,
    /// This node's hostname (the dir it drains + the audit chain it writes under).
    self_hostname: String,
    /// This node's id (`peer:<host>`) — reserved for future fleet-wide audit.
    #[allow(dead_code)]
    node_id: String,
    /// The secret store the `router/<mac>` cred is read from.
    secret_store: SecretStore,
    /// SAFETY 2 live gate — only shell out to the router when armed (env-gated).
    live_enabled: bool,
    /// Injected live applier (tests supply a fake; production uses [`live_apply`]).
    #[allow(clippy::type_complexity)]
    applier: Option<Box<dyn Fn(&ApplyPlan) -> ApplyOutcome + Send + Sync>>,
    /// Injected gateway resolver (tests supply a fake; production discovers it).
    #[allow(clippy::type_complexity)]
    gateway: Option<Box<dyn Fn() -> Option<RouterCandidate> + Send + Sync>>,
}

impl RouterActionWorker {
    /// Construct with production defaults. `self_hostname` keys the drain dir + the
    /// audit chain; `node_id` is the `peer:<host>` audit actor.
    #[must_use]
    pub fn new(workgroup_root: PathBuf, self_hostname: String, node_id: String) -> Self {
        let secret_store = SecretStore::resolve(&secret_store::repo_root(), &workgroup_root);
        let live_enabled = std::env::var(LIVE_ENV).ok().as_deref() == Some("1");
        Self {
            workgroup_root,
            self_hostname,
            node_id,
            secret_store,
            live_enabled,
            applier: None,
            gateway: None,
        }
    }

    /// Override the secret store (tests drive a seeded `LocalAead`).
    #[must_use]
    pub fn with_secret_store(mut self, store: SecretStore) -> Self {
        self.secret_store = store;
        self
    }

    /// Arm/disarm the live seam (tests + the operator gate).
    #[must_use]
    pub fn with_live(mut self, enabled: bool) -> Self {
        self.live_enabled = enabled;
        self
    }

    /// Inject a fake live applier (tests).
    #[must_use]
    pub fn with_applier(
        mut self,
        f: impl Fn(&ApplyPlan) -> ApplyOutcome + Send + Sync + 'static,
    ) -> Self {
        self.applier = Some(Box::new(f));
        self
    }

    /// Inject a fake gateway resolver (tests).
    #[must_use]
    pub fn with_gateway(
        mut self,
        f: impl Fn() -> Option<RouterCandidate> + Send + Sync + 'static,
    ) -> Self {
        self.gateway = Some(Box::new(f));
        self
    }

    fn discover_gateway(&self) -> Option<RouterCandidate> {
        match &self.gateway {
            Some(f) => f(),
            None => router_discovery::discover_primary(),
        }
    }

    fn apply(&self, plan: &ApplyPlan) -> ApplyOutcome {
        match &self.applier {
            Some(f) => f(plan),
            None => live_apply(plan),
        }
    }

    /// Best-effort audit append (SAFETY 3) — a store fault is logged, never fatal.
    fn audit(&self, req: &RouterActionRequest, outcome: &str) {
        if let Err(e) = append_audit(
            &self.workgroup_root,
            &self.self_hostname,
            &req.appliance_id,
            req.edit.op.as_str(),
            &req.edit.summary(),
            outcome,
            &req.from,
            now_ms(),
        ) {
            tracing::warn!(
                target: "mackesd::router_action",
                error = %e, appliance = %req.appliance_id, outcome,
                "router-action audit append failed"
            );
        }
    }

    /// Handle one request end-to-end: gate → (stage | resolve cred+gateway →
    /// commit-confirm apply) → audit → typed result. Never panics; every path is
    /// audited.
    fn process(&self, req: &RouterActionRequest) -> RouterActionResult {
        match pre_apply_decision(req, self.live_enabled) {
            PreApply::Refused { reason, outcome } => {
                self.audit(req, outcome);
                return RouterActionResult::failed(&req.id, reason);
            }
            PreApply::Staged => {
                self.audit(req, "staged-parked");
                return RouterActionResult::staged(
                    &req.id,
                    format!(
                        "{} — recorded + audited; live apply is operator-gated (set {LIVE_ENV}=1)",
                        req.edit.summary()
                    ),
                );
            }
            PreApply::Proceed => {}
        }

        // SAFETY: a node only mutates the router IT sits behind — resolve this
        // node's own primary gateway and require its MAC to match the request.
        let gateway = self.discover_gateway();
        let Some(cand) = gateway.filter(|c| c.mac.eq_ignore_ascii_case(req.appliance_id.trim()))
        else {
            self.audit(req, "error");
            return RouterActionResult::failed(
                &req.id,
                format!(
                    "appliance {} is not {}'s primary gateway — refused",
                    req.appliance_id, self.self_hostname
                ),
            );
        };

        let cred_ref = format!("router/{}", req.appliance_id);
        let Some(cred) = self.secret_store.get(&cred_ref).ok().flatten() else {
            self.audit(req, "error");
            return RouterActionResult::failed(
                &req.id,
                format!("no sealed `{cred_ref}` credential — cannot authenticate to the router"),
            );
        };
        let (user, pass) = router_discovery::parse_router_cred(&cred);
        let plan = ApplyPlan {
            ip: cand.ip,
            user,
            pass,
            commands: req.edit.to_vyatta_commands(),
            confirm_min: clamp_confirm_min(req.commit_confirm_min),
        };
        let outcome = self.apply(&plan);
        let (result, audit_outcome) = map_outcome(&req.id, &req.edit.summary(), &outcome);
        self.audit(req, audit_outcome);
        result
    }

    /// Drain + handle every request addressed to this host, writing each result back.
    fn execute_pending(&self) {
        for req in take_requests(&self.workgroup_root, &self.self_hostname) {
            let result = self.process(&req);
            tracing::info!(
                target: "mackesd::router_action",
                id = %req.id, appliance = %req.appliance_id, action = %req.edit.op.as_str(),
                ok = result.ok, staged = result.staged, reverted = result.reverted,
                "router-action request handled (WL-RUN-006)"
            );
            let _ = write_result(&self.workgroup_root, &self.self_hostname, &result);
        }
    }
}

/// The (operator-gated) LIVE commit-confirm applier — the parked live-smoke seam.
///
/// Mirrors `infra/tofu/edgeos/scripts/apply-firewall.sh`: it opens a Vyatta config
/// session, applies the FIXED lines, `commit-confirm <min>` (auto-revert armed),
/// re-checks SSH reachability, and only `confirm`s + `save`s when the box still
/// answers — otherwise it lets the timer auto-revert (SAFETY 2). The password is
/// fed via `SSHPASS` (never argv). Runs ONLY when `MDE_ROUTER_ACTION_LIVE=1`;
/// unit tests inject a fake applier, so this never touches a live router in CI.
#[must_use]
pub fn live_apply(plan: &ApplyPlan) -> ApplyOutcome {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let ssh_args = |cmd: &str| -> Vec<String> {
        vec![
            "-o".into(),
            "StrictHostKeyChecking=accept-new".into(),
            "-o".into(),
            "PreferredAuthentications=password".into(),
            "-o".into(),
            "PubkeyAuthentication=no".into(),
            "-o".into(),
            "ConnectTimeout=15".into(),
            format!("{}@{}", plan.user, plan.ip),
            cmd.into(),
        ]
    };

    // 1. Apply inside a commit-confirm window.
    let mut script = String::from("source /opt/vyatta/etc/functions/script-template\nconfigure\n");
    for line in &plan.commands {
        script.push_str(line);
        script.push('\n');
    }
    script.push_str(&format!("commit-confirm {}\nexit\n", plan.confirm_min));

    let mut child = match Command::new("sshpass")
        .arg("-e")
        .arg("ssh")
        .args(ssh_args("bash -l"))
        .env("SSHPASS", &plan.pass)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return ApplyOutcome::Error(format!("ssh spawn failed: {e}")),
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(script.as_bytes());
    }
    let out = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => return ApplyOutcome::Error(format!("ssh wait failed: {e}")),
    };
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
    .to_lowercase();
    if combined.contains("commit failed")
        || combined.contains("set failed")
        || combined.contains("delete failed")
        || combined.contains("invalid")
    {
        return ApplyOutcome::Error(
            "router rejected the edit (commit-confirm will auto-revert)".to_string(),
        );
    }

    // 2. Still reachable? Confirm (permanent) + save; else let it auto-revert.
    let reachable = Command::new("sshpass")
        .arg("-e")
        .arg("ssh")
        .args(ssh_args("true"))
        .env("SSHPASS", &plan.pass)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !reachable {
        return ApplyOutcome::Reverted;
    }

    let confirm =
        "source /opt/vyatta/etc/functions/script-template\nconfigure\nconfirm\nsave\nexit\n";
    let mut child = match Command::new("sshpass")
        .arg("-e")
        .arg("ssh")
        .args(ssh_args("bash -l"))
        .env("SSHPASS", &plan.pass)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return ApplyOutcome::Error(format!("confirm ssh spawn failed: {e}")),
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(confirm.as_bytes());
    }
    match child.wait_with_output() {
        Ok(o) => {
            let s = format!(
                "{}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            )
            .to_lowercase();
            if s.contains("fail") || s.contains("error") {
                ApplyOutcome::Error("confirm/save reported an issue".to_string())
            } else {
                ApplyOutcome::Confirmed
            }
        }
        Err(e) => ApplyOutcome::Error(format!("confirm wait failed: {e}")),
    }
}

#[async_trait::async_trait]
impl Worker for RouterActionWorker {
    fn name(&self) -> &'static str {
        "router_action"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        loop {
            self.execute_pending();
            tokio::select! {
                () = shutdown.wait() => return Ok(()),
                () = tokio::time::sleep(POLL) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mackes_mesh_types::router_action::{
        write_request, FirewallEdit, FirewallEditOp, FirewallLeaf,
    };

    fn edit() -> FirewallEdit {
        FirewallEdit {
            ruleset: "WAN_IN".into(),
            rule: "40".into(),
            op: FirewallEditOp::Set,
            default_action: None,
            attrs: vec![
                FirewallLeaf::new("action", "accept"),
                FirewallLeaf::new("protocol", "tcp"),
            ],
        }
    }

    fn req_with(confirm_token: &str) -> RouterActionRequest {
        RouterActionRequest {
            id: "01HZX".into(),
            appliance_id: "46:6a:7c:96:e8:aa".into(),
            target_host: "eagle".into(),
            from: "peer:laptop".into(),
            edit: edit(),
            confirm_token: confirm_token.into(),
            commit_confirm_min: 2,
        }
    }

    fn candidate() -> RouterCandidate {
        RouterCandidate {
            ip: "172.20.0.1".into(),
            mac: "46:6a:7c:96:e8:aa".into(),
            is_default: true,
            oui_hint: Some("ubiquiti".into()),
        }
    }

    /// A real-AEAD local store rooted under `dir` (mirrors the secret_store test
    /// helper) so `router/<mac>` cred round-trips.
    fn seeded_store(dir: &Path) -> SecretStore {
        let key_path = dir.join("mcnf-age-key");
        std::fs::write(
            &key_path,
            "AGE-SECRET-KEY-1QQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQSXKLP0E\n",
        )
        .unwrap();
        SecretStore::LocalAead {
            dir: dir.join("secrets"),
            key_path,
        }
    }

    // ── SAFETY 1 — the verb refuses without a matching typed-confirm token ──────
    #[test]
    fn refuses_without_typed_confirm() {
        // Empty token → refused (never proceeds), regardless of the live gate.
        assert!(matches!(
            pre_apply_decision(&req_with(""), true),
            PreApply::Refused {
                outcome: "refused-typed-confirm",
                ..
            }
        ));
        // Wrong appliance's MAC → still refused.
        assert!(matches!(
            pre_apply_decision(&req_with("aa:bb:cc:dd:ee:ff"), true),
            PreApply::Refused {
                outcome: "refused-typed-confirm",
                ..
            }
        ));
        // The correct token proceeds (live) / stages (parked).
        assert_eq!(
            pre_apply_decision(&req_with("46:6a:7c:96:e8:aa"), true),
            PreApply::Proceed
        );
        assert_eq!(
            pre_apply_decision(&req_with("46:6a:7c:96:e8:aa"), false),
            PreApply::Staged
        );
    }

    #[test]
    fn refuses_malformed_edit_even_with_token() {
        let mut r = req_with("46:6a:7c:96:e8:aa");
        r.edit.attrs = vec![FirewallLeaf::new("description", "boom'; reboot #")];
        assert!(matches!(
            pre_apply_decision(&r, true),
            PreApply::Refused {
                outcome: "refused-invalid",
                ..
            }
        ));
    }

    // ── SAFETY 2 — the commit-confirm auto-revert path ─────────────────────────
    #[test]
    fn outcome_mapping_distinguishes_revert_from_apply_and_error() {
        let (r, tok) = map_outcome("id", "set WAN_IN rule 40", &ApplyOutcome::Confirmed);
        assert!(r.ok && !r.reverted);
        assert_eq!(tok, "applied");

        let (r, tok) = map_outcome("id", "set WAN_IN rule 40", &ApplyOutcome::Reverted);
        assert!(!r.ok && r.reverted);
        assert_eq!(tok, "auto-reverted");

        let (r, tok) = map_outcome(
            "id",
            "set WAN_IN rule 40",
            &ApplyOutcome::Error("nope".into()),
        );
        assert!(!r.ok && !r.reverted && r.error == "nope");
        assert_eq!(tok, "error");
    }

    #[test]
    fn live_lost_reach_auto_reverts_end_to_end() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        write_request(&root, &req_with("46:6a:7c:96:e8:aa")).unwrap();
        let called = Arc::new(AtomicBool::new(false));
        let c2 = called.clone();
        let store = seeded_store(tmp.path());
        store.put("router/46:6a:7c:96:e8:aa", "ubnt:pw").unwrap();
        let w = RouterActionWorker::new(root.clone(), "eagle".into(), "peer:eagle".into())
            .with_secret_store(store)
            .with_live(true)
            .with_gateway(candidate_some)
            .with_applier(move |_plan| {
                c2.store(true, Ordering::SeqCst);
                ApplyOutcome::Reverted
            });
        w.execute_pending();
        assert!(called.load(Ordering::SeqCst), "the live applier ran");
        // The result says reverted, not a fabricated success.
        let res = mackes_mesh_types::router_action::take_result(&root, "eagle", "01HZX").unwrap();
        assert!(!res.ok && res.reverted);
        // The audit chain recorded exactly one `auto-reverted` row.
        let recs = read_audit(&audit_log_path(&root, "eagle"));
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].outcome, "auto-reverted");
    }

    fn candidate_some() -> Option<RouterCandidate> {
        Some(candidate())
    }

    #[test]
    fn typed_confirm_refusal_never_calls_the_applier() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        write_request(&root, &req_with("wrong-token")).unwrap();
        let w = RouterActionWorker::new(root.clone(), "eagle".into(), "peer:eagle".into())
            .with_live(true)
            .with_gateway(candidate_some)
            .with_applier(|_plan| panic!("applier must NOT run on a typed-confirm refusal"));
        w.execute_pending();
        let res = mackes_mesh_types::router_action::take_result(&root, "eagle", "01HZX").unwrap();
        assert!(!res.ok);
        let recs = read_audit(&audit_log_path(&root, "eagle"));
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].outcome, "refused-typed-confirm");
    }

    #[test]
    fn staged_when_live_gate_off_never_calls_the_applier() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        write_request(&root, &req_with("46:6a:7c:96:e8:aa")).unwrap();
        let w = RouterActionWorker::new(root.clone(), "eagle".into(), "peer:eagle".into())
            .with_live(false)
            .with_applier(|_plan| panic!("applier must NOT run when the live gate is off"));
        w.execute_pending();
        let res = mackes_mesh_types::router_action::take_result(&root, "eagle", "01HZX").unwrap();
        assert!(res.staged && !res.ok);
        let recs = read_audit(&audit_log_path(&root, "eagle"));
        assert_eq!(recs[0].outcome, "staged-parked");
    }

    // ── SAFETY 3 — the hash-chain audit detects tampering ──────────────────────
    #[test]
    fn hash_chain_audit_detects_tampering() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for (i, oc) in ["staged-parked", "applied", "auto-reverted"]
            .iter()
            .enumerate()
        {
            append_audit(
                root,
                "eagle",
                "46:6a:7c:96:e8:aa",
                "set",
                "set WAN_IN rule 40",
                oc,
                "peer:laptop",
                1_000 + i as i64,
            )
            .unwrap();
        }
        let path = audit_log_path(root, "eagle");
        // Intact chain of 3.
        assert!(matches!(
            verify_audit(&path),
            VerifyOutcome::Intact { verified: 3, .. }
        ));
        // Tamper: rewrite the middle row's outcome but keep its stored hash.
        let mut recs = read_audit(&path);
        recs[1].outcome = "applied-but-actually-not".into();
        let body: String = recs
            .iter()
            .map(|r| serde_json::to_string(r).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&path, format!("{body}\n")).unwrap();
        // The verifier flags the tampered row (seq 2).
        assert!(matches!(
            verify_audit(&path),
            VerifyOutcome::Break { at_event: 2, .. }
        ));
    }

    #[test]
    fn audit_chain_links_prev_hash_to_the_prior_row() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let r1 = append_audit(root, "eagle", "m", "set", "s", "applied", "f", 1).unwrap();
        let r2 = append_audit(root, "eagle", "m", "set", "s", "applied", "f", 2).unwrap();
        assert_eq!(r1.seq, 1);
        assert_eq!(r2.seq, 2);
        assert_eq!(r2.prev_hash, r1.hash, "each row chains on the prior hash");
        assert_ne!(r1.hash, r2.hash);
    }
}
