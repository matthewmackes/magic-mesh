# Platform Worklist

This is the only active platform worklist. Design notes, parity ledgers,
operational runbooks, review notes, and `docs/NEEDS-OPERATOR.md` are evidence
sources, not parallel trackers.

The complete pre-rework worklist, including all progress records and historical
reconciliation text, is preserved at
`docs/worklist-archive/2026-07-28-platform-worklist-pre-rework.md`.

## Current Snapshot - 2026-07-29 execution-first rework

- **7 active epics:** 7 `Remaining`, 0 `Blocked`, 0 `Needs clarification`.
- **P0:** WL-ARCH-008 (standalone old Browser repo plus VM Browser cutover),
  WL-FUNC-011 (Mesh Teams completion and one-product cutover), WL-FUNC-016
  (native text clipboard across seat/mesh/VDI), WL-FUNC-017 (complete Maps,
  navigation, and MG90 radio health), and WL-UX-011 (unified This Node hardware
  center).
- **P1:** WL-UX-009 (shared Quazar theme and design-language completion) and
  WL-UX-012 (full-width Construct taskbar and search-first Home).
- **First parallel wave:** preserve/extract the old Browser stack; implement the
  DRM/mesh/VDI clipboard foundation; repair MG90 cadence and introduce the
  complete radio-health contract; finish shared visual primitives and taskbar
  authority; close open Mesh Teams contract rows; and establish typed This Node
  capability adapters.
- **Integration wave:** cut Construct over to `browser-vm`; finish Mesh Teams
  against the shared clipboard/theme contracts; complete the real navigation
  and offline-map cutover; complete This Node using the same state and visual
  primitives; and cut Construct over to the full-width search-first taskbar.
- **Evidence policy:** unavailable hardware, stale installed packages, rendered
  screenshots, and external-provider demonstrations are recorded honestly but
  do not keep implementation-complete epics active. Runtime claims still require
  live evidence or an explicit unavailable-hardware note.

## Status Vocabulary

- `Remaining` - unfinished implementation that can proceed.
- `Blocked` - unfinished implementation that requires a named external action,
  account, secret, hardware resource, or release authority.
- `Needs clarification` - implementation cannot be specified safely from
  repository evidence and current operator decisions.

Completed and retired work is moved to `docs/worklist-archive/`; it never
remains here under a completed status.

## Core Architecture

### WL-ARCH-008 - Extract the host Browser stack and replace it with a VM Browser

- Status: Remaining
- Priority: P0
- Complexity: Epic
- Problem: Construct still ships a host Browser built around CEF/Servo
  offscreen helpers, shared-memory frame copies, CPU conversion, and shell
  texture uploads. Video refresh is visibly unreliable, Browser activity can
  compete with the shell repaint loop, and native host page execution violates
  the governed thin-client model in which applications run inside VM guests.
  Browser-owned code is spread across shell, daemon, packaging, worker, and
  mixed-purpose files, so deleting only the helper crates would leave reachable
  runtime and package seams.
- Required outcome: The complete old host Browser stack is preserved in an
  independent, history-bearing, clean-clone-buildable repository at
  `matthewmackes/magic-mesh-browser-stack`. Construct ships no CEF/Servo host
  engine, helper, Browser worker family, Browser RPM, or Browser runtime
  installer. `Surface::Browser` remains, but it only starts or resumes a
  dedicated `browser-vm`, attaches through VDI, renders the guest framebuffer,
  and forwards focused input. Chromium, browser chrome, tabs, page execution,
  media decode, and page failures remain inside the guest.
- Current state: The old shell Browser, helper engines, wire/client bridge,
  worker family, RPM variant, CEF/Widevine/model setup, SELinux units, and
  Browser-specific daemon integrations remain in `magic-mesh`. Existing
  Workloads contracts can provision `DeliveryType::DesktopVm`; the VDI stack
  already supports RDP/VNC/SPICE sessions and damage-aware texture uploads.
  RDP audio playback is currently disabled. The full pre-rework Browser
  inventory and decision record is preserved in the 2026-07-28 archive
  snapshot.
- Remaining work:

  1. Record the source commit, workspace/package/process inventory, persistent
     Browser data locations, focused test baseline, and a source-to-destination
     path map. Classify components as Browser-owned, mixed-purpose, or shared.
  2. Create a disposable history-filtered clone and publish
     `matthewmackes/magic-mesh-browser-stack` before deleting source from
     `magic-mesh`. Preserve attribution, relevant history, `LICENSE`, `NOTICE`,
     and an `UPSTREAM-SOURCE.md` provenance record. Never rewrite history in the
     live `magic-mesh` worktree.
  3. Move the old shell Browser UI, every `mde-web-*` crate,
     `mde-browser-workers`, Browser-only mixed-file logic, package assets,
     installers, policies, units, verification helpers, and implementation docs
     into the standalone repository.
  4. Make the new repository independently buildable. Add its own workspace,
     toolchain, lockfiles, CI, package instructions, and minimal local
     compatibility contracts. No path, submodule, or Git dependency may point
     back to `magic-mesh`. Keep CEF and Servo in separate/nested workspaces where
     their native runtime and SQLite constraints require separate lockfiles.
  5. Inventory and back up legacy profiles, bookmarks, history, sessions,
     downloads metadata, policies, passkeys, cache, extension state, and model
     assets. Add an idempotent import/export path for portable Chromium data.
     Never silently export cookies, passwords, private passkeys, or sealed
     credentials into the guest.
  6. Remove the host Browser from `magic-mesh`: workspace members/excludes,
     lockfile edges, shell engine/spawn/frame code, Browser worker registrations,
     Browser-only transfer/MPRIS/KDC/policy consumers, package variant, weak
     dependencies, binaries, runtime/model installers, policies, units, hooks,
     payload expectations, tests, and active host-Browser documentation.
  7. Add a reusable `browser-vm` profile using the existing
     `DeliveryType::DesktopVm` contract. Defaults are 4 vCPU, 8 GiB RAM, and
     64 GiB disk, operator-tunable through Workloads. The image contains
     Chromium, supported GPU/video acceleration, PipeWire integration, guest
     agents, and a supported RDP service.
  8. Replace shell `web` state with a small VM controller. Browser activation
     resolves and starts/resumes the stable workload, waits for its advertised
     desktop source, and attaches over RDP. SPICE then VNC are explicit degraded
     fallbacks, never silent substitutes for the primary performance proof.
  9. Preserve damage rectangles through VDI decode/apply/upload and add metrics
     for frame cadence, full/partial uploads, decode/apply/upload time,
     reconnects, shell repaint, and host CPU/GPU load. Enable RDP audio into
     PipeWire or add an equally typed guest-audio stream.
  10. Keep Construct navigation, status, emergency controls, session switching,
      and global shortcuts host-owned. Forward pointer/key/text/scroll and the
      WL-FUNC-016 VDI clipboard channel only while the viewport owns focus.
      Switching surfaces stops unnecessary host uploads without destroying the
      VM; failure shows bounded retry and actionable unavailable diagnostics.
  11. Remove the stale host Browser visual exception from current authority.
      Guest Chromium owns its UI; only Construct-owned VM connection,
      unavailable, and diagnostic states use the shared Quazar language.
  12. Cut over packages and upgrades only after the remote repository and clean
      clone are proven. Remove obsolete services without deleting user data and
      retain prior signed OSTree/RPM deployment as emergency rollback, not an
      in-tree host Browser fallback.
