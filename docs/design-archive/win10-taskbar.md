# WIN10-HYBRID — the single 48px bottom taskbar (Win10 structure · Construct identity)

Operator-locked 2026-07-12 (25-Q `/plan` survey). **This supersedes
[`vertical-dock.md`](vertical-dock.md) (VDOCK, 2026-07-04).** The shell chrome is a
**single horizontal bottom taskbar** modeled on the Windows 10 taskbar's *structure*,
wearing the **Construct-dark *identity***. The left vertical dock is **retired**; launching
folds into the taskbar's Start button.

> ⚠️ **Design-lock reversal, intentional and operator-approved.** VDOCK ripped out the
> bottom bar and made a left vertical dock "the shell chrome" (its Q25). WIN10-HYBRID
> reverses that: retire the vertical dock, the **bottom taskbar is the chrome** again. A
> future session reading `vertical-dock.md` must **not** re-implement the vertical dock —
> that doc is superseded. Likewise this revises two `polish/SKILL.md` locks (see
> *Reconciliation* below); those are updated in lockstep.

## Direction

**Win10 chrome *structure* + Construct *identity*.** Adopt the Windows-10 taskbar's familiar
anatomy and proportions (a real 48px bottom bar that reserves a strut, a Start launcher, a
running-app strip, a full tray with clock+date and an action-center) — but render it in the
locked Construct-dark language (azure `#5B8CFF` accent, IBM Plex mono-first type, subtle-alpha
translucency, square chrome). It is a **structural** borrow, not a visual reskin toward
Microsoft's Fluent look.

## Locked decisions (25-Q survey)

| Area | Lock |
|------|------|
| Height | **48px** — matches the Win10 taskbar (was an 18px floating rail) |
| Strut | **Reserve a bottom strut** — content ends *above* the taskbar (mirrors the old left-gutter), except full-screen remote/VDI which reserves 0 so guest resolution is unaffected |
| Placement | **Bottom only** — no top/side/multi-monitor placement in v1 |
| Left dock | **Retired** — launching folds into the taskbar Start; `DockState`/`DOCK_W` kept (the taskbar still owns picker state) |
| Start | **Grid of all surfaces** — the live-tile grid restyled into THE launcher (grouped grid) |
| Contents L→R | `[Start → surface grid] · [running sessions, icon-only 48px] · [spacer] · [Win10 tray: pinned pips + ▲ overflow flyout + clock **with date** + far-right action-center → opens Chat] · [show-desktop nub]` |
| Tray flyouts | **Back** — the Win10 tray restores the ▲ overflow flyout + action-center that VDOCK's Q15 had dropped (VDOCK routed tray icons straight to their surface with no flyouts) |
| Clock | **Two-line clock WITH date** (HH:MM over M/D/YYYY via `chat::civil_from_days`) — VDOCK had *removed* the clock; WIN10-HYBRID restores it |
| Auto-hide | **Optional** — ON → strut 0 + overlay reveal on bottom-edge hover; OFF → fixed 48 strut |
| Thumbnails | **Hover thumbnails for running (VDI) sessions** — only VDI sessions have a real frame source; static protocol badge first, live thumbnail later |
| Corners | **Square** on taskbar + Start; **rounded** (4/6/8px tiers) on surfaces |
| Translucency | **Subtle** — slight alpha, no gaussian blur (reuses the existing rail alpha) |
| Identity | **Kept:** IBM Plex mono-first (mono headings/nav/data, Inter prose); Construct azure `#5B8CFF` |
| Polish axes | **Build all four** as shared `mde_egui` tokens *with backing tests*, then adopt per-surface: **Motion** (spring chrome transitions, inertial + rubber-band scroll, hover-lift/press-scale/focus-glow/animated-toggle; reduce-motion best-effort), **Depth** (soft-shadow `Elevation` tiers + 4/6/8 radius), **Focus** (shared 2px ring), **Density** (Compact/Mouse/Comfortable/Touch, spacing-only) |
| Scope | All surfaces swept via the farm `/polish` loop; then cut an F44 RPM and deploy to the seats |

## What landed (as of origin `9783d7a4`)

- **Phase A — `mde-egui` shared tokens (complete):** `RADIUS_S/M/L=4/6/8` + `Elevation`/
  `ShadowToken` (`style.rs`); shared **2px focus ring** (`focus.rs`, `FOCUS_RING_W=2.0`);
  **4-density** `Compact/Mouse/Comfortable/Touch` (spacing 0.75/1.0/1.25/1.5); **motion**
  primitives (`Spring`+`spring_to`, `inertial_decay`, `rubber_band`, `hover_lift`/
  `press_scale`/`focus_glow`/`toggle_knob` — all reduce-motion-aware, `motion.rs`);
  **IBM Plex Mono** primary Monospace + named `heading`/`nav` families, Inter kept for prose
  (`fonts.rs`).
- **Phase B — shell chrome (taskbar landed; tray recompose remains):** taskbar fixed to
  **48px** decoupled from density (`rail_height()`); **bottom strut** (`taskbar_strut_height`
  + `reserved_taskbar_strut` + a `TopBottomPanel::bottom`, mirroring the old left gutter;
  full-screen-remote reserves 0); **left dock retired** (dropped the `dock::dock()` mount);
  **two-line clock with date** (`clock_date_text`). **Remaining (B5-rest):** the Win10 tray
  right-cluster recompose — ▲ overflow flyout, action-center → `Surface::Chat`, show-desktop
  nub, auto-hide bottom-edge reveal, running-icons-only, VDI hover thumbnails.
- **Phase C — polish sweep (complete):** the depth/focus/motion tokens adopted across 17
  surfaces via 4 farm-fanout waves (Timers a no-op — flat rows, nothing to lift; Browser
  excluded, it gets the separate Chrome-faithful rebuild). Integration-verified green.

## Reconciliation with the other locks

- **Supersedes `vertical-dock.md` (VDOCK) entirely.** Placement (bottom, not left), the
  clock (restored, not removed), tray flyouts (restored, not dropped). The VDOCK-era Timers
  & Alarms surface and the grouped-picker membership carry forward; its *placement* does not.
- **Revises `polish/SKILL.md` lock 2** ("gently rounded corners" everywhere) → **square
  chrome** on the taskbar + Start specifically; surfaces stay rounded (4/6/8px tiers).
- **Fulfils `polish/SKILL.md` lock 3** (mono-first): the "migrate `fonts.rs` off the Fira
  Code default" polish unit is **done** — IBM Plex Mono is the embedded primary Monospace.
- **Fulfils `polish/SKILL.md` lock 7** (density): the "extend Density toward compact/
  comfortable" unit is **done** — the 4-preset ladder shipped, spacing-only (UX-24 held).
- Supersedes the horizontal Win7/Win10 taskbar surveys in `win7-desktop-survey.md` for
  anything that conflicts; the surface roster / routing / accent tokens carry over.

## Acceptance (runtime-observable, on a seat)

- A single **48px** bottom taskbar; content ends above it (a reserved strut), except
  full-screen VDI which fills edge-to-edge (strut 0, guest resolution unchanged).
- **No** left vertical dock. Start opens the surface grid; every surface reachable exactly
  once from it.
- The tray shows pinned pips + ▲ overflow + a two-line **clock with date**; the far-right
  action-center opens Chat; the show-desktop nub minimises to the desktop.
- Polished surfaces read with the shared depth/focus/motion tokens; motion collapses under
  reduce-motion.
- Square taskbar/Start corners; rounded surface corners; subtle-alpha translucency, no blur.

## Tasks → the WIN10-HYBRID plan (`snappy-discovering-sphinx.md`), Phases A–D.
