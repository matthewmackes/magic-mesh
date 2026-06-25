//! FRONTDOOR-11 (backend half) — the mackesd typed **action worker** + audit.
//!
//! The Front Door's confirm-gate (design `docs/design/front-door.md`, Q17 + Q26)
//! says approved operator/Copilot actions run "**via a mackesd action worker
//! (typed, audited)**". This is the EXECUTION half of that: a worker that drains
//! a typed action-request topic, executes each request through an EXISTING typed
//! verb mechanism, writes a **hash-chain audit entry** for it (§8), and replies
//! with a typed result. The GUI confirm-gate / preview-diff UI is a separate
//! GUI-track task — there is no UI here.
//!
//! ## §9 — typed verbs only, NO raw shell channel, ever
//!
//! `AI_GOVERNANCE.md` §9 is load-bearing: "Remote execution is **typed verbs +
//! signed job bundles only — no raw shell channel, ever**." This worker therefore
//! does NOT accept a command string. It accepts a TYPED [`ActionRequest`] enum —
//! an allowlisted action KIND with typed params — and maps each allowlisted KIND
//! onto an EXISTING mackesd verb mechanism. The first cut allowlists exactly one:
//!
//! * [`ActionRequest::ServiceLifecycle`] → the PD-11 [`crate::lifecycle`] verb.
//!   It writes a typed [`crate::lifecycle::LifecycleRequest`] (kind ∈
//!   {container,vm}, op ∈ {start,stop,restart} — both already allowlisted by
//!   `lifecycle::valid_kind`/`valid_op`) to the target's replicated request dir.
//!   The target node's own `lifecycle_exec` worker then validates the name
//!   against **what that box actually offers** and runs the FIXED command plan
//!   (`podman <op> <name>` / `virsh <verb> <name>` — `lifecycle::command_plan`,
//!   the binary is hardcoded, the args come from the closed vocabulary). NO
//!   `Command::new(<user string>)`, NO shell, NO push-SSH — the typed request is
//!   carried by replication and the target runs it locally (§9: "Jobs are …
//!   the target runs locally; no push-SSH").
//!
//! An unknown / disallowed KIND, or one whose typed params fail the existing
//! vocabulary gate, is a typed **rejection** ([`ActionReply::rejected`]) — never a
//! panic, never a fallthrough to a generic executor (there is none).
//!
//! ## Audit — hash-chain, the existing plane (§8)
//!
//! Every executed action is recorded through the EXISTING hash-chained audit plane
//! ([`crate::events::append_and_alert`] → the `events` table, whose rows chain via
//! [`crate::audit::next_hash`] and are tamper-verified by `mackesd audit verify`).
//! We write one [`crate::events::EventKind::AdminAction`] row per request carrying
//! the action KIND, the typed params, the requesting node, and the dispatch
//! outcome. We do NOT invent a new audit format — this is the same chain the
//! reconcile/mesh-router writers append to.
//!
//! ## Leader-coordinated + graceful degrade (Q73 / Q33 / §2)
//!
//! Spawned on every node so failover is seamless, but LEADER-gated on the shared
//! `<workgroup_root>/.mackesd-leader.lock` (the same lock every other leader-gated
//! worker contends on): only the elected node dispatches + audits a request, so a
//! multi-node mesh executes + audits each action exactly once. A non-leader
//! advances its cursor and short-circuits without replying (the elected node
//! answers). Every failure path degrades to a typed reply + a log line — the
//! worker never panics, mirroring `copilot` / `dc_jobs`.

#![cfg(feature = "async-services")]

use std::path::PathBuf;
use std::time::Duration;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;

use crate::lifecycle::{self, LifecycleRequest};

use super::{ShutdownToken, Worker};

/// Bus action topic this worker drains.
///
/// Locked to the canonical `action/<domain>/<verb>` RPC convention (`rpc.rs`,
/// which rejects any topic outside `action/`) so the workbench publishes via the
/// standard RPC caller. `exec` is the domain, `request` the verb.
pub const ACTION_TOPIC: &str = "action/exec/request";

/// Poll cadence on the request topic. An action dispatch is local file I/O +
/// one audit insert (sub-millisecond), so a 400 ms poll keeps latency
/// imperceptible while bounding index-read churn (matches `copilot`).
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(400);

