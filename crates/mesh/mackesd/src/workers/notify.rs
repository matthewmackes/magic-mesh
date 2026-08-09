//! CHAT-FIX-2 — the local-notification producer worker (design:
//! `docs/design/console-frontdoor.md` Q34/46/47).
//!
//! The empty-Chat bug had two halves: the `chat` worker not running (CHAT-FIX-1,
//! the census), and — the real one — **nothing producing local system events** so
//! that, absent peer chatter, the operator's Chat surface stayed blank. This
//! worker is that producer: it watches the local node's own event sources and
//! emits typed notifications the operator's Chat surface renders as a timestamped
//! feed (+ the tray unread badge).
//!
//! **How it reaches the Chat feed (glue §6, no surface rewrite).** It publishes
//! each notification as an alert-shaped JSON body on an `event/notify/<source>`
//! Bus lane. The existing [`super::chat`] worker already folds *every* alert lane
//! ([`super::chat::ALERT_LANE_PREFIXES`], which now includes `event/notify/`) via
//! [`mde_chat::fold_alert`] into a [`mde_chat::Message`] from the originating host
//! — so a notification with `host = <self>` lands in this node's `alert:<self>`
//! conversation (rendered in the self-contact's ICQ timeline) **and** the matching
//! per-severity system room, and a Warning+ one bumps the tray badge / raises a
//! chyron. No emitter-side changes, no new render path: the notification is just
//! one more alert on a lane the Chat plumbing already understands.
//!
//! **Event sources** (each bounded — a slow cadence + edge-triggering, never a
//! per-tick firehose):
//!   * **mesh peer join/leave** — diff the replicated peer directory
//!     ([`mackes_mesh_types::peers`]) the mesh mirror already writes. First sight
//!     seeds the baseline silently (no "everyone joined" flood on boot).
//!   * **updates available** — `dnf check-update` on a slow (~hourly) cadence;
//!     edge-triggered (fires once when updates appear, silent until they clear).
//!   * **external application lifecycle notifications** — folds the Cloud lane
//!     into its shell status segment without duplicating the Chat event.
//!
//! Platform service, storage, SMART, journal, and grade state deliberately do
//! **not** emit from this worker. [`super::node_grade`] owns those observations,
//! conditions, grades, acknowledgements, remediation, and critical notification
//! edges through the typed System and Mesh Health authority. Keeping raw probes
//! here would recreate a second health ledger and was the source of false
//! "journal warnings" toasts alongside a healthy modal.
//!
//! **Honest degrade (§7).** Every external source runs through an injectable
//! [`Probe`]; a probe that returns `None` (the binary is absent — no `dnf`, no
//! `smartctl` — or it failed to spawn) is skipped honestly, never faked. A node
//! degrades to exactly the sources it can read.
//!
//! **Bounded (§7).** A [`NotifyLog`] ring caps the recently-emitted notifications
//! (200) and time-windows identical ones (5 min) so a flapping condition coalesces
//! and the worker's own memory can't grow without limit; the downstream chat
//! conversation ring is itself capped ([`mde_chat::conversation`] `DEFAULT_CAPACITY`).
//!
//! **Testability (§7).** The whole worker drives headless: the Bus is an injected
//! [`Persist`] (tempdir), the peer directory a tempdir, and every command a
//! [`MapProbe`] of fixture outputs — so each source → a notification is asserted
//! with no live net. The pure parsers ([`diff_peers`] and
//! [`parse_dnf_check_update`]) are unit-tested directly, and
//! an end-to-end test folds the emitted lane through a real [`super::chat`] worker
//! to prove the notifications reach `alert:<self>`.

#![cfg(feature = "async-services")]

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;
use std::time::Duration;

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use mde_chat::Severity;
use serde::{Deserialize, Serialize};

use super::{ShutdownToken, Worker};

/// The Bus topic prefix every local notification rides.
///
/// The [`super::chat`] worker folds this prefix (it is in
/// [`super::chat::ALERT_LANE_PREFIXES`]) into the `alert:<host>` conversation the
/// Chat surface renders.
pub const NOTIFY_TOPIC_PREFIX: &str = "event/notify/";
/// Prefix for daemon-owned status rollups consumed by the shell pips.
pub const NOTIFY_SEGMENT_TOPIC_PREFIX: &str = "state/notify/segment/";
/// test-obs-10 — topic prefix for the NAMED per-worker circuit-breaker-trip
/// alert. A trip publishes on `fleet/health/breaker/<worker>`, mirroring the
/// SELinux/MON-4 `fleet/sec/selinux/<host>` lane convention
/// (`fleet/<domain>/<kind>/<key>`) so the affected worker is named IN the topic —
/// not lost in the anonymous "N journal warnings" blob a trip used to coalesce
/// into. See [`breaker_trip_alert`].
pub const BREAKER_ALERT_TOPIC_PREFIX: &str = "fleet/health/breaker/";
/// Criticals on the affected local seat fire the edge cue.
pub const CRITICAL_POLICY_OWN_SEAT: &str = "own-seat-light-show";
/// Remote criticals stay pull-first: pip + Chat.
pub const CRITICAL_POLICY_REMOTE: &str = "remote-pip-chat";

/// Base poll cadence. Peer checks run every tick; package checks run on the slow
/// [`UPDATES_EVERY`] multiple so the worker never hammers.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(30);

/// `dnf check-update` cadence — every 120th tick (~1 h at the default). Updates
/// change slowly and the probe is the heaviest, so it runs rarely.
const UPDATES_EVERY: u64 = 120;

