//! NOTIFY-HUB-2 — the Notification Hub's motion + same-source stacking.
//!
//! Two self-contained pieces, both pure + unit-tested, driven by the Hub's
//! `subscription` tick (no toolkit dep in here — the bin's `view` reads these
//! and applies the offsets/tints to its themed widgets):
//!
//!   * [`collapse_stacks`] — same-source **and** same-title repeats collapse to
//!     one card carrying a repeat `count`, expandable on demand. This is the
//!     "stack" axis the design asks for (`group_items` already buckets by
//!     `Source`; this folds the duplicate *runs* inside a bucket).
//!   * [`HubAnim`] — the per-item motion state machine: a freshly-arrived item
//!     **slides in from the right** and **blinks 2× in its severity colour**,
//!     while the items already on screen **slide down** to make room. Built on
//!     the shared `mde_theme::animation` tweens + the Carbon `mde_theme::motion`
//!     duration grid (no scattered timing literals — §4); a later follow-up
//!     (MOTION-INFRA-2) consolidates onto the shared shell helpers.
//!
//! Self-contained so it lands without depending on a sibling agent's unmerged
//! `mde-theme` motion-helper branch (different files → no conflict).

use std::collections::HashMap;
use std::time::Instant;

use mde_notify::{AlertItem, Severity, Source};
use mde_theme::animation::{ease, lerp_f32, Tween};
use mde_theme::motion::{Easing, DURATION_MODERATE_02};

/// Slide-in travel: a new card starts this many px to the **right** of its rest
/// position and eases in. Component dimension (the slide-out is 420 px wide), so
/// it's a local constant, not a density-scaled metric.
pub const SLIDE_IN_PX: f32 = 36.0;

/// Slide-down travel: items already on screen drop by this many px as the new
/// card opens its row above them, then settle.
pub const SLIDE_DOWN_PX: f32 = 28.0;

/// Number of severity-colour blinks on a freshly-arrived card (design: "blinks
/// 2× in its severity colour").
pub const BLINK_COUNT: u32 = 2;

/// Peak alpha of the severity-colour blink wash over the new card's background.
/// A wash, not a flood — the row text stays legible through the pulse.
pub const BLINK_PEAK_ALPHA: f32 = 0.45;

/// One blink (up+down) lasts a Carbon `moderate-02` (240 ms); the slide-in runs
/// for the same single beat. The whole new-item animation is therefore
/// `BLINK_COUNT` beats long, and the slide finishes within the first.
fn blink_beat() -> std::time::Duration {
    DURATION_MODERATE_02
}

/// A collapsed stack of same-source + same-title notifications: the newest item
/// stands in for the run, with `count` total repeats. `count == 1` is a plain
/// single card; `count > 1` renders as one card with a "×N" badge, expandable
/// to reveal the individual repeats.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stack {
    /// The representative (newest) item — drives the card's title/severity/age.
    pub head: AlertItem,
    /// Every item in the stack, newest-first (`head` is `items[0]`). Shown when
    /// the stack is expanded.
    pub items: Vec<AlertItem>,
    /// Stable key (source label + title) — the expand/collapse toggle id and
    /// the dedup axis.
    pub key: String,
}

impl Stack {
    /// Repeat count — `1` for a lone notification, `>1` for a collapsed run.
    #[must_use]
    pub fn count(&self) -> usize {
        self.items.len()
    }

    /// `true` when this stack folds more than one notification (renders the
    /// count badge + is expandable).
    #[must_use]
    pub fn is_stacked(&self) -> bool {
        self.items.len() > 1
    }
}

/// The stable stack key for an item: its source group + its title. Same source
/// **and** same title ⇒ the same stack (e.g. a flapping peer re-emitting
/// "offline" collapses to one card with a count instead of N identical rows).
#[must_use]
pub fn stack_key(source: &Source, title: &str) -> String {
    format!("{}\u{1f}{title}", source.label())
}

