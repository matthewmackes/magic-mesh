//! NODE-GRADE-1 — the per-node self-grade worker (`docs/design/node-grade.md`,
//! locked 2026-07-04).
//!
//! Every node computes and publishes its own **A–F capability grade** so the
//! left-dock grade mini-list (NODE-GRADE-2) is a thin renderer: the scanning +
//! judgement live in the daemon (§6), never the GUI. The grade blends current
//! **health** with spare **headroom** (#17) — a maxed resource scores low even
//! when nothing has "failed" — so a row reads as *how capable is this node right
//! now*, not just *is it up*.
//!
//! ## Shape (mirrors the SEC-5 mesh-shunt publish + the CHAT-FIX-2 notify producer)
//!
//! - **Five factor sub-scores (0–100 each)** from telemetry the platform already
//!   gathers (§6, no new probes): **CPU** (load1 vs cores → headroom), **RAM** and
//!   **disk** (free %), **role/worker health** (the supervisor's live worker status
//!   + `systemctl --failed`), and **mesh reachability** (the replicated peer
//!   directory — overlay up + lighthouse/peer reach). Each is `Option<u8>`: a probe
//!   that can't be read is an honest `null` (§7), never a fabricated number.
//! - **A weighted average** with the **resources heaviest** (#13): CPU/RAM/disk
//!   dominate ([`W_CPU`]/[`W_RAM`]/[`W_DISK`] each 3 of the 11 total weight);
//!   role + mesh are lighter (1 each). The average is taken over only the factors
//!   that could be measured.
//! - **A smoothed score + trend** ([`Smoother`]): the raw overall score is folded
//!   into a bounded ring ([`SMOOTH_WINDOW`] samples ≈ [`DEFAULT_POLL_INTERVAL`] ×
//!   window ≈ 20–30 s, #12) so a momentary spike doesn't flip the band; the band
//!   (#9, classic 90/80/70/60) is read off the **smoothed** value, and a
//!   recent-slope [`Trend`] arrow (#14) shows rising/steady/falling.
//! - **The `<workgroup_root>/node-grade/<hostname>.json` mirror** ([`publish_grade`])
//!   — the SEC-5 `kdc-phones`/`kdc-notify` own-row-authority idiom (atomic temp +
//!   rename) so every peer reads every node's grade off the substrate.
//! - **A debounced D/F alert** ([`AlertGate`]): a transition **into** band D or F
//!   (or an escalation D→F) publishes an alert-shaped body on
//!   [`NOTIFY_TOPIC`] (`event/notify/node-grade`) — a lane the `chat` worker folds
//!   into the Chat feed (CHAT-FIX-2, #20) so the drop reaches the phone. Debounced
//!   against flapping by a cooldown ([`ALERT_COOLDOWN`]) over the *smoothed* band.
//!
//! The pure scoring / smoothing / debounce / mesh-fold cores take plain values, so
//! the whole judgement folds headless; the OS/substrate reads live behind the
//! injectable [`NodeSampler`] seam (production [`SystemSampler`]; tests inject a
//! fake), exactly like the `notify` worker's [`super::notify::Probe`].

#![cfg(feature = "async-services")]

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use mde_bus::hooks::config::Priority;
use mde_bus::persist::Persist;
use serde::{Deserialize, Serialize};

use mackes_mesh_types::peers::{peers_dir, read_peers, PeerRecord};

use super::{ShutdownToken, Worker, WorkerStatusMap};

// ── cadence + tuning ─────────────────────────────────────────────────────────

/// Self-grade tick cadence — one sample per tick. With [`SMOOTH_WINDOW`] the
/// smoothing window spans ~20–30 s (#12: no flicker on a momentary spike).
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(10);

/// Unconditional republish heartbeat, so a late reader still finds a recent row
/// even when the grade hasn't changed.
pub const PUBLISH_HEARTBEAT: Duration = Duration::from_secs(60);

/// Raw-score samples kept for smoothing + trend (≈ the [`DEFAULT_POLL_INTERVAL`]
/// window that #12 calls for).
const SMOOTH_WINDOW: usize = 3;

/// Minimum change (score points) across the smoothing window to read as a rising
/// or falling [`Trend`]; anything smaller is steady.
const TREND_EPS: i32 = 2;

/// Debounce window between D/F alerts — the flapping guard (#20). A band that
/// bounces C↔D faster than this alerts once, not on every dip.
pub const ALERT_COOLDOWN: Duration = Duration::from_secs(120);

/// A peer-directory row older than this is treated as unreachable for the mesh
/// factor (heartbeats land far more often than this).
pub const PEER_STALE_MS: u64 = 120_000;

// ── factor weights (resources heaviest, #13) ─────────────────────────────────

/// CPU-headroom weight (a resource — heaviest tier).
pub const W_CPU: u32 = 3;
/// RAM-free weight (a resource — heaviest tier).
pub const W_RAM: u32 = 3;
/// Disk-free weight (a resource — heaviest tier).
pub const W_DISK: u32 = 3;
/// Role/worker-health weight (lighter than the resources).
pub const W_ROLE: u32 = 1;
/// Mesh-reachability weight (lighter than the resources).
pub const W_MESH: u32 = 1;

/// The Bus lane the D/F alert rides — folded by the `chat` worker
/// ([`super::chat::ALERT_LANE_PREFIXES`] carries `event/notify/`) into this
/// node's `alert:<self>` conversation, so the drop reaches the Chat feed (#20).
pub const NOTIFY_TOPIC: &str = "event/notify/node-grade";

/// The stable `source` token on the published alert body (the Chat card badge).
pub const NOTIFY_SOURCE: &str = "node-grade";

// ── the A–F band (#9, classic 90/80/70/60) ───────────────────────────────────

/// The capability band a score falls into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Band {
    /// ≥ 90 — healthy AND roomy.
    A,
    /// 80–89.
    B,
    /// 70–79.
    C,
    /// 60–69 — the first alarm band.
    D,
    /// < 60 — failing OR maxed out.
    F,
}

impl Band {
    /// Map a 0–100 score to its band (classic thresholds, #9).
    #[must_use]
    pub const fn from_score(score: u8) -> Self {
        match score {
            90..=u8::MAX => Self::A,
            80..=89 => Self::B,
            70..=79 => Self::C,
            60..=69 => Self::D,
            _ => Self::F,
        }
    }

