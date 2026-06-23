//! MOTION-NET-1 — the canonical async load-state model.
//!
//! One vocabulary every surface (workbench panels, `mde-music`, applets) shares
//! for async/network state, replacing the ad-hoc `busy`/`loading`/`load_error`
//! booleans that scattered across panels with no shared meaning for *refreshing
//! vs degraded vs offline vs stale*.
//!
//! The state is legible **without motion**: every variant carries a distinct
//! text [`label`](LoadState::label) **and** a distinct [`icon`](LoadState::icon)
//! glyph (differing by shape, not only colour) — that pair is the a11y contract
//! (Q-A11Y: never rely on motion or colour alone). Activity cues map onto the
//! shared [`Motion`] presets so spinners/pulses come from one source, and route
//! through [`Motion::resolved`] for the reduce-motion contract. Colour
//! ([`tone`](LoadState::tone)) is a secondary cue layered on top, never the only
//! differentiator.

use crate::motion::Motion;

/// The seven canonical async states a surface can be in.
///
/// `Refreshing { stale }` distinguishes a background refresh that still has the
/// previous [`Loaded`](LoadState::Loaded) content to show (`stale: true`, the
/// stale-while-revalidate case — keep the old data visible) from one with
/// nothing to show yet (`stale: false`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadState {
    /// Nothing requested yet — the resting state before any fetch.
    Idle,
    /// A first load is in flight with no prior content to show.
    Loading,
    /// A background refresh is in flight. `stale` = the last `Loaded` content is
    /// still on screen underneath it.
    Refreshing {
        /// Whether prior content is still being shown beneath the refresh.
        stale: bool,
    },
    /// Loaded, but the source reported reduced function (partial data, a slow
    /// or quorum-degraded backend) — usable, with a caveat.
    Degraded,
    /// The source is unreachable (no mesh / no peers); a cached or empty view
    /// is shown and the surface is waiting to reconnect.
    Offline,
    /// The load failed terminally; the user needs a retry affordance.
    Failed,
    /// Fully loaded, current content.
    Loaded,
}

/// Semantic severity tone for a [`LoadState`].
///
/// Consumers map this onto the Carbon support tokens (e.g.
/// `support_error`/`support_success`); it is a secondary colour cue, never the
/// sole differentiator (the label + icon are).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateTone {
    /// No emphasis — idle / resting.
    Neutral,
    /// In-progress / informational.
    Info,
    /// Usable but caveated (degraded / offline-with-cache).
    Warning,
    /// Terminal failure.
    Danger,
    /// Success / current.
    Success,
}

