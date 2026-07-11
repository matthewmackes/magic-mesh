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
//!    The allowlist now includes FD-12's `code_edit` (the AI-editing-code
//!    capability, Q52/Q53): when an ask implies a CONFIG/CODE change the copilot
//!    can propose a typed `code_edit` (target path + full reviewed content +
//!    rationale) the SAME propose-only way — it carries the full edit so the
//!    operator reviews the exact change, and the apply is FD-11's gated,
//!    path-bounded, audited handler, never the copilot.
//!
//! Code/config editing (Q52/Q53 diff→apply→git) is the remaining, more sensitive
//! FD-12 piece and is deliberately NOT implemented in this cut.
//!
//! ## FRONTDOOR-10 — a status topic + the proactive suggestion engine (this cut)
//!
//! FD-10 adds two PROACTIVE, propose-only increments on top of the FD-9/12
//! request/reply path — both reusing this worker's grounding, codex invocation,
//! proposal typing, leader gating, and `!Sync`-safe phasing rather than
//! reinventing any of it:
//!
//! 1. **Status topic (Q33/Q7).** On a CHEAP cadence the worker publishes a compact
//!    [`CopilotStatus`] to [`STATUS_TOPIC`] (`state/copilot/status`) — `available`
//!    + `leader` + a coarse [`CopilotState`] (`ready`/`thinking`/`offline`) + the
//!    model + a last-activity stamp. The Front Door's Copilot tile (FD-4 left it a
//!    plain launcher because NO topic existed) can finally render "ready"/
//!    "thinking"/"offline" off this. It's a cheap leader check + a secret-store
//!    presence probe + one bus write — no codex spawn.
//!
//! 2. **Proactive engine (Q7/Q9/Q61).** On a MODERATE timer (every few minutes,
//!    LEADER-ONLY), the worker assembles the SAME bounded mesh context FD-12 grounds
//!    asks with ([`CopilotWorker::assemble_mesh_context`]), asks codex (the FAST
//!    tier — Q87) for a SMALL RANKED set of HIGH-IMPACT, HIGH-CONFIDENCE ops
//!    suggestions ONLY, parses them into typed [`Suggestion`]s (each optionally
//!    carrying an FD-12 [`ActionProposal`] via [`extract_proposal`]), and publishes
//!    the ranked set to [`SUGGESTIONS_TOPIC`] (`action/copilot/suggestions`) for the
//!    FD-10 GUI half to render inline on the relevant tile later. Moderate cadence
//!    + high-confidence-only so the operator is not spammed (Q61). Graceful degrade:
//!    no codex/key → status `offline`, no suggestions published.
//!
//! §9 holds across BOTH: suggestions are PROPOSALS — typed actions ride the same
//! FD-11-allowlisted [`ActionProposal`] path and are NEVER executed here, NEVER
//! published to FD-11's execution topic. No executor is added; the only process
//! spawn remains the codex subprocess. The proactive codex call is kept OUT of any
//! `!Sync` borrow across `.await`, mirroring FD-12's sync-collect → async-codex →
//! sync-write phasing.
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

/// Bus topic the copilot PUBLISHES its compact STATUS to (FD-10 §1).
///
/// Lives in the `state/` namespace (current-state, not a command or an event) so
/// the Front Door's Copilot tile reads the latest snapshot the same way other
/// tiles read live state. FD-4 left that tile a plain launcher because NO topic
/// existed — this is the topic that unblocks it. The status is cheap to compute
/// (a leader check + a secret-store presence probe) and is written on its own
/// cheap cadence, independent of any codex round-trip.
pub const STATUS_TOPIC: &str = "state/copilot/status";

/// Bus topic the copilot PUBLISHES its proactive ranked SUGGESTIONS to (FD-10 §2).
///
/// Mirrors the [`PROPOSAL_TOPIC`] precedent: a non-RPC publish under the canonical
/// `action/copilot/*` namespace the GUI drains with the standard reader. A message
/// here is a RANKED SET of proposals for the FD-10 GUI to render inline — NEVER an
/// instruction to execute. It is deliberately DISTINCT from FD-11's execution topic
/// (`action/exec/request`); any actionable suggestion carries an FD-12
/// [`ActionProposal`] the operator must approve + re-publish to FD-11 (§9).
pub const SUGGESTIONS_TOPIC: &str = "action/copilot/suggestions";

/// Bus topic the copilot PUBLISHES its ALERT TRIAGE to (FD-13 / Q38).
///
/// Lives in the `state/` namespace (a current-state snapshot, not a command or an
/// event) so the Front Door's Alerts tile reads the latest triage the same way the
/// Copilot tile reads `state/copilot/status`. The body is an [`AlertTriage`]: the
/// live alerts GROUPED + EXPLAINED, each group optionally carrying a typed FD-12
/// [`ActionProposal`] for the proposed one-click fix. The fix is a PROPOSAL — the
/// GUI routes it through the SAME confirm gate as a suggestion's "Act" (re-publish
/// to [`PROPOSAL_TOPIC`]); the triage is NEVER published to FD-11's execution topic
/// and the copilot NEVER executes it (§9).
pub const ALERT_TRIAGE_TOPIC: &str = "state/copilot/alert-triage";

/// Bus topic-prefix the datacenter health plane publishes alerts under
/// (`dc_health`'s `event/dc/health/<check>`). The triage pass reads the latest body
/// per `event/dc/health/*` topic to find the NON-OK checks (the live alerts the
/// Alerts tile counts), then asks codex to group + explain + propose a fix. A
/// read-only consume of the same plane the GUI Alerts tile projects (§7 — real
/// alerts, no demo data).
pub const ALERT_TOPIC_PREFIX: &str = "event/dc/health/";

/// Moderate alert-triage cadence (FD-13 / Q61). Like the proactive-suggestion pass,
/// the LEADER re-triages the live alerts every few minutes — surfacing a grouped,
/// explained, fix-proposed view without spamming the operator or hammering codex.
pub const DEFAULT_TRIAGE_INTERVAL: Duration = Duration::from_secs(300);

/// Hard cap on the alerts folded into one triage prompt (FD-13). A flapping plane
/// can't blow the context budget — the most recent N non-ok checks are triaged and
/// the rest summarized as a count.
pub const MAX_ALERTS_IN_TRIAGE: usize = 24;

/// Hard cap on the triage groups codex may return (FD-13). Keeps the Alerts tile
/// detail bounded — a chatty model can't flood it with micro-groups.
pub const MAX_TRIAGE_GROUPS: usize = 8;

/// Cheap status cadence (FD-10 §1). 15 s keeps the Copilot tile's ready/thinking/
/// offline indicator fresh without cost — each tick is a leader check, a
/// secret-store presence probe, and (only on change) one bus write.
pub const DEFAULT_STATUS_INTERVAL: Duration = Duration::from_secs(15);

/// Moderate proactive-suggestion cadence (FD-10 §2 / Q61). Every 5 minutes the
/// LEADER assembles the mesh context and asks codex for a small ranked set of
/// high-impact suggestions. "Moderate" (not a tight loop) is the Q61 lock: surface
/// problems already-triaged without spamming the operator or hammering codex.
pub const DEFAULT_SUGGESTION_INTERVAL: Duration = Duration::from_secs(300);

/// Hard cap on the proactive ranked set (FD-10 §2). Q61 — high-confidence,
/// high-impact ONLY: a SMALL set, not an exhaustive dump. Anything codex returns
/// past this is truncated so a chatty model can't flood the tile.
pub const MAX_SUGGESTIONS: usize = 5;

/// The model label reported in the status topic + used for the proactive
/// suggestion pass (Q87: the FAST tier for suggestions; the strong tier is for
/// actions/edits, out of FD-10 scope). Passed to `codex exec --model` for the
/// suggestion invocation; the per-ask FD-9 path keeps codex's own default.
pub const DEFAULT_SUGGESTION_MODEL: &str = "gpt-5-mini";

/// Cap on one proactive suggestion `codex exec` (FD-10 §2). A ranked-set round-trip
/// is a bit longer than a single ask, so this is more generous than the per-ask
/// ceiling — a wedged child still degrades to "no suggestions this round", never a
/// pinned worker.
pub const DEFAULT_SUGGESTION_TIMEOUT: Duration = Duration::from_secs(180);

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
fenced block exactly of one of these forms:\n\
```action\n\
{\"kind\":\"service_lifecycle\",\"target_host\":\"<node>\",\"service_kind\":\"container|vm\",\"name\":\"<name>\",\"op\":\"start|stop|restart\"}\n\
```\n\
or, when the operator should change a CONFIG or CODE file, the FD-12 code-edit \
form (propose the FULL new file content so the operator can review the exact \
change before approving):\n\
```action\n\
{\"kind\":\"code_edit\",\"path\":\"<path relative to the repo/workgroup root>\",\"content\":\"<the full new file content>\"}\n\
```\n\
Only `service_lifecycle` and `code_edit` are allowlisted; propose nothing else. \
For `code_edit` the `path` MUST be relative to the repo root — never absolute, \
never containing `..` (an out-of-bounds path is rejected). Omit the block entirely \
when no operation is implied. The proposal is queued for the operator to approve \
— proposing is NOT executing; you cannot apply a code edit yourself.";

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

// ============================ FD-10 §1: status topic =========================

