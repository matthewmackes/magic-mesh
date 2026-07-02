//! `touch` — the DRM seat's **touchscreen input** model (SURFACE-8, design lock 13).
//!
//! The bare-DRM seat ([`crate::drm::run_drm`]) owns the kernel evdev stream directly
//! (E12 no-compositor), so it — not a Wayland compositor — must turn raw multitouch
//! contacts into egui events. This module is the **pure, headless-testable core** of
//! that: the coordinate transform (active mode × fractional scale × rotation) and the
//! contact→egui-event translation are ordinary functions with fixtures. The one
//! hardware-bound step — reading the touchscreen off libinput — lives in
//! [`crate::drm`] behind `feature = "drm"` and honestly degrades headless (no faked
//! touch), feeding [`RawContact`]s into the [`TouchTranslator`] here.
//!
//! **Pipeline.** libinput reports each contact as a normalized position in the
//! *unrotated panel* (via `x_transformed(mode_w)/mode_w`), a per-contact **slot**
//! (multitouch), and a phase (down / move / up / cancel). The seat feeds those as
//! [`RawContact`]s; the translator:
//!
//! 1. maps the normalized coord through the active mode + fractional scale + the
//!    current [`Rotation`] to a **logical egui point** ([`TouchTransform::to_points`]),
//! 2. emits an egui [`egui::Event::Touch`] for **every** contact (so 2+-finger
//!    gestures survive intact for SURFACE-11), and
//! 3. for the **first / primary** contact only, synthesizes pointer events
//!    (`PointerMoved` + `PointerButton` press/release) so existing click/drag
//!    handlers work under a single finger — mirroring egui-winit's touch→pointer
//!    behaviour, so the seat has **one input pipeline** for kbd/mouse/touch.
//!
//! **Rotation** (design lock 13 / 15) is taken as an *input*: SURFACE-9 will drive the
//! live KMS rotation value; SURFACE-8 takes it via [`TouchTranslator::set_rotation`]
//! so taps land correctly in every orientation. The touch matrix pairs with the
//! DRM/xrandr rotate property **of the same name** (the standard coordinate-transform
//! matrix), so rotating the display and the touch surface together keeps taps aligned.

use std::collections::BTreeMap;

/// The display content's rotation, applied to both the KMS scanout (SURFACE-9) and,
/// here, the touch coordinate transform so the two rotate as one.
///
/// Each variant is a **clockwise** rotation of the displayed content and pairs with
/// the DRM/xrandr rotate property of the same magnitude; the normalized-coordinate
/// matrices in [`Rotation::apply_norm`] are the standard xrandr coordinate-transform
/// matrices, so a tap lands on the pixel it visually hits in every orientation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Rotation {
    /// No rotation — landscape, the panel's native orientation.
    #[default]
    None,
    /// 90° clockwise (xrandr `right`).
    Rotate90,
    /// 180° (xrandr `inverted`).
    Rotate180,
    /// 270° clockwise / 90° counter-clockwise (xrandr `left`).
    Rotate270,
}

impl Rotation {
    /// Rotate a normalized `(u, v)` coordinate (each in `[0, 1]`, in the *unrotated*
    /// panel) into the rotated framebuffer's normalized space.
    ///
    /// These are the xrandr coordinate-transformation matrices, evaluated in the unit
    /// square — the industry-standard pairing for a rotated touch surface.
    #[must_use]
    pub fn apply_norm(self, u: f32, v: f32) -> (f32, f32) {
        match self {
            Self::None => (u, v),
            Self::Rotate90 => (v, 1.0 - u),
            Self::Rotate180 => (1.0 - u, 1.0 - v),
            Self::Rotate270 => (1.0 - v, u),
        }
    }

    /// The rotated framebuffer's pixel dimensions given the unrotated mode size:
    /// unchanged for 0°/180°, swapped (portrait) for 90°/270°.
    #[must_use]
    pub const fn output_pixels(self, mode_w: u32, mode_h: u32) -> (u32, u32) {
        match self {
            Self::None | Self::Rotate180 => (mode_w, mode_h),
            Self::Rotate90 | Self::Rotate270 => (mode_h, mode_w),
        }
    }
}

