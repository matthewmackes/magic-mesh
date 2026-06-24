# MOTION-AUDIT-1 — static / no-feedback component gap list

**Unit:** MOTION-AUDIT-1 (Epic 8, `docs/design/motion-system.md`). **Type:** survey
/ report (no code change). **Method:** a `/audit`-style sweep over the GUI crates
(`mde-workbench`, `mde-files`, `mde-cosmic-applet`, `mde-music`, `mde-voice-hud`,
shared `mde-theme`/`controls`/`panel_chrome`) for **interactive** components that
lack the standard hover / focus / press / selection feedback or transition motion
defined by the MOTION epic.

Every gap below cites `file:component` evidence and is **lifted to a concrete
follow-on task** (a MOTION-FEEDBACK-* / MOTION-TRANS-* parent, or a new sub-item)
— satisfying the acceptance ("a checklist of gaps, each lifted to a
MOTION-FEEDBACK/TRANS task").

Base commit of this survey: `d87d0c7`.

---

## Coverage baseline (what is already done — NOT a gap)

So the gaps below are read in context, here is what the sweep found **already
covered** (do not re-file these):

- **Shared `variant_button`** (`crates/workbench/mde-workbench/src/controls.rs:80`,
  `variant_button_style:136`) — full hover-tint + hover-lift-shadow + press-darken
  + reduce-motion contract (MOTION-FEEDBACK-1 landed here, 43 call sites).
- **`mde-files` rows/tiles** (`crates/services/mde-files/src/widgets.rs:878`
  `file_row` / `:1141` `list_row`) — hover-lift, selection-accent tween,
  staggered reveal-slide, all off one `mde_theme::animation::Animator`, with the
  `reduce_motion` (`rm`) flag threaded through (`views.rs:1055`).
- **`mde-music`** (`crates/services/mde-music/src/main.rs`) — the showcase: welcome
  reveal, skeleton shimmer, hover-lift, slide-up, staggered queue reveal, one
  kill-switch gate (`:192`), optimistic transport + retry (MOTION-NET-4).
- **Apps launcher applet** (`crates/platform/mde-cosmic-applet/src/bin/mde-apps-applet.rs`)
  — per-tile hover-lift, open-in slide, tab crossfade off one `Animator`
  (APPS-FX-1, `:25`/`:306`).
- **Notification Hub** (`crates/workbench/mde-workbench/src/bin/mde-notify-center.rs`)
  — new-item slide/blink, open-in slide, beam tick (NOTIFY-FX-1, `:285`).
- **Voice HUD** (`crates/services/mde-voice-hud/src/main.rs:55`) — appear/hover
  tweens off one `Animator`.
- **`datacenter` panel cards** (`crates/workbench/mde-workbench/src/panels/datacenter.rs:950`)
  — per-card hover-lift tween (MOTION-FEEDBACK-2).
- **`home` panel buttons** (`crates/workbench/mde-workbench/src/panels/home.rs:1955`)
  — animated press ring + depress (covered, but **bespoke** — see AUDIT-2 note).
- **`panel_chrome::skeleton`** (`panel_chrome.rs:347`) — the MOTION-NET-2 animated
  shimmer skeleton (this is the live one; the `controls.rs` skeleton below is the
  stale static copy).
- **Header window-controls** (`header.rs:184` `control_button`) — hover + press.
- **`panel_chrome::empty_state` CTA** (`panel_chrome.rs:501`) — hover + press.

---

## GAPS (the checklist)

### G1 — `controls::variant_button` has no keyboard-focus ring (FORK-BLOCKED)
- **Evidence:** `crates/workbench/mde-workbench/src/controls.rs:136`
  `variant_button_style` matches only `Active | Hovered | Pressed | Disabled`. The
  vendored fork's `button::Status`
  (`~/.cargo/git/checkouts/libcosmic-*/iced/widget/src/button.rs:677`) has **no
  `Focused` variant** — the widget tracks `is_focused` internally (`:269`, used at
  `:429` for Enter-to-activate) but never surfaces it to the style closure, and
  the `is_focused()`/`focus()` accessors are commented out (`:723`–`:738`).
- **Gap:** the "animated 2 px Carbon focus ring keyed off REAL focus" promised by
  MOTION-FEEDBACK-1 is **unreachable on every shared button** — a keyboard-only
  user gets no focus indication on any `variant_button`. `FOCUS_RING_WIDTH`/
  `FOCUS_RING_OFFSET` consts exist (`controls.rs:44`) but are never applied to a
  button.
- **Lift to:** **MOTION-FEEDBACK-1** (re-open / new sub-item **MOTION-FEEDBACK-1a**:
  surface `Focused` from the fork's button — either add the enum variant + plumb
  `is_focused` into the status, or expose the `is_focused()` accessor — then paint
  the focus ring in `variant_button_style`). **Upstream/fork blocker**; note in
  `NEEDS-OPERATOR.md` if the fork edit is out of scope.

### G2 — `controls::toggle` does not animate the knob slide
- **Evidence:** `controls.rs:244` `toggle` — the knob position snaps
  (`knob_offset` is computed from the boolean value, no tween); the doc comment
  states "Slide animation (140 ms ease-out per spec) deferred to UX-9.a
  subscription wiring — stateless snap for now" (`:242`). Hover only does a flat
  `brighten(bg, 1.05)` (`:293`); no press feedback.
- **Gap:** the shared toggle pill pops between on/off with no slide and no press
  cue — inconsistent with the rest of the feedback vocabulary.
- **Lift to:** **MOTION-FEEDBACK-1** (toolbar/controls feedback) for the knob-slide
  + press; gated on MOTION-INFRA-1 `Animator` wiring into the workbench app the
  way `mde-files`/`mde-music` already do.

### G3 — `controls::skeleton` + `controls::spinner` are static
- **Evidence:** `controls.rs:450` `skeleton` ("shimmer animation wires in
  UX-9.a") renders a flat tinted rect; `controls.rs:479` `spinner` ("Static accent
  circle; animation wiring deferred to UX-9.a") never rotates. The static spinner
  is on a **live latency surface**: `crates/workbench/mde-workbench/src/panels/connect.rs:649`
  shows `controls::spinner(palette)` during the connect probe — a frozen "loading"
  dot, exactly the frozen-render the MOTION brief forbids.
- **Gap:** a no-motion loading affordance on a real wait. (Note: the **good**
  animated skeleton lives in `panel_chrome::skeleton`, MOTION-NET-2 — these two
  `controls` copies are stale and should either route through the shared shimmer
  or be removed.)
- **Lift to:** **MOTION-NET-2** (skeleton/shimmer) for `controls::skeleton`; a new
  **MOTION-FEEDBACK / MOTION-NET sub-item** for an animated `spinner` (or replace
  the `connect.rs` use with the `panel_chrome` loading indicator). Overlaps
  **MOTION-AUDIT-2** (remove the duplicate static copies).

### G4 — `mesh_federation` panel: 11 buttons with NO hover/press/focus feedback
- **Evidence:** `crates/workbench/mde-workbench/src/panels/mesh_federation.rs` — 11
  buttons built with `.sty(move |_t, _s: ButtonStatus| button::Style { … })`
  (`:745`, `:796`, `:870`, `:994`, `:1040`, `:1096`, …) that **discard the status
  param entirely**, so the mint / revoke / accept / add / remove federation
  actions are fully static (no hover, no press, no focus). Zero `variant_button`
  use in this panel.
- **Gap:** an entire high-stakes control panel (federation key minting/revocation)
  renders dead-flat controls.
- **Lift to:** **MOTION-FEEDBACK-1** — migrate these to `controls::variant_button`
  (inherits the shared hover/press for free) or, where a bespoke style is needed,
  consult `status`.

### G5 — `mesh_bus` panel: 9 buttons with NO hover/press/focus feedback
- **Evidence:** `crates/workbench/mde-workbench/src/panels/mesh_bus.rs` — 9 buttons
  with `.sty(move |_t: &Theme, _s: ButtonStatus| button::Style { … })` discarding
  the status (`:1208`, `:1374`, `:1425`, …). Zero `variant_button` use.
- **Gap:** the bus control panel's actions are static (same defect as G4).
- **Lift to:** **MOTION-FEEDBACK-1** — migrate to `variant_button` / consult
  `status`.

### G6 — `styled_text_input` + all raw `text_input` have no Carbon focus styling
- **Evidence:** `controls.rs:216` `styled_text_input` selects the built-in
  `cosmic::theme::iced::TextInput::Default` class (`:237`) because "the libcosmic
  `cosmic::Theme` text_input Catalog … has NO per-instance closure variant" — so
  the spec's "2 px accent on focus" (`:214`) and the bottom-divider line cannot be
  rendered. Plus **38 raw `text_input(` call sites** across the workbench bypass
  even `styled_text_input` and inherit the same Default class.
- **Gap:** focusing any text field gives no Carbon-consistent focus cue; the input
  chrome is whatever the cosmic Default paints, not the shell vocabulary.
- **Lift to:** **MOTION-FEEDBACK-1** (focus feedback) — **fork-gated** on the
  libcosmic text_input growing a per-instance style closure (same class of blocker
  as G1). File alongside CUT-1 fork-drift in `NEEDS-OPERATOR.md`; in the interim,
  a sub-task to route the 38 raw call sites through `styled_text_input` so the fix
  lands in one place when the fork allows it.

### G7 — `sidebar` nav rows + `section_label` groups have no motion (instant swaps)
- **Evidence:** `crates/workbench/mde-workbench/src/sidebar.rs:250` `nav_row`
  (style closure `:295`) and `:190` `section_label` (style closure `:209`) switch
  the background on `Hovered`/`Pressed` with instant
  `Background::Color(...)` swaps; the active-row accent stripe (`:259`) and the
  focus ring (`:305`) snap in/out. There is **no animated** selection-slide,
  hover-tint ease, or stripe transition.
- **Gap:** the primary navigation has correct *states* but no *motion* — switching
  panels/groups is an instant cut, inconsistent with the "selection animates the
  accent" + "panel switches crossfade" goals. Also: the nav focus ring keys off the
  app-level `sidebar_focused` flag, **not** real per-row keyboard focus (related to
  G1).
- **Lift to:** **MOTION-FEEDBACK-1** (nav-item hover/focus motion) +
  **MOTION-FEEDBACK-2** (selection-slide on the active stripe) +
  **MOTION-TRANS-1** (the route/panel-switch crossfade the nav drives).

### G8 — `panel_chrome::data_row` is a non-interactive shared row (no hover/select)
- **Evidence:** `panel_chrome.rs:117` `data_row` renders a plain label/value `row!`
  with no `button`/`mouse_area`, no hover, no selection — even though "rows/tables
  selection + row state" (MOTION-FEEDBACK-2) is a target. (It currently has **0
  call sites** — `grep data_row( panels/` is empty — so it is also dead; flag for
  AUDIT-2 / a dead-code sweep, but if revived it must gain row feedback.)
- **Gap:** the shared data-row primitive offers no interactive feedback path;
  panels needing selectable rows have nowhere shared to reach for.
- **Lift to:** **MOTION-FEEDBACK-2** — give the shared row an interactive variant
  (hover-tint + selection-accent) mirroring `mde-files` `list_row`, **or** remove
  it as dead (AUDIT-2) and standardize panels on the `mde-files` row helpers.

### G9 — workbench animated transitions are fork-gated (panel switch / data crossfade)
- **Evidence:** there is no panel-switch crossfade in `app.rs`, and MOTION-NET-3's
  old→new data crossfade is explicitly blocked: `mde_theme::animation` notes "the
  iced-0.13-fork having no opacity/transform widget" (WORKLIST MOTION-NET-3, `[>]`);
  MOTION-TRANS-1's crossfade depends on the same. Workbench panels currently hard-
  cut on view change.
- **Gap:** large view changes (panel/route switch, data replacement) are instant
  cuts in the workbench, not the intended crossfade.
- **Lift to:** **MOTION-TRANS-1** (route/panel crossfade) + **MOTION-NET-3**
  (data crossfade) — both already `[>]`/gated on the iced-0.14 opacity/transform
  widget (UX-PRE blocker). No new task; this gap **confirms** those parents are
  the right home and that no other workbench surface needs a separate ticket.

---

## Cross-cutting note (hand-off to MOTION-AUDIT-2)

The sweep also turned up **bespoke-but-present** feedback that is *not* an
AUDIT-1 gap (the component DOES give feedback) but IS an **AUDIT-2** target
(duplicate/one-off logic instead of the shared primitive):

- `mesh_control.rs:213`, `mesh_services.rs:323/609/616`, `routing.rs:320/904`,
  `home.rs:1955`, `mesh_federation.rs:640` — hand-rolled `Hovered`/`Pressed` match
  arms inside per-panel `.sty` closures instead of `controls::variant_button`.
- Counted via `grep -rn "_s: ButtonStatus" panels/` (no-feedback, G4/G5) vs.
  `grep -rn "Status::Hovered" panels/` (bespoke-but-present, AUDIT-2).

These are listed here only to keep the boundary clean: **AUDIT-1** = "no feedback
at all" (G1–G9); **AUDIT-2** = "feedback exists but isn't routed through the
shared module."

---

## Summary table

| Gap | Component | File:loc | Lift to |
|-----|-----------|----------|---------|
| G1 | shared button keyboard-focus ring (fork: no `Focused`) | `controls.rs:136`; fork `button.rs:677` | MOTION-FEEDBACK-1 (fork-blocked) |
| G2 | `toggle` knob slide + press | `controls.rs:244` | MOTION-FEEDBACK-1 |
| G3 | static `skeleton` + `spinner` | `controls.rs:450/479`; `connect.rs:649` | MOTION-NET-2 + AUDIT-2 |
| G4 | `mesh_federation` 11 no-feedback buttons | `mesh_federation.rs:745…` | MOTION-FEEDBACK-1 |
| G5 | `mesh_bus` 9 no-feedback buttons | `mesh_bus.rs:1208…` | MOTION-FEEDBACK-1 |
| G6 | text-input focus styling (fork-gated) | `controls.rs:216/237` (+38 raw sites) | MOTION-FEEDBACK-1 (fork-blocked) |
| G7 | sidebar nav/group no motion | `sidebar.rs:209/295` | FEEDBACK-1/-2 + TRANS-1 |
| G8 | `data_row` non-interactive (and dead) | `panel_chrome.rs:117` | MOTION-FEEDBACK-2 / AUDIT-2 |
| G9 | workbench panel-switch / data crossfade | `app.rs`; WORKLIST NET-3 `[>]` | MOTION-TRANS-1 + NET-3 (iced-0.14 gated) |
</content>
</invoke>
