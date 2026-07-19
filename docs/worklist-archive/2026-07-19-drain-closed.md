# Drain-closed epics — 2026-07-19

These 8 epics were verified COMPLETE on real code paths by the 2026-07-19 drain
reconciliation (`wf_924f2a46-283`) and moved out of the active worklist per the
stewardship archive-on-close rule. Full file:line evidence for each is in
`docs/platform/DRAIN-RECONCILIATION-2026-07-19.md`.

Disposition: **DONE** (2026-07-19). Marker had lagged the code.

---

### WL-CRIT-002 - VDI reconnect and disconnected-state UX

- Disposition: DONE (2026-07-19 drain reconciliation; evidence in DRAIN-RECONCILIATION-2026-07-19.md)
- Priority: P1
- Complexity: Medium
- Problem: A transport drop can leave the desktop frozen on the last frame or
  require manual recovery. Design docs promise reconnectable sessions, but the
  user-visible disconnected state and bounded reconnect loop are not complete.
- Required outcome: Every transport Error or Ended state tears down the dead
  live handle, shows an explicit disconnected overlay, offers Retry and Back to
  Chooser, and attempts bounded auto-reconnect where safe.
- Scope: RDP/VNC/SPICE live handles, broker state transitions, toast/status
  surfacing, and retry/backoff policy.
- Relevant files/components: `crates/desktop/mde-shell-egui/src/vdi.rs`,
  `crates/desktop/mde-shell-egui/src/session.rs`,
  `crates/mesh/mackesd/src/workers/session_broker.rs`,
  `crates/desktop/mde-vdi-rdp/src/connect.rs`.
- Dependencies: WL-CRIT-001 for mesh-hosted console paths; direct endpoint paths
  can be implemented independently.
- Acceptance criteria: Dropping a live transport shows the reason, stops sending
  input into a dead channel, retries with visible state, and returns broker state
  to Active after a successful reconnect.
- Verification method: Targeted unit tests for state transitions and a live
  drop/restore test against at least one transport.
- Origin or merged source IDs: E12-8, platform review `vdi-vm-4` and
  `shell-ux-1`, old worklist line 366.


### WL-CRIT-005 - Substrate-v2 fleet cutover and LizardFS wedge removal

- Disposition: DONE (2026-07-19 drain reconciliation; evidence in DRAIN-RECONCILIATION-2026-07-19.md)
- Priority: P0
- Complexity: Epic
- Problem: Incident notes show live FUSE/LizardFS wedge risk remains until the
  fleet is cut over to etcd plus Syncthing and the old single-master mount
  dependency is retired.
- Required outcome: The live fleet runs the substrate-v2 path with no required
  LizardFS/QNM mount for control-plane correctness, and reboot/failover does not
  reintroduce a wedged FUSE dependency.
- Scope: Fleet cutover, live lighthouses, QNM retirement, Syncthing/etcd
  verification, incident cleanup, and runbook updates.
- Relevant files/components: `automation/substrate/`, `docs/ops/substrate-v2-cutover-runbook.md`,
  `docs/ops/lighthouse-eagle-migration-recon.md`, mackesd substrate workers.
- Dependencies: Live maintenance window and operator authority on the deployed
  lighthouses.
- Acceptance criteria: Cutover completes on live nodes, no critical worker needs
  the retired FUSE mount, reboot recovery passes, and old wedge incidents cannot
  recur through the same dependency.
- Verification method: Operator-run cutover log, reboot/recovery gate, and
  post-cutover health snapshots.
- Origin or merged source IDs: OPROG-1, OPROG-2, LH-JOIN-QNM-1,
  INCIDENT-WEDGE-2, old worklist lines 2210, 2227, 2230, 2251.


### WL-ARCH-005 - Browser worker crypto seam and mde-seal emitter completion

- Disposition: DONE (2026-07-19 drain reconciliation; evidence in DRAIN-RECONCILIATION-2026-07-19.md)
- Priority: P2
- Complexity: Medium
- Problem: Browser worker extraction is mostly done, but passkey/credential
  crypto still needs a shared seal/crypto seam, and `mde-seal` carries emitter
  placeholders that should become a real generated-contract path or be removed.
- Required outcome: Browser passkey/secret operations use shared, tested crypto
  primitives and `mde-seal` has no dormant placeholder emitter paths.
- Scope: Shared crypto crate API, browser passkey workers, seal emitter, tests,
  and docs.
- Relevant files/components: `crates/mesh/mde-seal/src/lib.rs`,
  `crates/mesh/mde-browser-workers/`, `crates/mesh/mackesd/src/workers/browser_*`.
- Dependencies: Crypto API review.
- Acceptance criteria: No placeholder returns remain in production paths; browser
  passkey workers use the shared seam; old duplicate crypto helpers are gone or
  archived.
- Verification method: Unit tests for seal/passkey flows, grep for placeholder
  emitter paths, and cargo test for browser worker crates.
- Origin or merged source IDs: open ledger `arch-7`, TODO scan of `mde-seal`.


### WL-RUN-001 - Auto-repair must either repair or say observe-only

- Disposition: DONE (2026-07-19 drain reconciliation; evidence in DRAIN-RECONCILIATION-2026-07-19.md)
- Priority: P2
- Complexity: Medium
- Problem: The reconciler can queue repair intent while the actual take-action
  layer is gated, creating a say/do gap for self-healing claims.
- Required outcome: Either implement the take-action repair executor over the
  current substrate, or make observe-only status explicit in health/UI/audit text
  and track the executor separately.
- Scope: Reconcile worker, audit wording, health output, UI status, and repair
  executor.
- Relevant files/components: reconcile worker, openstack reconcile paths,
  health/status UI.
- Dependencies: Connectivity substrate decisions for safe repair actions.
- Acceptance criteria: A detected drift either changes state through a tested
  executor or records a clearly non-repairing observation; no row says queued as
  if action occurred.
- Verification method: Unit test with injected drift and audit assertions; live
  dry-run on non-destructive drift.
- Origin or merged source IDs: platform review `mackesd-03`.


### WL-RUN-005 - Device Manager multi-source inventory and fault notifications

- Disposition: DONE (2026-07-19 drain reconciliation; evidence in DRAIN-RECONCILIATION-2026-07-19.md)
- Priority: P2
- Complexity: Medium
- Problem: Device Manager needs source coverage and eventing beyond local PC
  inventory: Cloud/Nova instances, paired phones, LAN hosts, routers, and fault
  transitions should render accurately and notify without spam.
- Required outcome: Each host type contributes only applicable categories, and a
  transition into problem state emits a debounced notification to Chat/phone.
- Scope: Source adapters, host rail, device tree rendering, fault detector,
  notification routing, and tests.
- Relevant files/components: `crates/desktop/mde-shell-egui/src/device_manager/`,
  Nova registry, KDC, LAN probe, router registry, chat alert paths.
- Dependencies: Representative source data for each host type.
- Acceptance criteria: Tests map each source type to the right categories; a
  simulated fault fires once; flapping does not spam.
- Verification method: Unit tests with fixtures plus live test-bed render.
- Origin or merged source IDs: Device Manager open bullets, old worklist lines
  4369, 4370, 4386-4395.


### WL-FUNC-004 - Browser power tools, downloads, PDF/print, capture, and protocol handling

- Disposition: DONE (2026-07-19 drain reconciliation; evidence in DRAIN-RECONCILIATION-2026-07-19.md)
- Priority: P2
- Complexity: Large
- Problem: Browser has many first-party tools, but the daily-driver tail still
  includes Power mode, DevTools/view-source/UA/device APIs, downloader/scraper,
  full PDF/print/save-as-PDF, capture, translation/cache, notifications, and
  protocol handlers.
- Required outcome: Each tool is either implemented through the Browser command
  model or intentionally absent with no dead menu item.
- Scope: Browser command model, download manager, PDF/print, capture, DevTools,
  protocol handlers, offline/cache/translation, and notifications.
- Relevant files/components: `crates/desktop/mde-shell-egui/src/web/menubar.rs`,
  `crates/desktop/mde-shell-egui/src/web/chrome_ui/`, capture/printing modules,
  transfer service.
