//! BEAUT-FILES — perceived-performance state for the file/list view.
//!
//! Two jobs, both pure glue over `mde_theme`:
//!
//! 1. **Skeleton-first paint.** The instant a listing is navigated to, the view
//!    paints layout-matching greyed placeholder rows (a [`skeleton_rows`]) so the
//!    pane is never blank while the backend enumerates the directory. The bars are
//!    the shared Carbon [`mde_theme::SkeletonBlock`] geometry filled by a
//!    [`mde_theme::SkeletonShimmer`] — the grey *breathes* over the loading period
//!    and collapses to a STATIC grey under reduce-motion / the motion kill switch
//!    (the a11y contract: motion is never the only cue), single-sourced in
//!    [`mde_theme::skeleton`].
//!
//! 2. **Stale-while-refreshing.** A *refresh* of a listing that already has
//!    content keeps the previous rows on screen — dimmed (not blanked) — and
//!    crossfades the fresh rows in when they land, instead of flashing empty.
//!
//! The async vocabulary is the shared [`mde_theme::LoadState`]
//! (`Loading`/`Refreshing { stale }`/`Loaded`); the dim factor is
//! [`LoadState::content_alpha`], the skeleton placeholder is the shared
//! [`mde_theme::skeleton`] primitive (geometry + shimmer + the reduce-motion
//! contract), and the fade-in of fresh content is
//! [`mde_theme::animation::fade_in`]. This module owns NONE of that math — it is
//! the glue (`AI_GOVERNANCE.md` §4/§6/§7: reuse the shared primitive, don't
//! reimplement).

use std::time::{Duration, Instant};

