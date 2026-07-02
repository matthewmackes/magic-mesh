//! E12-10 "adaptive codec" — link-quality estimation + the quality ladder.
//!
//! A VDI session over the mesh must stay usable on a weak link: when the path
//! to the desktop degrades (Wi-Fi, a congested lighthouse hop, a WAN peer),
//! the session steps down to a lighter encoding tier, and when the link
//! recovers it steps back up — without ever flapping. This module is the
//! protocol-neutral core of that behaviour:
//!
//! * [`LinkEstimator`] — rolling estimates of RTT (TCP-style smoothed EWMA),
//!   loss/stall events, and effective frame throughput. Every observation
//!   carries a caller-supplied millisecond timestamp — the injectable
//!   probe/clock seam — so nothing here reads a real clock and the whole model
//!   is deterministic and unit-tested headless.
//! * [`LinkEstimate::grade`] — collapse an estimate to a [`LinkGrade`] against
//!   [`LinkThresholds`].
//! * [`QualityLadder`] — the ordered [`QualityTier`]s with hysteresis: degrade
//!   *fast* (a few consecutive bad samples) and upgrade *slowly* (a sustained
//!   good period), so a marginal link settles on a tier instead of
//!   oscillating.
//!
//! How a tier maps onto protocol knobs — and whether a change applies live or
//! only on reconnect ([`TierApplication`]) — is the per-protocol layer in
//! [`crate::tier`] and the session.
//!
//! **Twin-module note:** this file is deliberately kept byte-identical with
//! its twin in the sibling VDI backend crate (`mde-vdi-rdp` ⇄ `mde-vdi-vnc`).
//! The two backends are independent by design (the VNC crate carries no
//! external protocol dependency, and the E12-10 slice deliberately adds no
//! shared crate for this shape), so the shared *shape* lives as a mirrored
//! module — diff the twins when editing; they must not drift.

use std::collections::VecDeque;

/// Milliseconds of history the estimator keeps by default.
pub const DEFAULT_WINDOW_MS: u64 = 10_000;

/// An RTT sample larger than this is clamped before entering the EWMA — a
/// multi-minute "round trip" is a dead link, not a latency signal, and the
/// clamp keeps the fixed-point arithmetic overflow-free.
pub const RTT_CLAMP_MS: u32 = 60_000;

/// Rolling link-quality estimates fed through an injectable probe/clock seam.
///
/// The caller (the live wire pump, or a unit test) stamps every observation
/// with a monotonic millisecond timestamp; the estimator never reads a real
/// clock, so its behaviour is fully deterministic.
#[derive(Clone, Debug)]
pub struct LinkEstimator {
    /// History horizon for stall + throughput accounting.
    window_ms: u64,
    /// TCP-style smoothed RTT, stored ×8 (integer fixed point, α = 1/8).
    srtt_x8: Option<u32>,
    /// Timestamps of loss/stall events, pruned to the window.
    stalls: VecDeque<u64>,
    /// `(at_ms, bytes)` of decoded frame payloads, pruned to the window.
    frames: VecDeque<(u64, u64)>,
    /// Whether any frame was ever recorded — distinguishes "no data yet"
    /// (`None` throughput) from an idle-but-alive session (`Some(0)`).
    seen_frame: bool,
}

impl LinkEstimator {
    /// An estimator with the [`DEFAULT_WINDOW_MS`] history window.
    #[must_use]
    pub const fn new() -> Self {
        Self::with_window(DEFAULT_WINDOW_MS)
    }

    /// An estimator keeping `window_ms` of history (clamped to at least 1 ms
    /// so the throughput division is always defined).
    #[must_use]
    pub const fn with_window(window_ms: u64) -> Self {
        let window_ms = if window_ms == 0 { 1 } else { window_ms };
        Self {
            window_ms,
            srtt_x8: None,
            stalls: VecDeque::new(),
            frames: VecDeque::new(),
            seen_frame: false,
        }
    }

    /// Feed one measured round trip. Uses the classic TCP `SRTT` smoothing
    /// (`srtt += (sample - srtt) / 8`) in integer fixed point, so a single
    /// outlier nudges the estimate instead of owning it.
    pub fn record_rtt(&mut self, rtt_ms: u32) {
        let sample = rtt_ms.min(RTT_CLAMP_MS);
        self.srtt_x8 = Some(self.srtt_x8.map_or(sample * 8, |v| v - v / 8 + sample));
    }

