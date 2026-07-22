> **HISTORICAL / SUPERSEDED (2026-07-22):** interface-paradigm design retired by the PLATFORM-INTERFACES standard (Apple-HIG-principled Construct + Car); see [docs/design/platform-interfaces.md](../design/platform-interfaces.md). Archived; do not implement from this document.

# MOTION — unified motion, refresh & visual-feedback system (design)

> **HISTORICAL / SUPERSEDED IN PART (2026-07-19):** the motion *principles* here still guide the shell, but they were specced against the retired iced/`libcosmic`/`mde-workbench` GUIs. The live desktop is the egui-native, DRM-native shell `mde-shell-egui` with motion in the shared `mde-egui` `Motion` module — see [`quasar-vdi-desktop.md`](quasar-vdi-desktop.md). The toolkit APIs and crate paths below are stale.

**Status:** design locked (operator prompt, 2026-06-19). No survey forks — the
operator's brief is fully prescriptive (8 epics, fixed item structure); this doc
captures it against the real MCNF codebase and lifts it to a worklist.
**Trigger:** Operator — make MCNF "a smooth, consistent, motion-rich shell where
objects feel alive, fluid, responsive, and in action" — not decorative animation,
but motion that *communicates state* and *masks latency* during slow renders,
network refreshes, and background work.
**Scope/version:** a cross-cutting feature epic; lands incrementally, **11.0+**.

## Foundation that already exists (reuse — §6 glue, not reimplementation)

The motion *primitives* are already in `mde-theme` but are **not wired into the
GUI view layer** (the `motion` module was even slimmed to a private 2-fn stub in
GUI-5 — i.e. dead). This epic's job is to **complete + wire** them, not invent
them:

- **`crates/shared/mde-theme/src/motion.rs`** — IBM Carbon v11 motion tokens:
  duration scale (`DURATION_FAST_01` 70 ms … `DURATION_SLOW_02` 700 ms),
  `EASING_STANDARD/ENTRANCE/EXIT` béziers, presets (`Motion::panel_mount`,
  `dialog_mount`, `notification_pulse`, `tooltip_fade`), list-stagger,
  skeleton-shimmer (1200 ms), selection-slide, icon fill-morph timings.
- **`crates/shared/mde-theme/src/animation.rs`** — `Tween`, `LoopingTween`,
  `ease()`, `lerp_f32()`, `pulse_scale()`; `Tween::static_frame()` for
  reduce-motion. Pure math, no toolkit dep.
- **`crates/shared/mde-theme/src/accessibility.rs`** — `A11y { reduce_motion,
  high_contrast, colorblind_safe }`, `transition_duration_ms()` (caps to 80 ms
  under reduce-motion); sourced from `MDE_REDUCE_MOTION` + `preferences.toml`.
- **`crates/workbench/mde-workbench/src/panel_chrome.rs`** — `empty_state` +
  `error_state` renderers + status badge; the shared loading/empty/error surface.
- **Subscription tick pattern** — `cosmic::iced::time::every(Duration).map(..)`
  in `app.rs` / the applets (focus drain 200 ms, bell 5 s, beacon beam 150 ms,
  compute sampling). The established way to drive repaints.