/// The coordinate transform from a normalized digitizer position to a logical egui
/// point.
///
/// It folds the active mode, the fractional `HiDPI` scale (SURFACE-7's
/// `pixels_per_point`), and the current rotation into one mapping.
///
/// Pure + `Copy`: the seat rebuilds one when the mode/scale changes and mutates
/// [`rotation`](Self::rotation) when SURFACE-9 rotates the display.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TouchTransform {
    /// Active scanout mode width in **physical pixels** (unrotated).
    pub mode_w: u32,
    /// Active scanout mode height in **physical pixels** (unrotated).
    pub mode_h: u32,
    /// egui `pixels_per_point` — the fractional `HiDPI` scale (SURFACE-7 lock 11).
    pub scale: f32,
    /// The current display rotation (SURFACE-9 drives it; taken as input here).
    pub rotation: Rotation,
}

impl TouchTransform {
    /// Build a transform for an active mode + fractional scale, starting unrotated.
    #[must_use]
    pub const fn new(mode_w: u32, mode_h: u32, scale: f32) -> Self {
        Self {
            mode_w,
            mode_h,
            scale,
            rotation: Rotation::None,
        }
    }

    /// Map a normalized digitizer coordinate `(u, v)` (each `[0, 1]`, unrotated panel)
    /// to a logical egui point in the rotated, scaled coordinate space the UI lays out
    /// in. With `Rotation::None` and `scale == 1.0` this is `(u·mode_w, v·mode_h)`.
    #[must_use]
    #[allow(clippy::cast_precision_loss)] // mode dims are display sizes (< ~16k); f32-exact
    pub fn to_points(&self, u: f32, v: f32) -> egui::Pos2 {
        let (ru, rv) = self.rotation.apply_norm(u, v);
        let (fb_w, fb_h) = self.rotation.output_pixels(self.mode_w, self.mode_h);
        // Guard a degenerate scale so a bad panel report never divides by zero.
        let scale = if self.scale > f32::EPSILON {
            self.scale
        } else {
            1.0
        };
        egui::pos2(ru * fb_w as f32 / scale, rv * fb_h as f32 / scale)
    }
}

/// One raw multitouch contact event off the seat's evdev/libinput stream.
///
/// Positions are in the digitizer's **normalized** coordinate space (unrotated panel),
/// tagged with the per-contact slot so multitouch is preserved.
///
/// `Up`/`Cancel` carry no position — libinput's touch-up event has none — so the
/// translator uses the contact's last known position for the closing egui event.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RawContact {
    /// A new contact touched down at normalized `(u, v)`, with optional pressure.
    Down {
        /// The multitouch slot (per-contact tracking id).
        slot: u32,
        /// Normalized x in `[0, 1]` (unrotated panel).
        u: f32,
        /// Normalized y in `[0, 1]` (unrotated panel).
        v: f32,
        /// Contact pressure/force in `[0, 1]` if the digitizer reports it.
        force: Option<f32>,
    },
    /// An existing contact moved to normalized `(u, v)`.
    Move {
        /// The multitouch slot.
        slot: u32,
        /// Normalized x in `[0, 1]`.
        u: f32,
        /// Normalized y in `[0, 1]`.
        v: f32,
        /// Contact pressure/force in `[0, 1]` if reported.
        force: Option<f32>,
    },
    /// A contact lifted (no position — the last known one is used).
    Up {
        /// The multitouch slot that lifted.
        slot: u32,
    },
    /// A contact was cancelled (palm rejection / gesture abort).
    Cancel {
        /// The multitouch slot that was cancelled.
        slot: u32,
    },
}

/// The single seat touch device id. The bare seat exposes one logical touch device,
/// so every contact shares it; egui only uses it to group contacts by device.
const SEAT_TOUCH_DEVICE: egui::TouchDeviceId = egui::TouchDeviceId(0);

/// Stateful translator from raw multitouch contacts to egui events (design lock 13).
///
/// Owns the coordinate [`TouchTransform`] and the small per-contact state needed to
/// (a) synthesize a closing position for `Up`/`Cancel` and (b) drive **one** primary
/// pointer from the first active finger. Every contact still emits an
/// [`egui::Event::Touch`], so 2+-finger gestures (SURFACE-11) pass through untouched.
#[derive(Debug, Clone)]
pub struct TouchTranslator {
    transform: TouchTransform,
    /// Last logical position per active slot — the closing pos for `Up`/`Cancel`.
    slots: BTreeMap<u32, egui::Pos2>,
    /// The slot currently driving the synthesized pointer (the first finger down);
    /// `None` when no finger is the primary pointer. Mirrors egui-winit.
    pointer_slot: Option<u32>,
}