    /// Feed one loss/stall event (a read timeout, a forced resend, a dropped
    /// or aborted frame) at `now_ms` on the caller's clock.
    pub fn record_stall(&mut self, now_ms: u64) {
        self.stalls.push_back(now_ms);
        self.prune(now_ms);
    }

    /// Feed the payload size of one decoded frame/update at `now_ms` — the
    /// effective (post-wire, pre-render) frame throughput signal.
    pub fn record_frame(&mut self, now_ms: u64, bytes: usize) {
        self.seen_frame = true;
        self.frames
            .push_back((now_ms, u64::try_from(bytes).unwrap_or(u64::MAX)));
        self.prune(now_ms);
    }

    /// Drop history older than the window so memory stays bounded.
    fn prune(&mut self, now_ms: u64) {
        let horizon = now_ms.saturating_sub(self.window_ms);
        while self.stalls.front().is_some_and(|&at| at < horizon) {
            self.stalls.pop_front();
        }
        while self.frames.front().is_some_and(|&(at, _)| at < horizon) {
            self.frames.pop_front();
        }
    }

    /// The rolling estimate as of `now_ms`. Pure with respect to the fed
    /// samples: the same feed and the same `now_ms` always yield the same
    /// estimate.
    #[must_use]
    pub fn estimate(&self, now_ms: u64) -> LinkEstimate {
        let horizon = now_ms.saturating_sub(self.window_ms);
        let stalls = self.stalls.iter().filter(|&&at| at >= horizon).count();
        let bytes = self
            .frames
            .iter()
            .filter(|&&(at, _)| at >= horizon)
            .fold(0_u64, |acc, &(_, b)| acc.saturating_add(b));
        LinkEstimate {
            rtt_ms: self.srtt_x8.map(|v| (v + 4) / 8),
            stalls_in_window: u32::try_from(stalls).unwrap_or(u32::MAX),
            throughput_bps: self
                .seen_frame
                .then(|| bytes.saturating_mul(8_000) / self.window_ms),
            window_ms: self.window_ms,
        }
    }
}

impl Default for LinkEstimator {
    fn default() -> Self {
        Self::new()
    }
}

/// A point-in-time snapshot of the rolling link estimates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinkEstimate {
    /// Smoothed round-trip estimate; `None` until the first RTT probe lands.
    pub rtt_ms: Option<u32>,
    /// Loss/stall events observed inside the window.
    pub stalls_in_window: u32,
    /// Effective decoded-frame throughput over the window, in bits/s. `None`
    /// until the first frame is recorded; `Some(0)` for an idle-but-alive
    /// session. Throughput is reported (HUD / diagnostics) but does **not**
    /// gate [`LinkEstimate::grade`]: a still desktop on a perfect link also
    /// decodes zero bytes, so low throughput alone is demand, not damage.
    pub throughput_bps: Option<u64>,
    /// The estimator window the counts above cover.
    pub window_ms: u64,
}

impl LinkEstimate {
    /// Collapse the estimate to a [`LinkGrade`] against `thresholds`.
    ///
    /// Stalls are the strongest signal (a stall is *observed* damage); RTT
    /// separates good from degraded. An unknown RTT (no probe yet) grades
    /// [`LinkGrade::Degraded`], so the ladder holds position rather than react
    /// to missing data.
    #[must_use]
    pub const fn grade(&self, thresholds: &LinkThresholds) -> LinkGrade {
        if self.stalls_in_window >= thresholds.bad_stalls {
            return LinkGrade::Bad;
        }
        match self.rtt_ms {
            Some(rtt) if rtt >= thresholds.bad_rtt_ms => LinkGrade::Bad,
            Some(rtt) if rtt <= thresholds.good_rtt_ms && self.stalls_in_window == 0 => {
                LinkGrade::Good
            }
            _ => LinkGrade::Degraded,
        }
    }
}

/// Grade cut-offs for [`LinkEstimate::grade`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinkThresholds {
    /// RTT at or below this — with zero stalls — grades [`LinkGrade::Good`].
    pub good_rtt_ms: u32,
    /// RTT at or above this grades [`LinkGrade::Bad`].
    pub bad_rtt_ms: u32,
    /// This many stalls inside the window grade [`LinkGrade::Bad`] regardless
    /// of RTT.
    pub bad_stalls: u32,
}

