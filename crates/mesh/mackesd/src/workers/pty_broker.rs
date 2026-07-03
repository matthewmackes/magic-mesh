//! TERM-7 — the mackesd **mesh PTY-broker worker** (remote shells over the
//! Nebula overlay).
//!
//! Design: `docs/design/mesh-terminal.md` (the mesh-PTY section + Q3/Q9/Q18).
//! The `mde-term-egui` terminal surface gets **a shell on any mesh peer**: a
//! pane can run on a remote node over the encrypted overlay. Per §6 the
//! *lifecycle* is owned here, mesh-side — the desktop surface only **requests**
//! a session (typed verbs) and renders the returned bytes; the sealed key, the
//! ssh invocation, the reachability + reap concerns all stay in the mesh tier.
//!
//! ## What this worker owns
//!
//! * Drains `action/pty/<peer>` (the `action/<domain>/+` RPC shape, §9 — the
//!   `<peer>` is the topic's verb slot; the body carries a TYPED [`PtyVerb`],
//!   never a command string). The four verbs are the session lifecycle:
//!   `open` (spawn a remote shell), `write` (feed keystrokes), `resize`
//!   (window geometry), `close` (tear down). Each verb carries the
//!   **client-minted session `id`** so the surface can address the session it
//!   just opened without a round-trip.
//! * Publishes the per-session lifecycle to `state/pty/<id>` — an **append log**
//!   (not retained-latest): every output chunk is one record (base64 bytes +
//!   a monotonic `seq`), and the terminal record carries the child `exit`. The
//!   surface reads the log with a cursor (`list_since`) and feeds bytes → its VT
//!   engine in order (TERM-8).
//! * **Idle-reap** a live session untouched for [`Self::idle_timeout`], **dead-
//!   session reap** once the remote shell exits, and honest **typed gating** when
//!   a peer is unreachable / the key isn't provisioned — it NEVER fabricates a
//!   session (§7).
//!
//! ## §9 — typed verbs, no raw shell in the ACTION layer
//!
//! The worker never builds a shell-string from user data. Opening a remote shell
//! is a **typed argv** to `ssh` ([`plan_open`] → [`PtyPlan::args`]): `ssh -tt -i
//! <sealed-key> <opts> <user>@<peer>.mesh`, with **no trailing command** — ssh
//! runs the peer's login shell on a remote PTY (`-tt` forces remote PTY
//! allocation even though the broker has no local tty). The user's keystrokes
//! ride the session's byte stream, never the command line. Every side effect that
//! touches ssh goes through the injectable [`PtyBackend`] seam; the shared mesh
//! SSH key is resolved through FILEMGR-6's [`mesh_mount::KeyProvider`] (reused —
//! §6 glue, one sealed key). This keeps the pure folds ([`plan_open`],
//! [`transition`], [`idle_reap_due`], [`closed_reap_due`], [`parse_verb`])
//! unit-testable without a runtime, and lets the live ssh impl be honestly
//! **integration-gated**: on a headless build/CI box (no `ssh`, no provisioned
//! key, no reachable peer) [`SshPtyBackend`] returns a typed [`PtyError::Gated`]
//! — it NEVER fakes a session (§7).
//!
//! ## TERM-14 — session persistence + reattach
//!
//! A **brokered** (remote) session outlives its client: when the surface closes or
//! crashes the shell keeps running and its output keeps accruing into a bounded
//! per-session **ring buffer** ([`RING_CAP_BYTES`]). The client's liveness is a
//! [`PtyVerb::Heartbeat`] the surface repeats while a pane is attached; ceasing to
//! hear it past [`Self::client_grace`] flags the session *detached* in the
//! reattachable-session index the broker publishes on [`SESSIONS_TOPIC`]. A later
//! surface **reattaches** ([`PtyVerb::Reattach`]) to a still-running session — the
//! broker replays the ring as a one-shot scrollback snapshot and resumes streaming
//! the live PTY, so the SAME shell (buffered output + live bytes) is seen. An
//! [`PtyVerb::Detach`] is the polite close-but-keep. A session with **no client**
//! past [`Self::orphan_ttl`] is reaped (bounded cleanup of the abandoned); any
//! client signal resets that clock. Local panes never reach the broker, so they
//! stay ephemeral — only remote/brokered sessions persist.
//!
//! ## Scope (what this unit is NOT)
//!
//! A dropped/exited remote shell lands honestly in `Closed` with its exit code —
//! this worker does **not** auto-reconnect (a shell is stateful; reviving it
//! would hand back a *different* shell). A reattach reconnects a client to a shell
//! that is *still running*; it never revives a dead one. Because mackesd
//! `#![forbid(unsafe_code)]`
//! (no `pre_exec` for a local controlling PTY), a mid-session **remote** window-
//! change can't be propagated over the pipe-backed ssh transport; the `resize`
//! verb is fully wired + the geometry recorded, but the live reflow of an
//! already-running remote PTY is a documented limitation of the gated leg (the
//! session keeps its open-time geometry), never faked.

#![cfg(feature = "async-services")]

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;

use super::mesh_mount::{self, KeyProvider, MountError};
use super::{ShutdownToken, Worker};

/// The `action/pty/` RPC domain prefix this worker drains.
///
/// A request topic is `action/pty/<peer>` — the `<peer>` is the verb slot
/// (`action/<domain>/+`, `rpc.rs`), and the body carries the typed [`PtyVerb`]
/// (which itself carries the session id).
pub const ACTION_PREFIX: &str = "action/pty/";

/// The `state/pty/` publish prefix. One APPEND LOG per session
/// (`state/pty/<id>`) — output chunks + the terminal exit record, read with a
/// cursor by the surface.
pub const STATE_PREFIX: &str = "state/pty/";

/// Default poll cadence: the request drain + the output pump + reap tick.
///
/// A terminal wants low latency, so this is much tighter than `mesh_mount`'s 2 s
/// — output surfaces within one tick and keystrokes forward within one tick (the
/// overlay RTT dominates either way). Cheap at idle (one `list_topics`).
pub const DEFAULT_TICK_INTERVAL: Duration = Duration::from_millis(50);

/// Default idle window before a live-but-untouched session is reaped (bounded
/// resource cleanup).
///
/// Long, because a shell the user is *reading* (no I/O) must not die out from
/// under them — this reaps a genuinely-abandoned session, not a quiet one.
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// How long a terminal (`Closed`/`Unreachable`) entry lingers before it's
/// forgotten.
///
/// Long enough for the surface's cursor to observe the final record, short
/// enough to bound the map. The record itself is written durably+synchronously,
/// so this is only slack for the reader.
pub const DEFAULT_CLOSED_LINGER: Duration = Duration::from_secs(3);

/// Bounded ssh connect timeout (never a wedged open — the honest dead-end path).
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(8);

/// TERM-14 — how long the broker keeps a **client-less** (detached / crashed-away)
/// session alive before reaping it as orphaned.
///
/// Generous, so a user can reattach to a long job well after the surface closed;
/// bounded, so a genuinely abandoned ssh child + its remote shell don't linger
/// forever. Reattaching (or any client signal) resets the clock.
pub const DEFAULT_ORPHAN_TTL: Duration = Duration::from_secs(60 * 60);

/// TERM-14 — how long a live session may go without any client signal (a
/// [`PtyVerb::Heartbeat`] or other verb) before it reads *detached* in the
/// published index. A few missed heartbeats — a closed/crashed surface stops
/// heart-beating. Only flips the index flag; the reap is [`DEFAULT_ORPHAN_TTL`].
pub const DEFAULT_CLIENT_GRACE: Duration = Duration::from_secs(45);

/// TERM-14 — cap on a session's recent-output ring buffer: the bounded scrollback
/// replayed to a reattaching client. 256 KiB ≈ a few thousand lines; the oldest
/// bytes are evicted first.
pub const RING_CAP_BYTES: usize = 256 * 1024;

/// TERM-14 — the retained-latest topic the broker publishes its reattachable-
/// session index on: one JSON [`PtySessionIndex`] of the node's live sessions,
/// read by the surface's reattach picker. Deliberately NOT under [`STATE_PREFIX`]
/// so it never collides with a `state/pty/<id>` session log.
pub const SESSIONS_TOPIC: &str = "state/pty-sessions";

/// Fallback grid geometry when the `open` verb omits it (0 = unspecified).
pub const DEFAULT_COLS: u16 = 80;
/// Fallback grid geometry when the `open` verb omits it (0 = unspecified).
pub const DEFAULT_ROWS: u16 = 24;

/// Read chunk for the ssh→buffer pump. One kernel pipe buffer is ~64 KiB.
const READ_CHUNK: usize = 8192;

// ── the typed request verb ─────────────────────────────────────────────────

/// The typed body of an `action/pty/<peer>` request.
///
/// There is deliberately **no command/shell variant** (§9): `open` runs the
/// peer's login shell on a remote PTY, and every verb addresses a session by its
/// client-minted `id`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "verb", rename_all = "snake_case")]
pub enum PtyVerb {
    /// Open a remote shell on the topic's peer, keyed by `id`. `cols`/`rows` are
    /// the initial grid (0 → the [`DEFAULT_COLS`]/[`DEFAULT_ROWS`] fallback).
    Open {
        /// Client-minted session id (a ULID) — also the `state/pty/<id>` key.
        id: String,
        /// Initial grid columns (0 = unspecified).
        #[serde(default)]
        cols: u16,
        /// Initial grid rows (0 = unspecified).
        #[serde(default)]
        rows: u16,
    },
    /// Feed input bytes to a live session's shell. `data` is **base64** (terminal
    /// input is arbitrary bytes, not necessarily UTF-8).
    Write {
        /// Target session id.
        id: String,
        /// Base64-encoded input bytes.
        data: String,
    },
    /// Resize a session's grid geometry.
    Resize {
        /// Target session id.
        id: String,
        /// New grid columns.
        cols: u16,
        /// New grid rows.
        rows: u16,
    },
    /// Tear down a session (kill + reap the remote shell).
    Close {
        /// Target session id.
        id: String,
    },
    /// TERM-14 — detach a client WITHOUT tearing the session down: the surface is
    /// closing but the remote shell keeps running (+ buffering) for a later
    /// reattach. The polite counterpart of a crash (which stops [`Self::Heartbeat`]).
    Detach {
        /// Target session id.
        id: String,
    },
    /// TERM-14 — reattach a client to a still-running session: the broker replays
    /// the buffered output (the ring) as a one-shot scrollback snapshot and resumes
    /// streaming the live PTY. A no-op if the id isn't a live session.
    Reattach {
        /// Target session id.
        id: String,
    },
    /// TERM-14 — a client liveness ping: refreshes the session's client clock so a
    /// present-but-quiet client isn't flagged detached. A crashed surface stops
    /// sending these, which is how the broker notices the client is gone.
    Heartbeat {
        /// Target session id.
        id: String,
    },
    /// TERM-14 — ask the broker to (re)publish its reattachable-session index on
    /// [`SESSIONS_TOPIC`] (e.g. when a surface's reattach picker first opens).
    List,
}

