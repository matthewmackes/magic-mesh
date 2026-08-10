# WL-UX-012 — taskbar action-map hard cut (r4)

Date: 2026-08-10

Base revision: `4ba2428d`

## Production behavior

The full-width taskbar now exposes four distinct typed base actions:

- Start opens the one existing Front Door;
- Search opens and focuses that same Front Door search field;
- Back consumes the typed navigation history; and
- Home idempotently returns to the clean Bing-wallpaper Home.

The conflicting Home/Sessions click cycle and its retained navigation state
were deleted. Home no longer alternates between wallpaper, restore, and a
session chooser, so it cannot behave as a second launcher. Bottom and Left
geometry both reserve separate Start and Search controls and account for the
extra fixed slot in narrow-layout admission.

The action enum contains no raw-command or secondary-launcher variant. Search
focus is runtime-reachable because `FrontDoorState::open` arms
`focus_pending`, and the production renderer consumes it with
`response.request_focus()`.

## Focused farm proof

BigBoy slot `ux012-s2-actions` passed:

- two `taskbar_s2_` action/Home tests;
- `non_zero_screen_pointer_hits_each_base_control` (1/1); and
- `bottom_taskbar_does_not_emit_center_controls_without_a_physical_slot`
  (1/1).

`git diff --check` also passed. Package-wide formatting has unrelated existing
drift, so it was not used as evidence for this bounded action-map correction.