impl Default for LinkThresholds {
    /// Defaults tuned for an interactive desktop over the Nebula overlay:
    /// ≤ 80 ms feels local, ≥ 250 ms is visibly laggy, and two stalls inside
    /// one window mean frames are actually being lost.
    fn default() -> Self {
        Self {
            good_rtt_ms: 80,
            bad_rtt_ms: 250,
            bad_stalls: 2,
        }
    }
}

/// One graded link observation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkGrade {
    /// The link comfortably carries the current tier.
    Good,
    /// Marginal / unknown — hold the current tier.
    Degraded,
    /// The link is visibly failing the current tier.
    Bad,
}

/// The ordered encoding tiers, richest first. What each tier concretely maps
/// to on the wire is the per-protocol [`crate::tier`] layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum QualityTier {
    /// Full colour at the full update rate.
    Full,
    /// Reduced colour depth, mild compression.
    Reduced,
    /// Aggressive compression on top of reduced colour.
    Compressed,
    /// The lightest encoding the protocol can express.
    Minimal,
}

impl QualityTier {
    /// Every tier, richest first.
    pub const ALL: [Self; 4] = [Self::Full, Self::Reduced, Self::Compressed, Self::Minimal];

    /// Position in the ladder: 0 = richest … 3 = lightest (HUD ordering).
    #[must_use]
    pub const fn rank(self) -> u8 {
        match self {
            Self::Full => 0,
            Self::Reduced => 1,
            Self::Compressed => 2,
            Self::Minimal => 3,
        }
    }

    /// One step lighter, or `None` at the floor.
    #[must_use]
    pub const fn degraded(self) -> Option<Self> {
        match self {
            Self::Full => Some(Self::Reduced),
            Self::Reduced => Some(Self::Compressed),
            Self::Compressed => Some(Self::Minimal),
            Self::Minimal => None,
        }
    }

    /// One step richer, or `None` at the ceiling.
    #[must_use]
    pub const fn upgraded(self) -> Option<Self> {
        match self {
            Self::Full => None,
            Self::Reduced => Some(Self::Full),
            Self::Compressed => Some(Self::Reduced),
            Self::Minimal => Some(Self::Compressed),
        }
    }

    /// Stable lower-case label (logs / HUD).
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Reduced => "reduced",
            Self::Compressed => "compressed",
            Self::Minimal => "minimal",
        }
    }
}

/// Auto-adaptation vs an operator-pinned tier.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum QualityMode {
    /// The ladder drives the tier from the link samples.
    #[default]
    Auto,
    /// The operator pinned a tier; auto-stepping is suspended.
    Pinned(QualityTier),
}

/// A tier transition, for logging and for the caller to act on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TierChange {
    /// The tier stepped away from.
    pub from: QualityTier,
    /// The tier stepped onto.
    pub to: QualityTier,
    /// When the step happened, on the caller's clock.
    pub at_ms: u64,
}

impl TierChange {
    /// Whether this step went to a lighter tier.
    #[must_use]
    pub const fn is_degrade(&self) -> bool {
        self.to.rank() > self.from.rank()
    }
}

/// Whether a tier change takes effect live or is gated on a reconnect.
///
/// A backend must never silently accept a switch it cannot perform: when the
/// underlying protocol stack only exposes an encoding knob at connect time,
/// the tier API says so through this type and the session keeps reporting the
/// gap (e.g. `needs_reconnect`) until the caller reconnects with the new
/// settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TierApplication {
    /// The change applies mid-session, over the live connection.
    Live,
    /// The change can only apply on the next (re)connect.
    OnReconnect,
}

/// Hysteresis tuning for the [`QualityLadder`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LadderConfig {
    /// Consecutive [`LinkGrade::Bad`] samples that trigger one step down
    /// (treated as at least 1). Degrading is *fast*: with a few-second sample
    /// cadence a failing link sheds weight within seconds.
    pub degrade_after_bad: u32,
    /// Continuous [`LinkGrade::Good`] milliseconds required for one step up.
    /// Upgrading is *slow*: the link must prove itself for a sustained period,
    /// and the clock restarts after every step, so recovery never flaps.
    pub upgrade_after_good_ms: u64,
}

impl Default for LadderConfig {
    fn default() -> Self {
        Self {
            degrade_after_bad: 3,
            upgrade_after_good_ms: 15_000,
        }
    }
}