impl PtyVerb {
    /// Stable tag for logs.
    #[must_use]
    pub const fn tag(&self) -> &'static str {
        match self {
            Self::Open { .. } => "open",
            Self::Write { .. } => "write",
            Self::Resize { .. } => "resize",
            Self::Close { .. } => "close",
            Self::Detach { .. } => "detach",
            Self::Reattach { .. } => "reattach",
            Self::Heartbeat { .. } => "heartbeat",
            Self::List => "list",
        }
    }

    /// The session id a verb carries, or `""` for the id-less [`Self::List`].
    #[must_use]
    pub fn id(&self) -> &str {
        match self {
            Self::Open { id, .. }
            | Self::Write { id, .. }
            | Self::Resize { id, .. }
            | Self::Close { id }
            | Self::Detach { id }
            | Self::Reattach { id }
            | Self::Heartbeat { id } => id,
            Self::List => "",
        }
    }
}

/// Parse a typed pty request body. Unlike a mount (which defaults an empty body
/// to `mount`), a pty verb has no sensible default — every verb carries an id —
/// so an empty/malformed body is an error.
///
/// # Errors
/// A malformed or empty body surfaces as a human-readable string.
pub fn parse_verb(body: &str) -> Result<PtyVerb, String> {
    if body.trim().is_empty() {
        return Err("empty pty request body".to_string());
    }
    serde_json::from_str(body).map_err(|e| format!("malformed pty request: {e}"))
}

// ── the ssh invocation plan (pure) ─────────────────────────────────────────

/// A fully-resolved, executable remote-shell plan — the pure output of
/// [`plan_open`]. The [`PtyBackend`] seam turns this into an `ssh` invocation;
/// nothing here spawns a process or touches the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtyPlan {
    /// Short peer hostname (the roster key + topic verb slot).
    pub peer: String,
    /// The session id (for logs/diagnostics; NOT passed to the remote).
    pub id: String,
    /// The ssh login target, `user@<peer>.mesh`.
    pub target: String,
    /// The shared mesh SSH identity file the session authenticates with.
    pub identity_key: PathBuf,
    /// The full, typed `ssh` argv (§9 — no shell-interpolated command; the last
    /// element is the target, so ssh runs the peer's login shell on a PTY).
    pub args: Vec<String>,
    /// Initial grid columns.
    pub cols: u16,
    /// Initial grid rows.
    pub rows: u16,
}

/// Build the (pure) remote-shell plan for a `peer`/`id` session.
///
/// `mesh_user` + `identity_key` come from FILEMGR-6's shared sealed key. The argv
/// is the tuned overlay-ssh set: `-tt` (force remote PTY even without a local
/// tty), the sealed identity (with `IdentitiesOnly` so the agent isn't consulted),
/// first-use host-key pinning that never touches the operator's `known_hosts`,
/// `BatchMode` (never prompt), a bounded `ConnectTimeout`, and `ServerAlive*`
/// keepalives so a dropped transport is detected. **No trailing command** — the
/// remote login shell owns the PTY (§9).
#[must_use]
pub fn plan_open(
    peer: &str,
    id: &str,
    mesh_user: &str,
    identity_key: &Path,
    cols: u16,
    rows: u16,
    connect_timeout: Duration,
) -> PtyPlan {
    let fqdn = format!("{peer}.{}", super::mesh_dns::MESH_SUFFIX);
    let target = format!("{mesh_user}@{fqdn}");
    let connect = connect_timeout.as_secs().max(1);
    let args = vec![
        // Force remote PTY allocation even though the broker has no local tty.
        "-tt".to_string(),
        "-i".to_string(),
        identity_key.display().to_string(),
        // Only the sealed key — never fall through to a user agent/default id.
        "-o".to_string(),
        "IdentitiesOnly=yes".to_string(),
        // The overlay peer's host key rotates on re-enrollment; accept-new keeps
        // first-use pinning without a stale-key hard-fail, and we never persist
        // it to the operator's known_hosts.
        "-o".to_string(),
        "StrictHostKeyChecking=accept-new".to_string(),
        "-o".to_string(),
        "UserKnownHostsFile=/dev/null".to_string(),
        "-o".to_string(),
        "BatchMode=yes".to_string(),
        "-o".to_string(),
        format!("ConnectTimeout={connect}"),
        "-o".to_string(),
        "ServerAliveInterval=15".to_string(),
        "-o".to_string(),
        "ServerAliveCountMax=3".to_string(),
        // The login target is the LAST arg — nothing after it, so ssh runs the
        // peer's login shell interactively; there is no command string (§9).
        target.clone(),
    ];
    PtyPlan {
        peer: peer.to_string(),
        id: id.to_string(),
        target,
        identity_key: identity_key.to_path_buf(),
        args,
        cols: cols.max(1),
        rows: rows.max(1),
    }
}

// ── the pure decision folds ────────────────────────────────────────────────

/// Idle-reap decision: a live session untouched (no input/output/resize) for at
/// least `idle_timeout` is due to be reaped.
#[must_use]
pub fn idle_reap_due(idle_elapsed: Duration, idle_timeout: Duration) -> bool {
    idle_elapsed >= idle_timeout
}

/// Terminal-entry reap: a `Closed`/`Unreachable` session lingered at least
/// `linger` and is due to be forgotten.
#[must_use]
pub fn closed_reap_due(closed_elapsed: Duration, linger: Duration) -> bool {
    closed_elapsed >= linger
}

/// TERM-14 orphan-reap decision: a session whose client has been gone (no verb /
/// heartbeat) for at least `orphan_ttl` is due to be reaped. Kept a pure fold so
/// the "no client past a TTL is reaped" acceptance is unit-tested without a clock.
#[must_use]
pub fn orphan_reap_due(client_absent: Duration, orphan_ttl: Duration) -> bool {
    client_absent >= orphan_ttl
}

// ── the state machine (pure) ───────────────────────────────────────────────

/// The lifecycle phase of one session. Published (via [`Self::tag`]) on
/// `state/pty/<id>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PtyPhase {
    /// An open attempt is in flight (ssh spawning).
    Opening,
    /// The remote shell is live; bytes pump both ways.
    Open,
    /// The remote shell ended (exit recorded) or was torn down. Terminal.
    Closed,
    /// The peer is offline / the key isn't provisioned — an honest dead-end that
    /// never became a session. Terminal.
    Unreachable,
}

impl PtyPhase {
    /// The wire tag for the published state record.
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Opening => "opening",
            Self::Open => "open",
            Self::Closed => "closed",
            Self::Unreachable => "unreachable",
        }
    }
}

/// The lifecycle events the state machine reacts to. Requests arrive on the Bus;
/// the rest are produced by the worker's own pump + backend outcomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PtyEvent {
    /// An `open` request.
    OpenReq,
    /// The backend spawned the session.
    OpenOk,
    /// The backend hit a transient open failure (no fast retry — a shell open is
    /// one-shot; the surface re-opens with a fresh id).
    OpenFailed,
    /// The peer is offline / the key isn't provisioned (an honest dead-end).
    Unreachable,
    /// The remote shell exited (a normal end, or a dropped transport surfacing as
    /// a non-zero exit).
    Exited,
    /// A live session's transport dropped (distinct from a clean exit; reserved
    /// for a future liveness probe — see the lifecycle diagram).
    Dropped,
    /// The idle window elapsed.
    IdleTimeout,
    /// TERM-14 — no client (heartbeat) past the orphan TTL; reap the abandoned
    /// session (distinct from an idle reap, which is I/O-quiet, not client-less).
    Orphaned,
    /// An explicit `close` request.
    CloseReq,
}

/// The side effect the worker must perform after a [`transition`]. The table is
/// pure; the worker executes the action against the [`PtyBackend`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PtyStepAction {
    /// Nothing to do.
    None,
    /// Spawn the remote shell.
    Open,
    /// Kill + reap the remote shell.
    Kill,
}

/// The pure state-transition table. `(phase, event) → (next_phase, action)`.
/// Fully unit-testable without a runtime, ssh, or a peer — this is the
/// load-bearing lifecycle logic the acceptance criteria pin.
///
/// Written one-transition-per-arm on purpose: the table reads as the lifecycle
/// diagram, so semantically-distinct arms that share a result stay separate.
#[must_use]
#[allow(clippy::match_same_arms)]
pub const fn transition(phase: PtyPhase, event: PtyEvent) -> (PtyPhase, PtyStepAction) {
    use PtyEvent as E;
    use PtyPhase as P;
    use PtyStepAction as A;
    match (phase, event) {
        // A fresh open request always drives toward Opening + a spawn.
        (_, E::OpenReq) => (P::Opening, A::Open),

        // Open attempt outcomes.
        (P::Opening, E::OpenOk) => (P::Open, A::None),
        (P::Opening, E::OpenFailed) => (P::Closed, A::None),
        (P::Opening, E::Unreachable) => (P::Unreachable, A::None),

        // A live session ending. A clean exit needs no kill (the child is already
        // reaped by the backend); a dropped transport is killed to reap ssh.
        (P::Open, E::Exited) => (P::Closed, A::None),
        (P::Open, E::Dropped) => (P::Closed, A::Kill),
        (P::Open, E::IdleTimeout) => (P::Closed, A::Kill),
        // TERM-14: an orphaned (client-less) session is reaped like an idle one.
        (P::Open, E::Orphaned) => (P::Closed, A::Kill),

        // Explicit close from any phase (idempotent teardown).
        (_, E::CloseReq) => (P::Closed, A::Kill),

        // Everything else is a no-op self-loop (e.g. an Exited while already
        // Closed, an IdleTimeout while Opening) — never a panic.
        (p, _) => (p, A::None),
    }
}

// ── the typed backend errors + injectable seams ────────────────────────────

/// A typed open/write/resize failure. A failed open surfaces as one of these
/// (never a fabricated session), and the worker folds it onto the honest phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PtyError {
    /// The peer is offline / the transport can't be established.
    Unreachable(String),
    /// A bounded operation hit its deadline.
    Timeout,
    /// The backend prerequisites aren't available on this box (no `ssh`, no
    /// provisioned key). The **honest headless gate** — the live session is
    /// integration-only; it is NEVER faked as success (§7).
    Gated(String),
    /// Any other backend fault, with an operator-readable message.
    Backend(String),
}

impl std::fmt::Display for PtyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable(m) => write!(f, "unreachable: {m}"),
            Self::Timeout => write!(f, "operation timed out"),
            Self::Gated(m) => write!(f, "session unavailable (gated): {m}"),
            Self::Backend(m) => write!(f, "backend error: {m}"),
        }
    }
}

impl std::error::Error for PtyError {}

impl From<MountError> for PtyError {
    /// FILEMGR-6's [`KeyProvider`] resolves the sealed key with a [`MountError`]
    /// (a shared seam, §6 reuse); map it onto the pty error surface so a
    /// missing/unsealed key becomes an honest [`PtyError::Gated`], not a fake
    /// session.
    fn from(e: MountError) -> Self {
        match e {
            MountError::Unreachable(m) => Self::Unreachable(m),
            MountError::Timeout => Self::Timeout,
            MountError::Stale => Self::Backend("stale".to_string()),
            MountError::Gated(m) => Self::Gated(m),
            MountError::Backend(m) => Self::Backend(m),
        }
    }
}

