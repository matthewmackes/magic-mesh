# Rust / iced criteria — the implementation rubric

How "pleasing effects + UX" is built correctly in iced / libcosmic (the COSMIC
toolkit — a vendored iced fork with accesskit). Each item is a pass/fail
assertion the loop checks against the diff + the running binary. Motion math
already exists in `crates/shared/mde-theme::animation` (`Tween`, `LoopingTween`,
`Animator`, `RenderParams`, `ease`/`spring`) — these criteria govern how surfaces
*consume* it.

## Contents
1. Animate the cheap properties
2. Time-driven, idle-parked
3. Subscription identity gating
4. One backend per app
5. Reduce-motion is the app's job
6. Async as a state machine
7. Accessibility
8. wgpu performance
9. libcosmic shape

---

## 1. Animate the cheap properties
Animate **opacity, transform (translate / scale / rotate), and color** only —
**never width / height / padding** (those force a per-frame relayout and are the
common cause of iced jank). MCNF: `RenderParams { alpha, translate_y, scale }` is
the sanctioned vehicle. A diff that animates a layout dimension is a finding.

## 2. Time-driven, idle-parked
Store the animation state (an `iced::Animation<T>` / `cosmic-time` Timeline / an
`mde-theme::Animator`) in the model and sample it at an `Instant`; emit redraws
**only while `is_animating()`**; return `Subscription::none()` +
`RedrawRequest::Wait` when idle so the event loop parks and the wgpu surface stops
re-rendering.
- **Acceptance:** idle CPU of the running binary returns to **≈0** — spot-checked
  on the farm after any motion change. A surface that ticks at rest fails.

## 3. Subscription identity gating
Return `window::frames()` / `time::every(..)` **ONLY while animating**, and
`Subscription::none()` otherwise — the runtime kills the stream when you stop
returning it. **Do not** leave an always-on tick that filters internally (it keeps
the loop hot). MCNF: gate the subscription on `Animator::is_idle(now) == false`.

## 4. One backend per app
Pick **one** animation backend per surface and keep a single synchronized clock:
- `iced::Animation<T>` — simple state-driven motion (presets very_quick 100 /
  quick 200 / slow 400 / very_slow 500ms).
- `iced_anim` springs (smooth / bouncy) — interactive / interruptible motion
  (drag, hover, toggle).
- `cosmic-time` Timeline — COSMIC apps (one atomic clock).
- MCNF default: the `mde-theme::animation` Tween/Animator layer (toolkit-agnostic
  math), driven by one `time::every` while animating. **Do not mix backends** in
  one surface.

## 5. Reduce-motion is the app's job
Read COSMIC's **reduced-motion** accessibility config via the config subscription
and branch animations off it (skip / shorten). cosmic-time's low-motion
auto-detection is an explicit **unimplemented TODO** — the toolkit will NOT do it.
MCNF: thread the flag through `Tween::resolved(.., reduce_motion)` /
`Motion::resolved(reduce_motion)` (caps to ≤80ms linear). Every animated call site
must pass it; a hardcoded `false` is a finding.

## 6. Async as a state machine
Model loading as `enum State { Loading, Loaded(data), Error }` populated by
`Task::perform(async_fn, MsgCtor)`; render a **skeleton / spinner** in the
`Loading` arm and swap on the `Loaded` message. Keep `update()` swift — push
slow/blocking work into a `Task` or `Subscription` (`update()` runs in the
event loop and blocks the single-threaded UI). A blank panel during load (no
skeleton) is a finding (see MOTION-NET-1/2).

## 7. Accessibility
Enable libcosmic's experimental **accessibility** cargo feature so
`iced_accessibility` exposes the accesskit tree; give interactive widgets stable
identities + meaningful labels/roles. It is feature-gated → must be deliberately
enabled and fed correct semantics. A new interactive widget with no label/role is
a finding.

## 8. wgpu performance
Request the **high-performance adapter** where relevant (iced defaults to the
integrated/low-power GPU) and wrap `canvas` / `mesh` / custom-geometry widgets in
iced's geometry **`Cache`** so they rebuild only when inputs change, not every
frame.

## 9. libcosmic shape
Build surfaces on the `cosmic::Application` trait (`APP_ID`,
`Executor = cosmic::executor::Default`, `init` / `update` / `view` /
`subscription`) and **subscribe to config changes** so theme + reduced-motion
hot-reload into `update()` without a restart. A surface that reads theme once at
init (no config subscription) is a finding for re-theming + reduce-motion.