- Scope: This epic owns old-stack preservation, standalone buildability,
  user-state migration, complete Construct host-stack removal, Browser VM image
  and workload wiring, shell VDI behavior, guest audio/video quality,
  install/upgrade cleanup, docs, and rollback. It does not mirror guest tabs,
  omnibox, extensions, or browser chrome into Construct v1, and it does not add
  a Browser-specific Workloads delivery type.
- Relevant files/components: root workspace and packaging manifests;
  `crates/desktop/mde-shell-egui/src/web/` plus the `mde-web-*` and
  `mde-browser-workers` crates; existing Workloads, VDI, image-build, package,
  and Browser runtime assets; planned sibling checkout
  `/root/magic-mesh-browser-stack`.
- Dependencies: Use the existing typed Workloads placement, authorization,
  lifecycle, and console contracts as a completed, non-blocking foundation.
  Use WL-FUNC-016 for VM clipboard and WL-UX-009 for Construct-owned connection
  and failure states. Repository extraction can proceed in parallel with both.
- Acceptance criteria:

  1. The remote standalone repository records immutable provenance, contains
     every Browser-owned source/asset/doc, preserves relevant history, and
     builds/tests from a clean clone with no sibling checkout.
  2. Its top-level workspace and separate CEF/Servo manifests build the old
     Browser application, helpers, renderer, verifier, workers, and package
     artifacts without production stubs.
  3. `magic-mesh` has no `mde-web-*`, `mde-browser-workers`, host Browser
     engine/spawn/frame path, Browser helper/runtime package, setup unit,
     SELinux policy, Widevine/model installer, or `magic-mesh-browser`
     dependency/payload.
  4. Legacy user data is inventoried and backed up; portable migration is
     idempotent and reports imported/skipped/failed rows; downloads survive;
     secret material is never silently exposed.
  5. Opening Browser starts or resumes `browser-vm` through typed Workloads,
     displays live Chromium over RDP, forwards focused input, and leaves shell
     navigation usable. Missing sources and VM/transport crashes degrade only
     the viewport.
  6. Switching away stops unnecessary host texture work without killing the
     guest; returning resumes the same session; no old Browser helper process is
     present on the host.
  7. Guest `vainfo` succeeds and Chromium media diagnostics report GPU video
     decode for the acceptance stream.
  8. Five concurrent 1080p video tabs run for 15 minutes. The visible tab
     sustains at least 90 percent of source cadence up to the supported target,
     with a minimum target of 30 fps, no unexplained VDI frame stall over
     500 ms, and continuous updates for five stationary-pointer minutes.
  9. Under that load, Construct navigation/session switching is at or below
     100 ms p95 with no measured response over 250 ms; RDP damage produces
     partial uploads; hidden Browser state does not continuously repaint.
  10. Guest audio appears in the host PipeWire VM/application mixer path and
      follows mute/volume policy. A silent fallback is explicitly degraded and
      cannot satisfy release acceptance.
  11. Clean install and upgrade remove obsolete runtime/package state without
      deleting user data. Current docs describe only the VM Browser; historical
      host Browser evidence remains archived.
- Verification method: Build/test the standalone workspace and separate
  CEF/Servo manifests from a clean clone, with the longest native builds on
  BigBoy. In `magic-mesh`, run
  `@farm:{cargo test -p mde-shell-egui --features live-vdi}`,
  `@farm:{cargo test -p mde-vdi-rdp --features live-connect}`,
  `@farm:{cargo test -p mackesd}`, `@farm:{cargo test --workspace}`, and
  `@farm:{cargo clippy --workspace --all-targets --all-features -- -D warnings}`;
  run package/payload, migration, architecture, secret, and supersession gates.
  Complete the video/audio/latency/process/RPM/reconnect acceptance on a
  GPU-capable Workstation with timestamped metrics and rendered-output capture.
  Missing hardware must be recorded without claiming those live criteria pass.
- Origin or merged source IDs: 2026-07-28 operator Option 3 decision to remove
  the current Browser stack from `magic-mesh`, preserve everything Browser in a
  buildable standalone repository, and make the production Browser VM-backed.
  Corrective successor to archived WL-PERF-003, WL-FUNC-001, WL-FUNC-002,
  WL-FUNC-003, WL-FUNC-004, and WL-ARCH-005. Primary evidence source:
  `docs/design/browser-perf-native.md`.

## Functional Completeness

### WL-FUNC-011 - Complete Mesh Teams and cut over one collaboration product

- Status: Remaining
- Priority: P0
- Complexity: Epic
- Problem: The collaboration contracts, projections, workers, and Mesh Teams
  UI have substantial working foundations, but unfinished behavior remains
  split between this functional epic and the retired UX-only WL-UX-010. Pins,
  saved messages, tasks, provider-backed calls, two-way Discord, complete
  coauthoring/file workflows, migration, and removal of superseded surfaces are
  not all production-complete. Keeping backend and interface acceptance in
  separate epics obscures the one-product cutover.
- Required outcome: One `Mesh Teams` Communications product replaces Chat,
  Voice, Editor, Files, Transfers, Notifications, and Clipboard destinations
  without losing accepted behavior. Teams/channels organize signed,
  offline-first messaging, threads, documents, files, transfers, calls, alerts,
  text clipboard history, tasks, Discord provenance, and assistive
  DigitalOcean AI. One epic owns contracts, workers, projections, UI, migration,
  package removal, and final acceptance.
- Current state: `mde-collab-types`, `mde-collab-core`, `mde-collab-egui`, and
  shell/daemon integration exist. App/channel rails, Posts/Files/Calls,
  Activity, rich multiline composition, Details, local find, thread
  resolve/reopen, local reactions, provider placeholders, Discord status
  presentation, bounded Activity folds, and many signed/persistence boundaries
  are implemented. Completed evidence is in the pre-rework archive; the parity
  source is `docs/platform/WL-FUNC-011-parity-ledger.md`.
- Remaining work:

  1. Reconcile the parity ledger against current runtime/tests and leave only
     open rows. Every accepted legacy command, route, hotkey, state writer,
     migration source, package entry, and workflow must map to Mesh Teams or an
     explicit surveyed retirement.
  2. Add shared pin and private saved-message commands, events, projections,
     persistence, permissions, and active UI. Replace the current disabled
     affordances; no pending control may remain at release.
  3. Add basic channel task/action-item contracts, projections, ownership,
     completion state, offline convergence, and Posts/Details presentation.
  4. Complete the real two-way Discord bridge: sealed configuration, channel
     mapping, inbound/outbound provenance, loop prevention, replay/idempotence,
     bounded attachments, health/degraded state, and worker supervision.
  5. Bind real microphone/camera/screen providers and finish direct WebRTC P2P,
     elected LiveKit SFU fallback/failover, SIP ingress/egress, advancing media,
     device switching, call controls, screen share, and remote-control
     escalation. Recording and transcription remain absent.
  6. Complete full-IDE Markdown document entry, Yrs coauthoring, anchored review
     threads, shared presence/follow mode, external-write three-way review,
     version history, and safe Git behavior.
  7. Complete channel Files and transfer-first workflows against stable file
     references, resumable hash-verified transfers, new-member backfill,
     reference-versus-delete semantics, and the shared operation-progress
     authority.
  8. Keep platform clipboard ownership in WL-FUNC-016. Mesh Teams consumes its
     text history/actions; arbitrary MIME and large attachments flow through
     Files/Transfers, not a second clipboard store.
  9. Finish the typed DigitalOcean inference adapter, sealed key/model health,
     bounded consented context, cancellation/rate-limit behavior, suggestion
     attribution, and explicit user-apply flow. AI never mutates canonical
     state autonomously.
  10. Add an idempotent, rollback-safe importer for legacy collaboration,
      alert, clipboard-history, editor, file, transfer, SIP, launcher, and route
      state. Run parity comparison before cutover.
  11. When all parity rows pass, remove superseded routes, workers, state
      writers, crates, package entries, and current docs in one release. Keep
      only bounded migration readers for a documented support window.
  12. Finish desktop, narrow/tablet, and completed Car-interface render,
      performance, unavailable-state, and no-overlap acceptance using shared
      WL-UX-009 primitives.
