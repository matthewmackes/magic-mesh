//! TERM-8 — the desktop half of the TERM-7 **mesh PTY-broker** contract: a real
//! shell on a mesh peer, driven over the Bus.
//!
//! The `mackesd` `pty_broker` worker (TERM-7) owns the remote-shell lifecycle
//! mesh-side; this module is the surface's client. Per §6 the desktop tier leans
//! inward on `mde-bus` only — never the `mackesd` daemon crate (its worker types
//! are gated behind the heavy `async-services` feature: tokio / etcd / ssh). So,
//! exactly as [`crate::roster`] mirrors the chat roster and `mde-files-egui`
//! mirrors the mesh-mount worker, the verb + state shapes here are **local serde
//! mirrors** of the worker's wire contract, cross-checked against the worker's
//! constants in the tests.
//!
//! ## The fold this drives (TERM-8 acceptance)
//!
//! [`RemotePty`] mirrors [`crate::pty::LocalPty`]'s surface so the TERM-3
//! [`crate::widget::TerminalWidget`] renders a remote pane through the **same**
//! VT engine + grid renderer (§6 — no second terminal emulator):
//!
//! * **open** — mint a client session id, publish `action/pty/<peer>`
//!   `{"verb":"open",…}`, land in [`RemoteStatus::Connecting`].
//! * **stream** — [`RemotePty::poll`] reads the `state/pty/<id>` append log from a
//!   cursor, base64-decodes each output chunk, and feeds the bytes into the owned
//!   [`Terminal`] (the reused engine) — exactly the bytes a `LocalPty` reader
//!   thread would feed.
//! * **write** — [`RemotePty::send_input`] publishes `{"verb":"write",…}` with the
//!   keystrokes base64-encoded.
//! * **resize** — [`RemotePty::resize`] reflows the engine grid and publishes
//!   `{"verb":"resize",…}` on a geometry change.
//! * **close** — [`RemotePty::close`] (and [`Drop`]) publishes `{"verb":"close",…}`
//!   so a closed tab tears the remote shell down.
//!
//! ## Honest states (§7 — never a faked session)
//!
//! Every published phase folds onto a typed [`RemoteStatus`]: `unreachable` (an
//! offline peer / unprovisioned key) and `closed` (the remote shell exited)
//! surface as honest terminal chips, never a fabricated shell. A **transient
//! transport drop** (ssh exit 255 after the session was live) triggers a bounded
//! client reconnect ([`RECONNECT_BUDGET`] fresh sessions), surfaced as
//! [`RemoteStatus::Reconnecting`]; a **clean** shell exit closes the pane like a
//! local one, and an exhausted / never-opened drop lands in [`RemoteStatus::Failed`]
//! so the reason stays on screen.
//!
//! The [`PtyBus`] seam is injectable so the whole open→stream→write→resize→close
//! fold + the base64→grid decode + the reconnect/error mapping are unit-tested
//! headless against a fake bus that records the verbs and replays a state log;
//! the live leg (a real overlay peer) is honestly integration-gated, as TERM-7's.

use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use serde::{Deserialize, Serialize};

use crate::engine::{Terminal, DEFAULT_SCROLLBACK};

/// The `action/pty/` request domain prefix. MUST equal the worker's
/// `mackesd::workers::pty_broker::ACTION_PREFIX` (cross-checked in tests).
pub const ACTION_PREFIX: &str = "action/pty/";

/// The `state/pty/` publish domain prefix. MUST equal the worker's
/// `mackesd::workers::pty_broker::STATE_PREFIX` (cross-checked in tests).
pub const STATE_PREFIX: &str = "state/pty/";

/// The request topic for one peer (`action/pty/<peer>`).
#[must_use]
pub fn action_topic(peer: &str) -> String {
    format!("{ACTION_PREFIX}{peer}")
}

/// The state topic for one session (`state/pty/<id>`).
#[must_use]
pub fn state_topic(id: &str) -> String {
    format!("{STATE_PREFIX}{id}")
}

/// How many fresh reconnect sessions a transient transport drop may spend before
/// the pane lands in an honest [`RemoteStatus::Failed`].
pub const RECONNECT_BUDGET: u32 = 3;

/// The ssh exit code a broken/refused transport surfaces with (the worker's own
/// convention — `ssh` exits 255 on a connect/transport failure). Distinguishes a
/// transient drop (reconnect) from a clean shell exit (close).
const SSH_TRANSPORT_EXIT: i32 = 255;

/// Default poll cadence for the state-log drain. A terminal wants low latency,
/// but a per-frame `SQLite` scan is wasteful, so the read throttles to this while
/// the widget repaints at ~30 fps; the overlay RTT dominates either way.
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(40);

// ── the typed request verb (mirror of the worker's `PtyVerb`) ────────────────