/// Bound on the recently-emitted-notification ring ([`NotifyLog`]).
const NOTIFY_HISTORY_CAP: usize = 200;
/// Window within which an identical (source, summary) notification is coalesced
/// (dropped) rather than re-emitted — 5 minutes.
const COALESCE_WINDOW: Duration = Duration::from_secs(300);

/// Bounds for retrying a Bus that is not present or openable yet. The worker is
/// long-lived, so a startup ordering race must not require a service restart.
const MIN_BUS_RETRY_INTERVAL: Duration = Duration::from_millis(10);
const MAX_BUS_RETRY_INTERVAL: Duration = Duration::from_secs(2);

// ── the event sources ───────────────────────────────────────────────────────

/// The event source a notification came from — the lane suffix + the `source`
/// field the Chat card shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifySource {
    /// A mesh peer joined or left the replicated directory.
    Peer,
    /// Package / platform updates are available.
    Updates,
    /// Cloud notifications emitted by the cloud worker.
    Cloud,
}

impl NotifySource {
    /// The stable lane suffix / `source` token.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Peer => "peer",
            Self::Updates => "updates",
            Self::Cloud => "cloud",
        }
    }

    /// The full `event/notify/<source>` Bus topic.
    #[must_use]
    pub fn topic(self) -> String {
        format!("{NOTIFY_TOPIC_PREFIX}{}", self.key())
    }

    /// The status segment this source contributes to.
    #[must_use]
    pub const fn segment(self) -> NotifySegment {
        match self {
            Self::Peer => NotifySegment::Mesh,
            Self::Updates => NotifySegment::Power,
            Self::Cloud => NotifySegment::Alerts,
        }
    }
}

/// Lifecycle status segments retained outside the typed health authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum NotifySegment {
    /// Mesh peer/connectivity health.
    Mesh,
    /// Power/update posture.
    Power,
    /// General alert firehose.
    Alerts,
}

impl NotifySegment {
    /// Stable wire key.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Mesh => "mesh",
            Self::Power => "power",
            Self::Alerts => "alerts",
        }
    }

    /// Bus topic for this segment's latest rollup.
    #[must_use]
    pub fn topic(self) -> String {
        format!("{NOTIFY_SEGMENT_TOPIC_PREFIX}{}", self.key())
    }
}

/// One typed local notification: a severity, its source, and a short human line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    /// The color/mute axis (reuses the chat severity so `fold_alert` classifies it).
    pub severity: Severity,
    /// Where it came from.
    pub source: NotifySource,
    /// The one-line human message the Chat card shows.
    pub summary: String,
    /// The affected host, when different from the worker's own host.
    pub host: Option<String>,
}

impl Notification {
    fn new(severity: Severity, source: NotifySource, summary: impl Into<String>) -> Self {
        Self {
            severity,
            source,
            summary: summary.into(),
            host: None,
        }
    }

    fn for_host(
        severity: Severity,
        source: NotifySource,
        host: impl Into<String>,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            severity,
            source,
            summary: summary.into(),
            host: Some(host.into()),
        }
    }

    fn host<'a>(&'a self, self_host: &'a str) -> &'a str {
        self.host.as_deref().unwrap_or(self_host)
    }

    /// The coalescing fingerprint: same source + same text ⇒ same notification.
    fn fingerprint(&self) -> String {
        format!(
            "{}:{}:{}",
            self.source.key(),
            self.host.as_deref().unwrap_or(""),
            self.summary
        )
    }
}

const fn severity_rank(severity: Severity) -> u8 {
    match severity {
        Severity::Info => 1,
        Severity::Warning => 2,
        Severity::Critical => 3,
    }
}

fn severity_from_tag(tag: &str) -> Option<Severity> {
    match tag.trim().to_ascii_lowercase().as_str() {
        "critical" | "crit" | "error" | "fatal" | "urgent" => Some(Severity::Critical),
        "warning" | "warn" | "high" => Some(Severity::Warning),
        "info" | "notice" | "debug" => Some(Severity::Info),
        _ => None,
    }
}

/// The on-Bus body a notification serializes to — an alert-shaped JSON object the
/// chat [`mde_chat::fold_alert`] understands (`severity` drives the color, `host`
/// routes it to `alert:<host>`, the rest becomes the card's fields).
#[derive(Debug, Serialize)]
struct NotifyBody<'a> {
    severity: &'a str,
    source: &'a str,
    summary: &'a str,
    host: &'a str,
    ts_unix_ms: i64,
}

/// test-obs-10 — a fully-addressed Bus message the pure alert seam returns:
/// the topic plus the serialized JSON body. Returning this (rather than writing
/// straight into a live [`Persist`]) is what makes the breaker-trip alert
/// testable without a Bus — the caller does the single `persist.write`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlertMsg {
    /// The Bus topic to publish on (e.g. `fleet/health/breaker/mesh_router`).
    pub topic: String,
    /// The serialized alert-shaped JSON body.
    pub body: String,
}

/// test-obs-10 — the on-Bus body a circuit-breaker trip serializes to. Shaped
/// like [`NotifyBody`] (a `severity`/`summary` an alert-folding consumer can
/// classify) plus the machine-readable `worker`/`reason`/`breaker_open` fields
/// that name exactly WHICH worker died, WHY, and that this is the breaker-OPEN
/// state (distinct from a transient restart).
#[derive(Debug, Serialize)]
struct BreakerAlertBody<'a> {
    severity: &'a str,
    source: &'a str,
    worker: &'a str,
    summary: &'a str,
    reason: &'a str,
    /// The "permanent (breaker-open) failure" fact — `true` at the trip.
    breaker_open: bool,
}