/// Collapse a source group's items (already newest-first, as `group_items`
/// yields) into [`Stack`]s. Consecutive **and** non-consecutive repeats of the
/// same `(source, title)` fold into one stack; first-seen order of the *heads*
/// is preserved so the newest distinct alert stays on top. Pure + testable.
#[must_use]
pub fn collapse_stacks(items: &[AlertItem]) -> Vec<Stack> {
    let mut order: Vec<String> = Vec::new();
    let mut by_key: HashMap<String, Stack> = HashMap::new();
    for it in items {
        let key = stack_key(&it.source, &it.title);
        if let Some(stack) = by_key.get_mut(&key) {
            stack.items.push(it.clone());
        } else {
            order.push(key.clone());
            by_key.insert(
                key.clone(),
                Stack {
                    head: it.clone(),
                    items: vec![it.clone()],
                    key,
                },
            );
        }
    }
    order
        .into_iter()
        .filter_map(|k| by_key.remove(&k))
        .collect()
}

/// Per-card render motion at the current instant: how far the card is still
/// offset and how strongly the severity blink is washing its background.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CardMotion {
    /// Horizontal offset in px (positive = still to the right of rest, slides to 0).
    pub translate_x: f32,
    /// Vertical offset in px (positive = still below rest, slides up to 0) — the
    /// "existing items slide down then settle" motion.
    pub translate_y: f32,
    /// Severity-blink wash alpha `0.0..=BLINK_PEAK_ALPHA` over the card background.
    pub blink_alpha: f32,
}

impl CardMotion {
    /// A fully-settled card (no offset, no blink) — what every card resolves to
    /// once its animation completes (and what non-animating cards always get).
    pub const REST: Self = Self {
        translate_x: 0.0,
        translate_y: 0.0,
        blink_alpha: 0.0,
    };

    /// `true` when the card is at rest (nothing to draw differently).
    #[must_use]
    pub fn is_rest(self) -> bool {
        self.translate_x == 0.0 && self.translate_y == 0.0 && self.blink_alpha == 0.0
    }
}

/// One in-flight new-item animation: a slide-in tween (entrance easing) plus the
/// 2× severity blink, both started at the same instant.
#[derive(Debug, Clone, Copy)]
struct Entrance {
    /// Slide-in tween (px right → 0), one Carbon beat.
    slide: Tween,
    /// The whole entrance window (slide + `BLINK_COUNT` blink beats) — also the
    /// window during which the items below slide down then settle.
    window: Tween,
    reduce_motion: bool,
}

/// NOTIFY-HUB-2 — the Hub's motion state machine.
///
/// Tracks which item IDs are currently entering (slide-in + blink) and, while
/// any are, slides the rest of the list down. Tick-driven: the Hub advances it
/// from a single `iced::time::every` subscription that it only arms while
/// [`HubAnim::is_idle`] is `false` (no idle CPU).
#[derive(Debug, Default, Clone)]
pub struct HubAnim {
    /// item id → its in-flight entrance.
    entering: HashMap<String, Entrance>,
    reduce_motion: bool,
}

impl HubAnim {
    /// A fresh, idle state machine. `reduce_motion` collapses every animation to
    /// the Carbon ≤80 ms crossfade (no slide, blink reduced to a single short
    /// flash) via [`Tween::resolved`].
    #[must_use]
    pub fn new(reduce_motion: bool) -> Self {
        Self {
            entering: HashMap::new(),
            reduce_motion,
        }
    }

