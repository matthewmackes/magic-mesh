# Platform Worklist

This is the only active platform worklist. Design notes, evidence ledgers,
runbooks, and operator notes are inputs, not parallel trackers. Historical
implementation diaries remain in docs/worklist-archive/ and are not executable
tasks.

## Current Snapshot - 2026-08-15 executable story rewrite

- **3 active epics:** 3 `Remaining`, 0 `Blocked`, 0 `Needs clarification`.
- **Latest stable integration:** 43 exact hostile gates passed across four farm hosts: `evidence/WORKLIST-2026-08-11-stable-exact-wave-r473.md`.
- **Execution order:** complete ARCH-010 stories in order; then consume its
  contracts in ARCH-008, ARCH-009, FUNC-019, FUNC-018, and FUNC-020. Run the
  vertical slices FUNC-011/FUNC-016, FUNC-017, FUNC-021, and FUNC-022 next. Integrate
  UX-009, UX-011, UX-012, and UX-014 at their named story gates. Close
  every slice through CRIT-006 and CRIT-007.
- **Single-authority lock:** typed Workload operations are the only VM/container
  lifecycle API; mackesd is the only daemon authority; mde-bus is the only
  platform bus; the shell renders typed bounded projections and sends typed
  intent. Do not add a compatibility shim, parallel tracker, direct backend
  call, raw command, or GUI-owned service state.
- **Product lock:** Construct is one egui DRM thin-client shell. Native apps run
  in governed VMs or approved native collaboration/media surfaces. There is no
  Wayland compositor, host Browser engine, OpenStack control plane, or retired
  LizardFS/cloud-hypervisor path.
- **Evidence lock:** a story is incomplete until its deliverable, focused hostile
  tests, farm command and result, and required live/package evidence are recorded
  in docs/platform/evidence/. Missing hardware or provider access is a named
  blocker, not a passing substitute.
- **Shared release-proof ownership:** first-release input admission, signed
  artifact/package proof, installed baseline acceptance, corrected-forward
  recovery, and deferred provider/live proofs are owned by `WL-TEST-002`.
  Product epics must not duplicate those rollout tasks; they retain only
  product-specific implementation and integration gaps, and cite `WL-TEST-002`
  when its acceptance is a dependency.
- **Test-seat cap (operator lock 2026-08-14):** no validation, rollout proof,
  capture, chaos, recovery, or acceptance activity may require or exercise more
  than two physical test seats. An epic may substitute named seats when its
  hardware is the subject, but must remain at two or fewer. Historical three- and
  five-seat evidence stays factual but creates no forward multi-seat requirement.
  Lighthouses are not test seats and retain their independently governed quorum
  proof.
- **Privacy-retention lock (operator lock 2026-08-10):** system logs, Bus
  history, transfer ledgers, collaboration JSONL, application histories, and
  audit records have a fleet-wide maximum lifetime of six hours. No priority or
  audit class is exempt. Configuration, identities, credentials, current
  materialized state, user media, queued payloads, and VM disks are not history
  and must survive each epoch. Offline replicas must not be able to restore an
  expired record after they rejoin. The release-33 Dell churn, compact-Bus,
  synchronized privacy epoch, VM CPU/I/O profile, and exact cold-boot proof is
  recorded in `docs/platform/evidence/WL-ARCH-008-WL-ARCH-009-WL-ARCH-010-WL-CRIT-007-2026-08-10-dell-churn-release33-r134.md`.
- **Farm lock:** heavy verification is farm-only; route the longest job to
  BigBoy at 172.20.0.130, use explicit MCNF_BUILD_HOST and MCNF_BUILD_SLOT, and
  never run filler tests.
- **Rollout lock:** prove each release activity on no more than two selected
  physical seats and the independently required lighthouses. Wider fleet
  deployment, when needed, proceeds in separately bounded waves and does not
  expand the test requirement. Publish the red AI-GENERATED-ALERT and wait five
  seconds before each seat mutation. Recover failures by re-enrollment and
  corrected-forward deployment, never rollback.
- **Story format:** execute stories top-to-bottom. Do not start a story until
  every dependency is green. If a dependency or external resource is absent,
  set the epic to Blocked with the exact missing item; do not invent evidence.

## Active Drain Goal

Finish the Music and Media Player vertical slice (daemon catalog/playback,
mpv frame/audio, library/Jellyfin, cache/offline, discovery, casting, handoff,
visual proof) while preserving the single Workload, Bus, and typed executor
authorities. Archive old MEDIA and FUNC-007 IDs; they are evidence only.

## Service Release Queue

1. Workloads runtime and native attachment.
2. Browser VM cutover and standalone legacy repository.
3. Process-isolated mackesd and Workers.
4. Collaboration Suite and rich clipboard.
5. Maps/MG90 and universal resource browser.
6. Flatpak App VMs and Android Workloads.
7. Music/Media Player.
8. Clock, distributed alarms/timers, and notification entry cutover.
9. Quazar visual integration, health modal/Kiron, and release/recovery proof.
10. Shared first-release, installed-seat, provider, and corrected-forward proof
    under WL-TEST-002.

## Story execution contract

Every story below is a self-contained unit. The implementing agent must:
read the named inputs; change only the owned files; produce the named deliverable;
add the stated hostile or regression test; run the stated validation; record the
revision, command, result, and evidence path; and mark the story complete only
when the Done when condition is true. A passing compile without the named
behavioral evidence is not completion.

### WL-TEST-002 - Post-first-development-release testing and proofing

