# WL-UX-009 direct-DRM evidence — 2026-08-01

This is representative production-validation evidence for the shared Quazar
workspace frame. It does not close WL-UX-009 or claim production readiness.

## Current adoption audit — 2026-08-01

- `.138` (`172.20.146.138`) is reachable and currently reports
  `mde-shell-egui.service=active`, `NRestarts=0`, with installed payload
  `c2e6f20a01042ed854861a5457ff03caa19f165070f2f9ce71e0172b8c42d7a4`.
  This current tree includes the This Node, Explorer, Bookmarks, and Mesh
  Teams shared-frame migrations, plus the Files node-action surface. The binary
  hash is verified and a fresh EGL readback from this exact payload was
  inspected below; the complete desktop/narrow/large-text matrix remains
  open.
- Dell `.225` (`172.20.146.225`) currently has no route and no reachable SSH
  port. Dell rows later in this ledger are historical accepted captures for the
  payload named in each row; they are not evidence that Dell currently runs the
  latest `.138` payload.
- This distinction preserves the evidence boundary: historical Dell visual
  proof is retained, current synchronized Dell adoption is open, and no
  production-readiness claim follows from either one alone.

### Exact current-payload follow-up — 2026-08-02

After the Maps empty-state correction, the exact `.138` payload
`83fe8f3c4fdcbd96f2acf31dce92ea2e85d6c65f24bde7eafb73365b88d17b44` was
recaptured through the proof-only CPU-linear EGL readback. The following
Dark desktop/narrow and Light/Largest frames were visually inspected as
bounded, readable Construct-owned states. Narrow captures intentionally leave
unused right scanout; Browser rows show only the Construct-owned VM boundary
and make no guest Chromium readiness claim.

| Route/profile | PNG SHA-256 |
| --- | --- |
| Files Dark desktop | `72c5cb5833aac954547729563d4d4b65309bea2f42c8b71a7476cc272e74ff26` |
| Files Dark narrow | `4431a2f12be0b79b0176466dafbc33ef5fbdcff251153eca21f096535a4c3570` |
| Files Light/Largest | `d31e7b8c786b5aa1d28fc045518e6d6d6a3f13e68d9ec000da84b61da7da0cde` |
| Mesh Teams Dark desktop | `9fc4141ae82b1497c6ba504c7b2d58a1a3b27d336a5f25ec407320f55ac2ac2a` |
| Mesh Teams Dark narrow | `e9a3af4ea0e202cebd30e544dda01f641dc86eaade44ef2691077c45eeea053d` |
| Mesh Teams Light/Largest | `510ade1426b8cb1f861b8b65c36180365d0a66357707cbfa2fd2c986d46ad745` |
| Editor Dark desktop | `d1b49bd40cf10d4db11af7c26f616cfa8ff16920631ce3fba909264ed09aeb08` |
| Editor Dark narrow | `7c2f206d21c0f7149b27cbc1af72558cf62b7ccd74ae27f2ff057adb51f3f19f` |
| Editor Light/Largest | `5851b018261a7f5a08daf4ea9d8c7ebb7c124c73a1fbc7163a1ba4a84f7b9a0e` |
| Phones Dark narrow | `007818f12a3eaf81f0ee37b1e68dfc6300078694b9d521d97f300c095332b329` |
| Phones Light/Largest | `83f3f698e25e513b2cca5070d1a60ba91421c1934f80729880cb2271ead36742` |
| Music Dark narrow | `b0357ec9126e6ff8d8aba80850bcc420a39024ebfc8f0d8551d11f8f32ca01be` |
| Music Light/Largest | `0d74cbcf126b9a56d6ba03cf2f4eece52ea1ed2773a9aafcb4db0a3b16b25506` |
| Media Dark desktop | `a7326cc7572eba925b61f3af8609c5bd993ca70018b337c042f2d43f34cd6414` |
| Media Dark narrow | `90b68b7416b8c795c5e7a5a65026458bdbea60176fbeb6f15e2fa2edf9e201c3` |
| Media Light/Largest | `1518a2a45456362a79340e367e45a5d2fad5c674a3f6281f2f2cf26f3877d34f` |
| Terminal Dark desktop | `42f96d166c60bda69390dd1e193097870f9dec12d798c62a7c5aeef08fbd1ddc` |
| Terminal Light/Largest | `99564b5429e4bb77f870906e24777b31a2b55d8dcca5e2e08ecb02beb87fb41c` |
| This Node Dark desktop | `51840383503cef58062a3adf8014e9cf20a9762484924d4186771b95881c1d83` |
| This Node Light/Largest | `1a30843e1d1d275acbbb0e4fa72b28de851bf74f6adafa076b4238e3cbd7cb80` |
| Browser boundary Dark desktop | `80235ad4f39ff1dee54cb62bfef6e4ce20ac35f4df40c4839aea6366fbbe2c42` |
| Browser boundary Light/Largest | `4c8d403bfb6d13671a0addf68992190372ffb43d9a2e4719a190af09dbf15a64` |
| Phones Dark narrow | `b6aa228dc739417eb4e7f54d059e0e9bbab0849463b470bb69ec89e220fa86df` |
| Music Dark narrow | `b3dceb559cfcc651970216b9aada542a2160fcbede56fb5f3b4537cad1df0924` |
| Terminal Dark narrow | `8ce5621c527190eb95ce5541aaa5128767bfa9f205f6ed3393c053a66e14110d` |
| This Node Dark narrow | `232cd16e87c9a4b1a6654c5479ddde13770d66f455ac8444aca02fdc66fe4b7d` |
| Browser boundary Dark narrow | `ce351be4ad75222522d96ddb5c256c338d7d68d9ac632976f397c4aef13603e6` |

### Bookmarks AppFrame adoption — 2026-08-02

The Bookmarks title-only menu wrapper was replaced with the shared
`AppFrame::new("Bookmarks").leading_title()` while preserving the domain
search/sort/location strip below it. BigBoy passed the Bookmarks suite (41/41)
and the serial shell suite (1,351/1,351). The exact release payload
`a4a1a21a0c5fe820b4c3de23a1ae403cbe201628580074df3b133c34cf2fbc18` was
installed on `.138`; the service remained active with `NRestarts=0` after
capture and secure-state restoration.

| Route/profile | PNG SHA-256 |
| --- | --- |
| Bookmarks Dark desktop | `d5ad5896b372e4fc0b52a0296f54c59790146baa13f88e759a5a7f6f2a4ac3b0` |
| Bookmarks Dark narrow | `3cf3276c22584cce235e2c0f30b951be53096192bc01f0f4003376c616d98ead` |
| Bookmarks Light/Largest | `19c2891dff5d1c393ccdedd0f39ae642d925f95f31b4fdafbe3f3518e7385a63` |

All three cells were visually inspected as readable, bounded Construct-owned
workspace states. This closes the Bookmarks frame-adoption slice only; Dell
synchronized adoption, strict linear scanout, the remaining route/profile
matrix, and WL-UX-009 readiness remain open.

### Files node-action responsive validation — 2026-08-02

The Files sidebar now puts its ten node-aware interactions immediately after
node identity at every profile, reserves a dedicated action column, and keeps
the Airsonic/Music ownership state bounded as `Music-owned`. The exact BigBoy
release payload `c2e6f20a01042ed854861a5457ff03caa19f165070f2f9ce71e0172b8c42d7a4`
was installed on `.138`; the service remained active with `NRestarts=0` and
was restored to `require_login_at_boot:true` with Dark/Default appearance after
capture. The focused Files suite passed 166/166 on the farm.

| Route/profile | PNG SHA-256 | Evidence |
| --- | --- | --- |
| Files Dark desktop | `8fbfc68c599cc0acda4a0452509ed9f934b8f0846286de79afa867468cb291cb` | [PNG](evidence/WL-UX-009-2026-08-02-138-files-dark-desktop-c2e6f20a.png) |
| Files Dark narrow | `bcd35b394ef6db13a32444eb3121018879af0a4f0a77ba8bf6a11e2285407bcb` | [PNG](evidence/WL-UX-009-2026-08-02-138-files-dark-narrow-c2e6f20a.png) |
| Files Light/Largest | `6ae985cf982a60f28e879ca3edbe26361cf2ef4abb82d4708649dc593640b403` | [PNG](evidence/WL-UX-009-2026-08-02-138-files-light-largest-c2e6f20a.png) |

All three cells were visually inspected. Dark/narrow and Light/Largest keep
Browse, Share, Copy, Send, Receive, Sync, Conflicts, Availability, Airsonic,
and Activity visible before secondary sidebar content; the Light/Largest frame
also shows a separated Upload action and fully readable `Music-owned` state.
This closes the Files responsive node-action/current-payload slice only. Dell
`.225` remains unreachable, strict linear scanout remains unavailable on the
Intel X-tiled path, the remaining route/profile matrix is open, and this does
not claim WL-UX-009 readiness.

The seat was restored to `require_login_at_boot:true`, Dark/Default appearance,
`mde-shell-egui.service=active`, and `NRestarts=0`. This updates the listed
cells only; Dell synchronized adoption, strict linear scanout, the remaining
route/profile matrix, and WL-UX-009 readiness remain open.

### Files node-action current-payload recapture — 2026-08-02

The current Files node-action release `de4c6bbeebf9768c64c837af45f6cdde48774f8f8a4d9ebcb3914ce1a59e28ea`
was installed on `.138`; the service stayed active with `NRestarts=0`. The
focused Files suite passed **167/167**. The first capture attempt was rejected
because the proof route remained on Media; the frames below use the explicit
`files` route and were visually inspected after recapture.

| Route/profile | PNG SHA-256 | Evidence |
| --- | --- | --- |
| Files Dark desktop | `df87c8d061e8c49f9b39dea2b434111c402bcf455e292c54be2c872512a73670` | [PNG](evidence/WL-UX-009-2026-08-02-138-files-dark-desktop-de4c6bbe.png) |
| Files Dark narrow (`MDE_DRM_PROOF_LOGICAL_WIDTH=800`) | `c9d03a038e8359d95067571f5e5c33627b939552d5f1c8c12214dd3cb8b3791a7` | [PNG](evidence/WL-UX-009-2026-08-02-138-files-dark-narrow-de4c6bbe.png) |
| Files Light/Largest | `b0f3d646eacf0e41874b9fb618739b2a29079d86b98ed4888a3063baf1f3d0a7` | [PNG](evidence/WL-UX-009-2026-08-02-138-files-light-largest-de4c6bbe.png) |

All three cells show the ten-item inventory with a separate action lane; the
narrow frame keeps the inventory ahead of Places and the Light/Largest frame
keeps Airsonic ownership text and Upload readable. Secure state was restored:
`require_login_at_boot:true`, Dark/Default appearance, service active, and zero
restarts. This closes the Files node-action current-payload slice only; Dell
`.225`, strict linear scanout, the remaining route/profile matrix, and overall
WL-UX-009 readiness remain open.

### This Node current-payload validation — 2026-08-02

The unified `this-node` route was captured on exact release
`de4c6bbeebf9768c64c837af45f6cdde48774f8f8a4d9ebcb3914ce1a59e28ea` on `.138`.
The frames show the shared This Node frame, provider-aware settings rail,
health rail (`97% charging` and display count), and live display/device state
without claiming unsupported hardware actions.

| Route/profile | PNG SHA-256 | Evidence |
| --- | --- | --- |
| This Node → System Dark desktop | `e5e493bc0155c99dbea783ca9d53adb0062871c4d1738ffa9d1fdc6917d3ef42` | [PNG](evidence/WL-UX-009-2026-08-02-138-this-node-dark-desktop-de4c6bbe.png) |
| This Node → System Dark narrow (`MDE_DRM_PROOF_LOGICAL_WIDTH=800`) | `79acef76ef87d6eca7a6d63db908212e124651a0fa83476f2857473cc3edbbd8` | [PNG](evidence/WL-UX-009-2026-08-02-138-this-node-dark-narrow-de4c6bbe.png) |
| This Node → System Light/Largest | `31b27128b056b43380a0b7a0e5a0c66a982844c00d463886c9c4f47f07b6daca` | [PNG](evidence/WL-UX-009-2026-08-02-138-this-node-light-largest-de4c6bbe.png) |

All three cells were visually inspected as readable and bounded. The
Light/Largest body continues through the central scroll region above the
taskbar; the health rail remains readable and associated with the selected
settings workspace. Secure state was restored to `require_login_at_boot:true`,
Dark/Default appearance, active service, and zero restarts. This closes the
This Node current-payload validation slice only; `.225`, strict linear scanout,
the remaining route/profile matrix, and WL-UX-009 readiness remain open.

### Music current-payload validation — 2026-08-02

The explicit `music` route was captured on exact release
`de4c6bbeebf9768c64c837af45f6cdde48774f8f8a4d9ebcb3914ce1a59e28ea` on `.138`.
The surface presents the shared Music frame and a truthful unavailable state:
`Not connected`, `No music server connected`, and missing Airsonic credentials.
This does not claim connectivity to Subsonic server `172.20.0.2`.

| Route/profile | PNG SHA-256 | Evidence |
| --- | --- | --- |
| Music Dark desktop | `b7171515085af149359cc6b1318ff0e4aeffa4c8953bde3a1be72c4b382b786b` | [PNG](evidence/WL-UX-009-2026-08-02-138-music-dark-desktop-de4c6bbe.png) |
| Music Dark narrow (`MDE_DRM_PROOF_LOGICAL_WIDTH=800`) | `c410f2e1cc843ed3bbeecfe891eacd079abaa32858424fc92137efba65e78f8a` | [PNG](evidence/WL-UX-009-2026-08-02-138-music-dark-narrow-de4c6bbe.png) |
| Music Light/Largest | `2456cde2e51ccff6b8052390fa4744fe60c2a7b370d66c041bb934ea4549970b` | [PNG](evidence/WL-UX-009-2026-08-02-138-music-light-largest-de4c6bbe.png) |

All three cells were visually inspected as readable and bounded with no guest
or fabricated server content. Secure state was restored to
`require_login_at_boot:true`, Dark/Default appearance, active service, and zero
restarts. This closes the Music current-payload validation slice only; live
Airsonic/Subsonic connectivity, `.225`, strict linear scanout, the remaining
route/profile matrix, and WL-UX-009 readiness remain open.

### Media current-payload validation — 2026-08-02

The shared menubar correction was built on BigBoy and installed as exact release
`c75e01456f56902d92d36a4ebe8b9f68822e74c00670cb82b090b3bd05909dac` on `.138`.
The fix reserves the measured workspace title and separates the narrow command
row, so `MEDIA` remains complete at 800 logical pixels. The source surface was
visually inspected in its honest no-local-source/no-Jellyfin state; no VDI or
guest readiness is inferred.

| Route/profile | PNG SHA-256 | Evidence |
| --- | --- | --- |
| Media Dark desktop | `4eaa642cf395c0b23dd1472359099418cdf0e37aa2aba4aa586fa70dfc46d000` | [PNG](evidence/WL-UX-009-2026-08-02-138-media-dark-desktop-c75e0145.png) |
| Media Dark narrow (`MDE_DRM_PROOF_LOGICAL_WIDTH=800`) | `ecbf3349b2dafaab4c1f7cfdd99c89eeedd24d2fa042ac0aeb231ce76f79209f` | [PNG](evidence/WL-UX-009-2026-08-02-138-media-dark-narrow-c75e0145.png) |
| Media Light/Largest | `d17f32dd3a0870eac4ae76f7e89c43894634deb8ce5949e4d8ee3fa74ff18c9d` | [PNG](evidence/WL-UX-009-2026-08-02-138-media-light-largest-c75e0145.png) |

All three cells were visually inspected as complete and bounded, with the full
title, menu controls, source controls, status chips, and taskbar readable. Dell
was restored to `require_login_at_boot:true`, Dark/Default appearance,
`mde-shell-egui.service=active`, and `NRestarts=0`. This closes the Media
current-payload validation slice only; `.225`, strict linear scanout, the
remaining route/profile matrix, and WL-UX-009 readiness remain open.

### Editor current-payload validation — 2026-08-02

The Editor compact-toolbar correction was built on BigBoy and installed as exact
release `807f2430d61bbd8316410c6360d8beaba2de55dba643933f641ac4d59ab5ecd7` on
`.138`. At 800 logical pixels the width-heavy Zoom and paragraph-style controls
now use the existing `»` overflow affordances; no toolbar command is painted
outside the visible editor frame. Direct Editor entry keeps both optional
sidebars collapsed. The surrounding Mesh Teams frame is host-owned collaboration
chrome; this proof does not claim guest Chromium or focused-VDI pixels.

| Route/profile | PNG SHA-256 | Evidence |
| --- | --- | --- |
| Editor Dark desktop | `3c7cbc7050e1f51b92438c7bae0d24adb6742a065282f6cb0985ce258319de18` | [PNG](evidence/WL-UX-009-2026-08-02-138-editor-dark-desktop-807f2430.png) |
| Editor Light desktop | `e2f7622bbe950f54268deb88de223f448d3f7c064972db15618f3a3ffafe74c7` | [PNG](evidence/WL-UX-009-2026-08-02-138-editor-light-desktop-807f2430.png) |
| Editor Dark narrow (`MDE_DRM_PROOF_LOGICAL_WIDTH=800`) | `b08bd275e7a21f0df1a8f48e7776ea26e2a8e0254c271bc3b652212f7f23694c` | [PNG](evidence/WL-UX-009-2026-08-02-138-editor-dark-narrow-807f2430.png) |
| Editor Light/Largest | `b66c139be58685205edd49d13feaac18364fdf488b8f7cafe0e815f947800400` | [PNG](evidence/WL-UX-009-2026-08-02-138-editor-light-largest-807f2430.png) |

All four cells were visually inspected as readable and bounded. The rejected
pre-correction narrow frame is intentionally not included in the passing table.
Secure state was restored to `require_login_at_boot:true`, Dark/Default
appearance, active service, and zero restarts. This closes the Editor
current-payload validation slice only; `.225`, strict linear scanout, the
remaining route/profile matrix, and WL-UX-009 readiness remain open.

### Files node-action validation — 2026-08-02

The Files node-action slice was built on BigBoy and installed as exact release
`5ea5b72594482d8e4158e136b09d1c44407c3a68c85d8c97a17e3f7bd3794e28` on `.138`.
The sidebar exposes ten typed node interactions: browse shares, share targets,
copy links, send, inbox, sync status, conflicts, availability, Airsonic upload,
and node activity. The roster and destinations remain provider-backed; the
Airsonic target is visibly the shared Music-owned destination when no concrete
server advertisement is present. The narrow title reserve keeps the complete
`FILES` identity visible at the 800-logical-pixel proof width.

