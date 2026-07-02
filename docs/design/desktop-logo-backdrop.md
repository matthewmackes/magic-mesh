# Desktop logo backdrop — the empty-desktop brand lockup

*Operator directive 2026-07-02: "This image (`assets/brand/logo-lockup.png`) should
be at the center of the desktop when empty with no text over the image." Locked via a
10-question survey.*

The shell (`mde-shell-egui`) already draws an honest **"no-desktop EmptyState"**
(`vdi.rs` / `discovery.rs`) when nothing is attached. This feature makes the brand
lockup the desktop's persistent backdrop layer — centered, large, text-free — and
governs how it dims as content appears.

## Locks (10-Q survey)

| # | Question | Decision |
|---|---|---|
| 1 | What "empty" triggers it | **Persistent desktop backdrop** — the logo is the bottom-most background layer, revealed whenever nothing covers it. It replaces the text no-desktop EmptyState *and* shows behind an empty root desktop. |
| 2 | Text handling | **Logo only.** No text over the image ever. Any honest status (connecting…, connection lost, node id) renders as a small line placed **well below** the logo, never overlapping. Preserves §7 honesty. |
| 3 | Size | **Large hero** — target ~50% of the viewport's shorter dimension. |
| 4 | Centering | **Optical center of the free content area** — centered in the space *between* the top chrome bar and the dock, not the raw viewport middle. |
| 5 | Multi-display | **Every empty display** renders its own centered logo. |
| 6 | Partial coverage | **Dim to a watermark, show in gaps.** Stays as a backdrop; drops to a low-opacity watermark once any surface/window opens, so gap-peeking is subtle rather than absent. |
| 7 | Asset delivery | **Embedded in the binary** via `include_bytes!` — always present, no FS/RPM-path dependency. |
| 8 | Treatment | **Full-opacity logo on the solid Carbon §4 background token** (the canonical empty-canvas color). No raw hex. |
| 9 | Crispness | **Downscale-only, capped at native 1270 px.** Render up to native size; never upscale past it (stays crisp; on very large displays the hero simply stops growing at native). |
| 10 | Motion | **Crossfade on state change + slow idle breathe.** Fade-in when the empty state appears; smooth crossfade to/from the dimmed watermark (Q6); a very slow opacity "breathe" while idle. |

## Architecture

- **Where.** A backdrop render pass at the bottom of the shell's desktop paint, shared
  by the `vdi.rs` no-desktop EmptyState and the empty root-desktop path
  (`discovery.rs`). Extract the lockup into a small `backdrop` helper the empty paths
  call, replacing the current text-only EmptyState body (the status line moves below
  the logo per Q2).
- **Asset.** `include_bytes!("../../../../assets/brand/logo-lockup.png")` decoded once
  to an `egui::TextureHandle` (lazy, cached in shell state; one upload, not per-frame).
  1270×1270 RGBA.
- **Layout.** Compute the free rect = viewport minus the top chrome height and the dock
  height (both already known to the shell). Logo side = `min(free.w, free.h) * 0.5`,
  clamped to ≤ 1270 logical px, DPI-aware. Placed at the free rect's center.
- **Coverage state.** A shell-level opacity target: `1.0` when the display is fully
  empty, a low watermark alpha (e.g. ~0.12) when any surface/window is open on that
  display. Per-display (Q5) — each display resolves its own empty/covered state.
- **Motion.** An eased opacity animation toward the target (crossfade), plus a slow
  sinusoidal breathe (small amplitude) applied only while at the full-opacity idle
  state. Driven by egui's frame time; `request_repaint` while animating/breathing.
- **Tokens.** Backdrop fill = the Carbon §4 background token from `mde-theme`; the
  status line (below) uses the existing muted-text token. No raw hex, no scattered
  metric literals (§4).

## Acceptance (runtime-observable)

- On an empty display (no desktop attached, or an empty root desktop) the lockup renders
  centered in the free area at ~50% of the shorter side, at full opacity, on the Carbon
  background token, with **no text over the image**.
- Any status text renders as a small line clearly below the logo, never overlapping.
- The logo never upscales past 1270 px (crisp); it downscales on smaller free areas.
- Each empty display shows its own centered logo; a covered display drops the logo to a
  low-opacity watermark that remains visible in uncovered gaps.
- State changes crossfade; the idle logo breathes slowly.
- The asset is embedded (the binary renders it with no external file present).
- `cargo build/test/clippy -p mde-shell-egui` green; egui snapshot test of the empty
  backdrop; **zero raw hex** in the new code; `lint-layered-tiers.sh` clean.

## Out of scope

- User-selectable wallpapers / a wallpaper picker (this is the brand backdrop only).
- Theming/override of the lockup file (embedded default only; an override path is a
  possible later addition, not this unit).
- Animated/video backdrops.

## Risks

- **Optical-center drift** if the chrome/dock heights aren't the ones actually painted —
  must read the real bar heights the shell uses, not constants.
- **Breathe distraction** — keep the amplitude and period conservative; it must read as
  "alive," not "pulsing."
- **Texture memory** — one 1270² RGBA texture (~6.4 MB) per process is fine; decode once,
  never per-frame.