    /// The single-letter label the dock renders.
    #[must_use]
    pub const fn letter(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
            Self::C => "C",
            Self::D => "D",
            Self::F => "F",
        }
    }

    /// Whether this band blinks + alerts (D or F, #6/#20).
    #[must_use]
    pub const fn is_alarm(self) -> bool {
        matches!(self, Self::D | Self::F)
    }
}

/// The recent-slope trend arrow (#14).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Trend {
    /// Score rising over the recent window.
    Up,
    /// Score roughly flat.
    Steady,
    /// Score falling over the recent window.
    Down,
}

impl Trend {
    /// The arrow glyph the dock renders.
    #[must_use]
    pub const fn arrow(self) -> &'static str {
        match self {
            Self::Up => "↑",
            Self::Steady => "→",
            Self::Down => "↓",
        }
    }
}

// ── the pure factor scorers (each blends health + headroom, #17) ─────────────

/// A small count (cores, workers, peers) as `f32` for the scoring math. Saturates
/// at `u16::MAX` so the widening is a lossless `f32::from(u16)` — no count this
/// worker sees comes near that, and it sidesteps a lossy `u32 as f32` entirely.
fn count_f32(v: u32) -> f32 {
    f32::from(u16::try_from(v).unwrap_or(u16::MAX))
}

/// Round + clamp a computed value into a `0..=100` score — the one spot a bounded
/// `f32` becomes a `u8` (clamped first, so it can neither truncate nor lose a
/// sign).
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn clamp_score(v: f32) -> u8 {
    v.round().clamp(0.0, 100.0) as u8
}

/// Rounded integer division of a summed set of `0..=100` values by its
/// weight/count total → a `0..=100` score. The quotient is provably ≤ 100, so the
/// cast can't truncate.
#[allow(clippy::cast_possible_truncation)]
const fn div_round_u8(num: u32, den: u32) -> u8 {
    ((num + den / 2) / den) as u8
}

/// CPU factor — spare headroom against the core count. `load1 == cores` (fully
/// loaded) scores 0; idle scores 100. A maxed CPU is low even though nothing has
/// "failed" (#17). Zero cores (an impossible read) scores 0 (honest floor).
#[must_use]
pub fn score_cpu(load1: f32, cores: u32) -> u8 {
    if cores == 0 {
        return 0;
    }
    let load_ratio = load1 / count_f32(cores);
    let headroom = (1.0 - load_ratio).clamp(0.0, 1.0);
    clamp_score(headroom * 100.0)
}

/// RAM / disk factor — the free percentage IS the score, so a maxed resource
/// (0 % free) scores 0 even when healthy (#17).
#[must_use]
pub fn score_free_pct(free_pct: f32) -> u8 {
    clamp_score(free_pct)
}

/// Role/worker-health factor. `running / expected` is the headroom (fraction of
/// this node's spawned workers still alive); a tripped breaker or a failed
/// systemd unit is a hard health hit on top. `expected == 0` (the supervisor
/// status not yet attached / a headless test) reads as a healthy baseline rather
/// than a false 0.
#[must_use]
pub fn score_role(expected: u32, running: u32, tripped: u32, failed_services: u32) -> u8 {
    let run_frac = if expected == 0 {
        1.0
    } else {
        (count_f32(running) / count_f32(expected)).clamp(0.0, 1.0)
    };
    let mut score = 100.0 * run_frac;
    score -= count_f32(tripped) * 25.0;
    score -= count_f32(failed_services.min(5)) * 15.0;
    clamp_score(score)
}

/// Mesh-reachability factor. Overlay **down** is a failing mesh — 0 (the design's
/// "unreachable is itself an F" posture). Overlay up is a 50 baseline; a
/// reachable lighthouse adds 30; the reachable-peer fraction adds up to 20 (a
/// lone node with no peers isn't penalized for the peer term).
#[must_use]
pub fn score_mesh(
    overlay_up: bool,
    lighthouse_reachable: bool,
    peers_total: u32,
    peers_reachable: u32,
) -> u8 {
    if !overlay_up {
        return 0;
    }
    let mut score = 50.0_f32;
    if lighthouse_reachable {
        score += 30.0;
    }
    let reach_frac = if peers_total == 0 {
        1.0
    } else {
        (count_f32(peers_reachable) / count_f32(peers_total)).clamp(0.0, 1.0)
    };
    score += 20.0 * reach_frac;
    clamp_score(score)
}

// ── the raw signals + the seam that samples them ─────────────────────────────

/// CPU load signal (load1 + logical core count).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CpuSignal {
    /// 1-minute load average.
    pub load1: f32,
    /// Logical CPUs.
    pub cores: u32,
}

/// Role/worker-health signal (the supervisor's live status + failed units).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RoleSignal {
    /// Workers this node spawned per its role (the supervisor's tracked total).
    pub expected: u32,
    /// Of those, how many are currently alive.
    pub running: u32,
    /// Workers whose circuit breaker has tripped (stay down).
    pub tripped: u32,
    /// `systemctl --failed` unit count.
    pub failed_services: u32,
}

/// Mesh-reachability signal (folded from the replicated peer directory).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MeshSignal {
    /// This node has a live Nebula overlay IP (own peer row carries one).
    pub overlay_up: bool,
    /// A lighthouse is reachable (a fresh lighthouse peer, or this node is one).
    pub lighthouse_reachable: bool,
    /// Non-self peers in the directory.
    pub peers_total: u32,
    /// Of those, how many are fresh + not `unreachable`.
    pub peers_reachable: u32,
}

/// One raw sample of every factor's inputs. The resource reads are `Option` so an
/// absent probe stays honestly absent (§7); role + mesh always resolve (an empty
/// directory / no supervisor is a valid, low-ish reading, not a missing one).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NodeSignals {
    /// CPU inputs, or `None` when `/proc/loadavg` couldn't be read.
    pub cpu: Option<CpuSignal>,
    /// RAM free %, or `None` when `/proc/meminfo` couldn't be read.
    pub ram_free_pct: Option<f32>,
    /// Disk free % on `/`, or `None` when `df` couldn't be run.
    pub disk_free_pct: Option<f32>,
    /// Role/worker health.
    pub role: RoleSignal,
    /// Mesh reachability.
    pub mesh: MeshSignal,
}