use cosmic::iced::widget::{column, container, Space};
use cosmic::iced::{Background, Border, Color, Length, Padding};
use cosmic::Element;
use mde_theme::motion::Motion;
use mde_theme::{LoadState, Palette, Radii, SkeletonBlock, SkeletonShimmer, Space as Spacing};

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
    /// When the current `state` was entered — the origin the skeleton shimmer
    /// clock + the crossfade progress are sampled against.
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

    /// BEAUT-FILES — the shared Carbon skeleton shimmer for this listing's loading
    /// paint, its breathe clock anchored at the load `origin`. `reduce_motion`
    /// (already folding the a11y preference + the motion kill switch) collapses it
    /// to a STATIC grey block — the a11y contract single-sourced in
    /// [`mde_theme::skeleton`]. Consumed by [`skeleton_rows`]; its
    /// [`SkeletonShimmer::needs_tick`] is the visibility/liveness gate.
    #[must_use]
    pub fn skeleton_shimmer(self, reduce_motion: bool) -> SkeletonShimmer {
        SkeletonShimmer::new(self.origin, reduce_motion)
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

/// BEAUT-FILES — render `rows` greyed placeholder bars for a loading file list.
///
/// Each bar is a shared Carbon [`SkeletonBlock::line`] (fill-width, body line-box
/// height, `sm` corner) filled by the [`SkeletonShimmer`]: its grey *breathes*
/// over the loading period, or holds a STATIC mid-grey under reduce-motion / the
/// motion kill switch (the a11y contract, single-sourced in
/// [`mde_theme::skeleton`]). Geometry, tint and the reduce-motion fallback all
/// come from the shared primitive — this is glue, not a reimplementation (§6).
#[must_use]
pub fn skeleton_rows<'a>(
    rows: usize,
    shimmer: SkeletonShimmer,
    now: Instant,
    palette: &Palette,
) -> Element<'a, Message> {
    // Shared Carbon geometry: a fill-width single-line block (height = body
    // line-box, `sm` corner) — no re-derived bar height or radius literal (§6).
    let block = SkeletonBlock::line(None, Radii::defaults());
    let height = f32::from(block.height);
    let radius = f32::from(block.radius);
    // Shared shimmer fill: the palette `text` token at the breathe alpha,
    // composited over the surface — a token, not a raw colour (§4).
    let fill = shimmer.fill(now, palette);
    let bar_color = Color {
        r: fill.r as f32 / 255.0,
        g: fill.g as f32 / 255.0,
        b: fill.b as f32 / 255.0,
        a: fill.a,
    };
    // Inter-row gap = the Carbon "gap between adjacent list rows" spacing token
    // (§4 single source), so the skeleton keeps the eventual rows' rhythm.
    let gap = f32::from(Spacing::scaled(1.0).md);
    let mut col = column![].spacing(gap);
    for _ in 0..rows {
        col = col.push(
            container(
                Space::new()
                    .width(Length::Fill)
                    .height(Length::Fixed(height)),
            )
            .width(Length::Fill)
            .sty(move |_| container::Style {
                snap: false,
                background: Some(Background::Color(bar_color)),
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: radius.into(),
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
    // Carbon Gray-80 content surface (`t::PF_BG_300`, the pane background) — a
    // token, not a raw colour (§4). Fill-width: dims a flex-sized content pane.
    scrim_over(body, content_alpha, t::PF_BG_300, Length::Fill)
}

/// MOTION-TRANS-2 — like [`dim`], but for a **fixed-width** surface (the side
/// rail): the dim scrim is constrained to `width` so the stack keeps the rail's
/// own width instead of expanding to fill its slot. Used to crossfade the
/// sidebar in on a collapse/expand without disturbing the row layout. Full-alpha
/// (`>= 1.0`) is the zero-cost settled pass-through.
#[must_use]
pub fn dim_fixed<'a>(body: Element<'a, Message>, alpha: f32, width: f32) -> Element<'a, Message> {
    // Carbon Gray-90 side-rail surface (`t::WINDOW_SIDE`) so the rail fades toward
    // its own page colour, never a hole; width-pinned so it keeps the rail width.
    scrim_over(body, alpha, t::WINDOW_SIDE, Length::Fixed(width))
}

/// BEAUT-FILES / MOTION-TRANS-2 — the shared fake-opacity primitive: stack a
/// `surface`-coloured scrim over `body` at `1 - alpha`, so the content fades
/// toward its page colour (the iced fork has no opacity widget). `stack_w` pins
/// the resulting stack's width (Fill for a flex pane, Fixed for the side rail).
/// Full-alpha (`>= 1.0`) skips the scrim entirely — a zero-cost pass-through.
fn scrim_over<'a>(
    body: Element<'a, Message>,
    alpha: f32,
    surface: Color,
    stack_w: Length,
) -> Element<'a, Message> {
    if alpha >= 1.0 {
        return body;
    }
    let scrim_color = Color {
        a: (1.0 - alpha).clamp(0.0, 1.0),
        ..surface
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
        .width(stack_w)
        .height(Length::Fill)
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
        // BEAUT-FILES — a11y: the shared skeleton shimmer is STATIC (and arms no
        // tick) under reduce-motion, and the refresh crossfade caps to ≤80 ms.
        let t0 = Instant::now();
        // Static skeleton: the shared shimmer reports static + never ticks.
        let still = ListingLoad::default().skeleton_shimmer(true);
        assert!(still.is_static(), "reduce-motion skeleton is static grey");
        assert!(
            !still.needs_tick(true),
            "a static skeleton arms no breathe tick"
        );
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
    fn skeleton_shimmer_breathes_while_visible_and_is_anchored_at_origin() {
        // BEAUT-FILES — the loading paint drives the shared shimmer: a live shimmer
        // arms its breathe tick only while visible, and its fill resolves against
        // the palette `text` token at every frame across the loading window.
        let t0 = Instant::now();
        let mut l = ListingLoad::default();
        l.begin(t0, false);
        let live = l.skeleton_shimmer(false);
        assert!(!live.is_static(), "motion-live ⇒ the skeleton breathes");
        assert!(live.needs_tick(true), "a visible live skeleton arms a tick");
        assert!(!live.needs_tick(false), "a hidden skeleton arms no tick");
        // Fill resolves to the palette `text` hue at a breathe alpha, every frame.
        let pal = crate::theme::mde_files_palette();
        for ms in [0u64, 100, 350, 699, 700, 1400] {
            let f = live.fill(t0 + Duration::from_millis(ms), &pal);
            assert_eq!((f.r, f.g, f.b), (pal.text.r, pal.text.g, pal.text.b));
            assert!(
                (0.0..=1.0).contains(&f.a),
                "alpha {} out of range at {ms}ms",
                f.a
            );
        }
    }

    #[test]
    fn skeleton_widget_builds_for_any_row_count_and_motion_pref() {
        // §7 runtime-reachable — the skeleton renders for the empty, single, and
        // multi-row cases, with motion on and reduced, off the shared primitive.
        let now = Instant::now();
        let pal = crate::theme::mde_files_palette();
        for rows in [0usize, 1, SKELETON_ROW_COUNT] {
            let live = SkeletonShimmer::new(now, false);
            let still = SkeletonShimmer::new(now, true);
            let _: Element<'_, Message> = skeleton_rows(rows, live, now, &pal);
            let _: Element<'_, Message> = skeleton_rows(rows, still, now, &pal);
        }
    }

    #[test]
    fn dim_is_pass_through_at_full_opacity_and_wraps_when_dimmed() {
        // §7 runtime-reachable — `dim` builds for both the full-opacity (no scrim)
        // and the dimmed (scrim stacked) paths.
        let now = Instant::now();
        let pal = crate::theme::mde_files_palette();
        let sk = || skeleton_rows(1, SkeletonShimmer::new(now, false), now, &pal);
        let _: Element<'_, Message> = dim(sk(), 1.0);
        let _: Element<'_, Message> = dim(sk(), 0.55);
        let _: Element<'_, Message> = dim(sk(), 0.0);
    }
}