/// The quality ladder: current tier + the hysteresis state machine.
///
/// Feed it one [`LinkGrade`] per sampling interval via
/// [`QualityLadder::observe`]; it returns a [`TierChange`] when (and only
/// when) the tier steps.
#[derive(Clone, Debug)]
pub struct QualityLadder {
    config: LadderConfig,
    tier: QualityTier,
    /// Consecutive bad samples seen since the last non-bad sample or step.
    bad_streak: u32,
    /// Start of the current uninterrupted run of good samples.
    good_since_ms: Option<u64>,
}

impl QualityLadder {
    /// A ladder starting at `initial` with `config` hysteresis.
    #[must_use]
    pub const fn new(initial: QualityTier, config: LadderConfig) -> Self {
        Self {
            config,
            tier: initial,
            bad_streak: 0,
            good_since_ms: None,
        }
    }

    /// The current tier.
    #[must_use]
    pub const fn tier(&self) -> QualityTier {
        self.tier
    }

    /// Jump the ladder to `tier` and clear the hysteresis streaks — used when
    /// control returns from a pinned tier to auto, so auto resumes from where
    /// the operator left it instead of replaying stale state.
    pub const fn reset_to(&mut self, tier: QualityTier) {
        self.tier = tier;
        self.bad_streak = 0;
        self.good_since_ms = None;
    }

    /// Feed one graded sample at `now_ms`; returns the step it caused, if any.
    ///
    /// * [`LinkGrade::Bad`] — breaks any good run; after
    ///   [`LadderConfig::degrade_after_bad`] consecutive bad samples the tier
    ///   steps down and the streak restarts (a continued outage keeps
    ///   stepping, one threshold's worth of samples per step).
    /// * [`LinkGrade::Degraded`] — holds position and breaks both streaks:
    ///   not bad enough to shed weight, not good enough to earn it back.
    /// * [`LinkGrade::Good`] — after [`LadderConfig::upgrade_after_good_ms`]
    ///   of uninterrupted good the tier steps up and the clock restarts, so
    ///   each further step needs another full proving period.
    pub fn observe(&mut self, now_ms: u64, grade: LinkGrade) -> Option<TierChange> {
        match grade {
            LinkGrade::Bad => {
                self.good_since_ms = None;
                self.bad_streak = self.bad_streak.saturating_add(1);
                if self.bad_streak >= self.config.degrade_after_bad.max(1) {
                    self.bad_streak = 0;
                    return self.step_to(self.tier.degraded(), now_ms);
                }
                None
            }
            LinkGrade::Degraded => {
                self.bad_streak = 0;
                self.good_since_ms = None;
                None
            }
            LinkGrade::Good => {
                self.bad_streak = 0;
                let since = *self.good_since_ms.get_or_insert(now_ms);
                if now_ms.saturating_sub(since) >= self.config.upgrade_after_good_ms {
                    // Restart the proving clock whether or not a richer tier
                    // exists, so the arithmetic never overflows a long run.
                    self.good_since_ms = Some(now_ms);
                    return self.step_to(self.tier.upgraded(), now_ms);
                }
                None
            }
        }
    }

    /// Move onto `next` (when there is a rung to move to) and report the step.
    fn step_to(&mut self, next: Option<QualityTier>, now_ms: u64) -> Option<TierChange> {
        let to = next?;
        let change = TierChange {
            from: self.tier,
            to,
            at_ms: now_ms,
        };
        self.tier = to;
        Some(change)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        LadderConfig, LinkEstimator, LinkGrade, LinkThresholds, QualityLadder, QualityTier,
        RTT_CLAMP_MS,
    };

    // ── estimator: RTT ───────────────────────────────────────────────────────

    #[test]
    fn first_rtt_sample_is_the_estimate() {
        let mut e = LinkEstimator::new();
        assert_eq!(e.estimate(0).rtt_ms, None, "no probe yet");
        e.record_rtt(100);
        assert_eq!(e.estimate(0).rtt_ms, Some(100));
    }

    #[test]
    fn rtt_smooths_with_one_eighth_gain() {
        let mut e = LinkEstimator::new();
        e.record_rtt(100);
        e.record_rtt(200);
        // srtt_x8: 800 -> 800 - 100 + 200 = 900; rounded 904/8 = 113.
        assert_eq!(e.estimate(0).rtt_ms, Some(113));
        // Sustained 200s converge on 200.
        for _ in 0..64 {
            e.record_rtt(200);
        }
        assert_eq!(e.estimate(0).rtt_ms, Some(200));
    }

