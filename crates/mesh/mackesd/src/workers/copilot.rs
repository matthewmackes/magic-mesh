//! FRONTDOOR-9 — the mackesd `copilot` worker (the codex AI backend).
//!
//! The Front Door's AI ("Copilot", design `docs/design/front-door.md`) is a
//! mackesd worker that wraps **openai/codex** (an external binary, pulled at
//! runtime — Q100) and answers natural-language "ask" requests over the Mackes
//! Bus. A workbench tile publishes an [`AskRequest`] to `action/copilot/ask`;
//! this worker reads the sealed codex API key from the leader-managed mesh
//! secret-store, runs `codex exec` non-interactively per request (Q14), captures
//! the model's answer, and publishes an [`AskReply`] to `reply/<request-ulid>`
//! per the Bus RPC convention (`crates/platform/mde-bus/src/rpc.rs`).
//!
//! ## Leader-only (Q73 + Q78)
//!
//! Copilot runs on the **elected leader only** — the AI is concentrated on one
//! node, state follows leadership (the lease is renewed by `leader_election`,
//! and we gate via the same shared `<QNM-Shared>/.mackesd-leader.lock` every
//! other leader-gated worker contends on). Every node spawns the worker (so
//! failover is seamless), but a non-leader drains the topic and short-circuits
//! WITHOUT replying — the new leader picks up subsequent requests. This mirrors
//! `dc_health` / `dc_jobs` leader-gating exactly.
//!
//! ## FRONTDOOR-12 — grounding + typed proposals (this cut)
//!
//! FD-12 turns the FD-9 stub into a grounded, proposing copilot. Two §9-safe
//! increments land here:
//!
//! 1. **Grounding (Q16/Q69/Q95).** Instead of FD-9's static placeholder context,
//!    every codex call is grounded in a COMPACT real-state context assembled from
//!    live state this worker can read cheaply + synchronously — mesh nodes +
//!    roles + health buckets (`store::list_nodes` / `health::HealthReport`), the
//!    current leader lease, and the tail of the hash-chained event log. The dump
//!    is BOUNDED ([`MAX_NODES_IN_CONTEXT`] / [`MAX_EVENTS_IN_CONTEXT`]) — we never
//!    splat unbounded data into a prompt. No process is spawned to gather it (the
//!    full file-content reach of Q95 is deferred with code-edit, below).
//!
//! 2. **Typed proposals (Q53/§9).** When an ask IMPLIES an operation, the worker
//!    parses codex's answer for a typed [`crate::workers::action::ActionRequest`]
//!    (FD-11's allowlisted enum) and PUBLISHES it as a *proposal* on a DISTINCT
//!    topic ([`PROPOSAL_TOPIC`] = `action/copilot/proposal`) — for the operator /
//!    GUI to approve later. It is **never** published to FD-11's execution topic
//!    and the copilot **never** executes it. If codex returns no actionable
//!    proposal, the worker just returns the text answer (FD-9 behavior).
//!
//! Code/config editing (Q52/Q53 diff→apply→git) is the remaining, more sensitive
//! FD-12 piece and is deliberately NOT implemented in this cut.
//!
//! ## Governance — PROPOSE/SUGGEST only, NEVER execute (`AI_GOVERNANCE` §9)
//!
//! Remote OS execution on the mesh is typed verbs + signed job bundles ONLY;
//! there is **no raw shell channel, ever**. This worker therefore *answers,
//! suggests, and PROPOSES* — it spawns the `codex` AI subprocess itself (that is
//! the model, not the user's machine) and returns the model's text, and it may
//! emit a TYPED [`ActionRequest`] proposal on [`PROPOSAL_TOPIC`]. It does NOT run
//! arbitrary system commands, and it does NOT execute the proposal: §9 execution
//! stays exclusively in the gated, audited FD-11 action worker behind operator
//! approval. The copilot's ONLY process spawn remains the `codex` subprocess.
//!
//! ## Graceful degrade (Q33 + §2/§7)
//!
//! "Everything but AI keeps working." When the codex binary is absent (a node
//! without the runtime dependency installed), the API key isn't in the store yet,
//! the key read faults, or codex exits non-zero / times out, the worker NEVER
//! panics: it logs and publishes a reply with `answer: null` + an `error`
//! describing the unavailability, so the requester surfaces "AI unavailable"
//! instead of hanging until the RPC timeout. The rest of the Front Door is
//! unaffected.

#![cfg(feature = "async-services")]

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_bus::rpc::reply_topic;

use crate::ipc::secret_store::{self, SecretStore};
use crate::workers::action::ActionRequest;

use super::{ShutdownToken, Worker};

/// Bus action topic this worker drains.
///
/// Locked to the `action/<domain>/<verb>` Q96 + `rpc.rs` convention so the
/// workbench publishes via the canonical RPC caller (`rpc::publish_request`,
/// which rejects any topic outside `action/`).
pub const ACTION_TOPIC: &str = "action/copilot/ask";

/// Bus topic the copilot PUBLISHES typed action PROPOSALS to (FD-12 §2).
///
/// Deliberately DISTINCT from FD-11's execution topic (`action/exec/request`):
/// a message here is a *proposal* the operator/GUI reviews + approves later, NOT
/// an instruction to execute. The copilot writes here; it never writes to the
/// FD-11 execution topic and never executes a proposal itself (§9). Still under
/// the canonical `action/<domain>/<verb>` namespace so the GUI drains it via the
/// standard RPC reader.
pub const PROPOSAL_TOPIC: &str = "action/copilot/proposal";

/// Cap on the number of mesh nodes named in the grounding context (FD-12 §1).
/// Keeps the dump bounded — a large fleet never splats an unbounded node list
/// into the prompt; we name the first N (deterministic, `node_id`-sorted) and
/// summarize the rest as a count.
pub const MAX_NODES_IN_CONTEXT: usize = 24;

/// Cap on the number of recent events folded into the grounding context.
/// We take the TAIL of the hash-chained event log (the most recent), so the
/// model sees what just happened without the whole history.
pub const MAX_EVENTS_IN_CONTEXT: usize = 8;

/// Poll cadence — the control surface rate (`rpc::CONTROL_POLL_INTERVAL`).
///
/// A Copilot answer takes seconds (a model round-trip), so a 400 ms poll on the
/// request topic adds no perceptible latency while keeping index-read churn low.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(400);

