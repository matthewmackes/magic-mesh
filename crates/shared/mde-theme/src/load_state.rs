//! MOTION-NET-1 — the canonical async **load state** model.
//!
//! Before this, every panel/applet carried ad-hoc `busy` / `loading` /
//! `load_error: Option<String>` flags with no shared vocabulary: "refreshing
//! over stale data" was indistinguishable from "first load", "degraded" from
//! "offline", "failed" from "empty". 30-odd surfaces each re-derived the
//! is-error-then-is-empty-then-loaded branch by hand.
//!
//! [`LoadState`] is the one enum every async surface models its lifecycle with:
//!
//! ```text
//! Idle ──begin_load──▶ Loading ──on_loaded──▶ Loaded
//!                         │                      │
//!                         └─on_error─▶ Failed    ├─begin_refresh─▶ Refreshing{stale: true}
//!                                                │                     │
//!  (any) ─on_offline─▶ Offline                  │   on_loaded ◀───────┘  (back to Loaded)
//!  (any) ─on_degraded─▶ Degraded                │
//!                                               └─on_error─▶ Failed
//! ```
//!
//! ## Design split (mirrors `EmptyState`)
//!
//! This module is dependency-free: it owns the **states + transitions +
//! render-decisions** (the non-motion a11y label, status icon, severity, and
//! "should I still show stale content / an activity indicator" predicates).
//! The Iced widget that paints a [`LoadState`] into panel chrome lives in
//! `crates/workbench/mde-workbench/src/panel_chrome.rs::load_state_chrome` so
//! the toolkit dep never leaks into `mde-theme`.
//!
//! ## Acceptance (design doc Epic 4)
//!
//! All 7 states render **distinctly**, and the difference is legible **without
//! motion**: each carries a distinct text label ([`LoadState::label`]) plus a
//! status icon ([`LoadState::icon`]), and the (icon, label) pair is unique per
//! state — so reduce-motion / screen-reader users get the same information a
//! spinner would convey. (`Loading` and `Refreshing` share the activity glyph
//! to keep the icon set small; their labels disambiguate them.)

use crate::icons::Icon;

/// Severity tier for a [`LoadState`]'s status affordance. The dependency-free
/// counterpart of `panel_chrome::BadgeSeverity` (which it maps onto 1:1 in the
/// workbench), so the render-decision lives next to the state machine instead
/// of being re-derived per call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusSeverity {
    /// Neutral / muted — nothing is wrong, nothing is notable (Idle).
    Neutral,
    /// Accent / in-progress — work is happening (Loading / Refreshing).
    Info,
    /// Success / settled — data is present and current (Loaded).
    Success,
    /// Warning — usable but compromised (Degraded / Offline-with-stale-data).
    Warning,
    /// Danger — the load failed and there is nothing trustworthy to show.
    Danger,
}

/// The canonical async lifecycle state for any data-backed surface.
///
/// Construct via [`LoadState::default`] (= [`LoadState::Idle`]) and drive it
/// through the transition helpers ([`begin_load`](LoadState::begin_load),
/// [`on_loaded`](LoadState::on_loaded), …) rather than mutating variants by
/// hand, so the legal transitions (e.g. *refresh keeps the last data visible*)
/// stay centralized.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum LoadState {
    /// Nothing requested yet — the resting state before the first load.
    #[default]
    Idle,
    /// A first load is in flight; there is no prior data to show.
    Loading,
    /// A background refresh is in flight **over existing data**. `stale` is
    /// `true` while the previously-[`Loaded`](LoadState::Loaded) content is
    /// being shown dimmed (the common case); a consumer that has no content to
    /// keep visible may set it `false`.
    Refreshing {
        /// `true` = keep the last loaded content visible (dimmed) during the
        /// refresh; `false` = no prior content to retain.
        stale: bool,
    },
    /// Data is present and current — the settled success state.
    Loaded,
    /// Data is present but the source reported a partial/compromised result
    /// (e.g. some peers unreachable). Distinct from `Failed`: there is still
    /// something trustworthy to show, just with a caveat.
    Degraded,
    /// The transport (mesh/bus/network) is unreachable. Distinct from `Failed`
    /// (a request-level error) and `Degraded` (a partial result): the whole
    /// source is unavailable and a retry depends on connectivity returning.
    Offline,
    /// The load failed with an error and there is nothing trustworthy to show.
    /// Carries the error detail for the a11y label + the failure renderer.
    Failed(String),
}

