# WL-UX-009 direct-DRM evidence — 2026-08-01

This is representative production-validation evidence for the shared Quazar
workspace frame. It does not close WL-UX-009 or claim production readiness.

## Seat and build

- Seat: `.138` (`172.20.146.138`), physical Fedora 44 DRM seat.
- Connector: `card1-eDP-1`, connected; capture mode `1920x1080`.
- Latest installed shell payload: `8366b1094571c8ec520166e6af09e342954850bde149f14146613091e5317f4b`.
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
- Latest EGL-readback proof payload `3bd01985a670c723b87eefc5a1230344ead763d4b97c7fbb9401ee0b02ff91c8`
  was built on farm `.90` and deployed to Dell and `.138`; both services were
  active with `NRestarts=0`. The proof captured the actual live EGL back buffer
  before swap, while KMS continued to scan out the normal Intel X-tiled BO.
  The readback wrote CPU-linear RGBA PNGs plus JSON metadata and avoided the
  striped `kmsgrab` conversion path.
- Car layout candidate payload `736471b1d47f38162107915d36ff7542cd9aba27cc09ba0e49dd0b1aeb9cf46b`
  was built on farm `.90`, deployed to Dell and `.138`, and visually accepted
  for the Light/Largest narrow Auto Mode frame. Both seats remained active with
  `NRestarts=0`; `.138` was restored to `require_login_at_boot:true` after the
  proof run.
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
| `.138` Maps Dark desktop (EGL readback payload `9b2767c4`) | Maps & Location → Drive | native `1920x1080` direct-DRM EGL readback; dark HUD, health rail, alerts, and map content remain readable | `359422e1f0faa636ea506bab297993b53dc1c24ec952e8637af1407b57ade3c5` |
| `.138` Maps Light / Largest (EGL readback payload `9b2767c4`) | Maps & Location → Drive | native `1920x1080` direct-DRM EGL readback; explicit map-content text contrast remains readable over the dark HUD | `a62bc1cc49eebc8eecb995af83baefe50ac8519ab0984130123400b25bb31e4b` |
| `.138` Maps Dark narrow (EGL readback payload `9b2767c4`) | Maps & Location → Drive | `800` logical-width direct-DRM EGL readback; banner, health rail, alerts, and unused right scanout remain bounded | `b76ccbee69a043a46036db48ab4894662b98914a41926537b99ba63545c7ac50` |
| `.138` Maps Light / Largest narrow (EGL readback payload `36bf4864`) | Maps & Location → Drive | `800` logical-width direct-DRM EGL readback; responsive banner text, health rail, alerts, and unused right scanout remain readable and bounded | `7533850bb61a1aa407246b9bd8bfea83e87bfea014da19173e9f6fa76323418b` |
| `.138` Workbench Dark desktop (EGL readback payload `36bf4864`) | Fleet & Mesh → Workbench | native `1920x1080` direct-DRM EGL readback; shared title/menu, plane rail, unavailable provider state, and health cards remain readable without overlap | `b414091ce7af1b5d53cd4bbc658ec9397f184d21f2b64110d7e07a8191eccbf0` |
| `.138` Workloads Dark desktop (EGL readback payload `36bf4864`) | Infra as Code → Provision | native `1920x1080` direct-DRM EGL readback; shared WORKLOADS menu, lifecycle rail, placement card, and honest plan-only state remain bounded | `fccab7b0118539d285a4c6843b4ce477173a0dfc1caa03d297b90a4f2d1e6599` |
| `.138` Media Sources Dark desktop (EGL readback payload `36bf4864`) | Media → Sources | native `1920x1080` direct-DRM EGL readback; shared MEDIA menu, source tabs, local-capture/Jellyfin controls, and honest empty-source copy remain bounded | `ba301951c1c06de835fd50318f81df5a0b0feeb9b17407971eb79cfc1ae4d04b` |
| `.138` Browser boundary Dark desktop (EGL readback payload `36bf4864`) | Browser VM connection/guest boundary | native `1920x1080` direct-DRM EGL readback; Construct browser controls remain distinct from the blank guest viewport, with no Chromium content-readiness claim | `34e36824228b4848126bbe5ced9d6b7c808a2ccb58f190e1a3dfaea19cbecda0` |
| `.138` Mesh Teams Dark desktop (EGL readback payload `9cdc8f1f`) | Communications → Mesh Teams | native `1920x1080` direct-DRM EGL readback after the proof-only settle window; shared MESH TEAMS chrome, activity feed, details rail, and honest unconfigured bridge state remain readable and separated | `46e5d3fe975f7e6b4f2d89094ed5257b26bd4a8403cd75d67b3ab815f15edf21` |
| `.138` Music unavailable Dark desktop (EGL readback payload `9cdc8f1f`) | Music | native `1920x1080` direct-DRM EGL readback after the proof-only settle window; shared MUSIC menu/status chrome and honest missing-credentials state render without claiming Subsonic connectivity | `5b77cfe095d4f0f61f9b9ac9d36403fc517fffe31377707e8657f45a170f0259` |
| `.138` Terminal Dark desktop (EGL readback payload `36bf4864`) | Terminal | native `1920x1080` direct-DRM EGL readback; Terminal menu, tab strip, shell prompt, and taskbar remain separated in the honest idle state | `9674a818f45c5b97f18f6fd71d1788d444f60520baf63b84726092b66a67d35f` |
| `.138` This Node Dark desktop (EGL readback payload `36bf4864`) | This Node | native `1920x1080` direct-DRM EGL readback; searchable node center, section tabs, system menu, and reading-seat state remain bounded without overlap | `aae809d959f81f9049385c4840ee6dd6fedb4cbaa8c2eceb8a9ae72a90713d5e` |
| `.138` Mesh Teams Light / Largest narrow (EGL readback payload `9cdc8f1f`) | Communications → Mesh Teams | `800` logical-width direct-DRM EGL readback after the proof-only settle window; large-text activity feed, alert rows, taskbar, and intentional unused scanout remain readable and bounded | `08be7658a25320fd4c80cddbe22f1d116f3a0691173d8829c48b92fb3ddf1bd1` |
| `.138` Music unavailable Light / Largest narrow (EGL readback payload `9cdc8f1f`) | Music | `800` logical-width direct-DRM EGL readback after the proof-only settle window; large-text menu/status chrome and honest missing-credentials copy remain readable without claiming Subsonic connectivity | `62dd2bd45818213e4a01983d4be7652e4f743aa4943828f571bb92a129d0768b` |
| `.138` Terminal Light / Largest narrow (EGL readback payload `9cdc8f1f`) | Terminal | `800` logical-width direct-DRM EGL readback; large-text menu, tab strip, shell prompt, and honest idle/loading body remain bounded | `5e9d390c30b981e7a32dfda82276b3c38fd0ce58285df208a97f8e7fac27e264` |
| `.138` This Node Light / Largest narrow (EGL readback payload `9cdc8f1f`) | This Node | `800` logical-width direct-DRM EGL readback; large-text unified node navigation, section sidebar, and honest “Reading the seat…” state remain readable and bounded | `77f336a7e8978cf5c866ca2ff8d95efc7259b8d81745e810801f3848f800c1ac` |
| `.138` Workbench Light / Largest narrow (EGL readback payload `333503d0`) | Fleet & Mesh → Workbench | `800` logical-width direct-DRM EGL readback after removing the duplicate wrapper View control; the shared STATE OF THE MESH bar, plane rail, unavailable health state, and body remain readable and bounded | `a54de1af67c09467766e376b6e279d0f34702cb959e585432a65f8409933847d` |
| `.138` Workloads Light / Largest narrow (EGL readback payload `333503d0`) | Infra as Code → Provision | `800` logical-width direct-DRM EGL readback; large-text WORKLOADS menu, lifecycle rail, plan-only placement state, and controls remain readable and bounded | `699e569374991820f96357d8879a067a2649d3ec60cfbb797ae22859213d8542` |
| `.138` Media Sources Light / Largest narrow (EGL readback payload `333503d0`) | Media → Sources | `800` logical-width direct-DRM EGL readback; large-text MEDIA menu, source tabs, local/Jellyfin controls, and honest empty-source state remain readable and bounded | `f344202126cdbf266d74b96aa06808e2e5ca68ac4d0441ad2f1a98b934181f37` |
| `.138` Browser boundary Light / Largest narrow (EGL readback payload `333503d0`) | Browser VM connection/guest boundary | `800` logical-width direct-DRM EGL readback; Construct browser controls remain distinct from the blank guest viewport, with no Chromium content-readiness claim | `b69f25c3e699c799e70234a502f92938996762152337cc4beb9c20e2fb0d2937` |
| `.138` Phones Light / Largest narrow (EGL readback payload `3bd01985`) | Phones | `800` logical-width direct-DRM EGL readback after enabling the shared leading AppFrame title; paired/online status, hub tabs, feature card, and remote-input controls remain readable and bounded | `2d96bd5592e9f3325471a469b12bb2dbd031af91982cd7e7b6875c6af0fc4dd0` |
| `.138` Editor Light / Largest narrow (EGL readback payload `3bd01985`) | Mesh Teams → Editor | `800` logical-width direct-DRM EGL readback; nested communications/editor chrome, collapsed optional sidebars, document/project controls, formatting rows, empty document state, and status row remain readable and bounded | `b68b8335f07d5017c949389e15b166cf1b669dd7a8988041f20277913b57d106` |
| `.138` Car Light / Largest narrow (EGL readback payload `736471b1`) | Auto Mode → Car Home | `800` logical-width direct-DRM EGL readback after the Car route alias, zoom-aware instrument strip, width-safe elision, and three-column large-text grid fixes; Auto Mode title/cards/app strip and all 12 selected status tiles remain readable and bounded above the taskbar | `88e022c9427345ee94adb0e164553ba655a8e6ba2c85f501be2407db17d59e5e` |
| `.138` Editor Dark desktop (EGL readback payload `736471b1`) | Mesh Teams → Editor | native `1920x1080` direct-DRM EGL readback; nested communications/editor chrome, document/project controls, formatting rows, empty document state, status row, and details rail remain readable and bounded | `10bda5359c80b0f5148c518a6d198222a4abc75e07dd20ad3c92aa81cbc1bb98` |
| `.138` Terminal Dark desktop (EGL readback payload `736471b1`) | Terminal | native `1920x1080` direct-DRM EGL readback; shared TERMINAL menu, tab strip, mesh overview, shell prompt, and taskbar remain separated and readable | `a0f7998b2d23584bd1f5be0e90f7705ca23b9d9079270c9d528d7ed3bba84448` |
| `.138` Car Dark desktop (EGL readback payload `736471b1`) | Auto Mode → Car Home | native `1920x1080` direct-DRM EGL readback; Auto Mode title, navigation/media/vehicle cards, app strip, and taskbar remain separated and readable | `eab009624b2878b6133ecbd8935adfc2abccdedcc52841fa38e26ce30501e8b6` |
| `.138` Phones Dark desktop (EGL readback payload `8366b109`) | Phones | native `1920x1080` direct-DRM EGL readback after bounding the shared status/identity rows to the AppFrame inset; title, tabs, feature/remote-input cards, and empty state remain readable and bounded | `87f5b3e9a7c3a234b66a513e04b4b875dc87612e53733f214566352ca388799d` |
| `.138` Editor Light desktop (EGL readback payload `8366b109`) | Mesh Teams → Editor | native `1920x1080` direct-DRM EGL readback after the proof-only unlock; Light Mesh Teams/editor chrome, document body, details rail, and taskbar remain readable and bounded | `bc48241b3cd0fbed2a9b4f191527cd88437060307d5c83b8f4b1bb2f02fd69c5` |

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

The earlier Intel direct-DRM linear-request and fixed-height-banner attempts
remain historical failed evidence. The proof-only EGL readback path now emits
CPU-linear RGBA output, and the responsive banner fix was visually inspected
on `.138` at the `800` logical-width, Light/Largest profile. The accepted row
above closes this Maps matrix cell; the broader WL-UX-009 workspace matrix and
remaining Construct-owned routes remain open.

The first current-payload batch rendered Music and Mesh Teams before their
asynchronous/expressive body state had settled; those frames are excluded.
The proof-only settle window was then exercised on the new `9cdc8f1f` payload.
The resulting frames show the honest Music missing-credentials state and the
settled Mesh Teams activity view, and are the rows accepted above. The settle
window is opt-in and does not alter production timing.

## Boundary authority

The approved rendering boundaries remain documented in
[`docs/design/platform-interfaces.md`](../design/platform-interfaces.md):
focused VDI retains guest pixels, Maps retains its governed content-color
exception, and the Browser VM owns Chromium pixels and chrome. Construct owns
the connection, unavailable, reconnect, and diagnostic states around those
guests; no additional visual exceptions are introduced here.