| Route/profile | PNG SHA-256 | Evidence |
| --- | --- | --- |
| Files Dark desktop | `cba14cdbd0b969a6431346b66fccc67e055ff21245e6039f2068e76a60f0ed5e` | [PNG](evidence/WL-UX-009-2026-08-02-138-files-dark-desktop-5ea5b725.png) |
| Files Dark narrow (`MDE_DRM_PROOF_LOGICAL_WIDTH=800`) | `cba14cdbd0b969a6431346b66fccc67e055ff21245e6039f2068e76a60f0ed5e` | [PNG](evidence/WL-UX-009-2026-08-02-138-files-dark-narrow-5ea5b725.png) |
| Files Light desktop | `7f8ee0ad1e3d726428c07c1aec30aa31537c20fae889fcf63bab2e446392e883` | [PNG](evidence/WL-UX-009-2026-08-02-138-files-light-desktop-5ea5b725.png) |
| Files Light/Largest | `5460e0ba20f76db2481c0e1aa2812fc8141290a92baeb089046e9cb805a2f570` | [PNG](evidence/WL-UX-009-2026-08-02-138-files-light-largest-5ea5b725.png) |

All four frames were directly captured from the DRM seat and visually inspected
for complete title identity, readable node-action labels, truthful provider
states, and no action-lane overlap. Secure state was restored to
`require_login_at_boot:true`, Dark/Default appearance, active service, and zero
restarts. This closes the Files node-action validation slice only; provider
advertisement coverage, `.225`, strict linear scanout, the remaining route/profile
matrix, and WL-UX-009 readiness remain open.

### Editor and Terminal current-payload validation — 2026-08-02

The shared title allocation correction was built on BigBoy and installed as
exact release `911e781820822cb0be5bb18251147bf34c33f24047c936c5183222ed1e7081c8`
on `.138`. It preserves complete workspace identity at the 800-logical-pixel
narrow profile, including the previously clipped `TERMINAL` title. Editor
direct entry keeps its optional sidebars collapsed; its large-text toolbar uses
bounded rows and retains reachable overflow controls. Terminal shows its
Terminal-pattern top bar and truthful live node/session state.

| Route/profile | PNG SHA-256 | Evidence |
| --- | --- | --- |
| Terminal Dark desktop | `b975d5fd95c73c422a5146e3aeaa1e088a16154e33893d148e1412416716ac55` | [PNG](evidence/WL-UX-009-2026-08-02-138-terminal-dark-desktop-911e7818.png) |
| Terminal Dark narrow (`MDE_DRM_PROOF_LOGICAL_WIDTH=800`) | `18164633007a8d73ea8ba8b60616915a16f8efb5339bd6bffcb46f7a2dc64102` | [PNG](evidence/WL-UX-009-2026-08-02-138-terminal-dark-narrow-911e7818.png) |
| Terminal Light desktop | `7639d3561deea149dd9c652f6e4b8134d3ab8017e6b9fe4842c36364ee5df9e8` | [PNG](evidence/WL-UX-009-2026-08-02-138-terminal-light-desktop-911e7818.png) |
| Terminal Light/Largest | `db5d6a83c8fb61e68087c67e8575966d2af895b1445fb5358263c9eea5690c5b` | [PNG](evidence/WL-UX-009-2026-08-02-138-terminal-light-largest-911e7818.png) |
| Editor Dark desktop | `4bf2783adb9dddbf1fecc62730cf40ba72ae3225e9f96995aa2ab97dfc2afcf1` | [PNG](evidence/WL-UX-009-2026-08-02-138-editor-dark-desktop-911e7818.png) |
| Editor Dark narrow (`MDE_DRM_PROOF_LOGICAL_WIDTH=800`) | `c86de190e07468cce94b6dd19caeae5a53f03a41ef06e694e3a97c19dac0f885` | [PNG](evidence/WL-UX-009-2026-08-02-138-editor-dark-narrow-911e7818.png) |
| Editor Light desktop | `5cfe0537bb5e1e8d46906047d1c411a2407b1ae39049249b7ed758d297e832bf` | [PNG](evidence/WL-UX-009-2026-08-02-138-editor-light-desktop-911e7818.png) |
| Editor Light/Largest | `ca38536911964bb29f028a450d7b924ec9d596b5f6a8ab9c370d1847ee22194e` | [PNG](evidence/WL-UX-009-2026-08-02-138-editor-light-largest-911e7818.png) |

All eight frames were directly captured and visually inspected for complete
titles, readable controls, no clipping/overlap, and truthful unavailable/live
states. The secure seat state was restored with `mde-shell-egui.service=active`
and `NRestarts=0`. These close the Editor and Terminal current-payload slices
only; `.225`, strict linear scanout, guest VDI readiness, remaining route/profile
coverage, and WL-UX-009 readiness remain open.

## Seat and build

- Seat: `.138` (`172.20.146.138`), physical Fedora 44 DRM seat.
- Connector: `card1-eDP-1`, connected; capture mode `1920x1080`.
- Prior validation payload: `e37ec5c0df9ac05133c073f481823b5642a9fef85dd0237502f82abb2a9d13dd`.
- Latest installed shell payload (2026-08-02 Editor narrow-toolbar validation):
  `807f2430d61bbd8316410c6360d8beaba2de55dba643933f641ac4d59ab5ecd7`.
- Current Bookmarks proof build: BigBoy farm `172.20.0.130`, slot
  `wl-ux-009-bookmarks-release-20260802`, release features
  `drm,live-vdi,media-mpv`; pulled artifact matches the installed hash.
- Current adoption build: BigBoy farm `172.20.0.130`, slot
  `wl-ux-009-proof-route-20260802`, release features
  `drm,live-vdi,media-mpv`; pulled artifact matches the installed hash.
- `.138` Explorer-fix proof binary: `f58b42ba817c774824efd43e830451d0e189b218deb43816c45feadcbb4ace1a`;
  built on BigBoy with `drm,live-vdi,media-mpv`, installed only on `.138` while
  Dell `.225` was unreachable. No synchronized-farm deployment is claimed.
- Farm RPM: `magic-mesh-12.1.6-1.x86_64.rpm`, SHA-256
  `53835242591b1d3217fc2d83f14021f9cba41fb68a86c295d7c7373ffbee4d75`.
- Build: BigBoy farm `172.20.0.130`, slot 11, release shell with
  `drm,live-vdi,media-mpv`.
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

### Current-payload readback attempt — 2026-08-01

The prior current `.138` payload (`cbbd6465…`) produced a CPU-linear EGL
readback at `1920x1080`; PNG SHA-256 is
`7ab6003dc6b987e354e8ddf0f946471a84c81cd7a04db7e4d31a4459e8b4c6f1`.
Visual inspection confirms the live dark desktop/taskbar frame and the service
was restored to `active` with `NRestarts=0` after capture. This is one accepted
current-payload desktop observation, not a complete route/profile proof matrix.

The stricter linear-scanout proof remains fail-closed on this Intel seat:
linear allocation returns `Invalid argument`, the front buffer is
`DrmFourcc(XR30)` with modifier `I915_x_tiled`, and the proof rejects that
non-linear buffer. No screenshot from that invalid scanout path is accepted.

### Current-payload route batch — 2026-08-02

The exact installed payload `e37ec5c0…` was recaptured on `.138` using explicit
proof routing and the CPU-linear EGL readback. The following frames were
visually inspected and accepted:

| Route/profile | PNG SHA-256 |
| --- | --- |
| Files Dark desktop | `e54a3994bb70035c72ad0491a232847621aefdbdefb28ddd300967a2a0d06956` |
| Files Dark narrow | `0b69afce6daac5f7d0291e4a439116bdb4d73af82cef7afd801e23d8fd75cd82` |
| Files Light/Largest | `bd212292763ad11fbcb3408775c094d6f7a6ceba7f88572d71d2706c08862472` |
| Mesh Teams Activity Dark desktop | `b5c7e970d596fd8a2664d55026bab131f1eba411a82742f60b878ee32fd52d0a` |
| Mesh Teams Activity Dark narrow | `40e1f29da1dbd1237966257937358fb6aff2cc146848d69977931b50ef4505ea` |
| Mesh Teams Editor Dark desktop | `ec619a4489e126851522bfdf0667a831cdedeb9c87d26374206c0df49e54abb7` |
| Mesh Teams Editor Dark narrow | `08c8b40e1c4b856277f25dad0d663e40619012d1488a681c8034d8533ff32352` |
| Phones Dark desktop | `397ff2b05073ebd388b6fab2ef1c2fc6ff4bdf54a6c030705f68bbc9e2c1fdb5` |
| Music unavailable Dark desktop | `4e3d27909cb76c566f62710873a4e60b4dac3548fbaac7122ced983ba420110d` |
| Music unavailable Light/Largest | `ecaad635831e61f77e4e1dd7d4b70e250344749bc1cae513cb3f5baededdd8ae` |
| Media Sources Light/Largest | `0b01b4265b0c07717329209ebaa3b3e2810ca131f02d1ed213bee0ccac9f149a` |
| Terminal Dark narrow | `154f2a963691c689e8c50d2eeac00bb0212f16cdc564979324a80f2b0c8437cb` |
| Terminal Light/Largest | `afa281ee38fe4e897426d56d106563ba50186ff1590549dda8781ee2a78ffadf` |
| This Node/System Dark narrow | `edd0aac64298998987ab09855248162f93ea521865fb009fed05ba90bdea5c27` |
| This Node/System Light/Largest | `c3ce49daff9cafda7eddb3d7048170456e0034a699c277f607c91719aa08e11a` |
| Maps Dark desktop | `c8a5bf09f807b302c23cfc20590ec0f8a9f22c757c383afb07aecfbfd9a0b07f` |
| Maps Dark narrow | `80b729a523a8ca6e5418639298895d601b8627bb605efaff8e954630730fe87f` |
| Browser boundary Dark desktop | `0025e3d4ea9eb5d62dc54b5bbf873898e033871e685e01c6f4230da079936ddc` |
| Browser boundary Dark narrow | `d55938496b477ee9265e14b1f419c6e57d7f9a2915c9b3be449b97bc3971225e` |

The current Maps Light/Largest frame is retained as a rejected diagnostic: the
governed dark map canvas makes the central empty-state copy too faint beneath
the Light shell palette. Several other cells were not accepted because the
seat hit systemd's restart-rate limit during the batch and produced no frame.

### Maps empty-state contrast correction — 2026-08-02

The corrected DRM shell payload was built on BigBoy slot
`wl-ux-009-maps-light-empty-state-release-20260802` with release features
`drm,live-vdi,media-mpv`, pulled and installed on `.138` as
`83fe8f3c4fdcbd96f2acf31dce92ea2e85d6c65f24bde7eafb73365b88d17b44`.
The Maps no-data copy now uses a map-owned backed panel in a clamped lower
right lane, away from the health rail and alert pills. The following exact
payload frames were read back from the physical DRM seat, visually inspected,
and accepted:

| Route/profile | PNG SHA-256 |
| --- | --- |
| Maps Dark desktop | `c5d9b113301586b81751821f1cb79eb10545d84d8deb1eaf494db22f252c91fe` |
| Maps Dark narrow | `2567edea06518e1161c0329716c1eef83a6acc79ba8047e77c119141c76a5a33` |
| Maps Light/Largest | `8938f1376572d64f734f5902dda16793b64cf546f4aea972d3900af0352e6ccf` |