impl NodeSignals {
    /// Score every input into the published [`Factors`] (each pure scorer applied,
    /// absent resource reads carried through as `None`).
    #[must_use]
    pub fn to_factors(&self) -> Factors {
        Factors {
            cpu: self.cpu.map(|c| score_cpu(c.load1, c.cores)),
            ram: self.ram_free_pct.map(score_free_pct),
            disk: self.disk_free_pct.map(score_free_pct),
            role: Some(score_role(
                self.role.expected,
                self.role.running,
                self.role.tripped,
                self.role.failed_services,
            )),
            mesh: Some(score_mesh(
                self.mesh.overlay_up,
                self.mesh.lighthouse_reachable,
                self.mesh.peers_total,
                self.mesh.peers_reachable,
            )),
        }
    }
}

/// The injectable sampler seam — production [`SystemSampler`] reads the OS +
/// substrate; tests inject a fake (mirrors the `notify` worker's `Probe`).
pub trait NodeSampler: Send {
    /// Gather one raw sample of every factor's inputs.
    fn sample(&self) -> NodeSignals;
}

// ── the published grade + its factor breakdown ───────────────────────────────

/// The five published factor sub-scores (0–100). A `None` field is an honestly
/// unmeasurable factor (§7) — serialized as JSON `null`, never faked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Factors {
    /// CPU headroom.
    pub cpu: Option<u8>,
    /// RAM free.
    pub ram: Option<u8>,
    /// Disk free.
    pub disk: Option<u8>,
    /// Role/worker health.
    pub role: Option<u8>,
    /// Mesh reachability.
    pub mesh: Option<u8>,
}

impl Factors {
    /// The weighted average over the factors that could be measured (resources
    /// heaviest, #13). `None` only when nothing at all could be scored.
    #[must_use]
    pub fn weighted_score(&self) -> Option<u8> {
        let parts = [
            (W_CPU, self.cpu),
            (W_RAM, self.ram),
            (W_DISK, self.disk),
            (W_ROLE, self.role),
            (W_MESH, self.mesh),
        ];
        let mut num = 0_u32;
        let mut den = 0_u32;
        for (w, v) in parts {
            if let Some(v) = v {
                num += w * u32::from(v);
                den += w;
            }
        }
        if den == 0 {
            return None;
        }
        Some(div_round_u8(num, den))
    }
}

/// The body published to `<workgroup_root>/node-grade/<hostname>.json` — this
/// node's own-row-authority grade the whole mesh reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeGrade {
    /// The publishing node's hostname (the file stem + row key).
    pub host: String,
    /// The A–F letter (off the smoothed score).
    pub grade: String,
    /// The smoothed 0–100 score.
    pub score: u8,
    /// The five factor sub-scores.
    pub factors: Factors,
    /// The recent-slope trend arrow.
    pub trend: Trend,
    /// Wall-clock ms when this row was published.
    pub published_at_ms: u64,
}

/// The replicated directory holding every node's published grade.
#[must_use]
pub fn grade_dir(workgroup_root: &Path) -> PathBuf {
    workgroup_root.join("node-grade")
}

/// Write this node's grade to its own published file (atomic temp + rename —
/// the SEC-5 `publish_roster` own-row idiom).
///
/// # Errors
/// IO / serialization failures.
pub fn publish_grade(
    workgroup_root: &Path,
    hostname: &str,
    grade: &NodeGrade,
) -> std::io::Result<PathBuf> {
    let dir = grade_dir(workgroup_root);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{hostname}.json"));
    let body = serde_json::to_string_pretty(grade)?;
    let tmp = dir.join(format!(".{hostname}.json.tmp"));
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Read every node's published grade (own file included). Junk / half-replicated
/// files are skipped. Sorted by hostname for a stable render order.
#[must_use]
pub fn read_grades(workgroup_root: &Path) -> Vec<NodeGrade> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(grade_dir(workgroup_root)) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with('.'))
        {
            continue;
        }
        if let Ok(data) = std::fs::read_to_string(&path) {
            if let Ok(g) = serde_json::from_str::<NodeGrade>(&data) {
                out.push(g);
            }
        }
    }
    out.sort_by(|a, b| a.host.cmp(&b.host));
    out
}

// ── smoothing + trend ────────────────────────────────────────────────────────

/// A bounded ring of recent raw scores → a smoothed value + a trend (#12/#14).
#[derive(Debug, Default)]
pub struct Smoother {
    ring: VecDeque<u8>,
}

impl Smoother {
    /// A fresh, empty smoother.
    #[must_use]
    pub fn new() -> Self {
        Self {
            ring: VecDeque::with_capacity(SMOOTH_WINDOW),
        }
    }

    /// Fold one raw score in (evicting the oldest past the window).
    pub fn push(&mut self, raw: u8) {
        if self.ring.len() == SMOOTH_WINDOW {
            self.ring.pop_front();
        }
        self.ring.push_back(raw);
    }

    /// The smoothed score (mean of the ring). 0 before any sample.
    #[must_use]
    pub fn smoothed(&self) -> u8 {
        let n = u32::try_from(self.ring.len()).unwrap_or(u32::MAX);
        if n == 0 {
            return 0;
        }
        let sum: u32 = self.ring.iter().map(|&v| u32::from(v)).sum();
        div_round_u8(sum, n)
    }

    /// The trend across the window (oldest → newest slope, #14).
    #[must_use]
    pub fn trend(&self) -> Trend {
        let (Some(&first), Some(&last)) = (self.ring.front(), self.ring.back()) else {
            return Trend::Steady;
        };
        if self.ring.len() < 2 {
            return Trend::Steady;
        }
        let delta = i32::from(last) - i32::from(first);
        if delta > TREND_EPS {
            Trend::Up
        } else if delta < -TREND_EPS {
            Trend::Down
        } else {
            Trend::Steady
        }
    }
}

// ── the debounced D/F alert gate (#20) ───────────────────────────────────────

