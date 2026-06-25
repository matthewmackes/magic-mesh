# Motion contributor guide — copy-paste patterns

**Audience:** anyone adding motion to an MCNF GUI (`mde-workbench`, `mde-files`,
`mde-music`, `mde-voice-hud`, the cosmic applets).
**Companion:** [`motion-system.md`](motion-system.md) is the locked *design* (the
8 epics + acceptance). This file is the *how-to* — the shipped helpers with a
short compiling snippet per pattern. Every snippet is mirrored by a real test in
`crates/shared/mde-theme/tests/motion_guide.rs` (run `cargo test -p mde-theme`),
so the examples here cannot silently rot.

## The one rule

All motion timing comes from `mde_theme` — never a bare `Duration::from_millis(..)`
literal for an animation. The `install-helpers/lint-motion.sh` gate (CI, §4)
fails any binding that *reads as* animation timing (`*_STAGGER`/`*_FADE`/`*_TWEEN`/
… ) assigned a raw literal instead of a token. Source the duration from a
`Motion::*` preset or a `mde_theme::motion::list`/`icon` token; drive the tween
through `Animator`/`Tween::resolved`; let the helpers handle reduce-motion.

A frame-tick *cadence* (`time::every(..)` repaint clock, e.g.
`const ANIM_TICK = Duration::from_millis(16)`), a network `*_TIMEOUT`, an input
`DEBOUNCE`, an RTP `*_INTERVAL` are **not** animation timing — name them with
those words and the lint leaves them alone.

## The primitives at a glance

| Need | Use |
|------|-----|
| A named duration+easing | `mde_theme::motion::Motion::{hover, press, focus, panel_mount, dialog_mount, tooltip_fade, loading, refresh, success, error}` |
| Reduce-motion contract on a preset | `Motion::resolved(reduce_motion)` |
| A single-shot tween (reduce-motion-aware) | `mde_theme::animation::Tween::resolved(start, dur, reduce_motion)` |
| N tweens off one subscription | `mde_theme::animation::Animator` (`start`/`value`/`needs_tick`/`gc`) |
| One-call enter/exit/hover render params | `fade_in` · `slide_in` · `crossfade` · `lift_on_hover` |
| List/menu stagger beat | `mde_theme::motion::list::{STAGGER_STEP_MS, STAGGER_CAP}` |
| Skeleton breathe (static under reduce-motion) | `mde_theme::animation::shimmer_alpha(phase, reduce_motion)` |

---

## Pattern 1 — hover feedback (lift / press)

A control rises on hover and depresses on press, sourced from the Carbon
`fast-01` tier. **Under reduce-motion the transform is dropped** — the state still
reads via a colour tint, the surface never moves.

```rust
use std::time::Instant;
use mde_theme::animation::{lift_on_hover, RenderParams};
use mde_theme::motion::PANEL_MOUNT_TRANSLATE_Y_PX;

/// `start` = when the hover state last flipped; `hovered` = the target.
fn hover_offset(start: Instant, now: Instant, hovered: bool, reduce_motion: bool) -> RenderParams {
    // Rise 2px on enter, settle on leave — collapses to "no movement" under
    // reduce-motion. The duration is Motion::hover() inside the helper.
    lift_on_hover(start, now, 2.0, hovered, reduce_motion)
}

// Hover never moves the surface under reduce-motion (Q32):
let p = hover_offset(Instant::now(), Instant::now(), true, true);
assert_eq!(p.translate_y, 0.0);
let _ = PANEL_MOUNT_TRANSLATE_Y_PX; // the shared mount-rise token, if you need it
```

---

## Pattern 2 — list / grid stagger

A freshly-loaded list reveals top-down: each row's slide-in starts a shared
`STAGGER_STEP_MS` beat later, capped at `STAGGER_CAP` so a long list still
finishes promptly. **Never hand-roll the step** — that is exactly the literal the
lint catches.

```rust
use std::time::{Duration, Instant};
use mde_theme::animation::slide_in;
use mde_theme::motion::list::{STAGGER_CAP, STAGGER_STEP_MS};

/// The slide-in render params for row `i` of a list revealed at `start`.
fn row_params(start: Instant, now: Instant, i: u32, reduce_motion: bool) {
    // Cap the per-row delay so row 9 and row 900 share the last slot.
    let step = Duration::from_millis(u64::from(STAGGER_STEP_MS));
    let slot = i.min(STAGGER_CAP as u32);
    let row_start = if reduce_motion {
        start // no stagger under reduce-motion — every row reveals together
    } else {
        start + step * slot
    };
    let _ = slide_in(row_start, now, 6.0, reduce_motion);
}

row_params(Instant::now(), Instant::now(), 3, false);
```