/// The typed body of an `action/pty/<peer>` request — a local mirror of the
/// worker's `PtyVerb` wire shape (`{"verb":…}`, `snake_case`). There is deliberately
/// **no** command/shell variant (§9): `open` runs the peer's login shell and every
/// verb addresses a session by its client-minted `id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "verb", rename_all = "snake_case")]
enum ClientVerb {
    /// Open a remote shell keyed by `id`, at the given initial grid.
    Open { id: String, cols: u16, rows: u16 },
    /// Feed base64-encoded input bytes to a live session.
    Write { id: String, data: String },
    /// Resize a session's grid geometry.
    Resize { id: String, cols: u16, rows: u16 },
    /// Tear a session down.
    Close { id: String },
}

impl ClientVerb {
    /// The JSON request body to publish for this verb. The closed enum can't
    /// realistically fail to serialise; an empty fallback keeps this total
    /// without an `unwrap`.
    fn body(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

// ── the published state record (mirror of the worker's `PtyState`) ───────────

/// One record on the `state/pty/<id>` append log — a local mirror of the worker's
/// `PtyState`. Serde ignores any wire field this surface doesn't project.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PtyRecord {
    /// The session id.
    pub id: String,
    /// The peer the shell runs on.
    #[serde(default)]
    pub peer: String,
    /// The phase tag (`opening` / `open` / `closed` / `unreachable`).
    pub phase: String,
    /// Per-session monotonic sequence number.
    #[serde(default)]
    pub seq: u64,
    /// A base64 output chunk, when this record carries shell bytes.
    #[serde(default)]
    pub data: Option<String>,
    /// The remote shell's exit code, on the terminal record.
    #[serde(default)]
    pub exit: Option<i32>,
    /// A human reason on a degrade path (unreachable / transport drop).
    #[serde(default)]
    pub reason: Option<String>,
}

/// Parse a `state/pty/<id>` record body; `None` on malformed JSON (an honest
/// miss, never a panic).
#[must_use]
pub fn parse_record(raw: &str) -> Option<PtyRecord> {
    serde_json::from_str(raw).ok()
}

/// The lifecycle phase the worker publishes, parsed from a record's `phase` tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemotePhase {
    /// An open attempt is in flight.
    Opening,
    /// The remote shell is live.
    Open,
    /// The remote shell ended or was torn down (terminal).
    Closed,
    /// The peer is offline / the key isn't provisioned (terminal dead-end).
    Unreachable,
    /// An unrecognised tag — treated as still-opening (never a panic).
    Unknown,
}

impl RemotePhase {
    fn from_tag(tag: &str) -> Self {
        match tag {
            "opening" => Self::Opening,
            "open" => Self::Open,
            "closed" => Self::Closed,
            "unreachable" => Self::Unreachable,
            _ => Self::Unknown,
        }
    }
}

// ── the client-visible status + honest chip mapping ──────────────────────────

/// The tone of a status note, mapped to a `Style` token by the widget layer
/// (§4 — the mapping lives at the render boundary, keeping this module egui-free
/// and headless-testable).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusTone {
    /// A neutral / accent note.
    Neutral,
    /// A cautionary note (connecting / reconnecting).
    Warn,
    /// A failure note (unreachable / lost).
    Danger,
    /// A spent / dimmed note (clean session end).
    Dim,
}

/// The client's view of one remote session — folded from the published phases.
///
/// Drives the pane's honest chip, its cursor/repaint liveness, and whether the
/// pane reaps (a clean end) or lingers surfacing a failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteStatus {
    /// The open request is out; awaiting the first `open` phase.
    Connecting,
    /// The remote shell is live; bytes stream both ways.
    Open,
    /// A transient transport drop is being re-established with a fresh session
    /// (`attempt` of [`RECONNECT_BUDGET`]).
    Reconnecting {
        /// The 1-based reconnect attempt number.
        attempt: u32,
    },
    /// The remote shell exited cleanly — the pane reaps like a local one.
    Closed {
        /// The shell's exit code, when the worker reported one.
        exit: Option<i32>,
    },
    /// An honest dead-end that never became (or stopped being) a usable session:
    /// an offline peer, an unprovisioned key, or an exhausted transport drop.
    /// The pane lingers so the reason stays on screen (§7).
    Failed {
        /// The operator-readable reason.
        reason: String,
    },
}

impl RemoteStatus {
    /// Whether the session is still moving — drives the pane's cursor + the
    /// repaint/poll heartbeat.
    #[must_use]
    pub const fn is_live(&self) -> bool {
        matches!(
            self,
            Self::Connecting | Self::Open | Self::Reconnecting { .. }
        )
    }

    /// Whether the pane should reap (close) — only a **clean** end, so a local
    /// and a remote shell that both `exit` close their pane identically. A
    /// [`Self::Failed`] pane deliberately lingers.
    #[must_use]
    pub const fn is_reap(&self) -> bool {
        matches!(self, Self::Closed { .. })
    }

    /// Whether keystrokes are accepted (a live-ish session). A terminal session
    /// refuses input honestly.
    #[must_use]
    pub const fn accepts_input(&self) -> bool {
        self.is_live()
    }

