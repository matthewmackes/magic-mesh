# MOTION-DRM architecture note

Status: MOTION-DRM-0 through MOTION-DRM-6, plus the MOTION-DRM-4 Browser
control-state, anchored-popover, dialog/prompt, drawer-panel, page/body, and
drag-release settle slices, 2026-07-16. This note reconciles the operator's
questionnaire-complete motion direction in `docs/WORKLIST.md` with the current
egui/DRM code before invasive edits.

## Current Map

The production shell is a single egui app on the DRM/KMS seat. The render loop is
`crates/shared/mde-egui/src/drm.rs::run_drm`: it pumps libinput, runs egui, reads
the root viewport `repaint_delay`, and sleeps until input, a repaint deadline, or
a periodic hardware task needs another frame. This is the right contract for
motion: animations must request frames only while active, then let the DRM loop
return to idle.

Shared motion lives in `crates/shared/mde-egui/src/motion.rs`. It already has:

- Duration constants: `FAST = 0.08`, `BASE = 0.18`, `SLOW = 0.32`.
- Runtime modes: `MotionMode::{Normal, Reduced, Disabled}`, with the older
  boolean reduce-motion setter kept as compatibility.
- Named production presets: `Control`, `Panel`, `Popover`, `Dialog`, `Page`,
  `Layout`, and `DragSettle`.
- `MotionSpec`, `MotionEasing`, `Phase`, and `Animated<T>` carriers for scalar,
  2D, size, rect, opacity, scale, and color interpolation.
- `animate`, `animate_value`, and typed stable-ID drivers that request repaint
  while active and stop once settled.
- `Spring::{SNAPPY,GENTLE}`, `spring_to`, rest detection, and frame-delta clamping.
- Status fade/pulse helpers, alarm blinking, inertial decay, rubber-band bounds,
  and micro-interaction factors for hover, press, focus, and toggle states.

Live consumers already include the bottom taskbar reveal/status panel, Start
menu, lock curtain, on-screen keyboard, toast/OSD, shared menubar, Files hover
rows, Music hover cards, Explorer hover affordances, Browser reduced-motion
loading globe handling, Browser chrome control-state state layers, Browser
site-info anchored popover entry motion, Browser prompt-bar dialog entry motion,
Browser secondary drawer-stack panel reveal motion, Browser active-body page
cross-fade motion, Browser tab drag-release settle motion, Explorer hero paging,
boot splash progress easing, and System's persisted normal/reduced/disabled
motion mode.

## Gaps

The current module is useful but not yet the full MOTION-DRM system:

- Shell-local direct `egui::Context::animate_value_with_time` call sites have
  moved through shared motion presets; the only remaining direct scalar animator
  use is the central `Motion::animate_value` compatibility wrapper.
- MOTION-DRM-4 now has one representative for each required category: Browser
  chrome covers restrained control-state, anchored-popover, dialog/prompt,
  secondary drawer-stack panel, active-body page, and tab drag-release settle
  motion. Additional consumers can migrate opportunistically.
- MOTION-DRM-6 production gates now cover simultaneous preset carriers returning
  to DRM idle, common refresh intervals, long-pause clamping, and production
  `mde-egui --features drm` tests. Exact performance/allocation improvements are
  not claimed without profiler measurements.
- Some legacy boolean motion call sites still collapse both reduced and disabled
  modes; representative integrations should prefer the typed preset API when they
  need reduced short fades rather than endpoint-only behavior.
- DRM repaint behavior is now covered by MOTION-DRM-3 tests that drive shared
  motion through a real `egui::Context`, inspect the root viewport repaint delay,
  and apply the same DRM wake policy used by `run_drm`.

## Design Target

Extend `mde_egui::motion`; do not add a new animation crate. The shell already
depends on this crate everywhere, and the production loop already understands
egui repaint deadlines.

Public shapes landed by MOTION-DRM-1/2:

- `MotionMode`: `Normal`, `Reduced`, `Disabled`.
- `MotionPreset`: `Control`, `Panel`, `Popover`, `Dialog`, `Page`, `Layout`,
  `DragSettle`, plus status/alarm helpers that keep their existing semantics.
- `MotionSpec`: duration/easing plus optional spring parameters and reduced-mode
  substitution.
- `Phase`: `Hidden`, `Entering`, `Visible`, `Exiting`.
- `AnimatedScalar` first, then `AnimatedVec2`, `AnimatedSize`, `AnimatedRect`,
  `AnimatedColor`, `AnimatedOpacity`, and `AnimatedScale` as thin typed wrappers.