impl TouchTranslator {
    /// Build a translator over a coordinate transform (active mode + scale + rotation).
    #[must_use]
    pub const fn new(transform: TouchTransform) -> Self {
        Self {
            transform,
            slots: BTreeMap::new(),
            pointer_slot: None,
        }
    }

    /// The current transform (for the seat to read the active rotation/scale back).
    #[must_use]
    pub const fn transform(&self) -> &TouchTransform {
        &self.transform
    }

    /// Replace the coordinate transform (the seat calls this on a mode/scale change).
    pub const fn set_transform(&mut self, transform: TouchTransform) {
        self.transform = transform;
    }

    /// Update just the rotation (SURFACE-9's auto-rotate hook — the display and the
    /// touch matrix rotate as one; in-flight contacts keep their prior slot state).
    pub const fn set_rotation(&mut self, rotation: Rotation) {
        self.transform.rotation = rotation;
    }

    /// Whether a synthesized pointer is currently down (the primary finger is active).
    #[must_use]
    pub const fn pointer_active(&self) -> bool {
        self.pointer_slot.is_some()
    }

    /// Translate one raw contact into egui events, appending them to `out`.
    ///
    /// Every contact appends an [`egui::Event::Touch`]; the **primary** contact (the
    /// first finger down, tracked until it lifts) additionally appends the synthesized
    /// `PointerMoved` / `PointerButton` / `PointerGone` sequence, so a single-finger
    /// tap or drag drives ordinary click handlers while multitouch stays intact.
    pub fn feed(&mut self, contact: RawContact, out: &mut Vec<egui::Event>) {
        match contact {
            RawContact::Down { slot, u, v, force } => {
                let pos = self.transform.to_points(u, v);
                self.slots.insert(slot, pos);
                out.push(touch_event(slot, egui::TouchPhase::Start, pos, force));
                // First finger down becomes the primary pointer (egui-winit idiom):
                // move there, then press. A 2nd+ finger only emits Touch (gesture).
                if self.pointer_slot.is_none() {
                    self.pointer_slot = Some(slot);
                    out.push(egui::Event::PointerMoved(pos));
                    out.push(pointer_button(pos, true));
                }
            }
            RawContact::Move { slot, u, v, force } => {
                let pos = self.transform.to_points(u, v);
                self.slots.insert(slot, pos);
                out.push(touch_event(slot, egui::TouchPhase::Move, pos, force));
                if self.pointer_slot == Some(slot) {
                    out.push(egui::Event::PointerMoved(pos));
                }
            }
            RawContact::Up { slot } => {
                let pos = self.slots.remove(&slot).unwrap_or_default();
                out.push(touch_event(slot, egui::TouchPhase::End, pos, None));
                self.end_pointer_if_primary(slot, pos, out);
            }
            RawContact::Cancel { slot } => {
                let pos = self.slots.remove(&slot).unwrap_or_default();
                out.push(touch_event(slot, egui::TouchPhase::Cancel, pos, None));
                self.end_pointer_if_primary(slot, pos, out);
            }
        }
    }

    /// Release the synthesized pointer when the primary finger lifts/cancels.
    fn end_pointer_if_primary(&mut self, slot: u32, pos: egui::Pos2, out: &mut Vec<egui::Event>) {
        if self.pointer_slot == Some(slot) {
            self.pointer_slot = None;
            out.push(pointer_button(pos, false));
            // Tell egui the pointer left — a touch pointer has no hover position, so
            // it should not keep hovering the last widget after the finger lifts.
            out.push(egui::Event::PointerGone);
        }
    }
}

/// Build an egui touch event for a slot/phase/position.
fn touch_event(
    slot: u32,
    phase: egui::TouchPhase,
    pos: egui::Pos2,
    force: Option<f32>,
) -> egui::Event {
    egui::Event::Touch {
        device_id: SEAT_TOUCH_DEVICE,
        id: egui::TouchId(u64::from(slot)),
        phase,
        pos,
        force,
    }
}