    /// The honest chip text + tone to paint over the pane, or `None` when the
    /// session is plainly live (no chrome beyond the node marker).
    #[must_use]
    pub fn note(&self) -> Option<(String, StatusTone)> {
        match self {
            Self::Connecting => Some(("connecting\u{2026}".to_string(), StatusTone::Warn)),
            Self::Open => None,
            Self::Reconnecting { attempt } => Some((
                format!("reconnecting\u{2026} ({attempt}/{RECONNECT_BUDGET})"),
                StatusTone::Warn,
            )),
            Self::Closed { exit } => Some((
                exit.map_or_else(
                    || "session ended".to_string(),
                    |code| format!("session ended (exit {code})"),
                ),
                StatusTone::Dim,
            )),
            Self::Failed { reason } => Some((reason.clone(), StatusTone::Danger)),
        }
    }
}

// ── the injectable Bus seam ──────────────────────────────────────────────────

/// The Bus seam the remote session drives: publish a typed verb to a peer's
/// request topic, and read a session's state-log records since a cursor.
///
/// Injectable so the fold is unit-tested headless (a fake that records verbs +
/// replays a log) while production talks the live Bus ([`BusPtyClient`]).
pub trait PtyBus: Send + Sync {
    /// Publish a request `body` on `action/pty/<peer>`.
    ///
    /// # Errors
    /// An operator-readable string when the append can't be written (e.g. no Bus
    /// dir); it never blocks on a peer.
    fn publish(&self, peer: &str, body: &str) -> Result<(), String>;

    /// Read `state/pty/<id>` records appended after `cursor` (a ULID), in order,
    /// as `(ulid, body)` pairs. A non-blocking local spool scan — never a peer
    /// probe; an empty result on any transient error (an honest miss).
    fn read_since(&self, id: &str, cursor: Option<&str>) -> Vec<(String, String)>;
}

/// The live Bus-backed client — a synchronous local `Persist` read/write, the
/// same persist-first path `mde-files-egui`'s mesh-mount client uses.
///
/// Holds only the resolved spool dir; a fresh `Persist` is opened per call (it
/// isn't `Send`).
/// Degrades honestly to empty / an error when there's no Bus dir — never a panic,
/// never a hang.
#[derive(Debug, Clone)]
pub struct BusPtyClient {
    /// The resolved Bus client spool dir, or `None` when this node has no Bus.
    bus_root: Option<PathBuf>,
}

impl BusPtyClient {
    /// Resolve the Bus spool dir from the environment (the production path).
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            bus_root: mde_bus::client_data_dir(),
        }
    }

    /// Construct with an explicit spool root (tests point this at a tempdir).
    #[must_use]
    pub const fn with_root(bus_root: Option<PathBuf>) -> Self {
        Self { bus_root }
    }
}

impl PtyBus for BusPtyClient {
    fn publish(&self, peer: &str, body: &str) -> Result<(), String> {
        let Some(root) = self.bus_root.as_ref() else {
            return Err(
                "No mesh Bus directory — remote terminals are unavailable on this node."
                    .to_string(),
            );
        };
        let topic = action_topic(peer);
        mde_bus::persist::Persist::open(root.clone())
            .and_then(|p| {
                p.write(
                    &topic,
                    mde_bus::hooks::config::Priority::Default,
                    None,
                    Some(body),
                )
            })
            .map(|_| ())
            .map_err(|e| format!("Couldn't publish the remote-terminal request: {e}"))
    }

    fn read_since(&self, id: &str, cursor: Option<&str>) -> Vec<(String, String)> {
        let Some(root) = self.bus_root.clone() else {
            return Vec::new();
        };
        let Ok(persist) = mde_bus::persist::Persist::open(root) else {
            return Vec::new();
        };
        persist
            .list_since(&state_topic(id), cursor)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|m| m.body.map(|b| (m.ulid, b)))
            .collect()
    }
}

// ── the remote session ───────────────────────────────────────────────────────

/// The outcome of folding a `closed` record — extracted pure so the
/// reconnect-vs-fail-vs-clean decision is unit-tested without a bus.
#[derive(Debug, Clone, PartialEq, Eq)]
enum CloseOutcome {
    /// A transient transport drop with budget left — re-establish a fresh session.
    Reconnect,
    /// A transient drop that's exhausted its budget / never opened — an honest,
    /// lingering failure carrying `reason`.
    Fail(String),
    /// A clean shell exit — reap the pane like a local one.
    Clean(Option<i32>),
}

/// Classify a `closed` state record. A transport drop (`exit == 255`) that the
/// session had actually reached [`RemoteStatus::Open`] and still has budget for
/// reconnects; otherwise an honest failure; anything else is a clean exit.
fn classify_close(
    exit: Option<i32>,
    reason: Option<&str>,
    ever_open: bool,
    reconnects_left: u32,
) -> CloseOutcome {
    let transient = exit == Some(SSH_TRANSPORT_EXIT);
    if transient && ever_open && reconnects_left > 0 {
        CloseOutcome::Reconnect
    } else if transient {
        CloseOutcome::Fail(reason.unwrap_or("connection lost").to_string())
    } else {
        CloseOutcome::Clean(exit)
    }
}

