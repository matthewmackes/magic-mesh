> **HISTORICAL / SUPERSEDED (2026-07-22):** interface-paradigm design retired by the PLATFORM-INTERFACES standard (Apple-HIG-principled Construct + Car); see [docs/design/platform-interfaces.md](../design/platform-interfaces.md). Archived; do not implement from this document.

# DOCK-ACCENT — Carbon Blue accent line on the vertical dock's right edge

Operator-locked 2026-07-04 (5-Q survey). A thin Carbon Blue accent seam runs down the
**right edge** of the left vertical dock (the "left chooser"), layered just outside the
dock's existing hairline divider.

## Locked decisions (5)

| # | Area | Lock |
|---|------|------|
| 1 | Shade | **Carbon Interactive Blue 60** — the `Style` interactive-blue token (the SAME blue as the PICKER-GROUPS hairline; one blue language across the shell). |
| 2 | Weight | **1px hairline**. |
| 3 | Extent | **Full dock height** — top to bottom, edge to edge. |
| 4 | Behavior | **Brighten on reveal/hover** — dim-blue at rest, brightens (subtle) when the dock is revealed or hovered, via the shared `Motion` tokens (reduce-motion aware). |
| 5 | Divider | **Layered** — the blue hairline sits at the **outermost** right pixel; the existing subtle divider stays just inboard (blue seam + soft separator, two hairlines). |

## Architecture (mde-shell-egui / dock.rs)

- In `paint_dock_frame` (the dock `Area`'s frame paint), after the panel fill + the
  existing inboard hairline divider, paint a **1px vertical line at the panel's right
  edge x** (the outermost pixel), full height (`rect.top()..rect.bottom()`), stroked in
  the Carbon **Interactive Blue 60** `Style` token (reuse the same token
  `dock.rs`/PICKER-2 already reference for the blue hairline — no new hue, §4, no raw hex).
- **Reveal/hover brighten:** the stroke colour lerps between a dim and a full Blue 60
  by an `animate`d factor keyed on `state.shown()` (revealed/pinned) OR pointer-hover
  over the dock, using the shared `Motion` duration/easing (no literal duration in
  dock.rs, §4). At rest → dim; revealed/hovered → full Blue 60.
- The existing divider paint is unchanged (kept inboard); this ADDS the outer blue seam.
- A headless test asserts the right-edge blue stroke is present at the panel's right x,
  full height, in the Blue 60 token, and that the brighten factor tracks shown/hover.

## Acceptance (runtime-observable)
- A 1px Carbon Blue 60 hairline runs the full height of the dock's right edge, outside
  the existing divider; both are visible (layered).
- The blue is dim at rest and brightens on dock reveal/hover (Motion-driven, reduce-motion
  aware).
- All via `Style`/`Motion` tokens (§4 — no raw hex, no literal durations); style-leak grep clean.

## Notes
- Serialize on `dock.rs` with the VDOCK-6b teardown (both touch the frame paint).
- Tasks → `docs/WORKLIST.md` DOCK-ACCENT-1.
