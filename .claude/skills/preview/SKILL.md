---
name: preview
description: >-
  Render and visually verify the MCNF iced/Cosmic GUIs against the IBM
  Carbon reference. TRIGGER when the user wants to "preview", "screenshot the
  app", "verify the render", or confirm a visual change actually looks right
  (Carbon Gray 10 / 90 / 100). Use this instead of trusting a green `cargo test`
  for any UI change — launch the real app binary and inspect.
---

# preview — render & accuracy verification (MCNF)

A green `cargo test` does **not** verify the render. The desktop shell is gone
(Cosmic owns the desktop); what MCNF draws are its **iced 0.14 client areas** —
the Cosmic apps + the cosmic-applet. This skill verifies those render correctly
against the IBM-Carbon reference.

> **No bundled capture harness.** There is no `preview.sh` and no
> `tests/accuracy/` in this repo (those were the labwc-era shell's accuracy
> harness, retired in the E11 pivot). Verify by launching the real app binaries
> and inspecting; back it with the `mde-theme` token tests.

## Surfaces (each is its own binary)

```sh
cargo run -p mde-workbench    # the Cosmic control surface (fleet, devices, mesh health)
cargo run -p mde-files        # the file manager
cargo run -p mde-voice-hud    # voice/SIP HUD          (mde-voice-config = its config)
cargo run -p mde-music        # the music player        (mde-musicd = its daemon)
cargo run -p magic-fleet      # the Automation Mesh node engine
```

`mackesd` is a library crate (no binary); its surfaces are reached through the apps
above and `mde-bus` subscriptions. `salvage/from-mde-binary/` holds two not-yet-
rehomed surfaces (`birthright`, `mesh_status`) pending re-home onto Cosmic.

## How to use

1. **Build + launch + look.** `cargo build --workspace` first, then
   `cargo run -p <crate>` (or `timeout 10 ./target/debug/<bin>`) and inspect the
   client area against the change's intent. If running headless, capture with the
   system screenshot tool of the live Cosmic/Wayland session and **Read** the PNG.
2. **Quick no-panic check** for a single surface: `timeout 3 cargo run -p <crate>`
   — confirm it draws and doesn't panic on launch.
3. **One look, three grays.** The GUI is **strictly IBM Carbon**
   (carbondesignsystem.com); the only switchable themes are Carbon's gray themes —
   **Gray 10** (light), **Gray 90**, **Gray 100** (default dark). There are **no**
   era themes (Win2000/Windows10/BeOS — all retired). Carbon tokens (type scale, 8px
   spacing, components, 2px focus, motion) are single-sourced in `mde-theme`
   (`crates/shared/mde-theme`); flip the active gray theme there or via the app's
   own theme control, and restore it after.
4. **Static token check** (always headless-safe): `cargo test -p mde-theme` — the
   Carbon token / palette / metric ground truth. If a render looks off, suspect a
   `mde-theme` token edit before the surface code.

## Notes

- No raw hex / scattered metric literal lives anywhere but the `mde-theme` token
  modules (`AI_GOVERNANCE.md` §4) — a lint gate enforces this. Cosmic draws the
  panel/decorations; MCNF only draws its client areas.
- Pure-Rust toolkit: iced 0.14 (wgpu) + cosmic-text (no FreeType), rustls (no
  OpenSSL). A full build needs `gtk3-devel` + `alsa-lib-devel`.
- Visual verification is the §7 Definition-of-Done gate for any UI change — do not
  mark a task `[✓]` in `docs/WORKLIST.md` on a green `cargo test` alone.

See also: `/audit` (find dead/mock/stub UI), `/ship` (drain the worklist, accuracy-
verifying each change), `/release` (operator-gated RPM cut).