/// test-obs-10 — build the NAMED circuit-breaker-trip alert for `worker`.
///
/// A trip used to surface only as a journal `error!` line folded into an
/// anonymous warning blob — an operator could not tell WHICH worker died or that it was a
/// breaker-open trip (vs a transient restart). This emits a distinct, named
/// alert on `fleet/health/breaker/<worker>` ([`BREAKER_ALERT_TOPIC_PREFIX`])
/// carrying the worker name, the trip `reason`, and the breaker-open fact.
///
/// Pure: no clock, no Bus — the caller publishes the returned [`AlertMsg`]. A
/// timestamp is intentionally omitted so the message is deterministic under test.
#[must_use]
pub fn breaker_trip_alert(worker: &str, reason: &str) -> AlertMsg {
    let topic = format!("{BREAKER_ALERT_TOPIC_PREFIX}{worker}");
    let summary = format!(
        "worker '{worker}' died — circuit breaker OPEN (permanent until recovery): {reason}"
    );
    let body = BreakerAlertBody {
        severity: Severity::Critical.tag(),
        source: "breaker",
        worker,
        summary: &summary,
        reason,
        breaker_open: true,
    };
    // A struct of owned primitives never fails to serialize; fall back to an
    // empty body rather than panic in the (unreachable) error arm.
    let body = serde_json::to_string(&body).unwrap_or_default();
    AlertMsg { topic, body }
}

#[derive(Debug, Deserialize)]
struct ExternalNotifyBody {
    severity: String,
    summary: String,
    host: String,
}

#[derive(Debug, Clone, Serialize)]
struct SegmentRollupBody<'a> {
    segment: &'a str,
    severity: &'a str,
    source: &'a str,
    summary: &'a str,
    host: &'a str,
    critical_policy: &'a str,
    ts_unix_ms: i64,
}

// ── the command probe seam (honest degrade + testability) ───────────────────

/// Captured output of an external probe command.
#[derive(Debug, Clone)]
pub struct ProbeOut {
    /// The process exit code (`-1` if it was terminated by a signal).
    pub code: i32,
    /// Captured stdout, lossy-decoded.
    pub stdout: String,
}

/// An injectable runner for the external commands the sources poll.
///
/// Production is [`SystemProbe`] (`std::process::Command`); tests inject a
/// [`MapProbe`] of fixtures. A `None` return means the program is absent or failed
/// to spawn — the source is then skipped honestly (§7), never faked.
pub trait Probe: Send {
    /// Run `program args…`, returning captured stdout + exit code, or `None` when
    /// the program can't be run at all.
    fn run(&self, program: &str, args: &[&str]) -> Option<ProbeOut>;
}

/// Production probe: spawns the real command, captures stdout, and treats a
/// spawn failure (binary absent) as `None`.
pub struct SystemProbe;

impl Probe for SystemProbe {
    fn run(&self, program: &str, args: &[&str]) -> Option<ProbeOut> {
        let out = std::process::Command::new(program)
            .args(args)
            .output()
            .ok()?;
        Some(ProbeOut {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        })
    }
}

// ── pure parsers (each unit-tested against fixture output) ───────────────────

/// Diff two peer-directory snapshots into join/leave notifications.
///
/// A newly-seen host is an Info-level "joined"; a vanished host is a Warning-level
/// "left" (worth a badge bump). Order-stable (both inputs are sorted sets).
#[must_use]
pub fn diff_peers(prev: &BTreeSet<String>, now: &BTreeSet<String>) -> Vec<Notification> {
    let mut out = Vec::new();
    for joined in now.difference(prev) {
        out.push(Notification::new(
            Severity::Info,
            NotifySource::Peer,
            format!("peer {joined} joined the mesh"),
        ));
    }
    for left in prev.difference(now) {
        out.push(Notification::new(
            Severity::Warning,
            NotifySource::Peer,
            format!("peer {left} left the mesh"),
        ));
    }
    out
}

/// Parse `dnf check-update` into a count of available package updates.
///
/// dnf exits `100` when updates are available (`0` = none, other = error); the
/// body lists `name.arch  version  repo` rows after a blank-line-separated header.
/// A count of `0` means "nothing to report".
#[must_use]
pub fn parse_dnf_check_update(out: &ProbeOut) -> usize {
    if out.code != 100 {
        return 0; // 0 = up to date; anything else = an error we don't fabricate
    }
    out.stdout
        .lines()
        .filter(|l| {
            let cols: Vec<&str> = l.split_whitespace().collect();
            // A real update row: three columns and a `name.arch` first token.
            cols.len() >= 3 && cols[0].contains('.') && !l.starts_with(' ')
        })
        .count()
}

// ── the bounded, coalescing emit log (§7 "bounded") ─────────────────────────

/// A bounded ring of recently-emitted notifications. It caps memory at
/// [`NOTIFY_HISTORY_CAP`] and drops an identical (source, summary) notification
/// seen within [`COALESCE_WINDOW`] — the rate-limit that keeps a flapping source
/// from spamming the feed while still letting the same event through once the
/// window elapses.
#[derive(Default)]
struct NotifyLog {
    recent: VecDeque<(String, i64)>,
}

impl NotifyLog {
    /// Admit `n` at `now_ms`: `true` (emit) unless an identical fingerprint is
    /// still inside the coalesce window. Always keeps the ring ≤ the cap.
    fn admit(&mut self, n: &Notification, now_ms: i64) -> bool {
        let fp = n.fingerprint();
        let window_ms = i64::try_from(COALESCE_WINDOW.as_millis()).unwrap_or(i64::MAX);
        let suppressed = self
            .recent
            .iter()
            .any(|(f, ts)| f == &fp && now_ms.saturating_sub(*ts) < window_ms);
        if suppressed {
            return false;
        }
        self.recent.push_back((fp, now_ms));
        while self.recent.len() > NOTIFY_HISTORY_CAP {
            self.recent.pop_front();
        }
        true
    }
}

