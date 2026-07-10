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
//!   * **service failed/degraded** — `systemctl --failed`; emits a unit the first
//!     time it enters the failed set.
//!   * **disk-low / SMART** — `df` capacity threshold + `smartctl -H`; edge-
//!     triggered per mount/device so a full disk alerts once, not every poll.
//!   * **journal WARN-or-above** — `journalctl -p warning` since the last poll,
//!     **coalesced** to a single "N warnings / M errors" line per poll so a noisy
//!     journal can't spam the feed.
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
//! with no live net / live journal. The pure parsers ([`diff_peers`],
//! [`parse_failed_units`], [`parse_df_capacity`], [`smart_health_failed`],
//! [`parse_dnf_check_update`], [`coalesce_journal`]) are unit-tested directly, and
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
/// Criticals on the affected local seat fire the edge cue.
pub const CRITICAL_POLICY_OWN_SEAT: &str = "own-seat-light-show";
/// Remote criticals stay pull-first: pip + Chat.
pub const CRITICAL_POLICY_REMOTE: &str = "remote-pip-chat";

/// Base poll cadence. Peer + journal checks run every tick; the heavier probes
/// run on slow multiples ([`SERVICE_EVERY`] / [`DISK_EVERY`] / [`UPDATES_EVERY`])
/// so the worker never hammers.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(30);

/// `systemctl --failed` cadence — every 4th tick (~2 min at the default).
const SERVICE_EVERY: u64 = 4;
/// `df` + `smartctl` cadence — every 10th tick (~5 min at the default).
const DISK_EVERY: u64 = 10;
/// `dnf check-update` cadence — every 120th tick (~1 h at the default). Updates
/// change slowly and the probe is the heaviest, so it runs rarely.
const UPDATES_EVERY: u64 = 120;

/// Disk capacity (%) at/above which a filesystem raises a Warning.
const DISK_WARN_PCT: u8 = 90;
/// Disk capacity (%) at/above which a filesystem raises a Critical.
const DISK_CRIT_PCT: u8 = 95;

/// Bound on the recently-emitted-notification ring ([`NotifyLog`]).
const NOTIFY_HISTORY_CAP: usize = 200;
/// Window within which an identical (source, summary) notification is coalesced
/// (dropped) rather than re-emitted — 5 minutes.
const COALESCE_WINDOW: Duration = Duration::from_secs(300);

// ── the event sources ───────────────────────────────────────────────────────

/// The event source a notification came from — the lane suffix + the `source`
/// field the Chat card shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifySource {
    /// A mesh peer joined or left the replicated directory.
    Peer,
    /// Package / platform updates are available.
    Updates,
    /// A systemd unit is failed/degraded.
    Service,
    /// A filesystem is low on space, or a disk's SMART health failed.
    Disk,
    /// Journal entries at WARN-or-above (coalesced).
    Journal,
    /// OpenStack/cloud notifications emitted by the cloud worker.
    Cloud,
    /// A node capability grade entered D/F.
    NodeGrade,
}

impl NotifySource {
    /// The stable lane suffix / `source` token.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Peer => "peer",
            Self::Updates => "updates",
            Self::Service => "service",
            Self::Disk => "disk",
            Self::Journal => "journal",
            Self::Cloud => "cloud",
            Self::NodeGrade => "node-grade",
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
            Self::Service | Self::Disk => NotifySegment::Device,
            Self::Journal | Self::Cloud | Self::NodeGrade => NotifySegment::Alerts,
        }
    }
}

/// The four status segments the shell renders as pips.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum NotifySegment {
    /// Device/seat health.
    Device,
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
            Self::Device => "device",
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