- Scope: One collaboration domain and one user-facing Mesh Teams destination,
  including signed contracts, storage/replication, workers, UI, media/provider
  adapters, migration, and removal. Out of scope: recording/transcription,
  @mentions, urgent/scheduled messages, emoji/GIF/sticker systems, slash/bot
  platforms beyond Discord, global Mesh Teams search, autonomous AI actions,
  and a permanent old/new compatibility switch.
- Relevant files/components: `crates/shared/mde-collab-types/`,
  `crates/services/mde-collab-core/`, `crates/desktop/mde-collab-egui/`, and
  shell/daemon collaboration integration plus the parity ledger.
- Dependencies: Use WL-FUNC-016 as the single text clipboard authority and
  WL-UX-009 for shared visual/state primitives. The completed Car interface
  supplies glance constraints. Missing live SIP, media, Discord, or model
  credentials are evidence gates, not permission to ship stubs.
- Acceptance criteria:

  1. One Mesh Teams entry replaces every superseded collaboration destination
     and state writer; no competing product surface or dead control remains.
  2. Signed two/three-node partition, replay, membership, tombstone, blob,
     backfill, and migration fixtures converge without loss, duplication,
     invalid authority, or resurrection.
  3. Teams/channels, Activity, Posts/Files/Calls, Details, rich composition,
     local find, threads, pins, saved messages, tasks, alerts, and contextual
     clipboard/transfer actions work with durable state and honest failures.
  4. Documents provide the accepted full IDE, live coauthoring, anchored
     review, safe external-write merge, version history, and non-destructive Git
     behavior.
  5. Files preserve stable identity and distinguish reference removal from
     permanent deletion; transfers resume, verify hashes, backfill members, and
     report through the shared progress projection.
  6. Direct and SFU-relayed calls carry advancing audio/video/screen frames;
     SIP ingress/egress, device controls, failover, and remote-control
     escalation work; no recording/transcription artifact exists.
  7. Discord is genuinely two-way with provenance and loop prevention.
     DigitalOcean suggestions use consented bounded context, never auto-apply,
     and fail without impairing local collaboration.
  8. Migration is repeatable and rollback-safe, the parity ledger has no open
     row, and old runtime/package/doc surfaces are removed after cutover.
  9. Desktop, narrow/tablet, and Car render and interaction tests show no
     overlap, unbounded feeds, hidden commands, placeholders, or fabricated
     provider state.
- Verification method: Run focused unit/property tests for contracts,
  signatures, projections, permissions, replay, CRDT, file/transfer, media,
  Discord, AI consent, and migration. Run deterministic two/three-node fixtures
  plus `@farm:{cargo test --workspace --all-targets}` and
  `@farm:{cargo clippy --workspace --all-targets -- -D warnings}`, placing the
  longest jobs on BigBoy and parallelizing independent crates. Complete live
  media/SIP/Discord/model/file-backfill and rendered DRM evidence where
  resources exist; record unavailable external resources without substituting
  fake success.
- Origin or merged source IDs: `NOTIFY-CHAT`, `EDITOR-*`, `FILEMGR-*`,
  `TRANSFERS-*`, `E12-11`, `VOIP-GW-*`, clipboard/alert-relay workstreams,
  operator editor and Communications surveys, and retired/absorbed WL-UX-010.
  Its completed interface evidence and every unfinished acceptance row are
  owned here.

### WL-FUNC-016 - Native text clipboard across the DRM seat, mesh, and VDI

- Status: Remaining
- Priority: P0
- Complexity: Epic
- Problem: The direct DRM egui seat does not synthesize complete copy/cut/paste
  events or consume platform copy output, while remaining host synchronization
  depends on Wayland tools even though Construct has no compositor. Seat,
  KDC/mobile, Mesh Teams history, and VDI guest clipboard paths do not yet share
  one native text contract.
- Required outcome: Copy, cut, and paste work among local egui apps, authorized
  mesh/KDC producers, Mesh Teams history, and VDI guests through one canonical
  text lane. The active seat can publish and materialize clipboard rows without
  `wl-copy`/`wl-paste`; guests use real protocol channels or report explicit
  unsupported status. Browser participates only as the `browser-vm` VDI guest.
- Current state: The canonical `event/clipboard/clip` body and
  `action/clipboard/{list,pin,unpin,delete,clear}` verbs exist. Daemon history
  and bridge code, Mesh Teams presentation, and VNC cut-text primitives provide
  partial foundations, but the direct DRM seat path and complete bidirectional
  protocol integration are not production-complete.
- Remaining work:

  1. Map direct-seat shortcuts into `egui::Event::{Copy,Cut,Paste}`, consume
     platform `CopyText`, and maintain bounded local text clipboard state.
  2. Publish local copies through `event/clipboard/clip` with stable content ID,
     source identity, RFC3339 time, size bounds, and echo/dedup guards.
  3. Fold that lane into daemon history and implement authorized target-seat
     materialization without polling Wayland tools.
  4. Complete KDC/mobile inbound validation, authorization, size bounds,
     attribution, and active-seat materialization.
  5. Prove bidirectional VNC `ClientCutText`/`ServerCutText`. Add real RDP and
     SPICE text channels where supported; otherwise expose explicit protocol
     capability status instead of fake success.
  6. Route Mesh Teams clipboard UI/actions to the same lane. Route Browser copy
     and paste solely through its VDI protocol after WL-ARCH-008 removes host
     Browser mediation.
- Scope: UTF-8 text clipboard up to the existing 1 MiB guest transport cap,
  direct DRM seat integration, canonical mesh history/actions, KDC/mobile
  inbound materialization, and VDI host/guest channels. Arbitrary MIME, images,
  secret classification/filtering, and direct guest access to host memory are
  out of scope; Files/Transfers owns larger content.
- Relevant files/components: direct DRM handling in `mde-egui` and the shell;
  daemon clipboard sync/IPC/bridge workers; Mesh Teams clipboard presentation;
  `mde-vdi-rdp`, `mde-vdi-vnc`, and `mde-vdi-spice`.
- Dependencies: Preserve the existing event/action wire bodies, VDI
  authorization, echo guards, and 1 MiB cap. Coordinate Mesh Teams consumption
  with WL-FUNC-011 and Browser VM transport with WL-ARCH-008.
- Acceptance criteria:

  1. Copy/cut/paste works among local egui Editor, Terminal, and other text
     surfaces on the direct DRM seat without Wayland tools.
  2. Every accepted producer emits the canonical body, history consumes that
     lane, and authorized rows materialize on the target seat without loops.
  3. KDC/mobile ingress rejects malformed, oversized, unauthorized, duplicate,
     and echo payloads while preserving honest source attribution.
  4. VNC text is bidirectional over real RFB messages; RDP/SPICE use real
     channels or expose unsupported state. `browser-vm` uses the same capability
     and no host Browser exception.
  5. Mesh Teams history/actions operate on the canonical lane and large/non-text
     content is handed to Files/Transfers rather than silently truncated.
- Verification method: Run focused farm tests for DRM shortcut/output handling,
  shell publish/materialize, daemon history and authorization, KDC/mobile
  ingress, Mesh Teams actions, VNC wire messages, and RDP/SPICE capability
  reporting. Complete a live direct-seat round trip among Editor, Terminal,
  Mesh Teams, a VDI desktop, and `browser-vm`; unavailable protocol channels
  must be visibly unsupported.
