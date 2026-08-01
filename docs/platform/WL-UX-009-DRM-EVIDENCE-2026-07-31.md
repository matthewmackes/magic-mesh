# WL-UX-009 direct-DRM evidence — 2026-08-01

This is representative production-validation evidence for the shared Quazar
workspace frame. It does not close WL-UX-009 or claim production readiness.

## Seat and build

- Seat: `.138` (`172.20.146.138`), physical Fedora 44 DRM seat.
- Connector: `card1-eDP-1`, connected; capture mode `1920x1080`.
- Shell artifact: `bea573f929239e819eeeca2425ad8f1132485fadb1f8fb010f9ed97376dab8b7`.
- Build: BigBoy farm `172.20.0.130`, slot 11, release shell with
  `drm,live-helper,live-vdi,media-mpv`.
- Runtime: `mde-shell-egui.service` active, `NRestarts=0` after deployment.
- Dell deployment: `172.20.146.225` (`DELL-LAPTOP`) received the same artifact;
  service active with `NRestarts=0`.

## Captures

The capture harness used the proof-only `MDE_DRM_LINEAR_SCANOUT=1` environment
on the direct DRM seat. The first attempts were rejected because the seats
were still on the secure boot curtain or first-boot pin selector. After those
states were configured explicitly for the proof run, clean frames were
captured and visually inspected. The proof-only overrides and temporary
appearance/power settings were removed afterward; both seats returned to
normal secure runtime settings.

| Seat / profile | Route | Logical proof geometry | PNG SHA-256 |
| --- | --- | --- | --- |
| `.138` Dark desktop | Mesh Teams → Files → Editor | `1536x864` at `1.25` ppp | `b81178e67eed91a4b488e0efd858c73a9ca11b99657b6775800dc40857a0ad38` |
| `.138` Dark narrow | Mesh Teams → Files → Editor | `800x576` at `1.875` ppp; right-side unused scanout is intentional | `2975ee02529543deb2f237021bf377c420d22d1908b8af7305485a2d99401c75` |
| `.138` Light / Largest | Mesh Teams → Files → Editor | `1024x576` at `1.875` ppp | `32b9267765783a4019664ad87f3cee43fa81a5b48c0ef0233d93410620d5e2e0` |
| Dell Dark desktop | Mesh Teams → Files → Editor | native desktop capture, `1920x1080` | `10afd8cc2ee620817a3b43c4a902cb91223fecc52f7688dc256e3d62b4e0810d` |

The captures show the shared MenuBar, Editor workspace body, toolbars, side
rails, taskbar, Dark/Light palettes, and large-text containment without the
previous body collapse or scanout corruption. The narrow capture intentionally
uses an 800-point logical viewport on the 1920-pixel panel; the unused right
side is outside the proof viewport, not an overlap.

## Automated evidence

- `mde-egui` full package suite: 268 passed, 0 failed.
- `mde-egui` focused test:
  `menu_bar_leaves_body_space_on_desktop_and_narrow_layouts` — passed for
  1280px and 800px layouts.
- `mde-egui` focused navigation suite — 8 passed, including AppFrame large-text
  composition and Sidebar keyboard selection.
- Worklist acceptance remains open for the full workspace matrix, remaining
  Construct-owned routes/states, and final production evidence. This is clean
  representative proof, not a claim that every route is production-ready.

## Boundary authority

The approved rendering boundaries remain documented in
[`docs/design/platform-interfaces.md`](../design/platform-interfaces.md):
focused VDI retains guest pixels, Maps retains its governed content-color
exception, and the Browser VM owns Chromium pixels and chrome. Construct owns
the connection, unavailable, reconnect, and diagnostic states around those
guests; no additional visual exceptions are introduced here.
