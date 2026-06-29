# MOTION language — the contributor guide

**Status:** the shipped, code-backed companion to
[`motion-system.md`](motion-system.md) (the design). Where `motion-system.md`
captures the *epic*, this guide is the **how-to**: the one vocabulary every MCNF
surface animates through, with a copy-paste snippet per pattern. Referenced from
`AI_GOVERNANCE.md` §4 (Carbon tokens — motion durations/easings are single-sourced
in `mde-theme`, same as colour/spacing).

Every animation in the shell is **purposeful, one-vocabulary, latency-masking,
never input-blocking, compositor-friendly, accessible, and state-distinct** (the
seven locked principles — `motion-system.md` §"Motion language"). This guide is how
you obey them without re-deriving anything.

> **Snippets compile.** Each pattern below mirrors a real, compiling doctest in
> `crates/shared/mde-theme` (`animation.rs`, `feedback.rs`, `motion.rs` module
> docs + `lib.rs`). `cargo test -p mde-theme --doc` is the gate; the
> `install-helpers/lint-motion-tokens.sh` gate (MOTION-AUDIT-2, CI) additionally
> forbids any bespoke animation timing literal outside `mde-theme`.

---

## 0. Where the vocabulary lives (reuse — never reimplement, §6)

| Concern | Module | Key API |
|---------|--------|---------|
| Duration grid + easings + presets | `mde_theme::motion` | `Motion::{panel_mount,hover,press,focus,loading,refresh,success,error,…}`, `DURATION_*`, `Easing` |
| Reduce-motion contract | `mde_theme::motion` / `animation` | `Motion::resolved`, `Tween::resolved` |
| Decorative-vs-essential | `mde_theme::motion` / `prefs` | `MotionClass`, `MotionPrefs::apply_class` |
| Interpolation primitives | `mde_theme::animation` | `Tween`, `LoopingTween`, `ease`, `lerp_f32`, `spring` |
| One-clock registry + tick gating | `mde_theme::animation` | `Animator`, `Animator::needs_tick` |
| Enter/exit/crossfade/hover helpers | `mde_theme::animation` | `fade_in`, `slide_in`, `crossfade`, `lift_on_hover`, `Transition`, `RenderParams` |
| Control feedback (hover/press/focus) | `mde_theme::feedback` | `ControlFeedback`, `FocusRing` |
| Async/network state | `mde_theme::load_state` | `LoadState`, `StateTone` |
| Skeleton placeholders | `mde_theme::skeleton` | `SkeletonShimmer`, `SkeletonBlock` |

The toolkit dep never leaks into `mde-theme`: these return *pure values*
(`RenderParams`, alphas, offsets) that the GUI maps onto its themed widget.

---

## 1. Tokens, never literals

All timing comes from the Carbon duration grid. Pick the named preset that matches
the interaction — never an inline `Duration`.

```rust
use mde_theme::motion::{Motion, DURATION_MODERATE_02};
let mount = Motion::panel_mount();      // Carbon moderate-02 (240 ms) ease-out
assert_eq!(mount.duration, DURATION_MODERATE_02);
let hover = Motion::hover();            // fast-01 (70 ms) — micro-interaction
let loading = Motion::loading();        // slow-02 (700 ms) looping activity cue
```

## 2. The reduce-motion contract (always route through it)

Every consumer resolves its motion through the reduce-motion contract: under
reduce-motion a transition collapses to a ≤80 ms linear crossfade and loops drop.

```rust
use std::time::{Duration, Instant};
use mde_theme::motion::Motion;
use mde_theme::animation::Tween;
let reduce_motion = true;
// Preset-level:
let m = Motion::loading().resolved(reduce_motion);
assert_eq!(m.duration, Duration::from_millis(80));
// Tween-level — the single reduce-motion-aware constructor:
let tw = Tween::resolved(Instant::now(), Motion::panel_mount().duration, reduce_motion);
assert!(tw.duration() <= Duration::from_millis(80));
```

In a GUI, read the live flag once via `crate::live_theme::reduce_motion()`
(env `MDE_REDUCE_MOTION=1` or the persisted a11y pref).

## 3. Essential vs decorative motion (MOTION-A11Y-2)

Classify each animation. *Essential* motion (loading/progress/refresh, async state
transitions, focus, success/error) always plays; *decorative* polish (hover-lift,
shimmer breathe, selection-slide, staggered reveal) is dropped when the user
disables non-essential motion. Local config is authoritative.

```rust
use mde_theme::motion::{Motion, MotionClass};
use mde_theme::prefs::MotionPrefs;
let prefs = MotionPrefs { decorative: false, ..MotionPrefs::default() };
// Decorative collapses to a terminal (zero-duration) frame…
let deco = prefs.apply_class(Motion::hover(), MotionClass::Decorative, false);
assert_eq!(deco.duration, std::time::Duration::ZERO);
// …essential still animates.
let ess = prefs.apply_class(Motion::loading(), MotionClass::Essential, false);
assert!(ess.looping);
```

In a GUI, gate decorative movement on `crate::live_theme::decorative_motion()`
(env `MDE_MOTION_DECORATIVE=0` or the persisted pref). Keep the non-motion cue (a
colour token, a static placeholder, an instant select) so the state never vanishes.

## 4. One clock, zero idle ticks (MOTION-INFRA-1 / PERF-1)

Drive N concurrent animations off one `Animator` and gate the subscription on
`needs_tick` so a settled or hidden surface wakes the CPU zero times.