    #[test]
    fn absurd_rtt_is_clamped() {
        let mut e = LinkEstimator::new();
        e.record_rtt(u32::MAX);
        assert_eq!(e.estimate(0).rtt_ms, Some(RTT_CLAMP_MS));
    }

    // ── estimator: stalls + throughput windows ──────────────────────────────

    #[test]
    fn stalls_age_out_of_the_window() {
        let mut e = LinkEstimator::with_window(10_000);
        e.record_stall(1_000);
        e.record_stall(2_000);
        assert_eq!(e.estimate(2_000).stalls_in_window, 2);
        // At t=11.5s the first stall (t=1s) is out, the second (t=2s) still in.
        assert_eq!(e.estimate(11_500).stalls_in_window, 1);
        assert_eq!(e.estimate(30_000).stalls_in_window, 0);
    }

    #[test]
    fn throughput_is_bits_over_the_window() {
        let mut e = LinkEstimator::with_window(10_000);
        assert_eq!(e.estimate(0).throughput_bps, None, "no frame yet");
        e.record_frame(1_000, 10_000);
        e.record_frame(2_000, 2_500);
        // 12_500 bytes * 8 bits * 1000 / 10_000 ms = 10_000 bps.
        assert_eq!(e.estimate(2_000).throughput_bps, Some(10_000));
    }

    #[test]
    fn idle_after_frames_reads_zero_not_unknown() {
        let mut e = LinkEstimator::with_window(10_000);
        e.record_frame(0, 4_000);
        assert_eq!(e.estimate(60_000).throughput_bps, Some(0), "idle, not dead");
    }

    // ── grading ──────────────────────────────────────────────────────────────

    #[test]
    fn grade_covers_every_branch() {
        let t = LinkThresholds::default();
        let base = LinkEstimator::new().estimate(0);
        // Unknown RTT holds position.
        assert_eq!(base.grade(&t), LinkGrade::Degraded);

        let mut e = LinkEstimator::new();
        e.record_rtt(40);
        assert_eq!(e.estimate(0).grade(&t), LinkGrade::Good);

        let mut e = LinkEstimator::new();
        e.record_rtt(150);
        assert_eq!(e.estimate(0).grade(&t), LinkGrade::Degraded, "mid RTT");

        let mut e = LinkEstimator::new();
        e.record_rtt(400);
        assert_eq!(e.estimate(0).grade(&t), LinkGrade::Bad, "slow RTT");

        // Stalls trump a good RTT: one stall spoils Good, two grade Bad.
        let mut e = LinkEstimator::new();
        e.record_rtt(40);
        e.record_stall(10);
        assert_eq!(e.estimate(10).grade(&t), LinkGrade::Degraded);
        e.record_stall(20);
        assert_eq!(e.estimate(20).grade(&t), LinkGrade::Bad);
    }

    // ── ladder hysteresis ────────────────────────────────────────────────────

    fn ladder() -> QualityLadder {
        QualityLadder::new(QualityTier::Full, LadderConfig::default())
    }

    #[test]
    fn degrades_after_consecutive_bad_not_before() {
        let mut l = ladder();
        assert!(l.observe(0, LinkGrade::Bad).is_none());
        assert!(l.observe(1_000, LinkGrade::Bad).is_none());
        let change = l.observe(2_000, LinkGrade::Bad).expect("third bad steps");
        assert_eq!(change.from, QualityTier::Full);
        assert_eq!(change.to, QualityTier::Reduced);
        assert!(change.is_degrade());
        assert_eq!(l.tier(), QualityTier::Reduced);
    }

    #[test]
    fn a_good_sample_breaks_the_bad_streak() {
        let mut l = ladder();
        assert!(l.observe(0, LinkGrade::Bad).is_none());
        assert!(l.observe(1_000, LinkGrade::Bad).is_none());
        assert!(l.observe(2_000, LinkGrade::Good).is_none());
        // Streak restarted: two more bads are not enough.
        assert!(l.observe(3_000, LinkGrade::Bad).is_none());
        assert!(l.observe(4_000, LinkGrade::Bad).is_none());
        assert_eq!(l.tier(), QualityTier::Full, "no flap");
    }