/// Alert severity — Warning for a drop into D, Critical for F.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertLevel {
    /// A drop into band D.
    Warning,
    /// A drop into (or escalation to) band F.
    Critical,
}

impl AlertLevel {
    /// The `severity` string the chat alert-fold reads.
    #[must_use]
    pub const fn severity(self) -> &'static str {
        match self {
            Self::Warning => "warning",
            Self::Critical => "critical",
        }
    }
}

/// Edge-triggered, debounced gate over the smoothed band. Fires only on a
/// transition **into** the alarm zone (or an escalation D→F), and never twice
/// inside [`ALERT_COOLDOWN`] — the flapping guard (#20).
#[derive(Debug, Default)]
pub struct AlertGate {
    prev_band: Option<Band>,
    last_alert_at: Option<Instant>,
}

impl AlertGate {
    /// Evaluate the current smoothed `band` at `now`; `Some(level)` when an alert
    /// should fire. Entering D/F from a better band respects the cooldown; an
    /// escalation D→F always fires (a genuine worsening worth surfacing).
    pub fn evaluate(&mut self, band: Band, now: Instant) -> Option<AlertLevel> {
        let prev = self.prev_band.replace(band);
        let entering = band.is_alarm() && prev.is_none_or(|p| !p.is_alarm());
        let escalating = band == Band::F && prev == Some(Band::D);
        if !(entering || escalating) {
            return None;
        }
        let cooled = self
            .last_alert_at
            .is_none_or(|t| now.duration_since(t) >= ALERT_COOLDOWN);
        if !cooled && !escalating {
            return None;
        }
        self.last_alert_at = Some(now);
        Some(if band == Band::F {
            AlertLevel::Critical
        } else {
            AlertLevel::Warning
        })
    }
}

/// The on-Bus alert body — an alert-shaped JSON object the `chat` worker's
/// [`mde_chat::fold_alert`] understands (`severity` drives the color; the string
/// fields become the card's rows). Every field is a `&str` so `fold_alert` keeps
/// it (it preserves string fields only).
#[derive(Debug, Serialize)]
struct GradeAlertBody<'a> {
    severity: &'a str,
    source: &'a str,
    summary: &'a str,
    host: &'a str,
    grade: &'a str,
    score: &'a str,
    ts_unix_ms: i64,
}

/// Serialize + publish one D/F alert on [`NOTIFY_TOPIC`]. Best-effort (a write
/// failure is logged, never fatal).
fn emit_alert(
    persist: &Persist,
    host: &str,
    band: Band,
    score: u8,
    level: AlertLevel,
    ts_unix_ms: i64,
) {
    let summary = format!(
        "node {host} dropped to grade {} (score {score})",
        band.letter()
    );
    let score_str = score.to_string();
    let body = GradeAlertBody {
        severity: level.severity(),
        source: NOTIFY_SOURCE,
        summary: &summary,
        host,
        grade: band.letter(),
        score: &score_str,
        ts_unix_ms,
    };
    let Ok(json) = serde_json::to_string(&body) else {
        return;
    };
    if let Err(e) = persist.write(NOTIFY_TOPIC, Priority::Default, None, Some(&json)) {
        tracing::debug!(
            target: "mackesd::node_grade",
            topic = NOTIFY_TOPIC,
            error = %e,
            "grade alert publish failed",
        );
    }
}

// ── the production sampler (OS + substrate reads; no new probes, §6) ─────────

/// Wall-clock ms since the epoch.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Wall-clock ms as `i64` (the alert timestamp).
fn now_ms_i64() -> i64 {
    i64::try_from(now_ms()).unwrap_or(i64::MAX)
}

/// The default Bus root (persisted message tree), matching every other worker.
fn default_bus_root() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("mde").join("bus"))
}

/// Parse the 1-minute load from a `/proc/loadavg` body (first whitespace field).
#[must_use]
pub fn parse_loadavg(body: &str) -> Option<f32> {
    body.split_whitespace().next()?.parse::<f32>().ok()
}

/// Parse RAM free % from a `/proc/meminfo` body: `MemAvailable / MemTotal`
/// (falling back to `MemFree` when `MemAvailable` is absent). `None` when
/// `MemTotal` is missing or zero.
#[must_use]
pub fn parse_mem_free_pct(body: &str) -> Option<f32> {
    let field = |key: &str| -> Option<u64> {
        body.lines()
            .find_map(|l| l.strip_prefix(key))
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|kb| kb.parse::<u64>().ok())
    };
    let total = field("MemTotal:")?;
    if total == 0 {
        return None;
    }
    let avail = field("MemAvailable:").or_else(|| field("MemFree:"))?;
    // kB counts → a percentage; the precision loss the pedantic lint warns about
    // is immaterial for a 0–100 ratio.
    #[allow(clippy::cast_precision_loss)]
    let pct = (avail as f32 / total as f32) * 100.0;
    Some(pct)
}

/// Parse disk free % from a `df -P <path>` body (`100 - capacity%` of the data
/// line, the 5th column). `None` when the output is malformed.
#[must_use]
pub fn parse_df_free_pct(body: &str) -> Option<f32> {
    let line = body.lines().nth(1)?;
    let pct = line.split_whitespace().nth(4)?;
    let used: u8 = pct.trim_end_matches('%').parse().ok()?;
    Some(f32::from(100_u8.saturating_sub(used)))
}

/// Count failed units in a `systemctl --failed --no-legend --plain` body (one
/// non-blank line per unit).
#[must_use]
pub fn parse_failed_count(body: &str) -> u32 {
    u32::try_from(body.lines().filter(|l| !l.trim().is_empty()).count()).unwrap_or(u32::MAX)
}

