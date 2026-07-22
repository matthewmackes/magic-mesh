//! `construct` — WL-UX-006/U09: the **Construct chrome scaffold** — the ONE
//! system input-contract dispatcher plus the shared open/state flags the five
//! Construct chrome surfaces (springboard · status bar · app switcher ·
//! control center · notification center) mount onto.
//!
//! Authority: `docs/design/platform-interfaces.md` §2.3 (Q11/Q12) + §2.4
//! (Q13–Q16). The design locks **one contract table, one drain site**: every
//! system gesture/chord folds through [`intents_from_input`] into a typed
//! [`ChromeIntent`] the shell routes — no surface ever grows a private edge
//! or Super binding. The five overlay/chrome units (U10 springboard, U11
//! status bar, U12 switcher, U13 control center, U15 notification center)
//! land as new files consuming their intent from [`ConstructChrome`]'s
//! per-frame queue; `main.rs` never changes again for them (U09's whole
//! point).
//!
//! ## The Super overload, resolved
//!
//! The §2.3 table gives Super two pointer-parity rows: *Home* = Super tap
//! and *Spotlight* = "Super (on home)". The deterministic resolution
//! implemented here (the natural iPadOS reading of the lock):
//!
//! * **Super tap while an app is expanded → Home** — leave the app for the
//!   home base, exactly like the hardware home affordance;
//! * **Super tap while already on home → Spotlight** — the base is already
//!   showing, so the same key falls through to search focus.
//!
//! ## What the scaffold does NOT decide yet
//!
//! * **Bottom-edge swipe-up-*hold* → Switcher** — the SURFACE-11 side channel
//!   ([`mde_egui::drain_edge_swipes`]) carries a fired [`Edge`] with no hold /
//!   continuation signal, so the touch Switcher row is a **U16 follow-up**
//!   (extend the recognizer's channel); the keyboard row (Super+Tab) is live.
//! * **Top-edge x-position** rides the frame's synthesized pointer position
//!   (the SURFACE-8 touch translator emits `PointerMoved` for the primary
//!   contact); when no fix exists the pull resolves to the wider
//!   Notification Center target, honestly. U16 may carry the true swipe x
//!   through the side channel instead.
//! * The **VDI dwell guard** is a minimal two-swipe confirm
//!   ([`EdgeGuard`]) — U16 refines it with a visible arming affordance.

use std::time::{Duration, Instant};

use mde_egui::Edge;

/// The five system intents of the locked input contract — §2.3's five rows
/// (PLATFORM-INTERFACES Q11), each landing on one Construct chrome surface
/// (§2.4). Everything the shell's system gestures/chords can *mean*.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChromeIntent {
    /// Leave the expanded app for the home base (the U10 springboard; until it
    /// lands, the collapsed session view it replaces).
    Home,
    /// Open/close the app switcher (Q16; the U12 card grid).
    Switcher,
    /// Open/close the Control Center (Q13; the U13 sheet).
    ControlCenter,
    /// Open/close the Notification Center (Q14; the U15 pull-down).
    NotificationCenter,
    /// Focus system search — the Front Door engine reskinned (Q15, U14). The
    /// producers/ranking/keyboard flow stay byte-identical to today's launcher.
    Spotlight,
}

/// One drained edge swipe, plus where along the edge it happened when known.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EdgeSwipe {
    /// Which screen edge the swipe began at (the SURFACE-11 recognizer's
    /// classification, drained off the seat side channel).
    pub edge: Edge,
    /// The swipe's normalized screen-x (`0.0` = left, `1.0` = right), read
    /// from the frame's synthesized primary-contact pointer position. `None`
    /// when the frame carries no pointer fix — a top pull then resolves to
    /// the Notification Center (the wider §2.3 target), never a guess.
    pub x_frac: Option<f32>,
}