/// Build a synthesized primary-pointer button event (touch → mouse click).
fn pointer_button(pos: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    // --- coordinate transform: mode × scale × rotation ---------------------------

    #[test]
    fn transform_unrotated_unscaled_is_identity_in_pixels() {
        let t = TouchTransform::new(2000, 1000, 1.0);
        let p = t.to_points(0.5, 0.5);
        assert!(approx(p.x, 1000.0) && approx(p.y, 500.0), "{p:?}");
        // Corners.
        assert!(approx(t.to_points(0.0, 0.0).x, 0.0));
        assert!(approx(t.to_points(1.0, 1.0).x, 2000.0));
        assert!(approx(t.to_points(1.0, 1.0).y, 1000.0));
    }

    #[test]
    fn transform_applies_fractional_scale() {
        // SURFACE-7's ~2.25 HiDPI scale: a native-pixel touch lands at pixel/scale
        // in logical points, matching the run_drm screen_rect (wp/ppp, hp/ppp).
        let t = TouchTransform::new(2880, 1920, 2.25);
        let p = t.to_points(1.0, 1.0);
        assert!(approx(p.x, 2880.0 / 2.25), "{p:?}");
        assert!(approx(p.y, 1920.0 / 2.25), "{p:?}");
    }

    #[test]
    fn transform_rotations_map_corners_consistently() {
        // 2000×1000 landscape panel. Track the panel top-left contact (u=0,v=0)
        // through each rotation; the logical point must sit at the rotated
        // framebuffer's expected corner (xrandr CTM convention).
        let base = TouchTransform::new(2000, 1000, 1.0);

        // None: top-left → (0,0).
        let mut t = base;
        t.rotation = Rotation::None;
        let p = t.to_points(0.0, 0.0);
        assert!(approx(p.x, 0.0) && approx(p.y, 0.0), "none {p:?}");

        // 90° CW: portrait fb 1000×2000; (0,0) → (0, 2000).
        t.rotation = Rotation::Rotate90;
        let p = t.to_points(0.0, 0.0);
        assert!(approx(p.x, 0.0) && approx(p.y, 2000.0), "r90 {p:?}");
        // fb is 1000 wide, 2000 tall — the far corner is reachable and bounded.
        let far = t.to_points(1.0, 1.0);
        assert!(
            approx(far.x, 1000.0) && approx(far.y, 0.0),
            "r90 far {far:?}"
        );

        // 180°: (0,0) → (2000, 1000) (opposite corner), landscape dims unchanged.
        t.rotation = Rotation::Rotate180;
        let p = t.to_points(0.0, 0.0);
        assert!(approx(p.x, 2000.0) && approx(p.y, 1000.0), "r180 {p:?}");

        // 270° CW: portrait fb 1000×2000; (0,0) → (1000, 0).
        t.rotation = Rotation::Rotate270;
        let p = t.to_points(0.0, 0.0);
        assert!(approx(p.x, 1000.0) && approx(p.y, 0.0), "r270 {p:?}");
    }

    #[test]
    fn rotations_are_bijective_on_the_unit_square() {
        // Every rotation keeps a normalized coord in [0,1]² (no contact escapes the
        // framebuffer), and the center is fixed under all four.
        for rot in [
            Rotation::None,
            Rotation::Rotate90,
            Rotation::Rotate180,
            Rotation::Rotate270,
        ] {
            let (u, v) = rot.apply_norm(0.5, 0.5);
            assert!(approx(u, 0.5) && approx(v, 0.5), "center {rot:?}");
            for &(iu, iv) in &[(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0), (0.3, 0.7)] {
                let (ou, ov) = rot.apply_norm(iu, iv);
                assert!(
                    (0.0..=1.0).contains(&ou) && (0.0..=1.0).contains(&ov),
                    "{rot:?}"
                );
            }
        }
    }

    #[test]
    fn output_pixels_swaps_only_on_quarter_turns() {
        assert_eq!(Rotation::None.output_pixels(2000, 1000), (2000, 1000));
        assert_eq!(Rotation::Rotate180.output_pixels(2000, 1000), (2000, 1000));
        assert_eq!(Rotation::Rotate90.output_pixels(2000, 1000), (1000, 2000));
        assert_eq!(Rotation::Rotate270.output_pixels(2000, 1000), (1000, 2000));
    }

    #[test]
    fn transform_guards_degenerate_scale() {
        // A bad panel report (scale 0) must not divide by zero / NaN.
        let t = TouchTransform::new(2000, 1000, 0.0);
        let p = t.to_points(0.5, 0.5);
        assert!(p.x.is_finite() && p.y.is_finite());
        assert!(approx(p.x, 1000.0));
    }

    // --- contact → egui event translation ----------------------------------------

    /// Extract the phases of every emitted Touch event, in order.
    fn touch_phases(evs: &[egui::Event]) -> Vec<egui::TouchPhase> {
        evs.iter()
            .filter_map(|e| match e {
                egui::Event::Touch { phase, .. } => Some(*phase),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn single_touch_synthesizes_tap_pointer_sequence() {
        let mut tr = TouchTranslator::new(TouchTransform::new(1000, 1000, 1.0));
        let mut out = Vec::new();
        tr.feed(
            RawContact::Down {
                slot: 0,
                u: 0.5,
                v: 0.5,
                force: Some(0.9),
            },
            &mut out,
        );
        tr.feed(RawContact::Up { slot: 0 }, &mut out);

        // Touch Start + End, and a full pointer press→release with a PointerGone.
        assert_eq!(
            touch_phases(&out),
            vec![egui::TouchPhase::Start, egui::TouchPhase::End]
        );
        let presses: Vec<bool> = out
            .iter()
            .filter_map(|e| match e {
                egui::Event::PointerButton {
                    pressed, button, ..
                } if *button == egui::PointerButton::Primary => Some(*pressed),
                _ => None,
            })
            .collect();
        assert_eq!(presses, vec![true, false], "press then release");
        assert!(
            out.iter().any(|e| matches!(e, egui::Event::PointerGone)),
            "pointer leaves on lift"
        );
        // The synthesized press sits at the transformed center (500,500).
        let pressed_at = out.iter().find_map(|e| match e {
            egui::Event::PointerButton {
                pos, pressed: true, ..
            } => Some(*pos),
            _ => None,
        });
        assert_eq!(pressed_at, Some(egui::pos2(500.0, 500.0)));
        // Force is carried on the Touch event.
        let force = out.iter().find_map(|e| match e {
            egui::Event::Touch { force, .. } => Some(*force),
            _ => None,
        });
        assert_eq!(force, Some(Some(0.9)));
    }

    #[test]
    fn single_touch_drag_moves_pointer() {
        let mut tr = TouchTranslator::new(TouchTransform::new(1000, 1000, 1.0));
        let mut out = Vec::new();
        tr.feed(
            RawContact::Down {
                slot: 0,
                u: 0.1,
                v: 0.1,
                force: None,
            },
            &mut out,
        );
        tr.feed(
            RawContact::Move {
                slot: 0,
                u: 0.2,
                v: 0.3,
                force: None,
            },
            &mut out,
        );
        tr.feed(RawContact::Up { slot: 0 }, &mut out);
        // The drag emits a PointerMoved to (200,300) between press and release.
        let moves: Vec<egui::Pos2> = out
            .iter()
            .filter_map(|e| match e {
                egui::Event::PointerMoved(p) => Some(*p),
                _ => None,
            })
            .collect();
        assert!(moves.contains(&egui::pos2(200.0, 300.0)), "{moves:?}");
        assert!(!tr.pointer_active(), "pointer released after up");
    }

    #[test]
    fn multitouch_preserves_all_contacts_and_suppresses_second_pointer() {
        // Two fingers: BOTH emit Touch events (gesture data survives), but only the
        // first drives the synthesized pointer (no phantom second click).
        let mut tr = TouchTranslator::new(TouchTransform::new(1000, 1000, 1.0));
        let mut out = Vec::new();
        tr.feed(
            RawContact::Down {
                slot: 0,
                u: 0.2,
                v: 0.2,
                force: None,
            },
            &mut out,
        );
        tr.feed(
            RawContact::Down {
                slot: 1,
                u: 0.8,
                v: 0.8,
                force: None,
            },
            &mut out,
        );
        tr.feed(
            RawContact::Move {
                slot: 1,
                u: 0.9,
                v: 0.9,
                force: None,
            },
            &mut out,
        );

        // Two distinct Touch ids present → multitouch preserved.
        let ids: std::collections::BTreeSet<u64> = out
            .iter()
            .filter_map(|e| match e {
                egui::Event::Touch { id, .. } => Some(id.0),
                _ => None,
            })
            .collect();
        assert_eq!(ids, [0, 1].into_iter().collect());

        // Exactly ONE pointer press (slot 0); slot 1's down/move added no pointer.
        let presses = out
            .iter()
            .filter(|e| matches!(e, egui::Event::PointerButton { pressed: true, .. }))
            .count();
        assert_eq!(presses, 1, "second finger must not click");
        // slot 1's move did not move the pointer (only slot 0 is primary).
        assert!(tr.pointer_active());
    }

    #[test]
    fn second_finger_becomes_primary_after_first_lifts() {
        // Lifting the primary finger while a second is down releases the pointer;
        // a NEW finger down then re-arms it (matches egui-winit's single-pointer rule).
        let mut tr = TouchTranslator::new(TouchTransform::new(1000, 1000, 1.0));
        let mut out = Vec::new();
        tr.feed(
            RawContact::Down {
                slot: 0,
                u: 0.2,
                v: 0.2,
                force: None,
            },
            &mut out,
        );
        tr.feed(
            RawContact::Down {
                slot: 1,
                u: 0.8,
                v: 0.8,
                force: None,
            },
            &mut out,
        );
        assert!(tr.pointer_active());
        tr.feed(RawContact::Up { slot: 0 }, &mut out); // primary lifts
        assert!(!tr.pointer_active(), "no primary while only slot 1 remains");
        tr.feed(
            RawContact::Down {
                slot: 2,
                u: 0.5,
                v: 0.5,
                force: None,
            },
            &mut out,
        );
        assert!(tr.pointer_active(), "new finger re-arms the pointer");
    }

    #[test]
    fn up_uses_last_known_position() {
        // libinput touch-up has no coordinate; the End Touch + release must land at
        // the contact's last moved position, not the origin.
        let mut tr = TouchTranslator::new(TouchTransform::new(1000, 1000, 1.0));
        let mut out = Vec::new();
        tr.feed(
            RawContact::Down {
                slot: 0,
                u: 0.1,
                v: 0.1,
                force: None,
            },
            &mut out,
        );
        tr.feed(
            RawContact::Move {
                slot: 0,
                u: 0.4,
                v: 0.6,
                force: None,
            },
            &mut out,
        );
        out.clear();
        tr.feed(RawContact::Up { slot: 0 }, &mut out);
        let end_pos = out.iter().find_map(|e| match e {
            egui::Event::Touch {
                phase: egui::TouchPhase::End,
                pos,
                ..
            } => Some(*pos),
            _ => None,
        });
        assert_eq!(end_pos, Some(egui::pos2(400.0, 600.0)));
    }

    #[test]
    fn rotation_change_moves_where_taps_land() {
        // The same physical contact lands at different logical points once the
        // display (and thus the touch matrix) rotates — SURFACE-9's hook.
        let mut tr = TouchTranslator::new(TouchTransform::new(2000, 1000, 1.0));
        let mut out = Vec::new();
        tr.feed(
            RawContact::Down {
                slot: 0,
                u: 0.0,
                v: 0.0,
                force: None,
            },
            &mut out,
        );
        let a = out.iter().find_map(|e| match e {
            egui::Event::Touch { pos, .. } => Some(*pos),
            _ => None,
        });
        assert_eq!(a, Some(egui::pos2(0.0, 0.0)));

        tr.set_rotation(Rotation::Rotate90);
        out.clear();
        tr.feed(
            RawContact::Down {
                slot: 1,
                u: 0.0,
                v: 0.0,
                force: None,
            },
            &mut out,
        );
        let b = out.iter().find_map(|e| match e {
            egui::Event::Touch { pos, .. } => Some(*pos),
            _ => None,
        });
        assert_eq!(
            b,
            Some(egui::pos2(0.0, 2000.0)),
            "rotated tap lands elsewhere"
        );
    }

    #[test]
    fn cancel_releases_primary_pointer() {
        let mut tr = TouchTranslator::new(TouchTransform::new(1000, 1000, 1.0));
        let mut out = Vec::new();
        tr.feed(
            RawContact::Down {
                slot: 0,
                u: 0.5,
                v: 0.5,
                force: None,
            },
            &mut out,
        );
        tr.feed(RawContact::Cancel { slot: 0 }, &mut out);
        assert!(!tr.pointer_active());
        assert!(touch_phases(&out).contains(&egui::TouchPhase::Cancel));
        // A release + PointerGone still fire so no widget stays "pressed".
        assert!(out
            .iter()
            .any(|e| matches!(e, egui::Event::PointerButton { pressed: false, .. })));
    }
}