```rust
use std::time::{Duration, Instant};
use mde_theme::animation::Animator;
use mde_theme::motion::{Easing, Motion};
let t0 = Instant::now();
let mut a = Animator::new();
a.start("enter", t0, Motion::panel_mount(), false);
assert!(a.needs_tick(t0));                       // visible + in-flight ⇒ tick
let done = t0 + Duration::from_millis(300);
assert!(!a.needs_tick(done));                    // settled ⇒ subscription stops
let _ = a.value("enter", t0, Easing::EaseOut);   // read in `view`
```

Subscription shape (in the app's `subscription()`):

```text
if self.animator.needs_tick(Instant::now()) {
    subs.push(time::every(Duration::from_millis(16)).map(|_| Message::Tick));
}
```

## 5. Enter / exit / crossfade / hover (MOTION-INFRA-2)

Token-driven, reduce-motion-aware helpers return `RenderParams` (alpha /
translate_y / scale) you apply to a themed widget. Transforms collapse to a pure
opacity crossfade under reduce-motion.

```rust
use std::time::Instant;
use mde_theme::animation::{fade_in, slide_in, crossfade, lift_on_hover};
let (t0, now) = (Instant::now(), Instant::now());
let _appear   = fade_in(t0, now, false);
let _rise     = slide_in(t0, now, 8.0, false);            // fade + slide up
let (_out, _in_) = crossfade(t0, now, false);             // swap two surfaces
let _hover    = lift_on_hover(t0, now, 2.0, true, false); // rise on hover
```

## 6. Control feedback — hover / press / focus ring (MOTION-FEEDBACK-1)

`ControlFeedback` owns the three micro-interactions. The press fires on the *down*
edge (no input delay); the focus ring grows in (snaps under reduce-motion).

```rust
use std::time::Instant;
use mde_theme::feedback::ControlFeedback;
let now = Instant::now();
let fb = ControlFeedback::new().hovered(true, now).pressed(false).focused(true, now);
let geom = fb.params(now, false);            // translate_y / scale to apply
let ring = fb.focus_ring(now, false);        // accent outline alpha / width
let _ = (geom.is_at_rest(), ring.is_visible());
```

The shared `controls::variant_button` already applies this (hover tint + lift,
press darken, the engaged-state focus ring) across 100+ call sites.

## 7. Async / network state (MOTION-NET-1/5)

One vocabulary for every async surface; legible **without motion** (distinct icon
*shape* + label per state), colour is a secondary cue.

```rust
use mde_theme::load_state::LoadState;
let s = LoadState::Refreshing { stale: true };
assert_eq!(s.label(), "Refreshing…");
assert!(s.shows_content());                  // keep stale data visible, dimmed
assert!(LoadState::Offline.can_retry());     // degraded/offline auto-recover
```

## 8. Skeletons (MOTION-NET-2)

A loading placeholder is never motion-only: under reduce-motion / the kill switch
the shimmer is a static grey block.

```rust
use std::time::Instant;
use mde_theme::skeleton::SkeletonShimmer;
use mde_theme::Preferences;
let now = Instant::now();
let sh = SkeletonShimmer::from_prefs(now, &Preferences::default());
let _alpha = sh.alpha(now);                  // breathes, or static if reduced
let _tick  = sh.needs_tick(true);            // visible && live
```

## 9. No flashing — bound pulses to ≤3 Hz (MOTION-A11Y-3)

Any *visual flash* loop (pulse / blink / shimmer) must be built via
`LoopingTween::pulse`, which clamps the period up to the 3 Hz seizure threshold.

```rust
use std::time::{Duration, Instant};
use mde_theme::animation::LoopingTween;
use mde_theme::motion::MAX_PULSE_HZ;
let p = LoopingTween::pulse(Instant::now(), Duration::from_millis(50)); // requested 20 Hz
assert!(p.hz() <= MAX_PULSE_HZ + 1e-3);      // clamped to ≤3 Hz
```

Motion is presentation-only — it never reorders keyboard focus or mutates the
accesskit tree.

## 10. Transform / opacity only — no relayout (MOTION-PERF-2)

`RenderParams` carries *only* alpha + transform (translate_y / scale) and
structurally no layout field, so an animation can't trigger per-frame relayout.
`debug_assert` the guard before applying a frame.

```rust
use mde_theme::animation::Transition;
let p = Transition::SlideUp(8.0).params(0.5);
debug_assert!(p.is_compositor_safe());       // alpha∈[0,1], finite, scale>0
```

---

## Contributor checklist (paste into the PR description)

- [ ] Timing from a `Motion::*()` preset / `DURATION_*` token — **no inline `Duration`**.
- [ ] Resolves through `Motion::resolved` / `Tween::resolved` (reduce-motion).
- [ ] Decorative movement gated on `decorative_motion()`; essential cues kept.
- [ ] Subscription gated on `Animator::needs_tick` (zero idle/offscreen ticks).
- [ ] Transform/opacity only (`RenderParams::is_compositor_safe`) — no relayout.
- [ ] State legible without motion (icon shape + label), every loading/refresh
      state has a non-motion indicator.
- [ ] Any pulse/blink built via `LoopingTween::pulse` (≤3 Hz).
- [ ] Press fires on the down edge; motion never blocks input or hides an error.
- [ ] `install-helpers/lint-motion-tokens.sh` clean.
