# WL-UX-009 direct-DRM evidence — 2026-08-01

This is representative production-validation evidence for the shared Quazar
workspace frame. It does not close WL-UX-009 or claim production readiness.

## Seat and build

- Seat: `.138` (`172.20.146.138`), physical Fedora 44 DRM seat.
- Connector: `card1-eDP-1`, connected; capture mode `1920x1080`.
- Shell payload for the current proof rows: `703d3477ce574e30abda5f1d7bdc46834ad16881db8d3a1d727b794fc1222919`.
- Farm RPM: `magic-mesh-12.1.6-1.x86_64.rpm`, SHA-256
  `53835242591b1d3217fc2d83f14021f9cba41fb68a86c295d7c7373ffbee4d75`.
- Build: BigBoy farm `172.20.0.130`, slot 11, release shell with
  `drm,live-helper,live-vdi,media-mpv`.
- Runtime: `mde-shell-egui.service` active, `NRestarts=0` after deployment.
- Dell deployment: `172.20.146.225` (`DELL-LAPTOP`) received the same extracted
  shell payload; service active with `NRestarts=0`. The final Maps validation
  payload is `b88b6b8877a1fcf9ab441156a50c620b6e48ed1c244df56ceb0c036d38205f52`.
- Dell current shell proof payload: `703d3477ce574e30abda5f1d7bdc46834ad16881db8d3a1d727b794fc1222919`;
  service active with `NRestarts=0` during the final direct-DRM captures below.
- New Maps contrast-fix candidate payload: `8e35703c40721f9e0f031b93f3b48202a97cffcd366192135c7518df5cd83c23`;
  built on farm `.90` and deployed to Dell and `.138` on 2026-08-01. Both
  services report active with `NRestarts=0`; no live render pass is claimed
  until the replacement Maps frames are visually inspected.
- Replacement Maps capture attempt (2026-08-01): both seats returned an Intel
  X-tiled primary-plane modifier (`0x100000000000001` on `.138`; Dell's XR30
  frame also converted to visibly striped scanlines). These PNGs are invalid
  proof and are intentionally excluded from the passing capture table below.
  The temporary proof drop-ins and appearance overrides were removed; both
  seats are back to `require_login_at_boot:true` with `NRestarts=0`.
- Dell Device Manager strip-removal payload: `02bceab53a5b8fb48391f9f4cbe047cae2b6c37dd9d6972dc96c5083eabcb1d9`;
  service active with `NRestarts=0` after deployment. The KMS capture device
  acquired a real scanout frame (`ffmpeg -f kmsgrab ... -f null -` exit 0),
  but this seat's FFmpeg build could not convert its DRM-prime frame to PNG;
  no visual screenshot claim is made for this route yet.
- Prior proof allocator / Maps compile-integrity payload: `703d3477ce574e30abda5f1d7bdc46834ad16881db8d3a1d727b794fc1222919`;
  deployed to Dell and `.138` on 2026-08-01. Both services reported active with
  `NRestarts=0`. The `.138` proof retry still exposed XR30 scanout and failed
  FFmpeg DRM-prime-to-RGBA conversion, so no new visual screenshot claim is
  made. `.138` was restored to the secure login-at-boot runtime afterward.