/// One frame's decoded system input — exactly what the §2.3 contract table
/// reads. `main.rs` builds it from the existing drains (the SURFACE-11
/// edge-swipe channel, the E12-19 `HotkeyRouter`'s Super-tap latch and its
/// `Super+Tab` chord) so the dispatcher stays pure and headless-testable.
#[derive(Debug, Clone, PartialEq)]
pub struct ChromeInput {
    /// A clean Super tap this frame (`HotkeyRouter::take_dock_toggle`, the
    /// VDOCK-1 press+release-with-no-chord latch).
    pub super_tap: bool,
    /// The `Super+Tab` chord fired this frame (the fixed table's
    /// `SessionSwitch` chord — §2.3 makes it the Switcher's keyboard row).
    pub super_tab: bool,
    /// Whether an app is expanded over the home base (`nav.expanded`) — the
    /// Super-overload resolver (module doc): expanded → Home, home → Spotlight.
    pub app_expanded: bool,
    /// The `full_screen_remote_desktop` condition — a focused full-screen
    /// VDI/remote session in front. Edge gestures then require the
    /// [`EdgeGuard`] second-swipe confirm; Super chords always pass (§2.3).
    pub remote_session_focused: bool,
    /// Every edge swipe drained this frame (usually 0 or 1).
    pub edges: Vec<EdgeSwipe>,
    /// The shell's monotonic clock ([`ConstructChrome::now`]) — drives the
    /// dwell guard's confirm window.
    pub now: Duration,
}

/// A top-edge pull at/right of this x-fraction is the **Control Center**
/// (§2.3: "top-right pull-down"); anywhere left of it — or with no pointer
/// fix — is the **Notification Center** ("top-left/center pull-down").
pub const TOP_RIGHT_THIRD: f32 = 2.0 / 3.0;

/// How long an armed edge stays armed over a focused remote session before a
/// confirm swipe must re-arm (the two-swipe dwell window, §2.3's VDI guard).
pub const EDGE_CONFIRM_WINDOW: Duration = Duration::from_millis(1500);

/// The VDI edge-dwell guard (§2.3: "Over a focused VDI session, edge
/// gestures require dwell (second-swipe confirm); Super chords always
/// work.") — scaffolded as a simple armed-edge timestamp so U16 can refine
/// (visible arming affordance, tuned window) without reshaping the seam.
///
/// Semantics: over a focused remote session the FIRST swipe on an edge only
/// **arms** that edge (no intent); a second swipe on the SAME edge within
/// [`EDGE_CONFIRM_WINDOW`] **confirms** and fires. A different edge, or an
/// expired window, re-arms. Leaving the remote session clears the guard so a
/// stale arm never leaks into normal desktop use.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct EdgeGuard {
    /// The edge armed by a first swipe over a remote session, and when.
    armed: Option<(Edge, Duration)>,
}

impl EdgeGuard {
    /// Whether `edge` may fire its intent this frame. Pure given
    /// `(self, edge, remote_session_focused, now)` — the ONE place the dwell
    /// rule lives.
    fn admit(&mut self, edge: Edge, remote_session_focused: bool, now: Duration) -> bool {
        if !remote_session_focused {
            self.armed = None;
            return true;
        }
        match self.armed.take() {
            Some((armed_edge, at))
                if armed_edge == edge && now.saturating_sub(at) <= EDGE_CONFIRM_WINDOW =>
            {
                true
            }
            _ => {
                self.armed = Some((edge, now));
                false
            }
        }
    }
}

/// THE input contract dispatcher — the §2.3 table as one pure fold.
///
// PLATFORM-INTERFACES §2.3 (Q11) — the locked contract table:
//
// | Intent              | Touch                      | Pointer / keys            |
// |---------------------|----------------------------|---------------------------|
// | Home                | bottom-edge swipe up       | Super tap (app expanded)  |
// | App switcher        | bottom-edge swipe up+hold¹ | Super+Tab                 |
// | Spotlight           | pull-down on home²         | Super tap (on home)       |
// | Control Center      | top-right pull-down        | status-bar right cluster³ |
// | Notification Center | top-left/center pull-down  | status-bar clock click³   |
//
// ¹ the side channel carries no hold signal yet — U16 follow-up (module doc);
//   the Super+Tab row is live today.
// ² the on-home pull-down is the U10 springboard's own gesture (it owns the
//   home scroll surface); the Super-on-home row is live today.
// ³ the status-bar click rows land with U11, which PUSHES these intents into
//   the same [`ConstructChrome`] queue — no second dispatch path.
///
/// Determinism: chords first (Super tap, then Super+Tab), then edges in drain
/// order. The Super overload resolves on `app_expanded` (module doc). Edge
/// intents pass the [`EdgeGuard`] dwell rule; Super chords never consult it.
#[must_use]
pub fn intents_from_input(input: &ChromeInput, guard: &mut EdgeGuard) -> Vec<ChromeIntent> {
    let mut out = Vec::new();
    if input.super_tap {
        out.push(if input.app_expanded {
            ChromeIntent::Home
        } else {
            ChromeIntent::Spotlight
        });
    }
    if input.super_tab {
        out.push(ChromeIntent::Switcher);
    }
    for swipe in &input.edges {
        let Some(intent) = edge_intent(swipe) else {
            continue;
        };
        if guard.admit(swipe.edge, input.remote_session_focused, input.now) {
            out.push(intent);
        }
    }
    out
}