- **Already-filed, fold in (don't duplicate):** `MUSIC-RESPONSIVE-6` (skeletons),
  `MUSIC-RESPONSIVE-8` (optimistic transport), `APPS-FX-1` (launcher open/close +
  hover), `NOTIFY-FX-1` (Hub motion). These become the first concrete consumers
  of the unified system.

## Motion language (the locked principles)

1. **Purposeful, never decorative** — every animation communicates state,
   hierarchy, focus, loading, progress, or completion.
2. **One vocabulary everywhere** — the same interaction animates the same way in
   every surface; all timings/easings come from `mde-theme` tokens (§4 single
   source), never one-off literals.
3. **Latency-masking first** — motion exists primarily to make slow renders /
   network refreshes / background work feel continuously active, never frozen.
4. **Never blocks input or work** — animation is presentation-only; it never
   gates a click, delays a request, or hides a real error.
5. **Compositor-friendly** — transform/opacity-style changes, damage-aware,
   frame-paced; animation **ticks only when something is actually animating** and
   the surface is visible (no idle/offscreen CPU/GPU burn).
6. **Accessible** — fully honors reduce-motion (snap to end / crossfade-only),
   no flashing, every loading/refresh state has a **non-motion** indicator too.
7. **Distinct states** — the user can always tell idle vs loading vs refreshing
   vs degraded vs offline vs failed vs complete.

## Subsystems (the operator's ticket grouping)

| Subsystem | Epics | Home |
|-----------|-------|------|
| `motion-core` | Epic 1 (tokens), Epic 2 (infra) | `crates/shared/mde-theme` + a new `mde-motion` iced glue (or a module in each app) |
| `shell-components` | Epic 3 (feedback), Epic 5 (transitions) | `mde-workbench`, `mde-files`, `mde-music`, the applets |
| `network-state` | Epic 4 | the data layers + `panel_chrome` state renderers |
| `wayland-performance` | Epic 6 | subscription/redraw discipline across all GUIs |
| `accessibility` | Epic 7 | `mde-theme/accessibility.rs` + every consumer |
| (cross) | Epic 8 (audit) | whole shell |

---

## Epic 1 — MCNF Motion System Foundation (`motion-core`)

**Goal:** one complete, documented token + primitive set so every animation in
the shell is configured from `mde-theme`, reduce-motion-aware, with no literals.

- **MOTION-CORE-1 — Complete + un-private the motion token module.**
  **Problem:** `motion.rs` tokens exist but GUI-5 slimmed the module to a private
  stub; tokens aren't a public, complete, single source. **Goal:** a public,
  exhaustive token set (durations, easings, enter/exit, hover/focus/press,
  loading, refresh, error/success, stagger) consumed by every GUI. **Impl:**
  promote/extend `mde-theme::motion`; add any missing presets (hover-lift,
  press-depress, refresh-spin, success-check, error-shake-subtle); export a
  `Motion` API returning `(duration, easing)` pairs. **Affected:** `mde-theme`.
  **Acceptance:** every token has a `mde-theme` test asserting its value (§4); no
  motion literal exists outside `mde-theme` (a lint check greps the GUIs);
  reduce-motion variants resolve through `accessibility.rs`. **Testing:** unit
  tests per token; a grep gate in `install-helpers`. **Priority:** P0.
  **Deps:** none.
- **MOTION-CORE-2 — Spring/curve primitives + reduce-motion contract.**
  **Problem:** only cubic easing + linear tween exist; no spring feel, and
  reduce-motion isn't a hard contract. **Goal:** spring-like critically-damped
  interpolation for press/hover where appropriate, and a single
  `resolve(duration, a11y)` that ALL consumers must call. **Impl:** extend
  `animation.rs` (`spring()`, `Tween::resolved(a11y)`); reduce-motion → snap or
  ≤80 ms crossfade only. **Affected:** `mde-theme`. **Acceptance:** a spring
  settles monotonically without overshoot past tolerance (test); under
  reduce-motion every primitive returns its terminal value within ≤80 ms (test).
  **Testing:** property tests on settle/monotonicity. **Priority:** P0.
  **Deps:** MOTION-CORE-1.
- **MOTION-CORE-3 — Global animation config + kill switch.**
  **Problem:** no single place to disable/scale motion globally. **Goal:** a
  config (`preferences.toml [motion]`) for enable/disable + a global speed scale,
  plus the env override. **Impl:** add to `prefs.rs`; thread into the resolver.
  **Affected:** `mde-theme`, all GUIs. **Acceptance:** setting motion off makes
  every surface render terminal frames (no ticks scheduled); a speed scale of
  0.5/2.0 measurably changes durations; env overrides file. **Testing:** resolver
  unit tests. **Priority:** P1. **Deps:** MOTION-CORE-1,2.

## Epic 2 — COSMIC/Rust Animation Infrastructure (`motion-core`)

**Goal:** the iced-side machinery to run consistent motion with zero redundant
redraws.

- **MOTION-INFRA-1 — Central animation state + driver.**
  **Problem:** each surface hand-rolls ad-hoc `time::every` ticks; no shared
  animation clock or registry. **Goal:** a reusable `Animator` (a small module,
  e.g. `mde-motion` or per-app) holding active `Tween`s keyed by id, advanced by
  one subscription, that reports "any active?" so the tick can stop. **Impl:**
  wrap the `mde-theme` tweens; expose `start(id, motion)`, `value(id)`,
  `on_tick(now)`, `is_idle()`. **Affected:** new glue module + each app's
  `update`/`subscription`. **Acceptance:** N concurrent animations run off ONE
  subscription; when all settle the subscription yields nothing (verified: no
  `time::every` firing at idle). **Testing:** unit tests on the registry +
  is_idle. **Priority:** P0. **Deps:** Epic 1.
- **MOTION-INFRA-2 — Reusable transition helpers (enter/exit/crossfade).**
  **Problem:** no shared "animate this element in/out" helper. **Goal:** helpers
  that wrap an `Element` with opacity/translate/scale driven by an `Animator`
  value (Wayland-friendly: transform/opacity only). **Impl:** `fade_in`,
  `slide_in`, `crossfade(old,new)`, `lift_on_hover`; pure presentation.
  **Affected:** glue module. **Acceptance:** a panel mount fades+slides per the
  token; reduce-motion collapses to instant/crossfade; no layout reflow during
  the transition (transform/opacity only). **Testing:** render-logic tests on the
  interpolated values. **Priority:** P0. **Deps:** MOTION-INFRA-1.
- **MOTION-INFRA-3 — Frame-pacing + redundant-redraw avoidance + debug timing.**
  **Problem:** naive `time::every(16ms)` burns CPU even when idle/offscreen.
  **Goal:** tick only while animating AND visible; instrument frame time. **Impl:**
  gate the animation subscription on `Animator::is_idle()` + a visibility flag;
  add a debug overlay / log of frame interval behind a flag. **Affected:** all
  GUIs' `subscription()`. **Acceptance:** at rest the app schedules zero
  animation ticks (measured); an offscreen/hidden popup animates nothing; the
  debug timer reports per-frame ms. **Testing:** assert is_idle gating; manual
  frame-time capture. **Priority:** P0. **Deps:** MOTION-INFRA-1.

## Epic 3 — Shell-Wide Visual Feedback (`shell-components`)

**Goal:** consistent hover/focus/press/selection/state feedback on every common
component, all from the shared system.

- **MOTION-FEEDBACK-1 — Buttons, tabs, nav items, toolbar: hover/focus/press.**
  **Problem:** components are static (no press depress, hover lift, focus ring
  motion). **Goal:** uniform hover-lift, press-depress, animated 2 px Carbon focus
  ring, selection slide. **Impl:** apply MOTION-INFRA helpers in the shared
  button/nav/tab builders (`panel_chrome`, header, applet tiles). **Affected:**
  `mde-workbench` (header, nav, panel_chrome), the applets, `mde-files`,
  `mde-music`. **Acceptance:** every interactive control shows the same
  hover/focus/press motion; reduce-motion keeps the visual state change without
  movement; input never delayed (press fires on down). **Testing:** state-machine
  tests on the interaction → animation mapping. **Priority:** P0. **Deps:** Ep 2.
- **MOTION-FEEDBACK-2 — Cards, lists, tables: selection + row state.**
  **Problem:** selection/insert/remove are instant + jarring. **Goal:** selection
  slide/accent, subtle row hover, staggered list reveal (≤8-cap per the token).
  **Impl:** MOTION-INFRA in list/table builders; reuse list-stagger token.
  **Affected:** workbench panels, `mde-files`, `mde-music` grids/lists.
  **Acceptance:** selecting a row animates the accent; a freshly-loaded list
  staggers in (capped); reduce-motion → instant. **Testing:** logic tests on
  stagger indices/timing. **Priority:** P1. **Deps:** MOTION-FEEDBACK-1.
- **MOTION-FEEDBACK-3 — Menus, modals, drawers, notifications: enter/exit.**
  **Problem:** popups/menus/Hub appear/disappear instantly. **Goal:** consistent
  fade/scale-in + exit, matching `Motion::dialog_mount`/`panel_mount`. **Impl:**
  wrap popup/menu/drawer content via MOTION-INFRA; **this is where `APPS-FX-1` +
  `NOTIFY-FX-1` land** (folded in, not duplicated). **Affected:** apps applet,
  notify-center/Hub, workbench dialogs/drawers. **Acceptance:** the launcher,
  power menu, Hub, and dialogs all use the SAME enter/exit vocabulary;
  reduce-motion → crossfade only. **Testing:** shared-helper reuse asserted.
  **Priority:** P1. **Deps:** MOTION-FEEDBACK-1.

## Epic 4 — Network Refresh & Background-Work UX (`network-state`)

**Goal:** every async/network state is visually distinct and latency-masked.

- **MOTION-NET-1 — Canonical async state model.**
  **Problem:** panels carry ad-hoc `busy`/`load_error`/`loading` flags with no
  shared vocabulary for refreshing vs degraded vs offline vs stale. **Goal:** one
  `LoadState { Idle, Loading, Refreshing{stale}, Degraded, Offline, Failed{err},
  Loaded }` reused across surfaces. **Impl:** a shared enum + a `panel_chrome`
  renderer mapping each to a distinct visual (and a non-motion label). **Affected:**
  `panel_chrome`, workbench panels, `mde-music`, applets. **Acceptance:** each of
  the 7 states renders distinctly; the difference is legible without motion too
  (text/icon), satisfying a11y. **Testing:** a render test per state. **Priority:**
  P0. **Deps:** Epic 1.
- **MOTION-NET-2 — Skeletons + shimmer/pulse placeholders.**
  **Problem:** slow loads show blank or a "Loading…" string. **Goal:** greyed
  Carbon skeleton tiles/rows (known column/row count) with a subtle shimmer while
  loading. **Impl:** a `skeleton()` component driven by the shimmer token via
  MOTION-INFRA; **folds in `MUSIC-RESPONSIVE-6`.** **Affected:** `panel_chrome`,
  `mde-music`, list/grid panels. **Acceptance:** a slow first load shows
  skeletons matching the eventual layout; shimmer stops when data lands;
  reduce-motion → static grey (no shimmer). **Testing:** skeleton count matches
  target layout. **Priority:** P0. **Deps:** MOTION-NET-1, Epic 2.
- **MOTION-NET-3 — Stale-while-refreshing + smooth data replacement.**
  **Problem:** a refresh blanks the view then repaints. **Goal:** keep stale data
  visible (dimmed) with a background-refresh indicator, then crossfade to fresh.
  **Impl:** `Refreshing{stale}` keeps the last `Loaded` content + an inline
  progress affordance; crossfade on arrival. **Affected:** workbench panels,
  `mde-music` home/stats. **Acceptance:** a refresh never blanks the panel; the
  old→new swap crossfades; a background-refresh indicator is visible + dismissible.
  **Testing:** state-transition tests. **Priority:** P1. **Deps:** MOTION-NET-1,2.
- **MOTION-NET-4 — Optimistic updates + graceful retry.**
  **Problem:** transport/actions feel laggy waiting on the round-trip. **Goal:**
  apply the intended state immediately, reconcile from the async result, and show
  a clear retry on failure. **Impl:** optimistic apply + revert-on-error; a retry
  affordance for `Failed`/`Degraded`; **folds in `MUSIC-RESPONSIVE-8`.**
  **Affected:** `mde-music` transport, action buttons across panels. **Acceptance:**
  a transport toggle flips instantly + reconciles; a failed action reverts +
  offers retry without losing context. **Testing:** optimistic-then-revert tests.
  **Priority:** P1. **Deps:** MOTION-NET-1.
- **MOTION-NET-5 — Background-poll & connection-degraded indicators.**
  **Problem:** background polling + degraded mesh/bus connectivity are invisible.
  **Goal:** a subtle, consistent "working in background" indicator + a degraded/
  offline banner. **Impl:** tie to the existing bus-liveness probe + poll
  subscriptions; a shared status affordance. **Affected:** workbench header,
  applets, `mde-music`. **Acceptance:** background refresh shows a non-blocking
  indicator; bus/mesh down shows a degraded state that recovers automatically.
  **Testing:** indicator reflects probe state. **Priority:** P2. **Deps:**
  MOTION-NET-1.

## Epic 5 — Page & Layout Transitions (`shell-components`)

**Goal:** large view changes feel intentional + smooth.

- **MOTION-TRANS-1 — Route/panel switch transitions.** **Problem:** switching
  panels/pages is an instant cut. **Goal:** a consistent crossfade/slide on
  view change. **Impl:** MOTION-INFRA crossfade on the workbench content area +
  music page switch. **Affected:** `mde-workbench` app.rs, `mde-music`.
  **Acceptance:** panel switches crossfade; reduce-motion → instant; no input
  delay. **Testing:** transition trigger test. **Priority:** P1. **Deps:** Ep 2.
- **MOTION-TRANS-2 — Panel expand/collapse, drawer + modal open/close.**
  **Problem:** expanders/drawers/modals pop. **Goal:** height/opacity ease for
  expanders, slide for drawers, scale+fade for modals (tokens). **Impl:**
  MOTION-INFRA helpers. **Affected:** workbench drawers/dialogs, Hub.
  **Acceptance:** each uses its token preset consistently; reduce-motion safe.
  **Testing:** helper reuse. **Priority:** P2. **Deps:** MOTION-FEEDBACK-3.
- **MOTION-TRANS-3 — List insert/remove + table refresh transitions.**
  **Problem:** rows appear/vanish abruptly on refresh. **Goal:** animated
  insert/remove + a smooth table refresh (tie to MOTION-NET-3). **Impl:**
  keyed-diff reveal/collapse via MOTION-INFRA. **Affected:** lists/tables shell-
  wide. **Acceptance:** inserted rows reveal, removed rows collapse; a table
  refresh doesn't jump scroll. **Testing:** diff→animation mapping. **Priority:**
  P2. **Deps:** MOTION-FEEDBACK-2, MOTION-NET-3.
- **MOTION-TRANS-4 — Split-pane + window-resize polish (Wayland).** **Problem:**
  resize feels janky / full-redraw. **Goal:** smooth split-pane drag + clean
  resize without full-window thrash. **Impl:** damage-aware redraw, debounce
  layout. **Affected:** workbench split panes, music dock. **Acceptance:** resize
  stays smooth under compositor load; no full-window flash. **Testing:** manual
  on a Wayland compositor. **Priority:** P3. **Deps:** Epic 6.

## Epic 6 — Performance & Wayland Optimization (`wayland-performance`)

**Goal:** motion never makes the shell slower; correct on Wayland/HiDPI.

- **MOTION-PERF-1 — Idle/offscreen tick suppression (the core perf guard).**
  **Problem:** animation subscriptions can run forever. **Goal:** zero ticks at
  rest, none when a surface is hidden/offscreen. **Impl:** MOTION-INFRA-3's
  is_idle gating, audited across every `subscription()`. **Affected:** all GUIs.
  **Acceptance:** `top`/frame-log shows no animation wakeups at idle; a closed
  popup animates nothing. **Testing:** idle-wakeup assertion. **Priority:** P0.
  **Deps:** MOTION-INFRA-3.
- **MOTION-PERF-2 — Transform/opacity-only + no layout thrash.** **Problem:**
  animating layout properties reflows. **Goal:** animate transform/opacity where
  possible; avoid per-frame layout. **Impl:** prefer the MOTION-INFRA opacity/
  transform helpers; flag layout-animating code. **Affected:** all consumers.
  **Acceptance:** profiling shows no per-frame relayout during transitions.
  **Testing:** review + a debug counter. **Priority:** P1. **Deps:** Epic 2.
- **MOTION-PERF-3 — Frame-time + redraw-frequency instrumentation.** **Problem:**
  no visibility into motion cost. **Goal:** a debug flag logging frame interval +
  redraw count per surface. **Impl:** extend MOTION-INFRA-3. **Affected:** glue
  module. **Acceptance:** the flag yields per-surface frame timing; off by
  default, zero cost when off. **Testing:** flag on/off. **Priority:** P2.
  **Deps:** MOTION-INFRA-3.
- **MOTION-PERF-4 — HiDPI / fractional-scaling correctness + stress validation.**
  **Problem:** motion may break at fractional scale / under load. **Goal:**
  transitions correct at 1.0/1.25/1.5/2.0 scale and smooth under GPU/CPU/network
  stress. **Impl:** scale-aware sizing (reuse the cosmic-randr scale handling
  from APPS-FIT); validate on a real cosmic session. **Affected:** all GUIs.
  **Acceptance:** no clipping/blurring/jitter at fractional scale; motion stays
  smooth under stress. **Testing:** on-Cosmic at multiple scales + a stress run.
  **Priority:** P1. **Deps:** Epic 2.

## Epic 7 — Accessibility & User Controls (`accessibility`)

**Goal:** professional, accessible motion.

- **MOTION-A11Y-1 — Wire reduce-motion through every consumer.** **Problem:**
  `A11y::reduce_motion` exists but isn't wired into view rendering. **Goal:**
  every animation resolves through the reduce-motion contract (snap / ≤80 ms
  crossfade). **Impl:** thread `A11y` into the `Animator`/helpers; forbid any
  un-resolved duration. **Affected:** all GUIs. **Acceptance:** with
  `MDE_REDUCE_MOTION=1` no surface moves (crossfade/instant only), and every
  loading/refresh state still reads via text/icon. **Testing:** a reduce-motion
  render test per surface. **Priority:** P0. **Deps:** Epic 1, Epic 2.
- **MOTION-A11Y-2 — Disable non-essential motion + respect system prefs.**
  **Problem:** no user control beyond all/nothing. **Goal:** a setting to drop
  decorative motion while keeping state-communicating motion; respect a system
  reduce-motion signal if/when Cosmic exposes one (GUI-9 noted Cosmic doesn't
  today — keep local config authoritative). **Impl:** classify each animation
  essential/decorative; gate decorative on the setting. **Affected:** `mde-theme`
  + consumers. **Acceptance:** decorative-off removes lifts/shimmer but keeps
  loading/progress/state cues. **Testing:** classification test. **Priority:** P2.
  **Deps:** MOTION-A11Y-1.
- **MOTION-A11Y-3 — No flashing / no excessive pulse + keyboard/SR semantics.**
  **Problem:** pulses/shimmer could flash or break a11y semantics. **Goal:**
  bounded pulse rates (no >3 Hz flashing), motion never changes focus order or
  screen-reader semantics. **Impl:** clamp pulse frequency; ensure animated
  elements keep their accesskit roles/labels. **Affected:** all consumers.
  **Acceptance:** no animation exceeds the flash threshold; keyboard focus +
  accesskit tree unchanged during motion. **Testing:** frequency assertion +
  a11y-tree snapshot. **Priority:** P1. **Deps:** MOTION-A11Y-1.

## Epic 8 — Consistency Audit & Refactor (cross-cutting)

**Goal:** remove inconsistent/one-off behavior; document the approved patterns.

- **MOTION-AUDIT-1 — Inventory static/dull + no-feedback components.**
  **Problem:** unknown coverage. **Goal:** a list of every interactive component
  lacking the standard feedback. **Impl:** a sweep (like `/audit`) over the GUIs.
  **Affected:** all. **Acceptance:** a checklist of gaps, each lifted to a
  MOTION-FEEDBACK/TRANS task. **Testing:** n/a (report). **Priority:** P1.
  **Deps:** Epics 3,5.
- **MOTION-AUDIT-2 — Replace duplicate/one-off animation logic with the shared
  primitives.** **Problem:** ad-hoc `time::every` ticks + bespoke transitions.
  **Goal:** all motion routes through MOTION-INFRA; no isolated one-screen
  effects. **Impl:** refactor each ad-hoc tick onto the `Animator`. **Affected:**
  all GUIs. **Acceptance:** grep finds no bespoke animation literal/tick outside
  the shared module (a lint gate). **Testing:** the lint gate. **Priority:** P1.
  **Deps:** Epic 2.
- **MOTION-AUDIT-3 — Document the motion language + contributor examples.**
  **Problem:** no reference for future work. **Goal:** a `docs/` motion guide +
  copy-paste examples per pattern. **Impl:** write it from the shipped helpers.
  **Affected:** docs. **Acceptance:** the guide covers every pattern with a
  working snippet; referenced from `AI_GOVERNANCE` §4. **Testing:** snippets
  compile. **Priority:** P2. **Deps:** Epics 1–7.

## Hard rules (carried from the brief — enforced in acceptance)
No vague items · no cosmetic-only tickets · no motion that hides errors · never
block input while animating · no heavy/idle animation loops · no isolated
one-screen effects · no stubs (§7) · every item has measurable acceptance + a
performance + accessibility consideration.

## Acceptance standard
MCNF feels like a unified, polished, Wayland-native shell: the user always knows
whether the system is idle, working, refreshing, degraded, offline, failed, or
complete; the UI never feels frozen during slow renders/refreshes; motion is
smooth, subtle, consistent, purposeful — and fully reduce-motion-safe.

## Risks / notes
- **iced/libcosmic transform support** — opacity/translate/scale must be done via
  what the vendored iced fork actually exposes; where a true transform isn't
  available, prefer crossfade/size-ease over faking layout.
- **Token literal lint** — the §4 "no raw literals outside mde-theme" gate must
  extend to motion durations/easings (new lint).
- **Don't regress the perf win** — the whole point is masking latency without
  adding cost; MOTION-PERF-1 (idle suppression) is P0 and gates the rest.
- **GUI-5 removed dead motion widgets once already** — revive only what gets
  wired this time (§7 — no dead `pub` modules).