The seat was restored to `require_login_at_boot:true`, dark/default appearance,
and `mde-shell-egui.service=active` with `NRestarts=0` after capture. This
closes the current-payload Maps Light/Largest contrast cell only; it does not
close the broader WL-UX-009 matrix or Dell adoption.

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
| `.138` Workloads Dark desktop (EGL readback payload `e37ec5c0`) | Infra as Code → Provision | native `1920x1080` direct-DRM EGL readback after chooser-proof routing fix and settled handoff; WORKLOADS header, lifecycle rail, placement card, plan-only node state, status tray, and taskbar are readable and bounded | `67bdbc920ffa95e89da9f33c6a155dd53992867e18d81007176ed532e7aed4b8` |
| `.138` Workloads Dark narrow (EGL readback payload `e37ec5c0`) | Infra as Code → Provision | `800` logical-width direct-DRM EGL readback; lifecycle rail, placement card, plan-only node state, and taskbar remain bounded in the intentional unused right scanout | `65a2f548fe2ce904c21cd84d468236dac4dca45672b5fc8597cb958488dedee0` |
| `.138` Workloads Light / Largest (EGL readback payload `e37ec5c0`) | Infra as Code → Provision | native `1920x1080` direct-DRM EGL readback; Light palette and largest text remain readable, with the provision body continuing below the scroll boundary | `91332b390e93dacf7c110d689e7ecf9eaa595cf2e9a4053943190544b98dfd54` |
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
| `.138` Maps Dark desktop (EGL readback payload `bccc6e94`) | Maps & Location → Drive | native `1920x1080` direct-DRM EGL readback after the responsive HUD fixes; dark banner, health rail, alerts, map content, and FAB lane remain readable and bounded | `33de1d8696ffe1f2e3c8ed37d75b6c6759ebdf4390232051f52f9f306a0c01a9` |
| `.138` Maps Light / Largest desktop (EGL readback payload `bccc6e94`) | Maps & Location → Drive | native `1920x1080` direct-DRM EGL readback after explicit map-content palette resolution; health-card labels and unavailable state remain readable and separated from the FAB lane | `29f68032d4aeed4db1ca463f55a720062f6d37eb8706ecfa646b2df2ffd818d1` |
| `.138` Maps Dark narrow (EGL readback payload `bccc6e94`) | Maps & Location → Drive | `800` logical-width direct-DRM EGL readback after reserving the FAB lane and removing the redundant narrow GPS chip; banner, health rail, primary alerts, and unused right scanout remain readable and separated | `154c43dfbde479a0b5552a0323fadb242e74d26e08e6fecbfaa911ac7e366870` |
| `.138` Maps Light / Largest narrow (EGL readback payload `bccc6e94`) | Maps & Location → Drive | `800` logical-width direct-DRM EGL readback after reserving the FAB lane and removing the redundant narrow GPS chip; banner, health rail, primary alerts, and unused right scanout remain readable and separated | `e36ac4f5331f65b5a1fcc80de8a13ad35ea12c2076a8a821f8e86da2881cf51c` |
| `.138` Workbench Dark desktop (EGL readback payload `36bf4864`) | Fleet & Mesh → Workbench | native `1920x1080` direct-DRM EGL readback; shared title/menu, plane rail, unavailable provider state, and health cards remain readable without overlap | `b414091ce7af1b5d53cd4bbc658ec9397f184d21f2b64110d7e07a8191eccbf0` |
| `.138` Workbench Dark desktop (EGL readback payload `bccc6e94`) | Fleet & Mesh → Workbench → This Node plane | native `1920x1080` direct-DRM EGL readback; STATE OF THE MESH chrome, plane rail, node health dashboard, and truthful unavailable/provider state remain readable and bounded | `00873e26c14baffe72f167e4b4664f97fe87b467cb200c00dd381c2ecaffe6c5` |
| `.138` Workloads Dark desktop (EGL readback payload `36bf4864`) | Infra as Code → Provision | native `1920x1080` direct-DRM EGL readback; shared WORKLOADS menu, lifecycle rail, placement card, and honest plan-only state remain bounded | `fccab7b0118539d285a4c6843b4ce477173a0dfc1caa03d297b90a4f2d1e6599` |
| `.138` Workloads Dark desktop (EGL readback payload `bccc6e94`) | Infra as Code → Provision | native `1920x1080` direct-DRM EGL readback; WORKLOADS menu, lifecycle rail, plan-only placement card, capacity bars, and honest no-selection body remain readable and bounded | `7443434db3b1cbfbf2341c0aba389c1df52e18586fa6eb82589256f616b5f45c` |
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
| `.138` Workbench Light / Largest narrow (EGL readback payload `bccc6e94`) | Fleet & Mesh → Workbench → This Node plane | `800` logical-width direct-DRM EGL readback; large-text STATE OF THE MESH chrome, plane rail, node snapshot/loading state, and taskbar remain readable with the body continuing below the scroll boundary | `395ff7a883b2d89735660a5e6bb3b9bf0e7f89ba2c9d27a568cf91a42e53b0d3` |
| `.138` Workloads Light / Largest narrow (EGL readback payload `333503d0`) | Infra as Code → Provision | `800` logical-width direct-DRM EGL readback; large-text WORKLOADS menu, lifecycle rail, plan-only placement state, and controls remain readable and bounded | `699e569374991820f96357d8879a067a2649d3ec60cfbb797ae22859213d8542` |
| `.138` Workloads Light / Largest narrow (EGL readback payload `bccc6e94`) | Infra as Code → Provision | `800` logical-width direct-DRM EGL readback; large-text WORKLOADS menu, lifecycle rail, plan-only node card, controls, and taskbar remain readable with the body continuing below the scroll boundary | `48f3c1c7191de1580cf4b5d659320489d98e4be69a904cf22fecb4adcdbdf3ad` |
| `.138` Media Sources Light / Largest narrow (EGL readback payload `333503d0`) | Media → Sources | `800` logical-width direct-DRM EGL readback; large-text MEDIA menu, source tabs, local/Jellyfin controls, and honest empty-source state remain readable and bounded | `f344202126cdbf266d74b96aa06808e2e5ca68ac4d0441ad2f1a98b934181f37` |
| `.138` Music unavailable Dark desktop (EGL readback payload `bccc6e94`) | Music | native `1920x1080` direct-DRM EGL readback; current MUSIC menu/status chrome and honest missing-credentials state remain readable and bounded without claiming Subsonic connectivity | `79378b49c8b50ba58e6516cd8b6805cca322c350e6cad6fd64fd93e0e075fb02` |
| `.138` Music unavailable Light / Largest narrow (EGL readback payload `bccc6e94`) | Music | `800` logical-width direct-DRM EGL readback; large-text MUSIC menu/status chrome and honest missing-credentials copy remain readable and bounded without claiming Subsonic connectivity | `4521d334105b58baa193a857707106814ed6af383e79c9053659e6f5685861db` |
| `.138` Media Sources Dark desktop (EGL readback payload `bccc6e94`) | Media → Sources | native `1920x1080` direct-DRM EGL readback; current MEDIA menu, source tabs, local-capture/Jellyfin controls, and honest empty-source copy remain readable and bounded | `8baeb2706e5d76033a0c20eeca2f3198a76f2f0fc3bcdc63903bfb351df4c0c0` |
| `.138` Media Sources Light / Largest narrow (EGL readback payload `bccc6e94`) | Media → Sources | `800` logical-width direct-DRM EGL readback; large-text MEDIA menu, source tabs, local/Jellyfin controls, and honest empty-source state remain readable and bounded | `3d27686e63a9b8e268c424118b10ffad0bb811a9df24eb66f3b48d19cd706ed3` |
| `.138` Terminal Dark desktop (EGL readback payload `bccc6e94`) | Terminal | native `1920x1080` direct-DRM EGL readback; current TERMINAL menu, tab strip, mesh overview, shell prompt, and taskbar remain separated and readable | `f8372346b7e50e2380f53ddb57053b4393576057a21c94a50ca6dafef8791b40` |
| `.138` Terminal Light / Largest narrow (EGL readback payload `bccc6e94`) | Terminal | `800` logical-width direct-DRM EGL readback; large-text TERMINAL menu, tab strip, mesh overview, shell prompt, and taskbar remain readable with the body continuing below the scroll boundary | `1ff9a316cd1ef210272de5692fc3070f6bb2cfb50f2cfbad9d8fb7a372277a25` |
| `.138` This Node Dark desktop (EGL readback payload `bccc6e94`) | This Node → System | native `1920x1080` direct-DRM EGL readback; unified node navigation, SYSTEM menu, display/device body, health rail, and taskbar remain readable and bounded | `dad7a71ff1be7234d1d0ba062b25e7e446fe4d06d6d80e7f510a8e2322240201` |
| `.138` This Node Light / Largest narrow (EGL readback payload `bccc6e94`) | This Node → System | `800` logical-width direct-DRM EGL readback; large-text unified node navigation, SYSTEM menu, display/device body, health rail, and taskbar remain readable and bounded with the body continuing below the scroll boundary | `293107bf9892b66597003acd41cfa9986f34e51659cbda35ce8323302e9bdd29` |
| `.138` Editor Dark desktop (EGL readback payload `bccc6e94`) | Mesh Teams → Editor | native `1920x1080` direct-DRM EGL readback; nested Mesh Teams/editor chrome, document/project controls, formatting rows, empty document state, status row, and details rail remain readable and bounded | `755ce34d80b7a21a9213128113f8a5da9c599b90b57e70e1f9a3042fc07bbbcd` |
| `.138` Editor Light / Largest narrow (EGL readback payload `bccc6e94`) | Mesh Teams → Editor | `800` logical-width direct-DRM EGL readback; large-text nested editor chrome, collapsed optional sidebars, document/project controls, formatting rows, empty document state, and status row remain readable and bounded | `3260869744d8c0a309c3734ba92ac004eb766ed97ebc8590feb57417f4c1c0cc` |
| `.138` Phones Dark desktop (EGL readback payload `bccc6e94`) | Phones | native `1920x1080` direct-DRM EGL readback; shared Phones title, paired/online status, mesh identity, tabs, feature card, and remote-input controls remain readable and bounded | `911b8ffc365ed6a7e80982ed68a65c5c6501626ed73399ca08b00e13eaf2ac69` |
| `.138` Phones Light / Largest narrow (EGL readback payload `bccc6e94`) | Phones | `800` logical-width direct-DRM EGL readback; large-text Phones title, paired/online status, tabs, feature card, and remote-input controls remain readable and bounded | `e22d3eb593a67f2ae389ac717cc6ad7d25c8cc163fbf9739f39dbba3cb847839` |
| `.138` Car Dark desktop (EGL readback payload `bccc6e94`) | Auto Mode → Car Home | native `1920x1080` direct-DRM EGL readback; Auto Mode title, navigation/media/vehicle cards, instrument strip, telemetry tiles, app strip, and taskbar remain separated and readable | `b068e6610435882d56c1620181424ce2ef2dcfa2ad9f74e8d13fc187c894c7d9` |
| `.138` Car Light / Largest narrow (EGL readback payload `bccc6e94`) | Auto Mode → Car Home | `800` logical-width direct-DRM EGL readback; large-text Auto Mode cockpit, zoom-aware instrument grid, width-safe telemetry elision, cards, app strip, and taskbar remain bounded | `175befcaf889810cc7b8b19fb9c033fa500627168274657c14f944297fb29e45` |
| `.138` Browser boundary Dark desktop (EGL readback payload `bccc6e94`) | Browser VM connection/unavailable state | native `1920x1080` direct-DRM EGL readback; Construct-owned Browser VM selection, transport guidance, waiting state, and no-host-UI boundary remain readable without claiming guest Chromium pixels | `736a3bea5569552d2d98d6f403af45e91fbc0104a21e34ab00c4aebc63c0428b` |
| `.138` Browser boundary Light / Largest narrow (EGL readback payload `bccc6e94`) | Browser VM connection/unavailable state | `800` logical-width direct-DRM EGL readback; large-text VDI boundary, transport guidance, waiting state, and no-host-UI copy remain readable without claiming guest Chromium pixels | `982786b7024bd72b23d157fb27c73c3111b9c7601974c7136c621d9d36c442a6` |
| `.138` Explorer Dark desktop (EGL readback payload `f58b42ba`) | Fleet & Mesh → Explorer | native `1920x1080` direct-DRM EGL readback after removing the orphaned wrapper `View` control; Explorer title, filters, mesh card, and taskbar remain readable and bounded | `df371c09a97ea494c7f470da034897b580781aaf6d7808505116c7546e742280` |
| `.138` Explorer Light / Largest narrow (EGL readback payload `f58b42ba`) | Fleet & Mesh → Explorer | `800` logical-width direct-DRM EGL readback after removing the orphaned wrapper `View` control; large-text Explorer title, filters, mesh card, and taskbar remain readable and bounded | `1f9723cb2b556b6e67c12f236ad6191cdd91eaaf0702d88a493e8be13c182973` |
| `.138` Maps Dark desktop (EGL readback payload `f58b42ba`) | Maps & Location → Drive | native `1920x1080` direct-DRM EGL readback on the Explorer-fix payload; dark banner, health rail, alerts, map content, and FAB lane remain readable and bounded | `33de1d8696ffe1f2e3c8ed37d75b6c6759ebdf4390232051f52f9f306a0c01a9` |
| `.138` Maps Light / Largest narrow (EGL readback payload `f58b42ba`) | Maps & Location → Drive | `800` logical-width direct-DRM EGL readback on the Explorer-fix payload; governed map-content contrast, large-text health rail, alerts, and FAB lane remain readable and bounded | `e36ac4f5331f65b5a1fcc80de8a13ad35ea12c2076a8a821f8e86da2881cf51c` |
| `.138` Workbench Dark desktop (EGL readback payload `f58b42ba`) | Fleet & Mesh → Workbench → This Node plane | native `1920x1080` direct-DRM EGL readback on the Explorer-fix payload; STATE OF THE MESH chrome, plane rail, node health dashboard, and truthful provider state remain readable and bounded | `55b58e118c5f1abf52d21398c0b7d6f0973fe6d933e5db7e15b78499f1f1ff1d` |
| `.138` Workbench Light / Largest narrow (EGL readback payload `f58b42ba`) | Fleet & Mesh → Workbench → This Node plane | `800` logical-width direct-DRM EGL readback on the Explorer-fix payload; large-text STATE OF THE MESH chrome, plane rail, node health dashboard, and taskbar remain readable and bounded | `db1b223ced19616ac7be1995a4a5f0bce4feee59e9ab4e1bbf6acebbdb4aab0b` |
| `.138` Workloads Dark desktop (EGL readback payload `f58b42ba`) | Infra as Code → Provision | native `1920x1080` direct-DRM EGL readback on the canonical `infra-code` route after a 10-second proof-only settle window; WORKLOADS menu, lifecycle rail, placement card, plan-only state, and taskbar remain readable and bounded | `676675aa733f9dbd05f3bea08e4275523a24ec4445be9fce777404ac7fa19951` |
| `.138` Music unavailable Light / Largest narrow (EGL readback payload `f58b42ba`) | Music | `800` logical-width direct-DRM EGL readback; large-text MUSIC menu/status chrome and honest missing-credentials copy remain readable and bounded without claiming Subsonic connectivity | `df42b3a16641ee1dcb8f9b3576fa816dba1822b642ebfc0798506e6e32c21366` |
| `.138` Media Sources Dark desktop (EGL readback payload `f58b42ba`) | Media → Sources | native `1920x1080` direct-DRM EGL readback; MEDIA menu, source tabs, local/Jellyfin controls, and honest empty-source state remain readable and bounded | `c6621041bbd306657ad3c7e51b7ffcd92390358938d65b17356c039980dd9d3c` |
| `.138` Terminal Light / Largest narrow (EGL readback payload `f58b42ba`) | Terminal | `800` logical-width direct-DRM EGL readback; large-text TERMINAL menu, tabs, mesh overview, shell prompt, and taskbar remain readable and bounded | `71509a650b4c3f44e5b5b5935e773daa03b7c1ea32c61ad0de5b4388027e9619` |
| `.138` This Node → System Dark desktop (EGL readback payload `f58b42ba`) | This Node → System | native `1920x1080` direct-DRM EGL readback; SYSTEM menu, display/device body, health rail, and taskbar remain readable and bounded | `5154fcd9df2c45ac2a752325e7822f3c4fa48ad0637e2c2c58567fec4f10c2e8` |
| `.138` Editor Dark desktop (EGL readback payload `f58b42ba`) | Mesh Teams → Editor | native `1920x1080` direct-DRM EGL readback after the settled editor route; nested editor chrome, document/project controls, formatting rows, empty document state, status row, and details rail remain readable and bounded | `417803a081078bd7e949ea4f30c439dd3062992752f356312e691ac249b15a59` |
| `.138` Phones Dark desktop (EGL readback payload `f58b42ba`) | Phones | native `1920x1080` direct-DRM EGL readback; shared Phones title, pairing status, tabs, feature card, remote-input controls, and honest unpaired state remain readable and bounded | `4fc968ac213fc12954c54d2d1167e224be9ba064f862002e4e2bf847e7ce0d73` |
| `.138` Car Dark desktop (EGL readback payload `f58b42ba`) | Auto Mode → Car Home | native `1920x1080` direct-DRM EGL readback after the settled Car route; Auto Mode title, navigation/media/vehicle cards, instrument area, app strip, and taskbar remain separated and readable | `02de499fd92b71d788ea2b69233b27952d64cb4de6a2efb1a484dfd436ba0c06` |
| `.138` This Node → System Light / Largest narrow (EGL readback payload `f58b42ba`) | This Node → System | `800` logical-width direct-DRM EGL readback; large-text SYSTEM menu, display/device body, health rail, and taskbar remain readable and bounded within the proof viewport | `af02ada0fd52bcad36ac63b04fefe6fee22fd31c8afc537bde047262edd4f4bd` |
| `.138` Editor Light / Largest narrow (EGL readback payload `f58b42ba`) | Mesh Teams → Editor | `800` logical-width direct-DRM EGL readback; large-text editor chrome, document/project controls, formatting rows, empty document state, and status row remain readable and bounded | `483a684774d1ccb904e46d858f496adfe82cca3bdc72e5bc8adc8ed84b5e3d60` |
| `.138` Phones Light / Largest narrow (EGL readback payload `f58b42ba`) | Phones | `800` logical-width direct-DRM EGL readback; large-text Phones title, pairing status, tabs, feature card, and remote-input controls remain readable and bounded | `32f53d034a24794dbb41265602d2a8fac2096c1e75e6a6a6d07d299e6c56f430` |
| `.138` Car Light / Largest narrow (EGL readback payload `f58b42ba`) | Auto Mode → Car Home | `800` logical-width direct-DRM EGL readback; the governed AutoSync3 cockpit skin remains intentionally dark, with large-text navigation/media/vehicle cards, app strip, and taskbar readable and bounded | `7591f6fb3cefc142cf2339126fa01aae870b2113caf1e040884f8e5ccf48d451` |
| `.138` This Node → System Light desktop (EGL readback payload `f58b42ba`) | This Node → System | native `1920x1080` direct-DRM EGL readback; Light SYSTEM menu, display/device body, health rail, and taskbar remain readable and bounded | `ab0df0105c9e03b236b20db753a73fc52ba4ed5a88de52f6d809a9c530728729` |
| `.138` Editor Light desktop (EGL readback payload `f58b42ba`) | Mesh Teams → Editor | native `1920x1080` direct-DRM EGL readback; Light editor chrome, document/project controls, formatting rows, empty document state, status row, and details rail remain readable and bounded | `9d8802cf59b836d976dbf29bed5820e32b8c0bef34634d8fd1e7d28d1b15558d` |
| `.138` Phones Light desktop (EGL readback payload `f58b42ba`) | Phones | native `1920x1080` direct-DRM EGL readback; Light Phones title, pairing status, tabs, feature card, remote-input controls, and honest unpaired state remain readable and bounded | `975d15e9925b7cc310a0ac55880d40755df623d17369c7347d9a233581e36ef4` |
| `.138` Files Dark desktop (EGL readback payload `f58b42ba`) | Files | native `1920x1080` direct-DRM EGL readback; FILES menu, peer roster, node actions, list/preview panes, transfer controls, and bounded status row remain readable | `44b27b970c7718022c367023e71ab6e31bc93aee13ffe5b3cb488dc20ab58db2` |
| `.138` Files Light / Largest narrow (EGL readback payload `f58b42ba`) | Files | `800` logical-width direct-DRM EGL readback; large-text FILES menu, peer roster, node actions, list pane, preview boundary, and status row remain readable and bounded | `b5824b43e14a8edc2e1aee1b9fd964fa7f588f3338a808cdaa705630864fab62` |
| `.138` Browser boundary Dark desktop (EGL readback payload `f58b42ba`) | Browser VM connection/guest boundary | native `1920x1080` direct-DRM EGL readback; Construct-owned VM selection, transport guidance, waiting state, and no-host-UI boundary remain readable; no Chromium pixels are claimed | `aeb104ca01dbf41946491d28eecbd93a40e74586d3361f2cb7568c156f2ddb91` |
| `.138` Browser boundary Light / Largest narrow (EGL readback payload `f58b42ba`) | Browser VM connection/guest boundary | `800` logical-width direct-DRM EGL readback; large-text VDI guidance and waiting state remain readable and bounded; guest Chromium remains outside the Construct surface | `511156648fb389a690eca855a810323fa776ea420398086c71791806f7168256` |
| `.138` Music unavailable Dark desktop (EGL readback payload `f58b42ba`) | Music | native `1920x1080` direct-DRM EGL readback; MUSIC menu/status chrome and honest missing-credentials state remain readable without claiming Subsonic connectivity | `2facf38c93b3fe5d83d8530dced533e517bbe007c353b0ceb6b82ab89438676a` |
| `.138` Media Sources Light / Largest narrow (EGL readback payload `f58b42ba`) | Media → Sources | `800` logical-width direct-DRM EGL readback; large-text MEDIA menu, source tabs, local/Jellyfin controls, and honest empty-source state remain readable and bounded | `51432679a7db06442eb577a2463a92b861779f38b93313fae542aaf15372b97b` |
| `.138` Terminal Dark desktop (EGL readback payload `f58b42ba`) | Terminal | native `1920x1080` direct-DRM EGL readback; TERMINAL menu, tabs, mesh overview, shell prompt, and taskbar remain separated and readable | `929db052e17fa50f95105e8cabfd5229f38d1001cf91c25c6404eff2304cf93e` |
| `.138` Browser boundary Light / Largest narrow (EGL readback payload `333503d0`) | Browser VM connection/guest boundary | `800` logical-width direct-DRM EGL readback; Construct browser controls remain distinct from the blank guest viewport, with no Chromium content-readiness claim | `b69f25c3e699c799e70234a502f92938996762152337cc4beb9c20e2fb0d2937` |
| `.138` Phones Light / Largest narrow (EGL readback payload `3bd01985`) | Phones | `800` logical-width direct-DRM EGL readback after enabling the shared leading AppFrame title; paired/online status, hub tabs, feature card, and remote-input controls remain readable and bounded | `2d96bd5592e9f3325471a469b12bb2dbd031af91982cd7e7b6875c6af0fc4dd0` |
| `.138` Editor Light / Largest narrow (EGL readback payload `3bd01985`) | Mesh Teams → Editor | `800` logical-width direct-DRM EGL readback; nested communications/editor chrome, collapsed optional sidebars, document/project controls, formatting rows, empty document state, and status row remain readable and bounded | `b68b8335f07d5017c949389e15b166cf1b669dd7a8988041f20277913b57d106` |
| `.138` Car Light / Largest narrow (EGL readback payload `736471b1`) | Auto Mode → Car Home | `800` logical-width direct-DRM EGL readback after the Car route alias, zoom-aware instrument strip, width-safe elision, and three-column large-text grid fixes; Auto Mode title/cards/app strip and all 12 selected status tiles remain readable and bounded above the taskbar | `88e022c9427345ee94adb0e164553ba655a8e6ba2c85f501be2407db17d59e5e` |
| `.138` Editor Dark desktop (EGL readback payload `736471b1`) | Mesh Teams → Editor | native `1920x1080` direct-DRM EGL readback; nested communications/editor chrome, document/project controls, formatting rows, empty document state, status row, and details rail remain readable and bounded | `10bda5359c80b0f5148c518a6d198222a4abc75e07dd20ad3c92aa81cbc1bb98` |
| `.138` Terminal Dark desktop (EGL readback payload `736471b1`) | Terminal | native `1920x1080` direct-DRM EGL readback; shared TERMINAL menu, tab strip, mesh overview, shell prompt, and taskbar remain separated and readable | `a0f7998b2d23584bd1f5be0e90f7705ca23b9d9079270c9d528d7ed3bba84448` |
| `.138` Car Dark desktop (EGL readback payload `736471b1`) | Auto Mode → Car Home | native `1920x1080` direct-DRM EGL readback; Auto Mode title, navigation/media/vehicle cards, app strip, and taskbar remain separated and readable | `eab009624b2878b6133ecbd8935adfc2abccdedcc52841fa38e26ce30501e8b6` |
| `.138` Phones Dark desktop (EGL readback payload `8366b109`) | Phones | native `1920x1080` direct-DRM EGL readback after bounding the shared status/identity rows to the AppFrame inset; title, tabs, feature/remote-input cards, and empty state remain readable and bounded | `87f5b3e9a7c3a234b66a513e04b4b875dc87612e53733f214566352ca388799d` |
| `.138` Editor Light desktop (EGL readback payload `8366b109`) | Mesh Teams → Editor | native `1920x1080` direct-DRM EGL readback after the proof-only unlock; Light Mesh Teams/editor chrome, document body, details rail, and taskbar remain readable and bounded | `bc48241b3cd0fbed2a9b4f191527cd88437060307d5c83b8f4b1bb2f02fd69c5` |
| `.138` Mesh Teams Dark desktop (EGL readback payload `f58b42ba`) | Mesh Teams | native `1920x1080` direct-DRM EGL readback after the proof-only settle window; team/channel rail, Activity feed, channel details, collaboration tabs, and taskbar remain readable and bounded | `d521d2bc9bb6ddd25c86617f3d6f39f8d12a4c5b947251b1a1d6579c408cce45` |
| `.138` Workloads Light / Largest narrow (EGL readback payload `f58b42ba`) | Infra as Code → Provision | `800` logical-width direct-DRM EGL readback after the proof-only settle window; large-text WORKLOADS menu, lifecycle rail, placement card, plan-only state, and taskbar remain readable and bounded | `a1f39bdd4017e72f76901905f9dcf5b32ecf6b0c45a347dfdf4eb2a1f644885d` |
| `.138` Workloads Light desktop (EGL readback payload `f58b42ba`) | Infra as Code → Provision | native `1920x1080` direct-DRM EGL readback after the proof-only settle window; Light WORKLOADS menu, lifecycle rail, placement card, plan-only state, and taskbar remain readable and bounded | `d79c2a0f2b3c73b99c27368f326b1336bbe9084fe6e2408786b4a2113c36f55f` |
| `.138` Workloads Dark narrow (EGL readback payload `f58b42ba`) | Infra as Code → Provision | `800` logical-width direct-DRM EGL readback after the proof-only settle window; Dark WORKLOADS menu, lifecycle rail, placement card, plan-only state, and taskbar remain readable and bounded within the proof viewport | `0208abe9756e87eb328cfbb1f882344066441bdce428d82960625cd5971d7cd8` |
| `.138` Mesh Teams Light desktop (EGL readback payload `f58b42ba`) | Mesh Teams | native `1920x1080` direct-DRM EGL readback after the proof-only settle window; Light team/channel rail, Activity feed, channel details, collaboration tabs, and taskbar remain readable and bounded | `83c7155e13c40e87c00b70e17ace7a34a8440de39e676af23dd586c8ca27a3ad` |
| `.138` Mesh Teams Dark narrow (EGL readback payload `f58b42ba`) | Mesh Teams | `800` logical-width direct-DRM EGL readback after the proof-only settle window; Dark collaboration tabs, activity feed, alert rows, and taskbar remain readable and bounded within the proof viewport | `f61714910d076e707a432dcffbfe7d4c04797fa88c8ab2307154c18361a61298` |
| `.138` Files Light desktop (EGL readback payload `f58b42ba`) | Files | native `1920x1080` direct-DRM EGL readback; Light FILES menu, peer roster, node actions, list/preview panes, transfer controls, and status row remain readable and bounded | `447a4bdbfacc8854415c6d28d331e12a70d3ea00441610ac71ca4dd8922d0499` |
| `.138` Files Dark narrow (EGL readback payload `f58b42ba`) | Files | `800` logical-width direct-DRM EGL readback; Dark FILES menu, peer roster, node actions, list pane, preview boundary, and status row remain readable and bounded within the proof viewport | `1d72243df57304af5b506a0573cd1f526e4dd09e7d3bd8a07e14a892c02e061b` |
| `.138` Explorer Light desktop (EGL readback payload `f58b42ba`) | Fleet & Mesh → Explorer | native `1920x1080` direct-DRM EGL readback; Light Explorer title, filters, mesh card, health summary, and taskbar remain readable and bounded | `ef015cd45b9f5aa6f4ea7f89868263ebec70352d46a51eec7603b181f959fe4e` |
| `.138` Explorer Dark narrow (EGL readback payload `f58b42ba`) | Fleet & Mesh → Explorer | `800` logical-width direct-DRM EGL readback; Dark Explorer title, filters, mesh card, health summary, and taskbar remain readable and bounded within the proof viewport | `6206cabf9356935ece88f503f1409e226ef59613be03cdbf489f4dc9bfcedcee` |
| `.138` Maps Dark narrow (EGL readback payload `f58b42ba`) | Maps & Location → Drive | `800` logical-width direct-DRM EGL readback; Dark banner, health rail, alerts, map-content fallback, and FAB lane remain readable and bounded within the proof viewport | `154c43dfbde479a0b5552a0323fadb242e74d26e08e6fecbfaa911ac7e366870` |
| `.138` Workbench Light desktop (EGL readback payload `f58b42ba`) | Fleet & Mesh → Workbench → This Node plane | native `1920x1080` direct-DRM EGL readback; Light STATE OF THE MESH chrome, plane rail, node health dashboard, and taskbar remain readable and bounded | `0f4fab9adcdb9d10f32233d2f83c3f61e1fa568513861cc4ef170366c85f5` |
| `.138` Workbench Dark narrow (EGL readback payload `f58b42ba`) | Fleet & Mesh → Workbench → This Node plane | `800` logical-width direct-DRM EGL readback; Dark STATE OF THE MESH chrome, plane rail, node health dashboard, and taskbar remain readable and bounded within the proof viewport | `0a597fc5b1fd3d8d51bee06963e53355a0741fdcbf808e14e101afcfce52b5ee` |
| `.138` Maps Light desktop (EGL readback payload `bbe6a88b`) | Maps & Location → Drive | native `1920x1080` direct-DRM EGL readback after the no-data contrast fix; governed map-content fallback copy, health rail, alerts, and FAB lane remain readable and bounded | `0d59d2e1cd2ef36ff7ed7fd74c44725432469f59eac9a20308090e1edf4096f7` |
| `.138` Music unavailable Light desktop (EGL readback payload `bbe6a88b`) | Music | native `1920x1080` direct-DRM EGL readback; Light MUSIC menu/status chrome and honest missing-credentials state remain readable and bounded without claiming Subsonic connectivity | `a343fa34090e1bd18baf5b43b559b8fb9d15c9ededfdeda0e0bf54a14d6869c6` |
| `.138` Music unavailable Dark narrow (EGL readback payload `bbe6a88b`) | Music | `800` logical-width direct-DRM EGL readback; Dark MUSIC menu/status chrome and honest missing-credentials state remain readable and bounded within the proof viewport | `50a620d47acca21971f6ba5cb65caecb92488194c26c42e22bfddcd5d39ad929` |
| `.138` Media Sources Light desktop (EGL readback payload `bbe6a88b`) | Media → Sources | native `1920x1080` direct-DRM EGL readback; Light MEDIA menu, source tabs, local/Jellyfin controls, and honest empty-source state remain readable and bounded | `453621e3b0be9f9dbe54700ff499ee33bffa4d883a9df277fcbb95d9ce602109` |
| `.138` Media Sources Dark narrow (EGL readback payload `bbe6a88b`) | Media → Sources | `800` logical-width direct-DRM EGL readback; Dark MEDIA menu, source tabs, local/Jellyfin controls, and honest empty-source state remain readable and bounded within the proof viewport | `6b82836be6c9ce188a605d7f59e350543144d70bfa1c1659b167358d4d882632` |
| `.138` Terminal Light desktop (EGL readback payload `bbe6a88b`) | Terminal | native `1920x1080` direct-DRM EGL readback; Light TERMINAL menu, tab/pane strip, mesh overview, shell prompt, and taskbar remain separated and readable | `3c9f2ae3699717799fe2714166f2d1a76822eb4d5afad46baff29903846155d4` |
| `.138` Terminal Dark narrow (EGL readback payload `bbe6a88b`) | Terminal | `800` logical-width direct-DRM EGL readback; Dark TERMINAL menu, tab/pane strip, mesh overview, shell prompt, and taskbar remain bounded and readable within the proof viewport | `43c9a93eb7ac0e882f524bcd05d8d1b66d7085fb815010ad2663d082e0155093` |
| `.138` This Node → System Dark narrow (EGL readback payload `bbe6a88b`) | This Node → System | `800` logical-width direct-DRM EGL readback; Dark SYSTEM menu, settings navigation, display controls, health rail, and taskbar remain readable and bounded | `9a9d2de5931e0305105686eacb638b988d7a8df9387b9893f878d56693cd3341` |
| `.138` Editor Dark narrow (EGL readback payload `bbe6a88b`) | Mesh Teams → Editor | `800` logical-width direct-DRM EGL readback; Dark nested editor chrome, document/project controls, formatting rows, document body, and status row remain readable and bounded | `df916ce9737925df75794a6261da80c6c029597e5bba021ca520e583c91bce84` |
| `.138` Phones Dark narrow (EGL readback payload `bbe6a88b`) | Phones | `800` logical-width direct-DRM EGL readback; Dark Phones title, pairing state, tabs, feature card, remote-input controls, and honest empty state remain readable and bounded | `61e248bce70468a0b9c19ff1e24485eb321370d0d2dae9a9ad6258e4658e208b` |
| `.138` Car Light desktop (EGL readback payload `bbe6a88b`) | Auto Mode → Car Home | native `1920x1080` direct-DRM EGL readback; the governed AutoSync3 cockpit skin remains intentionally dark under Light profile, with navigation/media/vehicle cards and taskbar readable and bounded | `d1ff1660eaf343bc9103fcebe8b22234f336df111a4c02d52959ce3a3e7150c5` |
| `.138` Car Dark narrow (EGL readback payload `bbe6a88b`) | Auto Mode → Car Home | `800` logical-width direct-DRM EGL readback; governed AutoSync3 cockpit skin, large-text navigation/media/vehicle cards, and taskbar remain bounded | `2eea21043c9d7cb1dd4ef5846098ed9d1792d6b2b6759c89846e911ade03b877` |
| `.138` Browser boundary Light desktop (EGL readback payload `bbe6a88b`) | Browser VM connection/guest boundary | native `1920x1080` direct-DRM EGL readback; Construct-owned VM guidance and waiting state remain readable; guest Chromium pixels remain outside the host surface | `0d929dcd8170a6533871efd5abd6843ae9892619b043d2405f6d210d1a3807c6` |
| `.138` Browser boundary Dark narrow (EGL readback payload `bbe6a88b`) | Browser VM connection/guest boundary | `800` logical-width direct-DRM EGL readback; Dark VDI guidance, waiting state, and no-host-UI boundary remain readable and bounded; no guest readiness is claimed | `9640186a7296d7d4f364a447733a51f629ba3f673081830ebfc7120ddaba37f5` |
| `.138` Bookmarks Dark desktop (EGL readback payload `bbe6a88b`) | Bookmarks → Manager | native `1920x1080` direct-DRM EGL readback; Dark manager rail, bookmark list/detail panes, empty state, and taskbar remain readable and bounded | `84b7ca562b2c761dd5dab4f276a1ab72fa24b821c66a5b14110778cd39a94dec` |
| `.138` Bookmarks Light desktop (EGL readback payload `bbe6a88b`) | Bookmarks → Manager | native `1920x1080` direct-DRM EGL readback; Light manager rail, bookmark list/detail panes, empty state, and taskbar remain readable and bounded | `45b929245aa29889f43653e02bdc92ce25dc48a894ab2ccf2ec04898bf4288ff` |
| `.138` Bookmarks Dark narrow (EGL readback payload `bbe6a88b`) | Bookmarks → Manager | `800` logical-width direct-DRM EGL readback; Dark manager rail, list/detail panes, and empty state remain bounded within the proof viewport | `639c84c873f54f301499f64f465577da45bf9a1f1c2db9cd1b3b643805a6e066` |
| `.138` Bookmarks Light / Largest narrow (EGL readback payload `bbe6a88b`) | Bookmarks → Manager | `800` logical-width direct-DRM EGL readback; Light large-text manager content remains readable and bounded within the proof viewport | `8492a0599d76ffa96acfba7f6131a706e7eabec87c22e3930f8f03d4fd387de8` |
| `.138` Storage Dark desktop (EGL readback payload `bbe6a88b`) | This Node → Storage | native `1920x1080` direct-DRM EGL readback; Dark local-cylinder, disk, peer, capacity, and taskbar content remain readable and bounded | `9a95ed0f27ae91d84576381c446f47dd2547d355ae147c708f264f760f65368a` |
| `.138` Storage Light desktop (EGL readback payload `bbe6a88b`) | This Node → Storage | native `1920x1080` direct-DRM EGL readback; Light local-cylinder, disk, peer, capacity, and taskbar content remain readable and bounded | `a4ddb64e73e0ff7f73531868ed11cfe79c6a8050fdf8d9599c990cf5140d816f` |
| `.138` Storage Dark narrow (EGL readback payload `bbe6a88b`) | This Node → Storage | `800` logical-width direct-DRM EGL readback; Dark local-cylinder, disk, peer, and capacity content remains bounded with intentional unused right scanout | `b9f8dcadfb8e4a7e37596f5facea9452d288bd2b622f8af81a94805a1e1c1023` |
| `.138` Storage Light / Largest narrow (EGL readback payload `bbe6a88b`) | This Node → Storage | `800` logical-width direct-DRM EGL readback; Light large-text storage content remains readable and bounded with vertical continuation below the viewport | `f0986b3effdb29ec9f39aa1857904d781e0725ca740054b8ca4b6a6bf7b1b7a1` |
| `.138` Timers Dark desktop (EGL readback payload `bbe6a88b`) | Timers & Alarms | native `1920x1080` direct-DRM EGL readback; Dark timer/alarm header, clock, controls, and taskbar remain readable and bounded | `38a1c7ec34240f514a7eabf242b8d2c1b43f67aef4482d143e92d5c79d381100` |
| `.138` Timers Light desktop (EGL readback payload `bbe6a88b`) | Timers & Alarms | native `1920x1080` direct-DRM EGL readback; Light timer/alarm header, clock, controls, and taskbar remain readable and bounded | `837ee22f0f6af4c3bb937df687adfec728b5522c43ec0e3d3d9a85dd29d00992` |
| `.138` Timers Dark narrow (EGL readback payload `bbe6a88b`) | Timers & Alarms | `800` logical-width direct-DRM EGL readback; Dark timer/alarm header, clock, and controls remain readable and bounded | `b145603d4dee533f33eab2f71ef31cab6bd744d0c9705257c3950c85d374510b` |
| `.138` Timers Light / Largest narrow (EGL readback payload `bbe6a88b`) | Timers & Alarms | `800` logical-width direct-DRM EGL readback; Light large-text timer/alarm controls remain readable and bounded | `1c45520b6b7dc157042b472b33c689f8a904e5c4f21f3bd9ea4d990ee8c4401d` |
| `.138` About Dark desktop (EGL readback payload `bbe6a88b`) | This Node → About | native `1920x1080` direct-DRM EGL readback; Dark device inventory, host/mesh/router information, health rail, and taskbar remain readable and bounded | `8ae82cbf4a2dc11bde84f62467aac1cc67fa16e135e53a92db4833bfe7d91add` |
| `.138` About Light desktop (EGL readback payload `bbe6a88b`) | This Node → About | native `1920x1080` direct-DRM EGL readback; Light device inventory, host/mesh/router information, health rail, and taskbar remain readable and bounded | `d1401d10de38c81d25d98424965bfaf1e2d071ef539b6fd2544ed1c470a76335` |
| `.138` About Dark narrow (EGL readback payload `bbe6a88b`) | This Node → About | `800` logical-width direct-DRM EGL readback; Dark device inventory and node information remain readable and bounded within the proof viewport | `2f20121d8dd90140e426b41b57e66f72028abb4209671dbdd2a66426b5dd0893` |
| `.138` About Light / Largest narrow (EGL readback payload `bbe6a88b`) | This Node → About | `800` logical-width direct-DRM EGL readback; Light large-text device inventory and node information remain readable and bounded | `02197de6cb79d9fcb0a0d0594a8a57298de325df391d6c02fe0810ed0f411362` |

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