// ── the worker ──────────────────────────────────────────────────────────────

/// Per-run source state, carried across ticks so each source edge-triggers.
#[derive(Default)]
struct SourceState {
    /// The peer set as of the last poll (`None` before the first — seeds silently).
    known_peers: Option<BTreeSet<String>>,
    /// Whether updates were pending as of the last check (edge-trigger 0→N).
    updates_pending: bool,
    /// Cursors for external `event/notify/*` lanes owned by other workers.
    external_cursors: BTreeMap<String, Option<String>>,
    /// The bounded, coalescing emit log.
    log: NotifyLog,
    /// Current worst notification driving each status segment.
    rollups: BTreeMap<NotifySegment, Notification>,
}

/// The mackesd `notify` worker (CHAT-FIX-2). Runs on every node (rank 0).
pub struct NotifyWorker {
    self_host: String,
    workgroup_root: PathBuf,
    poll_interval: Duration,
    bus_root_override: Option<PathBuf>,
    probe: Box<dyn Probe>,
}

impl NotifyWorker {
    /// Construct with production defaults: the real [`SystemProbe`], the default
    /// Bus root, and the 30 s cadence.
    #[must_use]
    pub fn new(workgroup_root: PathBuf, self_host: String) -> Self {
        Self {
            self_host,
            workgroup_root,
            poll_interval: DEFAULT_POLL_INTERVAL,
            bus_root_override: None,
            probe: Box::new(SystemProbe),
        }
    }

    /// Override the Bus root (tests point it at a tempdir Persist).
    #[must_use]
    pub fn with_bus_root(mut self, p: PathBuf) -> Self {
        self.bus_root_override = Some(p);
        self
    }

    /// Override the poll cadence (tests use a short value).
    #[must_use]
    pub const fn with_poll_interval(mut self, d: Duration) -> Self {
        self.poll_interval = d;
        self
    }

    /// Inject a probe (tests supply fixture output; production is [`SystemProbe`]).
    #[must_use]
    pub fn with_probe(mut self, probe: Box<dyn Probe>) -> Self {
        self.probe = probe;
        self
    }

    /// One poll pass — the headless-testable core. `tick` selects which slow
    /// sources are due; `now_ms` stamps + windows the emissions.
    fn tick_once(&self, persist: &Persist, state: &mut SourceState, tick: u64, now_ms: i64) {
        let mut pending: Vec<Notification> = Vec::new();

        // Platform health is exclusively emitted by the typed node-grade health
        // authority. This legacy worker retains only non-health lifecycle lanes.
        pending.extend(self.check_peers(state));
        if tick % UPDATES_EVERY == 0 {
            pending.extend(self.check_updates(state));
        }

        for n in pending {
            if state.log.admit(&n, now_ms) {
                self.emit(persist, &n, now_ms);
                self.update_segment_rollup(persist, state, &n, now_ms);
            }
        }
        self.fold_external_notify_lane(persist, state, NotifySource::Cloud, now_ms);
    }

    /// Diff the replicated peer directory (the same source the chat/mesh mirror
    /// reads). First sight seeds the baseline silently.
    fn check_peers(&self, state: &mut SourceState) -> Vec<Notification> {
        let dir = mackes_mesh_types::peers::peers_dir(&self.workgroup_root);
        let now: BTreeSet<String> = mackes_mesh_types::peers::read_peers(&dir)
            .into_iter()
            .map(|r| r.hostname)
            .filter(|h| h != &self.self_host)
            .collect();
        // First sight (`None`) seeds the baseline silently — no "everyone joined"
        // flood on boot; thereafter diff the previous snapshot.
        state
            .known_peers
            .replace(now.clone())
            .map_or_else(Vec::new, |prev| diff_peers(&prev, &now))
    }

    /// `dnf check-update`: fire once when updates appear (0→N), stay silent until
    /// they clear.
    fn check_updates(&self, state: &mut SourceState) -> Vec<Notification> {
        let Some(out) = self.probe.run("dnf", &["check-update", "-q"]) else {
            return Vec::new(); // no dnf → skip honestly
        };
        let count = parse_dnf_check_update(&out);
        let was_pending = state.updates_pending;
        state.updates_pending = count > 0;
        if count > 0 && !was_pending {
            vec![Notification::new(
                Severity::Info,
                NotifySource::Updates,
                format!("{count} package update(s) available"),
            )]
        } else {
            Vec::new()
        }
    }

    fn fold_external_notify_lane(
        &self,
        persist: &Persist,
        state: &mut SourceState,
        source: NotifySource,
        now_ms: i64,
    ) {
        let topic = source.topic();
        let cursor = state.external_cursors.get(&topic).cloned().flatten();
        let msgs = match persist.list_since(&topic, cursor.as_deref()) {
            Ok(msgs) => msgs,
            Err(error) => {
                tracing::warn!(
                    target: "mackesd::notify",
                    %topic,
                    %error,
                    "external notification lane unreadable; retaining cursor and rollup"
                );
                return;
            }
        };
        if let Some(last) = msgs.last() {
            state
                .external_cursors
                .insert(topic.clone(), Some(last.ulid.clone()));
        }
        for msg in msgs {
            let Some(body) = msg.body.as_deref() else {
                continue;
            };
            let Ok(external) = serde_json::from_str::<ExternalNotifyBody>(body) else {
                continue;
            };
            let Some(severity) = severity_from_tag(&external.severity) else {
                continue;
            };
            let n = Notification::for_host(severity, source, external.host, external.summary);
            self.update_segment_rollup(persist, state, &n, now_ms);
        }
    }