/// The coarse Copilot state the Front Door tile renders (FD-10 §1). Three states
/// the tile maps to ready/thinking/offline indicators — deliberately compact so
/// the tile read is trivial.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CopilotState {
    /// The leader has a usable codex key — Copilot can answer/suggest.
    Ready,
    /// A codex round-trip is in flight right now (an ask or a proactive pass).
    Thinking,
    /// No codex available here (not the leader, key not sealed, or store fault) —
    /// the rest of the Front Door keeps working (Q33 graceful degrade).
    Offline,
}

impl CopilotState {
    /// Stable lowercase tag (matches the serde rename) for logs + assertions.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Thinking => "thinking",
            Self::Offline => "offline",
        }
    }
}

/// The compact Copilot status published to [`STATUS_TOPIC`] (FD-10 §1). Small by
/// design — everything the Front Door tile needs to render ready/thinking/offline
/// and nothing it doesn't.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CopilotStatus {
    /// The coarse state the tile renders.
    pub state: CopilotState,
    /// `true` when this node is the elected leader (Copilot is leader-only — Q73).
    pub leader: bool,
    /// `true` when Copilot can actually serve here (leader + a usable codex key).
    /// `state == Offline` ⇔ `!available`, but it's carried explicitly so the tile
    /// needn't infer it.
    pub available: bool,
    /// The model label suggestions run on (Q87 fast tier) — shown in the tile.
    pub model: String,
    /// Unix-epoch seconds of the most recent Copilot activity (an ask answered or a
    /// proactive pass), or `None` if it hasn't acted since this process started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity_s: Option<u64>,
}

impl CopilotStatus {
    /// Build the status from the cheap inputs: leadership, key presence, whether a
    /// codex call is in flight, and the last-activity stamp. `available` is
    /// `leader && key_present`; the state is `Thinking` when a call is live, else
    /// `Ready` when available, else `Offline`. Pure so the mapping is unit-testable
    /// without a leader lock or a secret store.
    #[must_use]
    pub fn derive(
        leader: bool,
        key_present: bool,
        in_flight: bool,
        last_activity_s: Option<u64>,
    ) -> Self {
        let available = leader && key_present;
        let state = if !available {
            CopilotState::Offline
        } else if in_flight {
            CopilotState::Thinking
        } else {
            CopilotState::Ready
        };
        Self {
            state,
            leader,
            available,
            model: DEFAULT_SUGGESTION_MODEL.to_string(),
            last_activity_s,
        }
    }

    /// JSON body for [`STATUS_TOPIC`]. Infallible — a serialize failure (impossible
    /// for this shape) degrades to a fixed offline marker so a status write never
    /// wedges the cadence.
    #[must_use]
    pub fn to_body(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| {
            r#"{"state":"offline","leader":false,"available":false,"model":"unknown"}"#.to_string()
        })
    }
}

// ====================== FD-10 §2: proactive suggestions ======================

/// One proactive ranked suggestion (FD-10 §2). Prose + an optional typed
/// [`ActionProposal`] (the SAME FD-12 path an ask uses), so the GUI can render a
/// card with a one-click "propose" affordance when the suggestion is actionable.
/// NEVER executed here — the proposal is queued for operator approval (§9).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Suggestion {
    /// A short, operator-facing headline (e.g. "mfsmaster on oak is down").
    pub title: String,
    /// The supporting detail / rationale (why it matters, what to do).
    pub detail: String,
    /// `high` | `medium` — kept high-confidence-only at the prompt (Q61); carried
    /// so the tile can sort/badge. A missing/odd value degrades to `"medium"`.
    #[serde(default = "default_impact")]
    pub impact: String,
    /// The typed action proposal this suggestion implies, if any — the FD-12
    /// [`ActionProposal`] the operator approves to act. `None` for an advisory-only
    /// suggestion. It is a PROPOSAL: never executed, never on FD-11's exec topic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal: Option<ActionProposal>,
}

fn default_impact() -> String {
    "medium".to_string()
}

/// The ranked set published to [`SUGGESTIONS_TOPIC`] (FD-10 §2). A bounded, ranked
/// list (highest-impact first) plus the stamp it was produced — so a tile can show
/// freshness and the GUI half can render the cards inline.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SuggestionSet {
    /// The ranked suggestions (highest-impact first), capped at [`MAX_SUGGESTIONS`].
    pub suggestions: Vec<Suggestion>,
    /// Unix-epoch seconds the set was produced.
    pub produced_at_s: u64,
}

impl SuggestionSet {
    /// JSON body for [`SUGGESTIONS_TOPIC`]. Infallible — a serialize failure
    /// degrades to an empty set so a malformed publish never wedges the cadence.
    #[must_use]
    pub fn to_body(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| {
            format!(
                r#"{{"suggestions":[],"produced_at_s":{}}}"#,
                self.produced_at_s
            )
        })
    }
}

/// Compose the proactive-suggestions prompt handed to `codex exec` (FD-10 §2).
///
/// Reuses the [`SYSTEM_CONTEXT`] (§9 lane + the `action`-fence proposal protocol)
/// and the SAME grounding block FD-12 grounds asks with, then asks for a SMALL
/// RANKED set of HIGH-IMPACT, HIGH-CONFIDENCE suggestions emitted as a strict JSON
/// array (Q61 — high-confidence only, do not spam). Each element may embed an
/// `action`-fenced proposal in its `detail`, parsed through FD-11's allowlist by
/// [`parse_suggestions`]. Pure so it's unit-testable without spawning codex.
#[must_use]
pub fn compose_suggestions_prompt(mesh_context: &str) -> String {
    let mut out = String::new();
    out.push_str(SYSTEM_CONTEXT);
    out.push_str("\n\n");
    if !mesh_context.trim().is_empty() {
        out.push_str("Live mesh state:\n");
        out.push_str(mesh_context.trim());
        out.push_str("\n\n");
    }
    out.push_str(SUGGESTIONS_TASK);
    out
}

/// The proactive-suggestions instruction appended after the grounding (FD-10 §2).
/// Demands HIGH-CONFIDENCE, HIGH-IMPACT only (Q61) and a strict JSON-array shape so
/// the parse is deterministic; an empty array is the explicit "nothing worth
/// surfacing" answer (the common, quiet-mesh case).
const SUGGESTIONS_TASK: &str = "Reviewing the live mesh state above, PROACTIVELY surface only the \
HIGHEST-IMPACT, HIGHEST-CONFIDENCE operational fixes or optimizations an operator should act on \
RIGHT NOW. Be conservative: if nothing is clearly worth the operator's attention, return an empty \
list — do NOT invent low-value busywork. Return AT MOST 5, ranked best-first.\n\
Respond with ONLY a JSON array (no prose outside it), each element:\n\
{\"title\":\"<short headline>\",\"detail\":\"<why it matters + the fix>\",\"impact\":\"high|medium\"}\n\
If a suggestion implies a concrete operation, append the SAME ```action fenced block described \
above INSIDE that element's `detail` string — it becomes a queued proposal the operator approves \
(proposing is not executing). Return [] when there is nothing high-confidence to surface.";

/// Parse codex's proactive-suggestions answer into a bounded, ranked
/// `Vec<Suggestion>` (FD-10 §2).
///
/// Tolerant: codex may wrap the array in a ```` ```json ```` fence or add stray
/// prose, so we extract the first top-level `[ … ]`. Each element's `detail` is run
/// through [`extract_proposal`] — so an `action`-fenced block becomes a typed
/// FD-12 [`ActionProposal`] (gated by FD-11's allowlist; an un-allowlisted kind is
/// dropped) and the fence is stripped from the displayed detail. The result is
/// truncated to [`MAX_SUGGESTIONS`] (Q61). Unparseable input ⇒ empty (no panic).
#[must_use]
pub fn parse_suggestions(answer: &str) -> Vec<Suggestion> {
    let Some(arr) = extract_json_array(answer) else {
        return Vec::new();
    };
    let raw: Vec<RawSuggestion> = match serde_json::from_str(&arr) {
        Ok(v) => v,
        Err(e) => {
            tracing::info!(
                target: "mackesd::copilot",
                error = %e,
                "proactive suggestions did not parse as a JSON array; publishing none this round",
            );
            return Vec::new();
        }
    };
    raw.into_iter()
        .take(MAX_SUGGESTIONS)
        .filter_map(|r| {
            let title = r.title.trim().to_string();
            if title.is_empty() {
                return None;
            }
            // Run the detail through the SAME FD-12 proposal extractor an ask uses:
            // an `action`-fenced block becomes a typed (allowlisted) proposal and
            // is stripped from the displayed prose. The proposal is queued for
            // approval — never executed here (§9).
            let (prose, proposal) = extract_proposal(&r.detail);
            let impact = match r.impact.trim().to_lowercase().as_str() {
                "high" => "high".to_string(),
                _ => "medium".to_string(),
            };
            Some(Suggestion {
                title,
                detail: prose,
                impact,
                proposal,
            })
        })
        .collect()
}

/// The wire shape codex emits per suggestion (the proposal lives inline in
/// `detail`, extracted by [`parse_suggestions`]).
#[derive(serde::Deserialize)]
struct RawSuggestion {
    #[serde(default)]
    title: String,
    #[serde(default)]
    detail: String,
    #[serde(default = "default_impact")]
    impact: String,
}