    /// Register freshly-arrived item ids, starting their slide-in + blink at
    /// `now`. Ids already animating are restarted (a re-fired repeat re-blinks).
    pub fn on_new_items<I, S>(&mut self, ids: I, now: Instant)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let slide = blink_beat();
        let window = blink_beat() * BLINK_COUNT;
        for id in ids {
            self.entering.insert(
                id.into(),
                Entrance {
                    slide: Tween::resolved(now, slide, self.reduce_motion),
                    window: Tween::resolved(now, window, self.reduce_motion),
                    reduce_motion: self.reduce_motion,
                },
            );
        }
    }

    /// `true` when no item is currently animating — the Hub stops ticking
    /// (MOTION-PERF-1: zero idle wakeups).
    #[must_use]
    pub fn is_idle(&self, now: Instant) -> bool {
        self.entering.values().all(|e| e.window.is_complete(now))
    }

    /// Drop every finished entrance (call once per tick). Returns the count
    /// still in flight so the subscription can stop when it reaches 0.
    pub fn gc(&mut self, now: Instant) -> usize {
        self.entering.retain(|_, e| !e.window.is_complete(now));
        self.entering.len()
    }

    /// `true` when at least one item is mid-entrance — i.e. the items below it
    /// should be sliding down.
    #[must_use]
    fn any_entering(&self, now: Instant) -> bool {
        self.entering.values().any(|e| !e.window.is_complete(now))
    }

    /// The render motion for the card with id `id` at `now`. A card that is
    /// itself entering slides in from the right + blinks; a settled card slides
    /// **down** while any sibling above it is still entering, then rests.
    #[must_use]
    pub fn card_motion(&self, id: &str, now: Instant) -> CardMotion {
        if let Some(e) = self.entering.get(id) {
            return entrance_motion(e, now);
        }
        // Not entering itself: drop down to make room while a new card opens
        // above, then settle. The drop eases out over one beat keyed to the
        // freshest entrance still running.
        if self.any_entering(now) {
            if let Some(t) = self.slide_down_progress(now) {
                return CardMotion {
                    translate_y: lerp_f32(SLIDE_DOWN_PX, 0.0, ease(t, Easing::EaseOut)),
                    ..CardMotion::REST
                };
            }
        }
        CardMotion::REST
    }

    /// Linear progress `0..=1` of the slide-down, driven by the *freshest*
    /// in-flight entrance's first beat (the others below all settle together).
    fn slide_down_progress(&self, now: Instant) -> Option<f32> {
        self.entering
            .values()
            .filter(|e| !e.window.is_complete(now))
            .map(|e| e.slide.progress(now))
            .reduce(f32::min)
    }
}

/// The slide-in + blink motion for an entering card at `now`.
fn entrance_motion(e: &Entrance, now: Instant) -> CardMotion {
    let slide_t = ease(e.slide.progress(now), Easing::EaseOut);
    let translate_x = lerp_f32(SLIDE_IN_PX, 0.0, slide_t);
    let blink_alpha = blink_alpha_at(e.window.progress(now), e.reduce_motion);
    CardMotion {
        translate_x,
        translate_y: 0.0,
        blink_alpha,
    }
}

/// The severity-blink wash alpha at window progress `t` (`0..=1`). Full motion:
/// `BLINK_COUNT` clean triangle pulses 0 → peak → 0 across the window. Under
/// reduce-motion: a single short fade-out flash (no strobe — the a11y contract).
#[must_use]
pub fn blink_alpha_at(t: f32, reduce_motion: bool) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if reduce_motion {
        // One gentle fade-out flash, never a repeating strobe.
        return lerp_f32(BLINK_PEAK_ALPHA, 0.0, t);
    }
    // `BLINK_COUNT` triangle pulses: each cycle ramps 0→peak→0.
    let cycle = (t * BLINK_COUNT as f32).fract();
    let triangle = if cycle < 0.5 {
        cycle * 2.0
    } else {
        2.0 - cycle * 2.0
    };
    BLINK_PEAK_ALPHA * ease(triangle, Easing::EaseInOut)
}