impl PtyError {
    /// Map a failed open onto the state event it should drive: a hard
    /// unreachable/gated dead-end vs a transient open failure.
    #[must_use]
    pub const fn as_open_event(&self) -> PtyEvent {
        match self {
            Self::Unreachable(_) | Self::Gated(_) => PtyEvent::Unreachable,
            Self::Timeout | Self::Backend(_) => PtyEvent::OpenFailed,
        }
    }
}

/// A live remote-shell handle.
///
/// The worker polls it each tick (non-blocking) and pumps its bytes onto the
/// Bus. Injectable so orchestration is tested with a fake and the live ssh impl
/// stays integration-gated.
pub trait PtySession: Send {
    /// Feed input bytes to the remote shell.
    ///
    /// # Errors
    /// A [`PtyError`] once the write side is gone (the shell exited).
    fn write(&mut self, data: &[u8]) -> Result<(), PtyError>;

    /// Update the session's grid geometry.
    ///
    /// # Errors
    /// A [`PtyError`] on a backend fault. (The pipe-backed live impl records the
    /// size and returns `Ok` — see the module note on the resize limitation.)
    fn resize(&mut self, cols: u16, rows: u16) -> Result<(), PtyError>;

    /// Non-blocking: drain any buffered output bytes (empty when none pending).
    fn read_output(&mut self) -> Vec<u8>;

    /// Non-blocking: the child's exit code once the remote shell has exited.
    fn poll_exit(&mut self) -> Option<i32>;

    /// Best-effort terminate + reap (idempotent). No zombies, no leaked fds.
    fn kill(&mut self);
}

/// The ssh seam (§9 — the only place that spawns a process). Injectable so the
/// worker's orchestration is tested with a fake and the live impl stays gated.
pub trait PtyBackend: Send + Sync {
    /// Open a remote shell per `plan`. Returns a live session or a typed error;
    /// NEVER fakes a session.
    ///
    /// # Errors
    /// Any [`PtyError`]; on a headless box it is [`PtyError::Gated`].
    fn open(&self, plan: &PtyPlan) -> Result<Box<dyn PtySession>, PtyError>;
}

// ── the live, integration-gated backend ────────────────────────────────────

/// The live ssh remote-shell backend.
///
/// Integration-only: it honestly refuses on a box without `ssh` or the sealed
/// key with [`PtyError::Gated`], bounds the connect (`ConnectTimeout`), and never
/// fakes a session (§7). `-tt` gives the remote shell a real PTY over the pipe
/// transport.
#[derive(Debug, Clone, Default)]
pub struct SshPtyBackend;

impl SshPtyBackend {
    /// Construct the live backend.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Preflight the ssh client + the provisioned key. Returns the gate reason
    /// (as [`PtyError::Gated`]) when the live session can't run here — this is
    /// what keeps the farm/CI path honest.
    fn preflight(plan: &PtyPlan) -> Result<(), PtyError> {
        if !mesh_mount::binary_on_path("ssh") {
            return Err(PtyError::Gated("ssh client not found".to_string()));
        }
        if !plan.identity_key.is_file() {
            return Err(PtyError::Gated(format!(
                "mesh SSH key not provisioned at {} (FILEMGR-6)",
                plan.identity_key.display()
            )));
        }
        Ok(())
    }
}

impl PtyBackend for SshPtyBackend {
    fn open(&self, plan: &PtyPlan) -> Result<Box<dyn PtySession>, PtyError> {
        // Honest gate FIRST: on a box without ssh/key we refuse cleanly rather
        // than shell out into a failure (or a hang).
        Self::preflight(plan)?;
        let mut child = Command::new("ssh")
            .args(&plan.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| PtyError::Backend(format!("spawn ssh: {e}")))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| PtyError::Backend("ssh stdin unavailable".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| PtyError::Backend("ssh stdout unavailable".to_string()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| PtyError::Backend("ssh stderr unavailable".to_string()))?;
        // Merge the remote-shell bytes (stdout) AND ssh's own diagnostics
        // (stderr — "Connection refused" etc. surface honestly in the pane) into
        // one ordered buffer via two dedicated reader threads.
        let buffer = Arc::new(Mutex::new(VecDeque::<u8>::new()));
        let readers = vec![spawn_reader(stdout, &buffer), spawn_reader(stderr, &buffer)];
        Ok(Box::new(SshPtySession {
            child,
            stdin: Some(stdin),
            buffer,
            readers,
            exit: None,
        }))
    }
}

/// Spawn a thread pumping `src` → the shared output buffer until EOF.
fn spawn_reader<R: Read + Send + 'static>(
    mut src: R,
    buffer: &Arc<Mutex<VecDeque<u8>>>,
) -> JoinHandle<()> {
    let buffer = Arc::clone(buffer);
    std::thread::spawn(move || {
        let mut buf = [0_u8; READ_CHUNK];
        loop {
            match src.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => lock_unpoisoned(&buffer).extend(&buf[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
    })
}

/// A live ssh remote-shell session.
struct SshPtySession {
    child: std::process::Child,
    /// `Option` so [`Self::kill`] can drop the write side (EOF the remote) and
    /// stay idempotent.
    stdin: Option<std::process::ChildStdin>,
    buffer: Arc<Mutex<VecDeque<u8>>>,
    readers: Vec<JoinHandle<()>>,
    /// Cached once observed, so [`Self::poll_exit`] is stable + non-blocking.
    exit: Option<i32>,
}

impl PtySession for SshPtySession {
    fn write(&mut self, data: &[u8]) -> Result<(), PtyError> {
        let Some(stdin) = self.stdin.as_mut() else {
            return Err(PtyError::Backend("session write side closed".to_string()));
        };
        stdin
            .write_all(data)
            .and_then(|()| stdin.flush())
            .map_err(|e| PtyError::Backend(format!("write: {e}")))
    }

    fn resize(&mut self, _cols: u16, _rows: u16) -> Result<(), PtyError> {
        // mackesd `#![forbid(unsafe_code)]` → no `pre_exec` for a local
        // controlling PTY, so a mid-session remote window-change can't be
        // propagated over this pipe-backed transport (ssh would need a local tty
        // to catch SIGWINCH). The worker records the geometry; the running remote
        // PTY keeps its open-time size. Honest no-op — NOT a faked reflow.
        Ok(())
    }

    fn read_output(&mut self) -> Vec<u8> {
        lock_unpoisoned(&self.buffer).drain(..).collect()
    }

    fn poll_exit(&mut self) -> Option<i32> {
        if let Some(code) = self.exit {
            return Some(code);
        }
        match self.child.try_wait() {
            Ok(Some(status)) => {
                let code = status.code().unwrap_or(-1);
                self.exit = Some(code);
                Some(code)
            }
            _ => None,
        }
    }

    fn kill(&mut self) {
        // 1. EOF the remote (close ssh stdin), 2. SIGKILL + reap ssh, 3. join the
        //    reader threads so nothing outlives the session.
        drop(self.stdin.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
        for r in self.readers.drain(..) {
            let _ = r.join();
        }
    }
}

impl Drop for SshPtySession {
    fn drop(&mut self) {
        self.kill();
    }
}

/// Lock a mutex, riding through poisoning: a panicked reader thread must not
/// wedge the worker (the buffer stays readable; the session is already dying).
fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

// ── the published state record ─────────────────────────────────────────────

/// One record on the `state/pty/<id>` append log.
///
/// Output chunks carry `data` (base64) with `phase = open`; the terminal record
/// carries `exit`. `seq` is a per-session monotonic counter so the surface
/// orders + de-dupes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PtyState {
    /// The session id.
    pub id: String,
    /// The peer hostname the shell runs on.
    pub peer: String,
    /// The phase tag (`opening` / `open` / `closed` / `unreachable`).
    pub phase: String,
    /// Per-session monotonic sequence number.
    pub seq: u64,
    /// A base64 output chunk, when this record carries shell bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    /// The remote shell's exit code, on the terminal record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<i32>,
    /// A human-readable reason on a degrade path (unreachable/gated/dropped).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// TERM-14 — set on the reattach scrollback-replay record: the buffered ring
    /// handed to a reattaching client, distinct from a live output chunk. (The
    /// surface feeds it into the VT engine just the same; the flag is for clarity
    /// + observability.)
    #[serde(default, skip_serializing_if = "is_false")]
    pub snapshot: bool,
    /// Wall-clock epoch millis of this record.
    pub since_ms: u64,
}

/// Serde skip helper — omit a `false` flag from the wire so existing records are
/// byte-for-byte unchanged.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(b: &bool) -> bool {
    !*b
}

/// TERM-14 — one reattachable session in the broker's published index.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PtySessionSummary {
    /// The session id (`state/pty/<id>` key + the reattach target).
    pub id: String,
    /// The peer/node the shell runs on (the reattach picker groups by this).
    pub peer: String,
    /// The phase tag (`opening` / `open`).
    pub phase: String,
    /// Whether a client is currently attached (a heartbeat seen within the grace).
    pub attached: bool,
    /// Current grid columns.
    pub cols: u16,
    /// Current grid rows.
    pub rows: u16,
    /// Bytes of buffered scrollback in the ring (a reattach hint).
    pub buffered_bytes: u64,
}

/// TERM-14 — the retained-latest index published on [`SESSIONS_TOPIC`]: this
/// node's reattachable sessions. The surface reads the newest record.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PtySessionIndex {
    /// The live (reattachable) sessions, sorted by `(peer, id)` for a stable list.
    pub sessions: Vec<PtySessionSummary>,
    /// Wall-clock epoch millis of this snapshot.
    pub since_ms: u64,
}

// ── the worker ─────────────────────────────────────────────────────────────

/// Per-session live bookkeeping (worker-internal; the pure decisions are folded
/// out into the free fns above).
struct SessionEntry {
    peer: String,
    phase: PtyPhase,
    /// The live handle, present only while `Open`.
    session: Option<Box<dyn PtySession>>,
    cols: u16,
    rows: u16,
    /// Last time input/output/resize touched this session (idle clock).
    last_activity: Instant,
    /// Per-session monotonic sequence for the published records.
    seq: u64,
    /// When the session became terminal (`Closed`/`Unreachable`) — the reap clock.
    closed_at: Option<Instant>,
    /// The last degrade reason, surfaced in the published record.
    reason: Option<String>,
    /// TERM-14 — whether a client is currently attached (a heartbeat/verb seen
    /// within [`PtyBrokerWorker::client_grace`]). A detached session keeps running
    /// + buffering for a later reattach.
    attached: bool,
    /// TERM-14 — last time ANY client signal (a verb or a heartbeat) touched this
    /// session. Drives the `attached` flag and the orphan reap.
    last_client_seen: Instant,
    /// TERM-14 — bounded ring of recent output bytes: the scrollback replayed to a
    /// reattaching client, capped at [`RING_CAP_BYTES`] (oldest evicted first).
    ring: VecDeque<u8>,
}

