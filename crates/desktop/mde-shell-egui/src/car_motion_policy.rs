//! `car_motion_policy` — WL-UX-007 U28: the **soft in-motion limits** fold
//! (PLATFORM-INTERFACES §3.3 Q35).
//!
//! When the MG90-reported speed exceeds a walking-pace threshold while the Car
//! profile is active, the shell softens itself: the OSK stops auto-raising,
//! car-path lists clamp to a glance count, and destructive confirmations defer
//! until the vehicle stops. **No hard lockouts** — every limit is a soft
//! default; physical keys and the manual OSK raise keep working (Q35: the
//! keyboard-first stance is the safety model, so the driver is never locked
//! out of a control).
//!
//! **Honesty (P8):** the limits engage only on a LIVE speed
//! ([`mde_maps_location_egui::car_status::live_speed_mph`] — `None` for the
//! simulated CAN/OBD seed or an offline adapter). No live speed ⇒ no limits,
//! ever: a simulator can never soft-lock the cockpit.
//!
//! Two halves, both pure/headless-testable:
//!
//! 1. [`CarMotionPolicy::next`] — the per-frame fold with hysteresis (engage
//!    at [`CAR_MOTION_THRESHOLD_MPH`], hold down to [`CAR_MOTION_RELEASE_MPH`])
//!    so a speed hovering at the threshold never flickers the UI.
//! 2. The [`egui::Context`] seam ([`publish`] → [`in_motion`] /
//!    [`glance_clamp`]) — `central_view` publishes the fold once per frame,
//!    before any consumer draws; any car-path renderer consults the ONE seam
//!    without new plumbing.

use mde_egui::egui::{self, RichText};
use mde_egui::{LayoutProfile, Style};

/// The in-motion **engage** threshold (Q35: "when MG90-reported speed exceeds
/// the threshold"). 5 mph is a walking-pace floor: below it the vehicle is
/// creeping in a lot or stopped in traffic and the full UI stays available;
/// at or above it the driver is genuinely underway and the soft limits engage.
pub const CAR_MOTION_THRESHOLD_MPH: f32 = 5.0;

/// The hysteresis **release** floor: once engaged, the limits hold until the
/// live speed falls *below* this (engage − 2 mph), so a speed hovering right
/// at [`CAR_MOTION_THRESHOLD_MPH`] doesn't flicker the OSK / list clamps on
/// and off frame-to-frame.
pub const CAR_MOTION_RELEASE_MPH: f32 = CAR_MOTION_THRESHOLD_MPH - 2.0;

/// While in motion, car-path lists clamp to this many rows — the glance count
/// (Q35: glance-range type, interaction depth ≤2 while moving; the same
/// six-item budget as the Auto home's app strip).
pub const CAR_GLANCE_LIST_MAX: usize = 6;

/// The deferred destructive-confirmation headline (Q35: "destructive prompts
/// defer until stopped").
pub const DEFERRED_NOTICE_TITLE: &str = "Stopped? Try again.";

/// The explainer line under the deferred-notice headline.
pub const DEFERRED_NOTICE_BODY: &str = "This action waits until the vehicle stops.";

/// The soft in-motion policy for one frame (PLATFORM-INTERFACES Q35).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CarMotionPolicy {
    /// Whether the soft limits are engaged this frame: Car profile active AND
    /// a LIVE MG90 speed at/above the motion threshold (with hysteresis).
    pub in_motion: bool,
}

impl CarMotionPolicy {
    /// Fold one frame of `(profile, live speed)` into the next policy.
    ///
    /// `in_motion` engages only when the profile is Car **and** the LIVE speed
    /// reaches [`CAR_MOTION_THRESHOLD_MPH`]; once engaged it holds until the
    /// speed drops below [`CAR_MOTION_RELEASE_MPH`] (hysteresis). A `None`
    /// speed — the simulated seed, an offline adapter, or no vehicle mirror at
    /// all — always disengages: the honesty rule (P8) is *no live speed = no
    /// limits*, so simulated telemetry can never soft-lock the cockpit.
    #[must_use]
    pub const fn next(self, profile: LayoutProfile, live_mph: Option<f32>) -> Self {
        let in_motion = profile.is_car()
            && match live_mph {
                // Honesty (P8): only a LIVE speed engages the limits.
                None => false,
                Some(mph) => {
                    if self.in_motion {
                        mph >= CAR_MOTION_RELEASE_MPH
                    } else {
                        mph >= CAR_MOTION_THRESHOLD_MPH
                    }
                }
            };
        Self { in_motion }
    }
}

/// The one Context key the published fold lives under for the frame.
fn motion_id() -> egui::Id {
    egui::Id::new("mcnf-car-motion-in-motion")
}

/// Publish this frame's fold to the [`egui::Context`] (temp memory — never
/// persisted). Called once per frame by `central_view`, before any consumer
/// (the OSK raise fold, a car-path list/confirm site) draws.
pub fn publish(ctx: &egui::Context, policy: CarMotionPolicy) {
    ctx.data_mut(|d| d.insert_temp(motion_id(), policy.in_motion));
}

/// Whether the soft in-motion limits are engaged this frame — the ONE seam a
/// car-path renderer consults. Absent (never published — e.g. a bare test
/// Context, or any non-shell embedding) reads `false`: no limits.
#[must_use]
pub fn in_motion(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(motion_id()))
        .unwrap_or(false)
}