## Current navigation/tray payload recapture

The current navigation, launcher, tray, and Files changes were built as release
payload `4cf673e2edb1f597571b972b067a88c0a995d7f55dddff19ca7a2201913cdee0`
with `drm,live-vdi,media-mpv`, installed directly on Dell `.225` and `.138`,
and verified by matching `/usr/bin/mde-shell-egui` hashes. Both seats reported
`mde-shell-egui.service` active with `NRestarts=0` after restoration.

On `.138`, proof-only EGL readback captured the current payload in This Node
Dark (`aae809d959f81f9049385c4840ee6dd6fedb4cbaa8c2eceb8a9ae72a90713d5e`),
This Node Light/Largest narrow
(`77f336a7e8978cf5c866ca2ff8d95efc7259b8d81745e810801f3848f800c1ac`), and
Terminal Dark
(`9674a818f45c5b97f18f6fd71d1788d444f60520baf63b84726092b66a67d35f`). These
frames were visually inspected and remain bounded/readable. The empty
Desktop/Remote Sessions frame was not accepted as tray evidence because the
seat has no live remote session. The proof drop-in, appearance overrides, and
boot-curtain override were removed; `.138` was restored to Dark/Default,
`require_login_at_boot:true`, and an active zero-restart service.

The Maps Light/Largest narrow recapture on payload
`bccc6e94dd7dc32427f6b809e92b36fb0c06591fb1af2dd58a18dc399148a3e2` was
visually inspected after the responsive banner/FAB-lane and GPS-chip changes.
It is accepted as a bounded narrow cell. The matching current-payload Maps Dark
desktop, Light/Largest desktop, and Dark narrow frames were subsequently
captured, visually inspected, and accepted as the four-cell Maps proof slice.

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

The current f58b42ba Maps Light desktop readback was pixel-valid but rejected
after visual inspection: the no-offline-map fallback inherited dark shared
shell text over the dark map-content canvas. Its rejected hash is
`48ef08456557ae5c30b8bfb10be9a00a95a7e0efdbc0c096338cb60ed43afc1c`; it is
not part of the passing evidence table. A source-level map-content palette fix
and fresh proof are required before this cell can be accepted. The replacement
payload `bbe6a88b…` uses explicit high-contrast map-content tokens in the
offline fallback; its replacement Light desktop frame is recorded above and
was visually accepted.

## Boundary authority

The approved rendering boundaries remain documented in
[`docs/design/platform-interfaces.md`](../design/platform-interfaces.md):
focused VDI retains guest pixels, Maps retains its governed content-color
exception, and the Browser VM owns Chromium pixels and chrome. Construct owns
the connection, unavailable, reconnect, and diagnostic states around those
guests; no additional visual exceptions are introduced here.

## Editor large-text menubar correction — 2026-08-02

The shared `MenuBar` now gives large-text and constrained editor panes an
explicit two-row contract: the menu strip stays single-line and horizontally
scrollable, while the status row remains visible. This prevents the nested
Editor toolbar from wrapping `Help` onto an accidental third line or consuming
the document body. Farm tests passed for `mde-egui` (269/269) and
`mde-editor-egui` (407/407); the deployed Dell `.138` release payload is
`4808bd30bfa72ab386056cd1ecbc4d6aac0251a144609aedcee3e209b8dc888c`.

The accepted Light/Largest direct-DRM EGL readback is [Editor proof PNG](evidence/WL-UX-009-2026-08-02-138-editor-light-largest-4808bd30.png),
SHA-256 `b1e0bafea6d63cd88f0979d11024da56a05611115f1e8ee52bbc4c19035371cb`.
Visual inspection confirms one readable menu row, visible formatting controls,
bounded document body, details rail, and taskbar. Dell was restored to
Dark/Default appearance, `require_login_at_boot:true`, active service, and
`NRestarts=0`. This closes the Editor large-text menubar correction slice only;
the broader WL-UX-009 route/profile matrix and production-readiness decision
remain open.

## Media exact-current-candidate matrix — 2026-08-02

Exact candidate `ae51c12447c342aa9457e7db148cdb046595eeb533463a731b712cd1b1bb0236`
was temporarily installed on `.138` and Dell `.225`, explicitly routed to
Media, and captured with proof-only EGL readback. All four profiles were
visually inspected on both seats:

| Seat / profile | SHA-256 | Proof |
|---|---|---|
| `.138` Dark desktop | `0052cfcce167a0d8758544ac2bd64f13a409487aab720677a62dfd5a15f4e3b6` | [PNG](evidence/WL-UX-009-2026-08-02-138-media-dark-desktop-ae51c1244.png) |
| `.138` Dark narrow (`800` logical) | `15a174295b71cdf57cb5f2be5306b8ec850ebc72d7c105428d3ae86ceaab5b1a` | [PNG](evidence/WL-UX-009-2026-08-02-138-media-dark-narrow-ae51c1244.png) |
| `.138` Light desktop | `6117d7719eaa92e504d13a4f42c110b4f39f916af05729acf585080344640952` | [PNG](evidence/WL-UX-009-2026-08-02-138-media-light-desktop-ae51c1244.png) |
| `.138` Light/Largest narrow (`800` logical) | `6291399fc54e3289fd043d65f5a12a6357fd979b9a3622ae9c9bdace1716de5a` | [PNG](evidence/WL-UX-009-2026-08-02-138-media-light-largest-narrow-ae51c1244.png) |
| Dell `.225` Dark desktop | `cc704b46e5c869647f3e5f922253956737ff5ed72578c3a4330fb27eabfee7ff` | [PNG](evidence/WL-UX-009-2026-08-02-225-media-dark-desktop-ae51c1244.png) |
| Dell `.225` Dark narrow (`800` logical) | `4f15c595700e2f3d1d2c0f8259555987539351d174253aedf93be1bad7884fd2` | [PNG](evidence/WL-UX-009-2026-08-02-225-media-dark-narrow-ae51c1244.png) |
| Dell `.225` Light desktop | `b229069fbaf16f1edfeac69e5b484b7e8d936d209f770bdbc355035a54610862` | [PNG](evidence/WL-UX-009-2026-08-02-225-media-light-desktop-ae51c1244.png) |
| Dell `.225` Light/Largest narrow (`800` logical) | `7ccd989c389d4de42de1a21bef6a55d6e03b134e026728b0c605c04ba87c30d2` | [PNG](evidence/WL-UX-009-2026-08-02-225-media-light-largest-narrow-ae51c1244.png) |

The shared MEDIA identity/menu, source tabs, local/Jellyfin controls, honest
empty-source state, and taskbar remain readable and bounded in every accepted
frame. Both seats were restored to payload `20955383…`, secure login-at-boot,
Dark/Construct/Default/Normal, active service, zero restarts, and no proof
drop-in. This closes only the exact-candidate Media matrix slice; strict
linear scanout, VDI guest readiness, the remaining route/profile matrix, and
overall WL-UX-009 readiness remain open.

## Phones exact-current-candidate matrix — 2026-08-02

Exact candidate `ae51c12447c342aa9457e7db148cdb046595eeb533463a731b712cd1b1bb0236`
was temporarily installed on `.138` and Dell `.225`, explicitly routed to
Phones, and captured with proof-only EGL readback. All four profiles were
visually inspected on both seats:

| Seat / profile | SHA-256 | Proof |
|---|---|---|
| `.138` Dark desktop | `39717f9e9078025db21c20169c8fd584e4f073c559ab9d98a508b42e455397f2` | [PNG](evidence/WL-UX-009-2026-08-02-138-phones-dark-desktop-ae51c1244.png) |
| `.138` Dark narrow (`800` logical) | `95495e3ed46be852ef3eb33c24b11cec6101c59541576d6d47a06eb340d6148d` | [PNG](evidence/WL-UX-009-2026-08-02-138-phones-dark-narrow-ae51c1244.png) |
| `.138` Light desktop | `4b8342e2216b9c6a91be297c01bd347175681e4d48fc4ba4a3f101f94d9d95a0` | [PNG](evidence/WL-UX-009-2026-08-02-138-phones-light-desktop-ae51c1244.png) |
| `.138` Light/Largest narrow (`800` logical) | `bca017b37bdb190f3be7857dd0efe7685886e1b89c62b8dd122d109d9155403c` | [PNG](evidence/WL-UX-009-2026-08-02-138-phones-light-largest-narrow-ae51c1244.png) |
| Dell `.225` Dark desktop | `7cf7f05c7321b1292991891fa2da5472704f3c162f03a34a1f6be833aff8b70d` | [PNG](evidence/WL-UX-009-2026-08-02-225-phones-dark-desktop-ae51c1244.png) |
| Dell `.225` Dark narrow (`800` logical) | `1d1f1e667fdd520bd36eb6a8598cb30eb481e649f73f96206df17d2bdbd5a59f` | [PNG](evidence/WL-UX-009-2026-08-02-225-phones-dark-narrow-ae51c1244.png) |
| Dell `.225` Light desktop | `2fbec2b236555675c3c4869c4b8ad5301b13c7a6caebb8bf893c5eb56d15a009` | [PNG](evidence/WL-UX-009-2026-08-02-225-phones-light-desktop-ae51c1244.png) |
| Dell `.225` Light/Largest narrow (`800` logical) | `604cd32e5eaf5243a26b22b34a4c450efaa58fcad0858ece5e66868ca07de64c` | [PNG](evidence/WL-UX-009-2026-08-02-225-phones-light-largest-narrow-ae51c1244.png) |

The shared Phones title/header, pairing state, tabs, feature and remote-input
controls, and large-text body remain readable and bounded in every accepted
frame. Both seats were restored to payload `20955383…`, secure login-at-boot,
Dark/Construct/Default/Normal, active service, zero restarts, and no proof
drop-in. This closes only the exact-candidate Phones matrix slice; strict
linear scanout, VDI guest readiness, the remaining route/profile matrix, and
overall WL-UX-009 readiness remain open.

## This Node exact-current-candidate matrix — 2026-08-02

Exact candidate `ae51c12447c342aa9457e7db148cdb046595eeb533463a731b712cd1b1bb0236`
was temporarily installed on `.138` and Dell `.225`, explicitly routed to
This Node, and captured with proof-only EGL readback. All four profiles were
visually inspected on both seats:

| Seat / profile | SHA-256 | Proof |
|---|---|---|
| `.138` Dark desktop | `579cb99320b4d544811537584cce81e333997694c9755acda150d7bd64bdc104` | [PNG](evidence/WL-UX-009-2026-08-02-138-this-node-dark-desktop-ae51c1244.png) |
| `.138` Dark narrow (`800` logical) | `90b72e199a7861869b2eb0ea71e72407942ef362e94deedb2b33a06f22c0e832` | [PNG](evidence/WL-UX-009-2026-08-02-138-this-node-dark-narrow-ae51c1244.png) |
| `.138` Light desktop | `f129dfe3d0c0a99a76d1c371ab52fea17aaf02ff10a6f1197a00bcbc453e0b00` | [PNG](evidence/WL-UX-009-2026-08-02-138-this-node-light-desktop-ae51c1244.png) |
| `.138` Light/Largest narrow (`800` logical) | `248b6cb4e50ed5a9712161677d926734329756f9794eedd8870575eaf376cdfe` | [PNG](evidence/WL-UX-009-2026-08-02-138-this-node-light-largest-narrow-ae51c1244.png) |
| Dell `.225` Dark desktop | `0e5be4a903b17aaa27cfff0009a7c09886b3409f5a6692f30c55d406d9facc71` | [PNG](evidence/WL-UX-009-2026-08-02-225-this-node-dark-desktop-ae51c1244.png) |
| Dell `.225` Dark narrow (`800` logical) | `e95399f455f6730fdceffcfcf96c08385137eb8023dec0914de4d4fbd12b93d3` | [PNG](evidence/WL-UX-009-2026-08-02-225-this-node-dark-narrow-ae51c1244.png) |
| Dell `.225` Light desktop | `bffe2356eb72032f0bb4f87269faf924ad1551f527d4760fabaaf6fdbec0db05` | [PNG](evidence/WL-UX-009-2026-08-02-225-this-node-light-desktop-ae51c1244.png) |
| Dell `.225` Light/Largest narrow (`800` logical) | `d575d24f9d49ffe15cf774c8e44f932959c70f8f31b8b7cb2d15085bbf42eacd` | [PNG](evidence/WL-UX-009-2026-08-02-225-this-node-light-largest-narrow-ae51c1244.png) |

The unified node navigation, status/health hierarchy, device and local-
operations body, large-text continuation, and taskbar remain readable and
bounded in every accepted frame. Both seats were restored to payload
`20955383…`, secure login-at-boot, Dark/Construct/Default/Normal, active
service, zero restarts, and no proof drop-in. This closes only the
exact-candidate This Node matrix slice; strict linear scanout, VDI guest
readiness, the remaining route/profile matrix, and overall WL-UX-009 readiness
remain open.

## Terminal exact-current-candidate matrix — 2026-08-02

Exact candidate `ae51c12447c342aa9457e7db148cdb046595eeb533463a731b712cd1b1bb0236`
was temporarily installed on `.138` and Dell `.225`, explicitly routed to
Terminal, and captured with proof-only EGL readback. All four profiles were
visually inspected on both seats:

| Seat / profile | SHA-256 | Proof |
|---|---|---|
| `.138` Dark desktop | `62b27660b46432ed49d87a980cfb94c921274195341b4c53cd092551d7cd31b0` | [PNG](evidence/WL-UX-009-2026-08-02-138-terminal-dark-desktop-ae51c1244.png) |
| `.138` Dark narrow (`800` logical) | `c7fa6e0d251b2665576d217bce009ea82acb4aa2b0834dd340a0adaad9e30c4a` | [PNG](evidence/WL-UX-009-2026-08-02-138-terminal-dark-narrow-ae51c1244.png) |
| `.138` Light desktop | `f35863a61e4d3c7ee9632060ef32d0348f1656a4e08754c3e251af2d358a5cc9` | [PNG](evidence/WL-UX-009-2026-08-02-138-terminal-light-desktop-ae51c1244.png) |
| `.138` Light/Largest narrow (`800` logical) | `6b1a28cce42f62d3cc1488951a3c59b09242347e2e3e56574b4644851252dd8d` | [PNG](evidence/WL-UX-009-2026-08-02-138-terminal-light-largest-narrow-ae51c1244.png) |
| Dell `.225` Dark desktop | `b225d4e93339f810661c547870581fcb2489091de968e73f7b096d18cfcc66c8` | [PNG](evidence/WL-UX-009-2026-08-02-225-terminal-dark-desktop-ae51c1244.png) |
| Dell `.225` Dark narrow (`800` logical) | `652dbe940a0d611a73cf8735d68629a4378ea3cc68f8de558b096a482014a7d3` | [PNG](evidence/WL-UX-009-2026-08-02-225-terminal-dark-narrow-ae51c1244.png) |
| Dell `.225` Light desktop | `8fedd3970420e333942c397950233864127a18c8e901a6fe2e370df03842df6d` | [PNG](evidence/WL-UX-009-2026-08-02-225-terminal-light-desktop-ae51c1244.png) |
| Dell `.225` Light/Largest narrow (`800` logical) | `b11619d1b85c6cee38fabd650f46a8ff2783a56f4e27876d2dc00ec8032335ca` | [PNG](evidence/WL-UX-009-2026-08-02-225-terminal-light-largest-narrow-ae51c1244.png) |

The complete `TERMINAL` identity, menu/session controls, shell body, taskbar
contrast, and bounded narrow layout remain readable in every accepted frame.
Both seats were restored to payload `20955383…`, secure login-at-boot,
Dark/Construct/Default/Normal, active service, zero restarts, and no proof
drop-in. This closes only the exact-candidate Terminal matrix slice; strict
linear scanout, VDI guest readiness, the remaining route/profile matrix, and
overall WL-UX-009 readiness remain open.

## Maps empty-state panel placement — 2026-08-02

The current release payload `4808bd30bfa72ab386056cd1ecbc4d6aac0251a144609aedcee3e209b8dc888c`
was recaptured on Dell `.138` with explicit `maps-location` routing and the
settled CPU-linear EGL readback. The map-owned no-data panel uses a clamped
right-biased placement with its own high-contrast content palette, keeping both
lines below the Radio & GNSS rail and clear of the alert pills.

| Profile | SHA-256 | Proof |
|---|---|---|
| Dark desktop | `c5d9b113301586b81751821f1cb79eb10545d84d8deb1eaf494db22f252c91fe` | [PNG](evidence/WL-UX-009-2026-08-02-138-maps-dark-desktop-4808bd30.png) |
| Dark narrow (`MDE_DRM_PROOF_LOGICAL_WIDTH=800`) | `2567edea06518e1161c0329716c1eef83a6acc79ba8047e77c119141c76a5a33` | [PNG](evidence/WL-UX-009-2026-08-02-138-maps-dark-narrow-4808bd30.png) |
| Light/Largest | `8938f1376572d64f734f5902dda16793b64cf546f4aea972d3900af0352e6ccf` | [PNG](evidence/WL-UX-009-2026-08-02-138-maps-light-largest-4808bd30.png) |

All three frames were visually inspected. The narrow frame intentionally
truncates dense health-card labels within their bounded cards; the empty-state
panel remains fully visible. Dell was restored to Dark/Default,
`require_login_at_boot:true`, active service, and `NRestarts=0`. This closes
the Maps empty-state placement slice only; the broader route/profile matrix,
Dell `.225` adoption, and WL-UX-009 readiness remain open.

## Browser boundary and Light taskbar contrast — 2026-08-02

The bottom taskbar/tray now uses the fixed high-contrast taskbar foreground
instead of resolving page text tokens over its opaque black backing. The
release payload `728e0dcb34706793e05adf0191ddc71b3130200af415b4ae0561f91660f3401d`
was installed on Dell `.138`; status-bar tests passed 21/21 and the service
returned active with `NRestarts=0` after proof.

| Profile | SHA-256 | Proof |
|---|---|---|
| Browser Dark desktop | `3a830120db0a4df370a4f700cd40e33a70cbf479301787719e61af47503937e4` | [PNG](evidence/WL-UX-009-2026-08-02-138-browser-dark-desktop-728e0dcb.png) |
| Browser Dark narrow (`MDE_DRM_PROOF_LOGICAL_WIDTH=800`) | `f301a1421b9c090451e8d918bed6af4b639e7649850e5d4c6f369697ada4a07f` | [PNG](evidence/WL-UX-009-2026-08-02-138-browser-dark-narrow-728e0dcb.png) |
| Browser Light/Largest | `b46569251ae22d9b2db082827d8f5b90f815735b45c1970d2ba5ad0038ce6e90` | [PNG](evidence/WL-UX-009-2026-08-02-138-browser-light-largest-728e0dcb.png) |

All three Browser frames were visually inspected. They show only the
Construct-owned Browser VM selection, transport guidance, waiting state, and
explicit no-host-UI boundary; no guest Chromium pixels are claimed. The
neighboring Maps Light/Largest recapture on the same payload is [also
recorded](evidence/WL-UX-009-2026-08-02-138-maps-light-largest-728e0dcb.png),
SHA-256 `8938f1376572d64f734f5902dda16793b64cf546f4aea972d3900af0352e6ccf`;
its map panel remains separated from the health/alert rail. The Browser
boundary slice and taskbar contrast correction are closed; overall
WL-UX-009 readiness, full matrix completion, strict linear scanout, and Dell
`.225` synchronized adoption remain open.

## This Node tooltip governance correction — 2026-08-02

This Node now keeps its provider/health explanation inside the shared themed
hover-card helper instead of adding a raw `egui` tooltip modifier. The style
leak gate reports zero leaks, and the full `mde-shell-egui` farm binary suite
passes 1363/1363 on `.90` slot `wl-ux-009-tooltip-suite-20260802`.

This is a source-level governance correction only. No new live-render payload
was deployed for this change; the current `.138` visual evidence remains
explicitly scoped to its recorded release payloads and does not claim this
tooltip correction is live there.

## This Node current-source proof — 2026-08-02

BigBoy release slot `wl-ux-009-current-source-adoption-20260802` produced the
exact payload
`15efa11a2cd84563cef4af6d94455df0a286b5b0b49a4065e52a4d00d541cac2`.
It was installed on both `.138` and `.225`; both services reported active with
zero restarts. The `.138` seat was returned to secure Dark/Default state with
`require_login_at_boot:true` after proof. `.225` has no new visual capture in
this slice, so this is binary adoption evidence, not synchronized visual
readiness.

| Profile | SHA-256 | Proof |
|---|---|---|
| This Node Dark desktop | `60f89eff33e497dd50284f2b580505389d664f00a4625f32036b1ef89a36ddea` | [PNG](evidence/WL-UX-009-2026-08-02-138-this-node-dark-desktop-15efa11a.png) |
| This Node Dark narrow (`MDE_DRM_PROOF_LOGICAL_WIDTH=800`) | `9a652dcc12733c05d3472c3c83b7f56b14fb77edfc1d9b963ab51aaff5587e10` | [PNG](evidence/WL-UX-009-2026-08-02-138-this-node-dark-narrow-15efa11a.png) |
| This Node Light/Largest | `1363783acc167814e7b9c363262308291e6211cf08d69ae7a963e2acdd18e431` | [PNG](evidence/WL-UX-009-2026-08-02-138-this-node-light-largest-15efa11a.png) |

All three frames were visually inspected. The direct DRM proof uses the
CPU-linear EGL readback path; strict linear scanout remains unavailable on
`.138`. The narrow frame retains intentional unused scanout, and the
Light/Largest frame keeps the System body bounded below the visible scroll
boundary without overlap or clipping. This closes only the named This Node
cells; the remaining current-payload matrix, `.225` visual proof, strict
linear scanout, and overall WL-UX-009 readiness remain open.

## This Node power-profile tooltip governance correction — 2026-08-02

The This Node power-profile action now uses the shared themed hover-card helper
instead of a raw `egui` tooltip modifier. The style-leak gate reports zero
leaks, and the focused This Node farm suite passes 34/34 on `.90` slot
`wl-ux-009-tooltip-power-suite-20260802`.

This source correction was made after the `15efa11a…` deployment and is not
claimed as live in the visual proof above; a subsequent release is required to
make it part of the installed payload.

## This Node Dell scroll-boundary correction — 2026-08-02

The unified This Node body now uses an explicit vertical scroll boundary so
the tree, workspace selector, overview card, and provider detail cannot clip
the lower System content on the `1366x768` Dell seat. Farm responsive proof
passed 1/1 for the shared-frame test. Release payload
`08cf147dc4c84faa591f8af478a9c18cf53c23cfe6a6fb73f491a21887f77e31` is
installed on both `.138` and `.225`, each active with zero restarts.

The `.225` body geometry frames below were visually inspected:

| Profile | SHA-256 | Proof |
|---|---|---|
| Dark desktop | `3c2fa3f2f7b57e683f18ed812df15f51939478663f631e2d74b0209435602240` | [PNG](evidence/WL-UX-009-2026-08-02-225-this-node-dark-desktop-08cf147d.png) |
| Light/Largest | `6b55b6104a29c5c6691d129e726ae4350dd5b2474fcfacfd9471b744b419f7e3` | [PNG](evidence/WL-UX-009-2026-08-02-225-this-node-light-largest-08cf147d.png) |

The Dark narrow frame remains rejected because it paints only the title;
rejected hash `30cebe431e5c80513c0fc599dc9e0f9b8ea101e513fe9f625cb3b529540a5af5`
is retained as [rejected evidence](evidence/WL-UX-009-2026-08-02-225-this-node-dark-narrow-rejected-08cf147d.png).
These rows prove body geometry only; bottom-tray behavior and the remaining
current-payload matrix are still open. Strict linear scanout remains
unavailable on `.138`.