impl LoadState {
    /// The non-motion text label — distinct for every state (the a11y contract).
    /// `Refreshing` reads the same regardless of `stale`; the staleness is
    /// conveyed by keeping the prior content on screen, not by the label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Loading => "Loading…",
            Self::Refreshing { .. } => "Refreshing…",
            Self::Degraded => "Degraded",
            Self::Offline => "Offline",
            Self::Failed => "Failed",
            Self::Loaded => "Ready",
        }
    }

    /// The non-motion icon glyph — distinct for every state, differing by
    /// **shape** so it reads under reduce-motion and for colour-blind users.
    #[must_use]
    pub const fn icon(self) -> char {
        match self {
            Self::Idle => '○',
            Self::Loading => '◍',
            Self::Refreshing { .. } => '↻',
            Self::Degraded => '△',
            Self::Offline => '⊘',
            Self::Failed => '✕',
            Self::Loaded => '✓',
        }
    }

    /// The semantic [`StateTone`] for the secondary colour cue.
    #[must_use]
    pub const fn tone(self) -> StateTone {
        match self {
            Self::Idle => StateTone::Neutral,
            Self::Loading | Self::Refreshing { .. } => StateTone::Info,
            Self::Degraded | Self::Offline => StateTone::Warning,
            Self::Failed => StateTone::Danger,
            Self::Loaded => StateTone::Success,
        }
    }

    /// The shared activity [`Motion`] this state animates with, if any. The
    /// static states (`Idle`/`Degraded`/`Offline`) return `None`. Callers apply
    /// [`Motion::resolved`] with the user's reduce-motion preference; the label
    /// + icon remain legible either way.
    #[must_use]
    pub fn motion(self) -> Option<Motion> {
        match self {
            Self::Loading => Some(Motion::loading()),
            Self::Refreshing { .. } => Some(Motion::refresh()),
            Self::Failed => Some(Motion::error()),
            Self::Loaded => Some(Motion::success()),
            Self::Idle | Self::Degraded | Self::Offline => None,
        }
    }

    /// `true` while a fetch is in flight (`Loading` or `Refreshing`) — the
    /// replacement for the old scattered `busy` flag.
    #[must_use]
    pub const fn is_busy(self) -> bool {
        matches!(self, Self::Loading | Self::Refreshing { .. })
    }

    /// `true` when there is real content to render underneath the chrome:
    /// fully `Loaded`, or `Refreshing` over still-shown stale content.
    #[must_use]
    pub const fn shows_content(self) -> bool {
        matches!(self, Self::Loaded | Self::Refreshing { stale: true })
    }

    /// `true` for the terminal failure state.
    #[must_use]
    pub const fn is_error(self) -> bool {
        matches!(self, Self::Failed)
    }

    /// `true` when the surface should offer a retry / reconnect affordance —
    /// the user can act to move the state forward.
    #[must_use]
    pub const fn can_retry(self) -> bool {
        matches!(self, Self::Failed | Self::Degraded | Self::Offline)
    }

    /// MOTION-NET-3 — the alpha to render kept-on-screen content at. Full (1.0)
    /// normally; **dimmed** while `Refreshing { stale: true }` so a refresh keeps
    /// the previous data visible-but-dimmed (stale-while-revalidate) instead of
    /// blanking the panel, until fresh data lands. The other states don't show
    /// content, so 1.0 is moot for them.
    #[must_use]
    pub const fn content_alpha(self) -> f32 {
        match self {
            Self::Refreshing { stale: true } => 0.55,
            _ => 1.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One representative of each of the seven states — the render/distinctness
    /// matrix the acceptance ("all 7 states render distinctly") is checked over.
    const ALL: [LoadState; 7] = [
        LoadState::Idle,
        LoadState::Loading,
        LoadState::Refreshing { stale: false },
        LoadState::Degraded,
        LoadState::Offline,
        LoadState::Failed,
        LoadState::Loaded,
    ];

    #[test]
    fn every_state_has_a_distinct_label() {
        // a11y: legible without motion — the text alone disambiguates all 7.
        let labels: Vec<&str> = ALL.iter().map(|s| s.label()).collect();
        for (i, a) in labels.iter().enumerate() {
            for b in &labels[i + 1..] {
                assert_ne!(a, b, "labels must be pairwise distinct: {a:?} vs {b:?}");
            }
            assert!(!a.is_empty(), "every state needs a non-empty label");
        }
    }

    #[test]
    fn every_state_has_a_distinct_icon() {
        // a11y: the icon differs by shape, so the state reads under reduce-motion
        // and without relying on colour.
        let icons: Vec<char> = ALL.iter().map(|s| s.icon()).collect();
        for (i, a) in icons.iter().enumerate() {
            for b in &icons[i + 1..] {
                assert_ne!(a, b, "icons must be pairwise distinct: {a} vs {b}");
            }
        }
    }

    #[test]
    fn refreshing_reads_the_same_label_and_icon_regardless_of_stale() {
        // `stale` changes whether prior content stays on screen, not the chrome.
        let fresh = LoadState::Refreshing { stale: false };
        let stale = LoadState::Refreshing { stale: true };
        assert_eq!(fresh.label(), stale.label());
        assert_eq!(fresh.icon(), stale.icon());
        // …but only the stale variant keeps content visible underneath.
        assert!(stale.shows_content());
        assert!(!fresh.shows_content());
    }

    #[test]
    fn motion_maps_to_the_shared_presets_only_for_active_states() {
        assert_eq!(LoadState::Loading.motion(), Some(Motion::loading()));
        assert_eq!(
            LoadState::Refreshing { stale: true }.motion(),
            Some(Motion::refresh())
        );
        assert_eq!(LoadState::Failed.motion(), Some(Motion::error()));
        assert_eq!(LoadState::Loaded.motion(), Some(Motion::success()));
        // Static states do not animate.
        assert_eq!(LoadState::Idle.motion(), None);
        assert_eq!(LoadState::Degraded.motion(), None);
        assert_eq!(LoadState::Offline.motion(), None);
    }

    #[test]
    fn reduce_motion_does_not_change_the_non_motion_chrome() {
        // The a11y guarantee: with reduce-motion the animation collapses, but the
        // label + icon (how the state is actually read) are unchanged.
        for s in ALL {
            let before = (s.label(), s.icon());
            // Resolving motion is a no-op on the label/icon — they are not motion.
            if let Some(m) = s.motion() {
                let _ = m.resolved(true);
            }
            assert_eq!((s.label(), s.icon()), before);
        }
    }

    #[test]
    fn predicates_partition_the_states_as_documented() {
        assert!(LoadState::Loading.is_busy() && LoadState::Refreshing { stale: false }.is_busy());
        assert!(!LoadState::Loaded.is_busy() && !LoadState::Idle.is_busy());

        assert!(LoadState::Loaded.shows_content());
        assert!(!LoadState::Loading.shows_content());

        assert!(LoadState::Failed.is_error());
        assert!(!LoadState::Degraded.is_error());

        // Retry affordance offered exactly for the recoverable problem states.
        for s in ALL {
            let expected = matches!(
                s,
                LoadState::Failed | LoadState::Degraded | LoadState::Offline
            );
            assert_eq!(s.can_retry(), expected, "{s:?}");
        }
    }

    #[test]
    fn tones_track_severity() {
        assert_eq!(LoadState::Idle.tone(), StateTone::Neutral);
        assert_eq!(LoadState::Loading.tone(), StateTone::Info);
        assert_eq!(LoadState::Degraded.tone(), StateTone::Warning);
        assert_eq!(LoadState::Offline.tone(), StateTone::Warning);
        assert_eq!(LoadState::Failed.tone(), StateTone::Danger);
        assert_eq!(LoadState::Loaded.tone(), StateTone::Success);
    }

    #[test]
    fn content_alpha_dims_only_stale_refresh() {
        // MOTION-NET-3: stale content kept-but-dimmed during a refresh; full
        // opacity otherwise.
        assert!(LoadState::Refreshing { stale: true }.content_alpha() < 1.0);
        assert_eq!(LoadState::Refreshing { stale: false }.content_alpha(), 1.0);
        assert_eq!(LoadState::Loaded.content_alpha(), 1.0);
        assert_eq!(LoadState::Idle.content_alpha(), 1.0);
        assert_eq!(LoadState::Failed.content_alpha(), 1.0);
    }
}
