# WL-UX-009 direct-DRM evidence — 2026-08-01

This is representative production-validation evidence for the shared Quazar
workspace frame. It does not close WL-UX-009 or claim production readiness.

## Seat and build

- Seat: `.138` (`172.20.146.138`), physical Fedora 44 DRM seat.
- Connector: `card1-eDP-1`, connected; capture mode `1920x1080`.
- Shell payload: `a40013fc62162da957e9e0df3619b0ddb1b3262cd36b8d077843a38c14f0399c`.
- Farm RPM: `magic-mesh-12.1.6-1.x86_64.rpm`, SHA-256
  `53835242591b1d3217fc2d83f14021f9cba41fb68a86c295d7c7373ffbee4d75`.
- Build: BigBoy farm `172.20.0.130`, slot 11, release shell with
  `drm,live-helper,live-vdi,media-mpv`.
- Runtime: `mde-shell-egui.service` active, `NRestarts=0` after deployment.
- Dell deployment: `172.20.146.225` (`DELL-LAPTOP`) received the same extracted
  shell payload; service active with `NRestarts=0`.
- The complete RPM was not installed on either seat because its Fedora build
  dependency set does not match the installed seat runtime. The verified shell
  payload was deployed directly for GUI proof, with the prior binary preserved
  on each seat as `/tmp/mde-shell-egui-before-appframe`.

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
| Dell Dark desktop (new shell payload) | Mesh Teams → Files → Editor | native desktop capture, `1366x768` | `1577918fe3f6477080c0ec6712391f2eb3018214e190908bebd0e944240f35b6` |
| Dell Dark System (new AppFrame payload) | System → Wallpaper | native desktop capture, `1366x768` | `f2a9d743deca96566651f0ac8caae3a0d313bd56c045aad10e92d70956c48958` |
| Dell Light / Largest System | System → Wallpaper | native `1366x768`; large text continues in the scrollable settings body | `35e20f37fc4685baf8edd4594d9789dd8ffd491edb8441f7d70c860a99170238` |
| Dell Dark Bookmarks (AppFrame detail payload) | Bookmarks → Details | native desktop capture, `1366x768` | `a74b0d78cba9b2b01059740573c68f5b5d9ac88971ab2153d055dc4dad652c42` |
| Dell Light / Largest Bookmarks | Bookmarks → Details | native desktop capture, `1366x768`; large text and scrollable body | `e7f7d1dc2ca4d880fbbe19d0fbb0225a738f2500fa42d837d4e0ab151e953742` |

| Dell Dark narrow Bookmarks | Bookmarks → Details | `800` logical width on native scanout; unused right side is intentional | `2db329f128c6d8d7af8e948b2bd5481bba8a3a93e1242a5fa1d73866ea953bed` |
| Dell Dark Storage | This Node → Storage | native desktop capture, `1366x768`; disk body continues below scroll boundary | `d8a7d843f6071d9041ab1f863e1050115f18c5d7fdadff8b2f20a1ccaa5fa453` |
| Dell Dark narrow Storage | This Node → Storage | `800` logical width; unused right side is intentional and disk body scrolls vertically | `8cd8f3d7882c54bf3bdfd30d51178b8b42d1840d41f402bb4d1c881a7e81a321` |
| Dell Light / Largest Storage | This Node → Storage | native desktop capture, `1366x768`; large-text disk body continues below scroll boundary | `06e2fa4bb72088f74f39008434ed06ba4ea6e932faf72f7e7a060a24f976c0bd` |
| Dell Dark Files (payload `44296eee`) | Files | native `1366x768` direct-DRM capture; shared MenuBar, sidebar, file list, preview pane, and status strip remain separated | `25aadf12b0dd4255feb1eed7d9890f3fbcefc646f2074695b9325cfb89da3fc7` |
| Dell Dark narrow Files (payload `44296eee`) | Files | `800` logical width; file toolbar, sidebar, list, preview boundary, and bottom status remain readable without horizontal overlap | `ff6e00b5d23cba1c8948f7c7c7841b20649b26e727444a9c9ed612cf61ff7ed1` |
| Dell Light / Largest Files (payload `44296eee`) | Files | native `1366x768` direct-DRM capture; large-text sidebar state rows are fully visible above the bottom status strip | `2f528d356901b79be899217a48e72a7a689cb5811cdd8139751ff7095e0febb9` |
| Dell Dark Mesh Teams (payload `44296eee`) | Mesh Teams → Activity | native `1366x768` direct-DRM capture; shared workspace title, channel header, activity feed, detail rail, and call bar remain separated | `58d722ba2f90a84f50b6d1f300b1fcd5da4aad5f2a97af63765834f1b280840c` |
| Dell Dark narrow Mesh Teams (payload `44296eee`) | Mesh Teams → Activity | `800` logical width; compact channel tabs and live activity feed remain readable with governed side rails omitted | `d3264288d33c5f6eb3765b42c345fc1d1e3eccc70e35eed6efc94cc5a4ec3911` |
| Dell Light / Largest Mesh Teams (payload `44296eee`) | Mesh Teams → Activity | native `1366x768` direct-DRM capture; large-text activity rows, channel tabs, and bottom taskbar remain readable without clipping | `b3f8b80240d8ea70830d0fc77db022f34535c8a7607d4c7e55987c145c0388db` |