The same release was recaptured on `.138` at `800` logical width. The full
This Node body, workspace selector, System tabs, power/status strip, and
taskbar remain readable and bounded:

| Seat / profile | SHA-256 | Proof |
|---|---|---|
| `.138` Dark narrow (`MDE_DRM_PROOF_LOGICAL_WIDTH=800`) | `47bbe29925791a8b9a04f39aac58c57f407129b4971b189aa114663e72b6aca3` | [PNG](evidence/WL-UX-009-2026-08-02-138-this-node-dark-narrow-08cf147d.png) |

Dell `.225` at the same exact payload and logical width still paints only the
This Node title; its rejected hash is recorded above. This differential proof
keeps the remaining `.225` narrow issue hardware/runtime-specific rather than
claiming a shared-source failure.

## Dell narrow differential — Terminal and Files — 2026-08-02

For the exact `08cf147d…` payload and `MDE_DRM_PROOF_LOGICAL_WIDTH=800`, Dell
`.225` renders both comparison workspaces completely. Terminal’s title, menu,
session chrome, and terminal body are bounded; Files’ node-action rail,
toolbar, list, preview boundary, and status row are readable without overlap.

| Route | SHA-256 | Proof |
|---|---|---|
| Terminal Dark narrow | `aec655d875897f85d29a7724224f54a0b9e9a605a149e594b37cdee5b1cb0f57` | [PNG](evidence/WL-UX-009-2026-08-02-225-terminal-dark-narrow-08cf147d.png) |
| Files Dark narrow | `0948cb4cf4bad50e5f6e35c349fdf4b1e1e9e7e40b4c166fcbe388df19f27a7f` | [PNG](evidence/WL-UX-009-2026-08-02-225-files-dark-narrow-08cf147d.png) |

This localizes the remaining `.225` narrow failure to This Node’s route
composition; it is not a seat-wide DRM or viewport failure. Dell was restored
to secure Dark/Default, `require_login_at_boot:true`, active service, and zero
restarts after the batch.

The headless contract was then tightened to include the exact `800` logical
width and require the body anchors `Find a section`, `Workspace`, and `Overview`,
not merely a non-empty primitive list and the title. The farm run passed `1/1`
on `.90` under `wl-ux-009-this-node-body-suite-20260802b`; the Dell title-only
frame is therefore retained as a live runtime/layout finding.

## Dell current-payload Maps proof — 2026-08-02

The synchronized `08cf147d…` payload was routed explicitly to Maps on Dell
`.225`. Dark desktop, Dark narrow at `800` logical pixels, and Light/Largest
were visually inspected and accepted:

| Profile | SHA-256 | Proof |
|---|---|---|
| Dark desktop | `521569b49418cacccc9b631c96c7a47da7af993e6ada9854446e4fa3b3b668aa` | [PNG](evidence/WL-UX-009-2026-08-02-225-maps-dark-desktop-08cf147d.png) |
| Dark narrow | `51154dcaaa1449326ca84153e9c1b23ccf5d60b2c35ab32306c08af32d63e53a` | [PNG](evidence/WL-UX-009-2026-08-02-225-maps-dark-narrow-08cf147d.png) |
| Light/Largest | `436e717c1e0cc64ea50ad19e92cef6549a069de1b4dc535a36dc1f2661a61bf0` | [PNG](evidence/WL-UX-009-2026-08-02-225-maps-light-largest-08cf147d.png) |

The proof drop-in was removed afterward. Dell returned to secure login-at-boot,
Dark/Default, active service, and `NRestarts=0`. These rows close only the Dell
Maps current-payload slice; they do not claim strict linear scanout or overall
WL-UX-009 readiness.

## Dell current-payload Browser boundary proof — 2026-08-02

The synchronized `08cf147d…` payload was routed explicitly to the Browser
boundary on Dell `.225`. Dark desktop, Dark narrow at `800` logical pixels, and
Light/Largest were visually inspected and accepted:

| Profile | SHA-256 | Proof |
|---|---|---|
| Dark desktop | `0636846ea1cc3f395c4582c578ea11f95bb0b1341399f0c832ca82aae0933df5` | [PNG](evidence/WL-UX-009-2026-08-02-225-browser-dark-desktop-08cf147d.png) |
| Dark narrow | `51cf0ad451aa7d4e8d4ef3f5632ea617b31468f4433f0b6f0d0c6644257b2876` | [PNG](evidence/WL-UX-009-2026-08-02-225-browser-dark-narrow-08cf147d.png) |
| Light/Largest | `e164eaa41182f9a5e8d82d67597b30e2acfb7ae3953c6e01445c3233ddafa3bd` | [PNG](evidence/WL-UX-009-2026-08-02-225-browser-light-largest-08cf147d.png) |

All three frames contain only Construct-owned VM selection, transport, and
waiting guidance. Chromium guest pixels remain outside the host evidence
boundary. The proof drop-in was removed and Dell returned to secure
login-at-boot, Dark/Default, active service, and `NRestarts=0`.

## Dell corrected large-text proof — 2026-08-02

The appearance override was corrected to the persisted schema fields
`layout_profile=construct` and `text_scale=largest`. Dell journal output records
`scheme=Light`, `density=Mouse`, `text_scale=Largest`, and `motion=Normal` for
these captures on payload `6538cf7e…`:

| Route / profile | SHA-256 | Proof |
|---|---|---|
| Maps Light/Largest desktop | `46f9dc895d401ff333ce2ce85de5661241262010697c4191b37ff0d726968476` | [PNG](evidence/WL-UX-009-2026-08-02-225-maps-light-largest-6538cf7e.png) |
| Browser boundary Light/Largest desktop | `e164eaa41182f9a5e8d82d67597b30e2acfb7ae3953c6e01445c3233ddafa3bd` | [PNG](evidence/WL-UX-009-2026-08-02-225-browser-light-largest-6538cf7e.png) |
| Phones Light/Largest desktop | `73ade9976db75e335932c0146b9cb8e43ce7c0373028fed173f79ddb446c70d9` | [PNG](evidence/WL-UX-009-2026-08-02-225-phones-light-largest-6538cf7e.png) |

Phones Light/Largest narrow remains rejected: hash
`1b3db9e7edfe012ffc0242fc0cf8e30fa1bfd5ba0733c4b2e42529cbf17ffdfb` contains
only the title and empty-state copy at `800` logical pixels. Dell was restored
to secure login-at-boot, Dark/Default, active service, and `NRestarts=0`.

## Dell Phones narrow resolution — 2026-08-02

The focused Phones contract passed on the farm after re-synchronizing the
current source tree. Release payload
`20955383f21c4e2a4aee6a519be24905a2c65b2605f52ad3f515dcf8f2b9ae5a` was then
deployed to both direct-DRM seats. Dell `.225` renders the complete body at
`800` logical pixels in both required profiles:

| Profile | SHA-256 | Proof |
|---|---|---|
| Dark narrow | `e119653d00bce06a7b8cb07537af6bbd451b0d9e56f44743607220ec5d7ae6be` | [PNG](evidence/WL-UX-009-2026-08-02-225-phones-dark-narrow-20955383.png) |
| Light/Largest narrow | `1b3db9e7edfe012ffc0242fc0cf8e30fa1bfd5ba0733c4b2e42529cbf17ffdfb` | [PNG](evidence/WL-UX-009-2026-08-02-225-phones-light-largest-narrow-20955383.png) |

Both frames include the title, tabs, Features panel, Remote input controls,
and empty-state copy. The earlier title-only interpretation was incorrect;
the hashes are identical to the earlier captures because the complete body was
present in the rendered pixels. Both seats were restored to secure
login-at-boot, Dark/Default, active service, and zero restarts after capture.

## Current-payload Workloads / Infra-as-Code proof — 2026-08-02

The canonical `infra-code` route was exercised on release payload
`20955383f21c4e2a4aee6a519be24905a2c65b2605f52ad3f515dcf8f2b9ae5a`. The
WORKLOADS menu, lifecycle rail, placement card, plan-only state, capacity bars,
and no-selection body were visually inspected. The route is intentionally
bounded to its content lane; the unused right side of the direct-DRM readback
is not a clipping failure.

| Seat / profile | SHA-256 | Proof |
|---|---|---|
| `.225` Dark desktop | `b93998eefdf676b594fd3bec805f6fd485d22804cab55dd987b79c59ba953acd` | [PNG](evidence/WL-UX-009-2026-08-02-225-workloads-dark-desktop-20955383.png) |
| `.225` Dark narrow (`800`) | `b93998eefdf676b594fd3bec805f6fd485d22804cab55dd987b79c59ba953acd` | [PNG](evidence/WL-UX-009-2026-08-02-225-workloads-dark-narrow-20955383.png) |
| `.225` Light/Largest desktop | `460c5f9c8d56e2319dbd14928c3dc5beeee9fc2ba932b6c5f0099b6a52d83de9` | [PNG](evidence/WL-UX-009-2026-08-02-225-workloads-light-largest-desktop-20955383.png) |
| `.225` Light/Largest narrow (`800`) | `460c5f9c8d56e2319dbd14928c3dc5beeee9fc2ba932b6c5f0099b6a52d83de9` | [PNG](evidence/WL-UX-009-2026-08-02-225-workloads-light-largest-20955383.png) |
| `.138` Dark desktop | `4c20bd02263bda16b36bde1712384312fc90ff24530d75650bffe007b93b3df9` | [PNG](evidence/WL-UX-009-2026-08-02-138-workloads-dark-desktop-20955383.png) |
| `.138` Dark narrow (`800`) | `4c20bd02263bda16b36bde1712384312fc90ff24530d75650bffe007b93b3df9` | [PNG](evidence/WL-UX-009-2026-08-02-138-workloads-dark-narrow-20955383.png) |
| `.138` Light/Largest desktop | `0980867f8a67d781747ac0442bcd915210b20076a94cffc85d3b7365d3531c62` | [PNG](evidence/WL-UX-009-2026-08-02-138-workloads-light-largest-desktop-20955383.png) |
| `.138` Light/Largest narrow (`800`) | `c5e824578e585bc12433b02c7c655380987dad01adfbd43c4a4cd11b95228130` | [PNG](evidence/WL-UX-009-2026-08-02-138-workloads-light-largest-narrow-20955383.png) |

The first `.138` Light/Largest desktop retry was discarded because it captured
the secure boot curtain; it is intentionally absent from the evidence set.
Both seats were restored to secure login-at-boot, Dark/Default, active service,
and zero restarts. This is visual/state proof only and does not claim a live
workload backend or VDI guest readiness.

## Current-payload Browser host/guest boundary proof — 2026-08-02

The explicit `browser` route was exercised on release payload
`20955383f21c4e2a4aee6a519be24905a2c65b2605f52ad3f515dcf8f2b9ae5a`. Every
inspected frame contains only Construct-owned VM selection, transport, waiting,
and unavailable guidance. Guest Chromium pixels remain outside the host
evidence boundary.

| Seat / profile | SHA-256 | Proof |
|---|---|---|
| `.225` Dark desktop | `51cf0ad451aa7d4e8d4ef3f5632ea617b31468f4433f0b6f0d0c6644257b2876` | [PNG](evidence/WL-UX-009-2026-08-02-225-browser-dark-desktop-20955383.png) |
| `.225` Dark narrow (`800`) | `51cf0ad451aa7d4e8d4ef3f5632ea617b31468f4433f0b6f0d0c6644257b2876` | [PNG](evidence/WL-UX-009-2026-08-02-225-browser-dark-narrow-20955383.png) |
| `.225` Light/Largest desktop | `15c02b1d9e902cfbe06857234fe6a0e12cfd3c11a554c7601296f5dd29c35d11` | [PNG](evidence/WL-UX-009-2026-08-02-225-browser-light-largest-desktop-20955383.png) |
| `.225` Light/Largest narrow (`800`) | `15c02b1d9e902cfbe06857234fe6a0e12cfd3c11a554c7601296f5dd29c35d11` | [PNG](evidence/WL-UX-009-2026-08-02-225-browser-light-largest-narrow-20955383.png) |
| `.138` Dark desktop | `386148203905ba62fb655224a971366f0dee7c5de9543185b1b2557b72564f74` | [PNG](evidence/WL-UX-009-2026-08-02-138-browser-dark-desktop-20955383.png) |
| `.138` Dark narrow (`800`) | `386148203905ba62fb655224a971366f0dee7c5de9543185b1b2557b72564f74` | [PNG](evidence/WL-UX-009-2026-08-02-138-browser-dark-narrow-20955383.png) |
| `.138` Light/Largest desktop | `5cb088185d457e003ec736b0c2c1ce099476ce56cfa30ca940636c14666e9275` | [PNG](evidence/WL-UX-009-2026-08-02-138-browser-light-largest-desktop-20955383.png) |
| `.138` Light/Largest narrow (`800`) | `5cb088185d457e003ec736b0c2c1ce099476ce56cfa30ca940636c14666e9275` | [PNG](evidence/WL-UX-009-2026-08-02-138-browser-light-largest-narrow-20955383.png) |

Both seats were restored to secure login-at-boot, Dark/Default, active service,
and zero restarts. This evidence validates the Construct boundary only; it
does not claim that the VDI guest or Chromium workload is attached or ready.

## Current-payload Music / Airsonic state proof — 2026-08-02

The explicit `music` route was exercised on release payload
`20955383f21c4e2a4aee6a519be24905a2c65b2605f52ad3f515dcf8f2b9ae5a`. The
shared MUSIC menu/status chrome and the honest unavailable state are readable
in every frame. The captures show missing Airsonic credentials; they do not
claim connectivity to Subsonic/Airsonic at `172.20.0.2`.

| Seat / profile | SHA-256 | Proof |
|---|---|---|
| `.225` Dark desktop | `8f6afee1292f088f9cb0620ac4764f4703290e09d1bddad29ae864b1cc30c188` | [PNG](evidence/WL-UX-009-2026-08-02-225-music-dark-desktop-20955383.png) |
| `.225` Dark narrow (`800`) | `8f6afee1292f088f9cb0620ac4764f4703290e09d1bddad29ae864b1cc30c188` | [PNG](evidence/WL-UX-009-2026-08-02-225-music-dark-narrow-20955383.png) |
| `.225` Light/Largest desktop | `159f13ecd30c8b20a785522ec78a39c18d423192e140a9ea55b3218016600340` | [PNG](evidence/WL-UX-009-2026-08-02-225-music-light-largest-desktop-20955383.png) |
| `.225` Light/Largest narrow (`800`) | `159f13ecd30c8b20a785522ec78a39c18d423192e140a9ea55b3218016600340` | [PNG](evidence/WL-UX-009-2026-08-02-225-music-light-largest-narrow-20955383.png) |
| `.138` Dark desktop | `60e2cfef1a6b4edc4492192b08b68dc839d4fb21bb272b58ea2f9d03e0e95929` | [PNG](evidence/WL-UX-009-2026-08-02-138-music-dark-desktop-20955383.png) |
| `.138` Dark narrow (`800`) | `60e2cfef1a6b4edc4492192b08b68dc839d4fb21bb272b58ea2f9d03e0e95929` | [PNG](evidence/WL-UX-009-2026-08-02-138-music-dark-narrow-20955383.png) |
| `.138` Light/Largest desktop | `fc2b03be13a61eaa58b02330597a223bfa5d0076b78c8eccfb2c1c41f407eeac` | [PNG](evidence/WL-UX-009-2026-08-02-138-music-light-largest-desktop-20955383.png) |
| `.138` Light/Largest narrow (`800`) | `fc2b03be13a61eaa58b02330597a223bfa5d0076b78c8eccfb2c1c41f407eeac` | [PNG](evidence/WL-UX-009-2026-08-02-138-music-light-largest-narrow-20955383.png) |

Both seats were restored to secure login-at-boot, Dark/Default, active service,
and zero restarts. This is visual/state evidence only; server discovery,
credentials, and playback readiness remain unproven.

## Current-payload Media / Sources proof — 2026-08-02

The explicit `media` route was exercised on release payload
`20955383f21c4e2a4aee6a519be24905a2c65b2605f52ad3f515dcf8f2b9ae5a`. The
corrected `MEDIA` title and menu, source tabs, local-capture controls, Jellyfin
fields, and honest no-source state were visually inspected. The earlier narrow
`ME` title clipping does not reproduce. No live media or Jellyfin backend is
claimed.

| Seat / profile | SHA-256 | Proof |
|---|---|---|
| `.225` Dark desktop | `4f0447b80294a22b9229176199d60367ca3645332a309749ed9725044522ef06` | [PNG](evidence/WL-UX-009-2026-08-02-225-media-dark-desktop-20955383.png) |
| `.225` Dark narrow (`800`) | `4f0447b80294a22b9229176199d60367ca3645332a309749ed9725044522ef06` | [PNG](evidence/WL-UX-009-2026-08-02-225-media-dark-narrow-20955383.png) |
| `.225` Light/Largest desktop | `73031131c61c0917be85056db418676538ebde085aca349f27ec78c8cd3f0220` | [PNG](evidence/WL-UX-009-2026-08-02-225-media-light-largest-desktop-20955383.png) |
| `.225` Light/Largest narrow (`800`) | `73031131c61c0917be85056db418676538ebde085aca349f27ec78c8cd3f0220` | [PNG](evidence/WL-UX-009-2026-08-02-225-media-light-largest-narrow-20955383.png) |
| `.138` Dark desktop | `8b97368ce43ca4352fb3860425b70f35480c12db85fbe0579cd91eb651f3526e` | [PNG](evidence/WL-UX-009-2026-08-02-138-media-dark-desktop-20955383.png) |
| `.138` Dark narrow (`800`) | `d7f947748efcb0c6379c17a20dce043a608a84e80e382d068cb4ec2b371b6866` | [PNG](evidence/WL-UX-009-2026-08-02-138-media-dark-narrow-20955383.png) |
| `.138` Light/Largest desktop | `c5ea1e50c2072b9b6d10e4bbbcc69d8b26a00f042cd37e3b8ac97c489a297841` | [PNG](evidence/WL-UX-009-2026-08-02-138-media-light-largest-desktop-20955383.png) |
| `.138` Light/Largest narrow (`800`) | `934b0af528679686468df171a3fafe0b4cb440010ca79f34f792c112860e79af` | [PNG](evidence/WL-UX-009-2026-08-02-138-media-light-largest-narrow-20955383.png) |

Both seats were restored to secure login-at-boot, Dark/Default, active service,
and zero restarts. This is visual/state evidence only and does not claim a
live media backend or Jellyfin readiness.

## Current-payload Terminal matrix — 2026-08-02

The shared Terminal route was exercised on exact release payload
`20955383f21c4e2a4aee6a519be24905a2c65b2605f52ad3f515dcf8f2b9ae5a` on both
direct-DRM seats. The complete `TERMINAL` identity, menu rail, session/tab
controls, terminal body, and footer chrome were visually inspected at desktop,
`800` logical-pixel narrow, and Light/Largest profiles. Original-resolution
title crops were used for the narrow captures because full-frame display scaling
can make the title appear truncated in an overview.

