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
//! * [`ActionRequest::CodeEdit`] (FRONTDOOR-12) → a typed, **path-bounded** file
//!   write + a FIXED-ARG `git commit`. This is the most sensitive AI capability
//!   (the AI editing code/config), so the safety model is non-negotiable and
//!   matches the others: it APPLIES **only** on an explicit operator-approved
//!   apply request reaching this worker (never auto-applied from a Copilot
//!   proposal — the Copilot emits a `CodeEdit` *proposal* on a DISTINCT topic and
//!   never publishes to [`ACTION_TOPIC`]). The target path is validated to be a
//!   **relative, traversal-free path inside the allowed root** (the workgroup/repo
//!   dir) BEFORE any side effect — an absolute path, a `..` escape, or any path
//!   that resolves outside the root is a typed rejection (audited), bounding the
//!   blast radius. The apply is TYPED, not shell: a `std::fs::write` of the
//!   reviewed content, then `Command::new("git")` with a CLOSED, FIXED arg vector
//!   (`add -- <validated-relpath>` then `commit -m <fixed-prefix+kind> -- <relpath>`)
//!   — the binary is a literal and the path is the validated in-root relpath, NOT
//!   a free-form command string (§9: no `Command::new(<user string>)`, no shell).
//!   The reviewable full content travels in the proposal so the operator sees the
//!   exact change before approving.
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

use std::path::{Component, Path, PathBuf};
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

    /// FRONTDOOR-12 — apply a reviewed **code/config edit** to a single file
    /// inside the allowed root, then commit it with FIXED git args. This is the
    /// AI-editing-code capability: it lands ONLY on an explicit operator-approved
    /// apply request (exactly like `ServiceLifecycle` — never auto-applied from a
    /// Copilot proposal). The `path` is validated to be a relative, traversal-free
    /// path that resolves INSIDE the allowed root before any write; `content` is
    /// the full reviewed file body the operator approved (the proposal carries it
    /// in full so the change is reviewable). There is deliberately no shell / diff
    /// / patch-program escape hatch — the worker writes the typed content and runs
    /// `git` with a closed arg vector (§9).
    CodeEdit {
        /// The edit target, as a path RELATIVE to the allowed root. Validated by
        /// [`validate_edit_path`]: rejected if absolute, if it contains a `..`
        /// component or a root/prefix component, or if it is empty. This bounds
        /// the blast radius to the workgroup/repo dir — `/etc`, `~`, and `../`
        /// escapes can never be written.
        path: String,
        /// The full, reviewed file content to write. This is what the operator saw
        /// and approved in the proposal — applied verbatim (a typed file write),
        /// never interpreted as a command or a patch program.
        content: String,
    },
}

impl ActionRequest {
    /// Stable kind tag for logs + the audit record (matches the serde tag).
    #[must_use]
    pub const fn kind_tag(&self) -> &'static str {
        match self {
            ActionRequest::ServiceLifecycle { .. } => "service_lifecycle",
            ActionRequest::CodeEdit { .. } => "code_edit",
        }
    }
}

/// The commit-message prefix every applied [`ActionRequest::CodeEdit`] carries.
/// Fixed (not operator/AI-supplied) so the `git commit -m` arg is a constant — the
/// only variable part is the validated in-root relpath, appended by the worker.
const CODE_EDIT_COMMIT_PREFIX: &str = "mackesd code-edit (FD-12, operator-approved):";