    /// Serialize + publish one notification on its `event/notify/<source>` lane —
    /// the alert-shaped body the [`super::chat`] worker folds into `alert:<self>`.
    fn emit(&self, persist: &Persist, n: &Notification, now_ms: i64) {
        let body = NotifyBody {
            severity: n.severity.tag(),
            source: n.source.key(),
            summary: &n.summary,
            host: n.host(&self.self_host),
            ts_unix_ms: now_ms,
        };
        let Ok(json) = serde_json::to_string(&body) else {
            return;
        };
        let topic = n.source.topic();
        if let Err(e) = persist.write(&topic, Priority::Default, None, Some(&json)) {
            tracing::debug!(target: "mackesd::notify", %topic, error = %e, "notify publish failed");
        }
    }

    fn update_segment_rollup(
        &self,
        persist: &Persist,
        state: &mut SourceState,
        n: &Notification,
        now_ms: i64,
    ) {
        let segment = n.source.segment();
        let should_replace = state
            .rollups
            .get(&segment)
            .is_none_or(|current| severity_rank(n.severity) >= severity_rank(current.severity));
        if !should_replace {
            return;
        }
        state.rollups.insert(segment, n.clone());
        let affected_host = n.host(&self.self_host);
        let critical_policy = if n.severity == Severity::Critical && affected_host == self.self_host
        {
            CRITICAL_POLICY_OWN_SEAT
        } else {
            CRITICAL_POLICY_REMOTE
        };
        let body = SegmentRollupBody {
            segment: segment.key(),
            severity: n.severity.tag(),
            source: n.source.key(),
            summary: &n.summary,
            host: affected_host,
            critical_policy,
            ts_unix_ms: now_ms,
        };
        let Ok(json) = serde_json::to_string(&body) else {
            return;
        };
        let topic = segment.topic();
        if let Err(e) = persist.write(&topic, Priority::Default, None, Some(&json)) {
            tracing::debug!(target: "mackesd::notify", %topic, error = %e, "segment rollup publish failed");
        }
    }

    /// Prime each source lane once at startup with a benign, chat-skipped message.
    ///
    /// The chat worker seeds each topic's drain cursor to the *head* the first time
    /// it sees the topic and drops that first message (its forward-only, no-backlog
    /// contract). A lane that first appears when a *real* notification lands would
    /// therefore lose that first notification. Priming makes the prime absorb the
    /// first-sight skip, so every real notification thereafter is folded.
    fn prime_lanes(&self, persist: &Persist, now_ms: i64) {
        for source in [NotifySource::Peer, NotifySource::Updates] {
            let body = NotifyBody {
                severity: Severity::Info.tag(),
                source: source.key(),
                summary: "notify monitor online",
                host: &self.self_host,
                ts_unix_ms: now_ms,
            };
            if let Ok(json) = serde_json::to_string(&body) {
                let _ = persist.write(&source.topic(), Priority::Default, None, Some(&json));
            }
        }
    }
}

fn resolve_default_bus_root(
    env_root: Option<std::ffi::OsString>,
    data_dir: Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(root) = env_root.filter(|root| !root.is_empty()) {
        return Some(PathBuf::from(root));
    }
    Some(data_dir?.join("mde").join("bus"))
}

fn default_bus_root() -> Option<PathBuf> {
    resolve_default_bus_root(std::env::var_os("MDE_BUS_ROOT"), dirs::data_dir())
}

fn notify_bus_root(override_root: Option<PathBuf>) -> PathBuf {
    notify_bus_root_or_system(override_root.or_else(default_bus_root))
}

fn notify_bus_root_or_system(resolved: Option<PathBuf>) -> PathBuf {
    resolved.unwrap_or_else(|| PathBuf::from(mde_bus::SYSTEM_BUS_ROOT))
}

fn next_bus_retry_interval(current: Duration) -> Duration {
    current
        .saturating_mul(2)
        .clamp(MIN_BUS_RETRY_INTERVAL, MAX_BUS_RETRY_INTERVAL)
}

fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[async_trait::async_trait]
impl Worker for NotifyWorker {
    fn name(&self) -> &'static str {
        "notify"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let bus_root = notify_bus_root(self.bus_root_override.clone());
        let mut retry_interval = MIN_BUS_RETRY_INTERVAL;
        let persist = loop {
            match Persist::open(bus_root.clone()) {
                Ok(persist) => break persist,
                Err(error) => tracing::warn!(
                    target: "mackesd::notify",
                    %error,
                    "Persist open failed; notify startup will retry"
                ),
            }
            tokio::select! {
                () = shutdown.wait() => return Ok(()),
                () = tokio::time::sleep(retry_interval) => {}
            }
            retry_interval = next_bus_retry_interval(retry_interval);
        };
        let mut state = SourceState::default();
        self.prime_lanes(&persist, now_unix_ms());
        let mut tick_no: u64 = 0;
        let mut tick = tokio::time::interval(self.poll_interval);
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    self.tick_once(&persist, &mut state, tick_no, now_unix_ms());
                    tick_no = tick_no.wrapping_add(1);
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
    use std::collections::BTreeMap;
    use std::path::Path;

    // ── an injectable fixture probe ─────────────────────────────────────

    /// A [`Probe`] returning canned output keyed by the program name.
    #[derive(Default)]
    struct MapProbe {
        by_program: BTreeMap<String, ProbeOut>,
        /// Programs to report as absent (returns `None` — honest degrade).
        absent: BTreeSet<String>,
    }

    impl MapProbe {
        fn program(mut self, prog: &str, code: i32, stdout: &str) -> Self {
            self.by_program.insert(
                prog.to_string(),
                ProbeOut {
                    code,
                    stdout: stdout.to_string(),
                },
            );
            self
        }
        fn absent(mut self, prog: &str) -> Self {
            self.absent.insert(prog.to_string());
            self
        }
    }