| Seat / profile | SHA-256 | Proof |
|---|---|---|
| `.225` Dark desktop | `91cb0b70a50e0ee4d6fc69face75ff4e6fc17ac2af7f9b80b05e3cd33bf7bcc0` | [PNG](evidence/WL-UX-009-2026-08-02-225-terminal-dark-desktop-20955383.png) |
| `.225` Dark narrow (`800`) | `aec655d875897f85d29a7724224f54a0b9e9a605a149e594b37cdee5b1cb0f57` | [PNG](evidence/WL-UX-009-2026-08-02-225-terminal-dark-narrow-20955383.png) |
| `.225` Light/Largest desktop | `e8bbba081b6fabbcc7e332af04256526dacdef1feacd03368729a3aa9153465f` | [PNG](evidence/WL-UX-009-2026-08-02-225-terminal-light-largest-desktop-20955383.png) |
| `.225` Light/Largest narrow (`800`) | `0e11837b6293a0838c8010a291db9397dc6cf56dc5953f174fdc1ef05ea65d9f` | [PNG](evidence/WL-UX-009-2026-08-02-225-terminal-light-largest-narrow-20955383.png) |
| `.138` Dark desktop | `4d4e7c35370cd8ce0834972abbdb0a1662aea5879776d5b33a0a34c03e58e39a` | [PNG](evidence/WL-UX-009-2026-08-02-138-terminal-dark-desktop-20955383.png) |
| `.138` Dark narrow (`800`) | `e52edcb34548ec702952ab93244ed26c7a34f5fc8e8c2cbc26470b08abc3d7f8` | [PNG](evidence/WL-UX-009-2026-08-02-138-terminal-dark-narrow-20955383.png) |
| `.138` Light/Largest desktop | `85a95533cd285076ece723df9a59404d3ed00effb2077daf99c9935ac2e933f9` | [PNG](evidence/WL-UX-009-2026-08-02-138-terminal-light-largest-desktop-20955383.png) |
| `.138` Light/Largest narrow (`800`) | `f846713428541d617a5770f1b046ff02c975e15ba11a1d5d41b779e09321300d` | [PNG](evidence/WL-UX-009-2026-08-02-138-terminal-light-largest-narrow-20955383.png) |

Both seats were restored to secure login-at-boot, Dark/Default, active service,
and zero restarts. This closes the current-payload Terminal render slice only;
it does not claim strict linear scanout or overall WL-UX-009 readiness.

## Current-payload This Node matrix — 2026-08-02

The unified This Node route was exercised with the exact release payload
`20955383f21c4e2a4aee6a519be24905a2c65b2605f52ad3f515dcf8f2b9ae5a` on both
direct-DRM seats. The title, search field, Status, Devices & Peripherals,
System, Inventory/Actions, Overview, and System/Storage/About controls were
visually inspected. The Light/Largest views use the intentional vertical
scroll boundary for the complete node catalog; the visible content does not
overlap the taskbar.

| Seat / profile | SHA-256 | Proof |
|---|---|---|
| `.225` Dark desktop | `3c2fa3f2f7b57e683f18ed812df15f51939478663f631e2d74b0209435602240` | [PNG](evidence/WL-UX-009-2026-08-02-225-this-node-dark-desktop-20955383.png) |
| `.225` Dark narrow (`800`) | `30cebe431e5c80513c0fc599dc9e0f9b8ea101e513fe9f625cb3b529540a5af5` | [PNG](evidence/WL-UX-009-2026-08-02-225-this-node-dark-narrow-20955383.png) |
| `.225` Light/Largest desktop | `6b55b6104a29c5c6691d129e726ae4350dd5b2474fcfacfd9471b744b419f7e3` | [PNG](evidence/WL-UX-009-2026-08-02-225-this-node-light-largest-desktop-20955383.png) |
| `.225` Light/Largest narrow (`800`) | `5cca3b7269c2ae30047f9e53796a44e4f324a645658f7a662f073fea0fa6de87` | [PNG](evidence/WL-UX-009-2026-08-02-225-this-node-light-largest-narrow-20955383.png) |
| `.138` Dark desktop | `5d8f4e9758fc26bb0d50244355d6efb3235981f3a85d0af64f93b7dcefc1a7c2` | [PNG](evidence/WL-UX-009-2026-08-02-138-this-node-dark-desktop-20955383.png) |
| `.138` Dark narrow (`800`) | `78f66e3f3f3e257badf02a6bbb78949c892aa7e875aa9955322bcf7b6cfe8666` | [PNG](evidence/WL-UX-009-2026-08-02-138-this-node-dark-narrow-20955383.png) |
| `.138` Light/Largest desktop | `a42e043ab6a3663493a3a1717beb5d1c81455e6c8f3e9068a031da6db0f680c7` | [PNG](evidence/WL-UX-009-2026-08-02-138-this-node-light-largest-desktop-20955383.png) |
| `.138` Light/Largest narrow (`800`) | `5f2de9cf8ede9c0305fd71e3794255f112d201fcc80b324de070006f3a5ffcbb` | [PNG](evidence/WL-UX-009-2026-08-02-138-this-node-light-largest-narrow-20955383.png) |

The first `.225` attempt was discarded because a stale duplicate proof
environment routed it to Terminal; the clean rerun above explicitly verified
`MDE_DRM_PROOF_SURFACE=this-node`. Both seats were restored to secure
login-at-boot, Dark/Default, active service, and zero restarts. This closes the
current-payload This Node visual slice only; it does not claim strict linear
scanout or overall WL-UX-009 readiness.

## Car palette candidate / routing rejection — 2026-08-02

The Car palette implementation was farm-tested on BigBoy: the focused
`mde-shell-egui` Car suite passed 15/15, and the feature-correct release build
used `drm,live-vdi,media-mpv`. The candidate started with `drm:true` on both
direct seats. The `.138` readback
`evidence/WL-UX-009-2026-08-02-138-car-light-largest-narrow-palette.png` has
SHA-256 `0f28ec07398382c237d5ed9879baba4c52dceba36d36be3cfdb9dcf6738b379e`,
but is rejected: it is the shell clock/dashboard surface, not Auto Mode. The
`.225` attempt produced no readback. Both seats were restored active with
secure login-at-boot; `.138` is at zero restarts and `.225` reports one restart
from the rejected proof attempt. No Car Light/Dark live acceptance is claimed.

## Files Airsonic advertisement proof / route rejection — 2026-08-02

The live service directory was inspected on both direct-DRM seats before the
candidate capture. `.138` reported an Airsonic record at `10.42.0.7:4040`
with provenance `probe` and health `up`; Dell reported no confirmed Airsonic
record and its visible records were stale published entries. The
feature-correct candidate shell payload `4f60e1bad5747454dcd071c68ee9ffe567acd065113f11bf30451bcb9c8f49f3`
was installed on `.138` only, with an explicit `MDE_DRM_PROOF_SURFACE=files`
override. The readback
`evidence/WL-UX-009-2026-08-02-138-files-airsonic-4f60e1ba.png` has SHA-256
`3ae546af92f23c9e939355a767d0af715af6e936de56a09f4d74af534232d612`, but it
shows the Music surface rather than Files and is rejected as a route failure.
The candidate was removed, the prior `20955383…` payload restored, and the
proof override removed; `.138` returned active with `NRestarts=0`. This proves
the service-advertisement input, not the Files live-render acceptance or
WL-UX-009 readiness.

The route-hardening candidate `7ecbee91fa26a69963739d491ff19c4cb0f78819fe6557698a82bb57d25f2d14`
was recaptured after explicitly restoring `.138` to `Dark / Construct /
Default / Normal`. The resulting readback
`evidence/WL-UX-009-2026-08-02-138-files-airsonic-construct-7ecbee91.png`
has SHA-256 `a5faa3191cfbc847d779e99ef57d782ce1cd85abde358c8b64132f5ce22d0443`
and still shows the Auto/clock surface rather than Files. It is rejected. The
candidate was removed again and `.138` returned to the prior payload with the
proof drop-in removed and `NRestarts=0`.

## Files Airsonic advertisement proof / route accepted — 2026-08-02

The synchronized release candidate `b888b0d163de8369b554569d5a75f3f17f257d8581f1e2558d14a3d479435f0c`
was built on BigBoy after the focused route suite passed 16/16 and the This
Node suite passed 37/37. On direct-DRM `.138`, the approved temporary
`require_login_at_boot:false` proof fixture was used, with
`MDE_DRM_PROOF_SURFACE=files` and the proof-only EGL readback path. The
visually inspected readback
`evidence/WL-UX-009-2026-08-02-138-files-airsonic-unlocked-b888b0d1.png`
has SHA-256
`64be0f4af1cdf26ecfc66e96172cf76a6234b64caed7b842fe2fec9c31e3b329`.

The frame is accepted for this slice: it shows the shared `FILES` chrome, the
complete ten-action NODE ACTIONS inventory, one reachable mesh peer, and the
truthful `Airsonic upload` / `Music-owned` action. This does not claim that
Dell has a live Airsonic provider or that the Subsonic server at `172.20.0.2`
is reachable. The proof fixture, candidate binary, and drop-in were removed;
`.138` was restored to payload `20955383…`, secure
`require_login_at_boot:true`, Dark/Construct/Default/Normal, active service,
and zero restarts. This closes only the Files/Airsonic route proof; it does not
close the remaining WL-UX-009 matrix, strict linear scanout, Dell adoption, or
overall readiness.

## Car Light palette resolution / native direct-DRM proof — 2026-08-02

Candidate `e30f36cd562f729f91620ef3842827190bbf3b055bcd8126072630d1dedcd0ee`
was built on BigBoy after the focused Car suite passed 14/14. It was
temporarily installed on both direct-DRM seats with the proof-only
`require_login_at_boot:false` fixture, `MDE_DRM_PROOF_SURFACE=car`, and
Light/Car/Largest appearance settings. The first `.138` frame was rejected as
the boot curtain; after correcting the temporary fixture to valid JSON, it was
restarted and recaptured.

| Seat / profile | Native readback | SHA-256 | Proof |
|---|---:|---|---|
| `.138` Car Light/Largest | 1920x1080 | `79355ca02ed2a8086d0f9cd14dcfa411233e1aad46f8496ab0b885f9c781332b` | [PNG](evidence/WL-UX-009-2026-08-02-138-car-light-largest-narrow.png) |
| `.225` Car Light/Largest | 1366x768 | `39361bc641021fd0ed4e5ec7c4dd92b86343d944c5ce2bab3432c2feffa4dcbb` | [PNG](evidence/WL-UX-009-2026-08-02-225-car-light-largest-narrow.png) |

Visual inspection accepts both native frames: Auto Mode is present, the
surface ground and cards use the Light palette, AutoSync3 vehicle accents are
retained, and the large-text layout is free of clipping or overlap. Both
seats were restored to payload `20955383…`, secure login-at-boot, Dark/
Construct/Default/Normal, active service, zero restarts, and no proof drop-in.
The proof-only logical-width override intentionally leaves the PNG at each
seat's native physical dimensions; the content is bounded to the requested
800 logical-pixel viewport (with unused scanout to the right). This closes the
Car Light/Largest narrow palette cell. Strict linear scanout, the remaining
route/profile matrix, and overall WL-UX-009 readiness remain open.
## Dell Terminal current-candidate recapture — 2026-08-02

Candidate `e30f36cd562f729f91620ef3842827190bbf3b055bcd8126072630d1dedcd0ee`
was temporarily installed on Dell `.225` and explicitly routed to Terminal.
All four required profiles were visually inspected:

| Profile | Native readback | SHA-256 | Proof |
|---|---:|---|---|
| Dark desktop | 1366x768 | `91cb0b70a50e0ee4d6fc69face75ff4e6fc17ac2af7f9b80b05e3cd33bf7bcc0` | [PNG](evidence/WL-UX-009-2026-08-02-225-terminal-dark-desktop-e30f36cd.png) |
| Dark narrow (`800` logical) | 1366x768 | `aec655d875897f85d29a7724224f54a0b9e9a605a149e594b37cdee5b1cb0f57` | [PNG](evidence/WL-UX-009-2026-08-02-225-terminal-dark-narrow-e30f36cd.png) |
| Light desktop | 1366x768 | `e8bbba081b6fabbcc7e332af04256526dacdef1feacd03368729a3aa9153465f` | [PNG](evidence/WL-UX-009-2026-08-02-225-terminal-light-desktop-e30f36cd.png) |
| Light/Largest narrow (`800` logical) | 1366x768 | `0e11837b6293a0838c8010a291db9397dc6cf56dc5953f174fdc1ef05ea65d9f` | [PNG](evidence/WL-UX-009-2026-08-02-225-terminal-light-largest-narrow-e30f36cd.png) |

The complete `TERMINAL` identity, menu/session controls, taskbar contrast,
and bounded terminal body are visible in all four frames. This supersedes the
earlier title-only interpretation of the Dell narrow capture. Dell was
restored to payload `20955383…`, secure login-at-boot, Dark/Construct/Default/
Normal, active service, zero restarts, and no proof drop-in. This closes the
Dell Terminal visual slice only; VDI guest readiness, strict linear scanout,
the remaining route/profile matrix, and WL-UX-009 readiness remain open.
## Dell Editor current-candidate recapture — 2026-08-02

Candidate `e30f36cd562f729f91620ef3842827190bbf3b055bcd8126072630d1dedcd0ee`
was temporarily installed on Dell `.225` and explicitly routed to Editor.
The route presents the Mesh Teams editor host chrome with the editor surface;
no guest or VDI pixels are claimed.

| Profile | Native readback | SHA-256 | Proof |
|---|---:|---|---|
| Dark desktop | 1366x768 | `99a8e5683c930d591ac4b19414336c494930ff6a7216b1fced6029570596b974` | [PNG](evidence/WL-UX-009-2026-08-02-225-editor-dark-desktop-e30f36cd.png) |
| Dark narrow (`800` logical) | 1366x768 | `5fd402958c280d5acb046fde62c51eee3038806a2057c1ef9435551cf797099d` | [PNG](evidence/WL-UX-009-2026-08-02-225-editor-dark-narrow-e30f36cd.png) |
| Light desktop | 1366x768 | `d5775cbb11ed7aea682f15114acb29ab6149c54a77f3664cc08548013d406c90` | [PNG](evidence/WL-UX-009-2026-08-02-225-editor-light-desktop-e30f36cd.png) |
| Light/Largest narrow (`800` logical) | 1366x768 | `998abc13504b7ab865e7e1afbe67a897271f7525de7c13689b71fefe6c45ab05` | [PNG](evidence/WL-UX-009-2026-08-02-225-editor-light-largest-narrow-e30f36cd.png) |

The four frames show the direct-entry sidebars collapsed, complete editor
toolbar/menu reachability, bounded document/status/details geometry, and
taskbar contrast without clipping or overlap. Dell was restored to payload
`20955383…`, secure login-at-boot, Dark/Construct/Default/Normal, active
service, zero restarts, and no proof drop-in. This closes the Dell Editor
visual slice only; the remaining route/profile matrix, guest VDI readiness,
strict linear scanout, and WL-UX-009 readiness remain open.

## Dell Files / Mesh Teams recapture and Mesh Teams Light contrast resolution — 2026-08-02

Candidate `e30f36cd562f729f91620ef3842827190bbf3b055bcd8126072630d1dedcd0ee`
was temporarily installed on Dell `.225` for the Files and Mesh Teams route
batch. Files Dark desktop, Dark narrow, Light desktop, and Light/Largest narrow
were visually accepted; the four Mesh Teams Dark frames were also accepted.
The first Mesh Teams Light frames were rejected because they were captured
during the shared page crossfade and appeared washed out. The follow-up source
fix resolves Mesh Teams Activity and frame-owned `TEXT`, `TEXT_DIM`, and
`TEXT_STRONG` tokens through the live shared palette, and the focused BigBoy
farm test passed `mde-collab-egui` 130/130, including an explicit Light-mode
render assertion for Activity and Mesh Teams rail text.

The exact contrast-fix candidate was
`ae51c12447c342aa9457e7db148cdb046595eeb533463a731b712cd1b1bb0236`.
After allowing the normal motion settle interval, the direct-DRM EGL readback
frames below were visually inspected and accepted:

| Seat / profile | Native readback | SHA-256 | Proof |
|---|---:|---|---|
| Dell Mesh Teams Light desktop | 1366x768 | `e2af1a7924317727a074c1ce948ab60237c53e6b3b0789981651cdb192cad1d7` | [PNG](evidence/WL-UX-009-2026-08-02-225-mesh-teams-light-desktop-ae51c1244-long.png) |
| Dell Mesh Teams Light/Largest narrow (`800` logical) | 1366x768 | `da75691086e79af9a4ebcbae1215d0a7c69567999f58ea187aeb501999694a18` | [PNG](evidence/WL-UX-009-2026-08-02-225-mesh-teams-light-largest-narrow-ae51c1244.png) |

The accepted frames show readable Activity/filter/rail copy on the Light
surface, clear status and alert rows, and no clipping or overlap at the large
text narrow width. The earlier 5-second transition frames are retained only as
rejected diagnostic evidence; they are not counted as passing cells. Dell was
restored to payload `20955383…`, secure login-at-boot, Dark/Construct/Default/
Normal, active service, zero restarts, and no proof drop-in. This closes the
Dell Files/Mesh Teams visual slice and Mesh Teams Light contrast finding only;
VDI guest readiness, strict linear scanout, the remaining route/profile matrix,
and overall WL-UX-009 readiness remain open.

## Editor exact-current-candidate matrix — 2026-08-02

Exact candidate `ae51c12447c342aa9457e7db148cdb046595eeb533463a731b712cd1b1bb0236`
was temporarily installed on `.138` and Dell `.225`, explicitly routed to
Editor, and captured with proof-only EGL readback. All four profiles were
visually inspected on both seats:

| Seat / profile | SHA-256 | Proof |
|---|---|---|
| `.138` Dark desktop | `07e86399d8693f28321e7de45da047c03ff14506625ad91638f92836599bb9e6` | [PNG](evidence/WL-UX-009-2026-08-02-138-editor-dark-desktop-ae51c1244.png) |
| `.138` Dark narrow (`800` logical) | `32b3b66c598f7c2f76a0d43b1593f8869568fbba4b209618ed6e77170f6e2536` | [PNG](evidence/WL-UX-009-2026-08-02-138-editor-dark-narrow-ae51c1244.png) |
| `.138` Light desktop | `259806e749e6674f6e931271d0437ac04a067e38c1deece90360a2b72d7ebba9` | [PNG](evidence/WL-UX-009-2026-08-02-138-editor-light-desktop-ae51c1244.png) |
| `.138` Light/Largest narrow (`800` logical) | `c34a6b6dd50064f7f0d233aaad84e13ded1b715bbf59405c094c15a968fa28c4` | [PNG](evidence/WL-UX-009-2026-08-02-138-editor-light-largest-narrow-ae51c1244.png) |
| Dell `.225` Dark desktop | `c5a5701c4da7016d87d5594d2155e560d5c0f3e8868d75b42eb2becc2be59452` | [PNG](evidence/WL-UX-009-2026-08-02-225-editor-dark-desktop-ae51c1244.png) |
| Dell `.225` Dark narrow (`800` logical) | `567504f274e3350a82682e1978259c9336a5fef9dcaa178ad27c6f6cd263f93f` | [PNG](evidence/WL-UX-009-2026-08-02-225-editor-dark-narrow-ae51c1244.png) |
| Dell `.225` Light desktop | `0b4527aefdbafe3869bfb7ceeab84c545ed81e8072ebb48c2f5c75533debe57e` | [PNG](evidence/WL-UX-009-2026-08-02-225-editor-light-desktop-ae51c1244.png) |
| Dell `.225` Light/Largest narrow (`800` logical) | `113f368a45d423c632474c60e824ffcdf795c943b4ab104cdfaeba70e6bbe9b5` | [PNG](evidence/WL-UX-009-2026-08-02-225-editor-light-largest-narrow-ae51c1244.png) |