The wrappers should own retargeting, completion detection, large-delta clamping,
and repaint requests. Call sites should provide a stable `egui::Id`, a target
value, and a preset; they should not manually manage normalized progress or
timestamps.

## Preset Table

| Preset | Normal | Reduced | Disabled | Notes |
|---|---:|---:|---:|---|
| `Control` | 0.08-0.12 s smoothstep | endpoint or <=0.08 s fade | endpoint | Hover, focus, press, selected/toggle micro states. No overshoot. |
| `Panel` | 0.16-0.20 s or `Spring::SNAPPY` | short fade/short travel | endpoint | Taskbar, drawers, sheets, Start-like panels. |
| `Popover` | 0.12-0.16 s fade/scale from anchor | fade only | endpoint | Context menus, anchored browser/site popups. |
| `Dialog` | 0.16-0.20 s fade + 0.96-0.98 scale | fade only | endpoint | Modal blocking starts on entry and releases after exit. |
| `Page` | 0.18-0.26 s directional/cross-fade | cross-fade | endpoint | Workspace/page switches; direction follows history when known. |
| `Layout` | 0.14-0.22 s near-critical | shortened movement | endpoint | List/card insert, remove, expand, collapse, and selection rails. |
| `DragSettle` | 0.18-0.30 s spring | snap or short settle | endpoint | Release/snap/cancel only; direct manipulation tracks input immediately. |

## DRM Repaint Contract

`Motion::animate` and egui's own animated values already schedule repaint while
traveling. `Motion::spring_to` explicitly calls `ctx.request_repaint()` until the
spring reaches rest. The DRM loop already converts egui's `repaint_delay` into
`next_repaint_at` and blocks indefinitely when egui reports idle.

MOTION-DRM-3 preserves that contract:

- Active animation means a finite repaint deadline or immediate repaint.
- Settled animation means no continuous repaint.
- Large frame gaps after suspend/VT switch are clamped before integration.
- Inputs still force a frame immediately.
- Page flips and GBM buffer reuse stay under the existing `run_drm` lifecycle.

MOTION-DRM-6 adds the production gate:

- Simultaneous `Control`, `Panel`, `Popover`, `Dialog`, `Page`, `Layout`, and
  `DragSettle` carriers request repaint while active, settle, and then report the
  root viewport idle sentinel so the DRM loop blocks indefinitely again.
- Common refresh intervals from 240 Hz down to 24 Hz stay finite, bounded, and
  within clamped frame deltas.
- Explorer and splash no longer bypass the shared preset/mode path for scalar
  page/progress motion.
- The `mde-egui --features drm` test suite passes on the farm; the shell's DRM
  feature also compiles through the focused splash lane.
- Source inspection found no new per-frame unbounded collections in the central
  typed motion driver; egui temp memory remains the storage mechanism per stable
  id. No exact allocation/performance claim is made without a profiler run.

## Next Slices

1. MOTION-DRM-1: done; named presets, mode/config, lifecycle, and typed animated
   values landed while preserving existing `Motion::animate` compatibility.
2. MOTION-DRM-2: done; deterministic tests cover endpoints, clamping, retargeting,
   reduced/disabled modes, spring convergence, and no NaN/inf.
3. MOTION-DRM-3: done; DRM wake/repaint tests now cover active-vs-settled
   animation deadlines and delayed-frame clamping before broad component integration.
4. MOTION-DRM-4: done; Browser chrome control-state layers now use
   `MotionPreset::Control` for toolbar icon buttons, tab pills, and Options rows,
   and the Browser site-info popup now uses `MotionPreset::Popover` with reduced
   fade-only and disabled endpoint behavior. Browser passkey, permission,
   before-unload, and password-save prompt bars now use `MotionPreset::Dialog`
   with reduced fade-only and disabled endpoint behavior. The Browser secondary
   drawer stack now uses `MotionPreset::Panel` for reveal motion with reduced
   short-travel/fade and disabled endpoint behavior. The Browser active body now
   uses `MotionPreset::Page` for route/tab/internal-page cross-fade with disabled
   endpoint behavior. Browser tab drag-release reorder now uses
   `MotionPreset::DragSettle` with tab-ID-keyed repaint state, a painter-only
   settle outline, and disabled endpoint-idle behavior.
5. MOTION-DRM-5: done; System appearance settings persist and apply
   normal/reduced/disabled motion mode, while legacy `reduce_motion` JSON migrates
   to `Reduced`.
6. MOTION-DRM-6: done; production/performance gates cover direct-call migration,
   simultaneous preset idle shutdown, common refresh intervals, clamped long
   frame gaps, input-triggered render policy, and farm `--features drm` success.