    impl Probe for MapProbe {
        fn run(&self, program: &str, _args: &[&str]) -> Option<ProbeOut> {
            if self.absent.contains(program) {
                return None;
            }
            self.by_program.get(program).cloned()
        }
    }

    fn persist_at(dir: &Path) -> Persist {
        Persist::open(dir.join("bus")).expect("open persist")
    }

    fn worker_with(root: &Path, probe: MapProbe) -> NotifyWorker {
        NotifyWorker::new(root.to_path_buf(), "eagle".into())
            .with_bus_root(root.join("bus"))
            .with_probe(Box::new(probe))
    }

    #[test]
    fn default_bus_root_resolution_honors_mde_bus_root() {
        assert_eq!(
            resolve_default_bus_root(
                Some(std::ffi::OsString::from("/run/mde-bus")),
                Some(PathBuf::from("/root/.local/share")),
            ),
            Some(PathBuf::from("/run/mde-bus")),
        );
        assert_eq!(
            resolve_default_bus_root(None, Some(PathBuf::from("/root/.local/share"))),
            Some(PathBuf::from("/root/.local/share/mde/bus")),
        );
    }

    #[test]
    fn service_bus_root_falls_back_to_the_shared_system_spool() {
        assert_eq!(
            notify_bus_root_or_system(None),
            PathBuf::from(mde_bus::SYSTEM_BUS_ROOT)
        );
        assert_eq!(
            notify_bus_root_or_system(Some(PathBuf::from("/tmp/notify-explicit-bus"))),
            PathBuf::from("/tmp/notify-explicit-bus")
        );
    }

    #[tokio::test]
    async fn late_bus_recovers_in_the_same_worker_and_primes_forward_lanes() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let bus_root = root.join("late-bus");
        std::fs::write(&bus_root, b"not a directory").unwrap();

        let mut worker = NotifyWorker::new(root, "eagle".into())
            .with_bus_root(bus_root.clone())
            .with_poll_interval(Duration::from_millis(5))
            .with_probe(Box::new(MapProbe::default().absent("dnf")));
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let task =
            tokio::spawn(
                async move { worker.run(ShutdownToken::from_receiver(shutdown_rx)).await },
            );

        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(
            !task.is_finished(),
            "an unopenable startup Bus is retryable"
        );
        std::fs::remove_file(&bus_root).unwrap();

        let persist = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Ok(persist) = Persist::open(bus_root.clone()) {
                    let peer_ready = persist
                        .list_since(&NotifySource::Peer.topic(), None)
                        .is_ok_and(|messages| !messages.is_empty());
                    let updates_ready = persist
                        .list_since(&NotifySource::Updates.topic(), None)
                        .is_ok_and(|messages| !messages.is_empty());
                    if peer_ready && updates_ready {
                        break persist;
                    }
                }
                assert!(!task.is_finished(), "worker exited before Bus recovery");
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("same worker must open the late Bus and prime its forward lanes");
        assert_eq!(count_notify_msgs(&persist, NotifySource::Peer), 1);
        assert_eq!(count_notify_msgs(&persist, NotifySource::Updates), 1);