/// Hard ceiling on one `codex exec` invocation.
///
/// A model round-trip is normally a few seconds; this bounds a wedged /
/// network-stalled child so a single ask can't pin the worker forever (it
/// degrades to an "AI unavailable" reply).
pub const DEFAULT_CODEX_TIMEOUT: Duration = Duration::from_secs(120);

/// The codex binary name, looked up on `PATH`.
///
/// The external dependency is pulled at runtime (Q100); absent → graceful
/// degrade.
pub const DEFAULT_CODEX_BIN: &str = "codex";

/// Ask-request payload published to [`ACTION_TOPIC`].
///
/// Only `prompt` is required; `context` is an optional caller-supplied grounding
/// string (e.g. the tile the ask originated from). It is layered ON TOP of the
/// FD-12 §1 live mesh-state grounding the worker assembles itself — the tile
/// context narrows, the mesh context grounds.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AskRequest {
    /// The operator's natural-language question.
    pub prompt: String,
    /// Optional caller-supplied context to ground the answer (e.g. the tile the
    /// ask originated from). Empty when absent.
    #[serde(default)]
    pub context: String,
}

/// Ask-reply payload published to `reply/<request-ulid>`.
///
/// `answer` carries the model's text on success; `error` is set (and `answer` is
/// `None`) on any degrade path so the requester can render "AI unavailable"
/// without timing out. `proposal_published` (FD-12 §2) is set when the ask
/// produced a typed [`ActionRequest`] the copilot published on [`PROPOSAL_TOPIC`]
/// for the operator to approve — so the tile can surface "a proposal is waiting"
/// alongside the prose answer. It is **not** an execution: the proposal is queued
/// for approval, never run by the copilot.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AskReply {
    /// The model's answer text on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    /// Human-readable unavailability description on a degrade path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The kind tag of a typed action proposal published for this ask (e.g.
    /// `"service_lifecycle"`), if codex returned an actionable one. `None` for a
    /// pure text answer (the FD-9 path). The proposal itself lives on
    /// [`PROPOSAL_TOPIC`]; this is just a flag for the requesting tile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal_published: Option<String>,
}

/// Parse an ask-request JSON body.
///
/// # Errors
///
/// Any serde-json failure surfaces as a `"malformed copilot ask: …"` string
/// suitable for use as the `error` field of an [`AskReply`].
pub fn parse_ask_request(body: &str) -> Result<AskRequest, String> {
    serde_json::from_str(body).map_err(|e| format!("malformed copilot ask: {e}"))
}

/// Build a success reply JSON body. `proposal_kind` is the kind tag of a typed
/// [`ActionRequest`] proposal published for this ask (FD-12 §2), or `None` for a
/// pure text answer (FD-9 path).
#[must_use]
pub fn build_answer_reply(answer: String, proposal_kind: Option<String>) -> String {
    let reply = AskReply {
        answer: Some(answer),
        error: None,
        proposal_published: proposal_kind,
    };
    serde_json::to_string(&reply)
        .unwrap_or_else(|_| r#"{"error":"reply encode failed"}"#.to_string())
}

/// Build a degrade reply JSON body — `answer: null` + the unavailability reason.
#[must_use]
pub fn build_unavailable_reply(reason: &str) -> String {
    let reply = AskReply {
        answer: None,
        error: Some(format!("AI unavailable: {reason}")),
        proposal_published: None,
    };
    serde_json::to_string(&reply).unwrap_or_else(|_| r#"{"error":"AI unavailable"}"#.to_string())
}

/// The system-context line prepended to every prompt. Keeps Copilot in its
/// PROPOSE/SUGGEST lane (§9): the model answers, suggests, and may PROPOSE a
/// typed action, but it never executes — execution is a separate gated, audited,
/// operator-approved path. The proposal protocol below is what FD-12 §2 parses
/// out of the answer; the schema mirrors FD-11's allowlisted `ActionRequest`.
const SYSTEM_CONTEXT: &str = "You are Copilot, the operations assistant embedded in the Magic Mesh \
Front Door. You are grounded with a snapshot of live mesh state below. Answer the \
operator's question or suggest a fix concisely. You can read and reason but you \
CANNOT execute any system action from here — actions go through a separate typed, \
audited path that the operator confirms.\n\
If — and only if — your answer implies a concrete operation the operator should \
run, you MAY PROPOSE one typed action by appending, AFTER your prose answer, a \
fenced block exactly of the form:\n\
```action\n\
{\"kind\":\"service_lifecycle\",\"target_host\":\"<node>\",\"service_kind\":\"container|vm\",\"name\":\"<name>\",\"op\":\"start|stop|restart\"}\n\
```\n\
Only the `service_lifecycle` kind is allowlisted; propose nothing else. Omit the \
block entirely when no operation is implied. The proposal is queued for the \
operator to approve — proposing is not executing.";

/// Assemble the prompt handed to `codex exec`: the system context, the live
/// mesh-state grounding, any caller-supplied context, then the operator's
/// question. `mesh_context` is the FD-12 §1 real-state dump (already bounded by
/// the assembler); empty when state is unavailable (graceful degrade — codex
/// still answers, just ungrounded). Pure so the composition is unit-testable
/// without spawning codex.
#[must_use]
pub fn compose_prompt(req: &AskRequest, mesh_context: &str) -> String {
    let mut out = String::new();
    out.push_str(SYSTEM_CONTEXT);
    out.push_str("\n\n");
    if !mesh_context.trim().is_empty() {
        out.push_str("Live mesh state:\n");
        out.push_str(mesh_context.trim());
        out.push_str("\n\n");
    }
    if !req.context.trim().is_empty() {
        out.push_str("Context:\n");
        out.push_str(req.context.trim());
        out.push_str("\n\n");
    }
    out.push_str("Question:\n");
    out.push_str(req.prompt.trim());
    out
}

/// Outcome of one codex invocation, mapped to a reply by the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexOutcome {
    /// The model answered; carries the captured stdout (trimmed).
    Answer(String),
    /// A degrade path — carries the operator-readable reason that becomes the
    /// reply's `AI unavailable: <reason>`.
    Unavailable(String),
}

// ============================== FD-12 §1: grounding ==========================

/// The COMPACT, BOUNDED real-state context (FD-12 §1) the worker reads cheaply +
/// synchronously and feeds to codex as grounding. It is assembled by
/// [`assemble_mesh_context`] from live state — never a placeholder. Each field is
/// independently fillable so one failed read leaves the rest intact (graceful
/// degrade); a fully-empty snapshot renders as an empty string and the prompt is
/// simply ungrounded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MeshContext {
    /// `node_id` of the current leader, if a lease is readable.
    pub leader: Option<String>,
    /// `(name, role, health)` for up to [`MAX_NODES_IN_CONTEXT`] mesh nodes,
    /// `node_id`-sorted. The tail (if any) is summarized as `overflow_nodes`.
    pub nodes: Vec<(String, String, String)>,
    /// Count of nodes elided past the cap (0 when all fit).
    pub overflow_nodes: usize,
    /// Health buckets: `(node_count, healthy, degraded, unreachable)`.
    pub health: (u32, u32, u32, u32),
    /// `true` when the audit hash-chain verified intact.
    pub audit_intact: bool,
    /// Most recent applied config revision, if any.
    pub applied_revision: Option<String>,
    /// One-line summaries of the most recent events (tail of the event log,
    /// newest first), at most [`MAX_EVENTS_IN_CONTEXT`].
    pub recent_events: Vec<String>,
}