/// Extract the first top-level `[ … ]` JSON array substring from `text`, tolerating
/// a surrounding ```` ```json ```` fence or stray prose. Bracket-depth scan so a
/// nested array inside an element doesn't end it early. `None` when there's no
/// balanced top-level array.
fn extract_json_array(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let start = bytes.iter().position(|&b| b == b'[')?;
    let mut depth = 0i32;
    let mut in_str = false;
    let mut esc = false;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if in_str {
            if esc {
                esc = false;
            } else if b == b'\\' {
                esc = true;
            } else if b == b'"' {
                in_str = false;
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(text[start..=i].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

// ===================== FD-13: alert triage (group/explain/fix) ===============

/// One live alert read off the datacenter health plane (FD-13). The compact
/// subset of a `event/dc/health/<check>` body the triage needs — the check name,
/// its non-ok status, and the human detail. A read-only projection of the SAME
/// plane the GUI Alerts tile counts (§7 — real alerts, no demo data).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Alert {
    /// The check name (`dom0:172.20.0.9`, `etcd`, `secret-store`, …).
    pub check: String,
    /// `warn` | `fail` (the triage only carries the NON-OK checks — an `ok` check
    /// is not an alert).
    pub status: String,
    /// A short human detail line — why the check warned/failed. Empty when absent.
    #[serde(default)]
    pub detail: String,
}

/// One GROUP of related alerts the copilot clustered, with its explanation and an
/// optional proposed fix (FD-13 / Q38). The copilot groups the live alerts, writes
/// one plain-language `explanation` per group, names the member `alerts`, and — when
/// the group implies a concrete operation — carries a typed FD-12 [`ActionProposal`]
/// the operator can act on with one click. The fix is a PROPOSAL: routed through the
/// confirm gate (re-published to [`PROPOSAL_TOPIC`]), NEVER executed here (§9).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AlertGroup {
    /// A short operator-facing headline for the cluster (e.g. "MeshFS master down
    /// on oak").
    pub title: String,
    /// The plain-language explanation: what is wrong, why these alerts cluster, and
    /// what the fix does.
    pub explanation: String,
    /// `high` | `medium` — the worst severity in the group, carried so the tile can
    /// sort/badge. A missing/odd value degrades to `"medium"`.
    #[serde(default = "default_impact")]
    pub severity: String,
    /// The member alert names (the `check`s) this group clusters. Carried so the
    /// tile shows WHICH alerts the explanation covers, not just the headline.
    #[serde(default)]
    pub alerts: Vec<String>,
    /// The typed one-click FIX this group proposes, if any — the FD-12
    /// [`ActionProposal`] the operator approves through the confirm gate. `None` for
    /// an explain-only group (no safe automated fix). It is a PROPOSAL: never
    /// executed, never on FD-11's exec topic (§9).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal: Option<ActionProposal>,
}

/// The grouped, explained triage published to [`ALERT_TRIAGE_TOPIC`] (FD-13). The
/// clustered groups (worst-first), the count of alerts that were triaged, and the
/// stamp it was produced — so the Alerts tile renders the triage with freshness.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AlertTriage {
    /// The clustered alert groups (worst-severity first), capped at
    /// [`MAX_TRIAGE_GROUPS`].
    pub groups: Vec<AlertGroup>,
    /// How many live alerts this triage covered (so the tile can show "N alerts
    /// triaged" even when they collapse into fewer groups).
    pub alert_count: usize,
    /// Unix-epoch seconds the triage was produced.
    pub produced_at_s: u64,
}

impl AlertTriage {
    /// JSON body for [`ALERT_TRIAGE_TOPIC`]. Infallible — a serialize failure
    /// degrades to an empty triage so a malformed publish never wedges the cadence.
    #[must_use]
    pub fn to_body(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| {
            format!(
                r#"{{"groups":[],"alert_count":0,"produced_at_s":{}}}"#,
                self.produced_at_s
            )
        })
    }
}

/// Render the live alerts as a compact, BOUNDED plain-text block for the triage
/// prompt (FD-13). The most recent [`MAX_ALERTS_IN_TRIAGE`] non-ok checks are named
/// with their status + detail; any beyond the cap are summarized as a count. Pure
/// so the composition is unit-testable without a Bus.
#[must_use]
pub fn render_alerts(alerts: &[Alert]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for a in alerts.iter().take(MAX_ALERTS_IN_TRIAGE) {
        let status = a.status.trim();
        let detail = a.detail.trim();
        if detail.is_empty() {
            lines.push(format!("- {} [{}]", a.check, status));
        } else {
            lines.push(format!("- {} [{}]: {}", a.check, status, detail));
        }
    }
    let overflow = alerts.len().saturating_sub(MAX_ALERTS_IN_TRIAGE);
    if overflow > 0 {
        lines.push(format!("- (+{overflow} more alerts)"));
    }
    lines.join("\n")
}

/// Compose the alert-triage prompt handed to `codex exec` (FD-13 / Q38).
///
/// Reuses [`SYSTEM_CONTEXT`] (the §9 lane + the `action`-fence proposal protocol)
/// and the SAME bounded mesh grounding the suggestion pass uses, then lists the live
/// alerts and asks codex to GROUP them, EXPLAIN each cluster in plain language, and
/// — when a safe fix exists — PROPOSE a typed one-click fix the SAME `action`-fenced
/// way (parsed through FD-11's allowlist). Strict JSON-array shape so the parse is
/// deterministic. Pure so it's unit-testable without spawning codex.
#[must_use]
pub fn compose_triage_prompt(mesh_context: &str, alerts: &[Alert]) -> String {
    let mut out = String::new();
    out.push_str(SYSTEM_CONTEXT);
    out.push_str("\n\n");
    if !mesh_context.trim().is_empty() {
        out.push_str("Live mesh state:\n");
        out.push_str(mesh_context.trim());
        out.push_str("\n\n");
    }
    out.push_str("Active alerts (the non-ok datacenter health checks):\n");
    out.push_str(&render_alerts(alerts));
    out.push_str("\n\n");
    out.push_str(TRIAGE_TASK);
    out
}

/// The triage instruction appended after the alerts (FD-13 / Q38). Demands GROUPING
/// of related alerts, a plain-language EXPLANATION per group, and an OPTIONAL typed
/// one-click fix per group via the `action` fence — emitted as a strict JSON array
/// so the parse is deterministic. An empty array is the explicit "no alerts to
/// triage" answer (the quiet, all-clear case).
const TRIAGE_TASK: &str = "Triage the active alerts above for the operator. GROUP related alerts \
into clusters (alerts that share a root cause belong to ONE group), EXPLAIN each cluster in plain \
language (what is wrong, why these cluster, and what — if anything — to do), and rank the groups \
WORST-FIRST. Return AT MOST 8 groups.\n\
Respond with ONLY a JSON array (no prose outside it), each element:\n\
{\"title\":\"<short headline>\",\"explanation\":\"<plain-language what+why+fix>\",\"severity\":\"high|medium\",\"alerts\":[\"<check name>\", …]}\n\
If — and only if — a group has a SAFE concrete fix, append the SAME ```action fenced block \
described above INSIDE that element's `explanation` string — it becomes a queued one-click \
proposal the operator approves through the confirm gate (proposing is not executing). Omit the \
fence for an explain-only group. Return [] when there are no alerts to triage.";

/// The wire shape codex emits per triage group (the proposal lives inline in
/// `explanation`, extracted by [`parse_triage`]).
#[derive(serde::Deserialize)]
struct RawAlertGroup {
    #[serde(default)]
    title: String,
    #[serde(default)]
    explanation: String,
    #[serde(default = "default_impact")]
    severity: String,
    #[serde(default)]
    alerts: Vec<String>,
}