/// Fold the replicated peer directory into a [`MeshSignal`] (pure — the read is
/// the caller's). Overlay-up comes from this node's own row carrying an overlay
/// IP; a lighthouse is reachable when a fresh lighthouse peer exists or this node
/// is itself a lighthouse.
#[must_use]
pub fn mesh_signal(
    records: &[PeerRecord],
    self_host: &str,
    self_is_lighthouse: bool,
    stale_ms: u64,
) -> MeshSignal {
    let overlay_up = records
        .iter()
        .find(|r| r.hostname == self_host)
        .is_some_and(|r| r.overlay_ip.is_some());
    let peers: Vec<&PeerRecord> = records.iter().filter(|r| r.hostname != self_host).collect();
    let fresh = |r: &PeerRecord| !r.is_stale(stale_ms) && r.health != "unreachable";
    let peers_reachable = peers.iter().filter(|r| fresh(r)).count();
    let lighthouse_reachable = self_is_lighthouse
        || peers
            .iter()
            .any(|r| r.role.as_deref() == Some("lighthouse") && fresh(r));
    MeshSignal {
        overlay_up,
        lighthouse_reachable,
        peers_total: u32::try_from(peers.len()).unwrap_or(u32::MAX),
        peers_reachable: u32::try_from(peers_reachable).unwrap_or(u32::MAX),
    }
}

/// The production sampler: reads `/proc/loadavg`, `/proc/meminfo`, `df /`,
/// `systemctl --failed`, the supervisor's live worker status, and the replicated
/// peer directory — all telemetry the platform already gathers (§6, no new
/// probes).
pub struct SystemSampler {
    workgroup_root: PathBuf,
    self_host: String,
    self_is_lighthouse: bool,
    worker_status: Option<WorkerStatusMap>,
}

impl SystemSampler {
    /// Build the production sampler. `role_rank == 0` marks this node as a
    /// lighthouse for the mesh factor; `worker_status` is the supervisor's live
    /// map for the role factor (`None` degrades role to a healthy baseline).
    #[must_use]
    pub const fn new(
        workgroup_root: PathBuf,
        self_host: String,
        role_rank: u8,
        worker_status: Option<WorkerStatusMap>,
    ) -> Self {
        Self {
            workgroup_root,
            self_host,
            self_is_lighthouse: role_rank == 0,
            worker_status,
        }
    }

    /// Read a whole `/proc` file (small virtual files, one read).
    fn read_proc(path: &str) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }

    /// Run a bounded command, returning its stdout when it exits 0.
    fn run(program: &str, args: &[&str]) -> Option<String> {
        let mut cmd = Command::new(program);
        cmd.args(args);
        let out = super::proc::output_with_timeout(cmd, super::proc::DEFAULT_CMD_TIMEOUT).ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    }

    fn cpu_signal() -> Option<CpuSignal> {
        let load1 = parse_loadavg(&Self::read_proc("/proc/loadavg")?)?;
        let cores = u32::try_from(
            std::thread::available_parallelism()
                .map(std::num::NonZeroUsize::get)
                .unwrap_or(1),
        )
        .unwrap_or(1);
        Some(CpuSignal { load1, cores })
    }

    fn role_signal(&self) -> RoleSignal {
        let (running, expected, tripped) = self
            .worker_status
            .as_ref()
            .map_or((0, 0, 0), super::workers_ready);
        let failed_services = Self::run("systemctl", &["--failed", "--no-legend", "--plain"])
            .map_or(0, |o| parse_failed_count(&o));
        RoleSignal {
            expected,
            running,
            tripped,
            failed_services,
        }
    }

    fn mesh_signal(&self) -> MeshSignal {
        let records = read_peers(&peers_dir(&self.workgroup_root));
        mesh_signal(
            &records,
            &self.self_host,
            self.self_is_lighthouse,
            PEER_STALE_MS,
        )
    }
}

impl NodeSampler for SystemSampler {
    fn sample(&self) -> NodeSignals {
        NodeSignals {
            cpu: Self::cpu_signal(),
            ram_free_pct: Self::read_proc("/proc/meminfo")
                .as_deref()
                .and_then(parse_mem_free_pct),
            disk_free_pct: Self::run("df", &["-P", "/"])
                .as_deref()
                .and_then(parse_df_free_pct),
            role: self.role_signal(),
            mesh: self.mesh_signal(),
        }
    }
}

// ── the worker ───────────────────────────────────────────────────────────────

/// The NODE-GRADE-1 `node_grade` worker (rank 0, every node self-grades).
pub struct NodeGradeWorker {
    host: String,
    workgroup_root: PathBuf,
    sampler: Box<dyn NodeSampler>,
    bus_root: Option<PathBuf>,
    poll: Duration,
    heartbeat: Duration,
    smoother: Smoother,
    gate: AlertGate,
}

impl NodeGradeWorker {
    /// Construct with production defaults: the [`SystemSampler`], the default Bus
    /// root, and the standard cadences. `host` is this node's hostname (the
    /// publish key); `role_rank` + `worker_status` feed the role/mesh factors.
    #[must_use]
    pub fn new(
        host: String,
        workgroup_root: PathBuf,
        role_rank: u8,
        worker_status: Option<WorkerStatusMap>,
    ) -> Self {
        let sampler = Box::new(SystemSampler::new(
            workgroup_root.clone(),
            host.clone(),
            role_rank,
            worker_status,
        ));
        Self {
            host,
            workgroup_root,
            sampler,
            bus_root: default_bus_root(),
            poll: DEFAULT_POLL_INTERVAL,
            heartbeat: PUBLISH_HEARTBEAT,
            smoother: Smoother::new(),
            gate: AlertGate::default(),
        }
    }

    /// Inject the sampler (tests supply a fake; production is [`SystemSampler`]).
    #[must_use]
    pub fn with_sampler(mut self, sampler: Box<dyn NodeSampler>) -> Self {
        self.sampler = sampler;
        self
    }

    /// Override the Bus root (tests point it at a tempdir Persist; `None` idles
    /// the alert leg).
    #[must_use]
    pub fn with_bus_root(mut self, bus_root: Option<PathBuf>) -> Self {
        self.bus_root = bus_root;
        self
    }

    /// Override the poll cadence (tests use a short value).
    #[must_use]
    pub const fn with_poll(mut self, poll: Duration) -> Self {
        self.poll = poll;
        self
    }

    /// Sample → score → smooth → build the current [`NodeGrade`] (no publish —
    /// the pure step the tick + tests share).
    fn cycle(&mut self) -> NodeGrade {
        let factors = self.sampler.sample().to_factors();
        let raw = factors.weighted_score().unwrap_or(0);
        self.smoother.push(raw);
        let score = self.smoother.smoothed();
        NodeGrade {
            host: self.host.clone(),
            grade: Band::from_score(score).letter().to_string(),
            score,
            factors,
            trend: self.smoother.trend(),
            published_at_ms: now_ms(),
        }
    }