impl MeshContext {
    /// Render the snapshot as the bounded plain-text block handed to codex. An
    /// all-default snapshot renders empty (the prompt is then ungrounded).
    #[must_use]
    pub fn render(&self) -> String {
        let mut lines: Vec<String> = Vec::new();
        if let Some(leader) = &self.leader {
            lines.push(format!("Leader: {leader}"));
        }
        let (total, healthy, degraded, unreachable) = self.health;
        if total > 0 {
            lines.push(format!(
                "Health: {total} nodes ({healthy} healthy, {degraded} degraded, {unreachable} unreachable); \
                 audit chain {}",
                if self.audit_intact { "intact" } else { "BROKEN" }
            ));
        }
        if let Some(rev) = &self.applied_revision {
            lines.push(format!("Applied revision: {rev}"));
        }
        if !self.nodes.is_empty() {
            let named = self
                .nodes
                .iter()
                .map(|(name, role, health)| format!("{name}[{role},{health}]"))
                .collect::<Vec<_>>()
                .join(", ");
            let suffix = if self.overflow_nodes > 0 {
                format!(" (+{} more)", self.overflow_nodes)
            } else {
                String::new()
            };
            lines.push(format!("Nodes: {named}{suffix}"));
        }
        if !self.recent_events.is_empty() {
            lines.push(format!("Recent events: {}", self.recent_events.join(" | ")));
        }
        lines.join("\n")
    }
}

/// Summarize one audit/event row into a one-line `kind:node detail…` string for
/// the context. Pure + bounded (the detail is truncated). Best-effort: an
/// unparseable payload becomes a terse `event` line rather than an error.
#[must_use]
fn summarize_event(payload: &[u8]) -> String {
    match serde_json::from_slice::<crate::events::Event>(payload) {
        Ok(ev) => {
            let kind = serde_json::to_string(&ev.kind)
                .unwrap_or_default()
                .trim_matches('"')
                .to_string();
            // A compact detail: the JSON object's keys/values, length-capped so a
            // verbose payload can't blow the context budget. Cap on a CHAR
            // boundary (the detail can hold multibyte UTF-8) so truncation never
            // panics.
            let detail_full = ev.detail.to_string();
            const DETAIL_CAP: usize = 120;
            let detail = if detail_full.len() > DETAIL_CAP {
                let end = (0..=DETAIL_CAP)
                    .rev()
                    .find(|&i| detail_full.is_char_boundary(i))
                    .unwrap_or(0);
                format!("{}…", &detail_full[..end])
            } else {
                detail_full
            };
            format!("{kind}@{} {detail}", ev.node_id)
        }
        Err(_) => "event".to_string(),
    }
}

// =========================== FD-12 §2: typed proposals =======================

/// A typed action PROPOSAL the copilot extracted from codex's answer (FD-12 §2):
/// the FD-11 [`ActionRequest`] enum value plus the human-readable rationale.
/// PUBLISHED on [`PROPOSAL_TOPIC`] for the operator to approve — NEVER executed
/// here. The rationale is the prose answer codex gave alongside the proposal.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ActionProposal {
    /// The typed, allowlisted action FD-11's worker would execute IF approved.
    /// Reusing FD-11's enum means the copilot can only ever propose something
    /// FD-11 is willing to accept — an un-allowlisted kind fails to parse here.
    pub action: ActionRequest,
    /// Why the copilot is proposing this — the operator-facing justification
    /// (the prose part of codex's answer). Carried so the approval UI shows the
    /// reasoning, not just a bare command.
    pub rationale: String,
}

