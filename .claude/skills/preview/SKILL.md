---
name: preview
description: >-
  Render and visually verify the MCNF egui/wgpu surfaces against the Quasar
  dark design language (Carbon-inspired, dark-only). TRIGGER when the user
  wants to "preview", "screenshot the app", "verify the render", or confirm a
  visual change actually looks right. Best-effort eyes-on evidence — the visual
  gate is LIFTED (operator 2026-06-11): a green build + tests + style-leak grep
  ships a UI change; this skill is how you *look*, never a blocker.
---

# preview — render & accuracy verification (MCNF, E12 egui era)

A green `cargo test` does **not** show you the render. Every MCNF surface is an
**egui panel** drawn through the shared `crates/shared/mde-egui` harness
(eframe = egui + winit + wgpu): on real hardware the shell owns the DRM/KMS
seat directly; in development each surface also runs as an ordinary **windowed
Wayland client** via the harness's `run_client` fallback — that windowed path
is what this skill drives.

> **The visual gate is lifted (operator, 2026-06-11 — reaffirmed in the
> 2026-07-03 design survey).** A UI change is *done* when it builds, tests
> green, and passes the style-leak grep (see `/polish`). Preview is
> **best-effort evidence, never a §7 gate** — do not hold a task `[>]` waiting
> for eyes-on, and do not mark visual verification as a blocker in
> `docs/WORKLIST.md`.

## Surfaces (E12 — all egui, all in `crates/desktop/`)

```sh
cargo run -p mde-shell-egui     # THE shell (chrome bar → Workbench)
cargo run -p mde-panel-egui     # panel chrome
cargo run -p mde-files-egui     # files panel
cargo run -p mde-music-egui     # music panel (+ mde-media-egui)
cargo run -p mde-voice-egui     # voice/SIP panel
cargo run -p mde-editor-egui    # editor panel
cargo run -p mde-term-egui      # terminal panel
cargo run -p mde-bookmarks-egui # bookmarks panel
cargo run -p mde-mesh-view      # mesh map view
```

Plus the VDI viewers (`mde-vdi-rdp` / `mde-vdi-spice` / `mde-vdi-vnc`) and
`mde-web-preview-client`. The iced/libcosmic binaries (`mde-workbench`,
`mde-files`, `mde-music`, `mde-voice-hud`, `magic-fleet`) are **retired** — if
one still runs, that's a rescue finding for `/polish` or `/audit`, not a
preview target.

## How to use

1. **Build + launch + look.** `cargo build -p <crate>` (ride the farm via
   `install-helpers/xcp-build.sh` for cold builds — see `/polish`; *rendering*
   needs a live Wayland session + GPU, so run where a session exists), then
   `cargo run -p <crate>` and inspect the panel against the change's intent.
2. **Quick no-panic check:** `timeout 3 cargo run -p <crate>` — confirm it
   draws real state (not `demo_data`) and doesn't panic on launch.
3. **Capture a PNG and `Read` it.** Until the harness grows its own capture
   hook, screenshot the windowed client with the live session's screenshot
   tool and **Read** the PNG. The right long-term hook is egui's
   `ViewportCommand::Screenshot` → `Event::Screenshot` wired into `mde-egui`'s
   `run_client` (dump a PNG on an env flag and exit) — adding it is a
   high-value `/polish` unit, already called out there.
4. **One theme: Quasar dark.** The look is the Carbon-inspired **Quasar dark**
   design language (dark-only; the Gray 10/90/100 switching of the mde-theme
   era is retired). Verify at **two scales** — 1.0 and a fractional
   `pixels_per_point` — and, where the surface supports it, both `Density`
   modes (Mouse/Touch): spacing may change, component dimensions must not
   (UX-24).
5. **Static ground truth (always headless-safe):** `cargo test -p mde-egui` —
   the `Style`/`Motion`/fonts values are render-agnostic data with unit tests.
   If a render looks off, suspect a shared-`Style` edit before the surface
   code. Then run the **style-leak grep** from `/polish` (zero
   `Color32::from_*` / literal durations in `crates/desktop`, pixel decoders
   and the ANSI palette excluded).

## What to look for (the fast checklist)

- Spacing on the 8px rhythm; no cramped or ragged gutters.
- Mono-first type (headings/nav/data in the mono, prose in the sans); no
  off-scale sizes.
- Soft-Carbon depth: 4–8px radii, layered soft shadows, subtle-translucent
  scrims only (no gaussian blur).
- Motion from the shared `mde_egui::motion` table — springs/inertia/
  micro-interactions feel continuous, and nothing animates from a bespoke
  literal.
- 2px focus ring visible; the panel is keyboard-reachable.
- Empty/loading/error states are designed (skeleton/empty-state), never a
  blank panel or frozen spinner; absent backends render an honest
  "not available" (§7).
- 1px strokes land crisp on pixel boundaries at fractional scales; textures
  (VDI planes, album art) aren't stretched.

## Notes

- The single source of look is `mde_egui::{Style, Motion, fonts}`
  (`AI_GOVERNANCE.md` §4) — surfaces never hand-roll a colour or duration; the
  style-leak grep in `/polish` is the mechanical check.
- Pure-Rust stack: egui/eframe 0.31 on wgpu (Vulkan via Mesa on hosts), rustls.
  Build prerequisites live in `docs/BUILD-ENVIRONMENT.md`.
- Frame time is observed here, never gated — a hitch you can see is a bug to
  file, not a polish gate (survey lock, 2026-07-03).

See also: `/polish` (the refinement loop this skill gives eyes to), `/audit`
(find dead/mock/stub UI), `/ship` (drain the general worklist), `/release`
(operator-gated RPM cut).