        shutdown_tx.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .expect("shutdown completes")
            .expect("worker joins")
            .expect("worker exits cleanly");
    }

    fn write_peer(root: &Path, host: &str) {
        let dir = mackes_mesh_types::peers::peers_dir(root);
        let rec = mackes_mesh_types::peers::PeerRecord::now(host, None, "ok");
        mackes_mesh_types::peers::write_peer_record(&dir, &rec).unwrap();
    }

    fn write_grade(root: &Path, host: &str, grade: &str, score: u8) {
        let dir = root.join("node-grade");
        std::fs::create_dir_all(&dir).unwrap();
        let body = serde_json::json!({
            "host": host,
            "grade": grade,
            "score": score,
            "factors": {},
            "trend": "steady",
            "published_at_ms": 1,
        })
        .to_string();
        std::fs::write(dir.join(format!("{host}.json")), body).unwrap();
    }

    // ── pure parsers ────────────────────────────────────────────────────

    #[test]
    fn peer_diff_detects_join_and_leave() {
        let prev: BTreeSet<String> = ["nyc3".into(), "fra1".into()].into_iter().collect();
        let now: BTreeSet<String> = ["nyc3".into(), "sfo3".into()].into_iter().collect();
        let out = diff_peers(&prev, &now);
        // sfo3 joined (Info), fra1 left (Warning).
        assert!(out.iter().any(|n| n.severity == Severity::Info
            && n.summary.contains("sfo3")
            && n.summary.contains("joined")));
        assert!(out.iter().any(|n| n.severity == Severity::Warning
            && n.summary.contains("fra1")
            && n.summary.contains("left")));
        assert_eq!(out.len(), 2);
        // No change ⇒ nothing.
        assert!(diff_peers(&now, &now).is_empty());
    }

    #[test]
    fn dnf_check_update_counts_only_on_exit_100() {
        let out = ProbeOut {
            code: 100,
            stdout: "\nkernel.x86_64 6.9.0 updates\nfirefox.x86_64 120.0 updates\n".into(),
        };
        assert_eq!(parse_dnf_check_update(&out), 2);
        // Exit 0 = up to date ⇒ never fabricate a count.
        let clean = ProbeOut {
            code: 0,
            stdout: String::new(),
        };
        assert_eq!(parse_dnf_check_update(&clean), 0);
    }

    // ── test-obs-10: the NAMED breaker-trip alert seam ──────────────────

    #[test]
    fn breaker_trip_alert_names_worker_topic_and_reason() {
        let reason = "8 failures within 120s (last error: link down)";
        let alert = breaker_trip_alert("mesh_router", reason);
        // Topic carries the worker name (fleet/<domain>/<kind>/<key>).
        assert_eq!(alert.topic, "fleet/health/breaker/mesh_router");
        assert_eq!(
            alert.topic,
            format!("{BREAKER_ALERT_TOPIC_PREFIX}mesh_router")
        );
        // Body names the worker, the reason, the breaker-open fact + severity.
        let body: serde_json::Value = serde_json::from_str(&alert.body).expect("valid alert JSON");
        assert_eq!(body["worker"], "mesh_router");
        assert_eq!(body["reason"], reason);
        assert_eq!(body["breaker_open"], serde_json::json!(true));
        assert_eq!(body["source"], "breaker");
        assert_eq!(body["severity"], Severity::Critical.tag());
        assert!(body["summary"].as_str().unwrap().contains("mesh_router"));
        assert!(body["summary"].as_str().unwrap().contains("link down"));
    }

    // ── bounded / coalescing log ────────────────────────────────────────

    #[test]
    fn log_coalesces_within_window_and_caps() {
        let mut log = NotifyLog::default();
        let n = Notification::new(Severity::Warning, NotifySource::Peer, "peer x left");
        assert!(log.admit(&n, 1_000), "first emit admitted");
        assert!(!log.admit(&n, 2_000), "duplicate within window suppressed");
        // Past the coalesce window the same event is admitted again.
        let past = 1_000 + i64::try_from(COALESCE_WINDOW.as_millis()).unwrap() + 1;
        assert!(log.admit(&n, past), "re-emit after the window");
        // The ring stays bounded under a flood of distinct notifications.
        for i in 0..(NOTIFY_HISTORY_CAP * 2) {
            let d = Notification::new(Severity::Info, NotifySource::Updates, format!("u{i}"));
            log.admit(&d, past + i64::try_from(i).unwrap());
        }
        assert!(
            log.recent.len() <= NOTIFY_HISTORY_CAP,
            "feed history capped"
        );
    }

    // ── worker ticks (headless: tempdir bus + peer dir + fixture probe) ──

    fn count_notify_msgs(persist: &Persist, source: NotifySource) -> usize {
        persist
            .list_since(&source.topic(), None)
            .map(|v| v.len())
            .unwrap_or(0)
    }

    fn count_topic_msgs(persist: &Persist, topic: &str) -> usize {
        persist
            .list_since(topic, None)
            .map(|v| v.len())
            .unwrap_or(0)
    }

    #[test]
    fn first_tick_seeds_peers_silently_then_emits_join() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let persist = persist_at(root);
        write_peer(root, "nyc3");
        let w = worker_with(root, MapProbe::default().absent("systemctl"));
        let mut st = SourceState::default();
        // First tick: baseline seed, no peer join emitted (prime lane only).
        w.tick_once(&persist, &mut st, 0, 10_000);
        // A new peer appears; next tick emits the join.
        write_peer(root, "fra1");
        w.tick_once(&persist, &mut st, 1, 20_000);
        let peers = persist
            .list_since(&NotifySource::Peer.topic(), None)
            .unwrap();
        assert!(
            peers
                .iter()
                .any(|m| m.body.as_deref().unwrap_or("").contains("fra1")),
            "the join for fra1 is on the peer lane"
        );
    }

    #[test]
    fn platform_health_probes_do_not_emit_duplicate_notification_lanes() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let persist = persist_at(root);
        write_peer(root, "nyc3"); // baseline peer

        let probe = MapProbe::default()
            .program(
                "systemctl",
                0,
                "sshd.service loaded failed failed OpenSSH\n",
            )
            .program(
                "df",
                0,
                "Filesystem 1k Used Avail Capacity Mount\n/dev/sda1 100 97 3 97% /\n",
            )
            .program("smartctl", 0, "/dev/sda -d scsi\n")
            .program(
                "dnf",
                100,
                "\nkernel.x86_64 6.9 updates\nfirefox.x86_64 120 updates\n",
            )
            .program("journalctl", 0, "oom killer invoked\ntask hung\n");

        let w = worker_with(root, probe);
        let mut st = SourceState::default();
        // Prime the lanes exactly as run() does (one skipped-by-chat msg per lane).
        w.prime_lanes(&persist, 50_000);
        // Seed peers and run the lifecycle sources.
        w.tick_once(&persist, &mut st, 0, 100_000);
        w.tick_once(&persist, &mut st, UPDATES_EVERY, 200_000);

        // Package updates remain an informational lifecycle notification.
        assert!(count_notify_msgs(&persist, NotifySource::Updates) >= 2);
        // Raw platform probes are owned by System and Mesh Health and must not
        // create a second set of alert/toast lanes, even with alarming fixtures.
        assert_eq!(count_topic_msgs(&persist, "event/notify/service"), 0);
        assert_eq!(count_topic_msgs(&persist, "event/notify/disk"), 0);
        assert_eq!(count_topic_msgs(&persist, "event/notify/journal"), 0);
        assert_eq!(count_topic_msgs(&persist, "event/notify/node-grade"), 0);
    }

    #[test]
    fn notifications_publish_worst_severity_segment_rollups() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let persist = persist_at(root);
        let w = worker_with(root, MapProbe::default());
        let mut st = SourceState::default();
        let critical = Notification::new(Severity::Critical, NotifySource::Cloud, "cloud down");
        w.update_segment_rollup(&persist, &mut st, &critical, 100_000);

        let alerts = persist
            .list_since(&NotifySegment::Alerts.topic(), None)
            .unwrap();
        let latest = alerts.last().and_then(|m| m.body.as_deref()).unwrap();
        assert!(latest.contains(r#""segment":"alerts""#));
        assert!(latest.contains(r#""source":"cloud""#));
        assert!(latest.contains(r#""severity":"critical""#));
        assert!(latest.contains(r#""critical_policy":"own-seat-light-show""#));

        let lower = Notification::new(Severity::Warning, NotifySource::Cloud, "later warning");
        w.update_segment_rollup(&persist, &mut st, &lower, 200_000);
        let alerts_after = persist
            .list_since(&NotifySegment::Alerts.topic(), None)
            .unwrap();
        assert_eq!(
            alerts_after.len(),
            alerts.len(),
            "a lower-severity source cannot overwrite the active critical rollup"
        );
    }

    #[test]
    fn legacy_node_grades_do_not_emit_duplicate_alerts() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let persist = persist_at(root);
        write_grade(root, "nyc3", "D", 62);
        let w = worker_with(root, MapProbe::default());
        let mut st = SourceState::default();

        w.tick_once(&persist, &mut st, 1, 100_000);
        assert_eq!(count_topic_msgs(&persist, "event/notify/node-grade"), 0);
        assert!(persist
            .list_since(&NotifySegment::Alerts.topic(), None)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn cloud_notify_lane_folds_into_alerts_segment_without_reemitting() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let persist = persist_at(root);
        let cloud_body = serde_json::json!({
            "severity": "critical",
            "source": "cloud",
            "summary": "nova-api went down",
            "host": "cloud-1",
            "service": "nova-api",
            "ts_unix_ms": 1000,
        })
        .to_string();
        persist
            .write(
                &NotifySource::Cloud.topic(),
                Priority::Default,
                None,
                Some(&cloud_body),
            )
            .unwrap();
        let w = worker_with(root, MapProbe::default());
        let mut st = SourceState::default();
        w.tick_once(&persist, &mut st, 1, 100_000);

        let cloud_lane = persist
            .list_since(&NotifySource::Cloud.topic(), None)
            .unwrap();
        assert_eq!(
            cloud_lane.len(),
            1,
            "notify folds the external cloud lane but does not duplicate its Chat event"
        );
        let rollups = persist
            .list_since(&NotifySegment::Alerts.topic(), None)
            .unwrap();
        let rollup = rollups.last().and_then(|m| m.body.as_deref()).unwrap();
        assert!(rollup.contains(r#""source":"cloud""#));
        assert!(rollup.contains(r#""host":"cloud-1""#));
        assert!(rollup.contains(r#""critical_policy":"remote-pip-chat""#));
    }

    #[test]
    fn absent_binaries_degrade_honestly() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let persist = persist_at(root);
        // Every external probe absent — only the (directory-based) peer source and
        // priming run; nothing is fabricated.
        let probe = MapProbe::default()
            .absent("systemctl")
            .absent("df")
            .absent("smartctl")
            .absent("dnf")
            .absent("journalctl");
        let w = worker_with(root, probe);
        let mut st = SourceState::default();
        w.tick_once(&persist, &mut st, 0, 100_000);
        w.tick_once(&persist, &mut st, UPDATES_EVERY, 200_000);
        // No REAL notifications beyond the single prime per lane (a prime is one msg).
        assert_eq!(count_topic_msgs(&persist, "event/notify/service"), 0);
        assert_eq!(count_topic_msgs(&persist, "event/notify/disk"), 0);
        assert_eq!(count_notify_msgs(&persist, NotifySource::Updates), 0);
        assert_eq!(count_topic_msgs(&persist, "event/notify/journal"), 0);
    }

    // ── end-to-end: the notifications reach the Chat feed ───────────────

    #[test]
    fn emitted_notification_folds_into_alert_self_exactly_as_chat_does() {
        use crate::workers::chat::{alert_message, is_alert_lane};
        use mde_chat::MessageKind;

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let persist = persist_at(root);

        // An informational package lifecycle notification lands on its lane.
        let probe = MapProbe::default()
            .program("dnf", 100, "\nfoo.x86_64 1 updates\nbar.noarch 2 updates\n")
            .absent("systemctl")
            .absent("smartctl")
            .absent("journalctl");
        let w = worker_with(root, probe);
        let mut st = SourceState::default();
        w.tick_once(&persist, &mut st, UPDATES_EVERY, 3_000);

        // Read the raw notification off its `event/notify/updates` lane.
        let topic = NotifySource::Updates.topic();
        assert!(
            is_alert_lane(&topic),
            "the chat worker's alert-lane filter must accept the notify lane"
        );
        let msgs = persist.list_since(&topic, None).unwrap();
        let raw = msgs
            .iter()
            .rev()
            .find(|m| m.body.as_deref().unwrap_or("").contains("2 package update"))
            .expect("the update notification is on the lane");

        // Fold it EXACTLY as `chat::drain_alerts` does — this is the real path a
        // notification takes into the `alert:<self>` conversation the Chat surface
        // renders (newest-first) + the tray badge.
        let folded = alert_message(
            &topic,
            &raw.ulid,
            raw.body.as_deref().unwrap(),
            raw.ts_unix_ms,
            "eagle",
        );
        assert_eq!(
            folded.sender, "eagle",
            "host=self ⇒ routes to the eagle alert:<self> feed"
        );
        let MessageKind::Alert {
            severity, fields, ..
        } = &folded.kind
        else {
            unreachable!("a folded notification is an Alert message");
        };
        assert_eq!(*severity, Severity::Info);
        assert_eq!(
            fields.get("summary").map(String::as_str),
            Some("2 package update(s) available")
        );
        assert_eq!(fields.get("source").map(String::as_str), Some("updates"));
    }
}