- Status: Remaining
- Priority: P1
- Complexity: Epic
- Problem: implementation and farm gates are complete for many slices, but first-development-release installation, operator providers, physical-seat behavior, direct-DRM rendering, guest/device integrations, and corrected-forward recovery were intentionally deferred until a signed release and its deployment inputs exist.
- Required outcome: after the first development release, execute and archive every deferred testing, proofing, and validation obligation in this epic; reconcile failures as implementation work or named external-input blockers, without weakening proof requirements.
- Current state: the queue is intentionally post-release and non-blocking for pre-release coding. No live provider, installed-seat, hardware, guest, or release-deployment claim is made.
- Dependencies: first development release, signed package/image artifacts, operator-approved provider credentials, and no more than two physical test seats.
- Deliverable: release identity and artifact admission, installed-seat baseline, provider readiness, live behavior captures, direct-DRM/GUI proof, guest/device proof, recovery/corrected-forward results, and farm commands/results with redacted operator evidence.
- Validation: run all focused farm gates first, then execute the named one-node/two-seat live checks on the exact installed release; attach evidence to the owning source epic and cross-reference this epic.
- Acceptance: tested bytes match the signed release; no fake connected/healthy state; provider absence and failure remain visible; recovery is corrected-forward; every result identifies hardware/provider authorization and stays within the two-seat cap.
- Owner: Platform collaboration and release verification.

#### WL-TEST-002 test queue

- **Pre-release verifier controls (2026-08-15):** corrected-forward verifier self-test passed 19/19 and release-gate matrix self-test accepted 1 valid fixture while rejecting 21 hostile fixtures; evidence: `evidence/WL-TEST-002-2026-08-15-release-recovery-verifier-r1.md`. This does not claim exact-release, installed-seat, provider, hardware, or live recovery proof.
- **Release-input boundary controls (2026-08-15):** mandatory-input/bootc receipt admission passed and the hostile first-release phase-boundary suite rejected unsigned or incomplete promotion; evidence: `evidence/WL-TEST-002-2026-08-15-release-input-boundary-r1.md`. This does not claim a signed release artifact.

1. **Release and package admission:** implementation-complete contracts already cover manifest/provenance, dependency closure, upgrade/install identity, and corrected-forward payload binding; after a signed first development release, verify those properties against the exact admitted bytes.
2. **Installed baseline:** implementation-complete service, Workers, Bus, storage, and restart/rejoin gates remain; install the exact release on the governed one-node/two-seat test set and record display, audio, network, and physical-seat observations.
3. **Collaboration and providers:** bounded collaboration and fail-closed provider controls are implementation-complete; activate the governed SIP account and test Calls connect/reconnect/mute/consent/revocation plus deferred provider-backed collaboration and transfer behavior without asserting a fake live state.
4. **GUI and direct-DRM proof:** implementation-complete shell, taskbar, style/font, Kiron, Maps, Workers, Editor, and Music slices still require real-display captures, including human visual review where required.
5. **Media and physical providers:** bounded media/Cast/handoff controls are implementation-complete; test mpv/audio/video, cache/network loss, renderer recovery, Cast/handoff/DLNA, installed package/CPU behavior, and external catalog/server paths with authorized provider inputs.
6. **Guest and device integrations:** guest/runtime admission controls are implementation-complete; test App VM/VDI, Browser/Android/Cuttlefish, GPU/audio/input, nested-KVM, remote-session, and guest reconnect/upgrade paths when signed images and runtime artifacts are admitted.
7. **Recovery and resilience:** recovery mechanisms and corrected-forward guards are implementation-complete; run deferred one-node service/process, display/session, lock/sleep, storage/network, generation, reboot, restart, recovery, and corrected-forward drills, recording failures rather than converting unavailable hardware into passes.
8. **Reconciliation:** map every result to its owning epic, archive release evidence, reopen implementation work for regressions, and retain named operator blockers for missing artifacts or authorization.

   **Moved implementation-closed UX proof queues:** `WL-UX-011` Surface
   Pro 5/6, provider, seat, and physical acceptance; and `WL-UX-014` KIRON
   renderer/live proof are post-development-release obligations owned here.
   Their implementation evidence is archived in
   `docs/worklist-archive/2026-08-15-wl-ux-011-ux-014-disposition.md`.

   **Moved implementation-closed Music proof queue:** `WL-FUNC-021` signed
   release, installed package/seat, provider continuity/loss, direct-render,
   Cast/DLNA, and live handoff observations are post-development-release
   obligations owned here. Music daemon/workspace/cast/handoff implementation
   evidence is archived in
   `docs/worklist-archive/2026-08-15-wl-func-021-disposition.md`.

   **Moved implementation-closed Collaboration proof queue:** `WL-FUNC-011`
   native-office runtime admission, governed SIP activation, exact-release,
   installed-seat, provider, visual, and live recovery observations are
   post-development-release obligations owned here. Implementation evidence
   is archived in
   `docs/worklist-archive/2026-08-15-wl-func-011-disposition.md`.

## Core Architecture


## User Interface And Experience

### WL-UX-009 evidence disposition — Editor large-text menubar correction (2026-08-02)

The shared `MenuBar` correction is accepted for the Editor Light/Largest
direct-DRM slice. The constrained nested editor pane now uses an explicit
two-row layout with a single-line horizontally scrollable menu strip, keeping
`Help`, formatting controls, the document body, status row, details rail, and
taskbar visible without accidental vertical expansion. Farm evidence is
`mde-egui` 269/269 and `mde-editor-egui` 407/407; Dell `.138` ran payload
`4808bd30bfa72ab386056cd1ecbc4d6aac0251a144609aedcee3e209b8dc888c` with an
active zero-restart service. Accepted proof:
`evidence/WL-UX-009-2026-08-02-138-editor-light-largest-4808bd30.png`,
SHA-256 `b1e0bafea6d63cd88f0979d11024da56a05611115f1e8ee52bbc4c19035371cb`.
This is a closed validation slice, not closure of WL-UX-009 or a production
readiness claim; the full current-payload route/profile matrix and remaining
hardware/boundary evidence are still required.

