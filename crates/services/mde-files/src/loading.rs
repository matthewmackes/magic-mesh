//! BEAUT-FILES — perceived-performance state for the file/list view.
//!
//! Two jobs, both pure glue over `mde_theme`:
//!
//! 1. **Skeleton-first paint.** The instant a listing is navigated to, the view
//!    paints layout-matching greyed placeholder rows (a [`skeleton_rows`]) so the
//!    pane is never blank while the backend enumerates the directory. The grey
//!    *breathes* via [`mde_theme::animation::shimmer_alpha`] (and is a STATIC grey
//!    under reduce-motion — the a11y contract: motion is never the only cue).
//!
//! 2. **Stale-while-refreshing.** A *refresh* of a listing that already has
//!    content keeps the previous rows on screen — dimmed (not blanked) — and
//!    crossfades the fresh rows in when they land, instead of flashing empty.
//!
//! The async vocabulary is the shared [`mde_theme::LoadState`]
//! (`Loading`/`Refreshing { stale }`/`Loaded`); the dim factor is
//! [`LoadState::content_alpha`], the skeleton shimmer is
//! [`mde_theme::animation::shimmer_alpha`], and the fade-in of fresh content is
//! [`mde_theme::animation::fade_in`]. This module owns NONE of that math — it is
//! the glue (`AI_GOVERNANCE.md` §4/§7: reuse, don't reimplement).
//!
//! ## Follow-up (BEAUT-THEME)
//!
//! [`skeleton_rows`] is a deliberately *minimal local* skeleton so this unit
//! stays parallel-buildable with BEAUT-THEME. Once BEAUT-THEME lands a shared
//! `mde_theme::skeleton`, consolidate this onto it and delete the local widget.

use std::time::{Duration, Instant};

use cosmic::iced::widget::{column, container, Space};
use cosmic::iced::{Background, Border, Color, Length, Padding};
use cosmic::Element;
use mde_theme::motion::Motion;
use mde_theme::LoadState;

use crate::app::Message;
use crate::cosmic_compat::ContainerSty;
use crate::theme as t;

/// BEAUT-FILES — how long a fresh listing shows the skeleton before its rows are
/// considered "landed". A directory enumeration in this crate is synchronous, so
/// the window is short by design — it's the *perceived* first-paint buffer, not a
/// real network wait. Sourced from the Carbon `loading` activity preset's
/// duration so there's no scattered metric literal (§4).
fn loading_window() -> Duration {
    Motion::loading().duration
}

/// BEAUT-FILES — how long fresh rows crossfade in over the dimmed stale rows on a
/// refresh. The Carbon `refresh` preset duration (§4 single source).
fn refresh_window() -> Duration {
    Motion::refresh().duration
}

/// BEAUT-FILES — the perceived-performance load state of the active listing.
///
/// Drives the skeleton-first paint + the stale-while-refreshing dim/crossfade.
/// Cheap to clone (a [`LoadState`] + an `Instant`); the app holds one and
/// transitions it in `refresh_snapshot`.
#[derive(Debug, Clone, Copy)]
pub struct ListingLoad {
    /// The shared async state vocabulary (`Loading`/`Refreshing`/`Loaded`).
    state: LoadState,
    /// When the current `state` was entered — the origin the skeleton-shimmer
    /// phase + the crossfade progress are sampled against.
    origin: Instant,
}

impl Default for ListingLoad {
    fn default() -> Self {
        Self {
            state: LoadState::Idle,
            origin: Instant::now(),
        }
    }
}

impl ListingLoad {
    /// The current async [`LoadState`].
    #[must_use]
    pub fn state(self) -> LoadState {
        self.state
    }

    /// BEAUT-FILES — transition when the active listing *signature* changed
    /// (navigated to a new directory/view). `had_content` = whether the prior
    /// listing already had rows on screen.
    ///
    /// * New listing with **no** prior content → [`LoadState::Loading`]: paint the
    ///   skeleton (there's nothing to keep).
    /// * New listing **with** prior content → `Refreshing { stale: true }`: keep
    ///   the old rows dimmed and crossfade the new ones in (no blank flash).
    pub fn begin(&mut self, now: Instant, had_content: bool) {
        self.state = if had_content {
            LoadState::Refreshing { stale: true }
        } else {
            LoadState::Loading
        };
        self.origin = now;
    }

