# BRAND — Carbon system-wide branding ("Branding Toggle")

Operator-locked 2026-06-16. One-way Carbon branding applied automatically on
first boot of a **Workstation-role** node. Server/Lighthouse skip it.

## Locked decisions

| # | Decision | Lock |
|---|----------|------|
| 1 | Reversibility | **One-way apply** (no revert path). |
| 2 | Carbon icon set | **Operator-provided tarball** (a full freedesktop icon theme). The feature installs the tar to `/usr/share/icons/<Name>` and sets it as the default; the in-repo `assets/icons/carbon/` (22 app glyphs) is the app-internal set, not the system theme. |
| 3 | Theme breadth | **GTK (3/4) + Qt (5/6) + Cosmic** — every toolkit. |
| 4 | Cosmic app replacement | **Set MDE as default + swap the applet, keep Cosmic installed.** `mde-files` = default file manager (MIME + XDG); the Cosmic notification applet → `mde-cosmic-applet` in the panel config; `cosmic-files`/`cosmic-applet` remain installed as fallback. |
| 5 | Apply trigger | **Auto at first boot, Workstation role only** — a oneshot systemd service bundled in the RPM. |

## Environment (verified on .13, 2026-06-16)
- Display manager: **LightDM** (`lightdm` + `lightdm-settings` installed). Greeter to theme is LightDM's (detect gtk- vs slick-greeter at apply time).
- Current (pre-brand) theme: `Mint-Y-Purple` icons / `Greybird` GTK — i.e. NOT Carbon yet. "Match .13" therefore means match its **Cosmic layout**, with Carbon theming applied **on top**.
- Plymouth: stock `bgrt`.
- Cosmic config: ~32 `~/.config/cosmic/com.system76.*` dirs — the layout template.

## Architecture
A `magic-mesh-brand apply` orchestrator (idempotent, one-way) invoked by a
first-boot oneshot unit `magic-mesh-brand.service` (gated: only runs when the
pinned role is Workstation; writes a `/var/lib/mde/branded` stamp so it runs
once). Steps:
1. **Icons** — extract the operator tarball to `/usr/share/icons/<Name>`, run
   `gtk-update-icon-cache`, set icon-theme across gsettings/GTK/Qt/Cosmic.
2. **GTK 3/4** — install a Carbon GTK theme (generated from the `mde-theme`
   Carbon tokens — Gray 10/90/100) to `/usr/share/themes/Carbon`; set `gtk-theme`.
3. **Qt 5/6** — Carbon via `qt5ct`/`qt6ct` (or Kvantum) + `QT_QPA_PLATFORMTHEME`.
4. **Cosmic** — write the `com.system76.CosmicTheme` Carbon palette.
5. **Plymouth** — a `Carbon` plymouth theme from `assets/brand` art;
   `plymouth-set-default-theme -R Carbon` (rebuilds initramfs).
6. **LightDM greeter** — Carbon greeter config + `assets/brand/greeter-*`.
7. **Default apps / applet** — `xdg-mime` default → `mde-files.desktop`; panel
   config swaps the notification applet → `mde-cosmic-applet`.
8. **Layout** — seed the user's `~/.config/cosmic` from the baked **.13 layout
   template** (`assets/brand/cosmic-layout/`), notification applet already swapped.

## Acceptance (runtime-observable, per task)
- A freshly-installed Workstation boots into a Carbon-themed Plymouth → Carbon
  LightDM greeter → Cosmic session with the Carbon icon theme, Carbon GTK/Qt/
  Cosmic widget styling, mde-files as the default file manager, the MDE
  notification applet in the panel, and the .13 panel/dock/workspace layout.
- Server/Lighthouse nodes are untouched (no branding service runs).

## Out of scope
- Reverting branding (one-way by decision #1).
- A theme picker/per-user override (system default only).

## Tasks → see `docs/WORKLIST.md` BRAND-1..10.