- Proof-route follow-up (2026-08-02): the explicit Files direct-DRM proof route
  is reasserted at shell construction and frame entry, but the live readback
  still lands on the Auto/clock surface after later automatic navigation drains.
  Add one final proof-only route assertion immediately before central workspace
  rendering, preserve ordinary navigation behavior, farm-test the route seam,
  and recapture the Files/Airsonic evidence before accepting the slice. This is
  a route-harness correction only; no readiness claim follows from it.
- Farm-integrity finding (2026-08-02): the current release source cannot compile
  the existing This Node hostile-fixture test because a JSON `\\u0000` escape is
  interpreted by Rust as an invalid source escape. Convert only that fixture to
  Rust's braced Unicode escape form, rerun the release build and focused test,
  and keep all visual/readiness claims open until the exact candidate is proven.
- Farm-integrity resolution (2026-08-02): the synchronized farm source already
  contains the braced `\\u{0000}` fixture form; the stale first release sync was
  discarded. The rerun release build on BigBoy completed successfully and
  produced exact payload `b888b0d163de8369b554569d5a75f3f17f257d8581f1e2558d14a3d479435f0c`.
- Proof-route resolution (2026-08-02): after the approved temporary
  `require_login_at_boot:false` proof fixture, exact candidate `b888b0d1…` on
  `.138` rendered the explicit Files route rather than the boot curtain. Visual
  inspection accepts the shared `FILES` frame, complete ten-action NODE ACTIONS
  inventory, reachable mesh peer, and `Airsonic upload · Music-owned` action;
  PNG SHA-256 is `64be0f4af1cdf26ecfc66e96172cf76a6234b64caed7b842fe2fec9c31e3b329`
  in the DRM evidence ledger. The temporary unlock, candidate binary, and proof
  drop-in were removed; `.138` is restored to payload `20955383…`,
  `require_login_at_boot:true`, Dark/Construct/Default/Normal, active service,
  and zero recorded restarts. This closes only the Files/Airsonic proof-route
  slice; Dell adoption, the remaining matrix, strict linear scanout, and
  WL-UX-009 readiness remain open.
- Car Light palette implementation slice (2026-08-02): the Car profile currently
  installs `AutoSync3` as the whole egui scheme, so the Auto Mode dashboard cannot
  honor a persisted Light choice even though its cards use shared palette tokens.
  Preserve AutoSync3's vehicle-specific accents/skin while pass-through rendering
  the persisted Dark/Light surface palette to AutoHome. Add a focused Light-vs-Dark
  render contract, farm-test it, then recapture exact current-payload Car Light /
  Largest direct-DRM frames on both seats before accepting the open cell.
- Car Light palette resolution (2026-08-02): candidate
  `e30f36cd562f729f91620ef3842827190bbf3b055bcd8126072630d1dedcd0ee` passed
  the focused Car suite 14/14. Visually accepted native Light/Largest Auto Mode
  frames were captured on `.138` (1920x1080,
  `79355ca02ed2a8086d0f9cd14dcfa411233e1aad46f8496ab0b885f9c781332b`) and
  Dell `.225` (1366x768,
  `39361bc641021fd0ed4e5ec7c4dd92b86343d944c5ce2bab3432c2feffa4dcbb`), with
  Light surface ground/cards and AutoSync3 accents. Both seats were restored to
  payload `20955383…`, secure Dark/Construct/Default/Normal, active service,
  zero restarts, and no proof drop-in. The proof-only logical-width override
  intentionally leaves each PNG at native physical dimensions while bounding
  content to the requested 800 logical-pixel viewport. This closes the Car
  Light/Largest narrow palette cell; strict linear scanout, the remaining
  route/profile matrix, and WL-UX-009 readiness remain open.
- Dell Terminal narrow recapture (2026-08-02): validate the current release
  candidate `e30f36cd…` on Dell `.225` for Terminal Dark desktop, Dark narrow
  (`800` logical width), Light desktop, and Light/Largest narrow. Inspect the
  full `TERMINAL` identity, command/session controls, taskbar contrast, and
  bounded body; accept only visually complete frames and restore secure seat
  state afterward.
- Dell Terminal recapture resolution (2026-08-02): candidate `e30f36cd…` on
  Dell `.225` passed visual inspection for Dark desktop, Dark narrow (`800`
  logical), Light desktop, and Light/Largest narrow. The full `TERMINAL`
  identity, menu/session controls, taskbar contrast, and bounded body are
  present; the earlier `TER…` interpretation is superseded. Evidence hashes
  are recorded in the DRM ledger. Dell was restored to payload `20955383…`,
  secure Dark/Construct/Default/Normal, active service, zero restarts, and no
  proof drop-in. This closes the Dell Terminal visual slice only; the remaining
  route/profile matrix, VDI guest readiness, strict linear scanout, and
  WL-UX-009 readiness remain open.
- Dell Editor current-candidate recapture (2026-08-02): validate candidate
  `e30f36cd…` on Dell `.225` for Editor Dark desktop, Dark narrow (`800`
  logical width), Light desktop, and Light/Largest narrow. Inspect direct-entry
  sidebar collapse, the shared `EDITOR` identity, internal menu/toolbar
  reachability, document/status/details geometry, and taskbar contrast; accept
  only complete frames and restore secure seat state afterward.