- Origin or merged source IDs: 2026-07-26 operator report that platform cut and
  paste is unusable, followed by the decision that all text paths connect
  natively through one mesh lane. Reworked 2026-07-28 to remove the retired
  host Browser clipboard model.

### WL-FUNC-017 - Complete Maps, navigation, and MG90 radio health

- Status: Remaining
- Priority: P0
- Complexity: Epic
- Problem: Maps and Car look finished but still contain an unstable MG90
  freshness contract, incomplete radio/GNSS accounting, dead controls, empty
  instruments, inert map rotation/pitch, hard-coded route geometry, fabricated
  lane/speed guidance, and no live Valhalla route session. The MG90 worker
  commonly publishes every 10-13 seconds while the consumer expires data after
  5 seconds. The wire model collapses Wi-Fi A/B, omits Bluetooth health, and
  cannot distinguish missing, failed, disabled, stale, or standby radios.
- Required outcome: Maps is a production offline mapping and turn-by-turn
  product in Construct and Car. MG90 remains a Workstation-attached vehicle
  gateway, never a node role. One typed snapshot accounts for Cellular A/B,
  Wi-Fi A/B, Bluetooth, GNSS, and detected LMR/satellite extensions. A
  persistent accessible health rail and gateway console expose honest
  per-radio state/freshness. Real route, maneuver, map, trip, and vehicle data
  replace placeholders. Carbon requirements retire while egui, shared `Style`,
  Construct, Car, and direct DRM remain governed.
- Current state: `mackesd` publishes `state/vehicle/<node>` with online, GNSS,
  dual-cellular, one collapsed Wi-Fi string, Ethernet/VPN, power, ignition, and
  partial airspace state. The live MG90 reports LTE/WAN and power but no GNSS
  fix; OBD parsing is absent. Raster MBTiles and an FTS5 gazetteer are installed
  for one small region. Maps owns real basemap/search seams, but Valhalla is
  explicitly unwired, route/lane/speed helpers are synthetic, Airspace lacks
  bearing, and several administration controls do not execute a backend action.
- Remaining work:
  1. Consume WL-UX-009's platform-wide Carbon-requirement retirement and update
     the Maps/Car-specific authority while retaining egui, shared `Style`,
     Construct/Car, HIG principles, and always-dark Car. Inspect governance
     history first, leave `AGENTS.md` untouched, and do not create a repo-root
     `CLAUDE.md`.
  2. Add versioned `VehicleState` v2. Include `schema_version`, monotonic
     `sequence`, `observed_at_ms`, `published_at_ms`, `expected_interval_ms`,
     gateway identity/capabilities, bounded radio inventory, feed health, and
     per-domain source/provenance. Accept v1 for one rolling-upgrade release,
     map its missing fields to `Unknown`, then delete the compatibility reader.
  3. Define stable `RadioId` values for Cellular A/B, Wi-Fi A/B, Bluetooth, and
     GNSS plus bounded extensions. Add `Installed`/`NotInstalled`/`Unknown`
     presence and `Active`/`Standby`/`Acquiring`/`Degraded`/`Fault`/`Disabled`/
     `Stale` operation, with reason code, age, configured role, and active-path
     flag.
  4. Add typed radio metrics and SKU discovery. Cellular carries SIM,
     registration, carrier, technology, reported RSSI/RSRP/RSRQ/SINR, address,
     and role; Wi-Fi carries WAN/AP role, association, SSID, band/channel, RSSI,
     clients, and backhaul; Bluetooth carries powered/scan/discoverable and
     bounded device counts; GNSS carries fix, satellites, HDOP/accuracy, dead
     reckoning, rate, and age. Cellular B is `NotInstalled` only when proven;
     antenna state appears only when reported. Ethernet/VPN are paths, not
     radios; LMR/satellite appear only after a typed probe detects them.
  5. Refactor the vehicle worker into independent schedulers: a fast status
     broadcast/lightweight-status receiver, slow LCI/SSH enrichers, and a
     publisher that emits immediately on change plus a heartbeat at most every
     2 seconds. Slow probes may time out without delaying the heartbeat or
     erasing fresher fields. Consumers mark a domain stale after three declared
     intervals and preserve the last sample only with a visible stale state.
  6. Complete proven MG90 adapters for identity, radio state, GNSS, power,
     GPIO, WAN policy, VPN, and device temperature. Decode OBD/HDOBD only from
     captured, sanitized, versioned fixtures that prove the payload schema and
     units. Report `Unsupported`, `NotInstalled`, or a specific probe failure
     instead of zero-filled telemetry.
  7. Add a persistent Radio Health Rail to Free Drive and Active Route. Keep
     stable positions for the six native interfaces so every radio is
     accounted for. Use color plus shape/text: green check active, blue ring
     standby, amber triangle acquiring/degraded, red crossed link fault, gray
     pause disabled, gray slash not installed, and clock outline stale/unknown.
     Keep signal strength and active-uplink selection separate. While moving,
     the rail and its bounded detail sheet are read-only; parked Car and
     Construct expose metrics, age, handoffs, failures, and diagnostics.
  8. Replace MG90 administration with Overview, Radios & GNSS, Vehicle I/O, and
     parked-only Maintenance, all driven by v2. Each enabled control uses an
     allowlisted typed `action/vehicle/*` request with reply, timeout, audit,
     confirmation, and in-motion gate; delete unsupported controls.
  9. Add typed route/session/maneuver/lane/limit/voice/reroute/cancel contracts
     and supervise local Valhalla over Bus request/reply. Disable Start until a
     route and compatible region are ready. Render only provider-returned
     polylines/guidance; delete `ROUTE_UV`, `ALT_UV`, `mock_lanes`,
     `mock_speed_limit`, and production fixtures. Add map matching, bounded
     reroute, maneuver/ETA advancement, cancellation, and restart recovery.
  10. Wrap MapLibre Native as a rendering dependency, not a UI toolkit. Render
      an offscreen RGBA frame into an egui texture on both the production
      `egui_glow` DRM path and windowed wgpu path. Support vector labels,
      collision handling, real route geometry, smooth zoom/bearing/pitch, and
      independent day/night map styles. Do not implement a new vector-map
      engine in Rust; remove rotation/pitch controls until their rendered effect
      and input behavior pass.
  11. Define a signed `OfflineRegionManifest` binding vector tiles, style/font
      assets, gazetteer, Valhalla graph, bounds, versions, byte sizes, and
      digests. Extend the existing region installer for verified staging,
      atomic activation, rollback, storage accounting, update, and removal.
      Never mix graph, gazetteer, and map revisions from incompatible bundles.
  12. Rebuild navigation around Free Drive, Active Route, and Explore. Car uses
      a full-canvas map, one maneuver/ETA card, the health rail, and at most four
      map actions; secondary controls live in sheets rather than a scrolling
      page. Construct adds search, saved places, recents, region management, and
      diagnostics. Replace empty instruments with three to five
      capability-backed readings; collapse missing feeds into one health row
      and repair invalid persisted choices.
  13. Complete trip recording, recent destinations, favorites, route history,
      breadcrumb replay, and export with bounded durable storage and explicit
      start/stop/delete actions. Record only fresh defensible fixes and mark
      gaps; never interpolate missing travel as observed history.
  14. Redesign Airspace around source truth. Identify the radio and scan age for
      each survey, use ranked spectrum/contact presentation when bearing is
      absent, and enable directional radar only for a proven bearing source.
      Prevent scans from disrupting an active Wi-Fi WAN/AP and report that gate
      explicitly. Geolocate contacts only from defensible coordinates.
  15. Replace Maps/Car Carbon-named glyphs and category mappings with
      license-audited repo-native navigation, radio, satellite, vehicle, and
      maneuver SVGs. Use Inter Variable for UI text and tabular instrument
      numerals; reserve display typography for branding. Coordinate reusable
      primitives with WL-UX-009 rather than creating a second platform theme.
  16. Cut over atomically: migrate persisted map/route/tile selections, deploy
      v2 producer before consumer enforcement, remove v1 after the documented
      window, delete simulator/runtime reachability and dead controls, update
      current help/design/governance, and archive superseded Maps plans with an
      explicit disposition.