- Dependencies: CUPS/printing environment and transfer service.
- Current evidence: The 2026-07-17 menu truthfulness pass tightened the Browser
  command model so Browser-owned internal pages no longer advertise helper/page
  tools, stale saved-PDF paths no longer enable `Open Last PDF`, and the no-page
  menu gate still leaves only genuine chrome/bookmark-manager controls active.
  Farm evidence: `.50` fmt, BigBoy `.130` internal-page menu test, `.90`
  stale-PDF menu test, and `.170` no-page menu test passed. A 2026-07-18 `.15`
  recovery pass confirmed passwordless sudo now works for `mm`, quarantined stale
  Browser session-sync/send-tab replay state from the root and mesh-storage
  Browser sync paths, restarted the shell service cleanly, and passed the
  installed split Browser RPM CEF/Servo display/input verifier. BigBoy `.130`
  then produced Fedora 44 replacement base and Browser RPMs with size guards
  passing (72.8 MiB and 39.0 MiB). The matched pair was staged on `.15` at
  `/home/mm/browser-f44-live-proof-20260718-022147/`, installed after a clean
  `rpm -Uvh --test --replacepkgs --force --nosignature`, and restarted
  `mde-shell-egui.service` to `MainPID=1890763`, `NRestarts=0`, start timestamp
  `2026-07-18 02:22:20 EDT`. Installed verifier evidence passed CEF and Servo
  display/input plus helper cleanup; the running `/proc/1890763/exe` hash
  matches `/usr/bin/mde-shell-egui`
  `df63dff6720eea1230997a9167d57b9a1c4810f243f40512b34f2ff7534c40a3`. RPM
  sha256s: base
  `fde1f7e072e0e125488d30dbae9743647b25cf1cdffc8146cc454b8f32bee567`, Browser
  `5445248561e901338306b32f3fe2cc34c93e79528642fc1b402f109f9c514cdb`.
  A later 2026-07-17 Browser Options pass replaced the generic disabled-row
  tooltip with command-specific gate explanations for typed-address, history,
  live web-page tools, painted-frame captures, saved-PDF viewer, CEF DevTools,
  loaded-URL share/download actions, first-party-site permission actions, and
  data-clear actions; farm `.50` fmt and `.130` focused
  `browser_options_disabled_rows_explain_their_command_gate` passed.
  A later 2026-07-17 Browser downloads drawer pass removed internal
  `browser_download`/ledger wording from the drawer header, replaced it with a
  user-facing live status summary derived from active/total Browser transfer
  counts, and kept the empty worker state honest without exposing implementation
  terms; farm `.50` fmt and `.130` focused
  `browser_download_drawer_header_uses_user_facing_status` passed.
  A later 2026-07-17 Browser artifact identity pass centralized the Browser
  product label, kept the new-tab dashboard on the same label, and changed
  capture/PDF folders, MHTML/offline-copy subjects, and generated CUPS job
  titles from superseded legacy Browser wording to the current Browser
  product label; farm
  `.50` fmt plus BigBoy focused artifact, dashboard, and CUPS title tests passed.
  A later 2026-07-17 Browser menu copy pass removed internal follow-up/v1/stub
  language from visible Power/Privacy captions while keeping the command gates
  intact; farm `.50` fmt plus `.90` focused Privacy menu coverage passed, and
  BigBoy `.130` focused Power menu coverage passed after session recovery.
  A later 2026-07-17 Browser scrape-export copy pass removed internal follow-up
  hook wording from generated Markdown artifacts, kept the bounded crawl status
  honest, and covered both no-DOM and DOM-backed scrape exports; farm `.50` fmt
  and BigBoy `.130` focused `scrape_export` coverage passed.
  A later 2026-07-17 Power-menu polish pass replaced implementation/backlog
  wording in visible media-manifest and scrape-export captions with user-facing
  behavior language and extended the Power-menu copy guard; farm `.50` fmt and
  BigBoy `.130` focused `power_mode_adds_power_menu_and_status_chip` passed.
  A later 2026-07-17 Browser Options copy pass replaced visible runtime,
  helper-backed, and internal-tab wording with user-facing Controls, Engines,
  open-tab, and live-web-page labels; BigBoy `.130` focused `browser_options`
  coverage passed.
  A later 2026-07-17 Browser downloads unavailable-state copy pass replaced the
  remaining visible Transfers worker wording with Browser-facing downloads
  unavailable copy and extended drawer guards against worker/helper/ledger terms;
  farm `.50` fmt and BigBoy `.130` focused drawer header/muted-note tests passed.
  A later 2026-07-17 Browser engine copy pass replaced visible `CEF / Chromium`
  and `Chromium/CEF runtime` wording in engine menu rows, Options, hover cards,
  DevTools gates, and runtime notices with user-facing Chromium labels while
  preserving CEF implementation markers; farm `.50` fmt, `.90` menu/hover tests,
  `.170` chrome-ui label tests, and BigBoy `.130` live-helper gate test passed.
  A later 2026-07-17 Browser update-status copy pass replaced raw updater state,
  runtime paths, CEF wording, and manifest errors in the engine update drawer and
  launch-block notice with Chromium-facing labels and sanitized verification
  details; farm `.50` fmt, `.90` focused drawer render coverage, `.170`
  status-chip coverage, and BigBoy `.130` live-helper gate coverage passed.
  A later 2026-07-17 Browser export/notice copy pass removed helper, handoff,
  and CEF viewer/tab wording from scrape Markdown artifacts, PDF viewer notices,
  DevTools gates, and malformed passkey notices while preserving the underlying
  CEF implementation paths; farm `.50` fmt, BigBoy `.130` scrape-export coverage,
  `.90` saved-PDF viewer coverage, and `.170` DevTools/passkey notice coverage
  passed.
  A later 2026-07-17 Browser speech/passkey copy pass moved read-aloud, voice
  input, and passkey approval chrome off raw TTS/STT/CEF/runtime wording while
  preserving worker payloads; farm `.50` fmt plus speech-status parser/display
  coverage, BigBoy `.130` drawer/prompt paint coverage, `.90` menubar
  chip/read-aloud notice coverage, and `.170` passkey/voice-command coverage
  passed.
  A later 2026-07-17 Browser empty/gate copy pass replaced the no-page body,
  AccessKit empty summary, no-seat gate, missing-engine gate, spawn-failure gate,
  and incomplete Chromium gate with Browser-facing language instead of
  sandbox/helper/Servo/runtime/path wording; farm `.50` fmt, BigBoy `.130`
  focused empty-body paint coverage, `.90` live-helper empty-state coverage, and
  `.170` live-helper spawn/engine-gate coverage passed.
  A later 2026-07-17 Browser spelling copy pass kept raw Hunspell failure details
  in the spellcheck result state for diagnostics but moved the visible spelling
  status notice, drawer header summary, and warning row to Browser-facing
  dictionary/service language; farm `.50` fmt and BigBoy `.130` focused
  `spellcheck` coverage passed.
  A later 2026-07-17 Browser print copy pass moved print drawer labels,
  unavailable-printer notices, queued/failure print notices, and print-job
  completion text off CUPS/lp/spool-path wording while keeping the raw CUPS/lp
  helpers tested internally; farm `.50` fmt and BigBoy `.130` focused `print`
  coverage passed.
  A follow-up 2026-07-18 Browser password-menu popup pass gave the toolbar
  password/autofill popup the same reserved Browser chrome width as other
  toolbar popups, bounded long site and username text, and kept the lock icon
  on the Browser/YAMIS icon paint path. Farm evidence: BigBoy `.130` focused
  `cargo test -p mde-shell-egui password -- --nocapture` passed, and `.50`
  `cargo fmt -p mde-shell-egui -p mde-files-egui -- --check` passed.
  A later 2026-07-17 Browser media-export copy pass renamed the visible Power
  menu media export row and status notices from media-manifest/spool wording to
  media-list/export language while preserving the internal JSON manifest format;
  farm `.50` fmt, BigBoy `.130` focused `media_export`, and `.90` focused Power
  menu coverage passed.
  A later 2026-07-17 Browser web-archive copy pass moved capture/offline-copy
  archive labels and status notices off MHTML wording while preserving the
  internal `.mhtml` archive format and save path; farm `.50` fmt, BigBoy `.130`
  focused offline-copy drawer coverage, `.90` focused menubar coverage, and
  `.170` focused capture-notice coverage passed.
  A later 2026-07-17 Browser offline-copy metadata pass moved the offline drawer
  and cached-body fallback off raw cache ids, tab indexes, engine labels, PNG
  format names, and timestamp-like counters while preserving the underlying
  cached snapshot state; farm `.50` fmt, BigBoy `.130` focused offline-copy
  drawer coverage, and `.90` focused cached-body coverage passed.
  A later 2026-07-17 Browser share/translation drawer metadata pass moved QR
  share and Translation drawers off raw request ids, peer hosts, matrix module
  terminology, tab indexes, and engine labels while preserving the underlying
  share route and translation result state; farm `.50` fmt, BigBoy `.130`
  focused QR drawer coverage, and `.90` focused Translation drawer coverage
  passed.
  A later 2026-07-17 Browser Site Styles copy pass moved the menu and drawer off
  injected/host/Userscripts implementation wording while preserving the CSS
  editor and site-style state; farm `.50` fmt, BigBoy `.130` focused Site
  Styles drawer coverage, and `.90` focused menubar coverage passed.
  A later 2026-07-17 Browser security/privacy copy pass moved safe-browsing and
  site-info surfaces off visible host/mesh-hosted wording while preserving host
  matching and policy-source state; farm `.170` fmt, BigBoy `.130` focused
  security panel coverage, `.90` focused Privacy menu coverage, and `.50`
  focused safe-browsing source coverage passed.
  A later 2026-07-17 Browser custom-filter policy pass made
  `browser/custom-filter-rules.txt` a real operator policy source instead of a
  test-only seam, publishes source-read status, keeps last-good rules active on
  missing/error, and shows the live custom-filter status in the Privacy menu;
  farm `.170` fmt, BigBoy `.130` focused custom-filter source coverage, `.90`
  focused Privacy menu coverage, and `.50` policy-source audit coverage passed.
  A later 2026-07-17 Browser synced-filter pass made
  `adfilter/compiled/engine.json` the real mesh-synced filter-list source for
  Browser blocking, preserves local operator custom rules while importing the
  compiled worker store, publishes source-read status, and replaces the static
  Privacy-menu filter-list promise with the live source count; farm `.170` fmt,
  BigBoy `.130` focused synced-filter source coverage, `.90` Privacy menu
  coverage, and `.50` policy-source audit coverage passed.
  A later 2026-07-17 Browser permission copy pass kept permission prompt
  decisions, Privacy menu captions, site-info panel text, and capture notices on
  user-facing blocked-by-default language while preserving the machine-readable
  permission enforcement events.
  A later 2026-07-17 Browser scrape-export engine-label pass kept JSON/CSV
  engine wire values stable while moving operator-facing Markdown exports off
  raw Servo wording and onto the same Lightweight engine label used in chrome.
  A later 2026-07-17 Browser offline-archive notice pass kept the saved archive
  format unchanged while moving the visible missing-archive notice off raw MHTML
  terminology.
  A later 2026-07-17 Browser download failure notice pass kept the transfer
  request staging paths unchanged while moving intercepted-download and
  observed-media/image download failures off raw spool/request-path terminology.
  A later 2026-07-17 Browser passkey notice pass kept WebAuthn request ids,
  handoff JSON, and ceremony terminology inside the wire/test layer while moving
  malformed, duplicate, approved, and denied passkey notices to Browser-facing
  copy; farm `.50` fmt and BigBoy `.130` focused `passkey` coverage passed.
  A later 2026-07-17 Browser output-notice pass kept capture/PDF/download paths
  in internal payloads and opener targets while moving visible Browser notices
  to filename/folder labels; farm `.50` fmt and BigBoy `.130` focused
  `browser_output_notices_hide_absolute_paths` coverage passed.
  A later 2026-07-17 Browser downloads AccessKit pass kept the existing
  download-manager buttons and transfer dispatch seams intact while exposing each
  visible download as a read-only accessible row with state, route, real progress
  metadata, verification, and error details; farm `.50` fmt and BigBoy `.130`
  focused `browser_download_rows_export_accesskit_status` coverage passed.
  A later 2026-07-17 Browser toolbar overflow pass replaced the fixed compact
  cutoff with an explicit toolbar budget model, hides optional controls before
  the address bar is squeezed, and clamps the omnibox minimum width to the
  actual remaining row budget so tight horizontal-tab layouts do not push the
  right-side controls offscreen. Farm evidence: BigBoy `.130`
  `cargo fmt -p mde-shell-egui --check`, `.90` focused
  `navigation_toolbar_compacts_before_squeezing_the_address_bar`, and `.50`
  focused `browser_visual_audit_screenshots_cover_tab_modes_and_viewports`
  passed; the visual audit wrote `browser-wide-vertical-options.png` and
  `browser-compact-horizontal-page.png` on the farm.
  A later 2026-07-17 Browser Chrome palette pass mapped the Browser-local
  chrome tokens to Chromium/Chrome Refresh light roles for white toolbar/page
  surfaces, the pale blue surface container, Google blue primary actions,
  neutral text, subtle text, and outline strokes. It also removed raw black
  alpha tab depth fills from tab pills and engine badges so the Browser chrome
  no longer inherits the darker shell look. Farm evidence: BigBoy `.130`
  focused `browser_chrome_palette_matches_chrome_refresh_light_roles`,
  `browser_tab_depth_uses_chrome_neutral_depth_not_raw_black`,
  `paused_active_browser_media_page_uses_low_rate_heartbeat`, and
  `cargo fmt -p mde-shell-egui --check` passed. A follow-up visual-audit guard
  made the Browser screenshot test require official light Chrome-palette pixel
  coverage in the wide toolbar, vertical tab rail, and compact horizontal
  toolbar; farm `.50` fmt and BigBoy `.130` focused
  `browser_visual_audit_screenshots_cover_tab_modes_and_viewports` passed and
  wrote refreshed `browser-wide-vertical-options.png` and
  `browser-compact-horizontal-page.png` screenshots.
  A 2026-07-18 Browser surface-scope pass made Browser-owned body/interstitial
  rendering, prompt bars, capture notices, drawer stack, popovers, context
  menus, and tooltips install Browser Chrome visuals at their own entry points
  so shell-invoked surfaces cannot inherit the shared dark shell text/fill
  palette. The Browser icon paint guards now accept both local vector fallback
  icons and YAMIS image meshes. Farm evidence: `.50` fmt and tooltip coverage,
  `.90` insecure-prompt coverage, `.170` page-context and capture-notice
  coverage, and BigBoy `.130` self-scope and dialog-prompt coverage passed. A
  later 2026-07-18 Browser Options compact-layout pass replaced the narrow
  category index's nested unbounded icon/text groups with bounded Chrome-colored
  chips, clipping labels inside stable chip rects so phone/tablet widths cannot
  collide or spill while preserving the command page and menu dispatch model.
  Farm evidence: BigBoy `.130` focused `browser_options` suite passed 9 tests;
  `.90` focused `browser_options_compact_category_chips_fit_phone_width` passed;
  `.170` focused
  `browser_options_page_uses_compact_single_column_layout_when_narrow` passed;
  `.50` `cargo fmt -p mde-shell-egui --check` passed. A later 2026-07-18
  Browser tab-search compact-layout pass removed fixed 300/288px widths from
  the toolbar tab-search popup, bounds the panel and rows to the available
  Browser chrome width, clips result labels, and collapses the clear control
  when the search field is cramped so narrow Browser layouts cannot spill rows
  or input controls offscreen. Farm evidence: BigBoy `.130` focused
  `tab_search` suite passed 6 tests; `.90` fresh-slot focused
  `tab_search_rows_clip_to_narrow_browser_chrome_width` passed; `.170` focused
  `tab_search_toolbar_anchor_uses_browser_icon_button` passed; `.50`
  `cargo fmt -p mde-shell-egui --check` passed. A later 2026-07-18 Browser
  Search Tabs popup-surface pass wrapped the popup contents in the shared
  Browser `chrome_popup_frame`, subtracting frame margin from the narrow content
  budget so the popup paints the official Chrome-light surface/outline without
  pushing beyond tight Browser chrome. Farm evidence: `.50` file-scoped
  `rustfmt --edition 2024 --config skip_children=true --check
  crates/desktop/mde-shell-egui/src/web/chrome_ui/mod.rs` passed, and BigBoy
  `.130` focused
  `cargo test -p mde-shell-egui tab_search_rows_clip_to_narrow_browser_chrome_width -- --nocapture`
  passed. A later 2026-07-18 Browser
  page-context compact-layout pass removed the fixed page-context menu minimum,
  made shared Browser menu rows and separators use the bounded available chrome
  width, clipped labels inside each row rect, and kept command accessibility and
  shared tab-context row behavior intact. The tab-context icon guard now accepts
  both local vector fallback icons and resolved YAMIS icon meshes. Farm
  evidence: BigBoy `.130` focused `page_context` suite passed 4 tests; `.90`
  focused `page_context_menu_rows_clip_to_narrow_browser_chrome_width` passed;
  `.170` focused `tab_context_menu_rows_use_browser_material_icons_and_text`
  passed after the YAMIS-aware test refresh; `.50`
  `cargo fmt -p mde-shell-egui --check` passed. A later 2026-07-18 Browser
  drawer compact-layout pass made drawer text fields, separators, progress
  bars, QR matrices, and download rows clamp to the bounded Browser drawer
  width. The downloads drawer now wraps its header and per-download actions at
  narrow widths instead of expanding the full Browser surface offscreen, while
  desktop alignment remains unchanged. Drawer icon paint tests now accept both
  Browser vector fallbacks and resolved YAMIS image meshes. Farm evidence:
  BigBoy `.130` focused `drawer` suite passed 25 tests; `.90` focused
  `browser_qr_share_drawer_matrix_clamps_to_narrow_drawer_width` passed; `.170`
  focused `browser_download_progress_bar_clamps_to_narrow_drawer_width` passed;
  `.50` `cargo fmt --check` passed. A later 2026-07-18 Browser drawer
  hover-state pass made selected print-drawer toggles and selector chips use
  Chrome on-color state layers instead of darkening selected controls toward the
  text role, keeping hover paint readable on selected Browser drawer controls.
  Farm evidence: BigBoy `.130` focused
  `browser_drawer_hover_layers_use_chrome_on_color_roles` passed; `.50`
  `cargo fmt -p mde-shell-egui --check` was attempted but is currently blocked by
  unrelated dirty formatting in Chat files and pre-existing Browser toolbar
  budget code. A later 2026-07-18 Browser drawer tooltip sweep routed print
  stepper and offline viewport-image hovers through Browser Chrome tooltip
  primitives instead of inline egui hover closures, with rendered coverage for
  the print stepper tooltip's Browser text/surface paint under dark shell
  visuals. Farm evidence: `.50`
  `cargo fmt -p mde-shell-egui -- --check` passed and BigBoy `.130` focused
  `browser_print_drawer_stepper_hover_uses_browser_tooltip_surface` passed. A
  later 2026-07-18 Browser context-menu chrome pass routed page, address-bar,
  and tab right-click menus through the Browser Chrome visual scope before egui
  builds the native menu frame, and mapped egui's open widget state to Browser
  Chrome roles so menu/open controls cannot inherit dark shell fills. Farm
  evidence: `.50` `cargo fmt -p mde-shell-egui -- --check` passed and `.90`
  focused `cargo test -p mde-shell-egui browser_chrome -- --nocapture` passed
  12 tests. A later 2026-07-18 Browser site-info popup pass routed both
  security-chip entry points through the same reserved Chrome popup frame so the
  Location-bar trust menu cannot collapse into a narrow wedge or inherit shell
  popup paint. Farm evidence: BigBoy `.130` slot `browser-site-info-popup`
  focused `security_chip_toolbar_popup_keeps_full_browser_site_info_width`
  passed; file-level `rustfmt --check` for `web/chrome_ui/mod.rs` passed on the
  same farm slot. A later 2026-07-19 Browser toolbar-popup right-edge slice
  replaced raw fixed outer popup sizing for the page-actions, bookmark-overflow,
  and password toolbar anchors with a clip-aware reservation helper, preserving
  full Browser menu width while proving page-actions, bookmark overflow, security
  chip, and password popups stay inside the right edge of narrow Browser frames.
  Farm evidence: `.90` slot `start-menu-light-style-test`
  `cargo test -p mde-shell-egui popup -- --nocapture` passed 10 tests; `.50`
  file-scoped `rustfmt --edition 2024 --config skip_children=true --check` for
  `web/chrome_ui/mod.rs` passed. A later 2026-07-18 Browser capture-artifact
  palette pass moved annotated, callout, and freehand Browser screenshot outputs
  off shared dark shell colors and onto Browser Chrome tokens: white/pale-blue
  caption surfaces, Google blue overlay accents, and Chrome text. Farm evidence:
  `.50` `cargo fmt -p mde-shell-egui --check` passed; `.90` focused
  `cargo test -p mde-shell-egui capture -- --nocapture` passed 19 tests,
  including the generated PNG pixel guards. A later 2026-07-18 Browser neutral
  icon pass aligned enabled toolbar, menu, option-row, tab-search, history, and
  page-action glyphs with Chrome's secondary icon color (`#5f6368`) instead of
  the darker primary text color, while preserving stronger text for labels and
  selected/active states. Farm evidence: `.130`
  `cargo fmt -p mde-shell-egui --check` passed; `.90` focused
  `browser_chrome_palette_matches_chrome_refresh_light_roles`,
  `page_action_tokens_cover_disabled_plain_and_bookmarked_states`,
  `tab_search_toolbar_anchor_uses_browser_icon_button`,
  `tab_context_menu_rows_use_browser_material_icons_and_text`, and
  `browser_history_drawer_rows_use_browser_material_icon_rows` passed; BigBoy
  `.130` focused `browser_visual_audit_screenshots_cover_tab_modes_and_viewports`
  passed and wrote refreshed wide/compact Browser screenshots. A later
  2026-07-18 Browser new-tab polish pass replaced the sparse default
  search-line feel with an explicitly centered Chrome-light dashboard heading,
  rounded search box, and bounded icon quick-link tiles for the installed mesh
  services while preserving the existing search/load gates. Farm evidence:
  `.50` `cargo fmt -p mde-shell-egui --check` passed and BigBoy `.130` focused
  `cargo test -p mde-shell-egui new_tab_dashboard -- --nocapture` passed 5
  tests. A later 2026-07-18 Browser page-input isolation pass tightened the
  focused page-canvas event gate so chrome/outside pointer presses cannot be
  transformed into clamped page-edge clicks, while drag-stop releases still reach
  the helper to avoid latched page buttons. A later 2026-07-18 Browser toolbar
  ordering pass made the toolbar slot model explicit: only New Tab/type, Back,
  Refresh/Stop, and Forward remain left of Location, while page/tool actions sit
  right of Location before Options, with the full-toolbar loading status included
  in the Location budget. Farm evidence: `.50`
  `cargo fmt -p mde-shell-egui --check` passed; BigBoy `.130` focused
  `browser_toolbar_order_model_keeps_only_page_navigation_left_of_location` and
  `browser_toolbar_keeps_only_page_navigation_left_of_location` passed. A later
  2026-07-18 CEF main-frame navigation hardening pass made Chromium address
  commits and navigation-generation wakeups ignore iframe/subframe callbacks, so
  Browser chrome no longer lets a subframe URL masquerade as the top-level page
  after link clicks on complex sites. Farm evidence: `.50` focused
  `cef_address_changes_only_update_top_level_url_from_the_main_frame` passed,
  `.170` focused
  `cef_navigation_generation_ignores_subframe_navigation_callbacks` passed, `.50`
  nearby `child_handler_pointers_resolve_non_null_to_their_registered_block`
  passed, and `.90`
  `cargo fmt --manifest-path crates/desktop/mde-web-cef/Cargo.toml -- --check`
  passed. A later 2026-07-18 Browser verifier-surface pass exposed the existing
  `cef-verify` clicked-link navigation probe through the installed
  `browser-verify-engines --link-navigation` wrapper, documents it as the
  deterministic pre-smoke for the Google/News class of "location changes but
  page does not commit" failures, and makes Browser RPM packaging coverage
  assert the shipped wrapper contains the mode and marker. Farm evidence: `.50`
  focused
  `cargo test -p mde-web-preview-client --features live-helper --bin cef-verify link_navigation -- --nocapture`
  passed 3 tests; `.90` focused
  `cargo test -p mackesd browser_rpm_ships_two_engine_operational_verifier_but_base_and_server_do_not --features async-services -- --nocapture`
  passed; local `bash -n install-helpers/browser-verify-engines.sh`, help output,
  conflict-mode parser check, and `git diff --check` passed. During the live
  `.15` proof, the first `c42953b5` candidate exposed a real release blocker:
  installed CEF painted frames and accepted pointer/key/text input, but emitted
  `nav_events=0` and blank `final_url`, so the Browser address/navigation wire
  path was still unsafe for the Google/News class of failures. Commit
  `0ba65d2a` fixed the CEF main-frame guard to use the pinned
  `cef_frame_t::is_main` ABI slot instead of comparing live wrapper pointers,
  with farm `.50` focused
  `cef_address_changes_only_update_top_level_url_from_the_main_frame` and `.90`
  `cargo fmt --manifest-path crates/desktop/mde-web-cef/Cargo.toml -- --check`
  passing. BigBoy `.130` then cut Fedora 44 split RPMs from the patched worktree
  with size guards passing (base 66.5 MiB, Browser 39.1 MiB); sha256s were base
  `174d360850c81ffebbe5f45bc802e4eb1cbe7df5185f95020f90573376063505` and
  Browser `7d965a4fc7144ffe4f3691e77af8a6ea0c1c79f60bdba3e2cb89b2e41227fa86`.
  The matched pair was staged on `.15` at
  `/home/mm/browser-f44-live-proof-0ba65d2a/`, installed after a clean
  `rpm -Uvh --test --replacepkgs --force --nosignature`, `rpm -V magic-mesh
  magic-mesh-browser` returned clean, and `mde-shell-egui.service` restarted to
  `MainPID=2913905`, `NRestarts=0`, start timestamp
  `2026-07-18 13:05:35 EDT`, with the running shell hash matching
  `/usr/bin/mde-shell-egui`. Installed `.15` proof passed
  `browser-verify-engines --engine all --budget 30 --timeout 60s`,
  `browser-verify-engines --engine cef --link-navigation --budget 30 --timeout
  60s`, and public CEF display/load smokes for `https://www.google.com/` and
  `https://news.google.com/`; the Google News smoke committed
  `https://news.google.com/home?hl=en-US&gl=US&ceid=US:en`, title
  `Google News`, favicon bytes, and painted frames. A follow-up 2026-07-18
  Fedora 44 split-RPM proof from commit `13844e25` produced base and Browser
  RPMs on BigBoy `.130` with size guards passing (base 66.5 MiB, Browser
  39.1 MiB; sha256s base
  `fb6b9484a27d7d94818dffb62c5f0f98e03ed5e0ae181d7a2924a024fce03d07`, Browser
  `0c0a4f9733ffb679d6a96da2b5f6397264c5385e417b376963f11b1252ff0b5e`). The
  matched pair was staged on `.15` at
  `/home/mm/browser-f44-live-proof-13844e25/`, transaction-tested, installed, and
  verified with clean `rpm -V magic-mesh magic-mesh-browser`. The live shell
  recovered to `MainPID=3087677`, `NRestarts=0`, start timestamp
  `2026-07-18 14:52:02 EDT`, and the running `/proc/3087677/exe` hash matched
  `/usr/bin/mde-shell-egui`
  `6ef89cdb22a012002586b00adb3dde86108f2af4e437c343e0fff000a1c816b6`.
  Installed `.15` proof passed
  `browser-verify-engines --engine all --budget 30 --timeout 60s`,
  `browser-verify-engines --engine cef --link-navigation --budget 30 --timeout
  60s`, `browser-verify-engines --engine cef --idle-media --budget 70 --timeout
  90s`, and public CEF display/load smokes for `https://www.google.com/` and
  `https://news.google.com/`; the Google smoke ended at
  `https://www.google.com/`, title `Google`, favicon bytes, and 215 painted
  frames, while the News smoke ended at
  `https://news.google.com/home?hl=en-US&gl=US&ceid=US:en`, title
  `Google News`, favicon bytes, and 211 painted frames.