/// Convenience: the severity colour the blink washes in, at the resolved alpha
/// (pure colour token math — no raw hex; the caller supplies the live palette).
#[must_use]
pub fn blink_tint(severity: Severity, palette: &mde_theme::Palette, alpha: f32) -> mde_theme::Rgba {
    mde_notify::severity_token(severity, palette).with_alpha(alpha.clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn item(id: &str, src: Source, sev: Severity, title: &str, ts: i64) -> AlertItem {
        AlertItem {
            id: id.into(),
            ts_unix_ms: ts,
            severity: sev,
            source: src,
            topic: "t".into(),
            host: None,
            title: title.into(),
            body: String::new(),
            read: false,
        }
    }

    // ── collapse / stack logic ──────────────────────────────────────────

    #[test]
    fn distinct_titles_do_not_collapse() {
        let items = vec![
            item("a", Source::System, Severity::Info, "disk low", 30),
            item("b", Source::System, Severity::Warning, "cpu hot", 20),
        ];
        let stacks = collapse_stacks(&items);
        assert_eq!(stacks.len(), 2);
        assert!(stacks.iter().all(|s| !s.is_stacked()));
        assert!(stacks.iter().all(|s| s.count() == 1));
    }

    #[test]
    fn same_source_same_title_repeats_collapse_with_a_count() {
        // The core acceptance: same-source repeats collapse to one card + a count.
        let items = vec![
            item("c", Source::Security, Severity::Critical, "csr denied", 30),
            item("b", Source::Security, Severity::Critical, "csr denied", 20),
            item("a", Source::Security, Severity::Critical, "csr denied", 10),
        ];
        let stacks = collapse_stacks(&items);
        assert_eq!(stacks.len(), 1, "three repeats fold to one card");
        assert_eq!(stacks[0].count(), 3);
        assert!(stacks[0].is_stacked());
        // The head is the newest (first in the newest-first input).
        assert_eq!(stacks[0].head.id, "c");
        // Every repeat is retained for the expanded view, newest-first.
        let ids: Vec<&str> = stacks[0].items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["c", "b", "a"]);
    }

    #[test]
    fn same_title_different_source_stays_separate() {
        // Stacking is keyed on source AND title — only true duplicates fold.
        let items = vec![
            item("a", Source::Security, Severity::Warning, "offline", 20),
            item(
                "b",
                Source::Peer("eagle".into()),
                Severity::Warning,
                "offline",
                10,
            ),
        ];
        let stacks = collapse_stacks(&items);
        assert_eq!(stacks.len(), 2, "different sources never merge");
    }

    #[test]
    fn non_adjacent_repeats_still_collapse_and_keep_head_order() {
        // A→B→A interleave: the two A's fold; B keeps its own card; head order
        // follows first appearance (A before B).
        let items = vec![
            item("a1", Source::System, Severity::Info, "ping", 30),
            item("b1", Source::System, Severity::Info, "pong", 20),
            item("a2", Source::System, Severity::Info, "ping", 10),
        ];
        let stacks = collapse_stacks(&items);
        assert_eq!(stacks.len(), 2);
        assert_eq!(stacks[0].key, stack_key(&Source::System, "ping"));
        assert_eq!(stacks[0].count(), 2);
        assert_eq!(stacks[1].key, stack_key(&Source::System, "pong"));
        assert_eq!(stacks[1].count(), 1);
    }

    #[test]
    fn empty_input_yields_no_stacks() {
        assert!(collapse_stacks(&[]).is_empty());
    }

    // ── motion state machine ────────────────────────────────────────────

    #[test]
    fn new_item_slides_in_from_the_right_and_settles() {
        let t0 = Instant::now();
        let mut anim = HubAnim::new(false);
        anim.on_new_items(["new"], t0);
        // At t0 the card is offset fully to the right.
        let m0 = anim.card_motion("new", t0);
        assert!(
            (m0.translate_x - SLIDE_IN_PX).abs() < 1e-3,
            "starts at full right offset, got {}",
            m0.translate_x
        );
        // Mid-slide it's partway in.
        let mid = t0 + blink_beat() / 2;
        let mm = anim.card_motion("new", mid);
        assert!(
            mm.translate_x > 0.0 && mm.translate_x < SLIDE_IN_PX,
            "interpolating, got {}",
            mm.translate_x
        );
        // After one beat the slide is done (blink may still run).
        let after_slide = t0 + blink_beat() + Duration::from_millis(1);
        assert!(
            anim.card_motion("new", after_slide).translate_x.abs() < 1e-3,
            "slide settles to 0 after one beat"
        );
    }

    #[test]
    fn new_item_blinks_exactly_twice_then_rests() {
        let t0 = Instant::now();
        let mut anim = HubAnim::new(false);
        anim.on_new_items(["new"], t0);
        let window = blink_beat() * BLINK_COUNT;
        // Sample the blink alpha across the window; count the rising edges past
        // a threshold — must equal BLINK_COUNT (two distinct pulses).
        let mut peaks = 0u32;
        let mut was_high = false;
        let steps = 200;
        for i in 0..=steps {
            let now = t0 + window.mul_f32(i as f32 / steps as f32);
            let a = anim.card_motion("new", now).blink_alpha;
            let high = a > BLINK_PEAK_ALPHA * 0.6;
            if high && !was_high {
                peaks += 1;
            }
            was_high = high;
            assert!(a <= BLINK_PEAK_ALPHA + 1e-3, "blink never exceeds peak");
        }
        assert_eq!(peaks, BLINK_COUNT, "exactly 2 severity blinks");
        // Past the window the card is fully at rest.
        let after = t0 + window + Duration::from_millis(1);
        assert!(anim.card_motion("new", after).is_rest());
        assert!(anim.is_idle(after));
    }

    #[test]
    fn existing_items_slide_down_while_a_new_one_enters_then_settle() {
        let t0 = Instant::now();
        let mut anim = HubAnim::new(false);
        anim.on_new_items(["new"], t0);
        // An item that is NOT entering is pushed down at the start of the entrance.
        let m0 = anim.card_motion("old", t0);
        assert!(
            (m0.translate_y - SLIDE_DOWN_PX).abs() < 1e-3,
            "existing card starts pushed down, got {}",
            m0.translate_y
        );
        assert_eq!(
            m0.translate_x, 0.0,
            "existing card never slides horizontally"
        );
        // It eases back up to rest within the first beat.
        let settled = t0 + blink_beat() + Duration::from_millis(1);
        assert!(anim.card_motion("old", settled).translate_y.abs() < 1e-3);
        // With nothing entering, an arbitrary card is always at rest.
        let after = t0 + blink_beat() * BLINK_COUNT + Duration::from_millis(1);
        assert!(anim.card_motion("old", after).is_rest());
    }

    #[test]
    fn gc_drops_finished_entrances_and_reports_inflight() {
        let t0 = Instant::now();
        let mut anim = HubAnim::new(false);
        anim.on_new_items(["a", "b"], t0);
        assert!(!anim.is_idle(t0));
        assert_eq!(anim.gc(t0), 2, "both still in flight");
        let done = t0 + blink_beat() * BLINK_COUNT + Duration::from_millis(1);
        assert_eq!(anim.gc(done), 0, "gc clears finished entrances");
        assert!(anim.is_idle(done));
    }

    #[test]
    fn idle_when_no_items_registered() {
        let anim = HubAnim::new(false);
        assert!(anim.is_idle(Instant::now()));
        // A never-registered id reads as fully at rest.
        assert!(anim.card_motion("ghost", Instant::now()).is_rest());
    }

    #[test]
    fn reduce_motion_collapses_to_a_short_single_flash() {
        let t0 = Instant::now();
        let mut anim = HubAnim::new(true);
        anim.on_new_items(["new"], t0);
        // Under reduce-motion the whole animation is capped to the ≤80 ms
        // crossfade, so it's settled well before a full Carbon beat.
        let capped = t0 + Duration::from_millis(80) + Duration::from_millis(1);
        assert!(
            anim.is_idle(capped),
            "reduce-motion entrance settles within the 80 ms cap"
        );
        assert!(anim.card_motion("new", capped).is_rest());
    }

    #[test]
    fn blink_alpha_is_bounded_and_zero_at_endpoints() {
        // Full motion: zero at both ends of the window, bounded by the peak.
        assert!(blink_alpha_at(0.0, false).abs() < 1e-3);
        assert!(blink_alpha_at(1.0, false).abs() < 1e-3);
        for i in 0..=20 {
            let a = blink_alpha_at(i as f32 / 20.0, false);
            assert!((0.0..=BLINK_PEAK_ALPHA + 1e-3).contains(&a));
        }
        // Reduce-motion: a monotone fade from peak to 0 (no strobe).
        assert!((blink_alpha_at(0.0, true) - BLINK_PEAK_ALPHA).abs() < 1e-3);
        assert!(blink_alpha_at(1.0, true).abs() < 1e-3);
        assert!(blink_alpha_at(0.5, true) < blink_alpha_at(0.25, true));
    }

    #[test]
    fn blink_tint_uses_the_severity_token_at_the_given_alpha() {
        let p = mde_theme::Palette::dark();
        let tint = blink_tint(Severity::Critical, &p, BLINK_PEAK_ALPHA);
        let base = mde_notify::severity_token(Severity::Critical, &p);
        assert_eq!((tint.r, tint.g, tint.b), (base.r, base.g, base.b));
        assert!((tint.a - BLINK_PEAK_ALPHA).abs() < 1e-6);
    }
}