- Scope: This epic owns MG90 vehicle/radio/GNSS contracts and workers; gateway
  health and safe management; Maps/Car navigation UX; Valhalla routing;
  MapLibre offscreen rendering; offline-region lifecycle; route/trip storage;
  Airspace source truth; Maps/Car icon and typography migration; rollout,
  cleanup, and proof. It does not create an MG90 node role, infer unreported
  antenna faults, configure carrier service, add paid map/feed providers,
  mutate radios while moving, build a general fleet-management product, or
  restyle unrelated Construct surfaces.
- Relevant files/components:
  `crates/mesh/mackes-mesh-types/src/{vehicle,airspace}.rs`,
  `crates/mesh/mackesd/src/workers/{vehicle,airspace}.rs`,
  `crates/desktop/mde-maps-location-egui/`, shell Car integration,
  `install-helpers/install-offline-map-region.sh`, packaging/service manifests,
  shared `mde-egui`/theme assets, and the current platform-interface authority.
- Dependencies: Use WL-UX-009 for shared frame/state/icon primitives without
  blocking data, routing, or renderer work. Preserve existing Bus, sealed
  credential, audit, direct-DRM, offline gazetteer, and overlay contracts.
  MapLibre Native, Valhalla, and their packaged data must remain local/offline
  at runtime. Installed MG90 hardware supplies live evidence; unavailable
  optional modules require explicit notes, not fabricated success.
- Acceptance criteria:
  1. A 30-minute MG90 bench run publishes at the declared cadence with no false
     live/stale flicker, no heartbeat gap over 3 expected intervals, and no
     slow enrichment probe blocking current radio, power, ignition, or GNSS
     state.
  2. Cellular A/B, Wi-Fi A/B, Bluetooth, and GNSS always have one stable
     inventory row. Single-radio variants show Cellular B as not installed only
     when proven; unknown hardware never appears failed or absent.
  3. Every radio passes active, standby, acquiring, degraded, fault, disabled,
     stale, unknown, and not-installed fixture transitions with the specified
     non-color cue. The active uplink is independently identifiable.
  4. Cellular, Wi-Fi, Bluetooth, and GNSS details display only reported metrics,
     correct units, source, and observation age. GNSS no-fix shows satellites
     and freshness without claiming an antenna fault or usable position.
  5. Car shows the complete health rail on Free Drive and Active Route without
     covering maneuver/ETA content. In-motion interaction is read-only and
     bounded; parked and Construct diagnostics expose the full matrix.
  6. MG90 Overview, Radios & GNSS, Vehicle I/O, and Maintenance consume the
     same snapshot. Every enabled control produces a real typed, authorized,
     audited reply; no UI-only toggle, dead button, or unbounded command input
     remains.
  7. OBD values appear only for a verified supported payload. Absent,
     unsupported, malformed, and stale OBD sources remain distinct and never
     produce zero-filled RPM, speed, fuel, temperature, or odometer readings.
  8. Search returns offline results; preview/start use live Valhalla; guidance
     advances and recovers from off-route, restart, cancel, and missing-region
     conditions. No hard-coded geometry, fabricated lane/limit, reachable
     simulator data, or enabled Start action remains when routing is unavailable.
  9. MapLibre renders installed vector tiles, labels, route, bearing, pitch,
      zoom, and day/night style through egui on windowed wgpu and the direct
      DRM/GLES seat. Stationary pointer input does not stop advancing frames.
  10. Region install/update/remove verifies signatures and digests, enforces
      bounds/storage, atomically activates compatible tile/style/gazetteer/graph
      data, rolls back on interruption, and never exposes a mixed bundle.
  11. Free Drive, Active Route, and Explore work at supported Car and Construct
      resolutions with no nested map scrolling, clipped sheets, unreachable
      controls, empty dashboard expanses, or more than four Car map actions.
  12. Instrument defaults choose available high-value data, persist valid user
      choices, repair invalid selections, and collapse unavailable feeds into a
      concise health explanation.
  13. Trips record only fresh fixes, preserve explicit gaps, replay on the real
      map, export through a working action, and honor bounded retention and
      confirmed deletion.
  14. Bearing-less Airspace contacts never appear directional; scan source,
      radio health, age, and disruptive-scan gates are visible and tested.
  15. Maps/Car uses no Carbon-required asset, identifier, or styling rule.
      Current governance no longer mandates Carbon; shared `Style`, Construct,
      Car, HIG principles, and direct DRM remain intact.
  16. Vehicle v1-to-v2 rolling upgrade is tested in both orders, then the v1
      reader and migration-only code are removed after the support window.
  17. Installed MG90 radio/GNSS behavior and final Car/Construct workflows have
      timestamped live/render evidence. Unavailable optional hardware is named
      without weakening fixture, contract, or no-fabrication gates.
- Verification method: Add bounded contract/property tests for vehicle v2,
  radio inventory/states, parser fixtures, cadence, time skew, and v1 rollout;
  deterministic worker tests with delayed/failed probes; route/maneuver,
  map-render, region-integrity, trip, Airspace, in-motion, dead-control, and
  screenshot tests. Run independent focused farm jobs for mesh types, `mackesd`,
  Maps, shell/Car, and packaging; put
  `@farm:{cargo test --workspace --all-targets}` and
  `@farm:{cargo clippy --workspace --all-targets --all-features -- -D warnings}`
  on BigBoy. Run worklist/doc/style/architecture/secret/package gates and
  `cargo fmt --all -- --check`. Finish with a sanitized MG90 replay, 30-minute
  hardware bench proof, direct-DRM Car/Construct captures, route drive/replay,
  offline install/rollback, and explicit evidence for every installed radio.
- Origin or merged source IDs: 2026-07-29 operator review of broken/incomplete
  mapping, navigation, and MG90 data; follow-up requiring visible health for
  every MG90 radio and GPS and permitting removal of all Carbon requirements.
  Corrective successor to archived WL-FUNC-010, WL-FUNC-012, WL-FUNC-013, and
  WL-UX-007. Evidence sources include `docs/design/maps-worldclass-plan.md`,
  `docs/design/maps-live-overlays.md`, the official MG90 hardware/setup
  inventory, and Apple/Google in-vehicle navigation guidance.

## User Interface And Experience

### WL-UX-009 - Complete the shared Quazar workspace design language

- Status: Remaining
- Priority: P1
- Complexity: Epic
- Problem: Shared Quazar fonts, light/dark palettes, style primitives, and some
  surface migrations exist, but user-facing egui workspaces still drift in app
  frames, navigation, state presentation, sheets/popovers, tooltips, motion,
  icons, tables, and internal Editor/Terminal chrome. Current authority still
  mandates Carbon-derived platform icons after the operator retired that
  requirement. Older scope also assumes a host Browser chrome exception that
  conflicts with WL-ARCH-008.