/// Clamp a car-path list to the glance count while in motion (Q35: "lists
/// shorten"); the full length whenever stopped, off-Car, or without live
/// telemetry. No car-path surface renders a long list today — the Auto home
/// is fixed cards + a six-tile strip and the instrument strip a fixed readout
/// grid — so this seam awaits its first list-bearing car surface.
#[allow(dead_code)] // seam for the first long car-path list (none exist yet)
#[must_use]
pub fn glance_clamp(ctx: &egui::Context, full_len: usize) -> usize {
    if in_motion(ctx) {
        full_len.min(CAR_GLANCE_LIST_MAX)
    } else {
        full_len
    }
}

/// The deferred destructive-confirmation notice (Q35): a car-path confirm site
/// renders this **instead of** its prompt while [`in_motion`] — the action
/// simply waits for the vehicle to stop; nothing is armed, nothing fires. On
/// the kept SYNC3 tokens (Q30) so it reads as native Auto Mode chrome. No
/// car-path site can trigger a destructive prompt today (`apply_car_action`
/// is jumps/transport/volume only), so this seam awaits its first wire.
#[allow(dead_code)] // seam for the first car-path destructive confirm (none exist yet)
pub fn deferred_notice(ui: &mut egui::Ui) {
    egui::Frame::new()
        .fill(Style::SYNC3_SURFACE)
        .stroke(egui::Stroke::new(1.0, Style::SYNC3_BORDER))
        .corner_radius(egui::CornerRadius::same(Style::RADIUS_M as u8))
        .inner_margin(egui::Margin::same(Style::SP_M as i8))
        .show(ui, |ui| {
            ui.label(
                RichText::new(DEFERRED_NOTICE_TITLE)
                    .size(Style::TYPE_TITLE3)
                    .color(Style::SYNC3_TEXT_STRONG),
            );
            ui.label(
                RichText::new(DEFERRED_NOTICE_BODY)
                    .size(Style::TYPE_SUBHEADLINE)
                    .color(Style::SYNC3_TEXT_DIM),
            );
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn car(prev: bool, mph: Option<f32>) -> bool {
        CarMotionPolicy { in_motion: prev }
            .next(LayoutProfile::Car, mph)
            .in_motion
    }

    // --- the pure fold ---------------------------------------------------------------

    #[test]
    fn non_car_profile_never_engages_even_at_highway_speed() {
        for prev in [false, true] {
            let p = CarMotionPolicy { in_motion: prev };
            assert!(
                !p.next(LayoutProfile::Construct, Some(80.0)).in_motion,
                "Construct never engages the car limits (prev = {prev})"
            );
        }
    }

    #[test]
    fn no_live_speed_never_engages_and_always_releases() {
        // Honesty (P8): the simulated seed / an offline adapter fold to `None`
        // upstream (`live_speed_mph`) — no live speed = no limits, ever.
        assert!(!car(false, None), "None speed never engages");
        assert!(!car(true, None), "None speed releases an engaged policy");
    }

    #[test]
    fn engages_at_threshold_with_hysteresis_release_band() {
        assert!(!car(false, Some(4.9)), "below engage stays disengaged");
        assert!(car(false, Some(CAR_MOTION_THRESHOLD_MPH)), "5.0 engages");
        // Hovering inside the 3..5 band: an engaged policy HOLDS…
        assert!(car(true, Some(3.5)), "engaged holds through the band");
        assert!(car(true, Some(CAR_MOTION_RELEASE_MPH)), "3.0 still holds");
        // …a disengaged one does NOT re-engage (no flicker).
        assert!(!car(false, Some(3.5)), "the band never re-engages");
        assert!(!car(true, Some(2.9)), "below release disengages");
    }

    #[test]
    fn leaving_car_profile_disengages_immediately() {
        let p = CarMotionPolicy { in_motion: true };
        assert!(!p.next(LayoutProfile::Construct, Some(60.0)).in_motion);
    }

    // --- the Context seam ------------------------------------------------------------

    #[test]
    fn ctx_roundtrip_and_absent_reads_no_limits() {
        let ctx = egui::Context::default();
        assert!(!in_motion(&ctx), "never-published reads false (no limits)");
        publish(&ctx, CarMotionPolicy { in_motion: true });
        assert!(in_motion(&ctx));
        publish(&ctx, CarMotionPolicy { in_motion: false });
        assert!(!in_motion(&ctx));
    }

    #[test]
    fn glance_clamp_shortens_only_while_in_motion() {
        let ctx = egui::Context::default();
        assert_eq!(glance_clamp(&ctx, 20), 20, "stopped: full length");
        publish(&ctx, CarMotionPolicy { in_motion: true });
        assert_eq!(glance_clamp(&ctx, 20), CAR_GLANCE_LIST_MAX);
        assert_eq!(glance_clamp(&ctx, 3), 3, "short lists pass through");
    }

    #[test]
    fn deferred_notice_paints_the_stopped_prompt() {
        let ctx = egui::Context::default();
        let out = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| deferred_notice(ui));
        });
        fn walk(shape: &egui::Shape, out: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(t) => out.push(t.galley.text().to_owned()),
                egui::Shape::Vec(inner) => {
                    for s in inner {
                        walk(s, out);
                    }
                }
                _ => {}
            }
        }
        let mut texts = Vec::new();
        for clipped in &out.shapes {
            walk(&clipped.shape, &mut texts);
        }
        let painted = texts.join("\n");
        assert!(
            painted.contains(DEFERRED_NOTICE_TITLE),
            "the deferred notice paints its headline; painted: {painted:?}"
        );
    }
}