fn node_grade_severity(grade: &str) -> Option<Severity> {
    match grade {
        "F" => Some(Severity::Critical),
        "D" => Some(Severity::Warning),
        _ => None,
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

#[derive(Debug, Deserialize)]
struct GradeRow {
    host: String,
    grade: String,
    score: u8,
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

/// Parse the unit names from `systemctl --failed --no-legend --plain` output. The
/// unit is the first whitespace-delimited column of each non-blank line.
#[must_use]
pub fn parse_failed_units(stdout: &str) -> BTreeSet<String> {
    stdout
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .filter(|u| u.contains('.')) // a unit name (foo.service), not a stray marker
        .map(str::to_string)
        .collect()
}

/// Parse `df -P` output into `(mount, capacity_pct)` rows, skipping pseudo
/// filesystems (their fullness is meaningless). The capacity column is the 5th
/// (`NN%`); the mount is the 6th (last).
#[must_use]
pub fn parse_df_capacity(stdout: &str) -> Vec<(String, u8)> {
    let mut out = Vec::new();
    for line in stdout.lines().skip(1) {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 6 {
            continue;
        }
        let fs = cols[0];
        let mount = cols[cols.len() - 1];
        if is_pseudo_fs(fs, mount) {
            continue;
        }
        let Ok(pct) = cols[4].trim_end_matches('%').parse::<u8>() else {
            continue;
        };
        out.push((mount.to_string(), pct));
    }
    out
}

/// Whether a `df` row is a pseudo/virtual filesystem whose capacity is noise.
fn is_pseudo_fs(fs: &str, mount: &str) -> bool {
    matches!(fs, "tmpfs" | "devtmpfs" | "efivarfs" | "overlay")
        || mount.starts_with("/proc")
        || mount.starts_with("/sys")
        || mount.starts_with("/dev")
        || mount.starts_with("/run")
        || mount == "/boot/efi"
}

/// The disk notification a `(mount, pct)` row raises, if it crossed a threshold.
fn disk_notification(mount: &str, pct: u8) -> Option<Notification> {
    if pct >= DISK_CRIT_PCT {
        Some(Notification::new(
            Severity::Critical,
            NotifySource::Disk,
            format!("filesystem {mount} is {pct}% full"),
        ))
    } else if pct >= DISK_WARN_PCT {
        Some(Notification::new(
            Severity::Warning,
            NotifySource::Disk,
            format!("filesystem {mount} is {pct}% full"),
        ))
    } else {
        None
    }
}

/// Parse `smartctl --scan` output into the device paths (the first token of each
/// line, e.g. `/dev/sda -d scsi # …`). Bounded by the caller.
#[must_use]
pub fn parse_smart_scan(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .filter(|t| t.starts_with("/dev/"))
        .map(str::to_string)
        .collect()
}

/// Whether a `smartctl -H <dev>` report is a health FAILURE (the SMART overall
/// self-assessment line reads anything other than PASSED/OK).
#[must_use]
pub fn smart_health_failed(stdout: &str) -> bool {
    stdout.lines().any(|l| {
        let l = l.to_ascii_lowercase();
        (l.contains("overall-health") || l.contains("smart health status"))
            && !l.contains("passed")
            && !l.contains("ok")
    })
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

/// Coalesce a `journalctl -p warning` batch into at most one notification.
///
/// One `-o cat` line per entry; the whole batch becomes a single "N journal
/// warnings" (Warning) so a noisy journal can't spam the feed. Empty → none. The
/// error split looks for an `error`/`fail`/`critical` token so the summary can
/// mention them, but it stays one coalesced line.
#[must_use]
pub fn coalesce_journal(stdout: &str) -> Option<Notification> {
    let lines: Vec<&str> = stdout.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return None;
    }
    let total = lines.len();
    let errors = lines
        .iter()
        .filter(|l| {
            let low = l.to_ascii_lowercase();
            low.contains("error") || low.contains("fail") || low.contains("critical")
        })
        .count();
    let summary = if errors > 0 {
        format!("{total} new journal warnings ({errors} error-level)")
    } else {
        format!("{total} new journal warnings")
    };
    Some(Notification::new(
        Severity::Warning,
        NotifySource::Journal,
        summary,
    ))
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
    /// Units already reported failed (edge-trigger on entry into the set).
    known_failed: BTreeSet<String>,
    /// Mounts already alerted over threshold (cleared when they drop back).
    alerted_mounts: BTreeSet<String>,
    /// Devices whose SMART failure was already alerted.
    alerted_smart: BTreeSet<String>,
    /// Whether updates were pending as of the last check (edge-trigger 0→N).
    updates_pending: bool,
    /// The journal `--since` cursor (unix seconds) — `None` before the first poll
    /// (which seeds it to now, so old warnings aren't replayed).
    journal_since: Option<i64>,
    /// Last alarm grade seen per host; C-or-better clears a host.
    known_node_grade_alarms: BTreeMap<String, String>,
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

        // Peers + journal every tick (both cheap + edge-triggered).
        pending.extend(self.check_peers(state));
        pending.extend(self.check_journal(state, now_ms));
        pending.extend(self.check_node_grades(state));

        if tick % SERVICE_EVERY == 0 {
            pending.extend(self.check_services(state));
        }
        if tick % DISK_EVERY == 0 {
            pending.extend(self.check_disk(state));
            pending.extend(self.check_smart(state));
        }
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

    /// `systemctl --failed`: emit a unit the first time it enters the failed set.
    fn check_services(&self, state: &mut SourceState) -> Vec<Notification> {
        let Some(out) = self
            .probe
            .run("systemctl", &["--failed", "--no-legend", "--plain"])
        else {
            return Vec::new(); // no systemctl → skip honestly
        };
        let failed = parse_failed_units(&out.stdout);
        let mut fresh = Vec::new();
        for unit in failed.difference(&state.known_failed) {
            fresh.push(Notification::new(
                Severity::Warning,
                NotifySource::Service,
                format!("service {unit} failed"),
            ));
        }
        state.known_failed = failed;
        fresh
    }

    /// `df -P`: alert a filesystem the first time it crosses a threshold; clear it
    /// when it drops back (so re-crossing re-alerts).
    fn check_disk(&self, state: &mut SourceState) -> Vec<Notification> {
        let Some(out) = self.probe.run("df", &["-P"]) else {
            return Vec::new();
        };
        let mut fresh = Vec::new();
        for (mount, pct) in parse_df_capacity(&out.stdout) {
            match disk_notification(&mount, pct) {
                Some(n) => {
                    if state.alerted_mounts.insert(mount) {
                        fresh.push(n);
                    }
                }
                None => {
                    state.alerted_mounts.remove(&mount);
                }
            }
        }
        fresh
    }

    /// `smartctl --scan` → per-device `smartctl -H`: alert a disk whose SMART
    /// health first fails. Bounded to the first 8 devices.
    fn check_smart(&self, state: &mut SourceState) -> Vec<Notification> {
        let Some(scan) = self.probe.run("smartctl", &["--scan"]) else {
            return Vec::new(); // no smartctl → skip honestly
        };
        let mut fresh = Vec::new();
        for dev in parse_smart_scan(&scan.stdout).into_iter().take(8) {
            let Some(health) = self.probe.run("smartctl", &["-H", &dev]) else {
                continue;
            };
            if smart_health_failed(&health.stdout) {
                if state.alerted_smart.insert(dev.clone()) {
                    fresh.push(Notification::new(
                        Severity::Critical,
                        NotifySource::Disk,
                        format!("disk {dev} SMART health check FAILED"),
                    ));
                }
            } else {
                state.alerted_smart.remove(&dev);
            }
        }
        fresh
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

    /// `journalctl -p warning` since the last poll, coalesced to one line. First
    /// poll seeds the cursor to now (no backlog replay of old warnings).
    fn check_journal(&self, state: &mut SourceState, now_ms: i64) -> Vec<Notification> {
        let now_secs = now_ms / 1000;
        let Some(since) = state.journal_since else {
            state.journal_since = Some(now_secs);
            return Vec::new(); // seed the cursor, don't replay history
        };
        let since_arg = format!("@{since}");
        let out = self.probe.run(
            "journalctl",
            &["-p", "warning", "-o", "cat", "--no-pager", "-S", &since_arg],
        );
        // Advance the cursor regardless (a probe failure just means "nothing new
        // this window" — we don't re-scan the same span forever).
        state.journal_since = Some(now_secs);
        match out {
            Some(o) => coalesce_journal(&o.stdout).into_iter().collect(),
            None => Vec::new(), // no journalctl → skip honestly
        }
    }

    /// Scan the existing node-grade mirror and alert on transitions into D/F.
    fn check_node_grades(&self, state: &mut SourceState) -> Vec<Notification> {
        let dir = self.workgroup_root.join("node-grade");
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut fresh = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let Ok(body) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(row) = serde_json::from_str::<GradeRow>(&body) else {
                continue;
            };
            let grade = row.grade.trim().to_ascii_uppercase();
            let Some(severity) = node_grade_severity(&grade) else {
                state.known_node_grade_alarms.remove(&row.host);
                continue;
            };
            let changed = state.known_node_grade_alarms.get(&row.host) != Some(&grade);
            if changed {
                state
                    .known_node_grade_alarms
                    .insert(row.host.clone(), grade.clone());
                fresh.push(Notification::for_host(
                    severity,
                    NotifySource::NodeGrade,
                    row.host.clone(),
                    format!(
                        "node {} dropped to grade {grade} (score {})",
                        row.host, row.score
                    ),
                ));
            }
        }
        fresh
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
        let msgs = persist
            .list_since(&topic, cursor.as_deref())
            .unwrap_or_default();
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
        for source in [
            NotifySource::Peer,
            NotifySource::Updates,
            NotifySource::Service,
            NotifySource::Disk,
            NotifySource::Journal,
            NotifySource::NodeGrade,
        ] {
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

fn default_bus_root() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("mde").join("bus"))
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
        let Some(bus_root) = self.bus_root_override.clone().or_else(default_bus_root) else {
            tracing::debug!(target: "mackesd::notify", "no bus root; worker idle");
            return Ok(());
        };
        let persist = match Persist::open(bus_root) {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!(target: "mackesd::notify", error = %e, "persist open failed; worker idle");
                return Ok(());
            }
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

    /// A [`Probe`] returning canned output keyed by the program name (args are
    /// ignored — each source calls exactly one program, except smartctl, handled
    /// via a per-arg map).
    #[derive(Default)]
    struct MapProbe {
        by_program: BTreeMap<String, ProbeOut>,
        /// `smartctl -H <dev>` outputs keyed by device.
        smart_health: BTreeMap<String, ProbeOut>,
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
        fn smart(mut self, dev: &str, stdout: &str) -> Self {
            self.smart_health.insert(
                dev.to_string(),
                ProbeOut {
                    code: 0,
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
        fn run(&self, program: &str, args: &[&str]) -> Option<ProbeOut> {
            if self.absent.contains(program) {
                return None;
            }
            if program == "smartctl" && args.first() == Some(&"-H") {
                return args.get(1).and_then(|d| self.smart_health.get(*d)).cloned();
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
    fn failed_units_parse_from_systemctl_plain() {
        let stdout = "sshd.service loaded failed failed OpenSSH server\n\
                      nginx.service loaded failed failed nginx\n";
        let failed = parse_failed_units(stdout);
        assert!(failed.contains("sshd.service"));
        assert!(failed.contains("nginx.service"));
        assert_eq!(failed.len(), 2);
        assert!(parse_failed_units("").is_empty());
    }

    #[test]
    fn df_capacity_parses_and_skips_pseudo() {
        let stdout = "Filesystem 1024-blocks Used Available Capacity Mounted on\n\
                      /dev/sda1 100 95 5 95% /\n\
                      tmpfs 100 1 99 1% /run\n\
                      /dev/sdb1 100 50 50 50% /data\n";
        let rows = parse_df_capacity(stdout);
        // tmpfs on /run is skipped; real mounts kept.
        assert!(rows.contains(&("/".to_string(), 95)));
        assert!(rows.contains(&("/data".to_string(), 50)));
        assert!(!rows.iter().any(|(m, _)| m == "/run"));
    }

    #[test]
    fn disk_threshold_maps_severity() {
        assert_eq!(disk_notification("/", 89), None);
        assert_eq!(
            disk_notification("/", 90).unwrap().severity,
            Severity::Warning
        );
        assert_eq!(
            disk_notification("/", 96).unwrap().severity,
            Severity::Critical
        );
    }

    #[test]
    fn smart_health_parse() {
        assert!(!smart_health_failed(
            "SMART overall-health self-assessment test result: PASSED"
        ));
        assert!(smart_health_failed(
            "SMART overall-health self-assessment test result: FAILED!"
        ));
        assert_eq!(
            parse_smart_scan("/dev/sda -d scsi # comment\n/dev/nvme0 -d nvme\n").len(),
            2
        );
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

    #[test]
    fn journal_coalesces_to_one_line() {
        let stdout = "a warning happened\nanother warning\nsomething failed hard\n";
        let n = coalesce_journal(stdout).expect("some warnings");
        assert_eq!(n.severity, Severity::Warning);
        assert!(n.summary.contains('3')); // 3 total
        assert!(n.summary.contains("error-level")); // one "failed" line
        assert!(coalesce_journal("").is_none());
        assert!(coalesce_journal("   \n\n").is_none());
    }

    // ── bounded / coalescing log ────────────────────────────────────────

    #[test]
    fn log_coalesces_within_window_and_caps() {
        let mut log = NotifyLog::default();
        let n = Notification::new(Severity::Warning, NotifySource::Service, "service x failed");
        assert!(log.admit(&n, 1_000), "first emit admitted");
        assert!(!log.admit(&n, 2_000), "duplicate within window suppressed");
        // Past the coalesce window the same event is admitted again.
        let past = 1_000 + i64::try_from(COALESCE_WINDOW.as_millis()).unwrap() + 1;
        assert!(log.admit(&n, past), "re-emit after the window");
        // The ring stays bounded under a flood of distinct notifications.
        for i in 0..(NOTIFY_HISTORY_CAP * 2) {
            let d = Notification::new(Severity::Info, NotifySource::Journal, format!("j{i}"));
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
    fn each_source_emits_a_notification_from_fixtures() {
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
            .smart(
                "/dev/sda",
                "SMART overall-health self-assessment test result: FAILED\n",
            )
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
        // Seed peers + journal cursor on tick 0 (also runs all the due sources).
        w.tick_once(&persist, &mut st, 0, 100_000);
        // A later tick where every slow source is due (tick chosen as a common
        // multiple of the cadences) so journal has a real since-window too.
        let common = SERVICE_EVERY * DISK_EVERY * UPDATES_EVERY; // divisible by all
        w.tick_once(&persist, &mut st, common, 200_000);

        // Service (sshd failed): reported the first time it's seen (tick 0).
        assert!(count_notify_msgs(&persist, NotifySource::Service) >= 2); // prime + real
                                                                          // Disk 97% → Critical; SMART FAILED → Critical (both on the disk lane).
        assert!(count_notify_msgs(&persist, NotifySource::Disk) >= 2);
        // Updates available.
        assert!(count_notify_msgs(&persist, NotifySource::Updates) >= 2);
        // Journal warnings coalesced (emitted on the second tick, since-window).
        assert!(count_notify_msgs(&persist, NotifySource::Journal) >= 2);

        // Content spot-checks on the disk lane.
        let disk = persist
            .list_since(&NotifySource::Disk.topic(), None)
            .unwrap();
        let bodies: String = disk.iter().filter_map(|m| m.body.clone()).collect();
        assert!(bodies.contains("97% full") || bodies.contains("SMART"));
        assert!(bodies.contains("\"severity\":\"critical\""));
    }

    #[test]
    fn notifications_publish_worst_severity_segment_rollups() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let persist = persist_at(root);
        let probe = MapProbe::default()
            .program(
                "systemctl",
                0,
                "sshd.service loaded failed failed OpenSSH\n",
            )
            .program(
                "df",
                0,
                "Filesystem 1k Used Avail Capacity Mount\n/dev/sda1 100 99 1 99% /\n",
            )
            .absent("smartctl")
            .absent("dnf")
            .absent("journalctl");
        let w = worker_with(root, probe);
        let mut st = SourceState::default();
        w.tick_once(&persist, &mut st, SERVICE_EVERY * DISK_EVERY, 100_000);

        let device = persist
            .list_since(&NotifySegment::Device.topic(), None)
            .unwrap();
        let latest = device.last().and_then(|m| m.body.as_deref()).unwrap();
        assert!(latest.contains(r#""segment":"device""#));
        assert!(latest.contains(r#""source":"disk""#));
        assert!(latest.contains(r#""severity":"critical""#));
        assert!(latest.contains(r#""critical_policy":"own-seat-light-show""#));

        let lower = Notification::new(Severity::Warning, NotifySource::Service, "later warning");
        w.update_segment_rollup(&persist, &mut st, &lower, 200_000);
        let device_after = persist
            .list_since(&NotifySegment::Device.topic(), None)
            .unwrap();
        assert_eq!(
            device_after.len(),
            device.len(),
            "a lower-severity source cannot overwrite the active critical rollup"
        );
    }

    #[test]
    fn node_grade_d_f_is_folded_into_alerts_segment() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let persist = persist_at(root);
        write_grade(root, "nyc3", "D", 62);
        let w = worker_with(root, MapProbe::default());
        let mut st = SourceState::default();

        w.tick_once(&persist, &mut st, 1, 100_000);
        let lane = persist
            .list_since(&NotifySource::NodeGrade.topic(), None)
            .unwrap();
        let body = lane.last().and_then(|m| m.body.as_deref()).unwrap();
        assert!(body.contains(r#""severity":"warning""#));
        assert!(body.contains("grade D"));
        let rollups = persist
            .list_since(&NotifySegment::Alerts.topic(), None)
            .unwrap();
        let rollup = rollups.last().and_then(|m| m.body.as_deref()).unwrap();
        assert!(rollup.contains(r#""segment":"alerts""#));
        assert!(rollup.contains(r#""source":"node-grade""#));

        w.tick_once(&persist, &mut st, 2, 130_000);
        assert_eq!(
            persist
                .list_since(&NotifySource::NodeGrade.topic(), None)
                .unwrap()
                .len(),
            lane.len(),
            "same D grade is edge-triggered, not spammed"
        );

        write_grade(root, "nyc3", "C", 74);
        w.tick_once(&persist, &mut st, 3, 160_000);
        write_grade(root, "nyc3", "F", 40);
        w.tick_once(&persist, &mut st, 4, 190_000);
        let lane = persist
            .list_since(&NotifySource::NodeGrade.topic(), None)
            .unwrap();
        let body = lane.last().and_then(|m| m.body.as_deref()).unwrap();
        assert!(body.contains(r#""severity":"critical""#));
        assert!(body.contains("grade F"));
        let rollups = persist
            .list_since(&NotifySegment::Alerts.topic(), None)
            .unwrap();
        let rollup = rollups.last().and_then(|m| m.body.as_deref()).unwrap();
        assert!(rollup.contains(r#""host":"nyc3""#));
        assert!(rollup.contains(r#""critical_policy":"remote-pip-chat""#));
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
        let common = SERVICE_EVERY * DISK_EVERY * UPDATES_EVERY;
        w.tick_once(&persist, &mut st, 0, 100_000);
        w.tick_once(&persist, &mut st, common, 200_000);
        // No REAL notifications beyond the single prime per lane (a prime is one msg).
        assert_eq!(count_notify_msgs(&persist, NotifySource::Service), 0);
        assert_eq!(count_notify_msgs(&persist, NotifySource::Disk), 0);
        assert_eq!(count_notify_msgs(&persist, NotifySource::Updates), 0);
        assert_eq!(count_notify_msgs(&persist, NotifySource::Journal), 0);
    }

    // ── end-to-end: the notifications reach the Chat feed ───────────────

    #[test]
    fn emitted_notification_folds_into_alert_self_exactly_as_chat_does() {
        use crate::workers::chat::{alert_message, is_alert_lane};
        use mde_chat::MessageKind;

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let persist = persist_at(root);

        // A disk-critical notification lands on the notify lane.
        let probe = MapProbe::default()
            .program(
                "df",
                0,
                "Filesystem 1k Used Avail Capacity Mount\n/dev/sda1 100 99 1 99% /\n",
            )
            .absent("systemctl")
            .absent("smartctl")
            .absent("dnf")
            .absent("journalctl");
        let w = worker_with(root, probe);
        let mut st = SourceState::default();
        w.tick_once(&persist, &mut st, DISK_EVERY, 3_000);

        // Read the raw notification off its `event/notify/disk` lane.
        let topic = NotifySource::Disk.topic();
        assert!(
            is_alert_lane(&topic),
            "the chat worker's alert-lane filter must accept the notify lane"
        );
        let msgs = persist.list_since(&topic, None).unwrap();
        let raw = msgs
            .iter()
            .rev()
            .find(|m| m.body.as_deref().unwrap_or("").contains("99% full"))
            .expect("the disk notification is on the lane");

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
        assert_eq!(*severity, Severity::Critical);
        assert_eq!(
            fields.get("summary").map(String::as_str),
            Some("filesystem / is 99% full")
        );
        assert_eq!(fields.get("source").map(String::as_str), Some("disk"));
    }
}