    /// BEAUT-FILES — transition when the *same* listing is re-listed (the periodic
    /// reconnect/refresh tick, or an explicit Refresh) and content is present:
    /// a background `Refreshing { stale: true }` so the rows dim + crossfade
    /// rather than blank. A no-op while a load is already in flight (don't reset
    /// the origin mid-animation).
    pub fn refresh_in_place(&mut self, now: Instant, has_content: bool) {
        if !has_content || self.state.is_busy() {
            return;
        }
        self.state = LoadState::Refreshing { stale: true };
        self.origin = now;
    }

    /// BEAUT-FILES — advance the state to [`LoadState::Loaded`] once the active
    /// window (loading or refresh) has elapsed, so a settled listing stops
    /// animating (MOTION-PERF-1: no idle wakeups). Returns the resolved state.
    pub fn settle(&mut self, now: Instant) -> LoadState {
        let window = match self.state {
            LoadState::Loading => loading_window(),
            LoadState::Refreshing { .. } => refresh_window(),
            other => return other,
        };
        if now.saturating_duration_since(self.origin) >= window {
            self.state = LoadState::Loaded;
        }
        self.state
    }

    /// BEAUT-FILES — is a load animation (skeleton shimmer or refresh crossfade)
    /// still in flight? The subscription gates its per-frame tick on this so the
    /// clock is armed only while the placeholder/crossfade is visible.
    #[must_use]
    pub fn is_animating(self, now: Instant) -> bool {
        let window = match self.state {
            LoadState::Loading => loading_window(),
            LoadState::Refreshing { .. } => refresh_window(),
            _ => return false,
        };
        now.saturating_duration_since(self.origin) < window
    }

    /// BEAUT-FILES — show the skeleton placeholder instead of the (empty) row list
    /// during a first load with no prior content.
    #[must_use]
    pub fn show_skeleton(self) -> bool {
        matches!(self.state, LoadState::Loading)
    }

    /// BEAUT-FILES — the skeleton-shimmer phase `0.0..=1.0` for [`skeleton_rows`],
    /// derived from the elapsed loading time over the Carbon `loading` period.
    #[must_use]
    pub fn skeleton_phase(self, now: Instant) -> f32 {
        let period = loading_window().as_secs_f32().max(f32::EPSILON);
        let elapsed = now.saturating_duration_since(self.origin).as_secs_f32();
        (elapsed % period) / period
    }

    /// BEAUT-FILES — the alpha to render kept-on-screen content at: full normally,
    /// dimmed while `Refreshing { stale: true }` (stale-while-revalidate). Pure
    /// pass-through to [`LoadState::content_alpha`] — the §4 single source.
    #[must_use]
    pub fn content_alpha(self) -> f32 {
        self.state.content_alpha()
    }

    /// BEAUT-FILES — the crossfade-in alpha `0.0..=1.0` for the *fresh* rows
    /// arriving over the dimmed stale ones during a refresh. `1.0` (no fade) when
    /// not refreshing. Under reduce-motion the fade is capped to ≤80 ms (via
    /// [`mde_theme::animation::fade_in`]), so fresh content snaps in.
    #[must_use]
    pub fn refresh_fade(self, now: Instant, reduce_motion: bool) -> f32 {
        if matches!(self.state, LoadState::Refreshing { .. }) {
            mde_theme::animation::fade_in(self.origin, now, reduce_motion).alpha
        } else {
            1.0
        }
    }
}

/// BEAUT-FILES — how many greyed placeholder bars the skeleton paints. A short
/// list that reads as "rows are coming" without implying an exact count.
pub const SKELETON_ROW_COUNT: usize = 6;

/// BEAUT-FILES — the placeholder bar height (px). Matches the 28 px list-row
/// height (`list_row`) minus the row's own vertical breathing room, so the
/// skeleton occupies the same vertical rhythm the real rows will.
const SKELETON_BAR_HEIGHT: f32 = 16.0;

/// BEAUT-FILES — vertical gap between placeholder bars (px). Mirrors the list
/// row divider rhythm so the skeleton lines up with the eventual rows.
const SKELETON_BAR_GAP: f32 = 12.0;