impl ActionProposal {
    /// JSON body for [`PROPOSAL_TOPIC`]. Infallible — a serialize failure
    /// (impossible for this shape) degrades to a fixed marker so a malformed
    /// proposal never wedges the publish.
    #[must_use]
    pub fn to_body(&self) -> String {
        serde_json::to_string(self)
            .unwrap_or_else(|_| r#"{"error":"proposal encode failed"}"#.to_string())
    }
}

/// The fence language tag codex is told to use for a proposal block.
const PROPOSAL_FENCE: &str = "action";

/// Split codex's answer into `(prose, optional typed proposal)` (FD-12 §2).
///
/// The model is instructed (see [`SYSTEM_CONTEXT`]) to append a ```` ```action ````
/// fenced JSON block ONLY when its answer implies a concrete operation. This
/// extracts that block, parses it through FD-11's `parse_action_request` (so an
/// un-allowlisted / malformed kind is silently dropped — NOT proposed), and
/// returns the prose with the block stripped plus the typed proposal. No fence
/// (the FD-9 path) ⇒ `(answer, None)`. Pure + unit-testable; spawns nothing.
#[must_use]
pub fn extract_proposal(answer: &str) -> (String, Option<ActionProposal>) {
    let Some((prose, json)) = split_action_fence(answer) else {
        return (answer.trim().to_string(), None);
    };
    // Parse through FD-11's gate: an unknown/disallowed kind fails here and is
    // dropped (the operator just gets the prose) — the copilot never proposes
    // something outside FD-11's allowlist.
    match crate::workers::action::parse_action_request(&json) {
        Ok(action) => {
            let rationale = if prose.is_empty() {
                "Copilot proposed this action (no prose rationale supplied).".to_string()
            } else {
                prose.clone()
            };
            (prose, Some(ActionProposal { action, rationale }))
        }
        Err(e) => {
            tracing::info!(
                target: "mackesd::copilot",
                error = %e,
                "codex emitted an action block that is not an allowlisted ActionRequest; dropping (prose answer kept)",
            );
            (prose, None)
        }
    }
}

/// Pull the first ```` ```action … ``` ```` fenced block out of `text`, returning
/// `(prose_without_block, block_body)`. `None` when there is no such fence. The
/// prose is everything outside the block, trimmed. Tolerant of the closing fence
/// being the end of the string.
fn split_action_fence(text: &str) -> Option<(String, String)> {
    let open_tag = format!("```{PROPOSAL_FENCE}");
    let open = text.find(&open_tag)?;
    // Body starts after the opening fence line (skip to the newline after the tag).
    let after_tag = open + open_tag.len();
    let body_start = text[after_tag..]
        .find('\n')
        .map(|i| after_tag + i + 1)
        .unwrap_or(text.len());
    // Closing fence: the next ``` at or after body_start.
    let rel_close = text[body_start..].find("```");
    let (json, after_close) = match rel_close {
        Some(rel) => {
            let close = body_start + rel;
            (text[body_start..close].trim().to_string(), close + 3)
        }
        // Unterminated fence — take the rest as the body.
        None => (text[body_start..].trim().to_string(), text.len()),
    };
    let mut prose = String::new();
    prose.push_str(text[..open].trim());
    let tail = text.get(after_close..).unwrap_or("").trim();
    if !tail.is_empty() {
        if !prose.is_empty() {
            prose.push('\n');
        }
        prose.push_str(tail);
    }
    Some((prose.trim().to_string(), json))
}

/// The leader-only Copilot worker. Drains [`ACTION_TOPIC`], runs codex per
/// request on the elected leader, and replies. Best-effort + graceful degrade.
pub struct CopilotWorker {
    /// Shared leader lock (`<workgroup_root>/.mackesd-leader.lock`) — the same
    /// file every leader-gated worker contends on.
    leader_lock: PathBuf,
    /// This node's id, for the leader lease.
    node_id: String,
    /// Deployed repo root, for [`SecretStore::resolve`] (where the mesh secret
    /// helper lives). NOT the process cwd (`/` under systemd).
    repo_dir: PathBuf,
    /// Workgroup root, for the local-AEAD secret-store fallback.
    workgroup_root: PathBuf,
    /// Request topic poll cadence.
    poll_interval: Duration,
    /// Per-invocation codex timeout.
    codex_timeout: Duration,
    /// The `codex` binary name/path (PATH-resolved). Tests inject a stub.
    codex_bin: String,
    /// Override the Bus spool root. Tests point this at a tempdir.
    bus_root_override: Option<PathBuf>,
    /// The hash-chained store DB path (the `nodes` + `events` tables) read to
    /// assemble the FD-12 grounding context. Defaults to [`crate::default_db_path`];
    /// tests point it at a tempdir. A read-only grounding read — never written.
    db_path: PathBuf,
}

impl CopilotWorker {
    /// Construct with production defaults: the shared leader lock under
    /// `workgroup_root`, the repo-root + workgroup-root secret-store resolution,
    /// and the PATH `codex` binary.
    #[must_use]
    pub fn new(workgroup_root: PathBuf, node_id: String) -> Self {
        Self {
            leader_lock: workgroup_root.join(".mackesd-leader.lock"),
            node_id,
            repo_dir: secret_store::repo_root(),
            workgroup_root,
            poll_interval: DEFAULT_POLL_INTERVAL,
            codex_timeout: DEFAULT_CODEX_TIMEOUT,
            codex_bin: DEFAULT_CODEX_BIN.to_string(),
            bus_root_override: None,
            db_path: crate::default_db_path(),
        }
    }

    /// Override the grounding store DB path. Tests point this at a tempdir so the
    /// FD-12 grounding read is exercised without touching `/var/lib/mde`.
    #[must_use]
    pub fn with_db_path(mut self, p: PathBuf) -> Self {
        self.db_path = p;
        self
    }