- Required outcome: Every Construct-owned egui surface reads as one dense,
  HIG-principled Quazar platform in Dark and Light: common app frame,
  navigation, state components, sheets/popovers, typography, icons, motion, and
  data presentation. Car keeps its governed always-dark vehicle treatment;
  focused VDI preserves full-screen pixels; guest applications, including
  Chromium in `browser-vm`, remain outside Construct styling. Carbon is not a
  theme or icon requirement; all retained or replacement assets use the shared
  registry, have clear licensing, and form one coherent visual language.
- Current state: Kdam Thmor Pro, Quazar Light, shared typography/palette
  projection, themed tooltip cleanup, and governance alignment have landed.
  Core `mde-egui` navigation, sheet, popover, motion, style, and capture
  primitives exist. The governance and icon-registry comments still carry a
  Mackes-Carbon V2 lock. The archived progress ledger records completed slices;
  an authority cleanup and full surface/internal-chrome adoption sweep remain.
- Remaining work:

  1. Reconcile `AI_GOVERNANCE.md` and the current platform-interface authority
     to retire Carbon theme/icon requirements while preserving egui, shared
     `Style`, Construct/Car, HIG principles, and direct DRM. Then inventory every
     launchable egui surface and classify raw styling, one-off app frames,
     navigation, state views, dialogs, hover text, icons, motion, tables/lists,
     licensing, and dark/light gaps.
  2. Finish the shared app-frame, top-bar/sidebar, loading/empty/stale/offline/
     error/destructive states, sheets, popovers, tooltips, table/list density,
     icon registry/cache, and centralized reduced-motion-safe primitives.
  3. Migrate shell chrome and every launchable workspace to those primitives,
     preserving explicit Maps content-color and focused-VDI pixel exceptions.
  4. Migrate Editor and Terminal internal tabs, toolbars, palettes, sidebars,
     popovers, and status rows without changing editor/terminal behavior.
  5. Apply the shared language to Mesh Teams, This Node, and Construct-owned
     `browser-vm` connection/unavailable/diagnostic states. Do not style the
     guest Chromium viewport or reintroduce host Browser chrome.
  6. Complete Dark/Light, desktop/narrow/large-text, reduced-motion, no-overlap,
     icon licensing/raster, and representative live DRM proof.
- Scope: Current design authority, shared `mde-egui` and brand/icon primitives,
  shell-owned chrome, launchable egui workspace frames, and Editor/Terminal
  internal chrome. Behavior contracts, security/auth, full AccessKit rollout,
  guest application UI, and general native-app hosting are out of scope.
- Relevant files/components: `crates/shared/mde-egui/`,
  `crates/shared/mde-theme/`, shell chrome, and all crates registered in the
  embedded surface inventory.
- Dependencies: Coordinate adoption with WL-ARCH-008, WL-FUNC-011, and
  WL-UX-011. Shared visual work may proceed independently and must not block
  functional contracts or create a second product epic.
- Acceptance criteria:

  1. Current authority contains no Carbon theme/icon requirement. Quazar
     Dark/Light pass palette, contrast, font, shape, licensed-icon, and
     deterministic screenshot tests.
  2. Construct-owned surfaces use shared frames/navigation/state/dialog/tooltip
     primitives unless an explicit governed exception is documented.
  3. Editor and Terminal internal chrome is migrated; dense tables/lists are the
     default operational idiom; motion is centralized and reduced-motion safe.
  4. Maps content exceptions are marked, focused VDI retains full-screen pixels,
     Car stays always dark, and guest Chromium receives no Construct chrome.
  5. Desktop, narrow, large-text, loading/error, and dynamic-data states render
     without overlap, clipping, hidden controls, or unstable geometry.
- Verification method: Run focused and integrated farm tests for `mde-egui`,
  `mde-theme`, the shell, and touched workspace crates; palette/font/icon/frame/
  state/motion and deterministic render tests; style-leak and supersession
  lints; and representative DRM/Sunshine captures when hardware is reachable.
- Origin or merged source IDs: 2026-07-26 operator unified-theme survey:
  HIG Quazar, Dark plus Light, common app frame, shared state language,
  sheets/popovers, dense operational views, centralized expressive motion,
  broad icon adoption, Editor/Terminal internal chrome, and farm/live proof.
  Reworked 2026-07-28 for the VM Browser architecture and 2026-07-29 to retire
  all Carbon theme/icon requirements.

### WL-UX-011 - Unified This Node hardware center

- Status: Remaining
- Priority: P0
- Complexity: Epic
- Problem: Local-node controls are fragmented across This Node, System,
  Storage, Device Manager, About, status chrome, and Control Center. Wi-Fi is
  absent, keyboard backlight has no complete backend/UI, detailed sound is
  read-only, tap-to-click lacks real direct-seat application, and laptop,
  thermal, dock, firmware, privacy, and safe OEM controls do not form one
  coherent hardware-management experience.
- Required outcome: `This Node` is the single searchable, progressively
  disclosed hardware center. Its hierarchy owns Overview; Connectivity;
  Display & Sound; Input; Power & Performance; Hardware; Personalization; and
  Mesh & System. Controls operate through typed node-local contracts, preserve
  mesh reachability, expose honest unavailable/degraded states, and bound
  privileged/OEM writes with confirmation, audit, safety limits, and recovery.
- Current state: Existing System, Storage, Device Manager, About, Control
  Center, status, BlueZ, display, power, firmware, and input code supplies
  partial UI and provider foundations. Several controls are read-only or
  presentation-only, durable routes are duplicated, and no unified typed
  hardware action boundary covers connectivity, audio, input, performance,
  docks, and OEM capabilities.
- Remaining work:

  1. Consolidate durable routes into one This Node sidebar and search index.
     Normalize legacy deep links to the corresponding page; keep Control Center
     transient and status chrome glanceable.
  2. Implement NetworkManager/ModemManager connectivity: Wi-Fi, Ethernet,
     cellular/APN, hotspot, DNS/proxy, and imported WireGuard/OpenVPN. Preserve
     `nebula1`, mesh DNS/routes, and lighthouse reachability; use an in-process
     SecretAgent and never serialize credentials into Bus/log/UI state.
  3. Complete BlueZ and display/sound/privacy pages: pairing/trust/forget;
     display enable/mode/refresh/arrangement/scale/rotation; LCD/DDC brightness;
     PipeWire/WirePlumber output/input/port/profile/app/VM strips and real
     meters; camera/microphone device and privacy state.
  4. Complete per-device keyboard/backlight, pointer, touch, pen, and gesture
     policy through the real udev/libinput direct-seat path, including hotkeys,
     OSD, and functional tap-to-click.
  5. Implement battery/source/health/time/charge-limit/profile/idle/lid/sleep
     behavior plus typed thermals, fan, CPU, GPU, and safe performance-profile
     controls.
  6. Integrate device/driver inventory, firmware, docks/Thunderbolt, storage,
     and existing Surface support into capability-driven pages with honest
     unsupported states.
  7. Add a privileged node-local hardware worker with explicit actions for
     platform profile, bounded fan mode/curve, CPU power limit, GPU profile,
     device enablement, and Thunderbolt authorization. Support standard kernel
     interfaces plus capability-detected Microsoft Surface, Dell, Lenovo, HP,
     and ASUS adapters.
  8. Bound manufacturer writes with arming, confirmation, audit, thermal limits,
     watchdog recovery, and automatic safe-profile fallback. Never expose
     arbitrary sysfs paths, raw MSR/SMI, `/dev/mem`, remote mutation, or shell
     command composition from the UI.
  9. Reconcile Control Center and status chrome with the same typed state:
     connectivity, Bluetooth, sound, LCD/keyboard brightness, power, underlay
     versus mesh health, numeric battery, and microphone/camera indicators.
  10. Finish desktop/narrow/large-text, unsupported-hardware, stale/provider
      failure, destructive confirmation, and physical-device proof using
      WL-UX-009 components.