    #[test]
    fn upgrades_only_after_sustained_good() {
        let mut l = QualityLadder::new(QualityTier::Compressed, LadderConfig::default());
        assert!(l.observe(0, LinkGrade::Good).is_none());
        assert!(l.observe(10_000, LinkGrade::Good).is_none(), "not yet 15s");
        let change = l.observe(15_000, LinkGrade::Good).expect("15s of good");
        assert_eq!(change.to, QualityTier::Reduced);
        assert!(!change.is_degrade());
        // The proving clock restarted: the next step needs another full run.
        assert!(l.observe(20_000, LinkGrade::Good).is_none());
        assert!(l.observe(29_000, LinkGrade::Good).is_none());
        let change = l.observe(30_000, LinkGrade::Good).expect("another 15s");
        assert_eq!(change.to, QualityTier::Full);
    }

    #[test]
    fn degraded_grade_holds_position_and_breaks_the_good_run() {
        let mut l = QualityLadder::new(QualityTier::Reduced, LadderConfig::default());
        assert!(l.observe(0, LinkGrade::Good).is_none());
        assert!(l.observe(10_000, LinkGrade::Degraded).is_none());
        // The good run restarted at 11s, so 25s total is not enough…
        assert!(l.observe(11_000, LinkGrade::Good).is_none());
        assert!(l.observe(25_000, LinkGrade::Good).is_none());
        // …but 15s measured from 11s is.
        let change = l.observe(26_000, LinkGrade::Good).expect("sustained good");
        assert_eq!(change.to, QualityTier::Full);
    }

    #[test]
    fn alternating_samples_never_flap() {
        let mut l = ladder();
        for i in 0..100_u64 {
            let grade = if i % 2 == 0 {
                LinkGrade::Bad
            } else {
                LinkGrade::Good
            };
            assert!(l.observe(i * 1_000, grade).is_none(), "sample {i} stepped");
        }
        assert_eq!(l.tier(), QualityTier::Full);
    }

    #[test]
    fn ladder_stops_at_floor_and_ceiling() {
        let mut l = QualityLadder::new(QualityTier::Minimal, LadderConfig::default());
        for i in 0..20 {
            assert!(l.observe(i * 500, LinkGrade::Bad).is_none(), "below floor");
        }
        assert_eq!(l.tier(), QualityTier::Minimal);

        let mut l = ladder();
        for i in 0..20 {
            assert!(
                l.observe(i * 20_000, LinkGrade::Good).is_none(),
                "above ceiling"
            );
        }
        assert_eq!(l.tier(), QualityTier::Full);
    }

    #[test]
    fn continued_outage_keeps_stepping_down() {
        let mut l = ladder();
        let mut steps = Vec::new();
        for i in 0..9_u64 {
            if let Some(c) = l.observe(i * 1_000, LinkGrade::Bad) {
                steps.push(c.to);
            }
        }
        assert_eq!(
            steps,
            vec![
                QualityTier::Reduced,
                QualityTier::Compressed,
                QualityTier::Minimal
            ]
        );
    }

    #[test]
    fn reset_to_clears_streaks() {
        let mut l = ladder();
        assert!(l.observe(0, LinkGrade::Bad).is_none());
        assert!(l.observe(1_000, LinkGrade::Bad).is_none());
        l.reset_to(QualityTier::Compressed);
        assert_eq!(l.tier(), QualityTier::Compressed);
        // The pre-reset bad streak is gone: two more bads do not step.
        assert!(l.observe(2_000, LinkGrade::Bad).is_none());
        assert!(l.observe(3_000, LinkGrade::Bad).is_none());
        assert_eq!(l.tier(), QualityTier::Compressed);
    }

    // ── tier helpers ─────────────────────────────────────────────────────────

    #[test]
    fn tier_steps_chain_across_the_whole_ladder() {
        let mut down = Some(QualityTier::Full);
        let mut seen = Vec::new();
        while let Some(t) = down {
            seen.push(t);
            down = t.degraded();
        }
        assert_eq!(seen, QualityTier::ALL.to_vec());
        // And back up.
        let mut up = Some(QualityTier::Minimal);
        let mut count = 0;
        while let Some(t) = up {
            count += 1;
            up = t.upgraded();
        }
        assert_eq!(count, 4);
        assert_eq!(QualityTier::Full.rank(), 0);
        assert_eq!(QualityTier::Minimal.rank(), 3);
        assert_eq!(QualityTier::Compressed.label(), "compressed");
    }
}