- Dell Editor recapture resolution (2026-08-02): candidate `e30f36cd…` on
  Dell `.225` passed visual inspection for Dark desktop, Dark narrow (`800`
  logical), Light desktop, and Light/Largest narrow. Direct-entry sidebars are
  collapsed; the Mesh Teams editor host chrome, editor toolbar/menu, document,
  status/details geometry, and taskbar remain bounded. Evidence hashes are in
  the DRM ledger; no guest/VDI pixels are claimed. Dell was restored to payload
  `20955383…`, secure Dark/Construct/Default/Normal, active service, zero
  restarts, and no proof drop-in. This closes the Dell Editor visual slice only;
  the remaining route/profile matrix, guest VDI readiness, strict linear
  scanout, and WL-UX-009 readiness remain open.
- Dell Files and Mesh Teams recapture (2026-08-02): validate candidate
  `e30f36cd…` on Dell `.225` for Files and Mesh Teams across Dark desktop, Dark
  narrow (`800` logical width), Light desktop, and Light/Largest narrow. Inspect
  Files' complete ten-action node inventory and sync/status lanes, plus Mesh
  Teams' shared identity strip, channel/app rails, and bounded body. Accept
  only visually complete frames and restore secure seat state after the batch.
- Mesh Teams Light contrast finding (2026-08-02): Dell `.225` current-candidate
  Mesh Teams Dark desktop and Dark narrow are readable, but Light desktop and
  Light/Largest narrow render Activity/body and rail copy with the Dark
  `TEXT`/`TEXT_DIM` values under the Light surface, producing washed-out,
  low-contrast content. Reject both Light cells; resolve Mesh Teams-owned text
  tokens through the shared runtime palette, add a Light render assertion, farm
  test, and recapture Dell before accepting the route.
- Mesh Teams Light contrast resolution (2026-08-02): candidate
  `ae51c12447c342aa9457e7db148cdb046595eeb533463a731b712cd1b1bb0236` maps the
  Activity and frame-owned text tokens through the shared runtime palette;
  BigBoy `mde-collab-egui --lib` passed 130/130, including an explicit
  Light-mode render assertion for Activity and Mesh Teams rail text. Dell Light desktop and
  Light/Largest narrow frames were recaptured after the normal page crossfade
  settled and visually accepted; their hashes and links are in the DRM ledger.
  The earlier transition frames remain rejected diagnostic evidence. Dell was
  restored to payload `20955383…`, secure Dark/Construct/Default/Normal,
  active service, zero restarts, and no proof drop-in. This closes only the
  Dell Files/Mesh Teams visual slice and Mesh Teams Light contrast finding;
  VDI guest readiness, strict linear scanout, the remaining matrix, and
  WL-UX-009 readiness remain open.
- VDI guest endpoint audit (2026-08-02): a fresh read-only probe of enrolled
  validation seats `.15`, `.138`, `.145`, and Dell `.225` found no open RDP,
  VNC, SPICE/VDI, or Sunshine endpoint on the approved validation ports.
  The approved boundary remains documented, but no guest framebuffer, guest
  input, or VDI readiness claim is made; retain this as an external-state
  evidence gap rather than styling the guest surface or claiming readiness.
- Current candidate Car matrix recapture (2026-08-02): validate exact release
  `ae51c124…` on both direct-DRM seats for Car Light/Largest narrow after the
  AutoHome palette resolution. Confirm the AutoSync3 vehicle skin remains
  intact while the persisted Light surface palette is honored; accept only
  complete, readable cockpit frames and restore secure seat state afterward.
- Current candidate Car matrix resolution (2026-08-02): exact candidate
  `ae51c124…` passed Car Light/Largest narrow on `.138` and Dell `.225`.
  Both direct-DRM frames show the complete Auto Mode cockpit, Light surface
  palette, preserved AutoSync3 accents, and bounded large-text cards; hashes
  and links are in the DRM ledger. Both seats were restored to payload
  `20955383…`, secure Dark/Construct/Default/Normal, active service, zero
  restarts, and no proof drop-in. This closes only the Car cell; strict linear
  scanout, VDI guest readiness, the remaining matrix, and WL-UX-009 readiness
  remain open.
- Current candidate Files matrix recapture (2026-08-02): validate exact
  release `ae51c124…` on `.138` and Dell `.225` for Files Dark desktop, Dark
  narrow (`800` logical width), Light desktop, and Light/Largest narrow.
  Confirm the complete node-action inventory, peer/status lanes, file list,
  preview boundary, and transfer status remain readable; restore secure seat
  state after capture and accept only inspected frames.
- Current candidate Files matrix resolution (2026-08-02): exact candidate
  `ae51c124…` passed Files Dark desktop, Dark narrow (`800`), Light desktop,
  and Light/Largest narrow on both `.138` and Dell `.225`. All frames show the
  complete ten-action node inventory, peer/status lanes, file list, preview
  boundary, and transfer/status strip without clipping or overlap; hashes and
  links are in the DRM ledger. Both seats were restored to payload `20955383…`,
  secure Dark/Construct/Default/Normal, active service, zero restarts, and no
  proof drop-in. This closes only the exact-candidate Files slice; strict
  linear scanout, VDI guest readiness, the remaining matrix, and WL-UX-009
  readiness remain open.
- Current candidate Editor matrix recapture (2026-08-02): validate exact
  release `ae51c124…` on `.138` and Dell `.225` for Editor Dark desktop, Dark
  narrow (`800` logical width), Light desktop, and Light/Largest narrow.
  Confirm direct-entry sidebar collapse, shared Editor identity, internal
  menu/toolbar reachability, document/status/details geometry, and taskbar
  contrast; restore secure seat state and accept only inspected frames.