- Proof telemetry/fail-closed payload `f69e42551724fae346b91bb4965194905d6583f62cf4265394f0fa49a8a8fc7e`
  was built cleanly on farm `.90` and installed on Dell and `.138`; both
  services are active with `NRestarts=0`. On `.138`, the proof-only linear
  allocation failed with `EINVAL`; the actual locked front buffer was logged
  as `XR30`, modifier `I915_x_tiled`, stride `7680`, and the application
  rejected it as non-linear. No screenshot was accepted from this run.
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
| Dell Dark Maps (payload `b88b6b88`) | Maps & Location → Drive | native `1366x768` direct-DRM capture; six radio/GNSS slots, alert state, map content boundary, FAB lane, and taskbar remain separated | `e96231b7377e7eaa2b1ef06f6c2d54413bb31dd2e20b6f1d644ec4743a821f06` |
| Dell Dark narrow Maps (payload `b88b6b88`) | Maps & Location → Drive | `800` logical-width proof route; the map HUD remains bounded with the radio rail separated from the FAB lane and alerts | `e96231b7377e7eaa2b1ef06f6c2d54413bb31dd2e20b6f1d644ec4743a821f06` |
| Dell Light / Largest Maps (payload `b88b6b88`) | Maps & Location → Drive | native `1366x768` direct-DRM capture; compact two-row health slots, large text, alert state, map content boundary, FAB lane, and taskbar remain readable without clipping | `6528918ade3d11abc1c2ef4afe4f0dade1b4e2d3eae0cb61af20edd9373b5749` |