- Acceptance criteria: Command rows dispatch to real behavior; disabled items
  explain the gate; no text-only stub menu remains.
- Verification method: Focused command dispatch tests, print/capture tests, and
  live smoke for at least one download and one PDF/print path.
- Origin or merged source IDs: BROWSER-DD-8, BROWSER-DD-10, BROWSER-DD-12, old
  worklist lines 4161, 4207, 4232.


### WL-PERF-001 - VDI dirty-rectangle display uploads

- Disposition: DONE (2026-07-19 drain reconciliation; evidence in DRAIN-RECONCILIATION-2026-07-19.md)
- Priority: P2
- Complexity: Large
- Problem: VDI display paths avoid some idle work, but changed frames can still
  upload full framebuffers instead of dirty sub-rectangles.
- Required outcome: VDI transports carry damage information to the shell and
  upload only changed regions where supported, with honest fallback to full-frame.
- Scope: SPICE/VNC/RDP frame metadata, `mde-vdi-core` image deltas, shell texture
  updates, and live visual validation.
- Relevant files/components: `crates/desktop/mde-vdi-core/`,
  `crates/desktop/mde-vdi-spice/`, `crates/desktop/mde-shell-egui/src/vdi.rs`.
- Dependencies: Stable delta API and transport support.
- Acceptance criteria: Dirty-rect transports update subregions, full-frame
  fallback remains correct, and visual output is unchanged.
- Verification method: Unit tests for ImageDelta plus live performance/visual
  smoke.
- Origin or merged source IDs: platform review `perf-7`, open ledger partial.


### WL-PERF-003 - Browser native-grade frame rate, occlusion, and audio

- Disposition: DONE (2026-07-19 drain reconciliation; evidence in DRAIN-RECONCILIATION-2026-07-19.md)
  passes"). P0/P1(tab+surface occlusion)/P2a/P3-engine done, deployed to seat .15,
  eyes-on pass. Optional refinements remain (P2b dirty-rect, P3 native-mode 🔊 pip,
  P4 HW decode) — not gaps; retire or spin out on next reconcile.