- Current candidate Editor matrix resolution (2026-08-02): exact candidate
  `ae51c124…` passed Editor Dark desktop, Dark narrow (`800`), Light desktop,
  and Light/Largest narrow on both `.138` and Dell `.225`. Direct-entry
  sidebars are collapsed and the shared Editor identity, menu/toolbar,
  document body, status row, details rail, and taskbar remain bounded; hashes
  and links are in the DRM ledger. Both seats were restored to payload
  `20955383…`, secure Dark/Construct/Default/Normal, active service, zero
  restarts, and no proof drop-in. This closes only the exact-candidate Editor
  slice; strict linear scanout, VDI guest readiness, the remaining matrix, and
  WL-UX-009 readiness remain open.
- Current candidate Terminal matrix recapture (2026-08-02): validate exact
  release `ae51c124…` on `.138` and Dell `.225` for Terminal Dark desktop,
  Dark narrow (`800` logical width), Light desktop, and Light/Largest narrow.
  Confirm the complete TERMINAL identity, menu/session controls, shell body,
  taskbar contrast, and bounded narrow layout; restore secure seat state and
  accept only inspected frames.
- Current candidate Terminal matrix resolution (2026-08-02): exact candidate
  `ae51c124…` passed Terminal Dark desktop, Dark narrow (`800`), Light desktop,
  and Light/Largest narrow on both `.138` and Dell `.225`. The complete
  `TERMINAL` identity, menu/session controls, shell body, taskbar, and bounded
  narrow layout remain readable; hashes and links are in the DRM ledger. Both
  seats were restored to payload `20955383…`, secure Dark/Construct/Default/
  Normal, active service, zero restarts, and no proof drop-in. This closes only
  the exact-candidate Terminal slice; strict linear scanout, VDI guest
  readiness, the remaining matrix, and WL-UX-009 readiness remain open.
- Current candidate This Node matrix recapture (2026-08-02): validate exact
  release `ae51c124…` on `.138` and Dell `.225` for This Node Dark desktop,
  Dark narrow (`800` logical width), Light desktop, and Light/Largest narrow.
  Confirm the unified node navigation, health-score/status hierarchy, device
  and local-operations body, large-text scroll boundary, and taskbar remain
  readable and bounded; restore secure seat state and accept only inspected
  frames.
- Current candidate This Node matrix resolution (2026-08-02): exact candidate
  `ae51c124…` passed This Node Dark desktop, Dark narrow (`800`), Light desktop,
  and Light/Largest narrow on both `.138` and Dell `.225`. Unified node
  navigation, status/health hierarchy, device/local-operations body,
  large-text continuation, and taskbar remain readable and bounded; hashes and
  links are in the DRM ledger. Both seats were restored to payload
  `20955383…`, secure Dark/Construct/Default/Normal, active service, zero
  restarts, and no proof drop-in. This closes only the exact-candidate This
  Node slice; strict linear scanout, VDI guest readiness, the remaining matrix,
  and WL-UX-009 readiness remain open.
- Current candidate Phones matrix recapture (2026-08-02): validate exact
  release `ae51c124…` on `.138` and Dell `.225` for Phones Dark desktop, Dark
  narrow (`800` logical width), Light desktop, and Light/Largest narrow.
  Confirm the shared title/header, pairing status, tabs, feature and
  remote-input controls, and bounded large-text body; restore secure seat state
  and accept only inspected frames.
- Current candidate Phones matrix resolution (2026-08-02): exact candidate
  `ae51c124…` passed Phones Dark desktop, Dark narrow (`800`), Light desktop,
  and Light/Largest narrow on both `.138` and Dell `.225`. The shared title,
  pairing state, tabs, feature and remote-input controls, and large-text body
  remain readable and bounded; hashes and links are in the DRM ledger. Both
  seats were restored to payload `20955383…`, secure Dark/Construct/Default/
  Normal, active service, zero restarts, and no proof drop-in. This closes only
  the exact-candidate Phones slice; strict linear scanout, VDI guest readiness,
  the remaining matrix, and WL-UX-009 readiness remain open.
- Current candidate Media matrix recapture (2026-08-02): validate exact
  release `ae51c124…` on `.138` and Dell `.225` for Media Dark desktop, Dark
  narrow (`800` logical width), Light desktop, and Light/Largest narrow.
  Confirm the shared MEDIA identity/menu, source tabs, local/Jellyfin controls,
  honest empty-source state, and taskbar remain readable and bounded; restore
  secure seat state and accept only inspected frames.
- Current candidate Media matrix resolution (2026-08-02): exact candidate
  `ae51c124…` passed Media Dark desktop, Dark narrow (`800`), Light desktop,
  and Light/Largest narrow on both `.138` and Dell `.225`. The shared MEDIA
  identity/menu, source tabs, local/Jellyfin controls, honest empty-source
  state, and taskbar remain readable and bounded; hashes and links are in the
  DRM ledger. Both seats were restored to payload `20955383…`, secure
  Dark/Construct/Default/Normal, active service, zero restarts, and no proof
  drop-in. This closes only the exact-candidate Media slice; strict linear
  scanout, VDI guest readiness, the remaining matrix, and WL-UX-009 readiness
  remain open.
- Current candidate Maps matrix recapture (2026-08-02): validate exact release
  `ae51c124…` on `.138` and Dell `.225` for Maps Dark desktop, Dark narrow
  (`800` logical width), Light desktop, and Light/Largest narrow. Confirm the
  governed map-content palette, health/alert rail, FAB lane, empty-state panel,
  and taskbar remain readable and separated; restore secure seat state and
  accept only inspected frames.