/// The touch column of the contract table: which intent an edge swipe means.
/// `Left`/`Right` carry no §2.3 row — `Left` keeps its legacy dock-reveal leg
/// inline at the drain site until the U29 cutover retires it; `Right` stays
/// unbound.
fn edge_intent(swipe: &EdgeSwipe) -> Option<ChromeIntent> {
    match swipe.edge {
        // Swipe-up-hold → Switcher is the U16 follow-up (no hold signal on
        // the channel yet, module doc) — until then bottom always means Home.
        Edge::Bottom => Some(ChromeIntent::Home),
        Edge::Top => Some(if swipe.x_frac.is_some_and(|x| x >= TOP_RIGHT_THIRD) {
            ChromeIntent::ControlCenter
        } else {
            ChromeIntent::NotificationCenter
        }),
        Edge::Left | Edge::Right => None,
    }
}

/// The Construct chrome's shared state: the three overlay open flags, the
/// VDI edge-dwell guard, and the per-frame intent queue the five mount slots
/// consume ([`Self::take_intent`]). Owned by the `Shell`; the U10–U15 units
/// read/write it from their own files without touching `main.rs`.
#[derive(Debug)]
pub struct ConstructChrome {
    /// The app switcher is showing (Q16, U12). The springboard and status bar
    /// are *persistent* chrome — they carry no open flag by design (§2.3).
    pub switcher_open: bool,
    /// The Control Center sheet is showing (Q13, U13).
    pub control_center_open: bool,
    /// The Notification Center pull-down is showing (Q14, U15).
    pub notification_center_open: bool,
    /// The VDI two-swipe dwell guard (§2.3).
    guard: EdgeGuard,
    /// The monotonic epoch [`Self::now`] measures the dwell window against.
    epoch: Instant,
    /// This frame's dispatched intents, queued by [`Self::dispatch`] and
    /// drained by the mount slots ([`Self::take_intent`]). Every intent has
    /// exactly one consumer each frame, so the queue never carries over.
    pending: Vec<ChromeIntent>,
}

impl Default for ConstructChrome {
    fn default() -> Self {
        Self {
            switcher_open: false,
            control_center_open: false,
            notification_center_open: false,
            guard: EdgeGuard::default(),
            epoch: Instant::now(),
            pending: Vec::new(),
        }
    }
}

impl ConstructChrome {
    /// The shell's monotonic clock for [`ChromeInput::now`] — elapsed since
    /// this chrome was built (never wall time; drives only the dwell window).
    #[must_use]
    pub fn now(&self) -> Duration {
        self.epoch.elapsed()
    }

    /// Fold one frame's decoded input through [`intents_from_input`] and
    /// queue the results for the mount slots. The caller (the shell's render)
    /// gates this on the curtain — a locked seat dispatches nothing, and the
    /// raw latches are drained by the caller regardless so nothing backs up.
    pub fn dispatch(&mut self, input: &ChromeInput) {
        let intents = intents_from_input(input, &mut self.guard);
        self.pending.extend(intents);
    }