---

## Pattern 3 — modal / panel fade (enter + crossfade)

A dialog or panel fades in on mount; a content swap crossfades old→new. Both are
opacity-only (the iced fork has no opacity/transform widget — interpolate a
container/text colour alpha), so they are already reduce-motion-safe; the ≤80 ms
cap is the only change.

```rust
use std::time::Instant;
use mde_theme::animation::{crossfade, fade_in};

fn dialog_open(start: Instant, now: Instant, reduce_motion: bool) {
    // 0 -> 1 opacity over Motion::panel_mount() (≤80 ms under reduce-motion).
    let params = fade_in(start, now, reduce_motion);
    let _alpha = params.alpha; // apply to the dialog container's fill alpha
}

fn swap_content(start: Instant, now: Instant, reduce_motion: bool) {
    // outgoing fades 1->0 while incoming fades 0->1 over the same window.
    let (out, incoming) = crossfade(start, now, reduce_motion);
    let _ = (out.alpha, incoming.alpha);
}

dialog_open(Instant::now(), Instant::now(), false);
swap_content(Instant::now(), Instant::now(), false);
```

---

## Pattern 4 — many animations off one clock (`Animator`)

When a surface has several concurrent tweens, hold ONE `Animator`: it keys tweens
by id, advances on a single subscription tick, and reports `needs_tick` so the
subscription stops at rest / while hidden (MOTION-PERF-1: zero idle wakeups). The
per-tween `reduce_motion` cap is applied for you in `Animator::start`.

```rust
use std::time::Instant;
use mde_theme::animation::Animator;
use mde_theme::motion::{Easing, Motion};

let now = Instant::now();
let mut anim = Animator::new();
anim.start("panel", now, Motion::panel_mount(), /* reduce_motion */ false);
anim.start("hover", now, Motion::hover(), false);

// In view(): read the eased 0..=1 value for each id.
let _v = anim.value("panel", now, Easing::EaseOut);

// In subscription(): arm the per-frame clock ONLY while this is true.
let _arm_tick = anim.needs_tick(now);

// In update() each tick: drop settled tweens; when 0 remain, stop ticking.
let _still_in_flight = anim.gc(now);
```

---

## Pattern 5 — reduce-motion fallback (the a11y contract)

Reduce-motion is honored by *routing through the helpers*, never by an `if` in
each view. The contract (Q32): every transition collapses to a ≤80 ms linear
crossfade, loops are dropped, and **every loading/refresh state keeps a non-motion
cue** (text/icon/static grey). The env knob is `MDE_REDUCE_MOTION=1`.

```rust
use std::time::{Duration, Instant};
use mde_theme::animation::{shimmer_alpha, Tween};
use mde_theme::motion::{Motion, REDUCE_MOTION_CAP_MS};

// A preset resolves to the ≤80 ms linear, loop-dropped form:
let reduced = Motion::loading().resolved(true);
assert_eq!(reduced.duration, Duration::from_millis(REDUCE_MOTION_CAP_MS));
assert!(!reduced.looping);

// A raw-duration tween caps the same way:
let tw = Tween::resolved(Instant::now(), Duration::from_millis(400), true);
assert_eq!(tw.duration(), Duration::from_millis(REDUCE_MOTION_CAP_MS));

// A skeleton becomes a STATIC mid-grey (no shimmer) — motion is never the only
// cue; the placeholder structure itself communicates "loading".
assert_eq!(shimmer_alpha(0.0, true), shimmer_alpha(0.9, true));
```

To resolve the live preference (persisted a11y pref OR the `MDE_REDUCE_MOTION`
env override), read it once where the GUI builds its theme and thread the `bool`
into the helpers above — see `mde_theme::A11y::reduce_motion` /
`A11y::transition_duration_ms`.

---

## Where the live consumers are (read these for full examples)

- `crates/services/mde-music/src/motion.rs` — `Reveal` (staggered queue),
  `Shimmer`, `MountReveal`, `button_feedback`.
- `crates/services/mde-files/src/loading.rs` — skeleton-first paint +
  stale-while-refreshing dim/crossfade over `LoadState`.
- `crates/workbench/mde-workbench/src/panels/datacenter.rs` — card-grid staggered
  reveal + hover + selection.
- `crates/platform/mde-cosmic-applet/src/bin/mde-apps-applet.rs` /
  `crates/services/mde-voice-hud/src/main.rs` — `Animator`-driven open/appear.