- Current candidate Maps matrix finding (2026-08-02): Dell `.225` Maps
  Light/Largest narrow (`800` logical width) remains rejected because native
  inspection shows the lower red alert pill clipped at the application
  viewport boundary above the taskbar. Evidence is recorded in the DRM
  ledger at `docs/platform/WL-UX-009-DRM-EVIDENCE-2026-07-31.md` with proof
  SHA `cc3c01404c1d96ace43d2cc68cb2fa843339a58b31f8144b545750497f3ddd96`.
  Keep this item open for a layout fix and a fresh direct-DRM recapture; do
  not claim Maps matrix closure or WL-UX-009 readiness.
- Maps alert-stack remediation (2026-08-02): update the Drive HUD's alert
  placement to reserve a bottom-safe viewport margin before painting multiple
  large-text status pills. Add geometry coverage for the Dell narrow profile,
  then rebuild and recapture the rejected `.225` Light/Largest Maps cell on
  direct DRM before changing its worklist disposition. Resolution evidence:
  focused Maps tests pass 274/274; candidate `b75e395a…` was recaptured on
  Dell direct DRM as `a52e044e00e6ba7cc4e305d7b97b8263e45f851b42817431a46288130c2f3b1a`.
  The two large-text status pills are fully visible above the taskbar, and the
  redundant no-data card is suppressed only in the combined no-fix/offline-
  blocked state. The seat was restored to the secure baseline. This resolves
  the recorded clipping finding; strict linear scanout, the remaining route /
  profile matrix, VDI guest readiness, and overall WL-UX-009 readiness remain
  open.
- Build-integrity blocker resolved (2026-08-02): reconciled the This Node
  `show_section_detail` call/definition after the application continuity slice;
  the focused BigBoy suite now compiles and passes 44/44. Live Maps proof
  remains blocked independently by the recorded narrow-layout finding below.
- Local services continuity update (2026-08-02): This Node's Services detail
  now folds a fixed, read-only local systemd failure provider alongside the
  existing mesh-published daemon health. The provider runs off the render
  thread, caps output at 32 unit names, treats systemd absence/refusal as an
  explicit unavailable state, and keeps restart behind the typed Actions
  confirmation/audit/recovery boundary. The focused BigBoy This Node suite
  passes 40/40; physical GUI recapture remains open.
- Printers/peripherals continuity update (2026-08-02): the durable Printers &
  Peripherals route now consumes a fixed, read-only local CUPS `lpstat` probe,
  bounded to 16 sanitized printer names plus local status/default evidence.
  Missing or refused CUPS remains explicitly unavailable; printer jobs,
  queues, USB authorization, and dock mutation remain outside the route until
  typed confirmation/audit/recovery providers exist. The focused BigBoy This
  Node suite passes 41/41; physical peripheral proof remains hardware-gated.
- Firewall posture continuity update (2026-08-02): Security & Privacy now
  consumes a fixed, off-render-thread firewalld `--state` observation and
  distinguishes running, not-running, unavailable, and unknown-provider
  states. Zone/rule detail, encryption, broader security policy, and firewall
  mutation remain explicitly unavailable; the UI does not infer a general
  security posture from one firewalld probe. The focused BigBoy This Node suite
  passes 42/42; physical security-policy evidence remains open.
- Remote-access continuity update (2026-08-02): Virtualization & Remote Access
  now reuses the durable System Remote Proofing policy and derived
  Sunshine/Moonlight/VNC service plan in the This Node detail route. It exposes
  bounded enablement, bind scope, firewall policy, capture/encoder, frame
  target, local approval, indicator, input, fallback, and provider warnings;
  lifecycle and trusted-session mutations remain owned by the existing System/
  VDI authorities. Catalog tests pass 9/9 and focused This Node tests pass
  42/42 on BigBoy.
- Backup posture continuity update (2026-08-02): Backup & Restore now reads
  metadata for the existing encrypted `state-backup.enc` artifact at the
  canonical workgroup/node/mackesd path. This Node reports bounded presence,
  size, modification time, missing, and invalid/symlink states without opening
  or exposing encrypted contents. Passphrase verification and restore remain
  privileged mackesd operations outside the UI. Catalog tests pass 9/9 and
  focused This Node tests pass 43/43 on BigBoy.
- Applications continuity update (2026-08-02): Services & Applications now
  reads the existing bounded `apps-installed.json` and `running-apps.json`
  mirrors under the canonical workgroup/node directory and exposes aggregate
  installed and running counts. Missing, malformed, symlinked, oversized, or
  unavailable mirrors remain explicit unknown/unavailable states; app names,
  launch, and mutation continue through the existing Front Door authority.
  Focused This Node tests pass 44/44 on BigBoy; physical application proof
  remains open.
- Encryption posture continuity update (2026-08-02): Security & Privacy now
  performs an off-render-thread, fixed-root observation of `/sys/class/block`
  device-mapper entries and counts only mappings whose local `dm/uuid` begins
  with `CRYPT-LUKS`. The route reports no mappings, observed encrypted versus
  total mappings, or an explicit provider failure; it never exposes mapping
  names, paths, keys, unlocked state, or a full-disk-encryption claim. Hostile
  fixture coverage passes in the focused This Node suite, now 46/46 on BigBoy.
  Encryption policy and mutation remain provider-gated; physical security
  evidence remains open.
- Security copy truthfulness update (2026-08-02): corrected the mesh-level
  Security & Privacy summary so it no longer contradicts the trusted local
  encryption/firewalld cards. It now distinguishes snapshot-wide policy, local
  observations, camera-permission gaps, and partial-fact limitations. The
  focused This Node suite remains green at 46/46 on BigBoy.