impl LoadState {
    // ---- transitions ----------------------------------------------------
    //
    // Each helper consumes `self` and returns the next state, so transitions
    // read as `self.state = self.state.begin_refresh()` and the legal moves
    // (refresh keeps stale data; a fresh load discards it) are enforced here,
    // not re-implemented per panel.

    /// Start a **first** load (no prior data). `Idle`/`Failed`/`Offline` →
    /// `Loading`; a state that already holds data (`Loaded`/`Degraded`/
    /// `Refreshing`) starts a [`begin_refresh`](LoadState::begin_refresh)
    /// instead so existing content is kept visible rather than blanked.
    #[must_use]
    pub fn begin_load(self) -> Self {
        if self.has_content() {
            self.begin_refresh()
        } else {
            Self::Loading
        }
    }

    /// Start a **background refresh** while keeping any current data visible.
    /// From a content-bearing state (`Loaded`/`Degraded`) → `Refreshing{stale:
    /// true}`; from a state with nothing to keep → `Loading` (a refresh with no
    /// stale content to show *is* a first load, visually).
    #[must_use]
    pub fn begin_refresh(self) -> Self {
        match self {
            Self::Loaded | Self::Degraded => Self::Refreshing { stale: true },
            Self::Refreshing { .. } => self, // already refreshing — idempotent
            _ => Self::Loading,
        }
    }

    /// Data arrived successfully → [`Loaded`](LoadState::Loaded).
    #[must_use]
    pub fn on_loaded(self) -> Self {
        Self::Loaded
    }

    /// Data arrived but partial/compromised → [`Degraded`](LoadState::Degraded).
    #[must_use]
    pub fn on_degraded(self) -> Self {
        Self::Degraded
    }

    /// The transport is unreachable → [`Offline`](LoadState::Offline).
    #[must_use]
    pub fn on_offline(self) -> Self {
        Self::Offline
    }

    /// The load failed with `err` → [`Failed`](LoadState::Failed).
    #[must_use]
    pub fn on_error(self, err: impl Into<String>) -> Self {
        Self::Failed(err.into())
    }

    // ---- render decisions -----------------------------------------------

    /// `true` when a load/refresh is in flight — drives the activity
    /// indicator (spinner/shimmer) and disables a Refresh control.
    #[must_use]
    pub fn is_busy(&self) -> bool {
        matches!(self, Self::Loading | Self::Refreshing { .. })
    }

    /// `true` when this state has trustworthy content to display — drives
    /// "render the data" vs "render placeholder/error chrome". Stale-while-
    /// refreshing keeps content; a first `Loading` and a `Failed`/`Offline`
    /// with nothing cached do not.
    #[must_use]
    pub fn has_content(&self) -> bool {
        match self {
            Self::Loaded | Self::Degraded => true,
            Self::Refreshing { stale } => *stale,
            Self::Idle | Self::Loading | Self::Offline | Self::Failed(_) => false,
        }
    }

    /// `true` when the visible content should be dimmed (stale-while-
    /// refreshing): a refresh is in flight over kept content. MOTION-NET-3
    /// reads this to dim the old data before crossfading in the new.
    #[must_use]
    pub fn content_is_stale(&self) -> bool {
        matches!(self, Self::Refreshing { stale: true })
    }

    /// `true` for the terminal failure state ([`Failed`](LoadState::Failed)) —
    /// the panel should render the error/retry chrome, not an empty state.
    #[must_use]
    pub fn is_failed(&self) -> bool {
        matches!(self, Self::Failed(_))
    }

    /// The error detail, if this is a [`Failed`](LoadState::Failed) state.
    #[must_use]
    pub fn error(&self) -> Option<&str> {
        match self {
            Self::Failed(e) => Some(e.as_str()),
            _ => None,
        }
    }

    /// The severity tier for this state's status affordance (badge tint /
    /// chrome accent). One stable mapping every surface shares.
    #[must_use]
    pub fn severity(&self) -> StatusSeverity {
        match self {
            Self::Idle => StatusSeverity::Neutral,
            Self::Loading | Self::Refreshing { .. } => StatusSeverity::Info,
            Self::Loaded => StatusSeverity::Success,
            Self::Degraded | Self::Offline => StatusSeverity::Warning,
            Self::Failed(_) => StatusSeverity::Danger,
        }
    }

