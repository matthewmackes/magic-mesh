# E12-3 shell composition — the six surfaces become panels in the one shell

> **Design-standard note (2026-07-22):** look-and-feel guidance in this doc is subordinate to the platform interface standard — see [platform-interfaces.md](platform-interfaces.md) (Apple-HIG-principled Construct + Car). Feature/behavior content remains authoritative.

**Status:** LOCKED + executing 2026-06-30 (operator: "yes, E12-3 execute").
**Scope of this doc:** the *composition* piece of E12-3 — mounting the polished
surfaces into `mde-shell-egui`. (The DRM-seat boot + PAM login/lock parts of the
E12-3 worklist item ride E12-2's DRM runner + a later login unit; they are
hardware/PAM-gated, not this doc.)

## Decision: EMBED, not broker

The surfaces (Music / Files / Voice) run as **embedded egui widgets inside the
shell process**, not as separate windows/processes the shell composites. Forced by
three facts, so no survey was needed:

1. **Construct §5 lock:** "the mesh-control surfaces (Workbench/Files/Music/Voice) are
   **panels inside the one shell**, not separate clients."
2. **No compositor (§5):** the egui shell owns the DRM/KMS seat directly — there is
   no Wayland compositor to composite other processes' windows. So a surface is
   either an embedded widget in the shell's `egui::Context`, or a texture (the VDI
   path, which is for VM desktops, not our own control surfaces).
3. **The surfaces are already libraries:** `mde-music-egui` / `mde-files-egui` /
   `mde-voice-egui` each already have a `lib.rs` holding the `eframe::App` view
   logic. Embedding is a small extraction, not a rewrite.

The monolith trade-off (one process runs Music+Files+Voice+Workbench) is inherent
to the seat-owning model and acceptable: these are lightweight mesh-control
surfaces; real apps live in VM guests (the isolation boundary), rendered as
textures. A surface panic is contained by egui's per-frame error handling, not a
separate process.

## Decomposition (farm-dispatchable, file-disjoint)

- **E12-3a-{music,files,voice}** — each surface lib exposes
  `pub fn <name>_panel(ui: &mut egui::Ui, state: &mut <its state>)` that renders its
  central view into a given `ui`. Its own binary's `App::update` calls the same fn
  (behaviour unchanged standalone). Pure extraction + a headless render smoke test.
  Three disjoint units, parallel.
- **E12-3b-mount** — `mde-shell-egui` depends on the three surface crates and mounts
  them as shell panels (a dock/launcher alongside the Workbench). The Datacenter /
  Fleet plane is already mounted (MV-6 `datacenter.show`); this adds the app panels.
  Depends on E12-3a.

## Acceptance (composition slice, §7)
- The shell renders the Music / Files / Voice surfaces as in-shell panels (their
  real views + live data, the same the standalone binaries show), reached from the
  shell chrome/launcher — no separate window, no external process.
- Each surface's standalone binary still builds + runs (the extraction kept it a
  thin frame wrapper over the shared panel fn).
- Everything draws through `mde_egui::Style`; the workspace builds + tests green.

## Out of scope (other E12-3 / later units)
- DRM-seat boot via systemd + "no display manager" (E12-2 DRM runner + a boot unit).
- PAM login/lock against mesh identity (a later shell-session unit).
- Wiring the under-populated Workbench planes (ThisNode/Controller/Network/
  Provisioning currently show descriptive copy) — a follow-up; Fleet is live (MV-6).
