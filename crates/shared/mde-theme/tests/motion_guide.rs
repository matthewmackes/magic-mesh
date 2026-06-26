//! MOTION-AUDIT-3 — compile + behavior guard for `docs/design/motion-guide.md`.
//!
//! The contributor guide ships a short snippet per motion pattern; its acceptance
//! is "the snippets compile". This test mirrors each snippet against the real
//! `mde_theme` API so a rename/signature change to a motion primitive breaks the
//! build here instead of letting the doc rot. Keep these in lockstep with the
//! fenced `rust` blocks in the guide.

use std::time::{Duration, Instant};

use mde_theme::animation::{
    crossfade, fade_in, lift_on_hover, shimmer_alpha, slide_in, Animator, RenderParams, Tween,
};
use mde_theme::motion::list::{STAGGER_CAP, STAGGER_STEP_MS};
use mde_theme::motion::{Easing, Motion, PANEL_MOUNT_TRANSLATE_Y_PX, REDUCE_MOTION_CAP_MS};

// Pattern 1 — hover feedback (lift / press).
fn hover_offset(start: Instant, now: Instant, hovered: bool, reduce_motion: bool) -> RenderParams {
    lift_on_hover(start, now, 2.0, hovered, reduce_motion)
}

#[test]
fn pattern1_hover_feedback() {
    let p = hover_offset(Instant::now(), Instant::now(), true, true);
    assert_eq!(p.translate_y, 0.0, "no movement under reduce-motion (Q32)");
    let _ = PANEL_MOUNT_TRANSLATE_Y_PX;
}

// Pattern 2 — list / grid stagger.
fn row_params(start: Instant, now: Instant, i: u32, reduce_motion: bool) -> RenderParams {
    let step = Duration::from_millis(u64::from(STAGGER_STEP_MS));
    let slot = i.min(STAGGER_CAP as u32);
    let row_start = if reduce_motion {
        start
    } else {
        start + step * slot
    };
    slide_in(row_start, now, 6.0, reduce_motion)
}

#[test]
fn pattern2_list_stagger() {
    let t0 = Instant::now();
    // A later row is less far into its tween at t0 (top-down reveal).
    let a0 = row_params(t0, t0, 0, false).alpha;
    let a3 = row_params(t0, t0, 3, false).alpha;
    assert!(a3 <= a0);
    // Under reduce-motion the stagger is dropped (rows share the start).
    let r0 = row_params(t0, t0, 0, true).alpha;
    let r7 = row_params(t0, t0, 7, true).alpha;
    assert_eq!(r0, r7);
}

// Pattern 3 — modal / panel fade (enter + crossfade).
fn dialog_open(start: Instant, now: Instant, reduce_motion: bool) -> f32 {
    fade_in(start, now, reduce_motion).alpha
}

fn swap_content(start: Instant, now: Instant, reduce_motion: bool) -> (f32, f32) {
    let (out, incoming) = crossfade(start, now, reduce_motion);
    (out.alpha, incoming.alpha)
}

#[test]
fn pattern3_modal_fade() {
    let t0 = Instant::now();
    assert!(dialog_open(t0, t0, false) < 1e-3, "starts transparent");
    let (out0, in0) = swap_content(t0, t0, false);
    assert!((out0 - 1.0).abs() < 1e-3 && in0 < 1e-3);
}

// Pattern 4 — many animations off one clock (Animator).
#[test]
fn pattern4_animator_one_clock() {
    let now = Instant::now();
    let mut anim = Animator::new();
    anim.start("panel", now, Motion::panel_mount(), false);
    anim.start("hover", now, Motion::hover(), false);
    let _v = anim.value("panel", now, Easing::EaseOut);
    assert!(anim.needs_tick(now), "in-flight + visible ⇒ tick armed");
    let in_flight = anim.gc(now);
    assert_eq!(in_flight, 2);
}

// Pattern 5 — reduce-motion fallback (the a11y contract).
#[test]
fn pattern5_reduce_motion_fallback() {
    let reduced = Motion::loading().resolved(true);
    assert_eq!(
        reduced.duration,
        Duration::from_millis(REDUCE_MOTION_CAP_MS)
    );
    assert!(!reduced.looping);

    let tw = Tween::resolved(Instant::now(), Duration::from_millis(400), true);
    assert_eq!(tw.duration(), Duration::from_millis(REDUCE_MOTION_CAP_MS));

    // A skeleton is a STATIC mid-grey under reduce-motion (phase-independent).
    assert_eq!(shimmer_alpha(0.0, true), shimmer_alpha(0.9, true));
}