/// A live remote-shell session, driven over the [`PtyBus`] seam.
///
/// Mirrors [`crate::pty::LocalPty`]'s surface so the TERM-3 widget renders it
/// through the same VT engine + grid (§6). Single-threaded: the owning surface
/// polls it each frame on the UI thread (no pump threads — the Bus is the
/// transport).
pub struct RemotePty {
    /// The Bus seam (publish verbs / read the state log).
    bus: Arc<dyn PtyBus>,
    /// The mesh peer short-name — the `action/pty/<peer>` verb slot + node marker.
    peer: String,
    /// The display label for the node marker (the peer, or the manual host typed).
    label: String,
    /// The current client-minted session id (`state/pty/<id>` key). A reconnect
    /// mints a fresh one.
    id: String,
    /// The reused VT engine — base64 output chunks are fed straight into it.
    terminal: Terminal,
    /// The state-log read cursor (last ULID seen), reset on a reconnect.
    cursor: Option<String>,
    /// The folded client status.
    status: RemoteStatus,
    /// Current grid geometry (the last size published).
    cols: u16,
    /// Current grid geometry (the last size published).
    rows: u16,
    /// Whether the current session ever reached `open` — gates a transport-drop
    /// reconnect (a peer that never opens must not be retried forever).
    ever_open: bool,
    /// Remaining reconnect budget for transient drops.
    reconnects_left: u32,
    /// Poll throttle bookkeeping.
    last_poll: Option<Instant>,
    /// The poll cadence (tests use `Duration::ZERO`).
    poll_interval: Duration,
}

impl RemotePty {
    /// Open a remote shell on `peer`, labelled `label`, at an initial grid, and
    /// publish the `open` verb. A publish failure (no Bus) lands the session in an
    /// honest [`RemoteStatus::Failed`] immediately — never a faked shell (§7).
    #[must_use]
    pub fn open(bus: Arc<dyn PtyBus>, peer: &str, label: &str, cols: u16, rows: u16) -> Self {
        let cols = cols.max(1);
        let rows = rows.max(1);
        let id = mint_id(peer);
        let terminal = Terminal::new(usize::from(cols), usize::from(rows), DEFAULT_SCROLLBACK);
        let mut this = Self {
            bus,
            peer: peer.to_string(),
            label: label.to_string(),
            id,
            terminal,
            cursor: None,
            status: RemoteStatus::Connecting,
            cols,
            rows,
            ever_open: false,
            reconnects_left: RECONNECT_BUDGET,
            last_poll: None,
            poll_interval: DEFAULT_POLL_INTERVAL,
        };
        this.emit_open();
        this
    }

    /// Override the poll cadence (tests use `Duration::ZERO` to poll every call).
    #[must_use]
    pub const fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    /// Publish the `open` verb for the current id, folding a publish failure into
    /// an honest [`RemoteStatus::Failed`].
    fn emit_open(&mut self) {
        let verb = ClientVerb::Open {
            id: self.id.clone(),
            cols: self.cols,
            rows: self.rows,
        };
        if let Err(e) = self.bus.publish(&self.peer, &verb.body()) {
            self.status = RemoteStatus::Failed { reason: e };
        }
    }

    /// The current folded status.
    #[must_use]
    pub const fn status(&self) -> &RemoteStatus {
        &self.status
    }

    /// The node marker label (the peer / manual host).
    #[must_use]
    pub fn node_label(&self) -> &str {
        &self.label
    }

    /// The mesh peer short-name (the verb slot).
    #[must_use]
    pub fn peer(&self) -> &str {
        &self.peer
    }