- Priority: P1
- Complexity: Large
- Problem: The CEF-OSR CPU-readback browser does not match native Chromium under
  load. Verified 2026-07-19: there is NO tab occlusion/visibility signal, so every
  background helper paints ~30 fps forever (the root cause of "5 video tabs tanks
  the system"); the frame path did 3 full-frame copies with no dirty-rect and a
  full-`Context` repaint per media frame; and page audio is silently dropped
  because `get_audio_parameters` opts into CEF's PCM stream only for the audible
  pip and then discards every sample, diverting audio away from the OS output.
- Required outcome: foreground tab sustains 60 fps (p99 <= 16.6 ms); each hidden
  tab drops to ~0 published fps; 5 simultaneous video tabs (1 visible) stay within
  a small multiple of one native tab's CPU; page audio plays to the OS in A/V sync.
- Scope (phased, disjoint-file partition):
  - P0 Instrumentation: `MDE_WEB_PERF` env-gated per-tab fps/convert/upload/paint
    metrics + a headless `mde-web-preview-client` fps bench. No ABI.
  - P1 Occlusion (flagship): `ControlMsg::SetHidden{hidden}` (wire tag 38) ->
    CEF `CefBrowserHost::WasHidden()` (vtable offset 312, field 34, on-seat
    header cross-check) -> shell per-tab visibility edges (active/PiP/surface).
  - P2 Frame path: fused BGRA->Color32 conversion (DONE); honor `on_paint` dirty
    rects + texture sub-rect upload; scope repaint off the full `Context`.
  - P3 Native audio: move audible bit to `on_audio_state_changed`, return 0 from
    `get_audio_parameters` so CEF plays to the OS, verify/enable sandbox audio out.
  - P4 HW decode (stretch): non-headless GL ozone + `VaapiVideoDecoder` — shared
    boundary with WL-FUNC-001's GPU-decode line; measure whether P1-P3 already
    hit the 5-video target before spending here.
- Relevant files/components: `crates/desktop/mde-web-wire/src/lib.rs`,
  `crates/desktop/mde-web-cef/src/cef_browser/mod.rs`,
  `crates/desktop/mde-web-cef/src/cef_init.rs`,
  `crates/desktop/mde-web-sandbox/src/lib.rs`,
  `crates/desktop/mde-web-preview-client/src/frame.rs`,
  `crates/desktop/mde-shell-egui/src/web/mod.rs`; design
  `docs/design/browser-perf-native.md`.
- Dependencies: F44 builder `.131` for seat-deployable shells; live seat `.15`/
  `.138` for eyes-on-glass. Overlaps WL-FUNC-001 (protected media / GPU decode).
- Current evidence: 2026-07-19, branch `agent/browser-enterprise-hardening`,
  farm-green (172.20.0.50), all pushed. DONE: **P0** `MDE_WEB_PERF` per-tab
  published-fps instrumentation (`d818c975`); **P1** occlusion end-to-end — wire
  `SetHidden` tag 38 (`233154ba`) + engine `was_hidden()` @offset **312 ON-SEAT
  VERIFIED** (`c735f0ee`) + shell `reconcile_tab_visibility` (`4b9cdd2c`); **P2a**
  BGRA converter fused (`233154ba`); **P3 engine** env-gated native-audio path
  `MDE_CEF_NATIVE_AUDIO`→CEF plays to OS (`ed56d8db`, default unchanged); plus a
  real perf fix (paused media no longer pins 60Hz, `fa16eddf`). shell 1637 pass.
  REMAINING: P2b dirty-rect + repaint; P3 native-mode 🔊 pip + seat-verify audio
  plays; P4 HW decode; then F44 build→seat deploy→live `MDE_WEB_PERF` 5-video +
  audio. NOTE: WIP base `aab908fb` carries 18 PRE-EXISTING GUI-polish test
  failures (start_menu/dock/chrome_ui-icons/menubar/tab-crash) to drain for
  beta-ready — 0 from the perf work (stash-baseline verified).
- Acceptance criteria: `MDE_WEB_PERF` shows the targets above; each phase is
  farm-green + unit-tested + 0 style-leaks; the 5-video case is smooth on a live
  seat with audible in-sync audio and quiescent background tabs.
- Verification method: headless fps bench + farm unit tests, then eyes-on-glass
  seat smoke per `deploy-shell-needs-drm-features`.
- Origin or merged source IDs: operator goal 2026-07-19 (native browser perf),
  memory `browser-perf-architecture-2026-07-19`.


---

## Wave 2 — operator-decided closures (2026-07-19)

Moved out of the active worklist per the operator's live drain decisions. Evidence in `docs/platform/DRAIN-RECONCILIATION-2026-07-19.md`.

### WL-ARCH-002 - Cloud resource verbs, forms, and typed arming

- Disposition: DONE (2026-07-19 operator: close as-is — unsupported verbs honestly absent, consistent with the OpenStack-exit direction; no CRUD forms wanted)
- Priority: P1
- Complexity: Large
- Problem: Cloud catalog and compute lifecycle paths exist, but generic
  create/update/delete forms and verbs for all resource kinds remain partial or
  omitted.
- Required outcome: Resource operations are catalog-driven, typed, armed before
  destructive mutation, audited, and backed by real Bus/OpenStack calls.
- Scope: Cloud UI forms, action verbs, typed arming, audit log, Heat/Octavia
  integration, and linked cross-service views.
- Relevant files/components: `crates/desktop/mde-shell-egui/src/iac/`,
  `crates/mesh/mackesd/src/workers/openstack/verbs.rs`,
  `crates/mesh/mackesd/src/workers/openstack/client/`.
- Dependencies: WL-ARCH-001 and an OpenStack test project.
- Current evidence: A 2026-07-17 Fleet/Data Center copy pass kept unsupported
  container lifecycle verbs absent from the container roster while replacing visible
  implementation/backlog wording with an operator-facing inventory-only note; farm
  `.50` fmt and BigBoy `.130` focused
  `datacenter_container_inventory_note_is_operator_facing` passed.
- Acceptance criteria: Compute, network, volume, image, and orchestration rows can
  list/show and run implemented mutations; unsupported verbs are absent, not dead
  buttons.
- Verification method: Unit tests with contract fixtures plus live create/delete
  smoke in a throwaway project.
- Origin or merged source IDs: QC-13, QC-16, QC-18, IAC partial rows, old
  worklist lines 4446, 4447, 4473.


### WL-BUILD-001 - Immutable bootc, ISO, and RPM release gate

- Disposition: CODE-DONE + PARKED TAIL (2026-07-19 operator: bootc/qcow2/farm/RPM layers done; signed-ISO cut + physical boot media = operator release-signing/hardware task)
- Priority: P1
- Complexity: Large
- Problem: Bootc image, ISO, Fedora RPM, and headless Workstation paths are
  partly implemented, but release acceptance requires live boot, signing, and
  registry/channel steps.
- Required outcome: A deployable Workstation/headless Workstation image boots,
  enrolls, starts the shell or headless services, and matches the published RPM
  payload.
- Scope: Bootc Containerfile, ISO/kickstart, RPM payload, signing, registry
  publish, role-gated units, and boot verification.
- Relevant files/components: `packaging/bootc/`, `packaging/kickstart/`,
  `install-helpers/build-rpm-fedora43.sh`, `automation/promotion/`.
- Dependencies: Live boot hardware/VM, signing material, and release authority.
- Acceptance criteria: Fresh install boots, role selection works, mackesd and
  shell/headless services start, rollback path is documented, and payload gates
  pass.
- Verification method: Farm RPM lane, boot smoke, promotion L1/L2 gates, and
  live hardware confirmation.
- Origin or merged source IDs: E12-13, OW-12, BOOT-REC-4, old worklist lines 384,
  429, 1430.


### WL-BUILD-002 - Farm shared cache and fresh-farm bootstrap

- Disposition: RETIRED (2026-07-19 operator: 'remove this work completely' — no farm shared-cache/fresh-farm-bootstrap)
- Priority: P2
- Complexity: Medium
- Problem: The farm has demand parsing and many successful lanes, but shared
  sccache/control-VM bootstrap and fresh-farm one-shot proof remain live-gated.
- Required outcome: A fresh farm node can bootstrap, join, build with shared
  cache hits, and return to a clean slot state without manual warmup.
- Scope: Golden build image, sccache backend, farm bootstrap script, slot cleanup,
  snapshot/revert, and documentation.
- Relevant files/components: `install-helpers/farm.sh`,
  `install-helpers/xcp-build.sh`, `install-helpers/farm-reconciler.sh`,
  `install-helpers/farm-sccache-proof.sh`, `automation/farm/`,
  `docs/BUILD-ENVIRONMENT.md`.
- Dependencies: Build farm control VM and live farm nodes.
- Current evidence: A 2026-07-17 shared-cache proof pass added
  `install-helpers/farm-sccache-proof.sh` and corrected
  `docs/BUILD-ENVIRONMENT.md` so it no longer claims shared sccache is live
  before proof. The live farm check reached `.50`, `.90`, `.130`, and `.170` and
  all four nodes reported no `sccache` binary and no `~/.sccache.env`, so the
  item remains open and accurately live-backed.
- Acceptance criteria: Node A build produces cache hits on node B, fresh-farm
  bootstrap completes, and slots drain without abandoned artifacts.
- Verification method: Farm lane with explicit `MCNF_BUILD_HOST` and
  `MCNF_BUILD_SLOT`, `install-helpers/farm-sccache-proof.sh status`, and
  sccache stats.
- Origin or merged source IDs: FARM-AUTO-PROD, DAR-34, DAR-35, DAR-36,
  old worklist lines 2265, 2277, 2278, 3011, 3019, 3027.


### WL-CRIT-001 - Mesh VDI console broker end to end

- Disposition: CODE-DONE + PARKED TAIL (code + unit tests complete; live two-node VM console proof needs libvirt guest + two overlay-reachable nodes = hardware/live-infra)
- Priority: P1
- Complexity: Large
- Problem: Mesh-discovered local KVM/libvirt VM desktops can publish lifecycle
  intent without a dialable console endpoint. The current architecture has
  `desktop_sources` and chooser/session plumbing, but the serving peer still
  needs a real brokered SPICE/RDP/VNC endpoint over the overlay before the shell
  can display pixels for peer-hosted VMs.
- Required outcome: Selecting a peer-hosted VM either opens an interactive
  desktop over Nebula with broker Open to Active state, or the chooser marks the
  lane honestly non-connectable with a reason.
- Scope: Console endpoint resolution, overlay relay or bind, session record
  publication, chooser endpoint resolution, and live transport attach.
- Relevant files/components: `crates/mesh/mackesd/src/workers/desktop_sources.rs`,
  `crates/mesh/mackesd/src/workers/session_broker.rs`,
  `crates/mesh/mackesd/src/workers/vm_lifecycle.rs`,
  `crates/desktop/mde-shell-egui/src/chooser/`,
  `crates/desktop/mde-shell-egui/src/vdi.rs`, `crates/desktop/mde-vdi-*`.
- Dependencies: A live libvirt/Nova host with a guest console and overlay
  reachability.
- Acceptance criteria: Broker resolves a live console port, publishes a dialable
  endpoint in the session/roster record, the shell consumes that endpoint, frames
  and input round-trip, and failed brokering is surfaced without claiming Active.
- Verification method: Unit tests for endpoint resolution and non-connectable
  states, farm build of shell and mackesd, then live seat proof against a real
  guest with frame checksum or video capture evidence.
- Origin or merged source IDs: E12-5, OW-8, QC-13, platform review `vdi-vm-1`,
  old worklist lines 353, 424, 3501.


### WL-CRIT-004 - Control-plane DR backup and guided rebirth

- Disposition: RETIRED (2026-07-19 operator: 'remove all backup — the platform can be rebuilt from scratch'; no DR backup wanted)
- Priority: P0
- Complexity: Large
- Problem: Backup/restore code exists for state and secrets, but the remaining
  DR acceptance depends on off-fleet CA/secret export, an operator-controlled
  target, and a guided restore that rebirths the control plane and re-elects a
  leader without unsafe secret handling.
- Required outcome: A documented and tested DR path backs up Tofu state, Nebula
  CA material, and secret store data to an off-fleet encrypted target, then
  restores a fresh control plane with a verified leader and usable enroll path.
- Scope: DR scripts, scheduler/RPC/button, CA-holder workflow, off-fleet target,
  restore runbook, and safety classification.
- Relevant files/components: `automation/dr/`, `docs/help/mesh-recovery.md`,
  `crates/mesh/mackesd/src/ca/`, `crates/mesh/mackesd/src/workers/`.
- Dependencies: Operator-run off-fleet export target and CA-holder access.
- Acceptance criteria: Backup bundle is produced without plaintext in logs or
  argv, restore verifies the bundle, a fresh node can enroll after restore, and
  the leader election is healthy.
- Verification method: Operator-run DR drill with logs redacted, plus local
  dry-run tests that never exfiltrate live secrets.
- Origin or merged source IDs: DR #4, DATACENTER-23, DAR-42, old worklist lines
  615 and 2507.


### WL-FUNC-001 - Browser protected media and hardware media path

- Disposition: CODE-DONE + PARKED TAIL (media-keys/PiP/media-session + named-requirement gate done; real-DRM Widevine 'passes' needs a bundle+DRM account = operator/legal/hardware)
- Priority: P1
- Complexity: Large
- Problem: CEF base operation is strong, but protected media, PiP, background
  audio, media keys, GPU/HW decode, and long-running playback are not all proven.
- Required outcome: DRM/protected-media sites work when Widevine is fetched by
  the user, non-DRM browsing still works without it, and media playback remains
  smooth on the live seat.
- Scope: Widevine fetch/install gate, protected-media permissions, media session
  control, PiP/background audio, GPU decode, and live smoke.
- Relevant files/components: `crates/desktop/mde-web-cef/`,
  `crates/desktop/mde-shell-egui/src/web/`, browser runtime installer.
- Dependencies: Widevine-capable CEF runtime and live test account/content where
  legally usable.
- Current evidence: A 2026-07-19 Browser PiP/background-media polling slice
  fixed the inactive-tab poll gate so the currently selected background
  Picture-in-Picture media tab drains helper events every Browser frame while
  the PiP overlay is visible, instead of waiting up to the one-second quiet
  background cadence. Quiet inactive tabs still use the bounded background poll
  cadence and cap, and known/unknown playing background media still bypasses
  the quiet-tab cap. Farm evidence: BigBoy `.130` slot `browser-pip-poll`
  `cargo test -p mde-shell-egui media_pip -- --nocapture` passed 5 tests; `.90`
  slot `browser-background-poll`
  `cargo test -p mde-shell-egui background -- --nocapture` passed 8 tests; `.50`
  slot `browser-pip-poll-fmt` file-scoped
  `rustfmt --edition 2021 --config skip_children=true --check` passed for
  `crates/desktop/mde-shell-egui/src/web/mod.rs`.
- Acceptance criteria: A protected-media smoke passes or is blocked with a named
  external requirement; normal browser works without CDM; media keys and PiP
  route through browser chrome.
- Verification method: Farm CEF tests plus live DRM/Spotify/Netflix-equivalent
  operator smoke.
- Origin or merged source IDs: BROWSER-DD-4, BROWSER-DD-9, old worklist lines
  4111 and 4184.


### WL-FUNC-002 - Browser passkeys, hardware keys, and phone authenticator

- Disposition: CODE-DONE + PARKED TAIL (software passkey + consent + honest UP/UV done; hardware FIDO2 key login needs a physical key + test IdP)
- Priority: P2
- Complexity: Large
- Problem: Browser passkey consent and software shapes have landed, but hardware
  CTAP2 keys, PIN/biometric verification, phone-as-authenticator, attestation, and
  real-site passwordless login remain unproven.
- Required outcome: Browser WebAuthn supports approved credential flows with
  honest User Presence/User Verification semantics and live third-party proof.
- Scope: CTAP2 hardware path, platform authenticator, KDC phone authenticator,
  attestation policy, UI prompts, and site compatibility.
- Relevant files/components: `crates/mesh/mde-browser-workers/`,
  `crates/desktop/mde-shell-egui/src/web/`, `crates/desktop/mde-web-cef/`,
  KDC components.
- Dependencies: Hardware key and test identity provider.
- Acceptance criteria: Hardware key login works, phone authenticator works or is
  explicitly gated, shell consent remains required, and UV is never asserted
  without real verification.
- Verification method: Browser worker tests plus live WebAuthn smoke against a
  controlled relying party.
- Origin or merged source IDs: BROWSER-DD-6, passkey review residuals, old
  worklist line 4123.


### WL-FUNC-007 - Media local video and library/art browse proof

- Disposition: CODE-DONE + PARKED TAIL (render-agnostic code + FakeMpv unit tests done; live mpv + media-library frame-render proof is seat/hardware-gated)
- Priority: P1
- Complexity: Medium
- Problem: Media/video has engine work, but live acceptance still needs proof
  that real mpv frames render to the Media stage on a seat and that library/art
  browsing works against a live source.
- Required outcome: Local video plays with visible frames, audio and controls on
  Eagle/test bed, and library browse/artwork paths work against the configured
  media service.
- Scope: mpv feature path, player stage, media library browser, artwork cache,
  live seat verification.
- Relevant files/components: `crates/desktop/mde-media-egui/`,
  `crates/desktop/mde-media-core/`, `crates/desktop/mde-shell-egui/`.
- Dependencies: Seat with libmpv and media library source.
- Acceptance criteria: Video frames advance, controls work, browse/artwork show
  real data, and missing engine paths show honest gated states.
- Verification method: Live seat smoke plus focused media tests.
- Origin or merged source IDs: BUG-VIDEO-1, MEDIA-VIDEO, MUSIC-BROWSE/ART,
  old worklist lines 3254, 6198, 1449.


### WL-FUNC-009 - Sunshine/Moonlight shadowing of the Magic Mesh shell

- Disposition: CODE-DONE + PARKED TAIL (policy/plan/helper/firewall/systemd/indicator scaffolding done; live DRM Workstation seat + hardware encode proof gated)
- Priority: P1
- Complexity: Large
- Problem: Magic Mesh can broker guest/VM desktop consoles, but there is no
  tracked path for shadowing the actual host egui/DRM shell from another device.
  The requested design is a Moonlight client connecting over the encrypted mesh
  to a Sunshine service on a Workstation, where Sunshine captures the Magic Mesh
  DRM/KMS desktop, hardware-encodes frames, and injects remote keyboard/mouse
  input back into the shell seat.
- Required outcome: A paired Moonlight client can view and control the live Magic
  Mesh shell desktop through Sunshine with an explicit operator exposure mode
  switch, local-user authorization, visible on-seat shadowing state, bounded
  input injection, and honest degraded states when capture or hardware encode is
  unavailable.
- Scope: Sunshine packaging/provisioning, Workstation service lifecycle,
  operator-selectable exposure (`mesh-only`, `lan`, and explicit
  `all-interfaces/public` with warning), Moonlight pairing and access policy,
  native on-seat pairing prompt, DRM/KMS capture permission, hardware encoder
  selection, remote input handoff, local indicator/kill switch, audit events,
  and live-seat validation.
- Relevant files/components: packaging/RPM assets and systemd units,
  `crates/desktop/mde-shell-egui/src/`, `crates/shared/mde-egui/src/drm.rs`,
  `crates/mesh/mackesd/src/workers/seat_remote_input.rs`,
  `install-helpers/seat-remote-input.py`, Device Manager, notification/indicator
  surfaces, `docs/THREAT_MODEL.md`, `docs/BUILD-ENVIRONMENT.md`.
- Dependencies: DRM-capable Workstation seat, supported hardware encoder, a
  Moonlight client, Sunshine availability/licensing review, WL-SEC-004 local
  remote-input authorization/indicator design, and mesh firewall/exposure policy.
- Acceptance criteria: Sunshine is installed or honestly gated on Workstation
  builds only; the service has a durable exposure switch and defaults to a
  conservative non-public bind; `mesh-only`, `lan`, and explicit
  `all-interfaces/public` modes map to Sunshine bind/origin/firewall policy
  without changing the rest of the feature; pairing raises a native Magic Mesh
  shell prompt that names the requesting client and requires local approval; the
  shell displays a persistent shadowing indicator with disconnect/kill control;
  Moonlight receives nonblank advancing frames from the Magic Mesh shell; remote
  keyboard/mouse events reach the shell only while authorized; disconnect revokes
  input and stops capture; audit/state publishes show active, denied,
  disconnected, and degraded modes.
- Verification method: Unit tests for policy/state/audit decisions, packaging
  tests proving Sunshine assets are Workstation-only, farm build checks, and live
  `.15` or spare-seat proof with a Moonlight client showing frame motion,
  hardware encoder use, input round-trip, indicator visibility, and exposure
  switch reachability for at least `mesh-only` and `lan`.
- Origin or merged source IDs: Operator request 2026-07-17, WL-CRIT-001,
  WL-SEC-004, WL-RUN-005, WL-PERF-002.
- Current evidence: On 2026-07-17 `.15` had the official Fedora 44 Sunshine RPM
  installed, Moonlight installed as a user Flatpak, `mm` added to `video`,
  `input`, and `render`, `/usr/bin/sunshine` granted `cap_sys_admin=p`,
  Sunshine configured with `capture = kms`, `encoder = vaapi`, `upnp =
  disabled`, `minimum_fps_target = 30`, and first proved on the mesh address
  `10.42.0.8`.
  After restarting the user manager, Sunshine started without the prior
  PipeWire CPU loop, opened `10.42.0.8:{47984,47989,47990,48010}`, reported KMS
  DRM capture on `i915`, found the DRM monitor/cursor plane, and found Intel
  i965 H.264/HEVC VAAPI encoders. After the operator reported Moonlight could
  not connect to the mesh-only address, `.15` was switched to LAN mode with
  `bind_address = 172.20.0.15`; firewalld runtime and permanent rules were added
  to the active `public`, `trusted`, and default zones for TCP
  `47984,47989,47990,48010` and UDP `47998-48010`; this dev host then proved
  `https://172.20.0.15:47990` returned `401` and TCP `47984`, `47989`, and
  `48010` accepted connections. The Moonlight PIN `8602` was accepted by
  Sunshine via `POST /api/pin` with `{"status":true}`. Credentials are stored
  on `.15` at
  `/home/mm/.config/sunshine/mde-proof-creds.txt` with mode `0600`. Remaining
  proof is a real Moonlight client pairing with advancing frames, input
  round-trip, shell indicator, disconnect revocation, and the exposure switch
  implemented in product code rather than a hand-edited Sunshine config.
  A later 2026-07-17 Settings integration pass added a render-free Remote
  Proofing service plan derived from the persisted Settings policy and displayed
  that effective plan in Mesh & System -> Remote Proofing. The plan maps
  disabled, mesh-only, LAN, and all-interface exposure to explicit Sunshine bind
  scope, firewall policy, capture backend, encoder backend, FPS floor, approval,
  indicator, remote-input, VNC fallback, and degraded-warning state. BigBoy
  `.130` passed `cargo fmt -p mde-shell-egui --check`, focused
  `remote_proofing` policy coverage, and
  `selecting_each_section_routes_the_detail_pane_and_paints`. A subsequent
  2026-07-17 bridge pass added the packaged
  `/usr/libexec/mackesd/mde-remote-proofing-apply` helper plus the
  `mde-remote-proofing-plan.{path,service}` Workstation-gated systemd watcher.
  The helper consumes `/run/mde-bus/settings-remote-proofing.json` and
  `/run/mde/mesh-status.json`, renders `/run/mde/remote-proofing/plan.json` and
  `/run/mde/remote-proofing/sunshine.conf`, models mesh/LAN/public firewall
  intent without opening ports, and defaults missing config to disabled. Local
  `py_compile`, helper `--self-test`, and fake-root `systemd-analyze verify`
  passed; BigBoy `.130` passed `cargo fmt -p mackesd --check` and the focused
  `full_rpm_ships_remote_proofing_bridge_but_server_variant_does_not` packaging
  test. A 2026-07-18 lifecycle pass extended the helper and unit to render
  `/run/mde/remote-proofing/lifecycle.json` alongside the plan/config. The
  lifecycle artifact names the `sunshine.service` user unit, desired
  stopped/ready/blocked state, bind scope/address, capture/encoder/FPS policy,
  firewall backend, ports, allowed sources, blockers, local approval,
  shadowing-indicator, remote-input, and VNC fallback controls, so the eventual
  supervisor can start/stop Sunshine and apply/remove firewalld rules without
  inferring state from comments. `.50` passed Python compile, helper
  `--self-test`, and fake-root `systemd-analyze verify`; BigBoy `.130` passed
  `cargo fmt -p mackesd --check`; `.90` passed the focused
  `full_rpm_ships_remote_proofing_bridge_but_server_variant_does_not` packaging
  test; `.170` passed the four focused `remote_proofing` Settings policy tests.
  A follow-up 2026-07-18 helper cleanup moved Magic Mesh-only state out of the
  generated `sunshine.conf` and into `lifecycle.json`, leaving the Sunshine
  config output to Sunshine-recognized keys (`upnp`, `capture`, `encoder`,
  `minimum_fps_target`, `address_family`, `origin_web_ui_allowed`, and optional
  `bind_address`). `.50` passed Python compile, helper `--self-test`, and
  fake-root `systemd-analyze verify` after that cleanup.
  A later 2026-07-18 supervisor pass wired the packaged Workstation unit to call
  `--apply-lifecycle`. The helper now treats a missing Settings policy as
  unmanaged/no-op, resolves only a regular `/home` desktop user (or a valid
  override), writes the generated Sunshine config to that user's
  `~/.config/sunshine/sunshine.conf` with a one-time backup, reconciles only
  Magic Mesh-owned firewalld rich rules recorded in
  `/var/lib/mde/remote-proofing/firewalld-state.json`, fail-closes Sunshine
  startup if firewall reconciliation fails, and restarts/stops the user
  `sunshine.service` through `runuser ... systemctl --user`. Verification:
  `.50` passed Python compile, helper `--self-test`, a structured `--apply-dry-run`
  proving mesh-scoped firewalld commands plus `mm` user-service restart, and
  fake-root `systemd-analyze verify`; BigBoy `.130` passed
  `cargo fmt -p mackesd --check`; `.90` passed the focused
  `full_rpm_ships_remote_proofing_bridge_but_server_variant_does_not` packaging
  test proving the unit ships with `--apply-lifecycle`. A follow-up LAN-mode fix
  made apply/dry-run apply resolve trusted-LAN exposure
  from the mesh snapshot's default gateway via `ip -j route get`, derive the
  bound local address and source CIDR from `ip -j addr`, remove the unresolved
  LAN blockers/notes, render the resolved `bind_address`, and apply owned
  firewalld rich rules scoped to that CIDR before restarting Sunshine. Local and
  `.50` verification passed Python compile, helper `--self-test`, structured LAN
  `--apply-dry-run` lifecycle assertions, and fake-root `systemd-analyze verify`.
  Live `.15` proof on 2026-07-18 then exposed and fixed a real path-unit loop:
  the watcher no longer uses level-triggered `PathExists=/run/mde-bus`, and the
  package regression forbids reintroducing a `[Path]` `PathExists=` trigger.
  The helper also now syncs the summary plan from the resolved lifecycle so
  `/run/mde/remote-proofing/plan.json` shows the effective LAN bind/source CIDR
  instead of the pre-resolution placeholder. Corrected Fedora 44 split RPMs were
  rebuilt on BigBoy `.130` with size guards passing (base 72.8 MiB, Browser
  39.0 MiB), transaction-tested and installed on `.15`, and `rpm -V
  magic-mesh magic-mesh-browser` returned clean. The installed helper hash
  matched source, the installed path unit has only `PathChanged=` triggers, the
  one-shot settled inactive/success, the path watcher settled active/waiting,
  `/run/mde/remote-proofing/{plan.json,lifecycle.json}` resolved LAN to
  `172.20.0.15` and `172.20.0.0/16` with no blockers, firewalld rich rules are
  scoped to `172.20.0.0/16`, and Sunshine is active/listening on
  `172.20.0.15:{47984,47989,47990,48010}`. Farm evidence: `.50` passed Python
  compile, helper `--self-test`, and corrected fake-root `systemd-analyze
  verify`; `.90` passed the focused
  `full_rpm_ships_remote_proofing_bridge_but_server_variant_does_not`
  regression. A later 2026-07-18 shell-status pass wired the existing daemon
  `state/seat/remote-input/{local-node}` retained indicator into the bottom
  status rail as a `Remote control` segment. The shell now polls only the local
  node's armed/active record, paints an obvious status pip, exposes a detail-row
  and AccessKit value naming the controlling source/client, and routes the pip
  through System/Settings instead of creating a second control surface. Farm
  evidence: `.50` passed `cargo fmt -p mde-shell-egui --check`; BigBoy `.130`
  passed the focused `remote_control_indicator_poll_feeds_local_status_segment`;
  `.90` passed `the_status_segment_pips_route_to_their_surfaces`; `.170` passed
  `status_bar_exports_accesskit_live_region_and_named_pips`. A same-day `.15`
  bounce fix made the lifecycle
  apply path idempotent: unchanged generated user configs no longer restart an
  active Sunshine service, while failed/stopped services recover with
  `reset-failed` plus `start`. The installed helper passed `--self-test`;
  Sunshine recovered from `start-limit-hit` to active/running, stayed on the
  same PID/invocation across planner runs at `18:38:56`, `18:39:26`,
  `18:39:57`, and `18:40:27`, `--print-apply-result` reported
  `config_changed=false`, `service_action=unchanged`, and `firewall.changed=false`,
  and `https://172.20.0.15:47990` returned `401`.
  Remaining work is
  native shell pairing/approval, actual Sunshine client-attached shadowing
  state, Moonlight frame motion, input round-trip, disconnect revocation, and
  exposure-switch live proof.


### WL-FUNC-010 - Native Maps & Location workspace and offline navigation readiness

- Disposition: CODE-DONE + PARKED TAIL (simulator/guardrails/offline-map/MG90/tessellation complete + farm-verified; only real live GNSS hardware remains)
- Priority: P2
- Complexity: Large
- Problem: The user-directed Maps & Location surface needs a native egui
  offline navigation, location-source, and MG90 management experience that is
  useful without MG90 hardware while staying honest about real adapter gaps.
- Required outcome: The shell exposes a native Maps & Location workspace with
  simulator-backed drive/map/routing/location-source/MG90 setup surfaces,
  render-agnostic readiness models, offline-map state, manual source selection,
  and no browser wrapper or fake hardware calls.
- Scope: `mde-maps-location-egui`, simulator scenarios, offline map status,
  location-source health, MG90 setup/settings/firmware guardrails, route/trip
  state, and later real adapter seams for MG90, gpsd, Valhalla, Nominatim, CAN,
  GPIO, serial recovery, firmware upload, and encrypted local vault storage.
- Relevant files/components: `crates/desktop/mde-maps-location-egui/`,
  shell surface registration, future MG90/gpsd/routing/geocoder/provider
  adapters.
- Dependencies: Real MG90 hardware, gpsd device, routing/geocoder daemons, and
  vehicle/CAN fixtures for full live acceptance; simulator and offline-map
  readiness logic remains testable without hardware.
- Current evidence: A 2026-07-18 Maps & Location readiness slice added a
  render-agnostic offline-navigation status projection over the selected
  location source, loaded offline map region, storage cap, local routing and
  geocoder provider contracts, MG90 setup step, and optional traffic/weather/
  satellite notes. Drive, Map, and Simulator tabs now render the readiness card;
  Simulator exposes stale-primary, missing-map-bundle, and restore-ready
  scenario buttons against the same model. Farm evidence: `.50`
  `cargo fmt -p mde-maps-location-egui -- --check` passed; `.90`
  `cargo test -p mde-maps-location-egui -- --nocapture` passed 14 tests.
  A later 2026-07-18 Maps & Location dead-zone slice classified the active MG90
  cellular link into weak/degraded/outage route-risk states, records dead zones
  from the selected primary location sample and current MG90 telemetry, exposes
  the recorder in Routes & Trips plus a Simulator scenario button, and refreshes
  the route-risk summary from recorded severities. Farm evidence: `.90` slot
  `mapsloc-dz` `cargo fmt -p mde-maps-location-egui -- --check` passed; `.50`
  slot `maps-dead-zone`
  `cargo test -p mde-maps-location-egui dead_zone -- --nocapture` passed 2
  tests; BigBoy `.130` slot `mapsloc-dz-render` and `.90` slot `maps-sim-ui`
  both passed the focused simulator tessellation proof. A later 2026-07-18
  manual-switch readiness slice made primary location switching require a
  connected, fresh, 5-meter-or-better source, removed invalid peers from healthy
  alternatives, reports primary source status failures even when the last sample
  itself looks healthy, and disables invalid `Make primary` actions in the
  Location Sources tab while showing switch-readiness text. Farm evidence: `.50`
  `cargo fmt -p mde-maps-location-egui -- --check` passed; `.90`
  `cargo test -p mde-maps-location-egui switch -- --nocapture` passed 2 tests;
  `.170` `cargo test -p mde-maps-location-egui primary_warning -- --nocapture`
  passed 1 test.
- Acceptance criteria: Offline turn-by-turn readiness is never claimed when the
  primary source is stale/unhealthy, no loaded offline map exists, storage
  exceeds the cap, local routing/geocoder contracts are unavailable, setup has
  not verified offline maps, or MG90 management is unauthenticated; healthy peer
  sources are offered as manual switches rather than auto-failover; every tab
  tessellates without hardware; real adapters replace simulator seams without
  changing the shell mount point.
- Verification method: Focused crate unit/render tests for readiness and
  simulator scenarios, shell embedding tests, then live MG90/gpsd/map/routing
  proof when hardware and daemons are available.
- Origin or merged source IDs: User directive 2026-07-18 Maps & Location hard
  epic.


### WL-UX-001 - Win10 hybrid bottom taskbar and tray live proof

- Disposition: CODE-DONE + PARKED TAIL (all geometry/overlap/tray-reachability logic implemented + test-covered; only a live seat visual sign-off remains)
- Priority: P2
- Complexity: Medium
- Problem: The Win10 hybrid taskbar/start/tray work has many completed slices,
  but the remaining tray composition and live visual proof are still gated.
- Required outcome: The bottom taskbar, start grid, tray, show-desktop nub, and
  action center render without overlap on a live seat and match the canonical
  Construct identity.
- Scope: Bottom bar geometry, tray/status area, action center, start grid,
  live-eye pass, and screenshots.
- Relevant files/components: `crates/desktop/mde-shell-egui/src/dock/mod.rs`,
  `crates/desktop/mde-shell-egui/src/start_menu.rs`, status/system modules.
- Dependencies: Live DRM seat for final visual proof.
- Acceptance criteria: No overlaps at supported resolutions; tray controls are
  reachable; live screenshots confirm layout.
- Verification method: Focused geometry tests and live seat screenshot/pixel
  inspection.
- Origin or merged source IDs: B5-rest, WIN10-HYBRID, old worklist line 4630.
- Evidence 2026-07-17: Start menu pinned/favorite tiles now persist through
  `start-menu.json` in the shell client-data directory; malformed, duplicate,
  unknown, and non-grid pins normalize on load. Live tray/screenshot proof remains
  the blocking tail for this item. A later 2026-07-17 Start-menu geometry pass
  moved the panel off the retired left-dock `DOCK_W` inset and back to the true
  screen-left edge, matching the bottom-taskbar-only architecture. A later
  2026-07-17 taskbar hover-preview pass added the static running-session preview
  with a real protocol badge above the taskbar; farm `.50` fmt and BigBoy `.130`
  focused `win10_hybrid_31_session_hover_preview_shows_protocol_badge` passed.
  A later 2026-07-17 taskbar live-thumbnail pass wired the current VDI desktop
  texture into that hover preview, preserving aspect ratio and matching only the
  intended broker/fallback rail entry; farm `.50` fmt, BigBoy `.130` focused
  `session_preview`, and the exact hover-card regression passed. A later
  2026-07-17 taskbar auto-hide settings pass made the already-tested dock
  auto-hide behavior reachable from the persisted Personalization appearance
  config and mirrored it into `DockState`; farm `.50` fmt, BigBoy `.130`
  focused `appearance`, and the edited legacy migration test passed.
  A later 2026-07-17 Start-menu pinned-layout pass bounded the pinned/grouped
  tile grid to the viewport above the fixed search field with a vertical scroll
  region, preventing pinned sections from painting into search; farm `.50` fmt
  and BigBoy `.130` focused pinned-layout coverage passed.
  A later 2026-07-17 source-comment hygiene pass aligned `main.rs` and
  `dock/mod.rs` with the bottom-taskbar-only architecture, removing stale live
  source prose that still described a mounted left vertical dock; farm `.50`
  fmt and BigBoy `.130` focused retired-gutter coverage passed.
  A later 2026-07-17 Start-menu source-doc pass removed stale placeholder and
  vertical-dock-launcher prose from `start_menu.rs` and the shell `Nav` comment,
  aligning the code comments with the shipped tile/search/pin Start Menu; farm
  `.50` fmt and BigBoy `.130` focused Start-menu grid coverage passed.
  A later 2026-07-17 Start-menu search-icon pass added reusable `ui-search` and
  `ui-close` line glyphs, rendered a leading search icon plus live query-clear
  icon button in the Start search field, and exposed the clear button to
  AccessKit; farm `.50` fmt, BigBoy `.130` focused clear-button coverage, and
  `.90` `mde-theme` icon rasterization coverage passed. A later 2026-07-17
  Start-search scroll pass bounded broad app/Console search results inside the
  pane above the fixed search field, added pixel proof that offscreen selected
  rows cannot paint into the field, and wrote
  `start-menu-search-results-scroll.png`; farm `.50` fmt and BigBoy `.130`
  focused `search_result` coverage passed, and the PNG was pulled to
  `/tmp/start-menu-search-scroll/` for visual inspection. A later 2026-07-17
  YAMIS icon migration pass added the new `assets/icons/YAMIS/YAMIS/` theme and
  moved the shared `mde-theme::brand::icons::IconId` resolver for the default
  platform surface/status/tray/action glyphs to YAMIS equivalents while keeping
  only the product mark/wordmark on brand assets; a later 2026-07-18 Construct
  brand pass replaced the Construct raster slots with Construct source artwork,
  Construct wallpaper set, Construct hicolor app icons, and Construct mark/wordmark
  sources;
  BigBoy `.130` focused `cargo fmt -p mde-theme --check` and
  `cargo test -p mde-theme icons -- --nocapture` passed. A later 2026-07-17
  packaging pass made YAMIS the installed default freedesktop icon theme for the
  full workstation RPM (`/usr/share/icons/YAMIS` plus GTK 3/4 default
  `gtk-icon-theme-name=YAMIS`) and added manifest coverage for the payload and
  post-install cache refresh. A later 2026-07-17 Browser chrome icon pass
  expanded the shared `IconId` catalog with YAMIS action glyphs and made
  Browser toolbar, options, drawer, and context-menu icon painting prefer
  YAMIS-backed textures for direct equivalents while retaining the existing
  hand-painted fallback for unmatched controls. A follow-up Browser icon pass
  added direct YAMIS-backed coverage for reload, stop/cancel, engine/internet,
  edit, and view glyphs, leaving only zoom and compact stepper glyphs on the
  Browser fallback painter; farm `.130`/`.90` fmt checks, `.50` `mde-theme` icon
  rasterization coverage, and `.170` focused Browser icon-mapping coverage
  passed. A later 2026-07-17 YAMIS completion pass added shared
  `list-remove`, `zoom-in`, and `zoom-out` currentColor action glyphs to the
  YAMIS tree, exposed them as `IconId::Remove`, `IconId::ZoomIn`, and
  `IconId::ZoomOut`, and mapped the Browser zoom/compact-minus controls through
  the YAMIS-backed icon texture path; BigBoy `.130` `mde-theme` icon
  rasterization coverage, `.90` focused Browser icon-mapping coverage, and
  `.50` fmt passed. A follow-up bottom-taskbar icon pass added a shared
  `more-horizontal` currentColor YAMIS glyph, exposed it as
  `IconId::MoreHorizontal`, and replaced the session-overflow More cell's
  painted text ellipsis with the shared icon texture path; BigBoy `.130`
  focused `win7_7_the_session_overflow_more_cell_reports_the_real_hidden_count`,
  `.90` `mde-theme` icon rasterization coverage, and `.50` fmt passed.
  A 2026-07-18 Start-menu chrome-copy pass moved the visible Start search
  placeholder to ASCII copy (`Search apps and commands...`) and added painted
  text coverage proving the rendered search field no longer emits a Unicode
  ellipsis; BigBoy `.130` focused
  `start_menu_search_hint_uses_ascii_chrome_copy` and `.50` fmt passed.
  A follow-up 2026-07-18 taskbar chrome-copy pass changed long running-session
  label truncation from a Unicode ellipsis to ASCII `...`, covering the shared
  helper used by session rail entries and hover/accessibility labels; BigBoy
  `.130` focused `taskbar_session_label_truncation_uses_ascii_ellipsis` and
  `.50` fmt passed.
  A later 2026-07-18 taskbar icon cleanup removed the retired Start-bar pin from
  `DockState` and the live `IconId::TRAY` subset, preserving the corrected
  white-on-black taskbar icon path and the distinct Desktop Sources/Health glyphs;
  farm `.50` file-scoped rustfmt passed, `.90` focused
  `tray_glyphs_rasterize_nonempty_at_16_and_24` passed, and BigBoy `.130` focused
  `taskbar_launch_sources_health_and_overflow_use_distinct_non_chevron_icons`
  passed. Integrated touched-package fmt
  (`cargo fmt -p mde-shell-egui -p mde-theme -p mackesd -- --check`) passed on
  `.50` after the concurrent Browser drawer slice landed.
  A later 2026-07-18 Chat/Settings chrome-copy pass replaced visible
  checkmark/arrow/paperclip/Unicode-ellipsis pseudo-icons in Chat delivery
  notes, file-send copy, alert action rows, composers, status/room hints, and
  menu captions with ASCII labels, and moved Settings loading copy plus display
  nudge controls to ASCII/YAMIS-backed icon paths; BigBoy `.130` focused
  `copy_uses_ascii`, `.90` focused
  `settings_chrome_copy_is_ascii_and_nudges_use_yamis_icons`, and `.50` fmt
  passed. A later 2026-07-18 Settings hover-polish slice replaced the display
  nudge controls' raw egui hover text with a Settings themed tooltip surface and
  rendered text-color coverage so icon hovers cannot regress into unreadable
  shared-shell popup text. A later 2026-07-19 OSK tooltip polish slice replaced
  the on-screen keyboard toggle's raw egui hover text with a keyboard-themed
  tooltip frame using active `Style` text/surface/border tokens and rendered
  coverage against raw black popup text. Farm evidence: `.90` slot
  `start-menu-light-style-test`
  `cargo test -p mde-shell-egui osk_toggle_tooltip -- --nocapture` passed; `.50`
  file-scoped `rustfmt --edition 2024 --config skip_children=true --check` for
  `crates/desktop/mde-shell-egui/src/keyboard.rs` passed. A follow-up
  2026-07-19 shell tooltip readability slice replaced the Timers disabled Start
  hover and Phones Hub destructive Unpair hover with local themed tooltip frames
  that resolve active `Style` text/surface/border colors, with light-mode
  rendered coverage proving text and surface stay distinct. Farm evidence: `.90`
  slot `timers-tooltip-test2`
  `cargo test -p mde-shell-egui disabled_start_tooltip -- --nocapture` passed;
  `.170` slot `phones-tooltip-test2`
  `cargo test -p mde-shell-egui unpair_hover_tooltip -- --nocapture` passed; `.50`
  slot `tooltip-fmt2` file-scoped
  `rustfmt --edition 2024 --config skip_children=true --check` for
  `crates/desktop/mde-shell-egui/src/timers.rs` and
  `crates/desktop/mde-shell-egui/src/phones_hub.rs` passed. A follow-up
  2026-07-19 Datacenter tooltip/provider-copy slice routed Fleet KVM service
  and cloud-owned VM row hovers through a Datacenter-themed tooltip frame,
  replaced visible `Nova-managed` VM badges and warnings with provider-neutral
  `Cloud-managed` copy, and left the Nova/libvirt detector internal. Farm
  evidence: `.90` slot `datacenter-tooltip-test`
  `cargo test -p mde-shell-egui datacenter_hover_tooltip -- --nocapture` passed;
  `.170` slot `datacenter-cloud-copy-test`
  `cargo test -p mde-shell-egui cloud_managed_vm_badge -- --nocapture` passed;
  `.50` slot `datacenter-tooltip-fmt2` file-scoped
  `rustfmt --edition 2024 --config skip_children=true --check` for
  `crates/desktop/mde-shell-egui/src/datacenter.rs` passed. A follow-up
  2026-07-19 panel tooltip readability slice routed the standalone
  `mde-panel-egui` mesh-health pip hover through a panel-themed tooltip frame
  using active `Style` text/surface/border colors, with light-mode rendered
  coverage. Farm evidence: `.90` slot `panel-tooltip-test`
  `cargo test -p mde-panel-egui panel_pip_tooltip -- --nocapture` passed; `.50`
  slot `panel-tooltip-fmt2` file-scoped
  `rustfmt --edition 2024 --config skip_children=true --check` for
  `crates/desktop/mde-panel-egui/src/main.rs` passed. A follow-up 2026-07-19
  Editor toolbar tooltip readability slice routed the Standard toolbar and
  Formatting toolbar hovers through local Editor-themed tooltip frames using
  active `Style` text/surface/border colors, with light-mode rendered coverage
  for both toolbar rows and existing compact-bar behavior preserved. Farm
  evidence: `.90` slot `editor-tooltip-tests`
  `cargo test -p mde-editor-egui tooltip -- --nocapture` passed; `.170` slot
  `editor-bars-tests` `cargo test -p mde-editor-egui bars -- --nocapture`
  passed; `.50` slot `editor-tooltip-fmt2` file-scoped
  `rustfmt --edition 2024 --config skip_children=true --check` for
  `crates/desktop/mde-editor-egui/src/toolbar.rs` and
  `crates/desktop/mde-editor-egui/src/format_bar.rs` passed. A follow-up
  2026-07-19 Terminal tooltip readability slice added a shared
  `mde-term-egui` Terminal tooltip helper and routed tmux toolbar/tab-template
  hovers, Terminal tab-bar utility hovers, and saved-layout launch hovers through
  themed `Style` text/surface/border colors. A residual raw-hover sweep across
  `crates/desktop/mde-term-egui/src` now finds no direct
  `on_hover_text` / `on_disabled_hover_text` call sites. Farm evidence: `.90`
  slot `term-tooltip-test`
  `cargo test -p mde-term-egui tooltip -- --nocapture` passed; `.170` slot
  `term-tabs-toggle-test` `cargo test -p mde-term-egui toggle -- --nocapture`
  passed 14 tests; `.90` slot `term-tmux-chrome-final`
  `cargo test -p mde-term-egui toolbar_and_status_bar_render_headless -- --nocapture`
  passed; `.50` slot `term-tooltip-fmt2`
  `cargo fmt -p mde-term-egui -- --check` passed. A follow-up 2026-07-19
  Terminal refined-height slice aligned the first-party Terminal tab strip and
  tmux status bar to the shared `mde_egui::menubar::BAR_HEIGHT`, removing the
  old 32pt local bands while preserving the existing toolbar/status render path.
  Farm evidence: `.90` slot `term-refined-height`
  `cargo test -p mde-term-egui refined_shared_chrome_height -- --nocapture`
  passed 2 focused height tests; `.170` slot `term-refined-render`
  `cargo test -p mde-term-egui toolbar_and_status_bar_render_headless -- --nocapture`
  passed; `.50` slot `term-refined-height-fmt` file-scoped
  `rustfmt --edition 2021 --check` passed for `tabs.rs` and `tmux_ui.rs`.
  A follow-up 2026-07-19
  Terminal tmux context-menu popup slice added Terminal-local popup visuals,
  routed tmux window/sidebar/pane/tab context menus through them, wrapped the
  nested `Join Into Window` menu, and covered dark/light menu text tokens so the
  popup path follows the same refined chrome/readability contract as the
  toolbars. Farm evidence: `.130`
  `cargo test -p mde-term-egui tmux_context_menu_popup -- --nocapture` passed;
  `.50` file-scoped
  `rustfmt --edition 2024 --config skip_children=true --check crates/desktop/mde-term-egui/src/tmux_ui.rs`
  passed. A follow-up 2026-07-19 Terminal grid selection-menu popup slice routed
  the actual terminal widget right-click selection menu through Terminal-local
  popup visuals, resolved caption text through the active light/dark palette,
  and covered the rendered menu body so mesh-action rows cannot regress to raw
  egui popup text. Farm evidence: `.90` slot `term-grid-menu-test`
  `cargo test -p mde-term-egui terminal_selection -- --nocapture` passed 2
  focused tests; `.50` slot `term-grid-menu-fmt` file-scoped
  `rustfmt --edition 2021 --config skip_children=true --check crates/desktop/mde-term-egui/src/widget.rs`
  passed. A follow-up 2026-07-19 Editor overflow-popup readability slice added
  an Editor-local popup visual scope and routed the Standard toolbar `Zoom`
  overflow plus Formatting toolbar `Paragraph style` overflow through it,
  resolving caption text and row states from the active light/dark palette
  instead of raw egui menu defaults. Farm evidence: `.90` slot
  `editor-overflow-toolbar`
  `cargo test -p mde-editor-egui toolbar_overflow -- --nocapture` passed;
  BigBoy `.130` slot `editor-overflow-format`
  `cargo test -p mde-editor-egui format_bar_overflow -- --nocapture` passed;
  `.170` slot `editor-popup-style`
  `cargo test -p mde-editor-egui editor_popup_visuals -- --nocapture` passed;
  `.50` slot `editor-overflow-fmt` file-scoped
  `rustfmt --edition 2021 --config skip_children=true --check` passed for
  `mde-editor-egui/src/tooltip.rs`, `toolbar.rs`, and `format_bar.rs`.
  A follow-up 2026-07-19 shared
  chrome density slice made ordinary toolbar button padding slimmer without
  shrinking the pointer hit target, reduced shared menu text by one point,
  reduced the top-left shared workspace title by two points, added a shared
  near-zero toolbar/header vertical inset, removed the extra vertical padding
  around the Files shared menu bar, and applied the refined inset to Files,
  Bookmarks, and Editor top toolbar/header strips. Farm evidence: BigBoy `.130`
  slot
  `shared-refined-chrome`
  `cargo test -p mde-egui refined -- --nocapture` passed 2 typography/density
  tests; `.90` slot `editor-refined-toolbar`
  `cargo test -p mde-editor-egui toolbar -- --nocapture` passed 8 toolbar tests;
  `.50` slot `files-refined-toolbar`
  `cargo test -p mde-files-egui files_navigation_toolbar_uses_yamis_icons -- --nocapture`
  passed; `.170` slot `bookmarks-refined-header`
  `cargo test -p mde-bookmarks-egui renders_the_empty_first_run_state -- --nocapture`
  passed; file-scoped farm `rustfmt --edition 2021 --check` passed for
  `mde-egui/src/style.rs`, `mde-files-egui/src/view.rs`,
  `mde-bookmarks-egui/src/view.rs`, and `mde-editor-egui/src/panel/mod.rs`.
  A follow-up 2026-07-19 Editor residual-hover readability slice added a shared
  `mde-editor-egui` tooltip helper and routed search, outline, follow banner,
  diagnostic/spelling hit regions, pane/tab chrome, and spell-control hovers
  through themed Editor tooltip surfaces. A residual raw-hover sweep across
  `crates/desktop/mde-editor-egui/src` now finds no direct `on_hover_text` /
  `on_disabled_hover_text` call sites. Farm evidence: `.90` slot
  `editor-shared-tooltip-test2`
  `cargo test -p mde-editor-egui tooltip -- --nocapture` passed; `.170` slot
  `editor-search-hover-test2`
  `cargo test -p mde-editor-egui search -- --nocapture` passed 14 tests; `.50`
  slot `editor-tooltip-fmt5` `cargo fmt -p mde-editor-egui -- --check` passed.
  A follow-up 2026-07-19 shared tooltip-margin refinement slice added
  `Style::tooltip_margin()` as the single compact 8x4 hover-card frame margin,
  removed the remaining thicker 10x7 tooltip frames, and routed themed tooltip
  helpers in Shell, Browser chrome, Files, Editor, Terminal, Media, Panel,
  Remote Sessions, Device Manager, Explorer, Phones, Storage, Timers,
  Datacenter, Keyboard, and Settings through the shared token. Residual scan
  evidence finds no `Margin::symmetric(10, 7)` under desktop/shared Rust
  surfaces. Farm evidence: `.90` slot `shared-tooltip-style`
  `cargo test -p mde-egui tooltip_margin -- --nocapture` passed; BigBoy `.130`
  slot `shared-tooltip-shell`
  `cargo test -p mde-shell-egui tooltip -- --nocapture` passed 14 shell/browser
  rendered tooltip tests; `.170` slot `shared-tooltip-media`
  `cargo test -p mde-media-egui tooltip -- --nocapture` passed; `.90` slot
  `shared-tooltip-editor` `cargo test -p mde-editor-egui tooltip -- --nocapture`
  passed 3 tests; `.170` slot `shared-tooltip-term`
  `cargo test -p mde-term-egui tooltip -- --nocapture` passed; `.50` slot
  `shared-tooltip-files`
  `cargo test -p mde-files-egui files_hover_tooltip -- --nocapture` passed;
  BigBoy `.130` slot `shared-tooltip-panel`
  `cargo test -p mde-panel-egui panel_pip_tooltip -- --nocapture` passed; `.50`
  file-scoped `rustfmt --edition 2021 --config skip_children=true --check`
  passed for the touched tooltip/style files after an intentionally broader
  package fmt check exposed unrelated package-level drift.
  A follow-up 2026-07-19 IaC Heat toolbar density slice replaced the raw egui
  Heat toolbar buttons with a compact shared `Style::toolbar_margin()` strip and
  `heat_toolbar_button` primitive using `Style::SMALL` text, bounded widths,
  shared surface/border tokens, and focus-ring painting while preserving the
  reverse-generate and new-stack state seams. Farm evidence: BigBoy `.130` slot
  `iac-heat-toolbar-test`
  `cargo test -p mde-shell-egui heat_toolbar -- --nocapture` passed 2 focused
  tests; `.50` slot `iac-heat-toolbar-fmt` file-scoped
  `rustfmt --edition 2021 --config skip_children=true --check` passed for
  `crates/desktop/mde-shell-egui/src/iac/mod.rs` and
  `crates/desktop/mde-shell-egui/src/iac/tests.rs`; local `git diff --check`
  passed for the touched IaC files. A later 2026-07-19 IaC refined-height
  correction made `HEAT_TOOLBAR_BUTTON_H` resolve directly to
  `Style::TOOLBAR_CONTROL_H` instead of the old 24pt `Style::SP_L`, tightened
  `heat_toolbar_uses_refined_shared_chrome_metrics` to assert that exact shared
  token and the below-24pt bound, and left the provider-neutral IaC copy/seams
  intact. Farm evidence: `.90` slot `iac-density-test`
  `cargo test -p mde-shell-egui heat_toolbar_uses_refined_shared_chrome_metrics -- --nocapture`
  passed; `.50` slot `iac-density-filefmt` file-scoped
  `rustfmt --edition 2021 --config skip_children=true --check` passed for
  `iac/mod.rs` and `iac/tests.rs`; local `git diff --check` passed for the
  touched IaC files. A package-wide fmt check was intentionally not used as the
  status gate after it exposed unrelated dirty formatting drift in `main.rs` and
  `power_settings.rs`. A follow-up 2026-07-19 uniform chrome
  density slice applied the same refined `Style::toolbar_margin()` path to the
  Explorer summary/filter/search/bulk-action/filmstrip chrome strips, while
  preserving body panel spacing; the shared `mde-egui` title/menu/button
  density tests remain the governing typography contract for all shared
  workspace menubars. Farm evidence: BigBoy `.130` slot `egui-density`
  `cargo test -p mde-egui refined -- --nocapture` passed 2 tests; `.90` slot
  `explorer-density`
  `cargo test -p mde-shell-egui explorer_chrome_strips_use_refined_toolbar_margin -- --nocapture`
  passed; `.50` file-scoped farm `rustfmt --edition 2021 --config
  skip_children=true --check` passed for the shared style/menubar, Explorer,
  Editor, Files, and IaC density files after a broader package fmt check exposed
  unrelated pre-existing formatting drift in `start_menu.rs`,
  `mde-egui/src/lib.rs`, and `iac/tests.rs`.
  A follow-up 2026-07-19 Browser refined-margin slice removed remaining thick
  hard-coded Browser chrome insets from Options category/command cards, the
  new-tab dashboard search pill, and Browser permission/passkey prompt bars,
  replacing them with named compact Browser margin helpers and a unit guard.
  Farm evidence: BigBoy `.130` slot `browser-refined-margins`
  `cargo test -p mde-shell-egui
  browser_chrome_transient_surfaces_use_refined_margins -- --nocapture` passed;
  `.90` slot `browser-dashboard-margin`
  `cargo test -p mde-shell-egui
  browser_new_tab_dashboard_uses_bing_style_search_language_and_centering --
  --nocapture` passed; `.170` slot `browser-prompt-margin`
  `cargo test -p mde-shell-egui
  browser_prompt_bars_use_material_action_buttons -- --nocapture` passed; `.50`
  slot `browser-refined-margin-fmt` file-scoped
  `rustfmt --edition 2021 --config skip_children=true --check` passed for
  `crates/desktop/mde-shell-egui/src/web/chrome_ui/mod.rs`. A follow-up
  2026-07-19 Browser bookmark-bar clipping slice clipped bookmark title paint
  to each bookmark button's text rect, so long bookmark names cannot overpaint
  adjacent bookmark buttons or overflow the Browser chrome, and updated adjacent
  icon regressions to accept either Browser vector fallback icons or YAMIS image
  icons. Farm evidence: BigBoy `.130` slot `browser-bookmark-clip`
  `cargo test -p mde-shell-egui browser_bookmark_bar_long_titles_clip_to_bookmark_button -- --nocapture`
  passed; `.90` slot `browser-bookmark-adjacent-2`
  `cargo test -p mde-shell-egui browser_bookmark -- --nocapture` passed 9
  focused bookmark tests; `.50` slot `browser-bookmark-clip-fmt-2`
  file-scoped
  `rustfmt --edition 2021 --config skip_children=true --check crates/desktop/mde-shell-egui/src/web/chrome_ui/mod.rs`
  passed.
  A follow-up 2026-07-19 refined chrome verification slice rechecked the current
  shared density contract after the operator asked for uniformly slimmer
  toolbars, one-point-smaller menu text, and two-point-smaller top-left
  workspace titles. The active contract is `Style::CONTROL_PAD_Y`,
  `Style::TOOLBAR_INSET_Y`, `Style::MENU_TEXT`, and `Style::WORKSPACE_TITLE`,
  consumed by the shared menubar and the toolbar surfaces that have already been
  migrated to `Style::toolbar_margin()`. Farm evidence: `.50` slot
  `refined-chrome-fmt` file-scoped `rustfmt --edition 2021 --check` passed for
  the shared style/menubar and representative shell, Files, chooser, Device
  Manager, Explorer, and Editor toolbar files; `.90` slot
  `refined-chrome-shared`
  `cargo test -p mde-egui refined -- --nocapture` passed 2 typography/density
  tests; BigBoy `.130` slot `shell-remote-fallback-refined`
  `cargo test -p mde-shell-egui shell_remote_sessions_fallback -- --nocapture`
  passed 4 tests; BigBoy `.130` slot `shell-refined-toolbar`
  `cargo test -p mde-shell-egui refined_toolbar -- --nocapture` passed the
  Explorer refined chrome-strip test and
  `cargo test -p mde-shell-egui refined_shared_chrome_metrics -- --nocapture`
  passed the IaC Heat shared chrome metrics test; `.90` slot
  `files-refined-popup`
  `cargo test -p mde-files-egui context_menu_visuals_use_themed_text_and_surface -- --nocapture`
  passed the Files popup/text/padding test. A follow-up 2026-07-19 Browser
  control-height slice trimmed Browser-owned toolbar buttons, horizontal tabs,
  and the location-bar frame while preserving the enlarged omnibox text from the
  earlier location-bar usability fix; the governing regression is
  `browser_omnibox_uses_readable_location_bar_metrics`. Farm evidence: BigBoy
  `.130` slot `browser-refined-height`
  `cargo test -p mde-shell-egui browser_omnibox_uses_readable_location_bar_metrics -- --nocapture`
  passed; `.90` slot `shared-refined-contract`
  `cargo test -p mde-egui refined -- --nocapture` passed 2 typography/density
  tests; `.50` slot `browser-refined-height-fmt` file-scoped
  `rustfmt --edition 2021 --check` passed for the touched Browser files; local
  `install-helpers/lint-style-leaks.sh` and scoped `git diff --check` passed.
  A follow-up 2026-07-19 Browser drawer control-height slice tied the Browser
  drawer text buttons, icon buttons, status icons, toggles, selector chips, and
  inline separators to the Browser-local `CHROME_BUTTON` 21pt chrome metric,
  removing the remaining 24pt drawer-control height literals while keeping the
  rendered print-drawer token path intact. Farm evidence: `.90` slot
  `browser-drawer-height`
  `cargo test -p mde-shell-egui browser_drawer_controls_use_refined_chrome_height -- --nocapture`
  passed; BigBoy `.130` slot `browser-drawer-render`
  `cargo test -p mde-shell-egui browser_print_drawer -- --nocapture` passed 5
  focused rendered print-drawer tests; `.50` slot `browser-drawer-refined-fmt`
  file-scoped `rustfmt --edition 2021 --check` passed for the touched Browser
  drawer files.
  A follow-up 2026-07-19 Maps refined-header slice reduced the Maps & Location
  first-party header to the shared menubar height plus a half-gutter and tightened
  the title/subtitle offset so it no longer carries the remaining thick 44pt
  header band. Farm evidence: `.90` slot `maps-refined-header`
  `cargo test -p mde-maps-location-egui maps_header_uses_refined_shared_chrome_height -- --nocapture`
  passed; `.170` slot `maps-refined-render`
  `cargo test -p mde-maps-location-egui maps_location_panel_renders_simulated_vertical_slice -- --nocapture`
  passed; `.50` slot `maps-refined-header-fmt` file-scoped
  `rustfmt --edition 2021 --check` passed for `view.rs`. A follow-up
  2026-07-19 Media refined-transport slice tied the Media transport button height
  to `mde_egui::menubar::BAR_HEIGHT`, preserving the compact transport icon/text
  render path while preventing local toolbar-height drift. Farm evidence: `.90`
  slot `media-refined-transport`
  `cargo test -p mde-media-egui transport_buttons_use_refined_shared_chrome_height -- --nocapture`
  passed; `.170` slot `media-transport-render`
  `cargo test -p mde-media-egui player_transport_controls_paint_icons_without_unicode_text -- --nocapture`
  passed; `.50` slot `media-refined-transport-fmt` file-scoped
  `rustfmt --edition 2021 --check` passed for `app.rs`.
  A follow-up 2026-07-19 Media queue-density slice moved the icon-only queue row
  actions off the remaining 24pt `Style::SP_L` visual button band and onto
  `Style::TOOLBAR_CONTROL_H`, with a Media-local queue-button scope so egui's
  default interaction floor cannot thicken those row controls while the painted
  remove/move icons and accessibility labels remain intact. Farm evidence: `.90`
  slot `media-queue-density-test`
  `cargo test -p mde-media-egui queue_action_buttons_use_refined_shared_chrome_height -- --nocapture`
  passed; BigBoy `.130` slot `media-queue-render-test`
  `cargo test -p mde-media-egui queue_view_renders_empty_and_with_items -- --nocapture`
  passed; `.50` slot `media-queue-density-fmt`
  `cargo fmt -p mde-media-egui -- --check` passed; local `git diff --check`
  passed for `crates/desktop/mde-media-egui/src/app.rs`.
  A follow-up 2026-07-19 Files refined-toolbar-control slice added shared
  `Style::TOOLBAR_CONTROL_H` as a 21pt visual control-height token, routed Files
  action/icon buttons plus the Files surface tab strip, top toolbar, pane
  navigation row, and pane tab strip through a Files toolbar scope using that
  metric, and kept the shared pointer hit-target floor covered by the existing
  density contract. Farm evidence: `.90` slot `shared-toolbar-control`
  `cargo test -p mde-egui refined -- --nocapture` passed 2 shared
  typography/density tests; `.170` slot `files-refined-action-height`
  `cargo test -p mde-files-egui refined -- --nocapture` passed the Files refined
  action-height and toolbar-scope tests; BigBoy `.130` slot `files-action-render`
  `cargo test -p mde-files-egui transfer_lifecycle_controls_use_files_action_button_tokens -- --nocapture`
  passed; `.50` slot `files-refined-action-fmt` file-scoped
  `rustfmt --edition 2021 --check` passed for `style.rs` and `view.rs`.
  A follow-up 2026-07-19 Browser suggestions-density slice removed the remaining
  128pt page-scale gutter from the omnibox suggestions row, replaced it with a
  `CHROME_BUTTON + CHROME_GAP` leading inset, and added a painted-geometry
  regression so the first suggestion category stays close to the location bar on
  narrow Browser surfaces. Farm evidence: BigBoy `.130` slot
  `browser-suggestion-regression`
  `cargo test -p mde-shell-egui browser_suggestion -- --nocapture` passed 4
  focused suggestion tests; `.90` slot `browser-suggestion-inset`
  `cargo test -p mde-shell-egui browser_suggestions_panel_uses_refined_leading_inset -- --nocapture`
  passed; `.50` slot `browser-suggestion-fmt` package-level `cargo fmt --check`
  exposed unrelated pre-existing formatting drift in other dirty `mde-shell-egui`
  files, then direct remote file-scoped `rustfmt --edition 2021 --config
  skip_children=true --check crates/desktop/mde-shell-egui/src/web/chrome_ui/mod.rs`
  passed.
  A follow-up 2026-07-19 Bookmarks refined-header slice reduced the Bookmarks
  top-left header title by the requested 2pt, introduced a Bookmarks-local
  toolbar scope that uses shared `Style::CONTROL_PAD_Y` and
  `Style::TOOLBAR_CONTROL_H`, and routed header/search/sort/add-form toolbar
  controls through the refined 21pt visual height while preserving 24pt bookmark
  data rows for list readability. Farm evidence: `.90` slot
  `bookmarks-density-tests2`
  `cargo test -p mde-bookmarks-egui bookmarks_ -- --nocapture` passed the 2
  focused density tests; `.170` slot `bookmarks-density-render2`
  `cargo test -p mde-bookmarks-egui renders_the_populated_manager -- --nocapture`
  passed the populated render path; `.50` slot `bookmarks-density-fmt2`
  `cargo fmt -p mde-bookmarks-egui -- --check` passed.
  A follow-up 2026-07-19 Storage refined-action-control slice replaced the
  remaining 24pt Storage icon button row with `Style::TOOLBAR_CONTROL_H`, added
  a Storage-local action-button padding scope so 16pt YAMIS icons still fit the
  refined 21pt visual height, and applied it to Refresh topology, Stage, and
  pending-queue move/remove controls while leaving disk segment bars and form
  fields untouched. Farm evidence: BigBoy `.130` slot `storage-density-test`
  `cargo test -p mde-shell-egui storage_action_buttons_use_refined_chrome_height -- --nocapture`
  passed; `.90` slot `storage-icon-render`
  `cargo test -p mde-shell-egui storage_queue_controls_do_not_paint_unicode_pseudo_icons -- --nocapture`
  passed; `.50` slot `storage-density-fmt` direct remote file-scoped
  `rustfmt --edition 2021 --config skip_children=true --check` passed for
  `storage/mod.rs` and `storage/tests.rs`.
  Shared-token evidence: `.90` slot `style-density`
  `cargo test -p mde-egui button_padding_keeps_toolbars_refined_without_shrinking_hit_targets -- --nocapture`
  passed; `.170` slot `menubar-density`
  `cargo test -p mde-egui menu_bar_uses_refined_chrome_typography -- --nocapture`
  passed.
  A local raw-hover sweep
  across shell, Files, Media, Panel, and shared egui surfaces now finds no direct
  `on_hover_text` / `on_disabled_hover_text` call sites outside themed helper
  names and Browser Chrome custom hover cards. A later 2026-07-19 follow-up
  extended `install-helpers/lint-style-leaks.sh` so direct raw egui hover text
  calls in `crates/desktop` or `crates/shared` are now a mechanical regression
  failure; the focused `rg` verification remains at 0 hits. A later 2026-07-19
  style-gate cleanup made the full `lint-style-leaks.sh` run green by separating
  true shared-shell chrome leaks from documented non-shell colour data: Browser
  chrome keeps its AI_GOVERNANCE §4 local Chrome/Material palette, CEF verifier
  pixel samples stay classified as test data, and the Maps vertical-slice canvas
  palette is allowed only on explicit `style-leak-ok: map-content-color` lines.
  Verification: local `bash -n install-helpers/lint-style-leaks.sh`,
  `install-helpers/lint-style-leaks.sh`, the raw colour search with the same
  exclusions, and `git diff --check` all passed; `.50` slot
  `style-lint-map-fmt` file-scoped
  `rustfmt --edition 2021 --config skip_children=true --check` passed for
  `crates/desktop/mde-maps-location-egui/src/view.rs`. A later 2026-07-18 Settings
  choice-tile polish slice
  replaced Theme, Wallpaper, and Remote Proofing raw selectable labels with a
  shared Settings choice button whose selected and hover colors resolve through
  the current dark/light palette and domain accent. Farm evidence: `.90`
  `cargo fmt -p mde-shell-egui --check` passed; BigBoy `.130` focused
  `settings_choice_tiles_use_themed_selected_and_hover_colors`,
  `each_mesh_system_section_renders_live_data_and_honest_unknown`, and
  `the_reworked_sections_paint_across_a_wide_detail_pane` passed. A later
  2026-07-18 Settings popup/ComboBox readability slice routed the Mouse primary
  button and Displays mode pickers through a Settings visual scope so raw egui
  popup/window/open/hover/active choice states resolve to `Style` surface, text,
  dim text, and border tokens instead of inherited shell defaults. Rendered
  popup choice coverage proves row text paints with Settings text and not raw
  black. Farm evidence: `.50` file-scoped
  `rustfmt --edition 2024 --config skip_children=true --check
  crates/desktop/mde-shell-egui/src/system/mod.rs
  crates/desktop/mde-shell-egui/src/system/tests.rs` passed; `.90` focused
  `cargo test -p mde-shell-egui
  settings_combobox_popups_use_themed_readable_choice_colors -- --nocapture`
  passed. A follow-up 2026-07-19 Power Settings dropdown polish slice routed the
  idle timeout, idle action, and lid-close action ComboBoxes through a local
  compact popup style helper so those power pickers inherit light/dark Settings
  surface/text/hover/open/selection roles instead of raw egui dropdown defaults,
  while preserving `PowerHonorConfig` save dispatch only on real selection
  changes. Farm evidence: BigBoy `.130` slot `power-popup-style`
  `cargo test -p mde-shell-egui power_combo_menu_style_uses_themed_compact_popup_chrome -- --nocapture`
  passed; `.90` slot `power-picker-render`
  `cargo test -p mde-shell-egui the_power5_pickers_draw_and_dispatch_nothing_on_an_untouched_frame -- --nocapture`
  passed; `.50` slot `power-popup-fmt` file-scoped
  `rustfmt --edition 2021 --check` passed for
  `crates/desktop/mde-shell-egui/src/power_settings.rs`. A later
  2026-07-18 Start tile context-menu polish slice wrapped the tile right-click
  menu in a Start-menu visual scope so the popup surface, widget states, and row
  text use shell `Style` tokens instead of inherited egui popup colors, while
  preserving Open/Pin behavior and AccessKit rows. Farm evidence: `.50`
  file-scoped
  `rustfmt --edition 2024 --config skip_children=true --check
  crates/desktop/mde-shell-egui/src/start_menu.rs` passed; BigBoy `.130`
  focused `cargo test -p mde-shell-egui tile_context_menu -- --nocapture`
  passed 2 tests. A later 2026-07-18 shared menu-bar light-mode readability
  slice resolved menu titles, dropdown row text, disabled/caption labels, accent
  focus/underline paint, the top-right Remote Sessions stroke, and status-chip
  fills/tone colors through the active `Style` color scheme before popup paint,
  so Windows-2000 light mode no longer paints shared drop-downs with dark-shell
  text tokens. Farm evidence: `.90` file-scoped
  `rustfmt --edition 2021 --check crates/shared/mde-egui/src/menubar.rs`
  passed; `.90` focused `cargo test -p mde-egui menubar -- --nocapture` passed
  12 tests; BigBoy `.130` focused
  `cargo test -p mde-shell-egui the_browser_bar_renders_headless -- --nocapture`
  passed. Broad `.50` `cargo fmt -p mde-egui -- --check` remains blocked by
  pre-existing rustfmt drift in `mde-egui` exports/imports outside this slice.
  A later 2026-07-18 live `.15` Chat-empty investigation found `chat` and
  `notify` workers healthy but publishing to root's legacy
  `/root/.local/share/mde/bus` spool while the GUI read `/run/mde-bus`; the
  source fix made both workers honor `MDE_BUS_ROOT` before the XDG fallback, and
  the live `.15` remediation preserved the installed RPM binary while symlinking
  the root legacy spool to `/run/mde-bus` and restarting `mackesd`. Post-fix
  `.15` evidence showed `state/chat/roster`, `state/chat/rooms`,
  `state/chat/notify`, and `event/notify/*` records on `/run/mde-bus`; BigBoy
  `.130` focused `default_bus_root_resolution_honors_mde_bus_root`,
  `roster_is_published_for_the_ui`, and
  `emitted_notification_folds_into_alert_self_exactly_as_chat_does` passed, with
  `.50` `cargo fmt -p mackesd --check` also passed.
  A follow-up 2026-07-18 Chat empty-state polish pass replaced the generic
  no-roster/no-selection pane with a themed waiting panel and a model-backed
  activity overview that surfaces real peer, room, unread, and folded-alert
  counts without selecting or acknowledging a lane on the operator's behalf;
  `.90` focused
  `home_overview_renders_activity_without_marking_notifications_read` wrote the
  rendered proof `target/screenshots/chat-home-overview.png`, BigBoy `.130`
  focused `cargo test -p mde-shell-egui chat -- --nocapture` passed 47 Chat and
  Chat-adjacent tests, and `.50` `cargo fmt -p mde-shell-egui --check` passed.
  A follow-up Chat default-surface pass made the home unread badge include the
  aggregate Notifications watermark without double-counting folded alerts and
  added painted-copy coverage for the no-roster waiting pane and loaded-roster
  activity overview; BigBoy `.130` focused
  `cargo test -p mde-shell-egui chat -- --nocapture` passed 49 Chat and
  Chat-adjacent tests, and `.50` `cargo fmt -p mde-shell-egui -- --check`
  passed.
  A later 2026-07-18 Chat mute-icon slice exposed YAMIS-backed
  `IconId::Notifications` and `IconId::NotificationsMuted`, replaced the
  contact/room mute button's bell emoji pseudo-icons with the shared icon
  texture path plus ASCII labels, and covered both the shared raster mapping
  and rendered Chat button copy. Farm evidence: `.90` focused
  `notification_glyphs_are_yamis_backed_and_rasterize_at_chat_button_size`,
  BigBoy `.130` focused
  `chat_mute_button_uses_yamis_icon_instead_of_bell_emoji_text`, and `.50`
  touched-file fmt passed.
  A follow-up 2026-07-18 Contacts action-icon pass routed Call, Remote Control,
  and self-status Edit through YAMIS-backed `IconId::Phones`,
  `IconId::Sessions`, and `IconId::TextEdit`, eliminating the remaining
  phone/desktop/pencil pseudo-icons in those Chat controls. Farm evidence: `.90`
  focused `chat_action_buttons_use_yamis_icons_instead_of_emoji_pseudo_icons`
  passed.
  A later 2026-07-18 Contacts Center ICQ layout slice made the Chat surface read
  as a persistent two-pane client: the Rooms/Contacts roster stays on the left,
  the right side is always a themed Messages browser, and the no-selection state
  previews recent real contact/room message rows without selecting a lane or
  clearing unread watermarks. Farm evidence: BigBoy `.130` focused
  `home_overview_renders_activity_without_marking_notifications_read` passed
  from the current tree and wrote the rendered Chat proof screenshot; `.90`
  independently passed the same focused Chat test; `.170` file-scoped
  `rustfmt --edition 2024 --config skip_children=true --check` passed across
  the touched shell GUI files.
  A follow-up 2026-07-18 Contacts layout fix replaced the stateful resizable
  roster side panel with a deterministic 25%/75% bounded split so the Messages
  browser cannot render off the right edge of the workspace. Farm evidence:
  BigBoy `.130` focused
  `contacts_layout_reserves_quarter_width_for_roster_and_keeps_messages_onscreen`
  and adjacent `surface_mounts_and_tessellates_over_real_state` passed; `.50`
  file-scoped chat `rustfmt --edition 2024 --config skip_children=true --check`
  passed.
  A later 2026-07-19 Contacts title-density slice added `CHAT_PANE_TITLE =
  Style::HEADING - 2.0` and routed the right-side Messages, contact,
  Notifications, and room headers through that refined pane-title rung while
  leaving metric values on `Style::HEADING` for emphasis. Farm evidence: `.90`
  reused warmed shell slot `iac-density-test`
  `cargo test -p mde-shell-egui contacts_pane_titles_use_refined_header_size -- --nocapture`
  passed; `.50` reused scoped file-format slot `iac-density-filefmt`
  `rustfmt --edition 2021 --config skip_children=true --check` passed for
  `chat/mod.rs` and `chat/tests.rs`; local `git diff --check` passed for the
  touched Chat files.
  A follow-up Files icon slice replaced raw tab-strip close/new-tab text
  controls with YAMIS-backed `IconId::Close` and `IconId::NewTab` icon
  buttons while preserving hover text and widget metadata; farm `.90`
  focused `files_tab_strip_controls_use_yamis_icon_buttons` and `.50`
  `cargo fmt -p mde-files-egui -- --check` passed.
  A follow-up 2026-07-18 Files tooltip polish slice routed all Files view hover
  and disabled-hover copy through a Files-local themed tooltip frame so file
  manager tooltips no longer inherit raw egui popup colors. Farm evidence:
  BigBoy `.130` focused
  `files_hover_tooltip_uses_themed_text_and_surface` passed, `.90` focused
  `mounts_and_renders_the_transfers_tab_with_ledger_fixtures` passed, and `.50`
  file-scoped `rustfmt --edition 2024 --config skip_children=true --check`
  passed for `mde-files-egui/src/view.rs`.
  A follow-up 2026-07-19 Files context-menu polish slice routed file-row
  right-click menus through a Files-local popup visual scope that resolves the
  active light/dark `Style` palette for menu surface, row states, disabled text,
  and selection while preserving existing Send to / Send in Chat / Transfer to /
  Editor / clipboard / Properties / Delete action paths. Farm evidence: `.90`
  slot `files-context-menu2`
  `cargo test -p mde-files-egui files_context_menu_visuals_use_themed_text_and_surface -- --nocapture`
  passed; `.170` slot `files-tooltip-guard2`
  `cargo test -p mde-files-egui files_hover_tooltip_uses_themed_text_and_surface -- --nocapture`
  passed; `.50` slot `files-context-fmt2`
  `cargo fmt -p mde-files-egui -- --check` passed. A follow-up 2026-07-19 Files
  nested-submenu polish slice routed the row context menu's `Send to`,
  `Send in Chat`, and `Transfer to` submenus through a Files-scoped popup
  helper so nested egui menu windows reapply the same light/dark text, hover,
  active, and compact spacing roles as the outer row context menu. Farm
  evidence: BigBoy `.130` slot `files-submenu-polish`
  `cargo test -p mde-files-egui files_nested_popup_scope_repairs_raw_menu_visuals -- --nocapture`
  passed; BigBoy `.130` slot `files-submenu-polish`
  `cargo fmt --package mde-files-egui -- --check` passed.
  A follow-up 2026-07-18 Device Manager tooltip polish slice routed host-rail,
  live-refresh, About, modal-close, and detail-drawer close/copy hovers through a
  Device Manager themed tooltip frame so the hardware inspector no longer
  inherits raw egui popup colors. Farm evidence: BigBoy `.130` focused
  `device_manager_tooltip_uses_themed_text_and_surface` passed, `.90` focused
  `the_tree_renders_headless_from_a_fixture_inventory` passed, and `.50`
  file-scoped `rustfmt --edition 2024 --config skip_children=true --check`
  passed for the touched `device_manager` files.
  A follow-up 2026-07-19 Device Manager context-menu polish slice routed device
  row right-click menus through a Device Manager popup visual scope that resolves
  active light/dark `Style` palette roles for menu surface, row states, disabled
  text, destructive selection tint, and compact row spacing while preserving
  Properties / Scan / Copy details / typed privileged-operation arming behavior.
  Farm evidence: BigBoy `.130` slot `devmgr-context-popup`
  `cargo test -p mde-shell-egui device_manager_context_menu_uses_themed_text_and_surface -- --nocapture`
  passed; `.90` slot `devmgr-context-render`
  `cargo test -p mde-shell-egui a_device_row_context_menu_renders_and_the_drawer_copy_path_is_live -- --nocapture`
  passed; `.50` slot `devmgr-context-fmt` file-scoped
  `rustfmt --edition 2021 --check` passed for the Device Manager source and test
  files.
  A follow-up 2026-07-18 Storage icon/tooltip polish slice routed Refresh
  topology, Stage, queue move up/down, and queue remove controls through
  shared `IconId` actions, replaced the remaining Storage lock/staging/arrow
  pseudo-icon text with plain labels, and routed Storage hover help through a
  themed tooltip frame. Farm evidence: `.90` focused
  `storage_queue_controls_do_not_paint_unicode_pseudo_icons` passed.
  A follow-up Media queue icon slice replaced raw `✕`/`▼`/`▲` queue row
  text buttons with labelled icon-only controls using the shared empty-button
  plus painted-icon pattern, preserving remove/move behavior, hover text,
  pointing cursor, and widget metadata. Farm evidence: BigBoy `.130` focused
  `cargo test -p mde-media-egui queue_view_renders_empty_and_with_items -- --nocapture`
  passed, and `.50` `cargo fmt -p mde-media-egui -- --check` passed.
  A follow-up taskbar hover-title slice clipped long running-session titles to
  the fixed hover-preview card body so wide VM names cannot paint into
  neighboring chrome, with headless clip-rect coverage. Farm evidence: `.50`
  `cargo fmt -p mde-shell-egui -- --check` passed; BigBoy `.130` focused
  `win10_hybrid_31_session_hover_preview_clips_long_titles_to_card_body`
  passed from an isolated clean worktree carrying only the dock patch. The
  follow-up 2026-07-18 `13844e25` Fedora 44 split-RPM proof installed the
  bounded progress/preview build on live `.15`, verified the active shell
  binary hash against `/usr/bin/mde-shell-egui`, and passed installed Browser
  all-engine, link-navigation, idle-media, Google, and Google News smokes.
  A later same-day taskbar health/tray polish pass replaced the Health status
  control's wireless-signal glyph with a dedicated YAMIS-backed smart-status
  icon, preserving distinct Desktop Sources, Health, overflow, and notification
  icons, and moved the Windows 11 tray-island proof to the headless screenshot
  raster path. Farm evidence: `.50` file-scoped rustfmt over `dock/mod.rs` and
  `dock/tests.rs` passed; `.170` focused
  `health_status_glyph_is_dedicated_and_rasterizes` passed; BigBoy `.130`
  focused
  `taskbar_launch_sources_health_and_overflow_use_distinct_non_chevron_icons`
  and `win11_tray_clock_and_notification_area_paint_a_grouped_island` passed,
  with `taskbar-win11-tray-island.png` generated. A follow-up 2026-07-19 taskbar
  token cleanup moved the black taskbar strip, white icon tint, cell hover/active
  fills, clock date tone, and Windows 11 tray-island fills/border from
  `dock/mod.rs` into shared `mde_egui::Style`, removing Dock from the
  `lint-style-leaks.sh` hardcoded-color hit list while preserving the rendered
  black-bar and grouped-tray proof paths. Farm evidence: `.50` slot
  `taskbar-style-test`
  `cargo test -p mde-egui taskbar_palette -- --nocapture` passed; BigBoy `.130`
  slot `taskbar-black-bar`
  `cargo test -p mde-shell-egui taskbar_controls_render_white_icons_on_a_black_bar -- --nocapture`
  passed; `.90` slot `taskbar-tray-island`
  `cargo test -p mde-shell-egui win11_tray_clock_and_notification_area_paint_a_grouped_island -- --nocapture`
  passed and wrote `taskbar-win11-tray-island.png`; `.170` file-scoped
  `rustfmt --edition 2021 --config skip_children=true --check` passed for
  `crates/shared/mde-egui/src/style.rs` and
  `crates/desktop/mde-shell-egui/src/dock/mod.rs`.
  Remaining live proof is the screenshot/pixel pass for the full taskbar,
  Start grid, tray, and action-center composition on the target seat.
  Remaining icon work is the full per-surface sweep for
  hand-painted icons or other code paths that bypass `IconId`, removal or
  repointing of stale Carbon/Material asset uses, and live rendered proof on the
  target seat.


### WL-UX-003 - Accessibility consumer and application sweep

- Disposition: RETIRED (2026-07-19 operator: 'AccessKit is NOT a goal — remove it as a requirement')
- Priority: P2
- Complexity: Epic
- Problem: The DRM AccessKit bridge and reduce-motion plumbing now exist, but a
  complete accessibility posture still needs a real consumer/screen-reader path,
  app-level annotations, toast live regions, and companion app coverage.
- Required outcome: The shipped DRM seat can expose a usable accessibility tree
  to an assistive consumer, and major shell/app surfaces have labels, roles,
  focus, live regions, and reduce-motion behavior.
- Scope: AccessKit consumer/TTS decision, app-picker/system quad, toasts,
  Explorer, curtain, VDI, Device Manager, Chooser, companion apps, and tests.
- Relevant files/components: `crates/shared/mde-egui/src/a11y.rs`,
  `crates/shared/mde-egui/src/drm.rs`, `crates/desktop/mde-shell-egui/src/`,
  companion egui crates.
- Dependencies: Accessibility output strategy; governance currently marks broad
  accessibility as deferred for the cutover.
- Acceptance criteria: `MDE_A11Y=1` or a persisted setting produces a consumable
  tree, critical toasts use live regions, raw-painted cells have names/roles, and
  reduce-motion reaches auto-rotating surfaces.
- Current evidence: A 2026-07-17 Start-menu reduced-motion pass made live-tile
  rotation read the system Appearance motion signal, freezes multi-fact tiles on
  their primary fact when motion is reduced/disabled, suppresses the rotating
  tile live region while frozen, and stops the settled-open rotation heartbeat;
  farm `.170` fmt, BigBoy `.130` focused Start-menu coverage, `.90` system
  motion-setting coverage, and `.50` shared motion coverage passed.
  A later 2026-07-17 Start-menu context-row AccessKit pass added named button
  nodes for the tile context menu's Open/Pin rows; farm `.50` fmt and BigBoy
  `.130` focused context-row coverage passed.
  A later 2026-07-17 Start-menu pinned-shortcut AccessKit pass kept pinned
  shortcut tiles visually identical to their grouped copies while prefixing the
  pinned copy's accessibility value with `Pinned shortcut`, so assistive
  consumers can distinguish the two Browser entries; farm `.50` fmt and BigBoy
  `.130` focused `pinned_tile_accesskit_value_names_the_shortcut_copy` coverage
  passed.
  A later 2026-07-17 Start-menu search-result AccessKit pass added positioned
  `Button` values for raw-painted app and embedded Console result rows, including
  selected keyboard-highlight state; farm `.50` fmt and BigBoy `.130` focused
  `search_result_rows_export_positioned_accesskit_buttons` coverage passed.
  A later 2026-07-17 Browser tab-search AccessKit pass added named clickable
  `Button` nodes for raw-painted tab-search result rows, including tab position
  values and selected active-tab state; farm `.50` fmt and BigBoy `.130`
  focused `tab_search_results_export_accesskit_buttons_for_switching_tabs`
  coverage passed.
  A later 2026-07-17 Browser omnibox-suggestion AccessKit pass added named
  clickable `Button` nodes for raw-painted bookmark, file, history, and search
  suggestion chips, including suggestion position values and selected keyboard
  highlight state; farm `.50` fmt and BigBoy `.130` focused
  `browser_suggestion_chips_export_accesskit_buttons` coverage passed.
  A later 2026-07-17 Browser Options AccessKit pass added named `Button` nodes
  for raw-painted command rows, including enabled on/off state, disabled gate
  reasons, shortcuts, selected checked rows, and click actions only for enabled
  commands; farm `.50` fmt and BigBoy `.130` focused
  `browser_options_rows_export_accesskit_buttons` coverage passed.
  A later 2026-07-17 Browser downloads AccessKit pass added read-only `Row`
  nodes for visible download-manager entries, including filename, state, route,
  real progress metadata, verification flag, and error text while leaving command
  behavior on the existing action buttons; farm `.50` fmt and BigBoy `.130`
  focused `browser_download_rows_export_accesskit_status` coverage passed.
  A later 2026-07-17 Browser history AccessKit pass added named clickable
  `Button` nodes for visible history rows, exposing the user-facing title and
  real URL value while preserving the existing click-to-open drawer path; farm
  `.50` fmt and BigBoy `.130` focused
  `browser_history_rows_export_accesskit_buttons` coverage passed.
  A later 2026-07-17 Browser bookmarks-bar AccessKit pass added named clickable
  `Button` nodes for raw-painted bookmark bar buttons and overflow rows,
  exposing the bookmark title and real URL value while preserving the existing
  click/open-tab behavior; farm `.50` fmt and BigBoy `.130` focused
  `browser_bookmark_buttons_export_accesskit_links` coverage passed.
- Verification method: AccessKit tree tests, live consumer smoke, and UI tests for
  named controls.
- Origin or merged source IDs: a11y-02/04/05/06/07/08, shell-ux-6, platform
  review accessibility cluster.