/// A TYPED action request — an allowlisted KIND with typed params. There is
/// deliberately **no `Command(String)` / `Shell(String)` variant**: §9 forbids a
/// raw-shell / arbitrary-command channel, so the only way to add an action is to
/// add a typed variant here backed by an existing verb mechanism.
///
/// `serde` tags the variant by `kind` so the wire form is
/// `{"kind":"service_lifecycle", ...typed params...}`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ActionRequest {
    /// Start/stop/restart an EXISTING service (container or VM) on a target node,
    /// via the PD-11 [`crate::lifecycle`] verb. The params mirror the typed
    /// [`LifecycleRequest`] fields; `service_kind`/`op` are gated by the existing
    /// `lifecycle::valid_kind`/`valid_op` allowlists before anything is written.
    ServiceLifecycle {
        /// The node the action targets (its short hostname — the
        /// `fleet/lifecycle/<target>` dir the executor on that box drains).
        target_host: String,
        /// `container` | `vm` — the typed service kind (allowlisted by
        /// `lifecycle::valid_kind`).
        service_kind: String,
        /// The container/guest name. NOT a command — the target validates it
        /// against its own live probe before acting (no arbitrary passthrough).
        name: String,
        /// `start` | `stop` | `restart` — the typed op (allowlisted by
        /// `lifecycle::valid_op`).
        op: String,
    },
}

impl ActionRequest {
    /// Stable kind tag for logs + the audit record (matches the serde tag).
    #[must_use]
    pub fn kind_tag(&self) -> &'static str {
        match self {
            ActionRequest::ServiceLifecycle { .. } => "service_lifecycle",
        }
    }
}

/// The typed reply published to `reply/<request-ulid>`.
///
/// `ok` mirrors the `dc/*` reply convention (`{"ok":true}` ⇒ success) so the
/// existing `dc_jobs` status classifier and any tile reads it uniformly.
/// `detail` carries a human-readable note on success; `error` is set (and `ok`
/// is false) on a rejection / dispatch-failure degrade path.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ActionReply {
    /// `true` once the typed action was accepted + dispatched through the verb
    /// mechanism (the target then executes + reports via its own result lane).
    pub ok: bool,
    /// Human-readable success note (e.g. "dispatched container restart to oak").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Why the action was rejected / could not be dispatched, on a degrade path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ActionReply {
    /// An accepted + dispatched reply.
    #[must_use]
    pub fn ok(detail: impl Into<String>) -> Self {
        Self {
            ok: true,
            detail: Some(detail.into()),
            error: None,
        }
    }

    /// A typed rejection / degrade reply (unknown KIND, vocabulary violation,
    /// malformed body, or a dispatch I/O fault). Never a panic.
    #[must_use]
    pub fn rejected(reason: impl Into<String>) -> Self {
        Self {
            ok: false,
            detail: None,
            error: Some(reason.into()),
        }
    }

    /// JSON body for the `reply/<ulid>` lane. Infallible — a serialize failure
    /// (impossible for this shape) degrades to a fixed rejection body.
    #[must_use]
    pub fn to_body(&self) -> String {
        serde_json::to_string(self)
            .unwrap_or_else(|_| r#"{"ok":false,"error":"reply encode failed"}"#.to_string())
    }
}

/// Parse a typed action-request JSON body.
///
/// # Errors
///
/// Any serde-json failure surfaces as a `"malformed action request: …"` string
/// suitable for the `error` field of an [`ActionReply`]. A body that is valid
/// JSON but carries an unknown `kind` tag fails here too (serde rejects the
/// untagged variant) — so an un-allowlisted KIND can never reach a dispatcher.
pub fn parse_action_request(body: &str) -> Result<ActionRequest, String> {
    serde_json::from_str(body).map_err(|e| format!("malformed action request: {e}"))
}