    /// The current session id (`state/pty/<id>`); changes on a reconnect.
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.id
    }

    /// Whether the pane should reap — a clean end only (see [`RemoteStatus::is_reap`]).
    #[must_use]
    pub const fn is_output_closed(&self) -> bool {
        self.status.is_reap()
    }

    /// Run `f` against the current engine state (mirrors [`crate::pty::LocalPty::with_terminal`]).
    pub fn with_terminal<R>(&self, f: impl FnOnce(&Terminal) -> R) -> R {
        f(&self.terminal)
    }

    /// Publish keystrokes to the remote shell as a base64 `write` verb. A terminal
    /// session refuses input honestly.
    ///
    /// # Errors
    /// [`io::ErrorKind::BrokenPipe`] once the session is no longer live, or the
    /// Bus publish error otherwise.
    pub fn send_input(&self, bytes: &[u8]) -> io::Result<()> {
        if !self.status.accepts_input() {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        let verb = ClientVerb::Write {
            id: self.id.clone(),
            data: B64.encode(bytes),
        };
        self.bus
            .publish(&self.peer, &verb.body())
            .map_err(io::Error::other)
    }

    /// Reflow the engine grid and publish a `resize` verb on a geometry change.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        let cols = cols.max(1);
        let rows = rows.max(1);
        if (cols, rows) == (self.cols, self.rows) {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        self.terminal.resize(usize::from(cols), usize::from(rows));
        if self.status.accepts_input() {
            let verb = ClientVerb::Resize {
                id: self.id.clone(),
                cols,
                rows,
            };
            let _ = self.bus.publish(&self.peer, &verb.body());
        }
    }

    /// Publish a `close` verb (best-effort) so the remote shell is torn down.
    /// Idempotent — a stale id the worker no longer tracks is a harmless no-op.
    pub fn close(&self) {
        let verb = ClientVerb::Close {
            id: self.id.clone(),
        };
        let _ = self.bus.publish(&self.peer, &verb.body());
    }

    /// Drain net-new state records if the throttle has elapsed. Called each frame
    /// by the owning widget.
    pub fn poll(&mut self) {
        let now = Instant::now();
        if let Some(last) = self.last_poll {
            if now.duration_since(last) < self.poll_interval {
                return;
            }
        }
        self.last_poll = Some(now);
        self.poll_once();
    }

    /// Drain net-new state records unconditionally (the throttle-free core; tests
    /// drive this directly). Feeds output into the engine and folds each phase.
    pub fn poll_once(&mut self) {
        if !self.status.is_live() {
            return;
        }
        let records = self.bus.read_since(&self.id, self.cursor.as_deref());
        for (ulid, body) in records {
            self.cursor = Some(ulid);
            let Some(rec) = parse_record(&body) else {
                continue;
            };
            if !self.ingest(rec) {
                // A reconnect / terminal fold changed the session (or the id) —
                // the remaining records belong to the old session; stop.
                break;
            }
        }
    }

    /// Fold one state record: feed its output into the engine, then advance the
    /// status by its phase. Returns whether to keep reading this batch.
    fn ingest(&mut self, rec: PtyRecord) -> bool {
        // Output first, so the final bytes before a close are still shown.
        if let Some(data) = &rec.data {
            if let Ok(bytes) = B64.decode(data) {
                self.terminal.feed(&bytes);
            }
        }
        match RemotePhase::from_tag(&rec.phase) {
            RemotePhase::Open => {
                // `ever_open` gates a transport-drop reconnect (a peer that never
                // opens is not retried). The reconnect budget is deliberately NOT
                // reset here — it bounds the *total* reconnects over the pane's
                // life, so a peer that flaps open-then-drop can't loop forever.
                self.status = RemoteStatus::Open;
                self.ever_open = true;
                true
            }
            // Still in flight — keep the current live status.
            RemotePhase::Opening | RemotePhase::Unknown => true,
            RemotePhase::Unreachable => {
                self.status = RemoteStatus::Failed {
                    reason: rec.reason.unwrap_or_else(|| "peer unreachable".to_string()),
                };
                false
            }
            RemotePhase::Closed => {
                match classify_close(
                    rec.exit,
                    rec.reason.as_deref(),
                    self.ever_open,
                    self.reconnects_left,
                ) {
                    CloseOutcome::Reconnect => {
                        self.reconnect();
                        false
                    }
                    CloseOutcome::Fail(reason) => {
                        self.status = RemoteStatus::Failed { reason };
                        false
                    }
                    CloseOutcome::Clean(exit) => {
                        self.status = RemoteStatus::Closed { exit };
                        false
                    }
                }
            }
        }
    }

    /// Re-establish after a transient transport drop: spend one reconnect, mint a
    /// fresh session id, reset the read cursor, and re-open — keeping the existing
    /// engine so the user's screen + scrollback survive the blip.
    fn reconnect(&mut self) {
        self.reconnects_left = self.reconnects_left.saturating_sub(1);
        let attempt = RECONNECT_BUDGET - self.reconnects_left;
        self.id = mint_id(&self.peer);
        self.cursor = None;
        self.ever_open = false;
        self.status = RemoteStatus::Reconnecting { attempt };
        self.emit_open();
    }
}

impl Drop for RemotePty {
    fn drop(&mut self) {
        // Best-effort teardown so a closed tab reaps the remote shell (TERM-8:
        // `pty/close` on tab close).
        self.close();
    }
}

/// Wall-clock epoch millis — the entropy half of a minted session id.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Mint a topic-safe, unique client session id for `peer`. The worker treats the
/// id as an opaque `state/pty/<id>` key, so this only needs to be unique + free of
/// topic separators; a process-global counter guarantees uniqueness even within a
/// millisecond and across panes.
fn mint_id(peer: &str) -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let slug: String = peer
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    format!("term-{slug}-{}-{n}", now_ms())
}

#[cfg(test)]
pub(crate) mod test_support {
    //! An in-memory [`PtyBus`] for headless tests: it records every published verb
    //! and replays a seeded `state/pty/<id>` log, so a test asserts the exact
    //! verbs the surface emitted and drives the output→grid fold without a live Bus
    //! or worker. Shared by the `remote`, `splits`, and `tabs` test suites.

    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use super::PtyBus;

    /// A recorded request: the peer topic slot + the raw JSON body.
    #[derive(Clone, Debug)]
    pub struct Published {
        /// The `action/pty/<peer>` verb slot.
        pub peer: String,
        /// The raw request body.
        pub body: String,
    }