    /// Drain every queued instance of `intent`, reporting whether any fired —
    /// the ONE consume seam each mount slot calls for its own intent.
    #[must_use]
    pub fn take_intent(&mut self, intent: ChromeIntent) -> bool {
        let before = self.pending.len();
        self.pending.retain(|i| *i != intent);
        self.pending.len() != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A quiet frame: no chords, no swipes, on home, no remote session.
    fn input() -> ChromeInput {
        ChromeInput {
            super_tap: false,
            super_tab: false,
            app_expanded: false,
            remote_session_focused: false,
            edges: Vec::new(),
            now: Duration::ZERO,
        }
    }

    fn swipe(edge: Edge, x_frac: Option<f32>) -> EdgeSwipe {
        EdgeSwipe { edge, x_frac }
    }

    // --- the contract rows, one by one (§2.3) ------------------------------------

    #[test]
    fn a_bottom_edge_swipe_means_home() {
        let mut guard = EdgeGuard::default();
        let intents = intents_from_input(
            &ChromeInput {
                edges: vec![swipe(Edge::Bottom, Some(0.5))],
                ..input()
            },
            &mut guard,
        );
        assert_eq!(intents, vec![ChromeIntent::Home]);
    }

    #[test]
    fn super_tab_means_switcher() {
        let mut guard = EdgeGuard::default();
        let intents = intents_from_input(
            &ChromeInput {
                super_tab: true,
                ..input()
            },
            &mut guard,
        );
        assert_eq!(intents, vec![ChromeIntent::Switcher]);
    }

    #[test]
    fn a_top_right_third_pull_means_control_center() {
        let mut guard = EdgeGuard::default();
        let intents = intents_from_input(
            &ChromeInput {
                edges: vec![swipe(Edge::Top, Some(0.9))],
                ..input()
            },
            &mut guard,
        );
        assert_eq!(intents, vec![ChromeIntent::ControlCenter]);
        // Exactly at the boundary counts as the right cluster (>=).
        let intents = intents_from_input(
            &ChromeInput {
                edges: vec![swipe(Edge::Top, Some(TOP_RIGHT_THIRD))],
                ..input()
            },
            &mut guard,
        );
        assert_eq!(intents, vec![ChromeIntent::ControlCenter]);
    }

    #[test]
    fn a_top_left_or_center_pull_means_notification_center() {
        let mut guard = EdgeGuard::default();
        for x in [Some(0.0), Some(0.5), Some(0.66)] {
            let intents = intents_from_input(
                &ChromeInput {
                    edges: vec![swipe(Edge::Top, x)],
                    ..input()
                },
                &mut guard,
            );
            assert_eq!(
                intents,
                vec![ChromeIntent::NotificationCenter],
                "top pull at {x:?}"
            );
        }
    }

    #[test]
    fn a_top_pull_with_no_pointer_fix_resolves_to_notification_center_honestly() {
        // No x → the wider top-left/center target, never a guessed Control
        // Center (the module doc's "resolve honestly" rule).
        let mut guard = EdgeGuard::default();
        let intents = intents_from_input(
            &ChromeInput {
                edges: vec![swipe(Edge::Top, None)],
                ..input()
            },
            &mut guard,
        );
        assert_eq!(intents, vec![ChromeIntent::NotificationCenter]);
    }

    #[test]
    fn left_and_right_edges_carry_no_contract_intent() {
        // Left keeps its legacy dock-reveal leg inline at the drain site
        // until U29; Right is unbound. Neither reaches the intent queue.
        let mut guard = EdgeGuard::default();
        let intents = intents_from_input(
            &ChromeInput {
                edges: vec![swipe(Edge::Left, Some(0.0)), swipe(Edge::Right, Some(1.0))],
                ..input()
            },
            &mut guard,
        );
        assert!(intents.is_empty(), "{intents:?}");
    }

    // --- the Super overload resolution (module doc) ------------------------------

    #[test]
    fn super_tap_with_an_app_expanded_means_home() {
        let mut guard = EdgeGuard::default();
        let intents = intents_from_input(
            &ChromeInput {
                super_tap: true,
                app_expanded: true,
                ..input()
            },
            &mut guard,
        );
        assert_eq!(intents, vec![ChromeIntent::Home]);
    }

    #[test]
    fn super_tap_on_home_means_spotlight() {
        let mut guard = EdgeGuard::default();
        let intents = intents_from_input(
            &ChromeInput {
                super_tap: true,
                app_expanded: false,
                ..input()
            },
            &mut guard,
        );
        assert_eq!(intents, vec![ChromeIntent::Spotlight]);
    }

    // --- the VDI dwell guard (§2.3) ----------------------------------------------

    #[test]
    fn over_a_remote_session_the_first_swipe_only_arms() {
        let mut guard = EdgeGuard::default();
        let intents = intents_from_input(
            &ChromeInput {
                remote_session_focused: true,
                edges: vec![swipe(Edge::Bottom, Some(0.5))],
                ..input()
            },
            &mut guard,
        );
        assert!(intents.is_empty(), "the first swipe arms, never fires");
        assert!(guard.armed.is_some(), "the edge is armed for the confirm");
    }

    #[test]
    fn a_same_edge_confirm_swipe_within_the_window_fires() {
        let mut guard = EdgeGuard::default();
        let armed = ChromeInput {
            remote_session_focused: true,
            edges: vec![swipe(Edge::Bottom, Some(0.5))],
            ..input()
        };
        assert!(intents_from_input(&armed, &mut guard).is_empty());
        let confirm = ChromeInput {
            now: EDGE_CONFIRM_WINDOW / 2,
            ..armed
        };
        assert_eq!(
            intents_from_input(&confirm, &mut guard),
            vec![ChromeIntent::Home],
            "the second same-edge swipe within the window confirms"
        );
        assert!(guard.armed.is_none(), "a confirm consumes the arm");
    }

    #[test]
    fn an_expired_or_cross_edge_confirm_re_arms_instead_of_firing() {
        // Expired window: the late swipe re-arms.
        let mut guard = EdgeGuard::default();
        let armed = ChromeInput {
            remote_session_focused: true,
            edges: vec![swipe(Edge::Bottom, Some(0.5))],
            ..input()
        };
        assert!(intents_from_input(&armed, &mut guard).is_empty());
        let late = ChromeInput {
            now: EDGE_CONFIRM_WINDOW + Duration::from_millis(1),
            ..armed.clone()
        };
        assert!(
            intents_from_input(&late, &mut guard).is_empty(),
            "an expired confirm re-arms, never fires"
        );
        // Cross-edge: a top pull after arming bottom re-arms on top.
        let cross = ChromeInput {
            edges: vec![swipe(Edge::Top, Some(0.9))],
            ..armed
        };
        assert!(
            intents_from_input(&cross, &mut guard).is_empty(),
            "a different edge re-arms, never fires"
        );
        assert_eq!(guard.armed.map(|(e, _)| e), Some(Edge::Top));
    }

    #[test]
    fn super_chords_always_pass_the_vdi_guard() {
        let mut guard = EdgeGuard::default();
        let intents = intents_from_input(
            &ChromeInput {
                super_tap: true,
                super_tab: true,
                app_expanded: true,
                remote_session_focused: true,
                ..input()
            },
            &mut guard,
        );
        assert_eq!(
            intents,
            vec![ChromeIntent::Home, ChromeIntent::Switcher],
            "§2.3: Super chords always work over a focused session"
        );
    }

    #[test]
    fn leaving_the_remote_session_clears_a_stale_arm() {
        let mut guard = EdgeGuard::default();
        let armed = ChromeInput {
            remote_session_focused: true,
            edges: vec![swipe(Edge::Bottom, Some(0.5))],
            ..input()
        };
        assert!(intents_from_input(&armed, &mut guard).is_empty());
        // Back on the desktop: the swipe fires directly AND the arm clears.
        let desktop = ChromeInput {
            remote_session_focused: false,
            ..armed
        };
        assert_eq!(
            intents_from_input(&desktop, &mut guard),
            vec![ChromeIntent::Home]
        );
        assert!(guard.armed.is_none(), "no stale arm off the session");
    }

    // --- the queue seam ----------------------------------------------------------

    #[test]
    fn take_intent_drains_exactly_its_own_intent_once() {
        let mut chrome = ConstructChrome::default();
        chrome.dispatch(&ChromeInput {
            super_tab: true,
            edges: vec![swipe(Edge::Top, Some(0.9))],
            ..input()
        });
        assert!(chrome.take_intent(ChromeIntent::Switcher));
        assert!(
            !chrome.take_intent(ChromeIntent::Switcher),
            "an intent drains exactly once"
        );
        assert!(
            !chrome.take_intent(ChromeIntent::Home),
            "an intent that never fired never reports"
        );
        assert!(chrome.take_intent(ChromeIntent::ControlCenter));
        assert!(chrome.pending.is_empty(), "every intent has one consumer");
    }
}