impl SessionEntry {
    fn new(peer: String, cols: u16, rows: u16) -> Self {
        let now = Instant::now();
        Self {
            peer,
            // Starts Opening; the OpenReq event keeps it Opening and drives the
            // spawn (the (_, OpenReq) arm), landing Open or a terminal phase.
            phase: PtyPhase::Opening,
            session: None,
            cols: cols.max(1),
            rows: rows.max(1),
            last_activity: now,
            seq: 0,
            closed_at: None,
            reason: None,
            attached: true,
            last_client_seen: now,
            ring: VecDeque::new(),
        }
    }

    /// TERM-14 — append output to the bounded ring, evicting the oldest bytes once
    /// past the cap so a chatty long-running session never grows unbounded.
    fn push_ring(&mut self, bytes: &[u8]) {
        self.ring.extend(bytes.iter().copied());
        let overflow = self.ring.len().saturating_sub(RING_CAP_BYTES);
        if overflow > 0 {
            self.ring.drain(..overflow);
        }
    }

    /// TERM-14 — the buffered ring as a contiguous `Vec` (the reattach snapshot).
    fn ring_bytes(&self) -> Vec<u8> {
        self.ring.iter().copied().collect()
    }

    /// TERM-14 — record a client signal: refresh the client clock + mark attached.
    fn touch_client(&mut self) {
        self.last_client_seen = Instant::now();
        self.attached = true;
    }
}

/// TERM-7 — the mesh PTY-broker worker.
pub struct PtyBrokerWorker {
    /// Desktop runtime base (`/run/user/<uid>`) — where the sealed key is
    /// materialized.
    #[allow(dead_code)]
    runtime_base: PathBuf,
    /// The mesh SSH login user.
    mesh_user: String,
    /// The ssh backend seam.
    backend: Arc<dyn PtyBackend>,
    /// The shared-key seam (FILEMGR-6, reused from `mesh_mount` — §6 glue).
    keys: Arc<dyn KeyProvider>,
    /// Bounded ssh connect timeout.
    connect_timeout: Duration,
    /// Idle-reap window for a live session.
    idle_timeout: Duration,
    /// Terminal-entry linger before reap.
    closed_linger: Duration,
    /// TERM-14 — how long a client-less session lives before the orphan reap.
    orphan_ttl: Duration,
    /// TERM-14 — how long without a client signal before a session reads detached.
    client_grace: Duration,
    /// Poll/pump/reap cadence.
    tick: Duration,
    /// Bus spool root override (tests point this at a tempdir).
    bus_root_override: Option<PathBuf>,
    /// Per-session live state, keyed by session id.
    sessions: HashMap<String, SessionEntry>,
    /// Per-topic request cursors (`action/pty/<peer>` → last ULID).
    cursors: HashMap<String, String>,
    /// TERM-14 — set when the reattachable-session index needs republishing;
    /// flushed once per tick so a burst of changes is one publish.
    registry_dirty: bool,
}

impl PtyBrokerWorker {
    /// Construct with production seams + defaults. `runtime_base` is the desktop
    /// user's `/run/user/<uid>`; `repo_dir`/`workgroup_root` locate the secret
    /// store FILEMGR-6 sealed the shared key in.
    #[must_use]
    pub fn new(runtime_base: PathBuf, repo_dir: PathBuf, workgroup_root: PathBuf) -> Self {
        // Materialize the shared key under our own runtime path; the sealed
        // material is FILEMGR-6's, resolved via mesh_mount's provider (§6 reuse).
        let key_path = runtime_base.join("mde-pty").join(".mesh-ssh-key");
        let keys = Arc::new(mesh_mount::SecretStoreKeyProvider::new(
            key_path,
            repo_dir,
            workgroup_root,
        ));
        Self {
            runtime_base,
            mesh_user: mesh_mount::DEFAULT_MESH_USER.to_string(),
            backend: Arc::new(SshPtyBackend::new()),
            keys,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
            closed_linger: DEFAULT_CLOSED_LINGER,
            orphan_ttl: DEFAULT_ORPHAN_TTL,
            client_grace: DEFAULT_CLIENT_GRACE,
            tick: DEFAULT_TICK_INTERVAL,
            bus_root_override: None,
            sessions: HashMap::new(),
            cursors: HashMap::new(),
            registry_dirty: false,
        }
    }

    /// Inject the backend seam (tests use a fake).
    #[must_use]
    pub fn with_backend(mut self, backend: Arc<dyn PtyBackend>) -> Self {
        self.backend = backend;
        self
    }

    /// Inject the key-provider seam (tests use a fake).
    #[must_use]
    pub fn with_key_provider(mut self, keys: Arc<dyn KeyProvider>) -> Self {
        self.keys = keys;
        self
    }

    /// Override the Bus spool root (tests).
    #[must_use]
    pub fn with_bus_root(mut self, root: PathBuf) -> Self {
        self.bus_root_override = Some(root);
        self
    }

    /// Override the idle-reap window (tests use a short value).
    #[must_use]
    pub const fn with_idle_timeout(mut self, d: Duration) -> Self {
        self.idle_timeout = d;
        self
    }

    /// Override the terminal-entry linger (tests use a short value).
    #[must_use]
    pub const fn with_closed_linger(mut self, d: Duration) -> Self {
        self.closed_linger = d;
        self
    }

    /// TERM-14 — override the orphan-reap TTL (tests use a short value).
    #[must_use]
    pub const fn with_orphan_ttl(mut self, d: Duration) -> Self {
        self.orphan_ttl = d;
        self
    }

    /// TERM-14 — override the client-presence grace (tests use a short value).
    #[must_use]
    pub const fn with_client_grace(mut self, d: Duration) -> Self {
        self.client_grace = d;
        self
    }

    /// Override the poll/pump cadence (tests use a short value).
    #[must_use]
    pub const fn with_tick(mut self, d: Duration) -> Self {
        self.tick = d;
        self
    }

    /// Override the mesh SSH login user.
    #[must_use]
    pub fn with_mesh_user(mut self, user: impl Into<String>) -> Self {
        self.mesh_user = user.into();
        self
    }

    /// Publish a record for `id` on `state/pty/<id>`: bumps the session's `seq`,
    /// attaches an optional output chunk (base64) + exit code, and folds in the
    /// current phase/peer/reason.
    fn publish(&mut self, persist: &Persist, id: &str, data: Option<&[u8]>, exit: Option<i32>) {
        self.publish_rec(persist, id, data, exit, false);
    }

    /// TERM-14 — publish the reattach scrollback-replay record: the buffered ring
    /// as a single `snapshot` chunk (or a bare phase confirmation when empty), so a
    /// reattaching client repaints the buffered output then streams the live PTY.
    fn publish_snapshot(&mut self, persist: &Persist, id: &str, bytes: &[u8]) {
        let data = (!bytes.is_empty()).then_some(bytes);
        self.publish_rec(persist, id, data, None, true);
    }

    /// The record-publish core shared by [`Self::publish`] + [`Self::publish_snapshot`].
    fn publish_rec(
        &mut self,
        persist: &Persist,
        id: &str,
        data: Option<&[u8]>,
        exit: Option<i32>,
        snapshot: bool,
    ) {
        let Some(entry) = self.sessions.get_mut(id) else {
            return;
        };
        entry.seq += 1;
        let rec = PtyState {
            id: id.to_string(),
            peer: entry.peer.clone(),
            phase: entry.phase.tag().to_string(),
            seq: entry.seq,
            data: data.map(|b| B64.encode(b)),
            exit,
            reason: entry.reason.clone(),
            snapshot,
            since_ms: now_ms(),
        };
        let body = serde_json::to_string(&rec).unwrap_or_default();
        let topic = format!("{STATE_PREFIX}{id}");
        if let Err(e) = persist.write(&topic, Priority::Default, None, Some(&body)) {
            tracing::warn!(target: "mackesd::pty_broker", id, error = %e, "state publish failed");
        }
    }

    /// TERM-14 — publish the reattachable-session index on [`SESSIONS_TOPIC`]: the
    /// node's live (`opening`/`open`) sessions, sorted by `(peer, id)`, each with
    /// its attach state + buffered-scrollback hint. The surface's reattach picker
    /// reads the newest record and groups by peer.
    fn publish_registry(&mut self, persist: &Persist) {
        let mut sessions: Vec<PtySessionSummary> = self
            .sessions
            .iter()
            .filter(|(_, e)| matches!(e.phase, PtyPhase::Open | PtyPhase::Opening))
            .map(|(id, e)| PtySessionSummary {
                id: id.clone(),
                peer: e.peer.clone(),
                phase: e.phase.tag().to_string(),
                attached: e.attached,
                cols: e.cols,
                rows: e.rows,
                buffered_bytes: u64::try_from(e.ring.len()).unwrap_or(u64::MAX),
            })
            .collect();
        sessions.sort_by(|a, b| a.peer.cmp(&b.peer).then_with(|| a.id.cmp(&b.id)));
        let index = PtySessionIndex {
            sessions,
            since_ms: now_ms(),
        };
        let body = serde_json::to_string(&index).unwrap_or_default();
        if let Err(e) = persist.write(SESSIONS_TOPIC, Priority::Default, None, Some(&body)) {
            tracing::warn!(target: "mackesd::pty_broker", error = %e, "session index publish failed");
        }
    }

    /// Drive one event through the pure [`transition`] table + execute the
    /// resulting [`PtyStepAction`], then publish the new phase.
    fn apply(&mut self, persist: &Persist, id: &str, event: PtyEvent) {
        let Some(entry) = self.sessions.get_mut(id) else {
            return;
        };
        let (next, action) = transition(entry.phase, event);
        entry.phase = next;
        match action {
            PtyStepAction::None => {}
            PtyStepAction::Open => self.do_open(id),
            PtyStepAction::Kill => self.do_kill(id),
        }
        self.publish(persist, id, None, None);
    }

    /// Execute an open attempt: resolve the sealed key, build the pure plan, call
    /// the backend, and fold the typed outcome back into the phase. NEVER fakes a
    /// session — a gated/unreachable open lands honestly in a terminal phase.
    fn do_open(&mut self, id: &str) {
        let Some((peer, cols, rows)) = self
            .sessions
            .get(id)
            .map(|e| (e.peer.clone(), e.cols, e.rows))
        else {
            return;
        };
        let key = match self.keys.identity_key() {
            Ok(k) => k,
            Err(e) => {
                self.record_open_failure(id, &PtyError::from(e));
                return;
            }
        };
        let plan = plan_open(
            &peer,
            id,
            &self.mesh_user,
            &key,
            cols,
            rows,
            self.connect_timeout,
        );
        match self.backend.open(&plan) {
            Ok(session) => {
                if let Some(entry) = self.sessions.get_mut(id) {
                    entry.session = Some(session);
                    entry.phase = PtyPhase::Open;
                    entry.reason = None;
                    entry.closed_at = None;
                    entry.last_activity = Instant::now();
                }
                tracing::info!(target: "mackesd::pty_broker", id, peer = %peer, "remote shell opened");
            }
            Err(e) => self.record_open_failure(id, &e),
        }
    }