/// Parse codex's triage answer into a bounded list of [`AlertGroup`]s (FD-13).
///
/// Tolerant (mirrors [`parse_suggestions`]): codex may wrap the array in a fence or
/// add stray prose, so the first top-level `[ … ]` is extracted. Each group's
/// `explanation` is run through [`extract_proposal`] — an `action`-fenced block
/// becomes a typed FD-12 [`ActionProposal`] (gated by FD-11's allowlist; an
/// un-allowlisted kind is dropped) and the fence is stripped from the displayed
/// explanation. Capped at [`MAX_TRIAGE_GROUPS`]; unparseable input ⇒ empty (no
/// panic). The proposed fix is a PROPOSAL — queued for approval, never executed (§9).
#[must_use]
pub fn parse_triage(answer: &str) -> Vec<AlertGroup> {
    let Some(arr) = extract_json_array(answer) else {
        return Vec::new();
    };
    let raw: Vec<RawAlertGroup> = match serde_json::from_str(&arr) {
        Ok(v) => v,
        Err(e) => {
            tracing::info!(
                target: "mackesd::copilot",
                error = %e,
                "alert triage did not parse as a JSON array; publishing none this round",
            );
            return Vec::new();
        }
    };
    raw.into_iter()
        .take(MAX_TRIAGE_GROUPS)
        .filter_map(|g| {
            let title = g.title.trim().to_string();
            if title.is_empty() {
                return None;
            }
            // Run the explanation through the SAME FD-12 proposal extractor a
            // suggestion/ask uses: an `action`-fenced block becomes a typed
            // (allowlisted) fix proposal and is stripped from the displayed prose.
            // The proposal is queued for approval — never executed here (§9).
            let (prose, proposal) = extract_proposal(&g.explanation);
            let severity = match g.severity.trim().to_lowercase().as_str() {
                "high" => "high".to_string(),
                _ => "medium".to_string(),
            };
            let alerts = g
                .alerts
                .into_iter()
                .map(|a| a.trim().to_string())
                .filter(|a| !a.is_empty())
                .collect();
            Some(AlertGroup {
                title,
                explanation: prose,
                severity,
                alerts,
                proposal,
            })
        })
        .collect()
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
    /// FD-10 §1 — cheap status cadence ([`STATUS_TOPIC`]). Tests shorten it.
    status_interval: Duration,
    /// FD-10 §2 — moderate proactive-suggestion cadence ([`SUGGESTIONS_TOPIC`]).
    /// Tests shorten it.
    suggestion_interval: Duration,
    /// FD-10 §2 — per-pass codex timeout for the suggestion invocation. Tests
    /// shorten it.
    suggestion_timeout: Duration,
    /// FD-13 — moderate alert-triage cadence ([`ALERT_TRIAGE_TOPIC`]). Tests
    /// shorten it.
    triage_interval: Duration,
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
            status_interval: DEFAULT_STATUS_INTERVAL,
            suggestion_interval: DEFAULT_SUGGESTION_INTERVAL,
            suggestion_timeout: DEFAULT_SUGGESTION_TIMEOUT,
            triage_interval: DEFAULT_TRIAGE_INTERVAL,
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

    /// FD-10 §1 — override the cheap status cadence. Tests use a shorter value.
    #[must_use]
    pub const fn with_status_interval(mut self, d: Duration) -> Self {
        self.status_interval = d;
        self
    }

    /// FD-10 §2 — override the proactive-suggestion cadence. Tests use a shorter
    /// value.
    #[must_use]
    pub const fn with_suggestion_interval(mut self, d: Duration) -> Self {
        self.suggestion_interval = d;
        self
    }

    /// FD-10 §2 — override the per-pass suggestion codex timeout. Tests shorten it.
    #[must_use]
    pub const fn with_suggestion_timeout(mut self, d: Duration) -> Self {
        self.suggestion_timeout = d;
        self
    }

    /// FD-13 — override the alert-triage cadence. Tests use a shorter value.
    #[must_use]
    pub const fn with_triage_interval(mut self, d: Duration) -> Self {
        self.triage_interval = d;
        self
    }

    /// Only the directory leader serves Copilot (Q73: leader-only; no-fixed-
    /// center: any eligible node can be it, the elected one answers). Reuses the
    /// shared leader lock — synchronous, called once per tick.
    fn is_leader(&self) -> bool {
        crate::leader_gate::LeaderGate::from_lock_path(
            self.leader_lock.clone(),
            self.node_id.clone(),
        )
        .is_leader()
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
        // Leader lease — the current mesh leader, from whichever substrate is
        // authoritative (etcd `/mesh/leader` on a cut-over fleet, else the fs lease).
        ctx.leader = crate::leader_gate::LeaderGate::from_lock_path(
            self.leader_lock.clone(),
            self.node_id.clone(),
        )
        .current_leader_id();
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
    /// trip doesn't pin the runtime thread; bounded by `timeout`. `model` selects
    /// the tier (Q87: `None` ⇒ codex's own default for the per-ask FD-9 path; the
    /// FD-10 §2 suggestion pass passes the FAST tier). Every failure mode degrades
    /// to [`CodexOutcome::Unavailable`] — never an error return, never a panic. The
    /// ONLY process this worker ever spawns is this codex subprocess (§9).
    async fn invoke_codex(
        &self,
        api_key: &str,
        prompt: &str,
        model: Option<&str>,
        timeout: Duration,
    ) -> CodexOutcome {
        let mut cmd = tokio::process::Command::new(&self.codex_bin);
        cmd.arg("exec");
        if let Some(m) = model {
            // Q87 tiered model — the fast tier for the proactive suggestion pass.
            cmd.arg("--model").arg(m);
        }
        cmd.arg(prompt)
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
        let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
            Ok(Ok(o)) => o,
            Ok(Err(e)) => {
                tracing::warn!(target: "mackesd::copilot", error = %e, "codex wait failed");
                return CodexOutcome::Unavailable(format!("codex wait failed: {e}"));
            }
            Err(_) => {
                tracing::warn!(
                    target: "mackesd::copilot",
                    timeout_s = timeout.as_secs(),
                    "codex exec timed out",
                );
                return CodexOutcome::Unavailable(format!(
                    "codex timed out after {}s",
                    timeout.as_secs()
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
        match self
            .invoke_codex(&api_key, &prompt, None, self.codex_timeout)
            .await
        {
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

    // ===================== FD-10 §1 — the status topic ======================

    /// Cheaply derive the current Copilot status (FD-10 §1) WITHOUT a codex spawn:
    /// a leader check + a secret-store presence probe. `in_flight` is whether a
    /// codex round-trip is live right now (drives `thinking`); `last_activity_s` is
    /// the most-recent-activity stamp. Pure-ish (only the two cheap reads), so the
    /// status cadence costs nothing even when codex is busy or absent.
    fn current_status(&self, in_flight: bool, last_activity_s: Option<u64>) -> CopilotStatus {
        let leader = self.is_leader();
        // Key presence is a cheap read; a store fault counts as "not present" for
        // the status (the tile shows offline rather than guessing).
        let key_present = matches!(self.read_codex_key(), Ok(Some(_)));
        CopilotStatus::derive(leader, key_present, in_flight, last_activity_s)
    }

    // ================ FD-10 §2 — the proactive suggestion engine ============

    /// Run ONE proactive suggestion pass (FD-10 §2), LEADER-ONLY. Mirrors the FD-12
    /// phasing: the grounding + key reads are synchronous (no `&Persist`/
    /// `&Connection` borrow crosses the codex `.await`); the codex round-trip is the
    /// only async/spawning step; the caller publishes the result. Returns the ranked
    /// [`SuggestionSet`] to publish, or `None` when there's nothing to publish (not
    /// leader, no key, codex unavailable, or an empty high-confidence set — Q61:
    /// quiet is the common case, and we don't publish an empty set as noise).
    /// PROPOSE-ONLY: any actionable suggestion carries a typed FD-12
    /// [`ActionProposal`] queued for approval — NEVER executed here (§9).
    async fn generate_suggestions(&self) -> Option<SuggestionSet> {
        if !self.is_leader() {
            // Leader-only (Q73): a follower never runs the proactive pass, so a
            // multi-node mesh produces one suggestion set, not N.
            return None;
        }
        let api_key = match self.read_codex_key() {
            Ok(Some(k)) => k,
            Ok(None) => {
                tracing::debug!(
                    target: "mackesd::copilot",
                    "proactive pass skipped: codex key not sealed yet (status will read offline)",
                );
                return None;
            }
            Err(e) => {
                tracing::warn!(
                    target: "mackesd::copilot",
                    error = %e,
                    "proactive pass skipped: secret-store fault",
                );
                return None;
            }
        };
        // Same bounded grounding FD-12 grounds asks with (a synchronous read — the
        // borrow is dropped before the codex `.await` below).
        let mesh_context = self.assemble_mesh_context().render();
        let prompt = compose_suggestions_prompt(&mesh_context);
        // The FAST tier (Q87) for suggestions; a more generous timeout than a
        // single ask since a ranked set is a longer round-trip.
        let outcome = self
            .invoke_codex(
                &api_key,
                &prompt,
                Some(DEFAULT_SUGGESTION_MODEL),
                self.suggestion_timeout,
            )
            .await;
        let answer = match outcome {
            CodexOutcome::Answer(a) => a,
            CodexOutcome::Unavailable(reason) => {
                tracing::info!(
                    target: "mackesd::copilot",
                    reason = %reason,
                    "proactive pass produced no suggestions (codex unavailable this round)",
                );
                return None;
            }
        };
        let suggestions = parse_suggestions(&answer);
        if suggestions.is_empty() {
            // Q61 — a quiet mesh is the common case; publish nothing rather than an
            // empty card the tile would have to special-case.
            tracing::debug!(
                target: "mackesd::copilot",
                "proactive pass: no high-confidence suggestions to surface this round",
            );
            return None;
        }
        let proposals = suggestions.iter().filter(|s| s.proposal.is_some()).count();
        tracing::info!(
            target: "mackesd::copilot",
            count = suggestions.len(),
            proposals,
            "copilot published proactive suggestions (proposals queued for operator approval; not executed)",
        );
        Some(SuggestionSet {
            suggestions,
            produced_at_s: now_epoch_s(),
        })
    }

    // ===================== FD-13 — the alert-triage engine ==================

    /// Read the live ALERTS off the datacenter health plane (FD-13), SYNCHRONOUSLY
    /// — the latest body per `event/dc/health/*` topic, keeping only the NON-OK
    /// checks (the alerts the GUI Alerts tile counts). The `&Persist` borrow is
    /// fully contained here (never held across the codex `.await`). Best-effort: a
    /// missing Bus / failed read leaves the list empty (the triage degrades to "no
    /// alerts", never an error). Read-only — never written. Deterministically
    /// `check`-sorted so the prompt is stable.
    fn read_alerts(persist: &Persist) -> Vec<Alert> {
        let topics = match persist.list_topics() {
            Ok(t) => t,
            Err(e) => {
                tracing::debug!(target: "mackesd::copilot", error = %e, "list_topics failed for alert triage");
                return Vec::new();
            }
        };
        let mut alerts: Vec<Alert> = Vec::new();
        for topic in topics {
            if !topic.starts_with(ALERT_TOPIC_PREFIX) {
                continue;
            }
            // The latest body on the topic is the current status of that check
            // (`dc_health` republishes on every transition); read just the newest.
            let Ok(msgs) = persist.list_since(&topic, None) else {
                continue;
            };
            let Some(body) = msgs.last().and_then(|m| m.body.clone()) else {
                continue;
            };
            let Ok(v) = serde_json::from_str::<serde_json::Value>(body.trim()) else {
                continue;
            };
            let check = v
                .get("check")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_else(|| topic.trim_start_matches(ALERT_TOPIC_PREFIX))
                .trim()
                .to_string();
            let status = v
                .get("status")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            // Only NON-OK checks are alerts; an "ok"/empty check is not triaged.
            if status.is_empty() || status == "ok" {
                continue;
            }
            let detail = v
                .get("detail")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            alerts.push(Alert {
                check,
                status,
                detail,
            });
        }
        alerts.sort_by(|a, b| a.check.cmp(&b.check));
        alerts
    }

    /// Run ONE alert-triage pass (FD-13 / Q38), LEADER-ONLY. Mirrors the FD-10
    /// suggestion phasing exactly: the `alerts` are read + the grounding + key reads
    /// are SYNCHRONOUS in the CALLER (no `&Persist`/`&Connection` borrow crosses the
    /// codex `.await` — `Persist` is `!Sync`, so the alerts arrive here ALREADY READ,
    /// owned); the codex round-trip is the only async/spawning step; the caller
    /// publishes the result. Returns the [`AlertTriage`] to publish, or `None` when
    /// there is nothing to publish (not leader, no alerts, no key, codex unavailable,
    /// or an empty triage). PROPOSE-ONLY: any group's fix is a typed FD-12
    /// [`ActionProposal`] queued for approval — NEVER executed here (§9).
    async fn generate_triage(&self, alerts: Vec<Alert>) -> Option<AlertTriage> {
        if !self.is_leader() {
            // Leader-only (Q73): a follower never triages, so a multi-node mesh
            // produces one triage, not N.
            return None;
        }
        if alerts.is_empty() {
            // All-clear is the common, quiet case — publish nothing rather than an
            // empty triage the tile would special-case (mirrors the suggestion pass).
            tracing::debug!(
                target: "mackesd::copilot",
                "alert triage: no active alerts to triage this round",
            );
            return None;
        }
        let api_key = match self.read_codex_key() {
            Ok(Some(k)) => k,
            Ok(None) => {
                tracing::debug!(
                    target: "mackesd::copilot",
                    "triage pass skipped: codex key not sealed yet (status will read offline)",
                );
                return None;
            }
            Err(e) => {
                tracing::warn!(
                    target: "mackesd::copilot",
                    error = %e,
                    "triage pass skipped: secret-store fault",
                );
                return None;
            }
        };
        let alert_count = alerts.len();
        // Same bounded grounding the suggestion pass grounds with (a synchronous
        // read — dropped before the codex `.await`).
        let mesh_context = self.assemble_mesh_context().render();
        let prompt = compose_triage_prompt(&mesh_context, &alerts);
        // The FAST tier (Q87) for triage, like suggestions; the more generous
        // suggestion timeout (a grouped round-trip is a bit longer than one ask).
        let outcome = self
            .invoke_codex(
                &api_key,
                &prompt,
                Some(DEFAULT_SUGGESTION_MODEL),
                self.suggestion_timeout,
            )
            .await;
        let answer = match outcome {
            CodexOutcome::Answer(a) => a,
            CodexOutcome::Unavailable(reason) => {
                tracing::info!(
                    target: "mackesd::copilot",
                    reason = %reason,
                    "triage pass produced no groups (codex unavailable this round)",
                );
                return None;
            }
        };
        let groups = parse_triage(&answer);
        if groups.is_empty() {
            tracing::debug!(
                target: "mackesd::copilot",
                "triage pass: codex returned no groups this round",
            );
            return None;
        }
        let fixes = groups.iter().filter(|g| g.proposal.is_some()).count();
        tracing::info!(
            target: "mackesd::copilot",
            groups = groups.len(),
            alert_count,
            fixes,
            "copilot published an alert triage (proposed fixes queued for operator approval; not executed)",
        );
        Some(AlertTriage {
            groups,
            alert_count,
            produced_at_s: now_epoch_s(),
        })
    }
}

/// Wall-clock epoch seconds for the status/suggestion stamps. Best-effort: a clock
/// before the epoch reads as 0 rather than panicking.
fn now_epoch_s() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
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
        // FD-10 — two additional cadences: a CHEAP status heartbeat (§1) and a
        // MODERATE proactive-suggestion timer (§2). Both run on the same `Send`
        // future as the ask sweep; neither holds a `&Persist` borrow across a codex
        // `.await` (the suggestion codex call is awaited with NO borrow live, then
        // the result is published synchronously — mirroring the ask phasing).
        let mut status_tick = tokio::time::interval(self.status_interval);
        let mut suggest_tick = tokio::time::interval(self.suggestion_interval);
        // FD-13 — the moderate alert-triage cadence, on the same `Send` future as
        // the ask sweep; like the suggestion pass it never holds a `&Persist` borrow
        // across the codex `.await` (the triage codex call is awaited with NO borrow
        // live, then the result is published synchronously).
        let mut triage_tick = tokio::time::interval(self.triage_interval);
        // Burn the immediate first tick on the ask + suggestion + triage cadences so
        // we wait a full interval on startup. The STATUS tick is allowed to fire
        // immediately so the tile gets a snapshot promptly on boot (Q92 — render
        // ready/offline without waiting a full 15 s).
        tick.tick().await;
        suggest_tick.tick().await;
        triage_tick.tick().await;
        // FD-10 §1 — last-activity stamp, set whenever Copilot acts (an ask
        // answered or a proactive pass). `None` until the first activity.
        let mut last_activity_s: Option<u64> = None;
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
                    let answered_any = !asks.is_empty();
                    for (ulid, body) in asks {
                        // FD-10 §1 — flip the tile to "thinking" while this ask's
                        // codex round-trip is in flight (a synchronous status write,
                        // no codex spawn — cheap, no borrow across the ask `.await`).
                        self.publish_status(&persist, true, last_activity_s);
                        let (reply, proposal) = self.handle_ask(&body).await;
                        replies.push((ulid, reply, proposal));
                    }
                    write_replies(&persist, replies);
                    if answered_any {
                        last_activity_s = Some(now_epoch_s());
                        // Drop back to "ready" (or offline if leadership/key
                        // changed) once the asks are answered.
                        self.publish_status(&persist, false, last_activity_s);
                    }
                }
                _ = status_tick.tick() => {
                    // FD-10 §1 — the cheap status heartbeat: a leader check + a
                    // key-presence probe + one bus write. No codex spawn, fully
                    // synchronous, so it's safe under the `&persist` borrow.
                    self.publish_status(&persist, false, last_activity_s);
                }
                _ = suggest_tick.tick() => {
                    // FD-10 §2 — the moderate proactive pass (LEADER-ONLY). The
                    // codex round-trip is awaited with NO `&Persist` borrow live
                    // (mirroring the ask phasing for the `!Sync` constraint); the
                    // resulting ranked set is published synchronously afterward.
                    // Suggestions are PROPOSALS — never executed, never on FD-11's
                    // exec topic (§9).
                    self.publish_status(&persist, true, last_activity_s);
                    let set = self.generate_suggestions().await;
                    if let Some(set) = set {
                        publish_suggestions(&persist, &set);
                        last_activity_s = Some(now_epoch_s());
                    }
                    self.publish_status(&persist, false, last_activity_s);
                }
                _ = triage_tick.tick() => {
                    // FD-13 — the moderate alert-triage pass (LEADER-ONLY). The live
                    // alerts are read synchronously, then the codex round-trip is
                    // awaited with NO `&Persist` borrow live (the `!Sync` constraint,
                    // mirroring the suggestion phasing); the resulting grouped triage
                    // is published synchronously afterward. The proposed fixes are
                    // PROPOSALS — never executed, never on FD-11's exec topic (§9).
                    self.publish_status(&persist, true, last_activity_s);
                    // Read the live alerts SYNCHRONOUSLY here — the `&persist` borrow
                    // is dropped before `generate_triage`'s codex `.await` (the
                    // `!Sync` constraint), so the alerts cross the await owned.
                    let alerts = Self::read_alerts(&persist);
                    let triage = self.generate_triage(alerts).await;
                    if let Some(triage) = triage {
                        publish_triage(&persist, &triage);
                        last_activity_s = Some(now_epoch_s());
                    }
                    self.publish_status(&persist, false, last_activity_s);
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

    /// FD-10 §1 — derive + publish the compact Copilot status to [`STATUS_TOPIC`].
    /// Synchronous (a leader check + a key-presence probe + one bus write — NO codex
    /// spawn), so it is safe to call while a `&persist` borrow is live and never
    /// crosses a codex `.await`. `Priority::Min` (silent log-only): this is a pure
    /// data topic the tile reads, not an operator notification.
    fn publish_status(&self, persist: &Persist, in_flight: bool, last_activity_s: Option<u64>) {
        let status = self.current_status(in_flight, last_activity_s);
        if let Err(e) = persist.write(STATUS_TOPIC, Priority::Min, None, Some(&status.to_body())) {
            tracing::debug!(
                target: "mackesd::copilot",
                error = %e,
                "status publish failed",
            );
        }
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

/// FD-10 §2 (sync): publish the proactive ranked [`SuggestionSet`] on
/// [`SUGGESTIONS_TOPIC`] for the GUI half to render inline. A fresh ULID is minted
/// by `persist.write`. The set carries PROPOSALS — it is NOT FD-11's execution
/// topic (`action/exec/request`), nothing here executes any of them; the operator
/// must approve + re-publish a proposal to FD-11 to act (§9). Free function so the
/// `&Persist` borrow is unambiguously scoped to this synchronous call — never live
/// across the codex `.await` that produced the set.
fn publish_suggestions(persist: &Persist, set: &SuggestionSet) {
    if let Err(e) = persist.write(
        SUGGESTIONS_TOPIC,
        Priority::Default,
        None,
        Some(&set.to_body()),
    ) {
        tracing::warn!(
            target: "mackesd::copilot",
            error = %e,
            "suggestions publish failed",
        );
    }
}

/// FD-13 (sync): publish the grouped [`AlertTriage`] on [`ALERT_TRIAGE_TOPIC`] for
/// the Front Door's Alerts tile to render. A `state/` snapshot (latest body is the
/// current triage), written with `Priority::Min` (a silent data topic the tile
/// reads, not an operator notification). The triage carries PROPOSED FIXES — it is
/// NOT FD-11's execution topic (`action/exec/request`); nothing here executes any of
/// them; the operator must approve a fix through the confirm gate (§9). Free
/// function so the `&Persist` borrow is unambiguously scoped to this synchronous
/// call — never live across the codex `.await` that produced the triage.
fn publish_triage(persist: &Persist, triage: &AlertTriage) {
    if let Err(e) = persist.write(
        ALERT_TRIAGE_TOPIC,
        Priority::Min,
        None,
        Some(&triage.to_body()),
    ) {
        tracing::warn!(
            target: "mackesd::copilot",
            error = %e,
            "alert triage publish failed",
        );
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
        let out = w
            .invoke_codex("fake-key", "hi", None, DEFAULT_CODEX_TIMEOUT)
            .await;
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
        // (model None ⇒ no `--model` arg, so argv stays `exec`, `<prompt>`.)
        let out = w.invoke_codex("k", "p", None, DEFAULT_CODEX_TIMEOUT).await;
        assert_eq!(out, CodexOutcome::Answer("exec".to_string()));
    }

    #[tokio::test]
    async fn worker_exits_on_shutdown_token() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut w = CopilotWorker::new(tmp.path().to_path_buf(), "n1".into())
            .with_bus_root(tmp.path().join("bus"))
            .with_db_path(tmp.path().join("audit.db"))
            .with_poll_interval(Duration::from_millis(20))
            // FD-10 — exercise the status + suggestion cadences in the run loop too
            // (short so they fire before the shutdown is observed). The proactive
            // pass degrades to "no suggestions" here (no codex/key), so it never
            // blocks the clean exit.
            .with_status_interval(Duration::from_millis(20))
            .with_suggestion_interval(Duration::from_millis(20))
            // FD-13 — exercise the triage cadence in the run loop too (short so it
            // fires before the shutdown is observed). It degrades to "no triage"
            // here (no codex/key/alerts), so it never blocks the clean exit.
            .with_triage_interval(Duration::from_millis(20));
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
            other => panic!("expected ServiceLifecycle, got {other:?}"),
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

    // -------------------- FD-12 code-edit proposal (propose-only) -----------

    fn answer_with_code_edit() -> String {
        // The code-edit proposal shape codex is now told to emit: prose rationale,
        // then a fenced `code_edit` block carrying the path + the FULL reviewed
        // content so the operator sees the exact change before approving.
        "bump the log level to debug in the app config.\n\
         ```action\n\
         {\"kind\":\"code_edit\",\"path\":\"config/app.toml\",\"content\":\"log_level = \\\"debug\\\"\\n\"}\n\
         ```"
            .to_string()
    }

    #[test]
    fn extract_proposal_maps_code_edit_to_typed_action() {
        // FD-12: an ask implying a config/code change → a typed CodeEdit proposal,
        // carrying the path + full content + rationale. It goes through the SAME
        // FD-11 allowlist gate (parse_action_request), proving the copilot can only
        // propose something the gated action worker is willing to apply.
        let (prose, proposal) = extract_proposal(&answer_with_code_edit());
        assert!(prose.contains("bump the log level"));
        assert!(!prose.contains("```"), "fence stripped: {prose:?}");
        let p = proposal.expect("a typed code_edit proposal was extracted");
        assert_eq!(p.action.kind_tag(), "code_edit");
        assert!(p.rationale.contains("log level"), "carries the rationale");
        match p.action {
            ActionRequest::CodeEdit { path, content } => {
                assert_eq!(path, "config/app.toml");
                assert_eq!(content, "log_level = \"debug\"\n");
            }
            other => panic!("expected CodeEdit, got {other:?}"),
        }
    }

    #[test]
    fn copilot_code_edit_proposal_is_propose_only_not_an_apply() {
        // SAFETY: the copilot ONLY proposes. extract_proposal returns an
        // ActionProposal published on PROPOSAL_TOPIC — it is NOT FD-11's execution
        // topic, and nothing here applies the edit. The proposal body carries the
        // full content for review but performs no write; the copilot never reaches
        // the action worker's apply handler (that needs an explicit operator apply
        // request on ACTION_TOPIC).
        let (_prose, proposal) = extract_proposal(&answer_with_code_edit());
        let p = proposal.expect("proposal");
        // The publish topic is the proposal lane, distinct from the execution lane.
        assert_ne!(PROPOSAL_TOPIC, crate::workers::action::ACTION_TOPIC);
        // The body is an ActionProposal (action + rationale for review), not a bare
        // ActionRequest dispatched for execution.
        let body = p.to_body();
        let back: ActionProposal = serde_json::from_str(&body).unwrap();
        assert_eq!(back, p);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["action"]["kind"], "code_edit");
        assert_eq!(v["action"]["path"], "config/app.toml");
        // The full content is present so the operator reviews the exact change.
        assert!(
            v["action"]["content"]
                .as_str()
                .unwrap()
                .contains("log_level"),
            "the reviewable full content travels in the proposal"
        );
    }

    #[test]
    fn extract_proposal_drops_out_of_bounds_path_at_apply_not_propose() {
        // The copilot can propose a code_edit with any path that PARSES (the path
        // bound is enforced at APPLY by the action worker, not at parse) — but an
        // un-allowlisted KIND is still dropped here. This documents the boundary:
        // propose carries the typed request; the path-bound gate lives in the
        // gated, audited apply handler (validate_edit_path), never the copilot.
        let answer = "edit it\n```action\n\
            {\"kind\":\"code_edit\",\"path\":\"/etc/passwd\",\"content\":\"x\"}\n```";
        let (_prose, proposal) = extract_proposal(answer);
        let p = proposal.expect("parses as a typed code_edit proposal");
        // The action worker is what REJECTS the out-of-bounds path at apply time.
        if let ActionRequest::CodeEdit { path, .. } = &p.action {
            assert!(crate::workers::action::validate_edit_path(
                std::path::Path::new("/srv/wg"),
                path
            )
            .is_err());
        } else {
            panic!("expected CodeEdit");
        }
    }

    // ===================== FD-10 §1 — the status topic =====================

    #[test]
    fn status_topic_is_state_namespaced() {
        // FD-4 left the Copilot tile a plain launcher because no topic existed.
        // This is the topic that unblocks it — under `state/` (current state).
        assert_eq!(STATUS_TOPIC, "state/copilot/status");
        assert!(STATUS_TOPIC.starts_with("state/"));
    }

    #[test]
    fn status_derive_offline_when_not_leader_or_no_key() {
        // Not the leader → offline + unavailable, regardless of key (Copilot is
        // leader-only — Q73).
        let s = CopilotStatus::derive(false, true, false, None);
        assert_eq!(s.state, CopilotState::Offline);
        assert!(!s.available);
        // Leader but no sealed key → still offline (graceful degrade — Q33).
        let s = CopilotStatus::derive(true, false, false, None);
        assert_eq!(s.state, CopilotState::Offline);
        assert!(!s.available);
    }

    #[test]
    fn status_derive_ready_and_thinking_when_available() {
        // Leader + key, idle → ready.
        let s = CopilotStatus::derive(true, true, false, Some(1_700_000_000));
        assert_eq!(s.state, CopilotState::Ready);
        assert!(s.available);
        assert!(s.leader);
        assert_eq!(s.last_activity_s, Some(1_700_000_000));
        // …and "thinking" while a codex round-trip is in flight.
        let s = CopilotStatus::derive(true, true, true, None);
        assert_eq!(s.state, CopilotState::Thinking);
        assert!(s.available);
    }

    #[test]
    fn status_body_shape_carries_state_available_model() {
        let s = CopilotStatus::derive(true, true, false, Some(42));
        let v: serde_json::Value = serde_json::from_str(&s.to_body()).unwrap();
        assert_eq!(v["state"], "ready");
        assert_eq!(v["leader"], true);
        assert_eq!(v["available"], true);
        assert_eq!(v["model"], DEFAULT_SUGGESTION_MODEL);
        assert_eq!(v["last_activity_s"], 42);
        // No last-activity → the field is omitted (not null).
        let s = CopilotStatus::derive(true, true, false, None);
        let v: serde_json::Value = serde_json::from_str(&s.to_body()).unwrap();
        assert!(!v.as_object().unwrap().contains_key("last_activity_s"));
    }

    #[test]
    fn status_published_on_the_bus_is_readable() {
        // End-to-end: the worker publishes a status the Front Door tile can read
        // back off STATUS_TOPIC (the unblock for FD-4's launcher-only tile).
        let tmp = tempfile::tempdir().unwrap();
        let persist =
            mde_bus::persist::Persist::open(tmp.path().join("bus")).expect("persist open");
        let w = CopilotWorker::new(tmp.path().to_path_buf(), "n1".into());
        w.publish_status(&persist, false, Some(99));
        let msgs = persist.list_since(STATUS_TOPIC, None).expect("list");
        assert_eq!(msgs.len(), 1, "exactly one status published");
        let body = msgs[0].body.clone().expect("status body");
        let got: CopilotStatus = serde_json::from_str(&body).expect("parse status");
        // No leader lock can be taken on a bare tempdir (shared_root_writable
        // guard), so a fresh worker reads as offline — the honest graceful-degrade
        // state, and exactly what the tile would render.
        assert_eq!(got.state, CopilotState::Offline);
        assert!(!got.available);
        assert_eq!(got.last_activity_s, Some(99));
    }

    // ================ FD-10 §2 — proactive suggestion engine ===============

    #[test]
    fn suggestions_topic_is_distinct_from_execution_topic() {
        // §9 backstop: suggestions are PROPOSALS — published on their own topic,
        // NEVER FD-11's execution topic. No path from a suggestion to execution
        // without operator approval + a re-publish to FD-11.
        assert_eq!(SUGGESTIONS_TOPIC, "action/copilot/suggestions");
        assert_ne!(SUGGESTIONS_TOPIC, crate::workers::action::ACTION_TOPIC);
        assert!(SUGGESTIONS_TOPIC.starts_with("action/"));
    }

    #[test]
    fn suggestions_prompt_grounds_and_demands_high_confidence_only() {
        let p = compose_suggestions_prompt("Leader: peer:oak\nNodes: anvil[peer,degraded]");
        // Grounded with the live mesh state.
        assert!(p.contains("Live mesh state:"));
        assert!(p.contains("anvil[peer,degraded]"));
        // Q61 — high-confidence/high-impact only, bounded, and an explicit empty-OK.
        assert!(p.contains("HIGHEST-IMPACT"));
        assert!(p.contains("empty list") || p.contains("Return []"));
        // The §9 proposal protocol is taught (reused from the system context).
        assert!(p.contains("```action"));
        assert!(p.contains("service_lifecycle"));
    }

    #[test]
    fn parse_suggestions_pulls_ranked_set_and_caps() {
        // A ranked JSON array (wrapped in stray prose + a json fence to prove the
        // tolerant extraction) parses to bounded, ranked suggestions.
        let answer = "Here are my findings:\n```json\n[\
            {\"title\":\"mfsmaster down\",\"detail\":\"restart it\",\"impact\":\"high\"},\
            {\"title\":\"disk 90% on anvil\",\"detail\":\"prune logs\",\"impact\":\"medium\"}\
            ]\n```\nthat's all";
        let got = parse_suggestions(answer);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].title, "mfsmaster down");
        assert_eq!(got[0].impact, "high");
        assert_eq!(got[1].impact, "medium");
        assert!(got[0].proposal.is_none(), "advisory-only suggestion");
    }

    #[test]
    fn parse_suggestions_caps_at_max() {
        // Q61 — a chatty model can't flood the tile; the set is truncated.
        let items: Vec<String> = (0..20)
            .map(|i| format!("{{\"title\":\"t{i}\",\"detail\":\"d\",\"impact\":\"high\"}}"))
            .collect();
        let answer = format!("[{}]", items.join(","));
        let got = parse_suggestions(&answer);
        assert_eq!(got.len(), MAX_SUGGESTIONS);
    }

    #[test]
    fn parse_suggestions_extracts_typed_proposal_inline() {
        // A suggestion whose `detail` carries an `action`-fenced block becomes a
        // typed FD-12 proposal (gated by FD-11's allowlist), stripped from the
        // displayed detail — the SAME path an ask uses. Queued, never executed (§9).
        let answer = "[{\"title\":\"nginx wedged\",\"detail\":\"restart nginx on oak.\\n\
            ```action\\n\
            {\\\"kind\\\":\\\"service_lifecycle\\\",\\\"target_host\\\":\\\"oak\\\",\
            \\\"service_kind\\\":\\\"container\\\",\\\"name\\\":\\\"nginx\\\",\\\"op\\\":\\\"restart\\\"}\\n\
            ```\",\"impact\":\"high\"}]";
        let got = parse_suggestions(answer);
        assert_eq!(got.len(), 1);
        let s = &got[0];
        assert!(
            !s.detail.contains("```"),
            "fence stripped from detail: {:?}",
            s.detail
        );
        let p = s.proposal.as_ref().expect("typed proposal extracted");
        assert_eq!(p.action.kind_tag(), "service_lifecycle");
    }

    #[test]
    fn parse_suggestions_drops_un_allowlisted_proposal_keeps_prose() {
        // §9: an un-allowlisted action in a suggestion is dropped (the operator
        // still gets the prose), never proposed.
        let answer = "[{\"title\":\"danger\",\"detail\":\"wipe it.\\n\
            ```action\\n{\\\"kind\\\":\\\"raw_shell\\\",\\\"cmd\\\":\\\"rm -rf /\\\"}\\n```\",\
            \"impact\":\"high\"}]";
        let got = parse_suggestions(answer);
        assert_eq!(got.len(), 1);
        assert!(
            got[0].proposal.is_none(),
            "un-allowlisted kind must not be proposed"
        );
    }

    #[test]
    fn parse_suggestions_tolerates_garbage_and_empty() {
        assert!(parse_suggestions("not json at all").is_empty());
        assert!(parse_suggestions("[]").is_empty());
        // An object instead of an array → empty (not a panic).
        assert!(parse_suggestions("{\"title\":\"x\"}").is_empty());
    }

    #[test]
    fn suggestion_set_body_round_trips() {
        let set = SuggestionSet {
            suggestions: vec![Suggestion {
                title: "t".into(),
                detail: "d".into(),
                impact: "high".into(),
                proposal: None,
            }],
            produced_at_s: 1_700_000_000,
        };
        let body = set.to_body();
        let back: SuggestionSet = serde_json::from_str(&body).expect("round-trip");
        assert_eq!(back, set);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["suggestions"][0]["title"], "t");
        assert_eq!(v["produced_at_s"], 1_700_000_000_u64);
    }

    #[tokio::test]
    async fn generate_suggestions_none_when_not_leader() {
        // Leader-only (Q73): on a bare tempdir no lock can be taken, so a fresh
        // worker is never leader → no proactive pass, no codex spawn.
        let tmp = tempfile::tempdir().unwrap();
        let w = CopilotWorker::new(tmp.path().to_path_buf(), "n1".into())
            .with_db_path(tmp.path().join("audit.db"))
            .with_codex_bin("definitely-not-a-real-codex-binary-xyz");
        assert!(
            w.generate_suggestions().await.is_none(),
            "a non-leader publishes no suggestions"
        );
    }

    #[test]
    fn cadences_are_moderate_not_spammy() {
        // Q61 — the proactive cadence is minutes, not a tight loop; the status
        // cadence is cheap-but-frequent. Lock both so a future edit can't silently
        // make Copilot spam the operator.
        assert!(
            DEFAULT_SUGGESTION_INTERVAL >= Duration::from_secs(60),
            "proactive cadence must be moderate (minutes), got {DEFAULT_SUGGESTION_INTERVAL:?}"
        );
        assert!(DEFAULT_STATUS_INTERVAL <= Duration::from_secs(30));
        assert!(
            MAX_SUGGESTIONS <= 5,
            "high-confidence => a SMALL ranked set"
        );
        // FD-13 — the triage cadence is moderate too (minutes, not a tight loop).
        assert!(
            DEFAULT_TRIAGE_INTERVAL >= Duration::from_secs(60),
            "triage cadence must be moderate (minutes), got {DEFAULT_TRIAGE_INTERVAL:?}"
        );
    }

    // ===================== FD-13 — alert triage =====================

    #[test]
    fn triage_topic_is_state_namespaced_and_not_the_exec_topic() {
        // §9 backstop: the triage is a STATE snapshot the tile reads, NEVER FD-11's
        // execution topic — no path from a triage to execution without operator
        // approval through the confirm gate.
        assert_eq!(ALERT_TRIAGE_TOPIC, "state/copilot/alert-triage");
        assert!(ALERT_TRIAGE_TOPIC.starts_with("state/"));
        assert_ne!(ALERT_TRIAGE_TOPIC, crate::workers::action::ACTION_TOPIC);
    }

    #[test]
    fn render_alerts_lists_status_detail_and_bounds() {
        let alerts: Vec<Alert> = (0..30)
            .map(|i| Alert {
                check: format!("check{i}"),
                status: if i % 2 == 0 { "fail" } else { "warn" }.into(),
                detail: format!("detail{i}"),
            })
            .collect();
        let s = render_alerts(&alerts);
        assert!(s.contains("check0 [fail]: detail0"));
        assert!(s.contains("check1 [warn]: detail1"));
        // Bounded: only MAX_ALERTS_IN_TRIAGE named, the rest summarized.
        assert!(
            s.contains(&format!("(+{} more alerts)", 30 - MAX_ALERTS_IN_TRIAGE)),
            "overflow summarized, not dumped: {s}"
        );
    }

    #[test]
    fn render_alerts_omits_detail_when_empty() {
        let s = render_alerts(&[Alert {
            check: "etcd".into(),
            status: "fail".into(),
            detail: String::new(),
        }]);
        assert_eq!(s, "- etcd [fail]");
    }

    #[test]
    fn triage_prompt_grounds_lists_alerts_and_teaches_the_proposal_protocol() {
        let alerts = [Alert {
            check: "mfsmaster".into(),
            status: "fail".into(),
            detail: "process down on oak".into(),
        }];
        let p = compose_triage_prompt("Leader: peer:oak\nNodes: oak[host,degraded]", &alerts);
        // Grounded with the live mesh state.
        assert!(p.contains("Live mesh state:"));
        assert!(p.contains("oak[host,degraded]"));
        // Lists the active alerts.
        assert!(p.contains("Active alerts"));
        assert!(p.contains("mfsmaster [fail]: process down on oak"));
        // Demands grouping + explanation + worst-first, with an empty-OK.
        assert!(p.contains("GROUP"));
        assert!(p.contains("EXPLAIN"));
        assert!(p.contains("Return []"));
        // The §9 proposal protocol is taught (reused from the system context).
        assert!(p.contains("```action"));
        assert!(p.contains("service_lifecycle"));
    }

    #[test]
    fn parse_triage_pulls_groups_with_alerts_and_severity() {
        let answer = "Here is the triage:\n```json\n[\
            {\"title\":\"MeshFS master down\",\"explanation\":\"mfsmaster is not running on oak\",\
             \"severity\":\"high\",\"alerts\":[\"mfsmaster\",\"meshfs-mount\"]},\
            {\"title\":\"cert expiring\",\"explanation\":\"the CA cert warns soon\",\
             \"severity\":\"medium\",\"alerts\":[\"ca-cert\"]}\
            ]\n```\nthat's all";
        let got = parse_triage(answer);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].title, "MeshFS master down");
        assert_eq!(got[0].severity, "high");
        assert_eq!(got[0].alerts, vec!["mfsmaster", "meshfs-mount"]);
        assert!(got[0].proposal.is_none(), "explain-only group");
        assert_eq!(got[1].severity, "medium");
    }

    #[test]
    fn parse_triage_extracts_typed_fix_proposal_inline() {
        // A group whose `explanation` carries an `action`-fenced block becomes a
        // typed FD-12 proposal (gated by FD-11's allowlist), stripped from the
        // displayed explanation — the SAME path an ask/suggestion uses. Queued,
        // never executed (§9).
        let answer = "[{\"title\":\"mfsmaster wedged\",\"explanation\":\"restart it on oak.\\n\
            ```action\\n\
            {\\\"kind\\\":\\\"service_lifecycle\\\",\\\"target_host\\\":\\\"oak\\\",\
            \\\"service_kind\\\":\\\"container\\\",\\\"name\\\":\\\"mfsmaster\\\",\\\"op\\\":\\\"restart\\\"}\\n\
            ```\",\"severity\":\"high\",\"alerts\":[\"mfsmaster\"]}]";
        let got = parse_triage(answer);
        assert_eq!(got.len(), 1);
        let g = &got[0];
        assert!(
            !g.explanation.contains("```"),
            "fence stripped from explanation: {:?}",
            g.explanation
        );
        let p = g
            .proposal
            .as_ref()
            .expect("a typed fix proposal was extracted");
        assert_eq!(p.action.kind_tag(), "service_lifecycle");
    }

    #[test]
    fn parse_triage_drops_un_allowlisted_fix_keeps_explanation() {
        // §9: an un-allowlisted action in a triage group is dropped (the operator
        // still gets the explanation), never proposed.
        let answer = "[{\"title\":\"danger\",\"explanation\":\"wipe it.\\n\
            ```action\\n{\\\"kind\\\":\\\"raw_shell\\\",\\\"cmd\\\":\\\"rm -rf /\\\"}\\n```\",\
            \"severity\":\"high\",\"alerts\":[\"disk\"]}]";
        let got = parse_triage(answer);
        assert_eq!(got.len(), 1);
        assert!(
            got[0].proposal.is_none(),
            "un-allowlisted kind must not be proposed"
        );
        assert!(got[0].explanation.contains("wipe it"));
    }

    #[test]
    fn parse_triage_caps_and_tolerates_garbage() {
        let items: Vec<String> = (0..20)
            .map(|i| format!("{{\"title\":\"g{i}\",\"explanation\":\"e\",\"severity\":\"high\"}}"))
            .collect();
        let answer = format!("[{}]", items.join(","));
        assert_eq!(parse_triage(&answer).len(), MAX_TRIAGE_GROUPS);
        assert!(parse_triage("not json").is_empty());
        assert!(parse_triage("[]").is_empty());
        assert!(parse_triage("{\"title\":\"x\"}").is_empty());
    }

    #[test]
    fn triage_body_round_trips() {
        let triage = AlertTriage {
            groups: vec![AlertGroup {
                title: "t".into(),
                explanation: "e".into(),
                severity: "high".into(),
                alerts: vec!["a".into()],
                proposal: None,
            }],
            alert_count: 3,
            produced_at_s: 1_700_000_000,
        };
        let body = triage.to_body();
        let back: AlertTriage = serde_json::from_str(&body).expect("round-trip");
        assert_eq!(back, triage);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["groups"][0]["title"], "t");
        assert_eq!(v["alert_count"], 3);
    }

    #[test]
    fn read_alerts_keeps_only_non_ok_health_checks() {
        // FD-13 end-to-end (the read half): seed the health plane with ok + non-ok
        // checks, then read — proves the triage consumes REAL alerts (§7), not a
        // placeholder, and drops the ok ones.
        let tmp = tempfile::tempdir().unwrap();
        let persist = Persist::open(tmp.path().join("bus")).expect("persist open");
        let mk = |check: &str, status: &str, detail: &str| {
            serde_json::json!({"check": check, "status": status, "detail": detail}).to_string()
        };
        persist
            .write(
                "event/dc/health/etcd",
                Priority::Default,
                None,
                Some(&mk("etcd", "ok", "")),
            )
            .unwrap();
        persist
            .write(
                "event/dc/health/mfsmaster",
                Priority::Default,
                None,
                Some(&mk("mfsmaster", "fail", "down on oak")),
            )
            .unwrap();
        persist
            .write(
                "event/dc/health/disk",
                Priority::Default,
                None,
                Some(&mk("disk", "warn", "/ at 90%")),
            )
            .unwrap();
        let alerts = CopilotWorker::read_alerts(&persist);
        assert_eq!(alerts.len(), 2, "the ok check is not an alert");
        // check-sorted: disk before mfsmaster.
        assert_eq!(alerts[0].check, "disk");
        assert_eq!(alerts[0].status, "warn");
        assert_eq!(alerts[1].check, "mfsmaster");
        assert_eq!(alerts[1].detail, "down on oak");
    }

    #[test]
    fn read_alerts_empty_when_plane_quiet() {
        let tmp = tempfile::tempdir().unwrap();
        let persist = Persist::open(tmp.path().join("bus")).expect("persist open");
        assert!(CopilotWorker::read_alerts(&persist).is_empty());
    }

    #[tokio::test]
    async fn generate_triage_none_when_not_leader() {
        // Leader-only (Q73): on a bare tempdir no lock can be taken, so a fresh
        // worker is never leader → no triage pass, no codex spawn (even with alerts).
        let tmp = tempfile::tempdir().unwrap();
        let w = CopilotWorker::new(tmp.path().to_path_buf(), "n1".into())
            .with_db_path(tmp.path().join("audit.db"))
            .with_codex_bin("definitely-not-a-real-codex-binary-xyz");
        let alerts = vec![Alert {
            check: "etcd".into(),
            status: "fail".into(),
            detail: "down".into(),
        }];
        assert!(
            w.generate_triage(alerts).await.is_none(),
            "a non-leader publishes no triage"
        );
    }

    #[test]
    fn triage_published_on_the_bus_is_readable() {
        // End-to-end (the publish half): the worker publishes a triage the Front
        // Door's Alerts tile can read back off ALERT_TRIAGE_TOPIC.
        let tmp = tempfile::tempdir().unwrap();
        let persist = Persist::open(tmp.path().join("bus")).expect("persist open");
        let triage = AlertTriage {
            groups: vec![AlertGroup {
                title: "g".into(),
                explanation: "e".into(),
                severity: "high".into(),
                alerts: vec!["etcd".into()],
                proposal: None,
            }],
            alert_count: 1,
            produced_at_s: 7,
        };
        publish_triage(&persist, &triage);
        let msgs = persist.list_since(ALERT_TRIAGE_TOPIC, None).expect("list");
        assert_eq!(msgs.len(), 1, "exactly one triage published");
        let body = msgs[0].body.clone().expect("triage body");
        let back: AlertTriage = serde_json::from_str(&body).expect("parse triage");
        assert_eq!(back, triage);
    }
}