    /// The replayable state log per session id: `id → [(ulid, body)]`.
    type LogMap = HashMap<String, Vec<(String, String)>>;

    /// The fake bus. `Clone` shares the log + records (`Arc`), so a test keeps a
    /// probe handle after boxing a clone into a [`super::RemotePty`].
    #[derive(Clone, Default)]
    pub struct FakeBus {
        published: Arc<Mutex<Vec<Published>>>,
        /// id → replayable `(ulid, body)` records.
        logs: Arc<Mutex<LogMap>>,
        /// When set, `publish` returns this error (the honest no-Bus path).
        fail: Arc<Mutex<Option<String>>>,
    }

    impl FakeBus {
        /// A fresh empty fake.
        pub fn new() -> Self {
            Self::default()
        }

        /// Make every `publish` fail with `reason` (the no-Bus honesty test).
        pub fn failing(reason: &str) -> Self {
            let this = Self::default();
            *this.fail.lock().expect("fail lock") = Some(reason.to_string());
            this
        }

        /// Seed one state-log record for `id` (auto-numbered ULID-ish cursor).
        pub fn push_state(&self, id: &str, body: &str) {
            let mut logs = self.logs.lock().expect("logs lock");
            let entry = logs.entry(id.to_string()).or_default();
            let ulid = format!("{:026}", entry.len() + 1);
            entry.push((ulid, body.to_string()));
            drop(logs);
        }

        /// Every recorded request, in order.
        pub fn published(&self) -> Vec<Published> {
            self.published.lock().expect("published lock").clone()
        }

        /// The parsed records of every recorded request (for verb assertions).
        pub fn published_verbs(&self) -> Vec<serde_json::Value> {
            self.published()
                .into_iter()
                .filter_map(|p| serde_json::from_str(&p.body).ok())
                .collect()
        }

        /// The count of published requests whose `verb` field equals `verb`.
        pub fn verb_count(&self, verb: &str) -> usize {
            self.published_verbs()
                .into_iter()
                .filter(|v| v.get("verb").and_then(|v| v.as_str()) == Some(verb))
                .count()
        }
    }

    impl PtyBus for FakeBus {
        fn publish(&self, peer: &str, body: &str) -> Result<(), String> {
            // Clone the fail flag out first so its guard drops before the branch.
            let fail = self.fail.lock().expect("fail lock").clone();
            if let Some(reason) = fail {
                return Err(reason);
            }
            self.published
                .lock()
                .expect("published lock")
                .push(Published {
                    peer: peer.to_string(),
                    body: body.to_string(),
                });
            Ok(())
        }