    /// Fold a typed open failure into an honest terminal phase (a hard
    /// unreachable/gated dead-end vs a transient open failure).
    fn record_open_failure(&mut self, id: &str, err: &PtyError) {
        let event = err.as_open_event();
        let Some(entry) = self.sessions.get_mut(id) else {
            return;
        };
        entry.reason = Some(err.to_string());
        let (next, _) = transition(entry.phase, event);
        entry.phase = next;
        entry.closed_at = Some(Instant::now());
        tracing::warn!(
            target: "mackesd::pty_broker",
            id,
            phase = next.tag(),
            error = %err,
            "open attempt failed",
        );
    }

    /// Best-effort teardown (idempotent). Kills the live ssh session if present.
    fn do_kill(&mut self, id: &str) {
        if let Some(entry) = self.sessions.get_mut(id) {
            if let Some(mut session) = entry.session.take() {
                session.kill();
            }
        }
    }

    /// Apply one typed verb to a session.
    fn handle_verb(&mut self, persist: &Persist, peer: &str, verb: PtyVerb) {
        match verb {
            PtyVerb::Open { id, cols, rows } => {
                if self.sessions.contains_key(&id) {
                    // A duplicate open for a live id — ignore (the surface mints a
                    // fresh id per session; a replay must not spawn a second shell).
                    return;
                }
                let cols = if cols == 0 { DEFAULT_COLS } else { cols };
                let rows = if rows == 0 { DEFAULT_ROWS } else { rows };
                self.sessions
                    .insert(id.clone(), SessionEntry::new(peer.to_string(), cols, rows));
                self.apply(persist, &id, PtyEvent::OpenReq);
                self.registry_dirty = true;
            }
            PtyVerb::Write { id, data } => {
                let bytes = match B64.decode(data.as_bytes()) {
                    Ok(b) => b,
                    Err(e) => {
                        tracing::warn!(target: "mackesd::pty_broker", id = %id, error = %e, "bad write payload");
                        return;
                    }
                };
                if let Some(entry) = self.sessions.get_mut(&id) {
                    if let Some(session) = entry.session.as_mut() {
                        if let Err(err) = session.write(&bytes) {
                            entry.reason = Some(err.to_string());
                        }
                        entry.last_activity = Instant::now();
                    }
                    // Input is both I/O activity AND proof a client is present.
                    entry.touch_client();
                }
            }
            PtyVerb::Resize { id, cols, rows } => {
                if let Some(entry) = self.sessions.get_mut(&id) {
                    entry.cols = cols.max(1);
                    entry.rows = rows.max(1);
                    if let Some(session) = entry.session.as_mut() {
                        let _ = session.resize(entry.cols, entry.rows);
                    }
                    entry.last_activity = Instant::now();
                    entry.touch_client();
                }
            }
            PtyVerb::Close { id } => {
                if self.sessions.contains_key(&id) {
                    self.apply(persist, &id, PtyEvent::CloseReq);
                    if let Some(entry) = self.sessions.get_mut(&id) {
                        entry.closed_at = Some(Instant::now());
                    }
                    self.registry_dirty = true;
                }
            }
            // TERM-14 — the client is leaving cleanly but wants the shell to keep
            // running. Mark detached (the reap clock runs from now); NEVER kill.
            PtyVerb::Detach { id } => {
                if let Some(entry) = self.sessions.get_mut(&id) {
                    entry.last_client_seen = Instant::now();
                    entry.attached = false;
                    self.registry_dirty = true;
                    tracing::debug!(target: "mackesd::pty_broker", id = %id, "client detached; session kept alive");
                }
            }
            // TERM-14 — a client reclaims a still-running session: replay the ring
            // as a scrollback snapshot + resume streaming. A no-op for a gone /
            // terminal id (the surface then just opens fresh).
            PtyVerb::Reattach { id } => {
                let replay = self.sessions.get_mut(&id).and_then(|e| {
                    (e.phase == PtyPhase::Open).then(|| {
                        e.touch_client();
                        e.ring_bytes()
                    })
                });
                if let Some(bytes) = replay {
                    self.publish_snapshot(persist, &id, &bytes);
                    self.registry_dirty = true;
                    tracing::info!(target: "mackesd::pty_broker", id = %id, bytes = bytes.len(), "client reattached; replayed scrollback");
                } else {
                    tracing::debug!(target: "mackesd::pty_broker", id = %id, "reattach to a non-live session ignored");
                }
            }
            // TERM-14 — a liveness ping: refresh the client clock (a re-attach if it
            // had been flagged detached) so a present-but-quiet client isn't reaped.
            PtyVerb::Heartbeat { id } => {
                if let Some(entry) = self.sessions.get_mut(&id) {
                    let was_attached = entry.attached;
                    entry.touch_client();
                    if !was_attached {
                        self.registry_dirty = true;
                    }
                }
            }
            // TERM-14 — republish the index on demand (a picker just opened).
            PtyVerb::List => self.publish_registry(persist),
        }
    }

    /// Drain net-new requests across every `action/pty/<peer>` topic + run the
    /// output-pump/reap tick. Fully synchronous so the `&Persist` borrow is held
    /// across the whole sweep without breaking `Send`.
    fn sweep(&mut self, persist: &Persist) {
        self.drain_requests(persist);
        self.pump_sessions(persist);
    }

    /// Poll each request topic since its cursor, dispatching the typed verb.
    fn drain_requests(&mut self, persist: &Persist) {
        let topics = match persist.list_topics() {
            Ok(t) => t,
            Err(e) => {
                tracing::debug!(target: "mackesd::pty_broker", error = %e, "list_topics failed");
                return;
            }
        };
        for topic in topics
            .into_iter()
            .filter(|t| t.starts_with(ACTION_PREFIX) && t.len() > ACTION_PREFIX.len())
        {
            let peer = topic[ACTION_PREFIX.len()..].to_string();
            let cursor = self.cursors.get(&topic).cloned();
            let msgs = match persist.list_since(&topic, cursor.as_deref()) {
                Ok(m) => m,
                Err(e) => {
                    tracing::debug!(target: "mackesd::pty_broker", topic, error = %e, "list_since failed");
                    continue;
                }
            };
            for msg in msgs {
                self.cursors.insert(topic.clone(), msg.ulid.clone());
                let verb = match parse_verb(msg.body.as_deref().unwrap_or_default()) {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!(target: "mackesd::pty_broker", peer = %peer, error = %e, "bad request");
                        continue;
                    }
                };
                self.handle_verb(persist, &peer, verb);
            }
        }
    }

    /// The pump/reap tick: drain each live session's output onto the Bus, fold a
    /// remote exit into a terminal record, idle-reap live sessions, and forget
    /// lingered terminal entries.
    fn pump_sessions(&mut self, persist: &Persist) {
        let now = Instant::now();
        let ids: Vec<String> = self.sessions.keys().cloned().collect();
        for id in ids {
            // 1. Drain output BEFORE polling exit, so the final bytes before an
            //    exit are still published.
            let output = self
                .sessions
                .get_mut(&id)
                .and_then(|e| e.session.as_mut().map(|s| s.read_output()));
            if let Some(bytes) = output {
                if !bytes.is_empty() {
                    self.publish(persist, &id, Some(&bytes), None);
                    if let Some(entry) = self.sessions.get_mut(&id) {
                        entry.last_activity = now;
                        // TERM-14: accrue into the bounded scrollback ring so a
                        // reattaching client can be handed the recent output.
                        entry.push_ring(&bytes);
                    }
                }
            }

            // 2. Remote exit → terminal record carrying the exit code.
            let exit = self
                .sessions
                .get_mut(&id)
                .and_then(|e| e.session.as_mut().and_then(|s| s.poll_exit()));
            if let Some(code) = exit {
                if let Some(entry) = self.sessions.get_mut(&id) {
                    let (next, _) = transition(entry.phase, PtyEvent::Exited);
                    entry.phase = next;
                    entry.closed_at = Some(now);
                    entry.session = None;
                    // ssh's own connect failure exits 255 — surface it honestly so
                    // a dropped/unreachable peer reads as more than a bare code.
                    entry.reason =
                        (code == 255).then(|| "ssh transport error / peer unreachable".to_string());
                }
                self.publish(persist, &id, None, Some(code));
                self.registry_dirty = true;
                continue;
            }

            // 3a. TERM-14 detached-flag sweep: a live session with no client signal
            //     past the grace reads *detached* in the index (a closed/crashed
            //     surface stopped heart-beating). Flips the flag only — the reap is
            //     the orphan TTL below.
            if let Some(entry) = self.sessions.get_mut(&id) {
                if entry.attached
                    && entry.phase == PtyPhase::Open
                    && now.duration_since(entry.last_client_seen) >= self.client_grace
                {
                    entry.attached = false;
                    self.registry_dirty = true;
                }
            }

            // 3b. TERM-14 orphan-reap: a live session whose client has been gone
            //     past the orphan TTL is reaped (kill + Closed). Any client signal
            //     (verb/heartbeat/reattach) resets `last_client_seen`, so an
            //     actively-used or freshly-reattached session is never reaped here.
            let orphaned = self.sessions.get(&id).is_some_and(|e| {
                e.phase == PtyPhase::Open
                    && orphan_reap_due(now.duration_since(e.last_client_seen), self.orphan_ttl)
            });
            if orphaned {
                if let Some(entry) = self.sessions.get_mut(&id) {
                    entry.reason = Some("orphaned: no client reattached".to_string());
                }
                self.apply(persist, &id, PtyEvent::Orphaned);
                if let Some(entry) = self.sessions.get_mut(&id) {
                    entry.closed_at = Some(now);
                }
                self.registry_dirty = true;
                continue;
            }

            // 3c. Idle-reap a live session untouched past the window.
            let idle = self.sessions.get(&id).is_some_and(|e| {
                e.phase == PtyPhase::Open
                    && idle_reap_due(now.duration_since(e.last_activity), self.idle_timeout)
            });
            if idle {
                if let Some(entry) = self.sessions.get_mut(&id) {
                    entry.reason = Some("idle timeout".to_string());
                }
                self.apply(persist, &id, PtyEvent::IdleTimeout);
                if let Some(entry) = self.sessions.get_mut(&id) {
                    entry.closed_at = Some(now);
                }
                self.registry_dirty = true;
                continue;
            }

            // 4. Forget a lingered terminal entry.
            let reap = self.sessions.get(&id).is_some_and(|e| {
                e.closed_at
                    .is_some_and(|at| closed_reap_due(now.duration_since(at), self.closed_linger))
            });
            if reap {
                self.sessions.remove(&id);
                self.registry_dirty = true;
            }
        }

        // TERM-14: flush the reattachable-session index once per tick if anything
        // changed (an open/close/detach/reattach/orphan or an attach-flag flip).
        if self.registry_dirty {
            self.publish_registry(persist);
            self.registry_dirty = false;
        }
    }
}

