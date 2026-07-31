# WL-UX-009 direct-DRM evidence — 2026-07-31

This is representative production-validation evidence for the shared Quazar
workspace frame. It does not close WL-UX-009 or claim production readiness.

## Seat and build

- Seat: `.138` (`172.20.146.138`), physical Fedora 44 DRM seat.
- Connector: `card1-eDP-1`, connected; capture mode `1920x1080`.
- Shell artifact: `4ba0b2f74db60894527bdd0866c62a3646d26330dcc98a8132777cb1e3610cd1`.
- Build: BigBoy farm `172.20.0.130`, slot 11, release shell with
  `drm,live-helper,live-vdi,media-mpv`.
- Runtime: `mde-shell-egui.service` active, `NRestarts=0` after deployment.

## Captures

The previous `2f427f…` artifact had representative captures reviewed during
the earlier pass, but those files are not retained as current proof. A fresh
capture attempt for the exact `4ba0b2f…` artifact produced scanout-corrupted
line artifacts on all three profiles, so those PNGs are explicitly rejected
and do not count as visual evidence.

| Profile | Route | Result |
| --- | --- | --- |
| Dark desktop | Mesh Teams → Files → Editor | Rejected: corrupted KMS scanout lines |
| Dark narrow | Mesh Teams → Files → Editor | Rejected: corrupted KMS scanout lines |
| Light / Largest | Mesh Teams → Files → Editor | Rejected: corrupted KMS scanout lines |

The earlier focused proof caught the shared MenuBar body-collapse defect and
the large-text Sidebar overflow defect; the farm tests below prove those
layout contracts. The live recapture must be repeated with a clean KMS frame
before any visual claim or readiness decision is made.

## Automated evidence

- `mde-egui` full package suite: 268 passed, 0 failed.
- `mde-egui` focused test:
  `menu_bar_leaves_body_space_on_desktop_and_narrow_layouts` — passed for
  1280px and 800px layouts.
- `mde-egui` focused navigation suite — 8 passed, including AppFrame large-text
  composition and Sidebar keyboard selection.
- Worklist acceptance remains open for the full workspace matrix, Dell, all
  remaining route captures, and final production evidence.

## Boundary authority

The approved rendering boundaries remain documented in
[`docs/design/platform-interfaces.md`](../design/platform-interfaces.md):
focused VDI retains guest pixels, Maps retains its governed content-color
exception, and the Browser VM owns Chromium pixels and chrome. Construct owns
the connection, unavailable, reconnect, and diagnostic states around those
guests; no additional visual exceptions are introduced here.