| Dell Dark Phones (`AppFrame`, payload `035c4f3a`) | Phones | native `1366x768` direct-DRM capture; centered shared frame title and paired-device card render without overlap | `d8e3c1ef03adc371e424372846ce0fc44ff4351d1f100fc555049ab2c65e115f` |
| Dell Dark narrow Phones (`AppFrame`, payload `035c4f3a`) | Phones | `800` logical width; centered frame title, status row, and clipboard field remain unobstructed | `6d203641bf923fb3b09c5f47512e5c3c947d06b121ca0c0155dcf8d2f469dc46` |
| Dell Light / Largest Phones (`AppFrame`, payload `035c4f3a`) | Phones | native `1366x768` direct-DRM capture; large-text frame, status row, and scrollable body remain readable | `457f62ce0e861feba10a50cc331cb5207cc0eb12482979d8f4e79e2bebffddff` |
| Dell Dark Timers (AppFrame payload `f49ae072`) | Timers & Alarms | native `1366x768` direct-DRM capture; timer/alarm controls and taskbar remain separated | `7b60a2b6fd3b27f2b6e7eff978efd1bd6ce431f727f36e8e922c098a06d6b183` |
| Dell Dark narrow Timers (AppFrame payload `f49ae072`) | Timers & Alarms | `800` logical width; controls remain readable with no horizontal overflow or floating-control overlap | `5fa6d47f3c22bc3f75d62bfbed954508ffdf96a0b275ea301a990e02e63ae272` |
| Dell Light / Largest Timers (AppFrame payload `f49ae072`) | Timers & Alarms | native `1366x768` direct-DRM capture; large-text timer/alarm body is readable and the profile control is absent when no bottom taskbar band is reserved | `a4dd2eed3cf376d3532772c8638bb6da0565839560dd0bf1584c847e6ec16218` |

The captures show the shared MenuBar, AppFrame detail header, Editor,
Bookmarks, and Storage workspace bodies, toolbars, side rails, taskbar,
Dark/Light palettes, and large-text containment without the
previous body collapse or scanout corruption. Phones and Timers now also have direct Dark,
Dark narrow, and Light/Largest proof; Files now also has current-payload direct
Dark, Dark narrow, and Light/Largest proof after tightening its large-text sidebar
gaps; Mesh Teams now has current-payload direct Dark, Dark narrow, and
Light/Largest proof while retaining its domain channel/call chrome; Phones uses
the shared `AppFrame`, and the
narrow layout keeps the workspace field clear by delegating the profile toggle to
Control Center. The narrow capture intentionally
uses an 800-point logical viewport on the 1920-pixel panel; the unused right
side is outside the proof viewport, not an overlap.

## Automated evidence

- `mde-egui` full package suite: 268 passed, 0 failed.
- `mde-egui` focused test:
  `menu_bar_leaves_body_space_on_desktop_and_narrow_layouts` — passed for
  1280px and 800px layouts.
- `mde-egui` focused navigation suite — 8 passed, including AppFrame large-text
  composition and Sidebar keyboard selection.
- `mde-shell-egui workbench::tests` — 5 passed, including the Workbench pane
  rendering through the shared `AppFrame` primitive.
- `mde-shell-egui system::tests::the_sidebar` — 3 passed, including the
  selected-section title count; `system::tests::a_narrowed_sidebar` — 1 passed.
- `mde-shell-egui::tests::layout_profile_button_sits_in_the_taskbar_gap_clear_of_surface_content` — passed.
- `mde-shell-egui::tests::compact_width_uses_control_center_for_the_layout_toggle` — passed.
- `mde-shell-egui::timers::tests::the_panel_renders_headless_over_real_state` — passed after the Timers `AppFrame` migration.
- `mde-files-egui` full package suite — 165 passed, 0 failed, including the
  real Files sidebar and large-text-compatible rendering fixtures.
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