/// Validate a [`ActionRequest::CodeEdit`] target path and resolve it to the
/// absolute on-disk path INSIDE `root`.
///
/// Pure + unit-testable: this is the path-bound enforcement that runs BEFORE any
/// write. The contract is strict — the path must be:
///
/// * non-empty,
/// * **relative** (an absolute path or a Windows-style prefix/root is rejected —
///   no `/etc`, no drive roots),
/// * free of any `..` (`ParentDir`) component (no traversal escape),
/// * composed only of plain `Normal` components (no bare `.` cur-dir, no root).
///
/// On success it returns `root.join(rel)` — guaranteed lexically within `root`.
/// On any violation it returns a typed rejection reason. We reject on the LEXICAL
/// path (before touching the filesystem) so a symlink race can't widen the bound;
/// the join of a root + a components-only relative path cannot escape the root.
///
/// # Errors
///
/// A human-readable reason suitable for an [`ActionReply`]'s `error` field.
pub fn validate_edit_path(root: &Path, path: &str) -> Result<PathBuf, String> {
    if path.trim().is_empty() {
        return Err("code_edit: empty path".to_string());
    }
    let rel = Path::new(path);
    // Reject anything that is not a pure sequence of normal path segments. This
    // single pass catches absolute paths, `..` traversal, bare `.`, and any
    // root/prefix component — the union of every escape we must bound.
    for comp in rel.components() {
        match comp {
            Component::Normal(_) => {}
            Component::ParentDir => {
                return Err(format!(
                    "code_edit: path `{path}` contains a `..` traversal component (rejected)"
                ));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!(
                    "code_edit: path `{path}` is absolute / has a root component (must be relative to the allowed root)"
                ));
            }
            Component::CurDir => {
                return Err(format!(
                    "code_edit: path `{path}` contains a `.` component (rejected)"
                ));
            }
        }
    }
    let joined = root.join(rel);
    // Belt-and-suspenders: the joined path must still start with the root prefix.
    // A components-only relative join can't escape, but this makes the invariant
    // explicit and survives any future change to the loop above.
    if !joined.starts_with(root) {
        return Err(format!(
            "code_edit: path `{path}` resolves outside the allowed root (rejected)"
        ));
    }
    Ok(joined)
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
pub fn plan_lifecycle(
    req: &ActionRequest,
    ulid: &str,
    from: &str,
) -> Result<LifecycleRequest, String> {
    let ActionRequest::ServiceLifecycle {
        target_host,
        service_kind,
        name,
        op,
    } = req
    else {
        return Err("plan_lifecycle: not a service_lifecycle request".to_string());
    };
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
            ActionRequest::CodeEdit { path, content } => serde_json::json!({
                // The full content is recorded — the audit IS the durable record of
                // exactly what was applied (§8) — alongside the path and a size so
                // the trail is greppable without re-reading the file.
                "path": path,
                "content_len": content.len(),
                "content": content,
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
    /// dispatch it. `ServiceLifecycle` writes a typed `lifecycle::write_request`
    /// (no command). `CodeEdit` applies a path-bounded typed file write + a
    /// FIXED-ARG `git commit` (the only spawned process, a literal binary with a
    /// closed arg vector — never a shell or a command string, §9). A vocabulary
    /// violation, an out-of-bounds path, or an I/O fault becomes a typed rejection.
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
            ActionRequest::CodeEdit { path, content } => self.apply_code_edit(path, content),
        }
    }

    /// Apply an operator-approved [`ActionRequest::CodeEdit`] (FRONTDOOR-12): a
    /// path-bounded, TYPED file write of the reviewed content, then a FIXED-ARG
    /// `git` commit. Reaching here means the explicit apply request already
    /// arrived on [`ACTION_TOPIC`] (the gate) — a Copilot *proposal* never lands
    /// here. Every failure (out-of-bounds path, traversal, write fault, commit
    /// fault) is a typed rejection; `handle_action` audits whatever this returns,
    /// so an apply AND a rejection both write a hash-chain row.
    ///
    /// §9: no shell, no `Command::new(<user string>)`. The only spawned process is
    /// `git` (a literal binary) with a CLOSED arg vector whose only variable is the
    /// validated in-root relpath; the commit message is the fixed
    /// [`CODE_EDIT_COMMIT_PREFIX`] plus the kind tag.
    fn apply_code_edit(&self, path: &str, content: &str) -> ActionReply {
        // 1. PATH BOUND — reject absolute/traversal/out-of-root BEFORE any write.
        let abs = match validate_edit_path(&self.workgroup_root, path) {
            Ok(p) => p,
            Err(reason) => return ActionReply::rejected(reason),
        };
        // 2. TYPED WRITE — create parent dirs inside the root, then write the
        //    reviewed content. Atomic-ish via a sibling tmp + rename so a partial
        //    write never leaves a truncated file under git.
        if let Some(parent) = abs.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return ActionReply::rejected(format!(
                    "code_edit: could not create parent dir for `{path}`: {e}"
                ));
            }
        }
        let tmp = abs.with_extension("mackesd-codeedit.tmp");
        if let Err(e) = std::fs::write(&tmp, content) {
            return ActionReply::rejected(format!("code_edit: write `{path}` failed: {e}"));
        }
        if let Err(e) = std::fs::rename(&tmp, &abs) {
            let _ = std::fs::remove_file(&tmp);
            return ActionReply::rejected(format!("code_edit: finalize `{path}` failed: {e}"));
        }
        // 3. FIXED-ARG GIT COMMIT — stage the one validated relpath, then commit it
        //    with a fixed message. The binary is the literal "git"; the only
        //    variable arg is `path` (already validated in-root). `--` fences the
        //    pathspec so a leading-dash path can't be read as a flag.
        if let Err(reason) = self.git_commit_edit(path) {
            // The file is written but not committed — surface it as a rejection so
            // the operator/audit see the commit didn't land (the write is recorded
            // in the audit summary regardless).
            return ActionReply::rejected(reason);
        }
        ActionReply::ok(format!("applied + committed code edit to `{path}`"))
    }

    /// Stage + commit the single validated relpath with FIXED git args. Returns a
    /// typed rejection reason on any non-zero/spawn failure. §9: `git` is a
    /// literal binary, the arg vector is closed (`add`/`commit`/`-m`/`--`), and the
    /// only data values are the validated relpath + the fixed commit message —
    /// there is no shell and no command string from the request.
    fn git_commit_edit(&self, rel_path: &str) -> Result<(), String> {
        let run = |args: &[&str]| -> Result<(), String> {
            let out = std::process::Command::new("git")
                .current_dir(&self.workgroup_root)
                .args(args)
                .output()
                .map_err(|e| format!("code_edit: `git {}` spawn failed: {e}", args.join(" ")))?;
            if out.status.success() {
                Ok(())
            } else {
                let stderr = String::from_utf8_lossy(&out.stderr);
                Err(format!(
                    "code_edit: `git {}` failed: {}",
                    args.join(" "),
                    stderr.trim()
                ))
            }
        };
        run(&["add", "--", rel_path])?;
        let message = format!("{CODE_EDIT_COMMIT_PREFIX} {rel_path}");
        run(&["commit", "-m", &message, "--", rel_path])
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
            other => panic!("expected ServiceLifecycle, got {other:?}"),
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
        let bad_op =
            parse_action_request(&lifecycle_req("oak", "container", "x", "explode")).unwrap();
        assert!(plan_lifecycle(&bad_op, "u", "f").is_err());
        let bad_kind = parse_action_request(&lifecycle_req("oak", "kernel", "x", "start")).unwrap();
        assert!(plan_lifecycle(&bad_kind, "u", "f").is_err());
    }

    #[test]
    fn plan_lifecycle_rejects_empty_target_and_name() {
        let no_target =
            parse_action_request(&lifecycle_req("", "container", "x", "start")).unwrap();
        assert!(plan_lifecycle(&no_target, "u", "f").is_err());
        let no_name =
            parse_action_request(&lifecycle_req("oak", "container", "", "start")).unwrap();
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
        let reply = w.handle_action(
            "01HZX",
            &lifecycle_req("oak", "container", "nginx", "restart"),
        );
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

    // ===================== FRONTDOOR-12: code-edit apply =====================

    fn code_edit_req(path: &str, content: &str) -> String {
        serde_json::json!({
            "kind": "code_edit",
            "path": path,
            "content": content,
        })
        .to_string()
    }

    /// Init a real git repo in `root` so the FIXED-ARG `git add`/`git commit` the
    /// apply handler runs has somewhere to land. Sets a local identity so commit
    /// doesn't fail on a CI box with no global git config.
    fn git_init(root: &std::path::Path) {
        let run = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .current_dir(root)
                .args(args)
                .output()
                .expect("git spawn");
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "test"]);
        // An initial commit so HEAD exists (not strictly required, but keeps the
        // repo in a normal state).
        std::fs::write(root.join("README"), "seed\n").unwrap();
        run(&["add", "--", "README"]);
        run(&["commit", "-q", "-m", "seed"]);
    }

    #[test]
    fn code_edit_parses_as_allowlisted_kind() {
        // The FD-12 variant deserializes from the wire form FD-11's allowlist
        // gate (parse_action_request) accepts — so the copilot's extract_proposal,
        // which routes through that SAME gate, can now propose a code_edit.
        let req =
            parse_action_request(&code_edit_req("config/app.toml", "x = 1\n")).expect("parse");
        assert_eq!(req.kind_tag(), "code_edit");
        match req {
            ActionRequest::CodeEdit { path, content } => {
                assert_eq!(path, "config/app.toml");
                assert_eq!(content, "x = 1\n");
            }
            other => panic!("expected CodeEdit, got {other:?}"),
        }
    }

    #[test]
    fn validate_edit_path_accepts_in_bounds_relative() {
        let root = Path::new("/srv/workgroup");
        let p = validate_edit_path(root, "config/app.toml").expect("in-bounds");
        assert_eq!(p, Path::new("/srv/workgroup/config/app.toml"));
        // A nested relative path is fine too.
        let p2 = validate_edit_path(root, "a/b/c.rs").expect("nested in-bounds");
        assert!(p2.starts_with(root));
    }

    #[test]
    fn validate_edit_path_rejects_absolute_escape() {
        let root = Path::new("/srv/workgroup");
        let err = validate_edit_path(root, "/etc/passwd").expect_err("absolute must reject");
        assert!(
            err.contains("absolute") || err.contains("root component"),
            "{err}"
        );
    }

    #[test]
    fn validate_edit_path_rejects_parent_traversal() {
        let root = Path::new("/srv/workgroup");
        let err = validate_edit_path(root, "../../etc/shadow").expect_err("traversal must reject");
        assert!(err.contains("traversal") || err.contains(".."), "{err}");
        // Even a traversal that re-enters the root by name is rejected — we bound
        // lexically, before the filesystem, so a symlink race can't widen it.
        assert!(validate_edit_path(root, "config/../../escape").is_err());
    }

    #[test]
    fn validate_edit_path_rejects_empty_and_curdir() {
        let root = Path::new("/srv/workgroup");
        assert!(validate_edit_path(root, "").is_err());
        assert!(validate_edit_path(root, "   ").is_err());
        assert!(validate_edit_path(root, "./config/x").is_err());
    }

    #[test]
    fn apply_in_bounds_writes_commits_and_audits() {
        // The full operator-approved path: an explicit code_edit apply request
        // (reaching the worker = the gate) writes the reviewed content to an
        // in-root path, commits it with FIXED git args, and audits the apply on
        // the hash-chain plane. No shell, no command string anywhere.
        let tmp = tempfile::tempdir().unwrap();
        git_init(tmp.path());
        let db = tmp.path().join("audit.db");
        let w = ActionWorker::new(tmp.path().to_path_buf(), "peer:self".into())
            .with_db_path(db.clone());

        let reply = w.handle_action("01HZX", &code_edit_req("config/app.toml", "answer = 42\n"));
        assert!(reply.ok, "{reply:?}");

        // The file landed with EXACTLY the reviewed content.
        let written = std::fs::read_to_string(tmp.path().join("config/app.toml")).unwrap();
        assert_eq!(written, "answer = 42\n");

        // It was committed (the working tree is clean for that path).
        let status = std::process::Command::new("git")
            .current_dir(tmp.path())
            .args(["status", "--porcelain", "--", "config/app.toml"])
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&status.stdout).trim().is_empty(),
            "edit should be committed (clean status)"
        );

        // And it wrote a hash-chain audit row recording the path + content.
        let conn = crate::store::open(&db).expect("open audit db");
        let rows = crate::store::load_audit_rows(&conn).expect("rows");
        assert_eq!(rows.len(), 1, "one audit row for the apply");
        assert!(matches!(
            crate::audit::verify(&rows),
            crate::audit::VerifyOutcome::Intact { verified: 1, .. }
        ));
        // The audit payload carries the path (reviewable trail, §8).
        let payload = String::from_utf8_lossy(&rows[0].payload);
        assert!(payload.contains("config/app.toml"), "{payload}");
    }

    #[test]
    fn apply_out_of_bounds_absolute_is_rejected_and_audited_no_write() {
        // An absolute path escapes the allowed root → typed rejection, NO file
        // written, and the rejection is still audited (a refused edit is on the
        // chain too).
        let tmp = tempfile::tempdir().unwrap();
        git_init(tmp.path());
        let db = tmp.path().join("audit.db");
        let w = ActionWorker::new(tmp.path().to_path_buf(), "peer:self".into())
            .with_db_path(db.clone());

        let target = tmp.path().join("pwned-marker");
        // Use an absolute path OUTSIDE the root (a tempdir sibling) — the handler
        // must refuse it.
        let outside = tmp.path().parent().unwrap().join("escape.txt");
        let reply = w.handle_action("01OOB", &code_edit_req(outside.to_str().unwrap(), "x"));
        assert!(!reply.ok);
        assert!(
            reply.error.as_deref().unwrap().contains("absolute")
                || reply.error.as_deref().unwrap().contains("root"),
            "{reply:?}"
        );
        assert!(!outside.exists(), "out-of-bounds path must NOT be written");
        assert!(!target.exists());

        // The rejection is audited (one row).
        let conn = crate::store::open(&db).expect("open audit db");
        let rows = crate::store::load_audit_rows(&conn).expect("rows");
        assert_eq!(rows.len(), 1, "a rejected apply is still audited");
        assert!(matches!(
            crate::audit::verify(&rows),
            crate::audit::VerifyOutcome::Intact { verified: 1, .. }
        ));
    }

    #[test]
    fn apply_traversal_is_rejected_and_audited_no_write() {
        // A `..` traversal that would write outside the root → typed rejection,
        // nothing written, audited.
        let tmp = tempfile::tempdir().unwrap();
        git_init(tmp.path());
        let db = tmp.path().join("audit.db");
        let w = ActionWorker::new(tmp.path().to_path_buf(), "peer:self".into())
            .with_db_path(db.clone());

        let escape = tmp.path().parent().unwrap().join("traversed.txt");
        let reply = w.handle_action("01TRV", &code_edit_req("../traversed.txt", "x"));
        assert!(!reply.ok);
        assert!(
            reply.error.as_deref().unwrap().contains("traversal")
                || reply.error.as_deref().unwrap().contains(".."),
            "{reply:?}"
        );
        assert!(!escape.exists(), "traversal target must NOT be written");

        let conn = crate::store::open(&db).expect("open audit db");
        let rows = crate::store::load_audit_rows(&conn).expect("rows");
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn apply_chain_stays_intact_across_mixed_outcomes() {
        // A success + a rejection both audit; the hash-chain stays tamper-intact
        // across both, proving §8 holds for the new code-edit action too.
        let tmp = tempfile::tempdir().unwrap();
        git_init(tmp.path());
        let db = tmp.path().join("audit.db");
        let w = ActionWorker::new(tmp.path().to_path_buf(), "peer:self".into())
            .with_db_path(db.clone());

        let _ = w.handle_action("01A", &code_edit_req("notes.md", "hello\n"));
        let _ = w.handle_action("01B", &code_edit_req("/etc/escape", "no"));
        let conn = crate::store::open(&db).expect("open audit db");
        let rows = crate::store::load_audit_rows(&conn).expect("rows");
        assert_eq!(
            rows.len(),
            2,
            "one row per handled apply (success + rejection)"
        );
        assert!(matches!(
            crate::audit::verify(&rows),
            crate::audit::VerifyOutcome::Intact { verified: 2, .. }
        ));
    }
}