| Dell Dark Phones (`AppFrame`, payload `035c4f3a`) | Phones | native `1366x768` direct-DRM capture; centered shared frame title and paired-device card render without overlap | `d8e3c1ef03adc371e424372846ce0fc44ff4351d1f100fc555049ab2c65e115f` |
| Dell Dark narrow Phones (`AppFrame`, payload `035c4f3a`) | Phones | `800` logical width; centered frame title, status row, and clipboard field remain unobstructed | `6d203641bf923fb3b09c5f47512e5c3c947d06b121ca0c0155dcf8d2f469dc46` |
| Dell Light / Largest Phones (`AppFrame`, payload `035c4f3a`) | Phones | native `1366x768` direct-DRM capture; large-text frame, status row, and scrollable body remain readable | `457f62ce0e861feba10a50cc331cb5207cc0eb12482979d8f4e79e2bebffddff` |
| Dell Dark Timers (AppFrame payload `f49ae072`) | Timers & Alarms | native `1366x768` direct-DRM capture; timer/alarm controls and taskbar remain separated | `7b60a2b6fd3b27f2b6e7eff978efd1bd6ce431f727f36e8e922c098a06d6b183` |
| Dell Dark narrow Timers (AppFrame payload `f49ae072`) | Timers & Alarms | `800` logical width; controls remain readable with no horizontal overflow or floating-control overlap | `5fa6d47f3c22bc3f75d62bfbed954508ffdf96a0b275ea301a990e02e63ae272` |
| Dell Light / Largest Timers (AppFrame payload `f49ae072`) | Timers & Alarms | native `1366x768` direct-DRM capture; large-text timer/alarm body is readable and the profile control is absent when no bottom taskbar band is reserved | `a4dd2eed3cf376d3532772c8638bb6da0565839560dd0bf1584c847e6ec16218` |
| Dell Dark desktop (current payload `285a5b35`) | VDI chooser / Desktop | native `1366x768` direct-DRM capture; current empty-state copy, wallpaper, taskbar, and shared chrome render without stale splash pixels | `a671ff1827bbfba525d3d3fd8f141c30ecd492b2970ae0692f990bf18d39ceac` |
| Dell Dark narrow (current payload `285a5b35`) | VDI chooser / Desktop | `800` logical width on native scanout; chooser copy and taskbar remain bounded, with unused right scanout intentional | `19d8cf7c12e2c562a62e5edb898145f0bc85079a5d15ddfe520bfc9d14dfcd72` |
| Dell Light / Largest (current payload `285a5b35`) | VDI chooser / Desktop | native `1366x768` direct-DRM capture; palette-resolved status card sits above the wallpaper lockup and remains readable at largest text | `1d4c327ec3e2df50d8e9ac7b5a43b3b88f69be48da0f18268883b1f71e93540b` |
| `.138` Dark desktop (current payload `703d3477`) | This Node → About | native `1920x1080` direct-DRM capture; unified MenuBar, Device Manager body, health rail, and bottom taskbar render without the retired title strip | `126629c02077edb6b7af58c93b88d0da6ca70b6a38ad1e598c120e77facde301` |
| `.138` Dark narrow (current payload `703d3477`) | This Node → About | `800` logical width on native scanout; workspace content stays bounded and the unused right scanout is intentional | `9e7a274021964e0b20242e65fa9994e9222794fe4c00abd700a377add249b1cc` |
| `.138` Light / Largest (current payload `703d3477`) | This Node → About | native `1920x1080` direct-DRM capture; Light palette and largest text remain readable with the body continuing below the scroll boundary | `1d29672b6863e46ed17162ec137d9716ab3d0d933c0a0ba964ecd6f3e7d0f760` |
| `.138` Browser boundary (current payload `703d3477`) | Browser VM connection/unavailable state | native `1920x1080` direct-DRM capture; Construct-owned browser controls and honest “No live browser page is available on this device” state render without claiming guest Chromium pixels | `43d776385fd789c343e914aecce70d37c045c198fa56f9d1d5ff351da319ae9d` |
| `.138` Explorer / Mesh lens (current payload `703d3477`) | Fleet & Mesh → Explorer | native `1920x1080` direct-DRM capture; shared workspace title/menu, mode chips, health rollup, and honest discovered-unit card remain separated above the taskbar | `f9734fa487007bfba16a899f7cb0670bb3e07e7d40a0930e1db70257b9632963` |
| `.138` Music unavailable state (current payload `703d3477`) | Music | native `1920x1080` direct-DRM capture; shared MUSIC menu/status chrome and honest missing-credentials state render; no Subsonic connectivity claim is made | `3a91b021177cb328cdcd5d854e38f1246623dd048e6421f4b5e777592a91853b` |
| `.138` Media sources state (current payload `703d3477`) | Media → Sources | native `1920x1080` direct-DRM capture; shared MEDIA menu, source tabs, capture/Jellyfin controls, and honest empty-source copy remain bounded above the taskbar | `d542a3e5c6444e7c51a860c5dc990199b24a0beaa24a33911cc242726c591a6a` |
| `.138` Files Dark desktop (current payload `703d3477`) | Files | native `1920x1080` direct-DRM capture; shared FILES menu, sidebar, list, preview pane, mesh destinations, and status strip remain separated | `a756678488efca1f5a908a787cae4ccf5002e6338e9f1d061a0fa48db008fff5` |
| `.138` Files Light / Largest (current payload `703d3477`) | Files | native `1920x1080` direct-DRM capture; large-text palette and dense file controls remain readable with the list/preview boundary intact | `1e32d9d96544b7b54b847b1dddbd9c7306431325202a8bf618205703464104ad` |
| `.138` Files Dark narrow (current payload `703d3477`) | Files | `800` logical width on native scanout; sidebar, list, preview, and bottom status remain bounded, with unused right scanout intentional | `a164abc361114a37a337deebd2de2162019c122ff56dfa8e45ffcbe1e1a2abf7` |

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
- `mde-maps-location-egui` full package suite — 267 passed, 0 failed,
  including the fixed health-rail grid and large-text tessellation coverage.
- Worklist acceptance remains open for the full workspace matrix, remaining
  Construct-owned routes/states, and final production evidence. This is clean
  representative proof, not a claim that every route is production-ready.

The latest proof attempt is intentionally absent from the passing table: the
Intel direct-DRM path returned an X-tiled BO despite the linear request, and
the fail-closed guard prevented another striped PNG from being mistaken for
pixel evidence. A detile/readback capture path and fresh Maps Light/Largest and
narrow visual inspection are still required.

## Boundary authority

The approved rendering boundaries remain documented in
[`docs/design/platform-interfaces.md`](../design/platform-interfaces.md):
focused VDI retains guest pixels, Maps retains its governed content-color
exception, and the Browser VM owns Chromium pixels and chrome. Construct owns
the connection, unavailable, reconnect, and diagnostic states around those
guests; no additional visual exceptions are introduced here.