The direct-entry sidebars are collapsed; the shared Editor identity,
menu/toolbar controls, document body, status row, details rail, and taskbar
remain reachable and bounded in every accepted frame. Both seats were restored
to payload `20955383…`, secure login-at-boot, Dark/Construct/Default/Normal,
active service, zero restarts, and no proof drop-in. This closes only the
exact-candidate Editor matrix slice; strict linear scanout, VDI guest
readiness, the remaining route/profile matrix, and overall WL-UX-009 readiness
remain open.

## VDI guest endpoint audit — 2026-08-02

A fresh read-only endpoint probe checked the enrolled validation seats
`.15`, `.138`, `.145`, and Dell `.225` for the approved RDP/VNC/VDI service
ports (3389, 5900/5901, 8443, and Sunshine control ports 47984/47989). No
endpoint was open on any seat. This corroborates the existing boundary
evidence: focused VDI may own guest pixels when a live guest session exists,
Browser VM owns Chromium pixels and chrome, and Construct owns only the host
connection/unavailable/diagnostic states. No guest framebuffer, guest input,
or VDI readiness claim is made.

## Car Light/Largest narrow current-candidate resolution — 2026-08-02

Exact candidate `ae51c12447c342aa9457e7db148cdb046595eeb533463a731b712cd1b1bb0236`
was temporarily installed on both direct-DRM seats with explicit `car`
routing, the proof-only unlock fixture, and `800` logical-pixel width. The
AutoHome surface was allowed to settle before capture.

| Seat / profile | Native readback | SHA-256 | Proof |
|---|---:|---|---|
| `.138` Car Light/Largest narrow | 1920x1080 | `667d424eaa003b0de5a758995459bc7aefe2828c67c5a3844b0cc7fb4a3aca26` | [PNG](evidence/WL-UX-009-2026-08-02-138-car-light-largest-narrow-ae51c1244.png) |
| Dell `.225` Car Light/Largest narrow | 1366x768 | `0146a43f36e26be8174998a0c61d9d1bc38a7c3aa05ccd8231349d6a89513566` | [PNG](evidence/WL-UX-009-2026-08-02-225-car-light-largest-narrow-ae51c1244.png) |

Both frames were visually accepted: the Auto Mode cockpit is complete, the
persisted Light surface palette is honored, AutoSync3 vehicle accents remain
intact, and large-text cards have no overlap or clipping. Both seats were
restored to payload `20955383…`, secure login-at-boot, Dark/Construct/Default/
Normal, active service, zero restarts, and no proof drop-in. This closes only
the current-candidate Car Light/Largest narrow cell; strict linear scanout,
VDI guest readiness, the remaining matrix, and overall WL-UX-009 readiness
remain open.

## Files exact-current-candidate matrix — 2026-08-02

Exact candidate `ae51c12447c342aa9457e7db148cdb046595eeb533463a731b712cd1b1bb0236`
was temporarily installed on `.138` and Dell `.225`, explicitly routed to
Files, and captured with the proof-only EGL readback path. All four profiles
were visually inspected on both seats:

| Seat / profile | SHA-256 | Proof |
|---|---|---|
| `.138` Dark desktop | `837d3e74668a9dd3fed6062ed985b216878a21092ead6ccae10064532983639e` | [PNG](evidence/WL-UX-009-2026-08-02-138-files-dark-desktop-ae51c1244.png) |
| `.138` Dark narrow (`800` logical) | `99cc13bcf7913bbce48e44fb83015657153486ae084f116aa5a57673527e72d3` | [PNG](evidence/WL-UX-009-2026-08-02-138-files-dark-narrow-ae51c1244.png) |
| `.138` Light desktop | `a855ae16495343f57cb88de304ed6dc97e165b456f1d1ef9e4863b347829d407` | [PNG](evidence/WL-UX-009-2026-08-02-138-files-light-desktop-ae51c1244.png) |
| `.138` Light/Largest narrow (`800` logical) | `b24810739a928fdade2452ddd7714c680be2510d8b6d8727fca47a22b2819eef` | [PNG](evidence/WL-UX-009-2026-08-02-138-files-light-largest-narrow-ae51c1244.png) |
| Dell `.225` Dark desktop | `856bc5559498047bdf210e3802fc83a1bb1e2fa5363f2b8dbd71b05ced90b4b1` | [PNG](evidence/WL-UX-009-2026-08-02-225-files-dark-desktop-ae51c1244.png) |
| Dell `.225` Dark narrow (`800` logical) | `441d86440bf3e2ee6eaf8db23c7d3e364b7fa35629c40954a9edd409a1065d33` | [PNG](evidence/WL-UX-009-2026-08-02-225-files-dark-narrow-ae51c1244.png) |
| Dell `.225` Light desktop | `7060edd6f007e54eee906aa24a6ceb0d7bac8fc241e901faa4f4e9b3b254f096` | [PNG](evidence/WL-UX-009-2026-08-02-225-files-light-desktop-ae51c1244.png) |
| Dell `.225` Light/Largest narrow (`800` logical) | `9d781bd9abbc416d5a2808f2477d7e585aff2d95d722a4216cece23306f2987b` | [PNG](evidence/WL-UX-009-2026-08-02-225-files-light-largest-narrow-ae51c1244.png) |

The complete ten-action node inventory, peer/status lanes, file list, preview
boundary, and transfer/status strip remain readable and bounded in every
accepted frame. Both seats were restored to payload `20955383…`, secure
login-at-boot, Dark/Construct/Default/Normal, active service, zero restarts,
and no proof drop-in. This closes the exact-candidate Files matrix slice only;
strict linear scanout, VDI guest readiness, the remaining route/profile matrix,
and overall WL-UX-009 readiness remain open.

## Maps Light/Largest narrow finding — 2026-08-02

Exact candidate `ae51c12447c342aa9457e7db148cdb046595eeb533463a731b712cd1b1bb0236`
was temporarily installed on `.138` and Dell `.225` with explicit Maps
routing. The eight direct-DRM proof frames were captured and inspected. Seven
cells are visually acceptable, but Dell `.225` Light/Largest narrow is not:

| Seat / profile | SHA-256 | Proof | Result |
|---|---|---|---|
| Dell `.225` Light/Largest narrow (`800` logical) | `cc3c01404c1d96ace43d2cc68cb2fa843339a58b31f8144b545750497f3ddd96` | [PNG](evidence/WL-UX-009-2026-08-02-225-maps-light-largest-narrow-ae51c1244.png) | **Rejected** |

Native-detail inspection shows the lower red alert pill is cut off at the
application viewport boundary immediately above the taskbar. This is a real
large-text narrow-layout clipping finding, not a capture-transition artifact.
The remaining seven Maps cells are retained as inspected evidence, but the
Maps matrix is not closed. Both seats were restored to payload `20955383…`,
secure login-at-boot, Dark/Construct/Default/Normal, active service, zero
restarts, and no proof drop-in. Strict linear scanout, VDI guest readiness,
this Maps finding, the remaining route/profile matrix, and overall WL-UX-009
readiness remain open.

## Maps Light/Largest narrow remediation — 2026-08-02

The Maps HUD was corrected to reserve the bottom-safe viewport lane for the
large-text alert stack. In the combined no-fix/offline-blocked state, the
authoritative alert row replaces the redundant no-data card so the two status
messages do not compete for the same pixels.

| Seat / profile | Candidate SHA-256 | Native readback SHA-256 | Proof |
|---|---|---|---|
| Dell `.225` Light/Largest narrow (`800` logical) | `b75e395a2fba1368c5349b87e851e92a4d1c3a1597e633641344798fdacd980f` | `a52e044e00e6ba7cc4e305d7b97b8263e45f851b42817431a46288130c2f3b1a` | [PNG](evidence/WL-UX-009-2026-08-02-225-maps-light-largest-narrow-alert-fix-b75e395a.png) |

Native inspection accepts the corrected frame: both pills are fully visible,
readable, and separated from the taskbar, with no redundant card overlap.
Focused Maps tests pass 274/274. Dell was restored to payload
`20955383…`, secure login-at-boot, Dark/Construct/Default/Normal, active
service, zero restarts, and no proof drop-in. This closes the recorded Maps
clipping finding only; strict linear scanout, VDI guest readiness, the
remaining route/profile matrix, and overall WL-UX-009 readiness remain open.

## Current-payload Music and Media route validation — 2026-08-02

Candidate `2f32f935c92a4cf84f926221a093a3666638fee9063ec4f9a8dc8ef1f686f628`
was built on BigBoy with the production shell features
`drm,live-vdi,media-mpv`, temporarily installed on `.138`, and explicitly
routed through the direct-DRM proof harness. Native frames were inspected for
route identity, clipping, overlap, contrast, taskbar separation, and the
large-text narrow lane.

| Seat / profile | Native readback SHA-256 | Proof |
|---|---|---|
| `.138` Music Dark desktop | `fe46ceef9e1e8edbdb99215f73cbc5c817a3d45b8b5f7708bc789cbfa406d7b3` | [PNG](evidence/WL-UX-009-2026-08-02-138-music-dark-desktop-2f32f935.png) |
| `.138` Music Dark narrow (`800` logical) | `e9db73dbb4f6792321773fc583b4f079b94be5a667c27479c17e361edf1f04de` | [PNG](evidence/WL-UX-009-2026-08-02-138-music-dark-narrow-2f32f935.png) |
| `.138` Music Light/Largest narrow (`800` logical) | `2f81bfd0572ebd8d37e4e21dad1ce9fd604a2c48baf2b4314cb909311c42a2e0` | [PNG](evidence/WL-UX-009-2026-08-02-138-music-light-largest-narrow-2f32f935.png) |
| `.138` Media Dark desktop | `e1fc42726e150ddb2fc69fb52de573324ab0f228acadda73e6b56d0f79555c25` | [PNG](evidence/WL-UX-009-2026-08-02-138-media-dark-desktop-2f32f935.png) |

All four frames are accepted as route-specific evidence. The Music frames
show the honest disconnected-server state without clipping; the Media frame
shows the Sources surface with its source, capture, Jellyfin, and mesh-source
boundaries visible. `.138` was restored to payload `20955383…`, secure
login-at-boot, Dark/Construct/Default/Normal, active service, zero restarts,
and no proof drop-in. This closes only the current-payload Music slice and one
Media cell; remaining route/profile coverage, strict linear scanout, Dell
adoption, VDI guest readiness, and overall WL-UX-009 readiness remain open.

## Current-payload Phones, Terminal, and Editor route validation — 2026-08-02

The same production-feature candidate
`2f32f935c92a4cf84f926221a093a3666638fee9063ec4f9a8dc8ef1f686f628` was
explicitly routed on `.138` through direct DRM. Native frames were inspected
for route identity, hierarchy, clipping, overlap, taskbar separation, and
host/guest ownership.

| Seat / route | Native readback SHA-256 | Proof |
|---|---|---|
| `.138` Phones Dark desktop | `d5fa4d440902add51624b992f53ed50beac8dac6b4e868fdc402923a87d7e14e` | [PNG](evidence/WL-UX-009-2026-08-02-138-phones-dark-desktop-2f32f935.png) |
| `.138` Terminal Dark desktop | `7dda6e156b399e91698b2cd710e3478d78e0c5870ae9b5c143e093491a4c1e10` | [PNG](evidence/WL-UX-009-2026-08-02-138-terminal-dark-desktop-2f32f935.png) |
| `.138` Editor via Mesh Teams Dark desktop | `ae818c3969955c7d6c5b94bf410d1c2a707a622fea99a10d7086dd85d7b47821` | [PNG](evidence/WL-UX-009-2026-08-02-138-editor-dark-desktop-2f32f935.png) |

All three frames are accepted as host-owned route evidence. The Editor frame
specifically shows the approved boundary: Construct owns the Mesh Teams host
frame and embedded Editor surface; no guest application styling or VDI
readiness claim is made. `.138` was restored to payload `20955383…`, secure
login-at-boot, Dark/Construct/Default/Normal, active service, zero restarts,
and no proof drop-in. Remaining profile cells, Dell adoption, Browser/VDI
boundary proof, strict linear scanout, and overall WL-UX-009 readiness remain
open.

## Dell current-payload Phones, Terminal, and Editor validation — 2026-08-02

Candidate `2f32f935c92a4cf84f926221a093a3666638fee9063ec4f9a8dc8ef1f686f628`
was temporarily installed on Dell `.225` with explicit direct-DRM routing.
All three Dark desktop frames were inspected and accepted.

| Seat / route | Native readback SHA-256 | Proof |
|---|---|---|
| Dell `.225` Phones Dark desktop | `ff7eab380c7c66b119a96602c760d18a0a743cd81071028ce9bd7898379920ee` | [PNG](evidence/WL-UX-009-2026-08-02-225-phones-dark-desktop-2f32f935.png) |
| Dell `.225` Terminal Dark desktop | `8fe55841f34d693f2e4a0cd72e9cffa2aefa9ed5c5ddb9041ce19a0c4da9b18f` | [PNG](evidence/WL-UX-009-2026-08-02-225-terminal-dark-desktop-2f32f935.png) |
| Dell `.225` Editor via Mesh Teams Dark desktop | `1a91b0bbfa0381be08bca5c0a16748124e909fa5b3af405962b166d61269c5d0` | [PNG](evidence/WL-UX-009-2026-08-02-225-editor-dark-desktop-2f32f935.png) |

The Editor frame preserves the approved boundary: Construct owns the Mesh
Teams host frame and embedded Editor surface; no guest application styling or
VDI readiness claim is made. Dell was restored to payload `20955383…`, secure
login-at-boot, Dark/Construct/Default/Normal, active service, zero restarts,
and no proof drop-in. This closes only the Dell Dark desktop route slice;
Light/Largest and narrow cells, remaining Media coverage, Browser/VDI
boundaries, strict linear scanout, and overall WL-UX-009 readiness remain open.

## Dell Editor profile validation — 2026-08-02

Candidate `cc56fdf0466a29506b8b7adcf27af8aa3f7a034d87bdebfe20143093289f2dbc`
was explicitly routed to the unified Editor/Communications surface on Dell
`.225` through direct DRM. All three remaining Editor profiles were inspected
and accepted.

| Seat / profile | Native readback SHA-256 | Proof |
|---|---|---|
| Dell `.225` Editor Dark narrow (`800` logical) | `657d05c170af3da6da3f0986416dc4de2eb3dca7947bccf61e087369b9fc66a7` | [PNG](evidence/WL-UX-009-2026-08-02-225-editor-dark-narrow-cc56fdf0.png) |
| Dell `.225` Editor Light desktop | `9becdde602942dc4be543f4903dceea7fbf80414c06d8919d067877965e08105` | [PNG](evidence/WL-UX-009-2026-08-02-225-editor-light-desktop-cc56fdf0.png) |
| Dell `.225` Editor Light/Largest narrow (`800` logical) | `170c1c5dbe760ff0b45d1801e4e47b853db13bceeec6dcd5d30944e80481dd51` | [PNG](evidence/WL-UX-009-2026-08-02-225-editor-light-largest-narrow-cc56fdf0.png) |

Native inspection accepts all three frames. The approved host-owned Mesh Teams
and embedded Editor boundary remains clear; no guest application styling or VDI
readiness claim is made. Dell was restored to payload `20955383…`, secure
login-at-boot, Dark/Construct/Default/Normal, active service, zero restarts,
and no proof drop-in. Remaining Media profiles, Browser/VDI boundaries, strict
linear scanout, and overall WL-UX-009 readiness remain open.

## Dell Terminal profile validation — 2026-08-02

Candidate `cc56fdf0466a29506b8b7adcf27af8aa3f7a034d87bdebfe20143093289f2dbc`
was explicitly routed on Dell `.225` through direct DRM. All three remaining
Terminal profiles were inspected and accepted.

| Seat / profile | Native readback SHA-256 | Proof |
|---|---|---|
| Dell `.225` Terminal Dark narrow (`800` logical) | `85f855c2002435a5d2cf061752eff8b92cec0d4593f6176b5da35d588de1bcc1` | [PNG](evidence/WL-UX-009-2026-08-02-225-terminal-dark-narrow-cc56fdf0.png) |
| Dell `.225` Terminal Light desktop | `cb4bb184d19c622d82e57305916166621a75d1c910333beaceaa3b2ab814c05d` | [PNG](evidence/WL-UX-009-2026-08-02-225-terminal-light-desktop-cc56fdf0.png) |
| Dell `.225` Terminal Light/Largest narrow (`800` logical) | `fb332bdeb46ff76c80415ac2c441b4515f9268e2f33c4126a1bd652e5e6fcf8a` | [PNG](evidence/WL-UX-009-2026-08-02-225-terminal-light-largest-narrow-cc56fdf0.png) |

Native inspection confirms the Terminal content remains bounded above the
taskbar in all three profiles. Dell was restored to payload `20955383…`,
secure login-at-boot, Dark/Construct/Default/Normal, active service, zero
restarts, and no proof drop-in. Remaining Editor/Media profiles, Browser/VDI
boundaries, strict linear scanout, and overall WL-UX-009 readiness remain open.

## Phones Light/Largest narrow remediation — 2026-08-02

The Dell `.225` Light/Largest narrow frame previously clipped the final
`Disarm now` control at the taskbar boundary. The explicit revoke action now
shares the wrapped arm-action lane, preserving the security control while
removing the unnecessary extra row at large text.

| Seat / profile | Candidate SHA-256 | Native readback SHA-256 | Proof |
|---|---|---|---|
| Dell `.225` Phones Light/Largest narrow (`800` logical) | `cc56fdf0466a29506b8b7adcf27af8aa3f7a034d87bdebfe20143093289f2dbc` | `4b4b8e97ea7c4129adca382d88c5607155af8d07579f74785f5b7ce44ad2b831` | [PNG](evidence/WL-UX-009-2026-08-02-225-phones-light-largest-narrow-cc56fdf0.png) |

Native inspection accepts the corrected frame: the Remote input card,
including `Disarm now`, is fully visible above the taskbar with no overlap or
clipping. Phones tests pass 26/26. Dell was restored to payload `20955383…`,
secure login-at-boot, Dark/Construct/Default/Normal, active service, zero
restarts, and no proof drop-in. Remaining route/profile cells, Browser/VDI
boundaries, strict linear scanout, and overall WL-UX-009 readiness remain open.