/// Validate a parsed request against the existing per-verb vocabulary gates.
/// Pure + unit-testable: this is the allowlist enforcement that runs BEFORE any
/// side effect. Returns the typed [`LifecycleRequest`] to dispatch, or a typed
/// rejection reason.
///
/// For [`ActionRequest::ServiceLifecycle`] the gates are exactly
/// `lifecycle::valid_kind` + `lifecycle::valid_op` — we do not re-derive the
/// allowlist, we reuse the one the executor itself enforces, so the worker can
/// never write a request the executor would refuse.
pub fn plan_lifecycle(req: &ActionRequest, ulid: &str, from: &str) -> Result<LifecycleRequest, String> {
    let ActionRequest::ServiceLifecycle {
        target_host,
        service_kind,
        name,
        op,
    } = req;
    if target_host.trim().is_empty() {
        return Err("service_lifecycle: empty target_host".to_string());
    }
    if name.trim().is_empty() {
        return Err("service_lifecycle: empty name".to_string());
    }
    if !lifecycle::valid_kind(service_kind) {
        return Err(format!(
            "service_lifecycle: kind `{service_kind}` not allowlisted (container|vm)"
        ));
    }
    if !lifecycle::valid_op(op) {
        return Err(format!(
            "service_lifecycle: op `{op}` not allowlisted (start|stop|restart)"
        ));
    }
    Ok(LifecycleRequest {
        // The request ulid IS the lifecycle id, so the requester can correlate
        // the eventual `<id>.result.json` the target writes back.
        id: ulid.to_string(),
        kind: service_kind.clone(),
        name: name.clone(),
        op: op.clone(),
        from: from.to_string(),
    })
}

/// The leader-only typed action worker. Drains [`ACTION_TOPIC`], dispatches each
/// allowlisted typed action through the existing verb mechanism, audits it on the
/// hash-chain plane, and replies. Best-effort + graceful degrade.
pub struct ActionWorker {
    /// Shared leader lock (`<workgroup_root>/.mackesd-leader.lock`) — the same
    /// file every leader-gated worker contends on.
    leader_lock: PathBuf,
    /// Workgroup root — the replicated volume the lifecycle verb writes under
    /// (`<root>/fleet/lifecycle/<target>/`).
    workgroup_root: PathBuf,
    /// This node's id (the lease holder + the `from`/actor on audit records).
    node_id: String,
    /// The hash-chained audit DB (the `events` table). Defaults to
    /// [`crate::default_db_path`]; tests point it at a tempdir.
    db_path: PathBuf,
    /// Request topic poll cadence.
    poll_interval: Duration,
    /// Override the Bus spool root. Tests point this at a tempdir.
    bus_root_override: Option<PathBuf>,
}

impl ActionWorker {
    /// Construct with production defaults: the shared leader lock under
    /// `workgroup_root`, the canonical audit DB path, the default Bus root.
    #[must_use]
    pub fn new(workgroup_root: PathBuf, node_id: String) -> Self {
        Self {
            leader_lock: workgroup_root.join(".mackesd-leader.lock"),
            workgroup_root,
            node_id,
            db_path: crate::default_db_path(),
            poll_interval: DEFAULT_POLL_INTERVAL,
            bus_root_override: None,
        }
    }

    /// Override the audit DB path. Tests point this at a tempdir so the
    /// hash-chain insert is exercised without touching `/var/lib/mde`.
    #[must_use]
    pub fn with_db_path(mut self, p: PathBuf) -> Self {
        self.db_path = p;
        self
    }

    /// Override the Bus spool root. Tests point this at a tempdir.
    #[must_use]
    pub fn with_bus_root(mut self, p: PathBuf) -> Self {
        self.bus_root_override = Some(p);
        self
    }

    /// Override the poll cadence. Tests use a shorter value.
    #[must_use]
    pub const fn with_poll_interval(mut self, d: Duration) -> Self {
        self.poll_interval = d;
        self
    }

    /// Only the directory leader executes actions (Q73; no-fixed-center: any
    /// eligible node can be it, the elected one acts). Reuses the shared leader
    /// lock — synchronous, called once per observed request.
    fn is_leader(&self) -> bool {
        matches!(
            crate::leader::try_acquire(&self.leader_lock, &self.node_id),
            Ok(crate::leader::AcquireResult::Acquired)
        )
    }

    /// Write the hash-chain audit row for one action (request + outcome) through
    /// the EXISTING audit plane. Best-effort: `append_and_alert` logs + swallows
    /// any store fault, so an audit-write hiccup never wedges the action lane.
    fn audit(&self, kind_tag: &str, req_summary: serde_json::Value, outcome: &ActionReply) {
        let detail = serde_json::json!({
            "action": kind_tag,
            "request": req_summary,
            "ok": outcome.ok,
            "detail": outcome.detail,
            "error": outcome.error,
        });
        crate::events::append_and_alert(
            &self.db_path,
            &self.node_id,
            crate::events::EventKind::AdminAction,
            detail,
        );
    }

