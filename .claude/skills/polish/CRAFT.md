# CRAFT: egui Enterprise Polish (companion reference for `/polish`)

**Scope:** Applies whenever writing or modifying Rust code that renders an E12
egui surface — shell chrome, panels, windows, menus, dialogs, popups, HUD
overlays, or any custom widget.

**Authority (read this first):** Subordinate to the root **`AI_GOVERNANCE.md`**
and to the **operator-locked Quasar dark design language** in
`SKILL.md` (20-Q survey, 2026-07-03). Where the locks speak — dark-only,
mono-first IBM Plex typography, `mde_egui::motion` FAST/BASE/SLOW, spring
physics for panel/sheet transitions, press-scale micro-interactions, 2 px
focus ring, Carbon icons, density-scales-spacing-only — **the locks win**.
This file fills in craft detail only where the locks are silent: geometry
discipline, window/menu construction, interaction-state completeness, and the
per-unit review pass. It introduces no new gates; the single hard rule and the
style-leak grep remain the mechanical discipline.

**Standard:** Every surface must read as a deliberate, professional desktop
application — the fit and finish of a mature commercial control panel, not a
debug tool or demo. Default egui styling is the *floor*, never the shipped
state.

**Version:** The workspace pins **egui 0.31.1** — the `CornerRadius` (u8) /
`Margin` (i8) / compact-`Shadow` era. Any code using the retired `Rounding`
API is a defect, not a style choice.

---

## 1. One Source of Look (restating the hard rule, with craft corollaries)

All look lives in `mde-egui` (`style`, `motion`, `fonts`); surface crates are
glue. Corollaries the grep can't catch but review should:

- A local `ui.style_mut()` in a surface crate is permitted only for a
  genuinely local exception (e.g., a destructive-action button) and must carry
  a comment naming the exception. Uncommented overrides are style leaks in
  spirit.
- If a needed value (color, duration, radius, spacing constant) is missing,
  add it to `mde-egui` with a backing test, then consume it — never
  approximate it locally "for now."