- Scope: Local workstation hardware state, typed node-local mutation,
  diagnostics, settings consolidation, capability discovery, quick controls,
  and safe OEM adapters. Remote hardware mutation, arbitrary path writes,
  lock/PAM replacement, raw privileged interfaces, and host application
  ecosystems are out of scope.
- Relevant files/components: shell This Node/System/Storage/Device Manager/
  Control Center/status/direct-seat modules; `mde-seat` and shared `mde-egui`;
  typed mesh contracts, daemon hardware/firmware workers, and package/provider
  dependencies.
- Dependencies: Use WL-UX-009 shared frames, state components, sheets, and
  responsive proof profiles. Provider and safety work proceeds independently;
  unsupported physical hardware never justifies fabricated data or controls.
- Acceptance criteria:

  1. One This Node route owns durable local settings/diagnostics; search,
     hierarchy, narrow layout, unavailable states, and legacy-route
     normalization are tested.
  2. Connectivity, Bluetooth, display/brightness, sound/metering, privacy, and
     input controls use real providers and typed contracts, preserve mesh
     reachability, and keep credentials out of observable state.
  3. Keyboard backlight, tap-to-click, device policy, battery/power, thermals,
     fans, CPU/GPU, firmware, docks, storage, and supported OEM controls perform
     real bounded node-local actions.
  4. Unsupported capabilities stay visible but disabled with a reason; provider
     loss/stale data never appears successful.
  5. Privileged actions are allowlisted, armed, audited, thermally constrained,
     watchdog-protected, and recover automatically to a safe profile.
  6. Control Center and status chrome consume the same authority without
     creating a second durable settings hierarchy.
  7. Desktop, narrow, large-text, degraded, and confirmation states have no
     overlap/clipping; available physical controls pass direct-seat proof and
     unavailable hardware is recorded honestly.
- Verification method: Run fixture/contract tests for routing/search and every
  provider/capability adapter; focused farm tests for shell, seat, typed
  contracts, daemon workers, package dependencies, architecture/secret/style
  gates, and workspace build/clippy/fmt with the longest shell job on BigBoy.
  Complete physical proof for reachable connectivity, audio, brightness, input,
  power, firmware/docks, and one safe action per reachable OEM; record explicit
  unavailability for hardware that cannot be exercised.
- Origin or merged source IDs: 2026-07-26 local-node GUI audit and operator
  decisions: one This Node center, full connectivity, progressive disclosure,
  complete laptop depth, safe OEM writes, and first-class Surface, Dell, Lenovo,
  HP, and ASUS capability adapters.

### WL-UX-012 - Full-width Construct taskbar and search-first Home

- Status: Remaining
- Priority: P1
- Complexity: Epic
- Problem: Construct Bottom mode is a fixed-width opaque-black capsule with
  oversized pill geometry, tiny launcher-group labels, undersized responsive
  controls, and no indication of the focused workspace. Fleet & Mesh and
  Workloads are absent. Navigation, workspace shortcuts, remote-desktop pins,
  and placement share one undifferentiated row. The Springboard App Grid
  duplicates discovery even though Front Door already provides complete search
  and routing.
- Required outcome: Construct Bottom mode is a full-width, Windows 11-inspired
  taskbar with icon-only navigation left, a geometrically centered
  user-managed workspace strip, and placement utility right. Start opens and
  focuses Front Door search. New and migrated profiles begin with Fleet & Mesh,
  Workloads, VMs, Terminal, Maps, Mesh Teams, Files, Music, Media, and Browser.
  Exactly one underline identifies the focused workspace; no running/open
  indicator exists. Home is an icon-free wallpaper and the App Grid is deleted.
- Current state: `nav_bar.rs` owns persisted `Floating`/`Docked` placement, a
  640x64 bottom pill, grouped launchers, chooser-pinned desktops, 24px minimum
  shrink behavior, and a slide/melt transition. `front_door.rs` already has
  complete search and focus-on-open. `springboard.rs` still owns grid layout,
  labels, keyboard selection, zoom ghosts, and tile activation.
  `Surface::FleetMesh` exists; Workloads retains the internal
  `Surface::InfraCode` identifier and an obsolete public label.
- Remaining work:

  1. Reconcile taskbar-specific authority after WL-UX-009 retires the global
     Carbon requirement. Supersede the bottom-centered Dock prohibition with
     this full-width Construct taskbar while retaining egui, direct DRM, shared
     `Style`, Construct/Car separation, the top status bar, and focused-VDI
     auto-hide. State that Start opens search and is not a Start menu.
  2. Replace Bottom geometry with a full-screen-width, 48px-high,
     bottom-edge taskbar: no outer margin, square outer corners, 1px top border,
     and shared low-elevation top shadow. Reserve exactly 48px in normal
     workspace layout. Use 40x40 targets, 24px icons, 4px control radii, and
     4px gaps; never shrink targets to the current 24px fallback.
  3. Add three independent layout zones. Put icon-only Start, Back, and Home
     left with 8px outer padding. Center workspace icons against the physical
     screen center, not leftover space. Put only the Bottom/Left placement
     control right. Reserve symmetric center gutters from the larger side
     cluster so unequal clusters never shift the workspace strip.
  4. Add typed `OpenSearch` navigation. Start uses the existing Construct mark
     and calls Front Door `open()` so search opens and requests text-field
     focus. Clicking while visible refocuses the same overlay; it never creates
     a Start menu or second search path. Use the tooltip/accessibility name
     `Start - Search`.
  5. Replace `DOCK_LAUNCHER_GROUPS` with ordered `DEFAULT_TASKBAR_PINS`:
     `FleetMesh`, `InfraCode`, `Desktop`, `Terminal`, `MapsLocation`,
     `Communications`, `Files`, `Music`, `Media`, and `Browser`. Present
     `InfraCode` as `Workloads` without renaming its internal enum or breaking
     persisted/deep-link compatibility. Present `FleetMesh` as `Fleet & Mesh`.
  6. Version `settings-nav-bar.json` with `schema_version`, existing serialized
     placement, and ordered `pinned_surfaces`. Migrate a legacy mode-only file
     to the ten defaults while preserving placement. Retain the first duplicate,
     discard unknown surfaces, bound the list to the searchable catalog, and
     fall back only when no valid versioned pin list exists. After migration,
     user choices are authoritative and defaults are not silently restored.
  7. Add personalization. Front Door app results expose `Pin to taskbar` or
     `Unpin from taskbar`. Taskbar icons expose `Move left`, `Move right`, and
     `Unpin from taskbar`; pointer drag reorders the same list. Persist context
     actions and completed drops immediately. Start, Back, Home, overflow, and
     placement controls cannot be pinned, moved, or removed.
  8. Add icon-only responsive overflow without recreating an App Grid. Keep
     40px targets and move excess pins into a `MoreHorizontal` single-column
     flyout with tooltips/accessibility names. Close it on activation, Escape,
     or click-away. If focus would be hidden, promote that icon into the final
     visible slot for the frame without changing persisted order.
  9. Pass a focused target to the taskbar: `Home`, `Surface(Surface)`, or
     `DesktopSource(id)`. Paint exactly one centered 18x3 accent underline 2px
     above the bottom. Normalize Workbench, Mesh Map, and Explorer aliases to
     Fleet & Mesh; match active VDI sources to their pinned desktop icon; mark
     Home on wallpaper. Search leaves the underlying marker unchanged. Never
     paint running/open/recent markers or expose taskbar Close actions.
  10. Delete all taskbar grouping labels, label geometry, headings, group gaps,
      and group-aware tooltip wording in Bottom and Left modes. Retain taxonomy
      only where search or compile-time surface coverage consumes it. Delete
      Springboard tile plates, labels, grid layout, keyboard selection, tile
      activation, open-presence zoom ghosts, and their tests. Preserve the
      wallpaper Home, Home intent, and pull-down-to-search gesture in a reduced
      Home gesture layer.