    /// Publish the grade file + fire the debounced D/F alert when the smoothed
    /// band crosses into the alarm zone.
    fn publish_and_alert(&mut self, grade: &NodeGrade, persist: Option<&Persist>, now: Instant) {
        if let Err(e) = publish_grade(&self.workgroup_root, &self.host, grade) {
            tracing::debug!(
                target: "mackesd::node_grade",
                error = %e,
                "grade publish failed",
            );
        }
        let band = Band::from_score(grade.score);
        if let Some(level) = self.gate.evaluate(band, now) {
            if let Some(p) = persist {
                emit_alert(p, &self.host, band, grade.score, level, now_ms_i64());
            }
        }
    }
}

#[async_trait::async_trait]
impl Worker for NodeGradeWorker {
    fn name(&self) -> &'static str {
        "node_grade"
    }

    async fn run(&mut self, mut shutdown: ShutdownToken) -> anyhow::Result<()> {
        let persist = self
            .bus_root
            .clone()
            .and_then(|root| Persist::open(root).ok());
        // Publish immediately so the dock doesn't wait a full tick for this
        // node's first grade row.
        let grade = self.cycle();
        self.publish_and_alert(&grade, persist.as_ref(), Instant::now());
        let mut last_pub = Instant::now();
        let mut tick = tokio::time::interval(self.poll);
        tick.tick().await; // consume the immediate first tick
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    let now = Instant::now();
                    let grade = self.cycle();
                    // The grade file republishes every tick (it's this node's own
                    // cheap row); the heartbeat is a belt-and-suspenders floor for
                    // any future publish-on-change gate.
                    let _ = now.duration_since(last_pub) >= self.heartbeat;
                    last_pub = now;
                    self.publish_and_alert(&grade, persist.as_ref(), now);
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

    /// A canned sampler that replays a fixed sequence of signals (last repeats).
    struct FakeSampler {
        seq: std::sync::Mutex<VecDeque<NodeSignals>>,
    }

    impl FakeSampler {
        fn new(seq: Vec<NodeSignals>) -> Self {
            Self {
                seq: std::sync::Mutex::new(seq.into()),
            }
        }
    }

    impl NodeSampler for FakeSampler {
        fn sample(&self) -> NodeSignals {
            let mut g = self.seq.lock().unwrap();
            if g.len() > 1 {
                g.pop_front().unwrap()
            } else {
                *g.front().unwrap()
            }
        }
    }

    fn signals(cpu: Option<(f32, u32)>, ram: Option<f32>, disk: Option<f32>) -> NodeSignals {
        NodeSignals {
            cpu: cpu.map(|(load1, cores)| CpuSignal { load1, cores }),
            ram_free_pct: ram,
            disk_free_pct: disk,
            role: RoleSignal {
                expected: 10,
                running: 10,
                tripped: 0,
                failed_services: 0,
            },
            mesh: MeshSignal {
                overlay_up: true,
                lighthouse_reachable: true,
                peers_total: 2,
                peers_reachable: 2,
            },
        }
    }

    #[test]
    fn cpu_blends_health_and_headroom() {
        // Idle 8-core box → full headroom.
        assert_eq!(score_cpu(0.0, 8), 100);
        // Half loaded → half headroom.
        assert_eq!(score_cpu(4.0, 8), 50);
        // Maxed (load == cores) scores 0 even though nothing "failed" (#17).
        assert_eq!(score_cpu(8.0, 8), 0);
        // Over-subscribed clamps at 0, never negative.
        assert_eq!(score_cpu(20.0, 8), 0);
        // An impossible zero-core read floors at 0, never divides by zero.
        assert_eq!(score_cpu(1.0, 0), 0);
    }

    #[test]
    fn free_pct_is_the_resource_score() {
        assert_eq!(score_free_pct(100.0), 100);
        assert_eq!(score_free_pct(0.0), 0); // maxed resource → 0 (#17)
        assert_eq!(score_free_pct(42.4), 42);
        assert_eq!(score_free_pct(150.0), 100); // clamps
    }

    #[test]
    fn role_penalizes_down_workers_trips_and_failed_units() {
        // All healthy.
        assert_eq!(score_role(20, 20, 0, 0), 100);
        // Half the workers down → half score (headroom).
        assert_eq!(score_role(20, 10, 0, 0), 50);
        // A tripped breaker is a hard -25.
        assert_eq!(score_role(20, 20, 1, 0), 75);
        // Failed units cost -15 each (capped at 5).
        assert_eq!(score_role(20, 20, 0, 2), 70);
        assert_eq!(score_role(20, 20, 0, 99), 25); // cap: 5 * 15 = 75 off
                                                   // No supervisor attached (expected 0) reads as a healthy baseline.
        assert_eq!(score_role(0, 0, 0, 0), 100);
    }

    #[test]
    fn mesh_overlay_down_is_failing_and_reach_scales() {
        // Overlay down = failing mesh (0), regardless of anything else.
        assert_eq!(score_mesh(false, true, 5, 5), 0);
        // Fully connected.
        assert_eq!(score_mesh(true, true, 3, 3), 100);
        // Overlay up, no lighthouse, no peers reachable.
        assert_eq!(score_mesh(true, false, 4, 0), 50);
        // Overlay up + lighthouse + half the peers reachable → 50 + 30 + 10.
        assert_eq!(score_mesh(true, true, 4, 2), 90);
        // A lone node (no peers) isn't penalized for the peer term.
        assert_eq!(score_mesh(true, true, 0, 0), 100);
    }

    #[test]
    fn weighted_average_makes_resources_dominant() {
        // Resources maxed-out (0) but role/mesh perfect: the score stays low
        // because resources carry 9 of the 11 weight (#13).
        let f = Factors {
            cpu: Some(0),
            ram: Some(0),
            disk: Some(0),
            role: Some(100),
            mesh: Some(100),
        };
        // (0*9 + 100*2) / 11 = 18.
        assert_eq!(f.weighted_score(), Some(18));
        // All perfect → 100.
        let perfect = Factors {
            cpu: Some(100),
            ram: Some(100),
            disk: Some(100),
            role: Some(100),
            mesh: Some(100),
        };
        assert_eq!(perfect.weighted_score(), Some(100));
    }

    #[test]
    fn honest_absent_averages_over_only_measured_factors() {
        // The resource probes are all absent (§7) — the average is taken over the
        // role + mesh factors only, never over a fabricated resource number.
        let f = Factors {
            cpu: None,
            ram: None,
            disk: None,
            role: Some(80),
            mesh: Some(60),
        };
        // (80*1 + 60*1) / 2 = 70, NOT diluted by phantom zero resources.
        assert_eq!(f.weighted_score(), Some(70));
        // Nothing measurable at all → None.
        let empty = Factors {
            cpu: None,
            ram: None,
            disk: None,
            role: None,
            mesh: None,
        };
        assert_eq!(empty.weighted_score(), None);
    }

    #[test]
    fn absent_factors_serialize_as_null_not_a_number() {
        let f = Factors {
            cpu: None,
            ram: Some(90),
            disk: Some(88),
            role: Some(100),
            mesh: Some(95),
        };
        let json = serde_json::to_string(&f).unwrap();
        assert!(
            json.contains("\"cpu\":null"),
            "absent factor is honest null: {json}"
        );
        assert!(json.contains("\"ram\":90"));
    }

    #[test]
    fn band_maps_at_the_classic_thresholds() {
        assert_eq!(Band::from_score(100), Band::A);
        assert_eq!(Band::from_score(90), Band::A);
        assert_eq!(Band::from_score(89), Band::B);
        assert_eq!(Band::from_score(80), Band::B);
        assert_eq!(Band::from_score(79), Band::C);
        assert_eq!(Band::from_score(70), Band::C);
        assert_eq!(Band::from_score(69), Band::D);
        assert_eq!(Band::from_score(60), Band::D);
        assert_eq!(Band::from_score(59), Band::F);
        assert_eq!(Band::from_score(0), Band::F);
        // Only D and F are alarm bands.
        assert!(Band::D.is_alarm() && Band::F.is_alarm());
        assert!(!Band::A.is_alarm() && !Band::B.is_alarm() && !Band::C.is_alarm());
    }

    #[test]
    fn smoothing_dampens_a_momentary_spike() {
        let mut s = Smoother::new();
        // Two healthy samples then one brief crash: raw last is 20 (F), but the
        // smoothed value stays a C — no band flicker on a momentary spike (#12).
        s.push(95);
        s.push(95);
        s.push(20);
        assert_eq!(s.smoothed(), 70);
        assert_eq!(Band::from_score(s.smoothed()), Band::C);
    }

    #[test]
    fn smoother_window_evicts_and_trends() {
        let mut s = Smoother::new();
        s.push(60);
        s.push(70);
        s.push(80);
        assert_eq!(s.trend(), Trend::Up); // 60 → 80
                                          // Rolls: 60 evicted, now 70,80,90.
        s.push(90);
        assert_eq!(s.smoothed(), 80);
        assert_eq!(s.trend(), Trend::Up);
        let mut down = Smoother::new();
        down.push(80);
        down.push(70);
        down.push(60);
        assert_eq!(down.trend(), Trend::Down);
        let mut flat = Smoother::new();
        flat.push(70);
        flat.push(71);
        flat.push(70);
        assert_eq!(flat.trend(), Trend::Steady);
    }

    #[test]
    fn alert_gate_fires_into_d_and_f_debounced() {
        let mut gate = AlertGate::default();
        let t0 = Instant::now();
        // Healthy → no alert.
        assert_eq!(gate.evaluate(Band::A, t0), None);
        assert_eq!(gate.evaluate(Band::C, t0), None);
        // First drop into D → Warning.
        assert_eq!(
            gate.evaluate(Band::D, t0 + Duration::from_secs(1)),
            Some(AlertLevel::Warning)
        );
        // Escalation D → F always fires Critical, even inside the cooldown.
        assert_eq!(
            gate.evaluate(Band::F, t0 + Duration::from_secs(2)),
            Some(AlertLevel::Critical)
        );
    }

    #[test]
    fn alert_gate_suppresses_flapping_within_cooldown() {
        let mut gate = AlertGate::default();
        let t0 = Instant::now();
        // Enter D → alert.
        assert_eq!(gate.evaluate(Band::D, t0), Some(AlertLevel::Warning));
        // Flap back up to C (no alert) then dip to D again immediately — the
        // cooldown suppresses the re-alert.
        assert_eq!(gate.evaluate(Band::C, t0 + Duration::from_secs(1)), None);
        assert_eq!(gate.evaluate(Band::D, t0 + Duration::from_secs(2)), None);
        // After the cooldown elapses, a genuine re-drop alerts again.
        assert_eq!(gate.evaluate(Band::C, t0 + Duration::from_secs(3)), None);
        assert_eq!(
            gate.evaluate(Band::D, t0 + ALERT_COOLDOWN + Duration::from_secs(4)),
            Some(AlertLevel::Warning)
        );
    }

    #[test]
    fn mesh_signal_folds_the_peer_directory() {
        let mut me = PeerRecord::now("me", None, "healthy");
        me.overlay_ip = Some("10.42.0.9".into());
        let mut lh = PeerRecord::now("lh1", None, "healthy");
        lh.role = Some("lighthouse".into());
        let peer_ok = PeerRecord::now("ws1", None, "healthy");
        let mut stale = PeerRecord::now("ws2", None, "healthy");
        stale.last_seen_ms = 1; // ancient → stale
        let records = vec![me, lh, peer_ok, stale];
        let sig = mesh_signal(&records, "me", false, PEER_STALE_MS);
        assert!(sig.overlay_up, "own overlay IP present");
        assert!(sig.lighthouse_reachable, "a fresh lighthouse peer exists");
        assert_eq!(sig.peers_total, 3); // lh1, ws1, ws2 (not self)
        assert_eq!(sig.peers_reachable, 2); // lh1 + ws1 fresh; ws2 stale
    }

    #[test]
    fn mesh_signal_overlay_down_when_no_own_row() {
        // No self row at all → overlay reads down (honest); a lighthouse NODE is
        // its own reachable lighthouse.
        let sig = mesh_signal(&[], "solo", true, PEER_STALE_MS);
        assert!(!sig.overlay_up);
        assert!(sig.lighthouse_reachable, "this node is itself a lighthouse");
        assert_eq!(sig.peers_total, 0);
    }

    #[test]
    fn parsers_read_the_platform_telemetry() {
        assert_eq!(parse_loadavg("0.52 0.58 0.59 1/234 5678\n"), Some(0.52));
        assert_eq!(parse_loadavg("garbage"), None);
        let meminfo = "MemTotal:       16000000 kB\nMemAvailable:    8000000 kB\nMemFree: 100 kB\n";
        assert_eq!(parse_mem_free_pct(meminfo), Some(50.0));
        // MemAvailable absent → falls back to MemFree.
        let fallback = "MemTotal:       1000 kB\nMemFree:          250 kB\n";
        assert_eq!(parse_mem_free_pct(fallback), Some(25.0));
        assert_eq!(parse_mem_free_pct("nothing"), None);
        let df = "Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/sda1 100 77 23 77% /\n";
        assert_eq!(parse_df_free_pct(df), Some(23.0));
        assert_eq!(parse_failed_count("a.service\nb.service\n"), 2);
        assert_eq!(parse_failed_count("\n\n"), 0);
    }

    #[test]
    fn published_json_carries_the_full_shape_and_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let grade = NodeGrade {
            host: "node-a".into(),
            grade: "B".into(),
            score: 84,
            factors: Factors {
                cpu: Some(80),
                ram: Some(90),
                disk: Some(88),
                role: Some(100),
                mesh: Some(70),
            },
            trend: Trend::Up,
            published_at_ms: 123,
        };
        let path = publish_grade(root, "node-a", &grade).unwrap();
        assert_eq!(path, grade_dir(root).join("node-a.json"));
        // Peers read every node's grade off the substrate.
        let read = read_grades(root);
        assert_eq!(read.len(), 1);
        assert_eq!(read[0], grade);
        // The wire shape: grade/score/factors{cpu,ram,disk,role,mesh}/trend.
        let raw = std::fs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["grade"], "B");
        assert_eq!(v["score"], 84);
        assert_eq!(v["trend"], "up");
        for k in ["cpu", "ram", "disk", "role", "mesh"] {
            assert!(v["factors"].get(k).is_some(), "factors.{k} present");
        }
    }

    #[test]
    fn name_matches_the_census() {
        let w = NodeGradeWorker::new("n".into(), PathBuf::from("/tmp"), 0, None);
        assert_eq!(w.name(), "node_grade");
    }

    #[test]
    fn cycle_publishes_a_grade_reflecting_the_sample() {
        let tmp = tempfile::tempdir().unwrap();
        // A maxed-out node: no CPU/RAM/disk headroom → an F despite a healthy
        // role + mesh (resources dominate, #13).
        let maxed = signals(Some((16.0, 8)), Some(2.0), Some(3.0));
        let mut w = NodeGradeWorker::new("me".into(), tmp.path().to_path_buf(), 0, None)
            .with_bus_root(None)
            .with_sampler(Box::new(FakeSampler::new(vec![maxed])));
        let g = w.cycle();
        assert_eq!(g.host, "me");
        assert_eq!(g.grade, "F", "a maxed node grades F even when healthy");
        assert!(g.factors.cpu.unwrap() < 20);
    }

    #[tokio::test]
    async fn drop_into_f_emits_a_debounced_chat_alert() {
        let tmp = tempfile::tempdir().unwrap();
        let bus = tempfile::tempdir().unwrap();
        // First a healthy sample, then a sustained maxed-out (F) sample.
        let healthy = signals(Some((0.0, 8)), Some(95.0), Some(95.0));
        let maxed = signals(Some((16.0, 8)), Some(1.0), Some(1.0));
        let mut w = NodeGradeWorker::new("me".into(), tmp.path().to_path_buf(), 0, None)
            .with_bus_root(Some(bus.path().to_path_buf()))
            .with_sampler(Box::new(FakeSampler::new(vec![healthy, maxed])));
        let persist = Persist::open(bus.path().to_path_buf()).unwrap();
        // Tick 1: healthy → grade file written, no alert.
        let g1 = w.cycle();
        w.publish_and_alert(&g1, Some(&persist), Instant::now());
        assert!(g1.grade == "A" || g1.grade == "B");
        assert!(
            persist.list_since(NOTIFY_TOPIC, None).unwrap().is_empty(),
            "no alert while healthy"
        );
        // Ticks into the maxed state (smoothing needs the window to fall to F).
        for _ in 0..SMOOTH_WINDOW {
            let g = w.cycle();
            w.publish_and_alert(&g, Some(&persist), Instant::now());
        }
        let alerts = persist.list_since(NOTIFY_TOPIC, None).unwrap();
        assert_eq!(alerts.len(), 1, "exactly one debounced drop alert");
        let body = alerts[0].body.as_ref().unwrap();
        let v: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(v["severity"], "critical", "an F drop is critical");
        assert_eq!(v["source"], "node-grade");
        assert_eq!(v["grade"], "F");
        // The grade file mirrors the drop for peers to read.
        assert_eq!(read_grades(tmp.path())[0].grade, "F");
    }

    #[tokio::test]
    async fn tick_loop_exits_promptly_on_shutdown() {
        let tmp = tempfile::tempdir().unwrap();
        let healthy = signals(Some((0.0, 8)), Some(90.0), Some(90.0));
        let mut w = NodeGradeWorker::new("node".into(), tmp.path().to_path_buf(), 0, None)
            .with_bus_root(None)
            .with_sampler(Box::new(FakeSampler::new(vec![healthy])))
            .with_poll(Duration::from_millis(10));
        let (tx, rx) = tokio::sync::watch::channel(false);
        let token = ShutdownToken::from_receiver(rx);
        let handle = tokio::spawn(async move { w.run(token).await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        tx.send(true).expect("signal shutdown");
        let joined = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(joined.is_ok(), "worker must exit promptly on shutdown");
        assert!(joined.unwrap().expect("join").is_ok());
        // It published at least one grade row.
        assert_eq!(read_grades(tmp.path()).len(), 1);
    }
}