    /// A compact JSON summary of a typed request for the audit record (the typed
    /// params, not a command — there is no command).
    fn request_summary(req: &ActionRequest) -> serde_json::Value {
        match req {
            ActionRequest::ServiceLifecycle {
                target_host,
                service_kind,
                name,
                op,
            } => serde_json::json!({
                "target_host": target_host,
                "service_kind": service_kind,
                "name": name,
                "op": op,
            }),
        }
    }

    /// Handle one typed action request end-to-end (synchronous): parse → validate
    /// against the existing allowlist → dispatch via the existing verb mechanism →
    /// audit on the hash-chain plane → typed reply. Every branch yields a reply
    /// (success or typed rejection) and audits the executed/attempted action, so
    /// the requester never hangs and the trail is never skipped.
    fn handle_action(&self, ulid: &str, body: &str) -> ActionReply {
        let req = match parse_action_request(body) {
            Ok(r) => r,
            Err(e) => {
                // Un-allowlisted / malformed: a typed rejection, audited as a
                // refused admin action. No dispatcher is ever reached.
                let reply = ActionReply::rejected(e);
                self.audit("unknown", serde_json::json!({ "raw": "rejected" }), &reply);
                return reply;
            }
        };
        let kind_tag = req.kind_tag();
        let summary = Self::request_summary(&req);
        let reply = self.dispatch(ulid, &req);
        self.audit(kind_tag, summary, &reply);
        reply
    }

    /// Map an allowlisted typed request onto its EXISTING verb mechanism and
    /// dispatch it. The ONLY external effect is `lifecycle::write_request` (a
    /// typed file write on the replicated volume) — never a spawned command. A
    /// vocabulary violation or an I/O fault becomes a typed rejection.
    fn dispatch(&self, ulid: &str, req: &ActionRequest) -> ActionReply {
        match req {
            ActionRequest::ServiceLifecycle { target_host, .. } => {
                let plan = match plan_lifecycle(req, ulid, &self.node_id) {
                    Ok(p) => p,
                    Err(reason) => return ActionReply::rejected(reason),
                };
                // The existing PD-11 verb: write a typed LifecycleRequest to the
                // target's replicated request dir. Replication carries it; the
                // target's lifecycle_exec validates the name against its own probe
                // and runs the FIXED command plan locally. No push, no shell.
                match lifecycle::write_request(&self.workgroup_root, target_host, &plan) {
                    Ok(_) => ActionReply::ok(format!(
                        "dispatched {} {} `{}` to {}",
                        plan.op, plan.kind, plan.name, target_host
                    )),
                    Err(e) => ActionReply::rejected(format!(
                        "service_lifecycle: dispatch write failed: {e}"
                    )),
                }
            }
        }
    }
}

fn default_bus_root() -> Option<PathBuf> {
    mde_bus::default_data_dir()
}

#[async_trait::async_trait]
impl Worker for ActionWorker {
    fn name(&self) -> &'static str {
        "action"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let Some(bus_root) = self.bus_root_override.clone().or_else(default_bus_root) else {
            tracing::debug!(target: "mackesd::action", "no bus root; worker idle");
            return Ok(());
        };
        let persist = match Persist::open(bus_root) {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!(
                    target: "mackesd::action",
                    error = %e,
                    "persist open failed; worker idle",
                );
                return Ok(());
            }
        };
        // Seed the cursor at the tail so a restart doesn't replay + re-execute
        // stale action requests (running an old action twice is worse than
        // dropping it on a restart).
        let mut cursor: Option<String> = persist.latest_ulid(ACTION_TOPIC).ok().flatten();
        let mut tick = tokio::time::interval(self.poll_interval);
        // Burn the immediate first tick so we wait a full interval on startup.
        tick.tick().await;
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    // Handling is fully synchronous (a typed file write + a
                    // synchronous audit insert — no `.await`), so unlike copilot
                    // there is no async phase and the `&Persist` borrow is held
                    // for the whole sweep without breaking `Send`.
                    self.sweep(&persist, &mut cursor);
                }
                () = shutdown.wait() => break,
            }
        }
        Ok(())
    }
}

