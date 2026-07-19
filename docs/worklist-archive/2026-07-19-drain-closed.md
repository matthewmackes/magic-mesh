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