/// BEAUT-FILES — render `rows` greyed placeholder bars for a loading file list.
///
/// Each bar is a Carbon `text_muted`-tinted block whose alpha *breathes* via
/// [`mde_theme::animation::shimmer_alpha`] (STATIC mid-grey under reduce-motion —
/// no shimmer, the a11y contract). This is the **minimal local** skeleton; once
/// BEAUT-THEME ships a shared `mde_theme::skeleton` this consolidates onto it.
#[must_use]
pub fn skeleton_rows<'a>(rows: usize, phase: f32, reduce_motion: bool) -> Element<'a, Message> {
    let alpha = mde_theme::animation::shimmer_alpha(phase, reduce_motion);
    // Carbon Gray-50 helper text tone (`t::FG_FAINT`) at the shimmer alpha — a
    // token, not a raw colour (§4).
    let bar_color = Color {
        a: alpha,
        ..t::FG_FAINT
    };
    let mut col = column![].spacing(SKELETON_BAR_GAP);
    for _ in 0..rows {
        col = col.push(
            container(
                Space::new()
                    .width(Length::Fill)
                    .height(Length::Fixed(SKELETON_BAR_HEIGHT)),
            )
            .width(Length::Fill)
            .sty(move |_| container::Style {
                snap: false,
                background: Some(Background::Color(bar_color)),
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: 2.0.into(),
                },
                ..container::Style::default()
            }),
        );
    }
    container(col)
        .width(Length::Fill)
        .padding(Padding::from([8.0, 8.0]))
        .into()
}