- Widgets built in surface crates that would be useful twice belong in
  `mde_egui::widgets` (per lock #9: consolidate before constructing).

## 2. Geometry Discipline

- **Rhythm:** Carbon's 8 px rhythm (lock: "the 8px rhythm") for margins,
  gutters, and inter-group spacing; 4 px subdivisions only for tight
  intra-widget padding. No 3s, 5s, 7s, or 13s anywhere. Density modes scale
  spacing only, never component dimensions (UX-24).
- **Corner radius:** The locked 4–8 px tiers, held everywhere: 4 for
  buttons/inputs, 6 for menus/popups, 8 for windows/sheets. egui's defaults
  mix 2/3 — the shared `Style` overrides all of
  `widgets.noninteractive/inactive/hovered/active/open`.
- **Interact sizes:** `interact_size.y` ≥ 24.0. Menu rows ≥ 26 px tall.
  Nothing clickable smaller than ~22 px in either axis (a11y lock: hit
  targets are a polish axis).
- **Strokes:** 1.0 px for borders and separators, always. The **2 px focus
  ring** (a11y lock #5) is the only heavier stroke. No 0.5 px hairlines (they
  shimmer at fractional DPI — auto-DPI is lock #7) and no ≥ 3 px borders.
- **Alignment:** Labels and values in settings-style layouts align on a
  shared column — `Grid` with explicit `min_col_width`, never eyeballed
  `add_space` shims.

## 3. Windows, Sheets, and Panels

- **Frames:** Every `Window`/sheet gets an explicit `Frame` from the shared
  `Style`: Quasar surface fill at its correct elevation tier, 1 px
  low-contrast border, locked radius tier, and the layered soft shadow for
  that elevation (lock #2: soft-Carbon depth — shadows are felt, not seen;
  subtle alpha on scrims, no gaussian blur pass).
- **Title bars:** Heading (mono) text style at restrained weight. Custom
  title bars include a full-height drag region and hover-reactive
  close/minimize affordances — never bare "X" text.
- **Open/close motion:** Windows, sheets, and modals never pop into
  existence. Entrances and exits use the `mde_egui::motion` primitives —
  springs for panel/sheet transitions, cross-fades where choreographed
  transitions call for them (lock #4). Exits: keep a `closing` flag + motion
  value and remove from the render list when the motion completes — never
  delete state instantly.
- **Modals:** `egui::Modal` with a dimming scrim (subtle alpha + dim per
  lock #2), animated in with the modal. Clicking the scrim closes
  non-destructive dialogs.
- **Panels:** `SidePanel`/`TopBottomPanel` get explicit `frame()` with
  margins on the rhythm and a 1 px separator stroke against the central
  panel. Resizable panels animate nothing while being dragged (direct
  manipulation is 1:1); collapsible panels transition via the shared motion
  primitives.

## 4. Motion Craft (inside the `mde_egui::motion` locks)

Lock #4 owns the vocabulary (springs, inertia, rubber-band, hover lift, press
scale, focus glow, staggered entrances — all in `mde_egui::motion`, never a
hand-rolled tween or literal duration in a surface crate). Craft rules for
*applying* that vocabulary:

- **Everything that changes state animates; nothing that follows the pointer
  does.** Hover, expand/collapse, tab switches, open/close: motion. Dragging,
  resizing, slider thumbs under direct input: instantaneous, 1:1.
- **Tier mapping:** FAST for micro-interactions (hover lift, press scale,
  focus glow), BASE for menus/popups/toggles, SLOW for structural transitions
  (panel collapse, hero expansions). If a motion feels like it needs more
  than SLOW, the design is wrong, not the table.
- **Dismissal is faster than arrival.** Menus and popups may enter at BASE
  but must leave at FAST or instantly — close lag reads as jank.
- **Expand/collapse:** Animate the revealed dimension, and fade contents in
  slightly *after* the dimension starts moving, so text never appears
  clipped mid-slide.
- **Staggered entrances** (lock #4) stagger by a FAST fraction per item and
  cap total choreography within SLOW — a 12-item list must not take a full
  second to settle.
- **Repaint hygiene:** egui's `Context::animate_*` requests repaints
  automatically while in flight; motion primitives must do the same — repaint
  only while motion is live, never unconditionally. An idle shell on a DRM
  seat has no compositor to hide behind: wasted frames are wasted watts.
  (Performance is not a gated axis — lock #8 — so a hitch found here is a bug
  to file, not a blocker.)

## 5. Menus, Popups, Context Menus

- Menus enter with a subtle 4–6 px slide from their anchor at BASE; they
  dismiss at FAST or instantly.
- Consistent width per menu (size to the longest item + padding; submenus
  never jitter to different widths).
- Rows: full-width hover highlight using the shared hover motion, a
  fixed-width left icon gutter (Carbon icons per lock #6; reserve the gutter
  even for icon-less items so labels align), right-aligned keyboard-shortcut
  hints in the weak text color.
- Separators between logical groups, never between every item.
- Submenus open on hover after ~200 ms or immediately on click; the parent
  row stays highlighted while its submenu is open (`WidgetVisuals::open` —
  the most commonly forgotten state; it is configured in the shared `Style`).
- Destructive items (Delete, Reset) use the theme's error color, positioned
  last, after a separator.

## 6. Interaction States — All Five, Always

Every interactive widget must visibly distinguish **rest / hovered / active
(pressed) / focused / disabled**. The shared `Style` configures all of
`widgets.inactive`, `.hovered`, `.active`, `.open`, and `noninteractive`
deliberately.

- Focus: the locked **2 px focus ring** plus the focus-glow micro-interaction.
  Complete keyboard reachability is a polish axis (a11y lock #5), so tab
  order is reviewable craft, not deferred work.
- Pressed: the shared press-scale micro-interaction plus a slightly darker
  fill than hover.
- Disabled: reduced-alpha text + fill, and `on_disabled_hover_text()`
  explaining *why* whenever the reason isn't obvious.
- Cursor: `PointingHand` on buttons/links, `ResizeHorizontal/Vertical` on
  grips, `Text` over editable fields. Wrong or missing cursors read as
  broken.
- Tooltips on every icon-only control, default show-delay left intact
  (instant tooltips feel cheap).

## 7. Typography and Color (within the locks)

- Mono-first (lock #3): IBM Plex Mono for headings, nav, data, metrics, IDs,
  code; the prose sans only for long-form text. The full `text_styles` set is
  defined in `mde_egui::fonts` — never egui defaults in a surface.
- Hierarchy through size/weight/color from the shared ramp, not scattered
  inline boldface. Secondary text uses the theme's weak color, never a local
  gray.
- Dark only (lock #1). Contrast held on every pair (a11y lock #5): body text
  ≥ 4.5:1 against its surface tier.
- One accent, used only for primary actions, selection, focus, and progress.
  If everything is accented, nothing is.

## 8. Per-Unit Review Pass (craft review — best-effort, not a §7 gate)

The mechanical gate remains: builds, tests green, style-leak grep clean,
renders through the shared `Style`. This pass is the eyes-on sweep, in the
spirit of the lifted visual gate — do it, note findings, never hold a unit
for it:

1. Hover every widget — does anything snap instead of moving through the
   shared hover motion?
2. Open/close every window, menu, and popup — any pop-in? Any close lag?
3. Tab through the surface — is the 2 px focus ring always visible and the
   order sane?
4. Headless screenshot and squint — do edges align to the rhythm? Are radius
   tiers and elevation shadows consistent?
5. Leave it idle — repaints should stop (a hitch or hot idle loop is a bug to
   file, per lock #8).
6. Shrink the surface — does layout degrade gracefully (scroll, wrap) rather
   than clip?
7. Sweep for craft leaks the grep can't see: uncommented local `style_mut`,
   `Rounding`-era calls, hand-rolled tweens, eyeballed `add_space` alignment.

**Prohibited outright:** egui default shadows on production surfaces,
hand-rolled tweens or literal durations in surface crates, instant-appearing
modals, pointer-following motion (drag/resize must be 1:1 — no smoothing),
hairline or ≥ 3 px strokes, uncommented local style overrides, colors minted
outside `mde-egui`, `Rounding`-era API calls, and unconditional per-frame
repaints.