        fn read_since(&self, id: &str, cursor: Option<&str>) -> Vec<(String, String)> {
            // Clone the session's records out under the lock, then filter with the
            // guard already released.
            let records = self.logs.lock().expect("logs lock").get(id).cloned();
            records
                .unwrap_or_default()
                .into_iter()
                .filter(|(ulid, _)| cursor.is_none_or(|c| ulid.as_str() > c))
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::test_support::FakeBus;
    use super::*;

    /// Build a JSON state-record body for the fake log.
    fn state_body(
        id: &str,
        phase: &str,
        data: Option<&str>,
        exit: Option<i32>,
        reason: Option<&str>,
    ) -> String {
        serde_json::json!({
            "id": id, "peer": "oak", "phase": phase, "seq": 1,
            "data": data, "exit": exit, "reason": reason, "since_ms": 0,
        })
        .to_string()
    }

    fn open_on(bus: Arc<dyn PtyBus>) -> RemotePty {
        RemotePty::open(bus, "oak", "oak", 80, 24).with_poll_interval(Duration::ZERO)
    }

    fn full_text(remote: &RemotePty) -> String {
        remote.with_terminal(|t| {
            let full = t.full();
            (0..full.rows())
                .map(|row| full.line_text(row))
                .collect::<Vec<_>>()
                .join("\n")
        })
    }

    // ── the wire contract MUST match the worker ──────────────────────────────

    #[test]
    fn prefixes_and_topics_match_the_worker_contract() {
        // Cross-check: these MUST equal mackesd::workers::pty_broker::{ACTION,STATE}_PREFIX.
        assert_eq!(ACTION_PREFIX, "action/pty/");
        assert_eq!(STATE_PREFIX, "state/pty/");
        assert_eq!(action_topic("oak"), "action/pty/oak");
        assert_eq!(state_topic("abc"), "state/pty/abc");
    }

    #[test]
    fn verbs_serialise_to_the_worker_wire_shape() {
        // The worker's parse_verb decodes `{"verb":…}` (serde tag="verb"); the
        // open verb carries the client-minted id + grid, never a shell string (§9).
        let open = ClientVerb::Open {
            id: "s1".into(),
            cols: 80,
            rows: 24,
        };
        assert_eq!(
            open.body(),
            r#"{"verb":"open","id":"s1","cols":80,"rows":24}"#
        );
        let write = ClientVerb::Write {
            id: "s1".into(),
            data: "aGk=".into(),
        };
        assert_eq!(write.body(), r#"{"verb":"write","id":"s1","data":"aGk="}"#);
        let resize = ClientVerb::Resize {
            id: "s1".into(),
            cols: 120,
            rows: 40,
        };
        assert_eq!(
            resize.body(),
            r#"{"verb":"resize","id":"s1","cols":120,"rows":40}"#
        );
        let close = ClientVerb::Close { id: "s1".into() };
        assert_eq!(close.body(), r#"{"verb":"close","id":"s1"}"#);
    }

    #[test]
    fn records_parse_and_ignore_unprojected_fields() {
        let rec = parse_record(
            r#"{"id":"s1","peer":"oak","phase":"open","seq":3,"data":"aGk=","since_ms":9}"#,
        )
        .expect("decodes the worker record");
        assert_eq!(rec.id, "s1");
        assert_eq!(rec.phase, "open");
        assert_eq!(rec.data.as_deref(), Some("aGk="));
        assert!(parse_record("not json").is_none());
    }

    // ── the open verb ─────────────────────────────────────────────────────────

    #[test]
    fn open_publishes_the_open_verb_and_connects() {
        let bus = FakeBus::new();
        let remote = open_on(Arc::new(bus.clone()));
        assert_eq!(*remote.status(), RemoteStatus::Connecting);
        assert_eq!(bus.verb_count("open"), 1);
        let v = &bus.published_verbs()[0];
        assert_eq!(v["verb"], "open");
        assert_eq!(v["id"], remote.session_id());
        assert_eq!(v["cols"], 80);
        // §9: no shell/command string on the open request.
        assert!(v.get("cmd").is_none() && v.get("command").is_none());
        // The request went to the peer's topic slot.
        assert_eq!(bus.published()[0].peer, "oak");
    }

    #[test]
    fn open_without_a_bus_fails_honestly_never_a_fake_shell() {
        let bus = FakeBus::failing("No mesh Bus directory");
        let remote = open_on(Arc::new(bus));
        assert!(
            matches!(remote.status(), RemoteStatus::Failed { reason } if reason.contains("No mesh Bus")),
            "expected an honest Failed, got {:?}",
            remote.status()
        );
        // A failed session refuses input (never a faked write path).
        assert!(remote.send_input(b"x").is_err());
    }

    // ── stream: base64 output → the reused engine/grid ───────────────────────

    #[test]
    fn poll_decodes_base64_output_into_the_grid() {
        let bus = FakeBus::new();
        let mut remote = open_on(Arc::new(bus.clone()));
        let id = remote.session_id().to_string();
        // The broker opens the session, then streams a base64 output chunk.
        bus.push_state(&id, &state_body(&id, "open", None, None, None));
        bus.push_state(
            &id,
            &state_body(&id, "open", Some(&B64.encode("hello-mesh")), None, None),
        );
        remote.poll_once();
        assert_eq!(*remote.status(), RemoteStatus::Open);
        assert!(
            full_text(&remote).contains("hello-mesh"),
            "the decoded bytes reached the engine grid"
        );
    }

    // ── write / resize verbs ──────────────────────────────────────────────────

    #[test]
    fn keystrokes_publish_base64_write_verbs() {
        let bus = FakeBus::new();
        let mut remote = open_on(Arc::new(bus.clone()));
        let id = remote.session_id().to_string();
        bus.push_state(&id, &state_body(&id, "open", None, None, None));
        remote.poll_once();
        remote
            .send_input(b"ls\n")
            .expect("live session accepts input");
        let write = bus
            .published_verbs()
            .into_iter()
            .find(|v| v["verb"] == "write")
            .expect("a write verb was published");
        assert_eq!(write["id"], id);
        assert_eq!(write["data"], B64.encode("ls\n"));
    }

    #[test]
    fn resize_publishes_on_change_and_reflows_the_engine() {
        let bus = FakeBus::new();
        let mut remote = open_on(Arc::new(bus.clone()));
        let id = remote.session_id().to_string();
        bus.push_state(&id, &state_body(&id, "open", None, None, None));
        remote.poll_once();
        remote.resize(120, 40);
        let resize = bus
            .published_verbs()
            .into_iter()
            .find(|v| v["verb"] == "resize")
            .expect("a resize verb was published");
        assert_eq!(resize["cols"].as_u64(), Some(120));
        assert_eq!(resize["rows"].as_u64(), Some(40));
        remote.with_terminal(|t| assert_eq!((t.cols(), t.rows()), (120, 40)));
        // An identical resize does not re-publish.
        let before = bus.verb_count("resize");
        remote.resize(120, 40);
        assert_eq!(bus.verb_count("resize"), before);
    }

    // ── close on drop ─────────────────────────────────────────────────────────

    #[test]
    fn dropping_publishes_a_close_verb() {
        let bus = FakeBus::new();
        let remote = open_on(Arc::new(bus.clone()));
        let id = remote.session_id().to_string();
        drop(remote);
        let close = bus
            .published_verbs()
            .into_iter()
            .find(|v| v["verb"] == "close")
            .expect("close verb on drop");
        assert_eq!(close["id"], id);
    }

    // ── honest terminal states ────────────────────────────────────────────────

    #[test]
    fn an_unreachable_phase_is_an_honest_failure_that_lingers() {
        let bus = FakeBus::new();
        let mut remote = open_on(Arc::new(bus.clone()));
        let id = remote.session_id().to_string();
        bus.push_state(
            &id,
            &state_body(
                &id,
                "unreachable",
                None,
                None,
                Some("unreachable: peer offline"),
            ),
        );
        remote.poll_once();
        assert!(
            matches!(remote.status(), RemoteStatus::Failed { reason } if reason.contains("offline")),
            "expected Failed, got {:?}",
            remote.status()
        );
        // A failure lingers (not reaped) so the reason stays on screen (§7).
        assert!(!remote.is_output_closed());
        assert!(!remote.status().is_live());
    }

    #[test]
    fn a_clean_exit_closes_the_pane_like_a_local_one() {
        let bus = FakeBus::new();
        let mut remote = open_on(Arc::new(bus.clone()));
        let id = remote.session_id().to_string();
        bus.push_state(&id, &state_body(&id, "open", None, None, None));
        bus.push_state(&id, &state_body(&id, "closed", None, Some(0), None));
        remote.poll_once();
        assert_eq!(*remote.status(), RemoteStatus::Closed { exit: Some(0) });
        assert!(remote.is_output_closed(), "a clean end reaps the pane");
    }

    // ── reconnect on a transient transport drop ──────────────────────────────

    #[test]
    fn classify_close_folds_transient_vs_clean() {
        // A transport drop after the session opened, with budget → reconnect.
        assert_eq!(
            classify_close(Some(255), Some("transport"), true, 3),
            CloseOutcome::Reconnect
        );
        // Exhausted budget → an honest failure.
        assert!(matches!(
            classify_close(Some(255), Some("lost"), true, 0),
            CloseOutcome::Fail(_)
        ));
        // Never opened → don't retry a peer that won't come up.
        assert!(matches!(
            classify_close(Some(255), None, false, 3),
            CloseOutcome::Fail(_)
        ));
        // A clean exit → close.
        assert_eq!(
            classify_close(Some(0), None, true, 3),
            CloseOutcome::Clean(Some(0))
        );
    }

    #[test]
    fn transient_drop_reconnects_with_a_fresh_session() {
        let bus = FakeBus::new();
        let mut remote = open_on(Arc::new(bus.clone()));
        let id1 = remote.session_id().to_string();
        // The session opens, then the transport drops (ssh exit 255).
        bus.push_state(&id1, &state_body(&id1, "open", None, None, None));
        bus.push_state(
            &id1,
            &state_body(&id1, "closed", None, Some(255), Some("ssh transport error")),
        );
        remote.poll_once();
        // A fresh session id was minted + a new open verb published.
        let id2 = remote.session_id().to_string();
        assert_ne!(id1, id2, "reconnect minted a fresh session id");
        assert_eq!(bus.verb_count("open"), 2, "a second open was published");
        assert!(matches!(
            remote.status(),
            RemoteStatus::Reconnecting { attempt: 1 }
        ));

        // The fresh session opens and streams — the reused engine keeps working.
        bus.push_state(&id2, &state_body(&id2, "open", None, None, None));
        bus.push_state(
            &id2,
            &state_body(&id2, "open", Some(&B64.encode("back-again")), None, None),
        );
        remote.poll_once();
        assert_eq!(*remote.status(), RemoteStatus::Open);
        assert!(full_text(&remote).contains("back-again"));
    }

    #[test]
    fn reconnect_budget_is_bounded_then_fails_honestly() {
        let bus = FakeBus::new();
        let mut remote = open_on(Arc::new(bus.clone()));
        // Drop the transport BUDGET+1 times; each drop after an open reconnects
        // until the budget is spent, then it lands in an honest Failed.
        for _ in 0..=RECONNECT_BUDGET {
            let id = remote.session_id().to_string();
            bus.push_state(&id, &state_body(&id, "open", None, None, None));
            bus.push_state(
                &id,
                &state_body(&id, "closed", None, Some(255), Some("dropped")),
            );
            remote.poll_once();
        }
        assert!(
            matches!(remote.status(), RemoteStatus::Failed { .. }),
            "an exhausted reconnect budget fails honestly, got {:?}",
            remote.status()
        );
        // BUDGET reconnects means BUDGET+1 total opens (the initial + each retry).
        assert_eq!(bus.verb_count("open"), (RECONNECT_BUDGET + 1) as usize);
    }

    // ── the minted id is topic-safe ──────────────────────────────────────────

    #[test]
    fn minted_ids_are_unique_and_topic_safe() {
        let a = mint_id("oak.mesh host");
        let b = mint_id("oak.mesh host");
        assert_ne!(a, b, "ids are unique even within a millisecond");
        assert!(!a.contains('/') && !a.contains(' ') && !a.contains('.'));
    }
}