    /// Override the codex binary (tests inject a stub script; a non-existent
    /// name exercises the absent-binary degrade path).
    #[must_use]
    pub fn with_codex_bin(mut self, name: impl Into<String>) -> Self {
        self.codex_bin = name.into();
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

    /// Override the per-invocation codex timeout. Tests shorten it.
    #[must_use]
    pub const fn with_codex_timeout(mut self, d: Duration) -> Self {
        self.codex_timeout = d;
        self
    }

    /// Only the directory leader serves Copilot (Q73: leader-only; no-fixed-
    /// center: any eligible node can be it, the elected one answers). Reuses the
    /// shared leader lock — synchronous, called once per tick.
    fn is_leader(&self) -> bool {
        matches!(
            crate::leader::try_acquire(&self.leader_lock, &self.node_id),
            Ok(crate::leader::AcquireResult::Acquired)
        )
    }

    /// Resolve the codex API key from the mesh secret-store. `Ok(Some(key))` when
    /// sealed + distributed; `Ok(None)` when genuinely absent (not set yet);
    /// `Err` on a real store fault. The three-way contract is preserved so an
    /// absent key (operator hasn't sealed it) reads differently from a broken
    /// store.
    fn read_codex_key(&self) -> Result<Option<String>, String> {
        let store = SecretStore::resolve(&self.repo_dir, &self.workgroup_root);
        store.get(&secret_store::codex_creds_ref())
    }

    /// Assemble the FD-12 §1 grounding context from live state this worker can
    /// read cheaply + SYNCHRONOUSLY — no process spawn, no network. Reads the
    /// `nodes`/`events` tables of the store and the leader lockfile, and returns a
    /// BOUNDED [`MeshContext`]. Every read is independent + best-effort: a missing
    /// store or a failed query leaves that field at its default, so grounding
    /// degrades to "less context" (or empty) rather than failing the ask. The
    /// `&Connection` borrow is fully contained here — never held across an
    /// `.await` (this is a synchronous call the async `handle_ask` invokes BEFORE
    /// awaiting codex).
    fn assemble_mesh_context(&self) -> MeshContext {
        let mut ctx = MeshContext::default();
        // Leader lease — a pure lockfile read.
        ctx.leader = crate::leader::read_current_lease(&self.leader_lock).map(|l| l.node_id);
        let Ok(conn) = crate::store::open(&self.db_path) else {
            // No store yet (fresh node) → just the leader line, if any.
            return ctx;
        };
        // Health buckets + audit-intact + applied revision: the existing store
        // aggregation (`HealthReport::from_store`), reused rather than re-derived.
        let health = crate::health::HealthReport::from_store(&conn);
        ctx.health = (
            health.node_count,
            health.healthy_nodes,
            health.degraded_nodes,
            health.unreachable_nodes,
        );
        ctx.audit_intact = health.audit_chain_intact;
        ctx.applied_revision = health.applied_revision;
        // Nodes (name/role/health), bounded to the cap; the remainder is counted.
        if let Ok(nodes) = crate::store::list_nodes(&conn) {
            let total = nodes.len();
            ctx.nodes = nodes
                .into_iter()
                .take(MAX_NODES_IN_CONTEXT)
                .map(|n| (n.name, n.role, n.health))
                .collect();
            ctx.overflow_nodes = total.saturating_sub(ctx.nodes.len());
        }
        // Recent events: the TAIL of the hash-chained log (newest first), capped.
        if let Ok(rows) = crate::store::load_audit_rows(&conn) {
            ctx.recent_events = rows
                .iter()
                .rev()
                .take(MAX_EVENTS_IN_CONTEXT)
                .map(|r| summarize_event(&r.payload))
                .collect();
        }
        ctx
    }

    /// Run `codex exec` for one composed prompt with the API key in the
    /// environment, capturing stdout. Async (`tokio::process`) so a model round-
    /// trip doesn't pin the runtime thread; bounded by [`Self::codex_timeout`].
    /// Every failure mode degrades to [`CodexOutcome::Unavailable`] — never an
    /// error return, never a panic.
    async fn invoke_codex(&self, api_key: &str, prompt: &str) -> CodexOutcome {
        let mut cmd = tokio::process::Command::new(&self.codex_bin);
        cmd.arg("exec")
            .arg(prompt)
            // codex reads OPENAI_API_KEY from the environment; the sealed mesh
            // secret never lands on a command line or on disk.
            .env("OPENAI_API_KEY", api_key)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                // The common degrade: the external dependency isn't installed.
                tracing::info!(
                    target: "mackesd::copilot",
                    bin = %self.codex_bin,
                    error = %e,
                    "codex binary not invocable; degrading to AI-unavailable",
                );
                return CodexOutcome::Unavailable(format!("codex not available: {e}"));
            }
        };
        let output = match tokio::time::timeout(self.codex_timeout, child.wait_with_output()).await
        {
            Ok(Ok(o)) => o,
            Ok(Err(e)) => {
                tracing::warn!(target: "mackesd::copilot", error = %e, "codex wait failed");
                return CodexOutcome::Unavailable(format!("codex wait failed: {e}"));
            }
            Err(_) => {
                tracing::warn!(
                    target: "mackesd::copilot",
                    timeout_s = self.codex_timeout.as_secs(),
                    "codex exec timed out",
                );
                return CodexOutcome::Unavailable(format!(
                    "codex timed out after {}s",
                    self.codex_timeout.as_secs()
                ));
            }
        };
        interpret_codex_output(output.status.success(), &output.stdout, &output.stderr)
    }

    /// Handle one ask request end-to-end: parse → ground with live mesh state →
    /// read key → invoke codex → split prose from a typed proposal. Returns
    /// `(reply_body, proposal_body)`: the JSON reply to publish on `reply/<ulid>`,
    /// and — when codex proposed a typed [`ActionRequest`] — the JSON proposal to
    /// publish on [`PROPOSAL_TOPIC`] (caller publishes it in the sync phase so no
    /// `&Persist` borrow is held across this `.await`). Every branch yields a
    /// reply (success or degrade) so the requester never hangs. The copilot only
    /// PROPOSES; it never executes (§9).
    async fn handle_ask(&self, body: &str) -> (String, Option<String>) {
        let req = match parse_ask_request(body) {
            Ok(r) => r,
            Err(e) => return (build_unavailable_reply(&e), None),
        };
        let api_key = match self.read_codex_key() {
            Ok(Some(k)) => k,
            Ok(None) => {
                tracing::info!(
                    target: "mackesd::copilot",
                    "codex API key not in the secret-store yet; degrading to AI-unavailable",
                );
                return (
                    build_unavailable_reply("codex API key not sealed in the mesh store"),
                    None,
                );
            }
            Err(e) => {
                tracing::warn!(target: "mackesd::copilot", error = %e, "secret-store read faulted");
                return (
                    build_unavailable_reply(&format!("secret-store fault: {e}")),
                    None,
                );
            }
        };
        // FD-12 §1 — ground codex with a bounded, real mesh-state snapshot (a
        // synchronous read; the `&Connection`/`&Persist` borrows do NOT cross the
        // codex `.await`).
        let mesh_context = self.assemble_mesh_context().render();
        let prompt = compose_prompt(&req, &mesh_context);
        match self.invoke_codex(&api_key, &prompt).await {
            CodexOutcome::Answer(a) => {
                // FD-12 §2 — split the prose answer from any typed proposal. A
                // proposal is published on PROPOSAL_TOPIC for the operator to
                // approve; it is NEVER executed here.
                let (prose, proposal) = extract_proposal(&a);
                match proposal {
                    Some(p) => {
                        let kind = p.action.kind_tag().to_string();
                        tracing::info!(
                            target: "mackesd::copilot",
                            kind = %kind,
                            "copilot produced a typed action PROPOSAL (queued for operator approval; not executed)",
                        );
                        (build_answer_reply(prose, Some(kind)), Some(p.to_body()))
                    }
                    None => (build_answer_reply(prose, None), None),
                }
            }
            CodexOutcome::Unavailable(reason) => (build_unavailable_reply(&reason), None),
        }
    }
}