/// BEAUT-FILES — render `body` dimmed to `content_alpha` (`0.0..=1.0`) for the
/// stale-while-refreshing case, WITHOUT an opacity widget (the iced 0.13 fork has
/// none — same constraint the `mde_theme::animation` helpers call out). A
/// page-background scrim is stacked over the content at `1 - content_alpha`, so
/// the kept-on-screen rows fade toward the Carbon page colour (a dim, never a
/// blank). At full opacity (`>= 1.0`) the scrim is skipped — a zero-cost
/// pass-through for the common `Loaded` path.
#[must_use]
pub fn dim<'a>(body: Element<'a, Message>, content_alpha: f32) -> Element<'a, Message> {
    if content_alpha >= 1.0 {
        return body;
    }
    let scrim_alpha = (1.0 - content_alpha).clamp(0.0, 1.0);
    // Carbon Gray-80 content surface (`t::PF_BG_300`, the pane background) at the
    // complementary alpha — a token, not a raw colour (§4).
    let scrim_color = Color {
        a: scrim_alpha,
        ..t::PF_BG_300
    };
    let scrim = container(Space::new().width(Length::Fill).height(Length::Fill))
        .width(Length::Fill)
        .height(Length::Fill)
        .sty(move |_| container::Style {
            snap: false,
            background: Some(Background::Color(scrim_color)),
            ..container::Style::default()
        });
    cosmic::iced::widget::Stack::with_children(vec![body, scrim.into()])
        .width(Length::Fill)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_listing_with_no_prior_content_loads_then_settles() {
        // BEAUT-FILES — a new directory with nothing to keep paints the skeleton,
        // then settles to Loaded once the loading window elapses.
        let t0 = Instant::now();
        let mut l = ListingLoad::default();
        l.begin(t0, false);
        assert_eq!(l.state(), LoadState::Loading);
        assert!(l.show_skeleton(), "first load with no content ⇒ skeleton");
        assert!(l.is_animating(t0), "skeleton shimmer is in flight");
        // Before the window: still loading.
        let mid = t0 + loading_window() / 2;
        assert_eq!(l.settle(mid), LoadState::Loading);
        assert!(l.show_skeleton());
        // After the window: settled, no skeleton, idle.
        let done = t0 + loading_window() + Duration::from_millis(1);
        assert_eq!(l.settle(done), LoadState::Loaded);
        assert!(!l.show_skeleton());
        assert!(!l.is_animating(done), "settled listing arms no tick");
    }

    #[test]
    fn navigating_with_prior_content_keeps_it_dimmed_not_blank() {
        // BEAUT-FILES — stale-while-refreshing: a new listing WITH prior content
        // dims the old rows (content_alpha < 1) instead of showing a skeleton.
        let t0 = Instant::now();
        let mut l = ListingLoad::default();
        l.begin(t0, true);
        assert_eq!(l.state(), LoadState::Refreshing { stale: true });
        assert!(!l.show_skeleton(), "prior content ⇒ keep it, no skeleton");
        assert!(
            l.content_alpha() < 1.0,
            "stale content is dimmed, not blanked"
        );
        // Fresh rows crossfade in from transparent over the dimmed stale rows.
        assert!(l.refresh_fade(t0, false) < 0.01, "fresh rows start hidden");
        let end = t0 + refresh_window();
        assert!(
            (l.refresh_fade(end, false) - 1.0).abs() < 0.05,
            "fresh rows are fully faded in by the end of the window"
        );
        // After the refresh window: Loaded, full opacity, no fade.
        let done = t0 + refresh_window() + Duration::from_millis(1);
        assert_eq!(l.settle(done), LoadState::Loaded);
        assert!((l.content_alpha() - 1.0).abs() < f32::EPSILON);
        assert!((l.refresh_fade(done, false) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn in_place_refresh_only_fires_with_content_and_not_mid_load() {
        // BEAUT-FILES — the periodic refresh tick dims+crossfades only when there
        // is content to keep, and never restarts a load already in flight.
        let t0 = Instant::now();
        let mut l = ListingLoad::default();
        // No content ⇒ no-op (stays Idle).
        l.refresh_in_place(t0, false);
        assert_eq!(l.state(), LoadState::Idle);
        // With content ⇒ becomes a stale refresh.
        l.refresh_in_place(t0, true);
        assert_eq!(l.state(), LoadState::Refreshing { stale: true });
        // A second refresh while busy is ignored (origin not reset).
        let later = t0 + Duration::from_millis(50);
        l.refresh_in_place(later, true);
        assert!(
            l.is_animating(t0),
            "origin unchanged ⇒ refresh not restarted mid-flight"
        );
    }

    #[test]
    fn reduce_motion_makes_skeleton_static_and_fade_instant() {
        // BEAUT-FILES — a11y: the skeleton grey is phase-independent (STATIC) and
        // the refresh crossfade caps to ≤80 ms under reduce-motion.
        let t0 = Instant::now();
        // Static skeleton: same alpha at every phase.
        let a = mde_theme::animation::shimmer_alpha(0.0, true);
        let b = mde_theme::animation::shimmer_alpha(0.9, true);
        assert_eq!(a, b, "reduce-motion skeleton is static grey");
        // Refresh fade is complete by the 80 ms cap under reduce-motion.
        let mut l = ListingLoad::default();
        l.begin(t0, true);
        let cap = t0 + Duration::from_millis(mde_theme::motion::REDUCE_MOTION_CAP_MS);
        assert!(
            (l.refresh_fade(cap, true) - 1.0).abs() < 0.01,
            "reduce-motion fresh content snaps in by the 80 ms cap"
        );
    }

    #[test]
    fn skeleton_phase_cycles_within_unit_interval() {
        let t0 = Instant::now();
        let mut l = ListingLoad::default();
        l.begin(t0, false);
        for ms in [0u64, 100, 350, 699, 700, 1400] {
            let p = l.skeleton_phase(t0 + Duration::from_millis(ms));
            assert!((0.0..=1.0).contains(&p), "phase {p} out of range at {ms}ms");
        }
    }

    #[test]
    fn skeleton_widget_builds_for_any_row_count_and_motion_pref() {
        // §7 runtime-reachable — the skeleton renders for the empty, single, and
        // multi-row cases, with motion on and reduced.
        for rows in [0usize, 1, SKELETON_ROW_COUNT] {
            let _: Element<'_, Message> = skeleton_rows(rows, 0.3, false);
            let _: Element<'_, Message> = skeleton_rows(rows, 0.3, true);
        }
    }

    #[test]
    fn dim_is_pass_through_at_full_opacity_and_wraps_when_dimmed() {
        // §7 runtime-reachable — `dim` builds for both the full-opacity (no scrim)
        // and the dimmed (scrim stacked) paths.
        let _: Element<'_, Message> = dim(skeleton_rows(1, 0.0, false), 1.0);
        let _: Element<'_, Message> = dim(skeleton_rows(1, 0.0, false), 0.55);
        let _: Element<'_, Message> = dim(skeleton_rows(1, 0.0, false), 0.0);
    }
}