    /// The status icon for this state — the **non-motion** half of the a11y
    /// contract (paired with [`label`](LoadState::label)). Distinct per state
    /// so the difference is legible with animation disabled.
    #[must_use]
    pub fn icon(&self) -> Icon {
        match self {
            Self::Idle => Icon::StatusUnknown,
            // Loading + Refreshing share the activity glyph; the label + the
            // `content_is_stale` predicate carry the first-load vs refresh
            // distinction, so the icon set stays small + recognizable.
            Self::Loading | Self::Refreshing { .. } => Icon::Refresh,
            Self::Loaded => Icon::StatusOk,
            Self::Degraded => Icon::StatusWarning,
            Self::Offline => Icon::Wifi,
            Self::Failed(_) => Icon::StatusError,
        }
    }

    /// A short, screen-reader-friendly **text** label for this state — the
    /// other non-motion half of the a11y contract. Every state's label is
    /// distinct, so a reduce-motion user reads the state a spinner would
    /// otherwise animate. `Failed` reports the error detail inline.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Idle => "Idle".to_string(),
            Self::Loading => "Loading…".to_string(),
            Self::Refreshing { stale: true } => "Refreshing…".to_string(),
            Self::Refreshing { stale: false } => "Loading…".to_string(),
            Self::Loaded => "Up to date".to_string(),
            Self::Degraded => "Degraded — showing partial data".to_string(),
            Self::Offline => "Offline".to_string(),
            Self::Failed(e) => format!("Couldn't load — {e}"),
        }
    }

    /// A stable lowercase identifier for this state (no payload) — for tests,
    /// logs, and metrics. Distinct per variant; `Refreshing` collapses its
    /// `stale` flag.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Loading => "loading",
            Self::Refreshing { .. } => "refreshing",
            Self::Loaded => "loaded",
            Self::Degraded => "degraded",
            Self::Offline => "offline",
            Self::Failed(_) => "failed",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every state used in the render-distinctness assertions below.
    fn all_states() -> Vec<LoadState> {
        vec![
            LoadState::Idle,
            LoadState::Loading,
            LoadState::Refreshing { stale: true },
            LoadState::Loaded,
            LoadState::Degraded,
            LoadState::Offline,
            LoadState::Failed("io error".into()),
        ]
    }

    #[test]
    fn default_is_idle() {
        assert_eq!(LoadState::default(), LoadState::Idle);
    }

    #[test]
    fn the_seven_states_have_distinct_labels() {
        // Design-doc acceptance: all 7 states render distinctly AND are legible
        // without motion (the text label is the non-motion channel).
        let labels: Vec<String> = all_states().iter().map(LoadState::label).collect();
        let unique: std::collections::HashSet<&String> = labels.iter().collect();
        assert_eq!(unique.len(), labels.len(), "labels: {labels:?}");
    }

    #[test]
    fn the_seven_states_have_a_distinct_non_motion_signature() {
        // Design-doc acceptance: each of the 7 states is legible WITHOUT motion
        // via text/icon. The non-motion render signature is the (icon, label)
        // pair — `Loading` and `Refreshing{stale}` deliberately share the
        // activity glyph (a small, recognizable icon set) but are still
        // distinct because their labels differ ("Loading…" vs "Refreshing…").
        let sigs: Vec<(Icon, String)> =
            all_states().iter().map(|s| (s.icon(), s.label())).collect();
        let unique: std::collections::HashSet<&(Icon, String)> = sigs.iter().collect();
        assert_eq!(unique.len(), sigs.len(), "signatures: {sigs:?}");
    }

    #[test]
    fn only_the_activity_states_share_an_icon() {
        // The icon set stays small: just Loading + Refreshing alias (both are
        // "work in flight"); every other state has its own glyph.
        assert_eq!(LoadState::Loading.icon(), Icon::Refresh);
        assert_eq!(
            LoadState::Refreshing { stale: true }.icon(),
            LoadState::Loading.icon()
        );
        assert_ne!(LoadState::Loaded.icon(), LoadState::Loading.icon());
        assert_ne!(LoadState::Offline.icon(), LoadState::Degraded.icon());
        assert_ne!(
            LoadState::Failed("x".into()).icon(),
            LoadState::Loaded.icon()
        );
    }

    #[test]
    fn the_seven_states_have_distinct_kinds() {
        let kinds: Vec<&str> = all_states().iter().map(LoadState::kind).collect();
        let unique: std::collections::HashSet<&&str> = kinds.iter().collect();
        assert_eq!(unique.len(), kinds.len(), "kinds: {kinds:?}");
    }

    #[test]
    fn begin_load_from_idle_is_first_load() {
        assert_eq!(LoadState::Idle.begin_load(), LoadState::Loading);
        assert_eq!(
            LoadState::Failed("x".into()).begin_load(),
            LoadState::Loading
        );
        assert_eq!(LoadState::Offline.begin_load(), LoadState::Loading);
    }

    #[test]
    fn begin_load_over_existing_data_keeps_it_visible() {
        // A reload while we already hold data is a refresh, not a blank-out.
        assert_eq!(
            LoadState::Loaded.begin_load(),
            LoadState::Refreshing { stale: true }
        );
        assert_eq!(
            LoadState::Degraded.begin_load(),
            LoadState::Refreshing { stale: true }
        );
    }

    #[test]
    fn begin_refresh_from_contentless_state_is_a_first_load() {
        assert_eq!(LoadState::Idle.begin_refresh(), LoadState::Loading);
        assert_eq!(LoadState::Loading.begin_refresh(), LoadState::Loading);
    }

    #[test]
    fn begin_refresh_is_idempotent() {
        let refreshing = LoadState::Refreshing { stale: true };
        assert_eq!(refreshing.clone().begin_refresh(), refreshing);
    }

    #[test]
    fn loaded_transition_settles() {
        assert_eq!(LoadState::Loading.on_loaded(), LoadState::Loaded);
        assert_eq!(
            LoadState::Refreshing { stale: true }.on_loaded(),
            LoadState::Loaded
        );
    }

    #[test]
    fn error_transition_carries_detail() {
        let s = LoadState::Loading.on_error("disk full");
        assert!(s.is_failed());
        assert_eq!(s.error(), Some("disk full"));
        assert!(s.label().contains("disk full"));
    }

    #[test]
    fn busy_only_while_loading_or_refreshing() {
        assert!(LoadState::Loading.is_busy());
        assert!(LoadState::Refreshing { stale: true }.is_busy());
        assert!(LoadState::Refreshing { stale: false }.is_busy());
        assert!(!LoadState::Idle.is_busy());
        assert!(!LoadState::Loaded.is_busy());
        assert!(!LoadState::Degraded.is_busy());
        assert!(!LoadState::Offline.is_busy());
        assert!(!LoadState::Failed("x".into()).is_busy());
    }

    #[test]
    fn has_content_matches_renderable_states() {
        assert!(LoadState::Loaded.has_content());
        assert!(LoadState::Degraded.has_content());
        assert!(LoadState::Refreshing { stale: true }.has_content());
        assert!(!LoadState::Refreshing { stale: false }.has_content());
        assert!(!LoadState::Idle.has_content());
        assert!(!LoadState::Loading.has_content());
        assert!(!LoadState::Offline.has_content());
        assert!(!LoadState::Failed("x".into()).has_content());
    }

    #[test]
    fn stale_predicate_only_for_refreshing_with_stale_data() {
        assert!(LoadState::Refreshing { stale: true }.content_is_stale());
        assert!(!LoadState::Refreshing { stale: false }.content_is_stale());
        assert!(!LoadState::Loaded.content_is_stale());
    }

    #[test]
    fn severity_mapping_is_stable() {
        assert_eq!(LoadState::Idle.severity(), StatusSeverity::Neutral);
        assert_eq!(LoadState::Loading.severity(), StatusSeverity::Info);
        assert_eq!(
            LoadState::Refreshing { stale: true }.severity(),
            StatusSeverity::Info
        );
        assert_eq!(LoadState::Loaded.severity(), StatusSeverity::Success);
        assert_eq!(LoadState::Degraded.severity(), StatusSeverity::Warning);
        assert_eq!(LoadState::Offline.severity(), StatusSeverity::Warning);
        assert_eq!(
            LoadState::Failed("x".into()).severity(),
            StatusSeverity::Danger
        );
    }

    #[test]
    fn full_lifecycle_round_trip() {
        // Idle → first load → loaded → refresh (keeps data) → loaded again.
        let mut s = LoadState::Idle;
        s = s.begin_load();
        assert_eq!(s, LoadState::Loading);
        s = s.on_loaded();
        assert_eq!(s, LoadState::Loaded);
        s = s.begin_refresh();
        assert_eq!(s, LoadState::Refreshing { stale: true });
        assert!(s.has_content() && s.content_is_stale());
        s = s.on_loaded();
        assert_eq!(s, LoadState::Loaded);
    }
}
