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
//! ## Governance — ASK/SUGGEST only (`AI_GOVERNANCE` §9)
//!
//! Remote OS execution on the mesh is typed verbs + signed job bundles ONLY;
//! there is **no raw shell channel, ever**. This worker therefore *answers and
//! suggests* — it spawns the `codex` AI subprocess itself (that is the model, not
//! the user's machine) and returns the model's text. It does NOT run arbitrary
//! system commands on the operator's behalf. Typed, audited OS actions are the
//! separate action worker (FRONTDOOR-11), out of scope here.
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

use super::{ShutdownToken, Worker};

/// Bus action topic this worker drains.
///
/// Locked to the `action/<domain>/<verb>` Q96 + `rpc.rs` convention so the
/// workbench publishes via the canonical RPC caller (`rpc::publish_request`,
/// which rejects any topic outside `action/`).
pub const ACTION_TOPIC: &str = "action/copilot/ask";

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
/// string (full mesh-state grounding is `FRONTDOOR-10`/§16 — here we accept
/// whatever the tile sends and prepend a small system-context line).
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
/// without timing out.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AskReply {
    /// The model's answer text on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    /// Human-readable unavailability description on a degrade path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
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

/// Build a success reply JSON body.
#[must_use]
pub fn build_answer_reply(answer: String) -> String {
    let reply = AskReply {
        answer: Some(answer),
        error: None,
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
    };
    serde_json::to_string(&reply)
        .unwrap_or_else(|_| r#"{"error":"AI unavailable"}"#.to_string())
}

/// The system-context line prepended to every prompt. Keeps Copilot in its
/// ASK/SUGGEST lane (§9): the model answers + suggests, it does not get a raw
/// execution channel here. Full mesh-state grounding is a later task.
const SYSTEM_CONTEXT: &str = "You are Copilot, the operations assistant embedded in the Magic Mesh \
Front Door. Answer the operator's question or suggest a fix concisely. You can \
read and reason but you cannot execute any system action from here — actions go \
through a separate typed, audited path that the operator confirms.";

/// Assemble the prompt handed to `codex exec`: the system context, any
/// caller-supplied grounding, then the operator's question. Pure so the
/// composition is unit-testable without spawning codex.
#[must_use]
pub fn compose_prompt(req: &AskRequest) -> String {
    let mut out = String::new();
    out.push_str(SYSTEM_CONTEXT);
    out.push_str("\n\n");
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
        }
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
        let output = match tokio::time::timeout(self.codex_timeout, child.wait_with_output()).await {
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

    /// Handle one ask request end-to-end: parse → read key → invoke codex →
    /// reply body. Returns the JSON reply string to publish on `reply/<ulid>`.
    /// Every branch yields a reply (success or degrade) so the requester never
    /// hangs.
    async fn handle_ask(&self, body: &str) -> String {
        let req = match parse_ask_request(body) {
            Ok(r) => r,
            Err(e) => return build_unavailable_reply(&e),
        };
        let api_key = match self.read_codex_key() {
            Ok(Some(k)) => k,
            Ok(None) => {
                tracing::info!(
                    target: "mackesd::copilot",
                    "codex API key not in the secret-store yet; degrading to AI-unavailable",
                );
                return build_unavailable_reply("codex API key not sealed in the mesh store");
            }
            Err(e) => {
                tracing::warn!(target: "mackesd::copilot", error = %e, "secret-store read faulted");
                return build_unavailable_reply(&format!("secret-store fault: {e}"));
            }
        };
        let prompt = compose_prompt(&req);
        match self.invoke_codex(&api_key, &prompt).await {
            CodexOutcome::Answer(a) => build_answer_reply(a),
            CodexOutcome::Unavailable(reason) => build_unavailable_reply(&reason),
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
                    let mut replies: Vec<(String, String)> = Vec::with_capacity(asks.len());
                    for (ulid, body) in asks {
                        let reply = self.handle_ask(&body).await;
                        replies.push((ulid, reply));
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
    fn collect_asks(&self, persist: &Persist, cursor: &mut Option<String>) -> Vec<(String, String)> {
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

/// Phase 3 (sync): write each `(ulid, reply)` back on `reply/<ulid>`. Free
/// function (not a method) so the `&Persist` borrow is unambiguously scoped to
/// this synchronous call — never live across the codex `.await`.
fn write_replies(persist: &Persist, replies: Vec<(String, String)>) {
    for (ulid, reply) in replies {
        if let Err(e) = persist.write(&reply_topic(&ulid), Priority::Default, None, Some(&reply)) {
            tracing::warn!(
                target: "mackesd::copilot",
                ulid = %ulid,
                error = %e,
                "reply write failed",
            );
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
        let p = compose_prompt(&req);
        assert!(p.contains("You are Copilot"));
        assert!(p.contains("Question:"));
        assert!(p.ends_with("what's wedged?"), "prompt trimmed + appended: {p:?}");
        // No caller context → no Context block.
        assert!(!p.contains("Context:"));
    }

    #[test]
    fn compose_prompt_includes_context_block_when_present() {
        let req = AskRequest {
            prompt: "explain".into(),
            context: "alerts tile, disk_full on anvil".into(),
        };
        let p = compose_prompt(&req);
        assert!(p.contains("Context:"));
        assert!(p.contains("alerts tile, disk_full on anvil"));
    }

    #[test]
    fn answer_reply_shape_carries_answer_no_error() {
        let body = build_answer_reply("the mfsmaster is down; restart it".into());
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["answer"], "the mfsmaster is down; restart it");
        assert!(!v.as_object().unwrap().contains_key("error"));
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
            .with_poll_interval(Duration::from_millis(20));
        let (tx, rx) = tokio::sync::watch::channel(false);
        let token = ShutdownToken::from_receiver(rx);
        let _ = tx.send(true);
        let result = tokio::time::timeout(Duration::from_secs(3), w.run(token))
            .await
            .expect("worker must exit on shutdown");
        assert!(result.is_ok());
    }
}