/// Map a finished codex invocation to a [`CodexOutcome`].
///
/// Pure — split out so the success / non-zero / empty-stdout branches are
/// unit-testable without spawning codex. Non-zero exit or empty stdout degrade to
/// `Unavailable`.
#[must_use]
pub fn interpret_codex_output(success: bool, stdout: &[u8], stderr: &[u8]) -> CodexOutcome {
    if !success {
        let err = String::from_utf8_lossy(stderr);
        let tail: String = err.trim().chars().rev().take(200).collect::<String>();
        let tail: String = tail.chars().rev().collect();
        return CodexOutcome::Unavailable(format!("codex exited non-zero: {tail}"));
    }
    let answer = String::from_utf8_lossy(stdout).trim().to_string();
    if answer.is_empty() {
        return CodexOutcome::Unavailable("codex produced no output".to_string());
    }
    CodexOutcome::Answer(answer)
}

fn default_bus_root() -> Option<PathBuf> {
    mde_bus::default_data_dir()
}

#[async_trait::async_trait]
impl Worker for CopilotWorker {
    fn name(&self) -> &'static str {
        "copilot"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let Some(bus_root) = self.bus_root_override.clone().or_else(default_bus_root) else {
            tracing::debug!(target: "mackesd::copilot", "no bus root; worker idle");
            return Ok(());
        };
        let persist = match Persist::open(bus_root) {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!(
                    target: "mackesd::copilot",
                    error = %e,
                    "persist open failed; worker idle",
                );
                return Ok(());
            }
        };
        // Seed the cursor at the tail so a restart doesn't replay stale asks
        // (an old question answered twice is worse than dropped on a restart).
        let mut cursor: Option<String> = persist.latest_ulid(ACTION_TOPIC).ok().flatten();
        let mut tick = tokio::time::interval(self.poll_interval);
        // Burn the immediate first tick so we wait a full interval on startup.
        tick.tick().await;
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    // `Persist` is `!Sync` (it wraps a rusqlite `RefCell`), so a
                    // `&Persist` may NOT be held across the codex `.await` or the
                    // `Worker::run` future stops being `Send` (which the
                    // supervisor requires). The sweep is therefore split into
                    // three phases, with the persist BORROW dropped before the
                    // async phase: (1) sync — collect net-new asks + advance the
                    // cursor; (2) async — run codex with NO persist borrow live;
                    // (3) sync — write the replies back. No dedicated OS thread
                    // or `Arc<Mutex<Persist>>` needed; `persist` is owned by this
                    // (`Send`) future the whole time, only its `&` is scoped.
                    let asks = self.collect_asks(&persist, &mut cursor);
                    // (ulid, reply_body, optional proposal_body). The proposal is
                    // published in the sync phase below so no `&Persist` borrow is
                    // ever held across the codex `.await`.
                    let mut replies: Vec<(String, String, Option<String>)> =
                        Vec::with_capacity(asks.len());
                    for (ulid, body) in asks {
                        let (reply, proposal) = self.handle_ask(&body).await;
                        replies.push((ulid, reply, proposal));
                    }
                    write_replies(&persist, replies);
                }
                () = shutdown.wait() => break,
            }
        }
        Ok(())
    }
}

impl CopilotWorker {
    /// Phase 1 (sync): read net-new asks since `cursor`, advance the cursor, and
    /// return the `(ulid, body)` pairs this leader should answer. A non-leader
    /// advances the cursor and returns nothing (the elected node answers) so
    /// failover is seamless without double-answers. The `&Persist` borrow is
    /// fully contained in this synchronous call — never held across an `.await`.
    fn collect_asks(
        &self,
        persist: &Persist,
        cursor: &mut Option<String>,
    ) -> Vec<(String, String)> {
        let msgs = match persist.list_since(ACTION_TOPIC, cursor.as_deref()) {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!(target: "mackesd::copilot", error = %e, "list_since failed");
                return Vec::new();
            }
        };
        let mut collected = Vec::new();
        for msg in msgs {
            *cursor = Some(msg.ulid.clone());
            if !self.is_leader() {
                tracing::debug!(
                    target: "mackesd::copilot",
                    ulid = %msg.ulid,
                    "not the leader; skipping ask (the elected node answers)",
                );
                continue;
            }
            collected.push((msg.ulid, msg.body.unwrap_or_default()));
        }
        collected
    }
}