- Physical-evidence audit update (2026-08-02): inspected the recorded direct-DRM
  This Node Dark desktop and Light/Largest narrow frames for hierarchy, health
  glyphs, Inventory/Actions navigation, contrast, taskbar touch targets, and
  scroll continuation. They remain usable layout evidence, but their exact
  candidate predates the later application, encryption, and Security copy
  changes; do not treat those hashes as proof of the newest payload. A fresh
  direct-DRM recapture is required after the next accepted release candidate.
- Local security freshness update (2026-08-02): Security & Privacy now gives
  firewalld and encryption observations independent `Fresh`, `Stale`, or
  `Awaiting local provider` badges. A successful local observation ages out
  after 45 seconds, so a hung or silent worker cannot leave old security facts
  looking current. Focused BigBoy This Node tests pass 46/46; the existing
  mesh-wide health authority remains unchanged.
- Local-provider freshness continuity update (2026-08-02): the same bounded
  freshness badge now appears on local Services, Printers & Peripherals, Backup
  & Restore, and Services & Applications cards. Each successful off-render-
  thread response records its own observation age; a silent worker becomes
  `Stale` after 45 seconds and never masquerades as current. Provider errors
  and not-yet-seen states remain distinct. Focused BigBoy This Node tests pass
  46/46.
- Diagnostics freshness continuity update (2026-08-02): the bounded redacted
  journal provider now shows its own `Fresh`, `Stale`, or `Awaiting local
  provider` badge. A stopped journal worker therefore cannot leave old warning
  and error lines looking current, while the fixed query, redaction, size cap,
  and no-user-query boundary remain unchanged. Focused BigBoy This Node tests
  pass 46/46.
- Hardware-provider freshness update (2026-08-02): the trusted `mde-seat`
  HardwareStatus contract now carries a bounded observation timestamp. Hardware
  detail renders `Fresh`, `Stale`, or `Awaiting provider timestamp` for local
  thermal/fan, storage, firmware, dock, and platform-profile evidence instead
  of relying solely on mesh snapshot age. No paths or mutation verbs cross the
  seam. BigBoy `mde-seat` hardware tests pass 3/3 and focused This Node tests
  pass 46/46.
- Recovery & Reset route continuity update (2026-08-02): This Node now indexes
  a distinct Recovery & Reset hierarchy/search route with a full-page boundary
  that names the privileged provider required for recovery-environment, reset,
  rollback, and destructive restoration controls. The page keeps encrypted
  backup metadata and passphrase-gated `mackesd` verification/restore as the
  existing safe continuity path, and presents no reset action as available.
  Catalog tests pass 9/9 and focused This Node tests pass 46/46 on BigBoy.
- Time/language/region provider update (2026-08-02): the durable Time, Language
  & Region route now consumes a bounded local provider for host locale/language
  values from fixed `/etc/locale.conf` or `/etc/default/locale` files and the
  host time zone from fixed timezone evidence. Values are sanitized, kept
  read-only, and show `Fresh`, `Stale`, or an explicit provider error; display
  clock preference remains owned by the typed System provider, while locale
  mutation, keyboard-region policy, and time synchronization remain gated.
  Focused This Node tests pass 47/47 on BigBoy.
- Full-page responsive evidence update (2026-08-02): the governed detail-route
  render fixture now mounts the typed System provider instead of testing every
  route only through a provider-less fallback, and renders every indexed page
  again at 520px logical width with 1.4x text scale. This covers the real
  locale, personalization, virtualization, and OS-management detail branches
  alongside the device pages. Focused This Node tests pass 47/47 on BigBoy.
- Physical-evidence audit update (2026-08-02): the available Dell proof target
  `172.20.146.225` accepts SSH but currently reports an inactive
  `mde-shell-egui.service` and no `/dev/dri/card0`; the `.138` proof target is
  not reachable on SSH. The checked-in This Node PNGs remain useful visual
  layout evidence, but are not current-payload or physical-control proof.
  Fresh direct-DRM capture and reachable-device action evidence remain open
  until a live DRM seat is available.
- Time-sync and keyboard-region continuity update (2026-08-02): the local
  Time, Language & Region provider now also reads bounded keyboard-region facts
  from fixed host configuration and the fixed `timedatectl` synchronization
  posture. The UI distinguishes synchronized, not synchronized, not reported,
  and provider-error states; it exposes no host mutation or false sync claim.
  Focused This Node tests pass 47/47 on BigBoy.
- Current-payload route validation (2026-08-02): full production-feature
  candidate `2f32f935c92a4cf84f926221a093a3666638fee9063ec4f9a8dc8ef1f686f628`
  was built on BigBoy with `drm,live-vdi,media-mpv`. On `.138`, Music Dark
  desktop, Music Dark narrow (`800` logical), and Music Light/Largest narrow
  were visually inspected and accepted; Media Dark desktop was also captured
  and accepted. Evidence and native readback hashes are recorded in the DRM
  ledger. `.138` was restored to payload `20955383…`, secure login-at-boot,
  active service, and zero restarts. This closes only the current-payload
  Music slice plus one Media cell; the remaining Media profiles, Phones,
  Terminal, Editor, Browser boundary, strict linear scanout, Dell adoption,
  and WL-UX-009 readiness remain open.
- Current-payload route validation continuation (2026-08-02): the same
  production-feature candidate `2f32f935…` was explicitly routed on `.138` to
  Phones, Terminal, and the unified Editor/Communications surface. Dark
  desktop frames were visually inspected and accepted for all three. The
  Editor frame records the approved boundary: Construct owns the Mesh Teams
  host frame and embedded Editor surface; no guest application styling is
  claimed. `.138` was restored to payload `20955383…`, secure login-at-boot,
  active service, and zero restarts. Remaining Light/Largest and narrow cells,
  Dell adoption, Browser/VDI boundaries, strict linear scanout, and overall
  WL-UX-009 readiness remain open.