#[async_trait::async_trait]
impl Worker for PtyBrokerWorker {
    fn name(&self) -> &'static str {
        "pty_broker"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let Some(bus_root) = self
            .bus_root_override
            .clone()
            .or_else(mde_bus::default_data_dir)
        else {
            tracing::debug!(target: "mackesd::pty_broker", "no bus root; worker idle");
            return Ok(());
        };
        let persist = match Persist::open(bus_root) {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!(target: "mackesd::pty_broker", error = %e, "persist open failed; worker idle");
                return Ok(());
            }
        };
        // Seed each existing request topic's cursor at its tail so a restart
        // doesn't replay + re-open stale sessions.
        if let Ok(topics) = persist.list_topics() {
            for topic in topics
                .into_iter()
                .filter(|t| t.starts_with(ACTION_PREFIX) && t.len() > ACTION_PREFIX.len())
            {
                if let Ok(Some(ulid)) = persist.latest_ulid(&topic) {
                    self.cursors.insert(topic, ulid);
                }
            }
        }
        // TERM-14: publish an initial (empty) index so the surface's reattach picker
        // sees a topic even before the first session opens.
        self.publish_registry(&persist);
        let mut tick = tokio::time::interval(self.tick);
        tick.tick().await; // burn the immediate first tick
        loop {
            tokio::select! {
                _ = tick.tick() => self.sweep(&persist),
                () = shutdown.wait() => break,
            }
        }
        // Clean shutdown: kill every live session so no orphaned ssh child (and
        // its remote shell) outlives the worker.
        let ids: Vec<String> = self.sessions.keys().cloned().collect();
        for id in ids {
            self.do_kill(&id);
        }
        Ok(())
    }
}

/// Resolve the desktop user's `/run/user/<uid>` runtime base (reused from
/// `mesh_mount` — the same seated session hosts the terminal + files surfaces).
#[must_use]
pub fn resolve_runtime_base() -> PathBuf {
    mesh_mount::resolve_runtime_base()
}