/// Phase 3 (sync): write each `(ulid, reply, proposal)` back. The reply goes on
/// `reply/<ulid>`; a present proposal is published on [`PROPOSAL_TOPIC`] for the
/// operator/GUI to approve (a fresh ULID is minted by `persist.write` — it is a
/// queued proposal, NOT a reply, and is NOT FD-11's execution topic, so it is
/// never executed). Free function (not a method) so the `&Persist` borrow is
/// unambiguously scoped to this synchronous call — never live across the codex
/// `.await`.
fn write_replies(persist: &Persist, replies: Vec<(String, String, Option<String>)>) {
    for (ulid, reply, proposal) in replies {
        if let Err(e) = persist.write(&reply_topic(&ulid), Priority::Default, None, Some(&reply)) {
            tracing::warn!(
                target: "mackesd::copilot",
                ulid = %ulid,
                error = %e,
                "reply write failed",
            );
        }
        if let Some(proposal_body) = proposal {
            // Publish the typed proposal for operator approval. PROPOSAL_TOPIC is
            // deliberately NOT FD-11's `action/exec/request` — nothing here
            // executes it; the GUI/operator must approve and re-publish to FD-11.
            if let Err(e) = persist.write(
                PROPOSAL_TOPIC,
                Priority::Default,
                None,
                Some(&proposal_body),
            ) {
                tracing::warn!(
                    target: "mackesd::copilot",
                    ulid = %ulid,
                    error = %e,
                    "proposal publish failed",
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_topic_is_canonical_three_segments() {
        // Locks the action/<domain>/<verb> shape so the workbench RPC caller
        // (which rejects topics outside `action/`) can publish to it.
        assert!(ACTION_TOPIC.starts_with("action/"));
        let parts: Vec<&str> = ACTION_TOPIC.split('/').collect();
        assert_eq!(parts, vec!["action", "copilot", "ask"]);
    }

    #[test]
    fn parse_ask_happy_path() {
        let req = parse_ask_request(r#"{"prompt":"why is node anvil down?"}"#).expect("parse");
        assert_eq!(req.prompt, "why is node anvil down?");
        assert!(req.context.is_empty());
    }

    #[test]
    fn parse_ask_carries_optional_context() {
        let req =
            parse_ask_request(r#"{"prompt":"explain","context":"alerts tile"}"#).expect("parse");
        assert_eq!(req.context, "alerts tile");
    }

    #[test]
    fn parse_ask_rejects_garbage() {
        let err = parse_ask_request("not json").expect_err("should fail");
        assert!(err.contains("malformed"), "{err}");
    }

    #[test]
    fn compose_prompt_includes_system_context_and_question() {
        let req = AskRequest {
            prompt: "  what's wedged?  ".into(),
            context: String::new(),
        };
        let p = compose_prompt(&req, "");
        assert!(p.contains("You are Copilot"));
        assert!(p.contains("Question:"));
        assert!(
            p.ends_with("what's wedged?"),
            "prompt trimmed + appended: {p:?}"
        );
        // No caller context → no Context block; empty grounding → no mesh block.
        assert!(!p.contains("Context:"));
        assert!(!p.contains("Live mesh state:"));
    }

    #[test]
    fn compose_prompt_includes_context_block_when_present() {
        let req = AskRequest {
            prompt: "explain".into(),
            context: "alerts tile, disk_full on anvil".into(),
        };
        let p = compose_prompt(&req, "");
        assert!(p.contains("Context:"));
        assert!(p.contains("alerts tile, disk_full on anvil"));
    }

    #[test]
    fn compose_prompt_includes_grounded_mesh_state() {
        // FD-12 §1 — the grounding block is woven into the prompt before the
        // question so codex reasons over real state.
        let req = AskRequest {
            prompt: "is anvil ok?".into(),
            context: String::new(),
        };
        let p = compose_prompt(&req, "Leader: peer:oak\nNodes: anvil[peer,degraded]");
        assert!(p.contains("Live mesh state:"));
        assert!(p.contains("Leader: peer:oak"));
        assert!(p.contains("anvil[peer,degraded]"));
        // The proposal protocol is taught to the model.
        assert!(p.contains("```action"));
        assert!(p.contains("service_lifecycle"));
    }

    #[test]
    fn answer_reply_shape_carries_answer_no_error() {
        let body = build_answer_reply("the mfsmaster is down; restart it".into(), None);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["answer"], "the mfsmaster is down; restart it");
        assert!(!v.as_object().unwrap().contains_key("error"));
        // No proposal flag for a pure text answer.
        assert!(!v.as_object().unwrap().contains_key("proposal_published"));
    }

    #[test]
    fn answer_reply_carries_proposal_flag_when_published() {
        let body = build_answer_reply("restarting nginx".into(), Some("service_lifecycle".into()));
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["proposal_published"], "service_lifecycle");
    }

    #[test]
    fn unavailable_reply_shape_carries_error_no_answer() {
        let body = build_unavailable_reply("codex not available: No such file");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(v["error"].as_str().unwrap().starts_with("AI unavailable:"));
        assert!(!v.as_object().unwrap().contains_key("answer"));
    }

    #[test]
    fn interpret_success_with_output_is_answer() {
        let out = interpret_codex_output(true, b"  restart mfsmaster  \n", b"");
        assert_eq!(out, CodexOutcome::Answer("restart mfsmaster".to_string()));
    }

    #[test]
    fn interpret_success_empty_stdout_degrades() {
        let out = interpret_codex_output(true, b"   \n", b"");
        assert_eq!(
            out,
            CodexOutcome::Unavailable("codex produced no output".to_string())
        );
    }

    #[test]
    fn interpret_non_zero_exit_degrades_with_stderr_tail() {
        let out = interpret_codex_output(false, b"", b"auth error: bad key");
        match out {
            CodexOutcome::Unavailable(r) => {
                assert!(r.contains("non-zero"), "{r}");
                assert!(r.contains("bad key"), "{r}");
            }
            CodexOutcome::Answer(_) => panic!("non-zero exit must degrade"),
        }
    }

    #[tokio::test]
    async fn invoke_codex_absent_binary_degrades_gracefully() {
        // The external dependency isn't installed → Unavailable, never a panic.
        let w = CopilotWorker::new(PathBuf::from("/tmp/wg-copilot-test"), "n1".into())
            .with_codex_bin("definitely-not-a-real-codex-binary-xyz");
        let out = w.invoke_codex("fake-key", "hi").await;
        match out {
            CodexOutcome::Unavailable(r) => assert!(r.contains("codex not available"), "{r}"),
            CodexOutcome::Answer(_) => panic!("absent binary must degrade"),
        }
    }

    #[tokio::test]
    async fn invoke_codex_captures_stub_stdout_as_answer() {
        // A stub standing in for codex: `printf` echoes a fixed answer. Proves
        // the real spawn → capture → answer path (codex exec is invoked for
        // real; only the binary is a stub here).
        let w = CopilotWorker::new(PathBuf::from("/tmp/wg-copilot-test"), "n1".into())
            .with_codex_bin("printf");
        // printf "exec" "<prompt>" — argv[1] ("exec") is the format string, so
        // it prints "exec" with no newline → captured + trimmed as the answer.
        let out = w.invoke_codex("k", "p").await;
        assert_eq!(out, CodexOutcome::Answer("exec".to_string()));
    }

    #[tokio::test]
    async fn worker_exits_on_shutdown_token() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut w = CopilotWorker::new(tmp.path().to_path_buf(), "n1".into())
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

    // ===================== FD-12 §1 — grounding =====================

    #[test]
    fn proposal_topic_is_distinct_from_execution_topic() {
        // §9 backstop: the copilot publishes PROPOSALS on its own topic, NEVER
        // FD-11's execution topic — there is no path from a copilot proposal to
        // execution without operator approval + a re-publish to FD-11.
        assert_eq!(PROPOSAL_TOPIC, "action/copilot/proposal");
        assert_ne!(PROPOSAL_TOPIC, crate::workers::action::ACTION_TOPIC);
        assert!(PROPOSAL_TOPIC.starts_with("action/"));
    }

    #[test]
    fn mesh_context_renders_empty_when_default() {
        // Graceful degrade: no readable state → empty grounding (the prompt is
        // simply ungrounded, codex still answers).
        assert!(MeshContext::default().render().is_empty());
    }

    #[test]
    fn mesh_context_renders_bounded_fields() {
        let ctx = MeshContext {
            leader: Some("peer:oak".into()),
            nodes: vec![
                ("anvil".into(), "peer".into(), "degraded".into()),
                ("oak".into(), "host".into(), "healthy".into()),
            ],
            overflow_nodes: 3,
            health: (5, 3, 1, 1),
            audit_intact: true,
            applied_revision: Some("r-2026-06-25-0007".into()),
            recent_events: vec!["lifecycle@anvil {\"action\":\"degraded\"}".into()],
        };
        let s = ctx.render();
        assert!(s.contains("Leader: peer:oak"));
        assert!(s.contains("5 nodes (3 healthy, 1 degraded, 1 unreachable)"));
        assert!(s.contains("audit chain intact"));
        assert!(s.contains("Applied revision: r-2026-06-25-0007"));
        assert!(s.contains("anvil[peer,degraded]"));
        assert!(
            s.contains("(+3 more)"),
            "overflow summarized, not dumped: {s}"
        );
        assert!(s.contains("Recent events:"));
    }

    #[test]
    fn assemble_mesh_context_reads_live_store_state() {
        // FD-12 §1 end-to-end: seed a store with nodes + an event, then assemble
        // the grounding from the REAL store — proves it's not a placeholder.
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("mded.db");
        {
            let mut conn = crate::store::open(&db).unwrap();
            // upsert_node defaults role='peer'/health='unknown'; set them so the
            // grounding reflects real role + health buckets.
            crate::store::upsert_node(&conn, "peer:anvil", "anvil", "pk1", None).unwrap();
            crate::store::set_node_health(&conn, "peer:anvil", "degraded").unwrap();
            crate::store::upsert_node(&conn, "peer:oak", "oak", "pk2", None).unwrap();
            crate::store::set_node_role(&conn, "peer:oak", "host").unwrap();
            crate::store::set_node_health(&conn, "peer:oak", "healthy").unwrap();
            crate::events::append_event(
                &mut conn,
                "peer:anvil",
                crate::events::EventKind::Lifecycle,
                serde_json::json!({"action": "degraded"}),
            )
            .unwrap();
        }
        let w = CopilotWorker::new(tmp.path().to_path_buf(), "peer:oak".into()).with_db_path(db);
        let ctx = w.assemble_mesh_context();
        assert_eq!(ctx.health.0, 2, "two nodes read from the live store");
        assert_eq!(ctx.health.1, 1, "one healthy");
        assert_eq!(ctx.health.2, 1, "one degraded");
        assert!(
            ctx.audit_intact,
            "freshly-appended event leaves chain intact"
        );
        assert_eq!(ctx.nodes.len(), 2);
        assert!(ctx
            .nodes
            .iter()
            .any(|(n, r, _)| n == "anvil" && r == "peer"));
        assert!(ctx.nodes.iter().any(|(n, r, _)| n == "oak" && r == "host"));
        assert!(
            !ctx.recent_events.is_empty(),
            "the appended event is grounded"
        );
        assert!(ctx.recent_events[0].contains("lifecycle"));
        // Renders to a non-empty grounding string.
        assert!(!ctx.render().is_empty());
    }

    #[test]
    fn assemble_mesh_context_degrades_on_absent_store() {
        // No store file at all → no panic, just the leader line (none here) and
        // empty buckets. Graceful degrade.
        let tmp = tempfile::tempdir().unwrap();
        let w = CopilotWorker::new(tmp.path().to_path_buf(), "n1".into())
            .with_db_path(tmp.path().join("does-not-exist.db"));
        let ctx = w.assemble_mesh_context();
        assert_eq!(ctx.health.0, 0);
        assert!(ctx.nodes.is_empty());
    }

    // ===================== FD-12 §2 — typed proposals =====================

    fn answer_with_proposal() -> String {
        // The shape codex is instructed to emit: prose, then a fenced block.
        "nginx on oak is wedged; restart its container.\n\
         ```action\n\
         {\"kind\":\"service_lifecycle\",\"target_host\":\"oak\",\"service_kind\":\"container\",\"name\":\"nginx\",\"op\":\"restart\"}\n\
         ```"
            .to_string()
    }

    #[test]
    fn extract_proposal_pulls_typed_action_and_prose() {
        let (prose, proposal) = extract_proposal(&answer_with_proposal());
        assert!(prose.contains("nginx on oak is wedged"));
        assert!(
            !prose.contains("```"),
            "the fence is stripped from the prose: {prose:?}"
        );
        let p = proposal.expect("a typed proposal was extracted");
        assert_eq!(p.action.kind_tag(), "service_lifecycle");
        // The rationale carries the operator-facing reasoning.
        assert!(p.rationale.contains("wedged"));
        match p.action {
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
    fn extract_proposal_plain_answer_has_no_proposal() {
        // FD-9 behavior preserved: a pure text answer → no proposal.
        let (prose, proposal) = extract_proposal("the master is fine, nothing to do.");
        assert_eq!(prose, "the master is fine, nothing to do.");
        assert!(proposal.is_none());
    }

    #[test]
    fn extract_proposal_drops_un_allowlisted_kind() {
        // §9: a block whose kind is NOT in FD-11's allowlist fails the FD-11 parse
        // and is DROPPED — the operator gets prose, never an un-allowlisted
        // proposal. (This is the same gate the executor enforces.)
        let answer = "I'll wipe the disk.\n\
            ```action\n\
            {\"kind\":\"raw_shell\",\"cmd\":\"rm -rf /\"}\n\
            ```";
        let (prose, proposal) = extract_proposal(answer);
        assert!(
            proposal.is_none(),
            "an un-allowlisted kind must NOT be proposed"
        );
        assert!(prose.contains("wipe the disk"), "prose is still returned");
    }

    #[test]
    fn extract_proposal_drops_malformed_block() {
        let answer = "do the thing\n```action\nnot json\n```";
        let (_prose, proposal) = extract_proposal(answer);
        assert!(proposal.is_none());
    }

    #[test]
    fn proposal_body_round_trips_and_is_not_an_execution_request() {
        // The published body is an ActionProposal (action + rationale), NOT a bare
        // ActionRequest on the execution topic — the operator must approve.
        let (_prose, proposal) = extract_proposal(&answer_with_proposal());
        let p = proposal.unwrap();
        let body = p.to_body();
        let back: ActionProposal = serde_json::from_str(&body).unwrap();
        assert_eq!(back, p);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(v.get("rationale").is_some(), "carries the human rationale");
        assert_eq!(v["action"]["kind"], "service_lifecycle");
    }
}