- Dell adoption validation (2026-08-02): the production-feature candidate
  `2f32f935…` was installed on Dell `.225` and the Phones, Terminal, and
  unified Editor/Communications Dark desktop frames were visually inspected
  and accepted. The Editor frame preserves the approved Construct-owned host
  and embedded-editor boundary. Dell was restored to payload `20955383…`,
  secure login-at-boot, active service, and zero restarts. This closes only
  the Dell Dark desktop route slice; Light/Largest and narrow cells, remaining
  Media coverage, Browser/VDI boundaries, strict linear scanout, and overall
  WL-UX-009 readiness remain open.
- Superseded inventory-health update (2026-08-02, retired 2026-08-03): the
  inventory landing's global score, local freshness rollup, and provider badges
  were removed by the System and Mesh Health cutover. Provider evidence still
  carries freshness, but issue presentation and A–F grading exist only in the
  centered modal.
- Superseded critical-alert update (2026-08-02, retired 2026-08-03): the linked
  This Node AlertInbox and inline health recovery card were removed. Typed
  conditions, acknowledgement, snooze, and guided recovery now live only in
  System and Mesh Health; signed mesh Chat remains notification transport and
  is not a second health ledger.
- NetworkManager SecretAgent boundary update (2026-08-02): `mde-seat` now
  provides an in-process, non-persistent NetworkManager SecretAgent lifecycle
  and a typed profile-activation method using only validated provider-issued
  profile/device object paths. Secret values exist only in the callback and
  D-Bus activation exchange; they are never serialized, logged, or exposed to
  This Node snapshots. Save/delete persistence is refused, malformed metadata
  is rejected, and mesh routes/DNS are not rewritten by activation. The This
  Node profile action remains fail-closed until a trusted-session responder is
  mounted by the shell. BigBoy `mde-seat` network tests pass 8/8 and focused
  This Node tests pass 48/48.
- Phones large-text remediation (2026-08-02): moved the always-available
  `Disarm now` control into the wrapped arm-action lane, removing the needless
  extra row at large text. Phones tests pass 26/26. Candidate
  `cc56fdf0466a29506b8b7adcf27af8aa3f7a034d87bdebfe20143093289f2dbc` was
  recaptured on Dell `.225` Light/Largest narrow; the complete Remote input
  card now ends above the taskbar and is accepted. Dell was restored to
  payload `20955383…`, secure login-at-boot, active service, and zero restarts.
  Remaining route/profile cells, Browser/VDI boundaries, strict linear
  scanout, and overall WL-UX-009 readiness remain open.
- Dell Editor profile validation (2026-08-02): candidate `cc56fdf0…` was
  explicitly routed to the unified Editor/Communications surface on Dell
  `.225`. Dark narrow, Light desktop, and Light/Largest narrow frames were
  visually inspected and accepted. The approved host-owned Mesh Teams and
  embedded Editor boundary remains clear at each profile. Dell was restored to
  payload `20955383…`, secure login-at-boot, active service, and zero restarts.
  Remaining Media profiles, Browser/VDI boundaries, strict linear scanout, and
  overall WL-UX-009 readiness remain open.
- Live-render finding (2026-08-02): Dell `.225` Media Light/Largest narrow
  current-payload proof clips the Jellyfin empty-state line at the taskbar
  boundary. Reject the cell; keep the status copy truthful and make the Media
  content lane taskbar-safe before recapturing the exact candidate.
- Dell Terminal profile validation (2026-08-02): candidate `cc56fdf0…` was
  explicitly routed on Dell `.225` and Dark narrow, Light desktop, and
  Light/Largest narrow frames were visually inspected and accepted. Terminal
  content remains bounded above the taskbar in all three profiles. Dell was
  restored to payload `20955383…`, secure login-at-boot, active service, and
  zero restarts. Remaining Editor/Media profiles, Browser/VDI boundaries,
  strict linear scanout, and overall WL-UX-009 readiness remain open.
- Network profile activation continuation (2026-08-02): This Node/System now
  mounts the in-process NetworkManager SecretAgent only while the trusted
  session is viewing the relevant action surface. Profile activation uses
  typed, provider-issued profile/device object paths, requires a visible
  second confirmation, and collects credentials through an ephemeral modal;
  secrets are not persisted, placed in snapshots, or written to audit output.
  APN/DNS/proxy edits and imported VPN mutation remain unavailable, and the
  action warns that activation may interrupt underlay/mesh reachability.
  BigBoy bridge tests pass 1/1; the existing BigBoy `mde-seat` network suite
  passes 8/8 and focused This Node tests pass 48/48. Recovery/reset, typed
  local service restart, remaining provider gaps, and physical DRM evidence
  remain open.
- Typed service-control continuation (2026-08-02): the existing bounded failed
  systemd-service observation now has a matching `mde-seat` D-Bus provider.
  This Node Actions offers only provider-reported, validated `.service` units;
  the operator must arm and confirm the exact unit, and systemd resolves and
  restarts it without a shell fallback. Provider refusal, absent system D-Bus,
  malformed targets, and stale projections remain visible as unavailable or
  refused; audit output contains only the fixed action label, outcome, and
  timestamp. BigBoy service-provider tests pass 4/4 and focused This Node tests
  pass 48/48. Recovery/reset, update application, remaining provider gaps, and
  physical DRM evidence remain open.