/// Wall-clock epoch millis for the published state record.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // ── pure plan fold (§9 — a typed argv, never a shell string) ─────────

    #[test]
    fn plan_open_builds_the_typed_ssh_argv() {
        let plan = plan_open(
            "oak",
            "01ABC",
            "root",
            Path::new("/keys/id"),
            100,
            40,
            Duration::from_secs(8),
        );
        assert_eq!(plan.target, "root@oak.mesh");
        assert_eq!(plan.cols, 100);
        assert_eq!(plan.rows, 40);
        // §9: force a remote PTY, use ONLY the sealed key, bound the connect.
        assert!(plan.args.contains(&"-tt".to_string()));
        assert!(plan.args.contains(&"/keys/id".to_string()));
        assert!(plan.args.contains(&"IdentitiesOnly=yes".to_string()));
        assert!(plan.args.contains(&"ConnectTimeout=8".to_string()));
        assert!(plan.args.contains(&"ServerAliveInterval=15".to_string()));
        // The login target is the LAST arg — there is NO trailing command string
        // for a remote shell to interpret (the user's bytes ride the PTY stream).
        assert_eq!(plan.args.last().unwrap(), "root@oak.mesh");
        // The client-minted id is never leaked into the remote invocation.
        assert!(!plan.args.iter().any(|a| a.contains("01ABC")));
    }

    #[test]
    fn plan_open_clamps_a_zero_geometry() {
        let plan = plan_open(
            "oak",
            "id",
            "root",
            Path::new("/k"),
            0,
            0,
            Duration::from_secs(1),
        );
        assert_eq!((plan.cols, plan.rows), (1, 1));
    }

    // ── verb parse ───────────────────────────────────────────────────────

    #[test]
    fn verb_parse_roundtrips_the_four_verbs() {
        assert_eq!(
            parse_verb(r#"{"verb":"open","id":"a","cols":80,"rows":24}"#).unwrap(),
            PtyVerb::Open {
                id: "a".into(),
                cols: 80,
                rows: 24
            }
        );
        // open with omitted geometry defaults to 0 (the worker fills the fallback).
        assert_eq!(
            parse_verb(r#"{"verb":"open","id":"a"}"#).unwrap(),
            PtyVerb::Open {
                id: "a".into(),
                cols: 0,
                rows: 0
            }
        );
        assert_eq!(
            parse_verb(r#"{"verb":"write","id":"a","data":"aGk="}"#).unwrap(),
            PtyVerb::Write {
                id: "a".into(),
                data: "aGk=".into()
            }
        );
        assert_eq!(
            parse_verb(r#"{"verb":"resize","id":"a","cols":120,"rows":50}"#).unwrap(),
            PtyVerb::Resize {
                id: "a".into(),
                cols: 120,
                rows: 50
            }
        );
        assert_eq!(
            parse_verb(r#"{"verb":"close","id":"a"}"#).unwrap(),
            PtyVerb::Close { id: "a".into() }
        );
        assert_eq!(parse_verb(r#"{"verb":"open","id":"a"}"#).unwrap().id(), "a");
    }

    #[test]
    fn verb_parse_rejects_empty_and_unknown() {
        assert!(parse_verb("").is_err());
        assert!(parse_verb("   ").is_err());
        assert!(parse_verb(r#"{"verb":"exec","id":"a"}"#).is_err());
        // a write without its id is malformed (no default).
        assert!(parse_verb(r#"{"verb":"write","data":"x"}"#).is_err());
    }

    // ── the state-transition table ──────────────────────────────────────

    #[test]
    fn transitions_cover_the_open_lifecycle() {
        // request → opening(open) → open
        assert_eq!(
            transition(PtyPhase::Opening, PtyEvent::OpenReq),
            (PtyPhase::Opening, PtyStepAction::Open)
        );
        assert_eq!(
            transition(PtyPhase::Opening, PtyEvent::OpenOk),
            (PtyPhase::Open, PtyStepAction::None)
        );
        // a transient open failure is a terminal Closed (no auto-retry).
        assert_eq!(
            transition(PtyPhase::Opening, PtyEvent::OpenFailed),
            (PtyPhase::Closed, PtyStepAction::None)
        );
        // an offline peer / unprovisioned key is an honest Unreachable dead-end.
        assert_eq!(
            transition(PtyPhase::Opening, PtyEvent::Unreachable),
            (PtyPhase::Unreachable, PtyStepAction::None)
        );
    }

    #[test]
    fn transitions_cover_exit_drop_idle_close() {
        // a clean exit needs no kill (the child is already reaped).
        assert_eq!(
            transition(PtyPhase::Open, PtyEvent::Exited),
            (PtyPhase::Closed, PtyStepAction::None)
        );
        // a dropped transport / idle reap kills to reap ssh.
        assert_eq!(
            transition(PtyPhase::Open, PtyEvent::Dropped),
            (PtyPhase::Closed, PtyStepAction::Kill)
        );
        assert_eq!(
            transition(PtyPhase::Open, PtyEvent::IdleTimeout),
            (PtyPhase::Closed, PtyStepAction::Kill)
        );
        // explicit close from anywhere is an idempotent teardown.
        assert_eq!(
            transition(PtyPhase::Open, PtyEvent::CloseReq),
            (PtyPhase::Closed, PtyStepAction::Kill)
        );
        assert_eq!(
            transition(PtyPhase::Opening, PtyEvent::CloseReq),
            (PtyPhase::Closed, PtyStepAction::Kill)
        );
    }

    #[test]
    fn unknown_transitions_are_noop_self_loops() {
        // an Exited while already Closed, an IdleTimeout while Opening — no panic.
        assert_eq!(
            transition(PtyPhase::Closed, PtyEvent::Exited),
            (PtyPhase::Closed, PtyStepAction::None)
        );
        assert_eq!(
            transition(PtyPhase::Opening, PtyEvent::IdleTimeout),
            (PtyPhase::Opening, PtyStepAction::None)
        );
        assert_eq!(
            transition(PtyPhase::Unreachable, PtyEvent::Exited),
            (PtyPhase::Unreachable, PtyStepAction::None)
        );
    }

    #[test]
    fn pty_error_maps_to_the_right_event() {
        assert_eq!(
            PtyError::Unreachable("x".into()).as_open_event(),
            PtyEvent::Unreachable
        );
        assert_eq!(
            PtyError::Gated("x".into()).as_open_event(),
            PtyEvent::Unreachable
        );
        assert_eq!(PtyError::Timeout.as_open_event(), PtyEvent::OpenFailed);
        assert_eq!(
            PtyError::Backend("x".into()).as_open_event(),
            PtyEvent::OpenFailed
        );
    }

    #[test]
    fn mount_error_maps_onto_the_pty_gate() {
        // FILEMGR-6's KeyProvider gate (an unsealed key) must become a Gated
        // pty error, never a fake session.
        assert_eq!(
            PtyError::from(MountError::Gated("no key".into())),
            PtyError::Gated("no key".into())
        );
        assert_eq!(
            PtyError::from(MountError::Unreachable("off".into())),
            PtyError::Unreachable("off".into())
        );
    }

    // ── pure reap folds ──────────────────────────────────────────────────

    #[test]
    fn idle_reap_decision_fires_at_the_window() {
        assert!(!idle_reap_due(
            Duration::from_secs(59),
            Duration::from_secs(60)
        ));
        assert!(idle_reap_due(
            Duration::from_secs(60),
            Duration::from_secs(60)
        ));
    }

    #[test]
    fn closed_reap_decision_fires_at_the_linger() {
        assert!(!closed_reap_due(
            Duration::from_secs(2),
            Duration::from_secs(3)
        ));
        assert!(closed_reap_due(
            Duration::from_secs(3),
            Duration::from_secs(3)
        ));
    }

    // ── the live backend is honestly gated (never fakes a session) ───────

    #[test]
    fn live_backend_never_fakes_a_session_when_key_absent() {
        // A plan pointing at a nonexistent identity key: the live backend MUST
        // refuse with a typed Gated error (never Ok) — the §7 honest gate.
        // Deterministic: the key-file preflight fails fast, no network, no spawn.
        let plan = plan_open(
            "nohost",
            "id",
            "root",
            Path::new("/nonexistent/definitely/not/a/key"),
            80,
            24,
            Duration::from_secs(1),
        );
        let backend = SshPtyBackend::new();
        let res = backend.open(&plan);
        assert!(res.is_err(), "headless live open must never succeed");
        assert!(matches!(res, Err(PtyError::Gated(_))));
    }

    // ── orchestration over fake seams (no ssh, no network) ───────────────

    /// Shared inspection state between a [`FakeBackend`] and the [`FakeSession`]
    /// it hands out, so a test can preload output/exit + observe writes/resizes.
    #[derive(Default)]
    struct Shared {
        writes: Vec<Vec<u8>>,
        resizes: Vec<(u16, u16)>,
        output: VecDeque<Vec<u8>>,
        exit: Option<i32>,
        killed: bool,
    }

    struct FakeBackend {
        fail: Option<PtyError>,
        shared: Arc<Mutex<Shared>>,
        opens: AtomicUsize,
    }

    impl FakeBackend {
        fn ok() -> Arc<Self> {
            Arc::new(Self {
                fail: None,
                shared: Arc::new(Mutex::new(Shared::default())),
                opens: AtomicUsize::new(0),
            })
        }
        fn failing(err: PtyError) -> Arc<Self> {
            Arc::new(Self {
                fail: Some(err),
                shared: Arc::new(Mutex::new(Shared::default())),
                opens: AtomicUsize::new(0),
            })
        }
    }

    impl PtyBackend for FakeBackend {
        fn open(&self, _plan: &PtyPlan) -> Result<Box<dyn PtySession>, PtyError> {
            self.opens.fetch_add(1, Ordering::SeqCst);
            if let Some(e) = &self.fail {
                return Err(e.clone());
            }
            Ok(Box::new(FakeSession {
                shared: Arc::clone(&self.shared),
            }))
        }
    }

    struct FakeSession {
        shared: Arc<Mutex<Shared>>,
    }

    impl PtySession for FakeSession {
        fn write(&mut self, data: &[u8]) -> Result<(), PtyError> {
            self.shared.lock().unwrap().writes.push(data.to_vec());
            Ok(())
        }
        fn resize(&mut self, cols: u16, rows: u16) -> Result<(), PtyError> {
            self.shared.lock().unwrap().resizes.push((cols, rows));
            Ok(())
        }
        fn read_output(&mut self) -> Vec<u8> {
            self.shared
                .lock()
                .unwrap()
                .output
                .pop_front()
                .unwrap_or_default()
        }
        fn poll_exit(&mut self) -> Option<i32> {
            self.shared.lock().unwrap().exit
        }
        fn kill(&mut self) {
            self.shared.lock().unwrap().killed = true;
        }
    }

    struct FakeKeys;
    impl KeyProvider for FakeKeys {
        fn identity_key(&self) -> Result<PathBuf, MountError> {
            Ok(PathBuf::from("/tmp/fake-mesh-key"))
        }
    }

    fn worker_with(backend: Arc<dyn PtyBackend>) -> PtyBrokerWorker {
        PtyBrokerWorker::new(
            PathBuf::from("/run/user/1000"),
            PathBuf::from("/nonexistent-repo"),
            PathBuf::from("/nonexistent-wg"),
        )
        .with_backend(backend)
        .with_key_provider(Arc::new(FakeKeys))
    }

    fn temp_persist() -> (tempfile::TempDir, Persist) {
        let dir = tempfile::tempdir().unwrap();
        let persist = Persist::open(dir.path().to_path_buf()).unwrap();
        (dir, persist)
    }

    /// Every `state/pty/<id>` record, decoded, in log order.
    fn states(persist: &Persist, id: &str) -> Vec<PtyState> {
        persist
            .list_since(&format!("{STATE_PREFIX}{id}"), None)
            .unwrap()
            .into_iter()
            .filter_map(|m| serde_json::from_str(m.body.as_deref().unwrap_or_default()).ok())
            .collect()
    }

    fn open_verb(id: &str) -> PtyVerb {
        PtyVerb::Open {
            id: id.to_string(),
            cols: 80,
            rows: 24,
        }
    }

    #[test]
    fn open_request_reaches_open_over_the_fake_backend() {
        let (_d, persist) = temp_persist();
        let backend = FakeBackend::ok();
        let mut w = worker_with(backend.clone());
        w.handle_verb(&persist, "oak", open_verb("s1"));
        assert_eq!(w.sessions["s1"].phase, PtyPhase::Open);
        assert!(w.sessions["s1"].session.is_some());
        assert_eq!(backend.opens.load(Ordering::SeqCst), 1);
        // an Open record was published.
        let recs = states(&persist, "s1");
        assert!(recs.last().is_some_and(|r| r.phase == "open"));
    }

    #[test]
    fn unreachable_peer_lands_in_unreachable_not_open() {
        let (_d, persist) = temp_persist();
        let backend = FakeBackend::failing(PtyError::Unreachable("offline".into()));
        let mut w = worker_with(backend);
        w.handle_verb(&persist, "gone", open_verb("s1"));
        assert_eq!(w.sessions["s1"].phase, PtyPhase::Unreachable);
        assert!(w.sessions["s1"].session.is_none());
        assert!(w.sessions["s1"].reason.is_some());
        let recs = states(&persist, "s1");
        assert!(recs.last().is_some_and(|r| r.phase == "unreachable"));
    }

    #[test]
    fn gated_backend_never_fakes_a_session() {
        let (_d, persist) = temp_persist();
        // The honest headless gate (no ssh / no key) — must land Unreachable with
        // a reason, never Open, never a session handle.
        let backend = FakeBackend::failing(PtyError::Gated("no ssh".into()));
        let mut w = worker_with(backend);
        w.handle_verb(&persist, "oak", open_verb("s1"));
        assert_eq!(w.sessions["s1"].phase, PtyPhase::Unreachable);
        assert!(w.sessions["s1"].session.is_none());
        assert!(w.sessions["s1"]
            .reason
            .as_deref()
            .unwrap()
            .contains("gated"));
    }

    #[test]
    fn output_bytes_are_published_base64_to_the_state_log() {
        let (_d, persist) = temp_persist();
        let backend = FakeBackend::ok();
        backend
            .shared
            .lock()
            .unwrap()
            .output
            .push_back(b"hello\x1b[0m".to_vec());
        let mut w = worker_with(backend);
        w.handle_verb(&persist, "oak", open_verb("s1"));
        w.pump_sessions(&persist);
        // a data record carrying the base64 of the chunk was appended.
        let recs = states(&persist, "s1");
        let data_rec = recs
            .iter()
            .find(|r| r.data.is_some())
            .expect("a data record");
        let decoded = B64.decode(data_rec.data.as_ref().unwrap()).unwrap();
        assert_eq!(decoded, b"hello\x1b[0m");
        assert_eq!(data_rec.phase, "open");
        // seq is monotonic across the log.
        let seqs: Vec<u64> = recs.iter().map(|r| r.seq).collect();
        assert!(
            seqs.windows(2).all(|w| w[0] < w[1]),
            "seq monotonic: {seqs:?}"
        );
    }

    #[test]
    fn remote_exit_publishes_exit_and_closes() {
        let (_d, persist) = temp_persist();
        let backend = FakeBackend::ok();
        let mut w = worker_with(backend.clone());
        w.handle_verb(&persist, "oak", open_verb("s1"));
        // the shell exits 0.
        backend.shared.lock().unwrap().exit = Some(0);
        w.pump_sessions(&persist);
        assert_eq!(w.sessions["s1"].phase, PtyPhase::Closed);
        assert!(w.sessions["s1"].session.is_none());
        let recs = states(&persist, "s1");
        let exit_rec = recs.iter().find(|r| r.exit.is_some()).expect("exit record");
        assert_eq!(exit_rec.exit, Some(0));
        assert_eq!(exit_rec.phase, "closed");
    }

    #[test]
    fn ssh_connect_failure_exit_255_carries_an_honest_reason() {
        let (_d, persist) = temp_persist();
        let backend = FakeBackend::ok();
        let mut w = worker_with(backend.clone());
        w.handle_verb(&persist, "oak", open_verb("s1"));
        backend.shared.lock().unwrap().exit = Some(255);
        w.pump_sessions(&persist);
        let recs = states(&persist, "s1");
        let exit_rec = recs.iter().find(|r| r.exit == Some(255)).expect("exit 255");
        assert!(exit_rec
            .reason
            .as_deref()
            .unwrap()
            .contains("transport error"));
    }

    #[test]
    fn write_forwards_decoded_bytes_to_the_session() {
        let (_d, persist) = temp_persist();
        let backend = FakeBackend::ok();
        let mut w = worker_with(backend.clone());
        w.handle_verb(&persist, "oak", open_verb("s1"));
        w.handle_verb(
            &persist,
            "oak",
            PtyVerb::Write {
                id: "s1".into(),
                data: B64.encode(b"ls -la\n"),
            },
        );
        assert_eq!(
            backend.shared.lock().unwrap().writes,
            vec![b"ls -la\n".to_vec()]
        );
    }

    #[test]
    fn resize_updates_tracked_geometry_and_the_session() {
        let (_d, persist) = temp_persist();
        let backend = FakeBackend::ok();
        let mut w = worker_with(backend.clone());
        w.handle_verb(&persist, "oak", open_verb("s1"));
        w.handle_verb(
            &persist,
            "oak",
            PtyVerb::Resize {
                id: "s1".into(),
                cols: 120,
                rows: 50,
            },
        );
        assert_eq!((w.sessions["s1"].cols, w.sessions["s1"].rows), (120, 50));
        assert_eq!(backend.shared.lock().unwrap().resizes, vec![(120, 50)]);
    }

    #[test]
    fn idle_session_is_reaped_and_killed() {
        let (_d, persist) = temp_persist();
        let backend = FakeBackend::ok();
        let mut w = worker_with(backend.clone()).with_idle_timeout(Duration::from_millis(0));
        w.handle_verb(&persist, "oak", open_verb("s1"));
        assert_eq!(w.sessions["s1"].phase, PtyPhase::Open);
        w.pump_sessions(&persist);
        // idle_timeout 0 → the session is reaped (killed) and lands Closed.
        assert_eq!(w.sessions["s1"].phase, PtyPhase::Closed);
        assert!(
            backend.shared.lock().unwrap().killed,
            "the ssh session was killed"
        );
    }

    #[test]
    fn explicit_close_kills_the_session() {
        let (_d, persist) = temp_persist();
        let backend = FakeBackend::ok();
        let mut w = worker_with(backend.clone());
        w.handle_verb(&persist, "oak", open_verb("s1"));
        w.handle_verb(&persist, "oak", PtyVerb::Close { id: "s1".into() });
        assert_eq!(w.sessions["s1"].phase, PtyPhase::Closed);
        assert!(backend.shared.lock().unwrap().killed);
        assert!(w.sessions["s1"].session.is_none());
    }

    #[test]
    fn terminal_entry_is_reaped_after_the_linger() {
        let (_d, persist) = temp_persist();
        let backend = FakeBackend::ok();
        let mut w = worker_with(backend).with_closed_linger(Duration::from_millis(0));
        w.handle_verb(&persist, "oak", open_verb("s1"));
        w.handle_verb(&persist, "oak", PtyVerb::Close { id: "s1".into() });
        assert!(w.sessions.contains_key("s1"));
        // linger 0 → the terminal entry is forgotten on the next pump.
        w.pump_sessions(&persist);
        assert!(!w.sessions.contains_key("s1"), "terminal entry reaped");
    }

    #[test]
    fn duplicate_open_does_not_spawn_a_second_shell() {
        let (_d, persist) = temp_persist();
        let backend = FakeBackend::ok();
        let mut w = worker_with(backend.clone());
        w.handle_verb(&persist, "oak", open_verb("s1"));
        w.handle_verb(&persist, "oak", open_verb("s1"));
        assert_eq!(
            backend.opens.load(Ordering::SeqCst),
            1,
            "id replay is ignored"
        );
    }

    #[test]
    fn drain_requests_dispatches_from_the_action_topic() {
        let (_d, persist) = temp_persist();
        let backend = FakeBackend::ok();
        let mut w = worker_with(backend.clone());
        // enqueue a real open request on the action topic.
        let body = serde_json::to_string(&open_verb("s1")).unwrap();
        persist
            .write("action/pty/oak", Priority::Default, None, Some(&body))
            .unwrap();
        w.sweep(&persist);
        assert_eq!(w.sessions["s1"].phase, PtyPhase::Open);
        assert_eq!(w.sessions["s1"].peer, "oak");
        assert_eq!(backend.opens.load(Ordering::SeqCst), 1);
    }

    // ── TERM-14: persistence + reattach + idle/TTL orphan reap ───────────

    /// The newest reattachable-session index the broker published.
    fn sessions_index(persist: &Persist) -> PtySessionIndex {
        let recs = persist.list_since(SESSIONS_TOPIC, None).unwrap();
        let body = recs
            .last()
            .expect("a session index record")
            .body
            .clone()
            .unwrap();
        serde_json::from_str(&body).unwrap()
    }

    #[test]
    fn verb_parse_roundtrips_the_reattach_verbs() {
        assert_eq!(
            parse_verb(r#"{"verb":"detach","id":"a"}"#).unwrap(),
            PtyVerb::Detach { id: "a".into() }
        );
        assert_eq!(
            parse_verb(r#"{"verb":"reattach","id":"a"}"#).unwrap(),
            PtyVerb::Reattach { id: "a".into() }
        );
        assert_eq!(
            parse_verb(r#"{"verb":"heartbeat","id":"a"}"#).unwrap(),
            PtyVerb::Heartbeat { id: "a".into() }
        );
        assert_eq!(parse_verb(r#"{"verb":"list"}"#).unwrap(), PtyVerb::List);
        // the id-less List has an empty id slot.
        assert_eq!(PtyVerb::List.id(), "");
        assert_eq!(PtyVerb::List.tag(), "list");
    }

    #[test]
    fn orphan_transition_reaps_a_client_less_session() {
        // Open + Orphaned → Closed + Kill (reap the abandoned ssh child).
        assert_eq!(
            transition(PtyPhase::Open, PtyEvent::Orphaned),
            (PtyPhase::Closed, PtyStepAction::Kill)
        );
        // Orphaned while already terminal is a no-op self-loop (never a panic).
        assert_eq!(
            transition(PtyPhase::Closed, PtyEvent::Orphaned),
            (PtyPhase::Closed, PtyStepAction::None)
        );
    }

    #[test]
    fn orphan_reap_decision_fires_at_the_ttl() {
        assert!(!orphan_reap_due(
            Duration::from_secs(59),
            Duration::from_secs(60)
        ));
        assert!(orphan_reap_due(
            Duration::from_secs(60),
            Duration::from_secs(60)
        ));
    }

    /// THE TWO-PHASE ACCEPTANCE — open a remote session → disconnect (drop the
    /// client) → reattach → the SAME running shell is seen (its buffered output +
    /// the live PTY), with no second shell ever spawned.
    #[test]
    fn session_survives_disconnect_and_reattach_sees_the_same_shell() {
        let (_d, persist) = temp_persist();
        let backend = FakeBackend::ok();
        // The remote shell has already produced a line of a long job.
        backend
            .shared
            .lock()
            .unwrap()
            .output
            .push_back(b"long-job: step 1\n".to_vec());
        let mut w = worker_with(backend.clone());

        // Phase 1 — open the remote session; its output accrues into the ring.
        w.handle_verb(&persist, "oak", open_verb("s1"));
        assert_eq!(w.sessions["s1"].phase, PtyPhase::Open);
        w.pump_sessions(&persist);
        assert_eq!(backend.opens.load(Ordering::SeqCst), 1);
        assert!(
            !w.sessions["s1"].ring.is_empty(),
            "output buffered into the ring while attached"
        );

        // Disconnect: the surface closes → a detach keeps the shell ALIVE.
        w.handle_verb(&persist, "oak", PtyVerb::Detach { id: "s1".into() });
        assert!(!w.sessions["s1"].attached, "flagged detached");
        assert_eq!(w.sessions["s1"].phase, PtyPhase::Open, "still running");
        assert!(
            w.sessions["s1"].session.is_some(),
            "the live shell was NOT torn down on disconnect"
        );

        // Phase 2 — reattach: the SAME shell (opens still 1), scrollback replayed.
        w.handle_verb(&persist, "oak", PtyVerb::Reattach { id: "s1".into() });
        assert_eq!(
            backend.opens.load(Ordering::SeqCst),
            1,
            "reattach reconnected the SAME shell — no second spawn"
        );
        assert!(w.sessions["s1"].attached, "reattached");
        let recs = states(&persist, "s1");
        let snap = recs
            .iter()
            .find(|r| r.snapshot)
            .expect("a reattach scrollback snapshot record");
        let replayed = B64.decode(snap.data.as_ref().unwrap()).unwrap();
        assert_eq!(replayed, b"long-job: step 1\n", "buffered output replayed");

        // The live PTY keeps streaming AFTER reattach — same running shell.
        backend
            .shared
            .lock()
            .unwrap()
            .output
            .push_back(b"step 2\n".to_vec());
        w.pump_sessions(&persist);
        let recs = states(&persist, "s1");
        assert!(
            recs.iter().any(|r| r
                .data
                .as_ref()
                .and_then(|d| B64.decode(d).ok())
                .is_some_and(|b| b == b"step 2\n")),
            "live output streamed after reattach"
        );
    }

    #[test]
    fn a_detached_long_job_keeps_running_and_buffering() {
        let (_d, persist) = temp_persist();
        let backend = FakeBackend::ok();
        let mut w = worker_with(backend.clone());
        w.handle_verb(&persist, "oak", open_verb("s1"));
        w.handle_verb(&persist, "oak", PtyVerb::Detach { id: "s1".into() });
        // The job keeps producing output while detached; pumps keep buffering it,
        // never reaping it (default TTLs are long).
        for i in 0..3 {
            backend
                .shared
                .lock()
                .unwrap()
                .output
                .push_back(format!("line {i}\n").into_bytes());
            w.pump_sessions(&persist);
        }
        assert_eq!(w.sessions["s1"].phase, PtyPhase::Open, "detached but alive");
        assert!(!backend.shared.lock().unwrap().killed);
        assert!(!w.sessions["s1"].ring.is_empty());
    }

    #[test]
    fn orphaned_session_is_reaped_after_the_ttl() {
        let (_d, persist) = temp_persist();
        let backend = FakeBackend::ok();
        let mut w = worker_with(backend.clone()).with_orphan_ttl(Duration::from_millis(0));
        w.handle_verb(&persist, "oak", open_verb("s1"));
        assert_eq!(w.sessions["s1"].phase, PtyPhase::Open);
        // With a 0 orphan TTL, the first pump finds a client-less session + reaps it.
        w.pump_sessions(&persist);
        assert_eq!(w.sessions["s1"].phase, PtyPhase::Closed);
        assert!(
            backend.shared.lock().unwrap().killed,
            "the abandoned remote shell was killed"
        );
        let recs = states(&persist, "s1");
        assert!(recs
            .iter()
            .any(|r| r.reason.as_deref().is_some_and(|m| m.contains("orphaned"))));
    }

    #[test]
    fn a_quiet_session_reads_detached_after_the_grace() {
        let (_d, persist) = temp_persist();
        let backend = FakeBackend::ok();
        let mut w = worker_with(backend).with_client_grace(Duration::from_millis(0));
        w.handle_verb(&persist, "oak", open_verb("s1"));
        assert!(w.sessions["s1"].attached);
        w.pump_sessions(&persist);
        assert!(
            !w.sessions["s1"].attached,
            "no client signal past the grace → detached"
        );
        // and the published index reflects the detached flag.
        let idx = sessions_index(&persist);
        let s = idx
            .sessions
            .iter()
            .find(|s| s.id == "s1")
            .expect("s1 in the index");
        assert!(!s.attached);
    }

    #[test]
    fn a_heartbeat_keeps_a_session_attached() {
        let (_d, persist) = temp_persist();
        let backend = FakeBackend::ok();
        let mut w = worker_with(backend).with_client_grace(Duration::from_millis(0));
        w.handle_verb(&persist, "oak", open_verb("s1"));
        w.pump_sessions(&persist);
        assert!(
            !w.sessions["s1"].attached,
            "flipped detached with a 0 grace"
        );
        // a heartbeat re-attaches (a present-but-quiet client).
        w.handle_verb(&persist, "oak", PtyVerb::Heartbeat { id: "s1".into() });
        assert!(
            w.sessions["s1"].attached,
            "heartbeat re-attached the client"
        );
    }

    #[test]
    fn index_lists_reattachable_sessions_per_node() {
        let (_d, persist) = temp_persist();
        let backend = FakeBackend::ok();
        let mut w = worker_with(backend);
        w.handle_verb(&persist, "oak", open_verb("s1"));
        w.handle_verb(&persist, "oak", open_verb("s2"));
        w.handle_verb(&persist, "birch", open_verb("s3"));
        // A List verb republishes the index immediately.
        w.handle_verb(&persist, "oak", PtyVerb::List);
        let idx = sessions_index(&persist);
        assert_eq!(idx.sessions.len(), 3);
        // Sorted by (peer, id): birch/s3, then oak/s1, oak/s2.
        assert_eq!(idx.sessions[0].peer, "birch");
        assert_eq!(idx.sessions[1].peer, "oak");
        assert!(idx.sessions.iter().all(|s| s.phase == "open"));
        let oak = idx.sessions.iter().filter(|s| s.peer == "oak").count();
        assert_eq!(oak, 2, "both oak sessions listed under the node");
    }

    #[test]
    fn the_output_ring_is_bounded() {
        let (_d, persist) = temp_persist();
        let backend = FakeBackend::ok();
        // A single chunk larger than the cap.
        backend
            .shared
            .lock()
            .unwrap()
            .output
            .push_back(vec![b'x'; RING_CAP_BYTES + 512]);
        let mut w = worker_with(backend);
        w.handle_verb(&persist, "oak", open_verb("s1"));
        w.pump_sessions(&persist);
        assert_eq!(
            w.sessions["s1"].ring.len(),
            RING_CAP_BYTES,
            "the scrollback ring is capped at the bound (oldest evicted)"
        );
    }

    #[test]
    fn reattach_to_an_unknown_session_is_a_harmless_noop() {
        let (_d, persist) = temp_persist();
        let backend = FakeBackend::ok();
        let mut w = worker_with(backend);
        // No panic, no snapshot record for a never-opened id.
        w.handle_verb(&persist, "oak", PtyVerb::Reattach { id: "ghost".into() });
        assert!(states(&persist, "ghost").is_empty());
    }
}