- Scope: Construct taskbar geometry, appearance, focus state, Start/Search,
  default and user pins, overflow, preference migration, Bottom/Left
  compatibility, icon-free Home, and App Grid removal. Car chrome, top-status
  contents, search ranking/providers, workspace business logic, VDI protocols,
  guest UI, and general accessibility rollout are out of scope. Taskbar
  tooltips, context menus, search results, and accessibility names may use text;
  persistent taskbar and overflow controls remain icon-only.
- Relevant files/components: Construct shell navigation and action dispatch;
  Front Door result actions; Springboard/Home gesture layer; shared
  `mde-egui::Style` and the existing icon registry.
- Dependencies: Coordinate colors, elevation, motion, Dark/Light behavior, and
  shared interactions with WL-UX-009 without blocking navigation work. Preserve
  WL-ARCH-008's `Surface::Browser` route during its `browser-vm` cutover. Do not
  duplicate WL-FUNC-017's Maps/MG90 implementation.
- Acceptance criteria:

  1. Bottom mode paints a full-width 48px taskbar with square screen-edge
     geometry, shared Dark/Light material, 40px targets, and no pill, margin,
     group heading, or raw opaque-black requirement.
  2. Start, Back, and Home are left-aligned; user pins are centered on the
     physical screen; placement is right-aligned; side-cluster width changes do
     not move the centered strip.
  3. Start opens and focuses Front Door without a Start menu, duplicate search
     engine, or duplicate overlay.
  4. Fresh and legacy profiles show all ten defaults, including Fleet & Mesh
     and Workloads. Pin, unpin, drag/context reorder, restart persistence,
     malformed recovery, and placement preservation are tested.
  5. Exactly one focus underline appears for Home, every pinned surface, Fleet
     & Mesh aliases, Workloads, and matching pinned desktops. No open/running
     marker or taskbar Close action exists.
  6. Narrow layouts retain 40px targets, keep focus visible, and place hidden
     pins in the icon-only single-column overflow without overlap or clipping.
  7. Bottom and Left modes contain icons only and share persisted pin order; no
     `Infra`, `Ops`, `Life`, or other taskbar group labels remain.
  8. App Grid tiles, labels, selection, activation, and zoom ghosts are removed
     and unreachable. Home remains wallpaper-backed; all surfaces remain
     discoverable through Start/Search.
  9. Back history, Home, surface routing, chooser-pinned desktop connection,
     placement persistence, reduced motion, lock-curtain priority, and
     immersive-VDI auto-hide continue to work.
  10. Governance, platform-interface authority, implementation comments, tests,
      and this worklist consistently describe the taskbar and contain no stale
      pill/App Grid lock.
- Verification method: Run worklist, doc-supersession, diff, and focused
  preference/geometry/action/search/context/drag/focus/overflow/Home tests. Run
  complete `mde-shell-egui`, `mde-egui`, and `mde-theme` tests on the build
  farm, with the longest shell gate on BigBoy. Produce deterministic Dark/Light
  captures at 480x480, 800x600, 1280x800, and 1920x1080 covering normal,
  focused, overflow, search-open, and reduced-motion states. Complete
  representative direct-DRM or Sunshine/Moonlight proof when a seat is
  reachable; otherwise record hardware unavailability honestly.
- Origin or merged source IDs: 2026-07-29 operator dock review: add Fleet &
  Mesh and Workloads; use Windows 11-style full-width Bottom geometry; put
  navigation left and user-pinned workspace icons center; add Start opening
  search; show only the focused-workspace indicator; remove group labels; use
  icon-only persistent chrome; remove the App Grid; and retain wallpaper Home.
  Supersedes conflicting Springboard Dock portions of the 2026-07-22/26
  interface locks without reviving the retired Start-menu implementation.

## Stewardship

This file is the only active tracker. An active epic describes unfinished work,
not its chronological implementation diary.

### ID Scheme

- Every active item is `### WL-<FAMILY>-<NNN> - <title>`.
- Valid families are `ARCH`, `BUILD`, `CRIT`, `DOC`, `FUNC`, `PERF`, `RUN`,
  `SEC`, `TEST`, and `UX`.
- IDs are zero-padded and never reused or renumbered after archival.
- Old source IDs remain in `Origin or merged source IDs`; they are not valid
  active headings.

### Required Fields

Every active epic carries these fields exactly once and in this order:

| Field | Rule |
|---|---|
| `Status` | `Remaining`, `Blocked`, or `Needs clarification`. |
| `Priority` | `P0` through `P3`. |
| `Complexity` | `Small`, `Medium`, `Large`, or `Epic`. |
| `Problem` | User-visible, architectural, security, or correctness gap. |
| `Required outcome` | Observable end state that closes the epic. |
| `Current state` | Concise landed foundation and exact gap; maximum 12 physical lines. |
| `Remaining work` | Ordered executable implementation, migration, and rollout slices only. |
| `Scope` | Explicit in-scope and out-of-scope boundaries. |
| `Relevant files/components` | Concrete starting points, not an exhaustive repository dump. |
| `Dependencies` | Optional; active blocking/coordination relationships only. |
| `Acceptance criteria` | Verifiable closure conditions. |
| `Verification method` | Farm, fixture, live, migration, and lint evidence required. |
| `Origin or merged source IDs` | Lineage and absorbed workstreams. |

An active epic may contain nested numbered milestones, but it may not contain a
top-level `Progress` field. Completed-slice evidence belongs in Git history or a
dated archive snapshot. Active epics are limited to 220 physical lines.

### Completion And Archival

- On completion or retirement, move the epic to a dated note under
  `docs/worklist-archive/` with a one-line disposition and concrete file, test,
  wire, farm, or live evidence.
- Record optional unavailable-hardware or external-provider proof honestly; do
  not retain otherwise completed implementation solely to await that proof.
- Keep the ID in the archive forever. Never leave `Done` or `Completed` status
  in this active file.
- Batch compaction may preserve the full pre-rework file as a historical
  snapshot when that is safer than selectively deleting evidence.

### Duplicate-Workstream Rule

- One user-visible product or architectural cutover has one epic. Backend,
  worker, and interface layers are implementation lanes, not separate epics,
  unless they are independently releasable outcomes.
- Before adding an epic, search active headings, origin fields, and archived IDs.
  Extend or absorb the existing owner instead of creating a parallel tracker.
- Shared primitives such as clipboard, theme, VDI, or typed contracts stay
  separate only when multiple products consume them and they have independent
  acceptance.

### Evidence And Enforcement

- Completion claims cite concrete files/tests or live/wire artifacts. Intent is
  not evidence.
- GUI/runtime claims require farm/live proof or an explicit unavailable-hardware
  note.
- `install-helpers/lint-worklist.sh` enforces field presence/order, values,
  active-epic bounds, snapshot counts, no progress diaries, line length, secret
  shapes, and cargo-only `@farm` payloads.
- Run `install-helpers/lint-worklist.sh --self-test`,
  `install-helpers/lint-worklist.sh`,
  `install-helpers/lint-doc-supersession.sh`, and `git diff --check` for every
  worklist structure change.