impl ActionWorker {
    /// One poll pass: read net-new requests since `cursor`, advance the cursor,
    /// and — only on the elected leader — handle + reply to each. A non-leader
    /// advances the cursor and replies to nothing (the elected node acts), so
    /// failover is seamless without double-execution.
    fn sweep(&self, persist: &Persist, cursor: &mut Option<String>) {
        let msgs = match persist.list_since(ACTION_TOPIC, cursor.as_deref()) {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!(target: "mackesd::action", error = %e, "list_since failed");
                return;
            }
        };
        for msg in msgs {
            *cursor = Some(msg.ulid.clone());
            if !self.is_leader() {
                tracing::debug!(
                    target: "mackesd::action",
                    ulid = %msg.ulid,
                    "not the leader; skipping action (the elected node executes)",
                );
                continue;
            }
            let body = msg.body.unwrap_or_default();
            let reply = self.handle_action(&msg.ulid, &body);
            tracing::info!(
                target: "mackesd::action",
                ulid = %msg.ulid,
                ok = reply.ok,
                "action handled",
            );
            if let Err(e) = persist.write(
                &reply_topic(&msg.ulid),
                Priority::Default,
                None,
                Some(&reply.to_body()),
            ) {
                tracing::warn!(
                    target: "mackesd::action",
                    ulid = %msg.ulid,
                    error = %e,
                    "reply write failed",
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lifecycle_req(target: &str, kind: &str, name: &str, op: &str) -> String {
        serde_json::json!({
            "kind": "service_lifecycle",
            "target_host": target,
            "service_kind": kind,
            "name": name,
            "op": op,
        })
        .to_string()
    }

    #[test]
    fn action_topic_is_canonical_three_segments() {
        // Locks the action/<domain>/<verb> shape so the workbench RPC caller
        // (which rejects topics outside `action/`) can publish to it.
        assert!(ACTION_TOPIC.starts_with("action/"));
        let parts: Vec<&str> = ACTION_TOPIC.split('/').collect();
        assert_eq!(parts, vec!["action", "exec", "request"]);
    }

    #[test]
    fn parse_allowlisted_service_lifecycle() {
        let req = parse_action_request(&lifecycle_req("oak", "container", "nginx", "restart"))
            .expect("parse");
        assert_eq!(req.kind_tag(), "service_lifecycle");
        match req {
            ActionRequest::ServiceLifecycle {
                target_host,
                service_kind,
                name,
                op,
            } => {
                assert_eq!(target_host, "oak");
                assert_eq!(service_kind, "container");
                assert_eq!(name, "nginx");
                assert_eq!(op, "restart");
            }
        }
    }

    #[test]
    fn parse_rejects_unknown_kind_no_executor_reached() {
        // An un-allowlisted KIND fails to deserialize (serde rejects the tag) —
        // it can never reach a dispatcher. This is the §9 backstop: there is no
        // generic/shell fallthrough.
        let body = serde_json::json!({ "kind": "raw_shell", "cmd": "rm -rf /" }).to_string();
        let err = parse_action_request(&body).expect_err("unknown kind must reject");
        assert!(err.contains("malformed action request"), "{err}");
    }

    #[test]
    fn parse_rejects_garbage() {
        let err = parse_action_request("not json").expect_err("should fail");
        assert!(err.contains("malformed"), "{err}");
    }

    #[test]
    fn plan_lifecycle_accepts_allowlisted_vocabulary() {
        let req = parse_action_request(&lifecycle_req("oak", "vm", "win11", "stop")).unwrap();
        let plan = plan_lifecycle(&req, "01HZX", "peer:self").expect("planned");
        assert_eq!(plan.id, "01HZX");
        assert_eq!(plan.kind, "vm");
        assert_eq!(plan.name, "win11");
        assert_eq!(plan.op, "stop");
        assert_eq!(plan.from, "peer:self");
        // The plan a real executor would accept: command_plan maps it to a FIXED
        // (virsh, [shutdown, win11]) — no free-form command anywhere.
        let (bin, args) = lifecycle::command_plan(&plan).expect("command plan");
        assert_eq!(bin, "virsh");
        assert_eq!(args, ["shutdown", "win11"]);
    }

    #[test]
    fn plan_lifecycle_rejects_bad_op_and_kind() {
        let bad_op = parse_action_request(&lifecycle_req("oak", "container", "x", "explode"))
            .unwrap();
        assert!(plan_lifecycle(&bad_op, "u", "f").is_err());
        let bad_kind =
            parse_action_request(&lifecycle_req("oak", "kernel", "x", "start")).unwrap();
        assert!(plan_lifecycle(&bad_kind, "u", "f").is_err());
    }

    #[test]
    fn plan_lifecycle_rejects_empty_target_and_name() {
        let no_target =
            parse_action_request(&lifecycle_req("", "container", "x", "start")).unwrap();
        assert!(plan_lifecycle(&no_target, "u", "f").is_err());
        let no_name = parse_action_request(&lifecycle_req("oak", "container", "", "start")).unwrap();
        assert!(plan_lifecycle(&no_name, "u", "f").is_err());
    }

    #[test]
    fn reply_ok_and_rejected_shapes() {
        let ok = ActionReply::ok("dispatched");
        let v: serde_json::Value = serde_json::from_str(&ok.to_body()).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["detail"], "dispatched");
        assert!(!v.as_object().unwrap().contains_key("error"));

        let rej = ActionReply::rejected("nope");
        let v: serde_json::Value = serde_json::from_str(&rej.to_body()).unwrap();
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"], "nope");
        assert!(!v.as_object().unwrap().contains_key("detail"));
    }

    #[test]
    fn handle_dispatches_allowlisted_action_writes_lifecycle_request() {
        // The end-to-end allowlisted path: a valid service_lifecycle request is
        // dispatched via the EXISTING verb (a typed file write the target's
        // lifecycle_exec drains), and audited — no shell anywhere.
        let tmp = tempfile::tempdir().unwrap();
        let w = ActionWorker::new(tmp.path().to_path_buf(), "peer:self".into())
            .with_db_path(tmp.path().join("audit.db"));
        let reply =
            w.handle_action("01HZX", &lifecycle_req("oak", "container", "nginx", "restart"));
        assert!(reply.ok, "{reply:?}");
        // The lifecycle verb wrote the typed request the target will consume.
        let got = lifecycle::take_requests(tmp.path(), "oak");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, "01HZX");
        assert_eq!(got[0].name, "nginx");
        assert_eq!(got[0].op, "restart");
        assert_eq!(got[0].from, "peer:self");
    }

    #[test]
    fn handle_rejects_disallowed_action_without_dispatch() {
        // A vocabulary-violating request (valid KIND, bad op) is a typed
        // rejection and writes NO lifecycle request — nothing is dispatched.
        let tmp = tempfile::tempdir().unwrap();
        let w = ActionWorker::new(tmp.path().to_path_buf(), "peer:self".into())
            .with_db_path(tmp.path().join("audit.db"));
        let reply = w.handle_action("01HZX", &lifecycle_req("oak", "container", "x", "explode"));
        assert!(!reply.ok);
        assert!(reply.error.unwrap().contains("not allowlisted"));
        assert!(
            lifecycle::take_requests(tmp.path(), "oak").is_empty(),
            "a rejected action must not dispatch anything"
        );
    }

    #[test]
    fn handle_writes_hash_chain_audit_row_per_action() {
        // Every handled action (accepted OR rejected) appends a tamper-verifiable
        // hash-chain row to the EXISTING events plane.
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("audit.db");
        let w = ActionWorker::new(tmp.path().to_path_buf(), "peer:self".into())
            .with_db_path(db.clone());
        // One accepted + one rejected → two audit rows, an intact chain.
        let _ = w.handle_action("01A", &lifecycle_req("oak", "container", "nginx", "start"));
        let _ = w.handle_action("01B", &lifecycle_req("oak", "container", "x", "explode"));
        let conn = crate::store::open(&db).expect("open audit db");
        let rows = crate::store::load_audit_rows(&conn).expect("rows");
        assert_eq!(rows.len(), 2, "one audit row per handled action");
        assert!(matches!(
            crate::audit::verify(&rows),
            crate::audit::VerifyOutcome::Intact { verified: 2, .. }
        ));
    }

    #[tokio::test]
    async fn worker_exits_on_shutdown_token() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut w = ActionWorker::new(tmp.path().to_path_buf(), "peer:self".into())
            .with_bus_root(tmp.path().join("bus"))
            .with_db_path(tmp.path().join("audit.db"))
            .with_poll_interval(Duration::from_millis(20));
        let (tx, rx) = tokio::sync::watch::channel(false);
        let token = ShutdownToken::from_receiver(rx);
        let _ = tx.send(true);
        let result = tokio::time::timeout(Duration::from_secs(3), w.run(token))
            .await
            .expect("worker must exit on shutdown");
        assert!(result.is_ok());
    }

    #[test]
    fn worker_name_is_locked() {
        let w = ActionWorker::new(PathBuf::from("/tmp/x"), "peer:self".into());
        assert_eq!(w.name(), "action");
    }
}
