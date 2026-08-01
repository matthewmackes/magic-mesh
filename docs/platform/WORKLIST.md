# Platform Worklist

This is the only active platform worklist. Design notes, parity ledgers,
operational runbooks, review notes, and `docs/NEEDS-OPERATOR.md` are evidence
sources, not parallel trackers.

The complete pre-rework worklist, including all progress records and historical
reconciliation text, is preserved at
`docs/worklist-archive/2026-07-28-platform-worklist-pre-rework.md`.

## Current Snapshot - 2026-07-30 fit-for-purpose audit integration

- **9 active epics:** 9 `Remaining`, 0 `Blocked`, 0 `Needs clarification`.
- **P0:** WL-ARCH-008 (standalone old Browser repo plus VM Browser cutover),
  WL-FUNC-011 (Mesh Teams completion and one-product cutover), WL-FUNC-016
  (native text clipboard across seat/mesh/VDI), WL-FUNC-017 (complete Maps,
  navigation, and MG90 radio health), WL-UX-011 (unified This Node hardware
  center), and WL-CRIT-006 (production evidence, six-node acceptance, and
  corrected-forward recovery).
- **P1:** WL-UX-009 (shared Quazar theme and design-language completion),
  WL-UX-012 (full-width Construct taskbar and search-first Home), and
  WL-FUNC-018 (seamless Flatpak Front Door backed by App VMs).
- **First parallel wave:** preserve/extract the old Browser stack; implement the
  DRM/mesh/VDI clipboard foundation; repair MG90 cadence and introduce the
  complete radio-health contract; finish shared visual primitives and taskbar
  authority; close open Mesh Teams contract rows; and establish typed This Node
  capability adapters.
- **Integration wave:** cut Construct over to `browser-vm`; finish Mesh Teams
  against the shared clipboard/theme contracts; complete the real navigation
  and offline-map cutover; complete This Node using the same state and visual
  primitives; cut Construct over to the full-width search-first taskbar; and
  make Flatpak applications appear as Front Door apps backed by App VMs.
- **Delivery policy:** build one integrated engineering-preview release from the
  nine epics. Publish the preview with unavailable live evidence documented;
  production promotion is a separate decision and requires every selected live
  gate to pass. The preview is not a production-readiness claim.
- **Rollout policy:** deploy the preview to Dell, Eagle, and seat 15
  simultaneously, using seat 15 for observation. Require all three lighthouses
  and the complete six-node substrate before production promotion. Do not rely
  on rollback; a failed node is repaired by re-enrollment and corrected forward
  deployment.
- **Repository policy:** the local GitHub checkout is authoritative for commits
  and pushes. Farm lanes fetch the pushed revision; BigBoy runs the longest
  build/test gate. Heavy build and release validation remains farm-only.
- **Evidence policy:** implementation metrics and deterministic tests remain
  required for acceptance, but no new post-release monitoring surface is added.
  Human GUI review is the required visual signoff. Missing live hardware,
  external providers, or stale installed packages are recorded honestly for the
  preview and remain hard gates for production promotion.
- **Audit record:** the 2026-07-30 fit-for-purpose finding and all 25 selected
  actions are recorded in
  [`docs/platform/FIT-FOR-PURPOSE-AUDIT-2026-07-30.md`](FIT-FOR-PURPOSE-AUDIT-2026-07-30.md).
  `AUD-*` labels there are evidence references, not a parallel tracker; action
  ownership is this file's active epics.

## Survey Decision Register - 2026-07-29

The following decisions are normative modifications to the active epics below.
The latest survey answer wins when it conflicts with an older planning note;
the current code, governance, and archive remain evidence sources. These are
implementation instructions, not a second worklist.

### Integrated release actions

1. Keep the seven active epics separate for ownership and acceptance, but build
   one integrated engineering-preview release. Do not mark an epic complete or
   promote the platform to production until its stated production gates pass.
2. Use the local checkout as the source of truth. Commit and push the selected
   revision directly to `master`; farm jobs fetch that exact GitHub revision.
   No additional review guard is required by this operator decision, but farm
   gates remain mandatory.
3. Keep Fedora 43 thin-lighthouse packages separate from Fedora 44 workstation
   packages. Auto-upgrade role-package drift during deployment and verify the
   resulting role contract before declaring a node healthy.
4. Deploy the preview to Dell, Eagle, and seat 15 simultaneously; use seat 15
   as the observation seat rather than a pre-rollout canary. Require all three
   lighthouses, canonical substrate completeness, hard resource budgets, and a
   hard free-space floor for production promotion.
5. If an upgrade fails, stop promotion, preserve diagnostics, re-enroll the
   failed node, and deploy a corrected revision forward. Do not make rollback a
   required recovery path. Preserve prior signed artifacts for provenance only.
6. Record the operator decisions in this file and keep dated archive snapshots
   immutable. Completed implementation with missing live proof is archived with
   the evidence gap for the preview, while production promotion remains gated.

### Cross-epic contract actions

1. Make the public `magic-mesh-browser-stack` repository history-bearing and
   independently buildable before deleting the host Browser. Browser VM sizing
   is hardware-adaptive with safe operator bounds; RDP and Sunshine/Moonlight
   are equal first-class display paths with explicit health and selection.
2. Make Mesh Teams the single collaboration destination. Use named node channels
   (`System · Dell`, `System · Eagle`, and `System · 15`), combined etcd/overlay
   roster validation, deterministic last-writer-wins offline conflict handling,
   per-user saved messages plus team pins, minimal channel tasks, disabled-until-
   configured Discord, trusted-session remote control, and separate reference
   removal versus permanent deletion.
3. Make Mesh Teams the owner of persisted team clipboard history/actions. The
   clipboard epic owns the transport and seat/VDI contract; use UTF-8 text,
   same-team synchronization, mesh-membership trust, and session-only retention.
   Files/Transfers remains the path for non-text or oversized content.
4. Make Maps offline-capable with a native local routing engine and open or
   zero-cost feeds. Show source age and disable unsafe actions for stale data.
   Use the retained MG90 mirror with direct polling as a stale-data fallback,
   enable GPS when Maps opens, retain no additional motion policy, and support
   Quazar Dark/Light in Car. Live MG90 proof is a production gate, not a preview
   blocker.
5. Make This Node the only durable local settings route without retaining
   legacy aliases. Keep Control Center transient. Allow typed, admin-authorized
   remote hardware actions, use trusted-session authorization, expose unsupported
   capabilities visibly, auto-recover unsafe profiles, warn before network
   disruption, and require all named OEM adapters for production.
6. Make Quazar the only platform visual language with Dark and Light modes,
   balanced information density, licensed shared icons, expressive Apple-like
   shared motion as the default, the Terminal-pattern top bar with the
   previously approved exemptions, and a 25%-thinner two-row zebra side tab bar.
   Preserve the governed Maps content-color and focused-VDI exceptions; guest
   Browser chrome remains outside Construct styling.
7. Make the taskbar full-width with 40px targets, Front Door search as Start,
   user-selected first-boot pins, pin/unpin personalization without reordering,
   one focus underline, icon-only overflow, physical-screen centering, shared
   Bottom/Left order, deleted App Grid, and focused-VDI auto-hide.

### Operator decisions - 2026-07-31

1. Remove OpenStack/Nova references and implementation code completely. The
   provider-neutral replacement path is the required destination; do not retain
   OpenStack compatibility or user-facing terminology.
2. Fleet-wide search/index access is authorized with a seven-day retention
   window. Expired data must be removed and access remains subject to the
   existing mesh trust and authorization model.
3. A real screen-reader/TTS consumer is not required for the current scope.
   Retain accessible touch interaction as a requirement for tablet surfaces.
4. Resolve the Browser chrome visual ruling using platform best practice:
   conform it to the Quazar dark/light design tokens, with touch-sized tablet
   controls where applicable.
5. Integrate LibreOffice as a dedicated App VM, not as a host-native or
   in-process dependency. Expose Writer, Calc, and Impress through the
   Workspace shortcut group; launch and reconnect through the existing VDI
   session path. Files must offer the appropriate LibreOffice association for
   supported office formats.
6. Printing is a Construct-owned, world-class workflow over the App VM:
   printer discovery and health, local/mesh printer selection, live
   page-faithful preview, paper/tray/orientation/margins, duplex/binding,
   color, copies, collation, ranges, scaling, N-up/booklet, PDF output,
   presets, progress, cancel, retry, and tablet-sized touch controls.

### Unified This Node survey completion - 2026-07-31

The operator requested completion at the 50-question cap. Questions 45-50
were closed with the recommended defaults below so the resulting decisions are
recorded before implementation. These choices extend WL-UX-011 and do not
close it or imply production readiness.

45. A critical health state opens an urgent alert with affected components,
    current impact, and guided recovery actions.
46. Notifications are configurable and persistent, support severity,
    acknowledgement, snooze, and escalation, and remain linked to the owning
    health item.
47. Health views provide safe, guided recovery actions with explicit impact
    and confirmation before mutation.
48. All operators may view health; only privileged roles may configure health
    policy or override a reported state, with every change audited.
49. Health refresh is live or on demand according to the view, and every
    stale or unavailable interval is explicit rather than silently inferred.
50. Health and This Node history can be exported as operator-readable,
    audit-backed evidence without exporting credentials or secret material.

### MG90 ownership, sharing, and cache contract

1. Treat an MG90 as a network device attached to a workstation, not as a mesh
   node or lighthouse. MG90-local state includes its radios, GNSS, IMU, WAN,
   Ethernet, power, vehicle I/O, firmware, capabilities, and diagnostics.
   Workstation-local state includes management credentials, the typed vehicle
   worker, Bus publication, authorization, and audit. `management_node_id` is
   the assignment; a workstation may manage multiple MG90 devices.
2. Give every MG90 an independent snapshot and stable identity using ESN plus
   operator alias. Permit multiple active workstation managers after discovery
   and explicit approval. One approval covers all enrolled workstations; any
   enrolled node may revoke immediately, revocation removes every assignment,
   and re-sharing requires fresh approval. Only workstation roles may manage.
3. Publish snapshots at
   `state/vehicle/<management-node>/<mg90-id>`. Deduplicate competing manager
   publications to the freshest valid complete snapshot, retain only the latest
   stale manager snapshot for diagnostics, and show MG90 identity, management
   node, source, age, and sharing state in normal Maps, Car, This Node, and
   other relevant views. Remote views render the same reported domains as
   read-only remote data; they never invent local hardware state.
4. Use direct Ethernet first, then an authorized mesh path. Lighthouses are
   transparent Nebula relays only: they never manage or store MG90 snapshots.
   Fall back automatically to the healthy lowest-latency lighthouse, migrate
   streams to a better relay, pause for a full snapshot resync, render the last
   values with an explicit resyncing state, queue actions during resync, expire
   queued actions with a visible failure, and reject duplicate queued writes for
   the same setting while one is pending.
5. Allow any active authorized manager to issue idempotent MG90 actions. Order
   concurrent actions by mesh arrival time using last-accepted-action-wins;
   actions already in flight may finish after revocation. A failed manager
   triggers automatic takeover by another manager, the returning original
   manager resumes automatically, queued actions are discarded during takeover,
   and all nodes receive ephemeral takeover notifications. MG90 availability
   is optional for workstation mesh readiness; all nodes retain the last shared
   telemetry as stale when managers are offline.
6. Keep stale data in a viewer-local, OS-protected cache for 24 hours. Cache
   telemetry, redacted raw diagnostics, and action outcomes but never
   credentials. Any local user may view or purge one MG90 or the whole cache;
   revocation clears it immediately. Show alias/ESN, management node, last-seen
   time, stale age, and relay path. Stale views are read-only. Under disk
   pressure disable caching while live telemetry/actions continue with an
   affected-view warning; automatic expiry and manual purge restore normal
   behavior when space is available.

### Fit-for-purpose audit actions - 2026-07-30

The operator selected the following cross-cutting decisions. They are owned by
`WL-CRIT-006` unless an existing feature epic is named; do not create duplicate
epics for these rows. The complete rationale and interface requirements are in
[`docs/platform/FIT-FOR-PURPOSE-AUDIT-2026-07-30.md`](FIT-FOR-PURPOSE-AUDIT-2026-07-30.md).

1. GitHub required checks are authoritative; the build farm is the heavy
   self-hosted execution backend and publishes signed evidence.
2. Production requires three lighthouses and three workstations.
3. Failed nodes recover through corrected-forward re-enrollment; rollback is
   not a required recovery path.
4. Flat trust gains capability quarantine, default-deny workload exposure, and
   explicit blast-radius diagnostics.
5. Enrollment becomes one-time, scope-bound, guided, approved, and auditable.
6. Readiness becomes capability-based rather than one all-or-nothing verdict.
7. Every replicated domain declares its deterministic merge and provenance rule.
8. The host Browser is removed in favor of the dedicated Browser VM.
9. VDI sessions receive stable workload identity and resume/reconnect behavior.
10. Clipboard uses one bounded UTF-8 text contract; Files/Transfers owns larger
    or non-text content.
11. Mesh Teams is the sole collaboration destination.
12. This Node is the only durable local-settings authority.
13. Hardware mutations use typed, allowlisted, audited adapters with safe
    fallback and watchdog recovery.
14. Maps is offline-first and MG90 state is typed, fresh, sourced, and honest.
15. Quazar owns shared Dark/Light tokens, typography, icons, motion, and states.
16. The taskbar is full-width and Start opens focused Front Door search.
17. Releases publish an explicit Fedora compatibility matrix using the oldest
    supported ABI.
18. Signed provenance binds source, artifacts, SBOM, static gates, and live
    evidence.
19. Incident bundles correlate health, audit, worker, transport, workload, and
    operator events.
20. Replicated live state is the target recovery model; current encrypted
    backups remain mandatory until peer-recovery proof passes.
21. Lighthouse failover is automatic and visibly degraded while recovering.
22. Workloads require capability and resource-budget admission.
23. Data uses minimal retention with explicit replication, TTL, redaction, and
    purge rules.
24. The immutable image exposes bounded capability profiles rather than one
    unqualified all-in-one contract.
25. A permanent six-node integration and chaos testbed is a production gate.

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
  public, history-bearing, clean-clone-buildable repository at
  `matthewmackes/magic-mesh-browser-stack`. Construct ships no CEF/Servo host
  engine, helper, Browser worker family, Browser RPM, or Browser runtime
  installer. `Surface::Browser` remains, but it only starts or resumes a
  dedicated `browser-vm`, attaches through VDI, renders the guest framebuffer,
  and forwards focused input. Chromium, browser chrome, tabs, page execution,
  media decode, and page failures remain inside the guest.
- Current state: The old shell Browser, helper engines, worker/package seams,
  and Browser-specific daemon integrations remain in `magic-mesh`; Workloads
  provision `DeliveryType::DesktopVm`, while VDI supports RDP/VNC/SPICE and
  damage-aware texture uploads. The 199-path source/destination inventory and
  fail-closed verifier remain under `docs/design/browser-stack-extraction/` and
  `install-helpers/`. Typed `browser-provision` has the 4-vCPU/8192-MiB/64-GiB
  baseline and tests; `packaging/browser-vm/profile.env` fixes guest identity,
  provenance, Chromium/Sway ownership, transport preference, and the resource
  floor with a fail-closed verifier. Front Door/AccessKit expose truthful guest
  states; the image-owned validator admits only bounded identity/digest/transport
  records and rejects commands, URLs, paths, symlinks, and extra inputs. Its farm
  contract suite passes; live realization and extraction remain open.
- Remaining work:

  1. Re-run and review the source commit, workspace/package/process inventory,
     persistent Browser data locations, focused test baseline, and
     source-to-destination path map before each extraction batch.
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
     `DeliveryType::DesktopVm` contract. Size it from host capabilities within
     safe operator bounds, retaining 4 vCPU, 8 GiB RAM, and 64 GiB disk as the
     baseline profile. The new repository-owned profile contract fixes the
     guest identity, Chromium/Sway ownership, Sunshine default, RDP alternate,
     and `host_browser=false`; the image still must contain Chromium, supported
     GPU/video acceleration, PipeWire integration, and guest agents.
  8. Replace shell `web` state with a small VM controller. Browser activation
     resolves and starts/resumes the stable workload, waits for its advertised
     desktop source, and exposes RDP and Sunshine/Moonlight as equal first-class
     display paths with explicit transport health and user-visible selection.
     Sunshine/Moonlight is the default; unavailable default transport requires
     an explicit one-time RDP choice rather than silent switching. Store the
     mesh-wide preference in replicated settings, expose it in Browser settings
     and This Node, and apply changes on the next Browser launch. If the selected
     path is unavailable, offer the alternate path and a preference change; if
     Sunshine/Moonlight audio fails, offer RDP.
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
  12. Cut over packages and upgrades only after the public repository and clean
      clone are proven. Remove obsolete services without deleting user data.
      Preserve prior signed artifacts for provenance, but recover failed
      upgrades by re-enrollment and corrected forward deployment rather than a
      required rollback path.
- Scope: This epic owns old-stack preservation, standalone buildability,
  user-state migration, complete Construct host-stack removal, Browser VM image
  and workload wiring, shell VDI behavior, guest audio/video quality,
  install/upgrade cleanup, docs, and forward recovery. It does not mirror guest tabs,
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
     displays live Chromium over RDP or Sunshine/Moonlight, forwards focused
     input, and leaves shell navigation usable. Missing sources and VM/transport
     crashes degrade only the viewport. Switching transport preserves the same
     VM session and never silently changes the global preference.
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
      follows mute/volume policy. A silent fallback cannot satisfy production
      acceptance; the engineering preview may record unavailable audio evidence.
      Preview transport failure must show an explicit degraded warning and
      production promotion requires working audio on the selected path.
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
  DigitalOcean AI. Every enrolled workstation has a named system channel:
  `System · Dell`, `System · Eagle`, and `System · 15`. One epic owns contracts,
  workers, projections, UI, migration, package removal, and final acceptance.
- Current state: `mde-collab-types`, `mde-collab-core`, `mde-collab-egui`, and
  shell/daemon integration exist. App/channel rails, Posts/Files/Calls,
  Activity, rich multiline composition, Details, local find, thread
  resolve/reopen, local reactions, provider placeholders, Discord status
  presentation, bounded Activity folds, and many signed/persistence boundaries
  are implemented. Shared pin/save commands, events, projections, persistence,
  and projected UI affordances now have focused farm coverage. Completed
  task/action-item update, complete, reopen, membership, bounded-read, and projected UI slices now also have focused farm coverage. Severity-aware
  Activity coalescing, truthful repeat counts, visible pause/resume, keyboard
  posting, and AccessKit send labels now have focused farm coverage. Completed
  evidence is in the pre-rework archive; parity source is `docs/platform/WL-FUNC-011-parity-ledger.md`; Calls unavailable-state
  semantics have accessible labels and 19 focused tests pass at `76dd2457`.
- Remaining work:

  1. Reconcile the parity ledger against current runtime/tests and leave only
     open rows. Every accepted legacy command, route, hotkey, state writer,
     migration source, package entry, and workflow must map to Mesh Teams or an
     explicit surveyed retirement. Validate the combined etcd/overlay roster
     before publishing membership and populate the named system channel for
     every enrolled workstation.
  2. Extend the landed shared pin/private-save baseline through the real Mesh
     Teams runtime adapter and release acceptance; no pending control may
     remain at release.
  3. Add basic channel task/action-item contracts, projections, ownership,
     completion state, offline convergence, and Posts/Details presentation.
     Bound the Notification Stream with severity-aware coalescing, virtualization,
     and a visible pause/resume control so notification volume cannot hold the
     interface down.
  4. Complete the real two-way Discord bridge: sealed configuration, channel
     mapping, inbound/outbound provenance, loop prevention, replay/idempotence,
     bounded attachments, health/degraded state, and worker supervision. Keep
     Discord disabled and visibly unconfigured until its provider is configured.
  5. Bind real microphone/camera/screen providers and finish direct WebRTC P2P,
     elected LiveKit SFU fallback/failover, SIP ingress/egress, advancing media,
     device switching, call controls, screen share, and remote-control
     escalation. Remote control requires a trusted-session approval. Recording
     and transcription remain absent.
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
  10. Add an idempotent, failure-safe importer for legacy collaboration,
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
  3. Teams/channels, including each enrolled workstation's named system
     channel, Activity, Posts/Files/Calls, Details, rich composition, local
     find, threads, pins, saved messages, tasks, alerts, and contextual
     clipboard/transfer actions work with durable state and honest failures.
     Notification floods remain bounded and cannot monopolize rendering.
  4. Documents provide the accepted full IDE, live coauthoring, anchored
     review, safe external-write merge, version history, and non-destructive Git
     behavior.
  5. Files preserve stable identity and distinguish reference removal from
     permanent deletion; transfers resume, verify hashes, backfill members, and
     report through the shared progress projection.
  6. Direct and SFU-relayed calls carry advancing audio/video/screen frames;
     SIP ingress/egress, device controls, failover, and remote-control
     escalation work; no recording/transcription artifact exists.
  7. Discord is genuinely two-way with provenance and loop prevention only
     after sealed provider configuration; otherwise it is visibly disabled.
     DigitalOcean suggestions use consented bounded context, never auto-apply,
     and fail without impairing local collaboration.
  8. Migration is repeatable and failure-safe, the parity ledger has no open
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
  unsupported status. Mesh Teams owns persisted team clipboard history/actions;
  this epic owns transport and seat/VDI behavior. Browser participates only as
  the `browser-vm` VDI guest.
- Current state: Canonical `event/clipboard/clip` and action verbs exist. DRM
  Copy/Cut/Paste, bounded `CopyText`, shell materialization, VNC state,
  RDP/SPICE capability reporting, KDC/mobile authorization, bounds,
  deduplication, and attribution have focused coverage. Local publishing is
  session-scoped opt-in in Mesh Teams Settings; daemon history has a durable
  cursor and retry-safe acknowledgement. Signed VDI events authorize the exact
  target seat and leave failed writes retryable. KDC has 31 focused tests; VNC
  has a verified ClientCutText/ServerCutText round trip; the shell adapter has
  58 VDI tests. The daemon bridge validates serving peer and target seat with
  source attribution, deduplication, and retryability (27 bridge + 43 sync
  tests). RDP CLIPRDR/SPICE vdagent remain typed unsupported and gated.
- Remaining work:

  1. Wire the production OS/guest clipboard adapter for the signed target-seat
     handoff, preserving bounded retry and explicit unavailable-state reporting;
     do not reintroduce Wayland polling.
  2. Connect the completed KDC/mobile active-seat materialization queue to the
     production OS/guest adapter without losing remote history or attribution.
  3. Route Mesh Teams clipboard UI/actions to the same lane. Persist team
     history/actions for the session only, support per-user opt-out, and keep
     remote history visible when local publishing is disabled. Route Browser
     copy and paste solely through its VDI protocol after WL-ARCH-008 removes
     host Browser mediation.
- Scope: UTF-8 text clipboard up to the existing 1 MiB guest transport cap,
  direct DRM seat integration, Mesh Teams-owned session history/actions,
  KDC/mobile inbound materialization, and VDI host/guest channels. Arbitrary
  MIME, images, secret classification/filtering, and direct guest access to host
  memory are out of scope; Files/Transfers owns larger content.
- Relevant files/components: direct DRM handling in `mde-egui` and the shell;
  daemon clipboard sync/IPC/bridge workers; Mesh Teams clipboard presentation;
  `mde-vdi-rdp`, `mde-vdi-vnc`, and `mde-vdi-spice`.
- Dependencies: Preserve the existing event/action wire bodies, VDI
  authorization, echo guards, and 1 MiB cap. Coordinate Mesh Teams consumption
  with WL-FUNC-011 and Browser VM transport with WL-ARCH-008.
- Acceptance criteria:

  1. Copy/cut/paste works among local egui Editor, Terminal, and other text
     surfaces on the direct DRM seat without Wayland tools.
  2. Every accepted producer emits the canonical body, Mesh Teams history
     consumes that lane for the session, and authorized rows materialize on the
     target seat without loops. Local publish defaults off for new users and
     opt-out never hides remote history.
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
  gateway, never a node role. Each MG90 has an independent, versioned snapshot
  with ESN/alias identity, `management_node_id`, source/provenance, approval and
  sharing state, and complete radio/GNSS/vehicle domains. Multiple workstation
  managers may publish and issue authorized idempotent actions; remote nodes
  render the freshest valid snapshot read-only, with stale/resync/cache state.
  A persistent accessible health rail and gateway console expose honest
  per-radio state/freshness. Real route, maneuver, map, trip, and vehicle data
  replace placeholders. Carbon requirements retire while egui, shared `Style`,
  Construct, Car, and direct DRM remain governed.
- Current state: `mackesd` still assumes one managed MG90/direct Ethernet; the live unit reports LTE/WAN and power but no GNSS fix, and OBD parsing is absent.
  Raster MBTiles/FTS5 serves one region; Maps owns basemap/search, while Valhalla, route/lane/speed helpers, bearing, and admin actions remain unwired.
  Typed v2 identity/radio/freshness/provenance dual-publishes beside v1; Maps/Car project bounded radio and mirror states, with 13 consumer tests passing.
  The MG90/manager roster has independent poll/heartbeat plans, latest-wins/no-source behavior, and 44 vehicle tests pass. Cached observations heartbeat every
  two seconds without resetting v2 age. Maps HUD/Airspace expose honest unavailable/stale states; route preview has 7 focused tests at `05ba6870`.
  Navigation start rejects malformed or stale selections without mutation; the fresh farm full-crate Maps suite passes 264 tests with no failures. Live adapters
  and route/render cutover remain.
- Remaining work:
  1. Consume WL-UX-009's platform-wide Carbon-requirement retirement and update
     the Maps/Car-specific authority while retaining egui, shared `Style`,
     Construct/Car, HIG principles, and Car Dark/Light modes. Inspect governance
     history first, leave `AGENTS.md` untouched, and do not create a repo-root
     `CLAUDE.md`.
  2. Extend the landed versioned `VehicleState` v2 baseline to multiple MG90s,
     multiple managers, and the full rolling-upgrade removal plan.
  3. Extend the landed bounded `RadioId`/health inventory with live SKU
     discovery, multiple-manager routing, and consumer-side stale/resync state.
  4. Extend the landed typed radio metrics with proven device discovery and
     live adapter coverage; never infer absent hardware or zero-fill telemetry.
  5. Refactor the vehicle worker into independent schedulers for multiple MG90s
     and multiple managers: fast status, slow enrichers, direct-Ethernet-first
     transport, authorized mesh fallback, and a publisher that emits on change
     plus a heartbeat at most every 2 seconds. Deduplicate to the freshest
     complete snapshot. Slow probes may time out without delaying heartbeats or
     erasing fresher fields; consumers mark domains stale after three intervals.
     Lighthouses relay transparently without storing snapshots; relay migration
     pauses for full resync and renders last values with a resyncing state.
  6. Complete proven MG90 adapters for identity, radio state, GNSS, power,
     GPIO, WAN policy, VPN, and device temperature. Decode OBD/HDOBD only from
     captured, sanitized, versioned fixtures that prove the payload schema and
     units. Discovery requires explicit approval; report `Unsupported`,
     `NotInstalled`, or a specific probe failure instead of zero-filled
     telemetry, and never place credentials in snapshots, logs, or cache.
  7. Add a persistent Radio Health Rail to Free Drive and Active Route. Keep
     stable positions for the six native interfaces so every radio is
     accounted for. Use color plus shape/text: green check active, blue ring
     standby, amber triangle acquiring/degraded, red crossed link fault, gray
     pause disabled, gray slash not installed, and clock outline stale/unknown.
     Keep signal strength and active-uplink selection separate. Car and
     Construct expose metrics, age, handoffs, failures, diagnostics, management
     node, source, relay, stale, and resync state without a motion-based policy.
  8. Replace MG90 administration with Overview, Radios & GNSS, Vehicle I/O, and
     Maintenance, all driven by v2. Any active approved workstation manager may
     issue an idempotent allowlisted typed `action/vehicle/*` request. Order
     concurrent actions by mesh arrival time with last-accepted-action-wins;
     queue during resync, expire failed queues, reject duplicate pending writes,
     discard queues during takeover, and keep typed reply, timeout, audit, and
     revocation behavior visible. Delete unsupported controls.
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
  expose credentials, build a general fleet-management product, or restyle
  unrelated Construct surfaces. Typed authorized actions are not blocked by a
  motion policy; stale, unavailable, and unapproved actions remain disabled.
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
     covering maneuver/ETA content. Car and Construct expose the same bounded
     matrix, including manager/source/stale/resync state, without a motion-based
     policy.
  6. MG90 Overview, Radios & GNSS, Vehicle I/O, and Maintenance consume the
     same snapshot. Multiple approved workstation managers, takeover,
     revocation, idempotency, mesh-arrival ordering, resync queue expiry, and
     read-only stale-cache behavior are covered. Every enabled control produces
     a real typed, authorized, audited reply; no UI-only toggle, dead button, or
     unbounded command input remains.
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
- Verification method: Add bounded contract/property tests for vehicle v2,
  radio inventory/states, parser fixtures, cadence, time skew, and v1 rollout;
  deterministic worker tests with delayed/failed probes; route/maneuver,
  map-render, region-integrity, trip, Airspace, dead-control, and
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

### WL-FUNC-018 - Seamless Flatpak Front Door backed by App VMs

- Status: Remaining
- Priority: P1
- Complexity: Epic
- Problem: Construct has no supported way to discover or launch Wayland
  Flatpak applications. The host deliberately owns the DRM seat directly,
  ships no compositor, and does not run native host applications, so adding a
  normal Flatpak launcher would silently violate the thin-client boundary.
  Flatpak applications also need a real Wayland session, portal services,
  runtimes, GPU/audio policy, and durable application identity; none of those
  contracts currently connect the Front Door catalog to App VM provisioning.
- Required outcome: A user searches Front Door, selects a Flatpak app, and
  sees it launch as a Construct application without managing or seeing the
  underlying VM. Construct owns catalog, favorites, permissions explanation,
  launch state, stop/resume, reconnect, and honest failure presentation.
  `mackesd` places or resumes a Wayland-enabled App VM through the existing
  Workloads/session-broker/VDI planes; the guest owns Flatpak, its compositor,
  portals, app files, and app execution. No host compositor, host Flatpak
  sandbox, arbitrary host D-Bus access, or native-app fallback is introduced.
- Current state: Front Door validates signed catalog rows and honest unavailable,
  stale, unsigned, and non-launchable states; typed App VM declarations and
  `OpenApp` cross Workloads/session-broker/VDI. The bounded Sway profile,
  lifecycle reasons, admission-gated favorites, immutable runtime verifier,
  and no-public-remote policy are implemented. Session retries preserve
  catalog/capability/resume intent while rejecting retargets; 20 App VM and
  guest-runtime readiness tests plus 39 broker tests pass. Guest rows use
  distinct `Guest App`/`Guest Flatpak` accessibility language. On farm `.170`,
  the Fedora 44 RPM-backed image passed all static checks with profile
  `wayland-standard-v1`, image `6e16ff...6044c54`, and immutable base
  `sha256:4bd2...faa3b5`; full provision/guest/VDI convergence remains open.
- Remaining work:

  1. Complete the Front Door catalog experience around the validated projection:
     finish permission explanation and launch-state/reconnect presentation.
     Preserve unavailable, stale, unsigned, and not-installed states instead of
     presenting a launchable card for missing guest content.
  2. Finish integration of the typed App VM launch/session contract across the
     Workloads and `action/vdi/session` seams, including app identity, catalog
     revision, guest profile, placement constraints, session identity, requested
     capabilities, and resume/reconnect intent without raw command, mount,
     environment, or socket input.
  3. Complete `mackesd` reconciliation for App VM images and app declarations:
     the typed desired-state and dedicated image-selection/build lane now land;
     signed-image admission now distinguishes unavailable, unsigned, stale,
     and fresh matching evidence before declaration; still required are
     idempotent provision/resume now requires fresh matching guest-runtime evidence;
     full guest install/update execution through the admitted
     `curated` remote, and complete daemon-published readiness. The guest unit
     now reports bounded install/start/failure evidence and the broker consumes
     it; the guest image validates every identity before admission. The image
     now has a fixed compositor/app supervisor; full process convergence remains
     required. Repeated launches must converge
     on one app session, not duplicate VMs or processes.
  4. Finish the hardened App VM profile: the image definition now pins the
     supported Wayland compositor, portal frontend/backends, Flatpak, PipeWire,
     input, and deny-by-default profile, while the new static built-image
     verifier now checks those contents and requires immutable image provenance;
     remote signing and bounded writable app-data policy remain to be
     implemented and inspected in a built image.
     The image-owned launcher binds app process lifetime to the
     guest compositor and emits terminal failure evidence. Portal prompts and file access remain guest-scoped; the host
     does not proxy arbitrary portal requests or grant unrestricted host paths.
  5. Complete VDI app-mode presentation so the shell renders the application
     surface with Construct chrome, forwards keyboard/pointer/touch/text input,
     preserves focus, and handles resize, close, suspend, reconnect, and
     compositor/app crash separately. A full guest desktop may be a diagnostic
     recovery view only; it is not the normal Flatpak UX. The Workloads-side
     launch action and typed session handoff are now landed; Construct-owned
     app identity chrome, close, input routing, and bounded reconnect/failure
     presentation are covered by focused tests. Focusing the resulting
     broker-visible App VM rail entry now routes the typed session into the
     existing VDI console resolver once; live guest-backed rendering, resize,
     reconnect, and compositor/app crash proof remain required.
  7. Add explicit user-facing permission and lifecycle states: installing,
     waiting for placement, starting guest, starting app, connected, paused,
     reconnecting, unavailable, denied, stale catalog, and failed with retry
     guidance. Destructive actions (remove app/data, stop guest, reset guest)
     require typed confirmation and explain their scope. The shared lifecycle
     enum now enforces legal forward/retry edges and idempotent repeated states
     in the daemon broker. The serving daemon also emits a signed
     `starting_guest` handoff while the session is still waiting, without
     rewinding later evidence; guest runtime probes, publication of later
     transitions, and shell consumption of connected/reconnecting states remain
     required.
  8. Integrate app data, updates, favorites, recents, and removal with the
     existing replicated-state and retention rules. Keep app data in the guest
     profile, distinguish app reset from VM reset, and make purge behavior
     explicit; never replicate secrets or silently expose guest home data to
     the Construct host.
  9. Add admission and security policy: default-deny network/host exposure,
     capability and resource-budget checks, signed image/catalog provenance,
     audit records for install/launch/permission/reset, and blast-radius
     diagnostics. Reuse WL-CRIT-006 Workloads admission and provenance rather
     than creating a second authorization plane.
  10. Provide a staged migration and rollout: catalog-only records first,
      one curated Flatpak in one App VM profile second, then updates,
      favorites, roaming, remote placement, and broader catalog coverage.
      Existing Construct surfaces and the Browser VM route remain unchanged;
      an unavailable provider produces an honest disabled state.
  11. Add a curated LibreOffice App VM profile using the same App VM/VDI
      contract. Support Writer, Calc, and Impress, typed application identity,
      document associations, and Workspace shortcut-group presentation. The
      normal UX must not expose the guest desktop.
  12. Add Construct-owned printing for the LibreOffice profile: printer
      discovery/health, local and mesh printer selection, page-faithful preview,
      paper/tray/orientation/margins, duplex/binding, color, copies/collation,
      page ranges, scaling, N-up/booklet, PDF output, presets, progress,
      cancel/retry, explicit offline/error states, and touch-sized tablet
      controls. Print and preview must share pagination/rendering semantics.
- Scope: Flatpak catalog/discovery, Front Door UX, App VM lifecycle and
  placement, guest Wayland/portal/runtime image, VDI app-mode display/input,
  permissions, app data, updates, roaming, resource admission, audit, recovery,
  the curated LibreOffice App VM, and its printing workflow. Native host
  Wayland mode, a host compositor, arbitrary user-built Flatpak manifests,
  unrestricted host filesystem/D-Bus access, and a general host desktop/window
  manager are out of scope.
- Relevant files/components:
  `crates/desktop/mde-shell-egui/src/front_door.rs`,
  `crates/desktop/mde-shell-egui/src/front_door_peer_apps.rs`,
  `crates/desktop/mde-shell-egui/src/iac/views/app_vm.rs`,
  `crates/desktop/mde-shell-egui/src/{discovery,session_rail,vdi}/`,
  `crates/mesh/mackesd/src/workers/{session_broker,vm_lifecycle,desktop_sources}.rs`,
  `crates/mesh/mackes-mesh-types/src/cloud.rs`, and App VM/bootc packaging.
- Dependencies: Coordinate guest ownership and app-mode behavior with
  WL-ARCH-008; use WL-FUNC-016 for bounded text clipboard across VDI; use
  WL-CRIT-006 for resource admission, provenance, retention, and production
  evidence. Reuse the existing Workloads Bus contracts and session identity;
  do not create a Flatpak-specific control plane or a second active worklist.
- Acceptance criteria:

  1. A signed curated Flatpak catalog appears in Front Door with stable app
     IDs, icons, metadata, source/revision, install state, and accessible
     search results. Malformed, duplicate, stale, and unsigned records are
     non-launchable and explain why.
  2. Selecting an installed app creates or resumes exactly one admitted App VM
     session through typed Workloads plus `action/vdi/session`; no shell code
     executes a catalog-provided command or opens an arbitrary host path.
  3. The guest boots a supported Wayland session and portal stack; the selected
     Flatpak renders through VDI app-mode, receives focused input, and supports
     close, pause/resume, reconnect, resize, and app-crash recovery states.
  4. The normal user experience shows an app surface with Construct chrome,
     not a host window and not an unmanaged guest desktop. No host compositor
     or native host Flatpak path is required.
  5. Guest portal requests are bounded and auditable. File chooser, OpenURI,
     settings, notifications, audio, clipboard, and screen/input capabilities
     either work through the declared guest policy or show an honest reason
     they are unavailable; no unrestricted host access is granted.
  6. Install, update, launch, stop, resume, remove, reset, and purge behavior
     is idempotent, authorized, auditable, and tested across local placement,
     remote placement, node loss, VM restart, app crash, and reconnect.
  7. App data and credentials remain guest-scoped with explicit retention and
     purge semantics; favorites/recents replicate only approved metadata.
  8. Resource admission rejects unsafe CPU, memory, storage, GPU, network, or
     capability requests before provisioning and exposes the rejection reason.
  9. A curated end-to-end proof passes on the build farm plus a reachable
     VDI/guest seat; missing live Wayland, portal, GPU, audio, or hardware
     evidence is recorded as unavailable rather than implied by unit tests.
- Verification method: Add pure catalog, identity, ranking, permission,
  lifecycle, migration, and malformed-input tests; Bus contract tests for
  typed App VM launch/session records; deterministic `mackesd` reconciliation
  and idempotency tests; VDI frame/input/reconnect tests; guest-image and
  portal-policy inspection; package/signature/provenance/resource-admission
  gates. Run focused shell, mesh-types, `mackesd`, VDI, and packaging tests on
  independent farm slots, with the longest App VM/guest build on BigBoy, plus
  workspace clippy/fmt and worklist/doc/architecture/secret gates. Finish with
  a live curated-app launch, portal file-selection, audio/input, app-crash,
  suspend/resume, reconnect, and corrected-forward recovery capture when the
  seat and guest provider are available.
- Origin or merged source IDs: 2026-07-31 operator brainstorm: “Flatpak Front
  Door backed by App VMs” — combines the Flatpak catalog/control-plane path
  with the existing App VM/VDI runtime path. Evidence sources are the current
  Construct no-host-app/no-compositor governance, App VM view, Front Door
  provider, Workloads contracts, session broker, VDI transport, and Flatpak /
  Wayland / xdg-desktop-portal upstream contracts.

### WL-CRIT-006 - Production evidence, six-node acceptance, and corrected-forward recovery

- Status: Remaining
- Priority: P0
- Complexity: Epic
- Problem: Construct has strong static engineering evidence but no single
  authoritative production-readiness contract. GitHub CI, farm execution,
  package compatibility, live hardware, multi-node recovery, and operator
  evidence are described in separate places. The platform therefore risks a
  static-green release being mistaken for a production-ready release.
- Required outcome: GitHub required checks are authoritative, the farm is the
  heavy self-hosted backend, and every release emits a signed evidence bundle.
  Production promotion requires a verified three-lighthouse/three-workstation
  topology, capability-based readiness, corrected-forward recovery, automatic
  lighthouse failover, resource admission, minimal-retention policy, and a
  permanent six-node integration/chaos gate.
- Current state: The repository has static build/test/clippy/fmt/coverage,
  policy, SBOM, package, health, audit, encrypted-backup, farm, and live-proof
  foundations; preview remains distinct from promotion and needs three
  lighthouses plus the six-node substrate. `release-evidence.sh` emits
  schema-v2 source/artifact/SBOM/gate bindings; `sign-release.sh --evidence`
  publishes signed provenance and checksums. `ci-gate.sh verify` rejects
  non-green, unobserved, stale, or unbound farm results and requires matching
  `github-required=pass`. A fail-closed six-node verifier requires exactly three
  lighthouses, three workstations, fresh artifact-bound recovery evidence, and
  `--require-live`; no real bundle exists. Farm `.90` slot 118 passes both
  `ci-gate.sh --self-test` and `release-evidence.sh --self-test`. Farm-to-GitHub
  publication, six-node/live gates, and operator-key publication remain open.
- Remaining work:

  1. Make GitHub required checks the promotion authority and connect the farm as
     the heavy self-hosted execution backend. The release envelope now consumes
     `ci-gate.sh verify` as a fail-closed farm-result check and requires the
     matching `github-required` result; complete the operational publication
     of that GitHub/farm association. Until that path is operational,
     production promotion remains blocked.
  2. Define and verify the fixed six-node topology: three lighthouses and three
     workstations, with explicit degraded and recovery states.
  3. Implement guided scope-bound enrollment, capability quarantine, typed
     readiness, per-domain merge/provenance contracts, and automatic lighthouse
     failover.
  4. Implement corrected-forward re-enrollment and preserve current encrypted
     backups until replicated-live-state recovery has passed destructive drills.
  5. Add typed workload resource admission, profile activation, retention rules,
     redaction/purge behavior, and incident evidence bundles.
  6. Maintain a permanent six-node testbed with nightly failure injection,
     lighthouse loss, re-enrollment, VDI reconnect, workload pressure, and
     state-convergence scenarios.
  7. Reconcile normative guidance and historical banners across root, help,
     operations, design, and tracked agent-skill docs; keep the canonical
     worklist pointer and current gate list synchronized with dated evidence.
- Scope: Cross-cutting release authority, topology, enrollment/recovery,
  readiness, failover, state convergence, workload admission, retention,
  provenance, incident evidence, and multi-node acceptance. Feature behavior
  remains owned by WL-ARCH-008, WL-FUNC-011, WL-FUNC-016, WL-FUNC-017,
  WL-UX-009, WL-UX-011, and WL-UX-012. Current encrypted backups remain in
  service during the transition; this epic does not authorize disabling them.
- Relevant files/components: `AI_GOVERNANCE.md`; `.github/workflows/ci.yml`;
  `install-helpers/ci-gate.sh`; `install-helpers/xcp-build.sh`;
  `docs/farm.md`; `docs/BUILD-ENVIRONMENT.md`; packaging/release helpers;
  enrollment, health, audit, workload, and recovery contracts.
- Dependencies: Coordinate Browser, Teams, clipboard, Maps, This Node, theme,
  and taskbar acceptance with their existing epics. Use the existing Bus,
  healthz, audit, CA, Workloads, and farm contracts rather than creating a
  second authority.
- Acceptance criteria:

  1. A GitHub required-check result and signed release-evidence artifact exist
     for every candidate release; farm execution is traceable by job and slot.
  2. A release cannot receive a production verdict when a required static,
     compatibility, topology, recovery, live, or hardware gate is missing,
     stale, unavailable, or manually asserted.
  3. Three lighthouses and three workstations pass join, steady-state, loss,
     failover, re-enrollment, and corrected-forward recovery drills.
  4. Node and capability readiness distinguish healthy, degraded, stale,
     unavailable, blocked, and recovering without hiding unaffected services.
  5. Workload placement explains resource/capability decisions and preserves
     control-plane headroom; retention and purge rules are enforced.
  6. Replicated domains expose deterministic merge rules, source identity,
     revision provenance, conflict behavior, and bounded recovery.
  7. Incident bundles correlate health, audit, worker, transport, workload,
     certificate, and operator events without including secrets.
  8. Existing encrypted backups remain verified until peer-replication recovery
     is proven; the transition has an explicit evidence record.
  9. The six-node testbed runs repeatable chaos and recovery scenarios and
     publishes artifacts suitable for production promotion review.
  10. Normative guidance names only current commands, roles, boundaries, and
      gates; retired architecture docs are bannered; documentation and
      worklist lints pass.
- Verification method: Run worklist, governance, documentation, secret,
  package, compatibility, and provenance lints; run GitHub required checks and
  farm gates with BigBoy carrying the long pole; run parallel six-node tests for
  join, failover, recovery, state convergence, resource pressure, retention,
  and VDI reconnect; perform signed-artifact and Fedora transaction checks;
  preserve live evidence bundles and explicit unavailable-hardware notes.
- Origin or merged source IDs: Fit-for-purpose audit 2026-07-30 (`AUD-01`
  through `AUD-07`, `AUD-09`, and `AUD-17` through `AUD-25`); operator-selected
  production gate, topology, recovery, trust, retention, and testbed decisions.

## User Interface And Experience

### WL-UX-009 - Complete the shared Quazar workspace design language

- Status: Remaining
- Priority: P1
- Complexity: Epic
- Problem: Shared Quazar fonts, light/dark palettes, style primitives, and some
  surface migrations exist, but user-facing egui workspaces still drift in app
  frames, navigation, state presentation, sheets/popovers, tooltips, motion,
  icons, tables, and internal Editor/Terminal chrome. Older scope also assumes
  a host Browser chrome exception that conflicts with WL-ARCH-008.
- Required outcome: Every Construct-owned egui surface reads as one dense,
  HIG-principled Quazar platform in Dark and Light: common app frame,
  navigation, state components, sheets/popovers, typography, icons, motion, and
  data presentation. Car supports governed Dark and Light modes; focused VDI
  preserves full-screen pixels; guest applications, including
  Chromium in `browser-vm`, remain outside Construct styling. Carbon is not a
  theme or icon requirement; all retained or replacement assets use the shared
  registry, have clear licensing, and form one coherent visual language. The
  Terminal-pattern unified top bar is shared across workspaces, with approved
  exemptions recorded individually; the side tab bar is 25% thinner, two-row,
  and zebra-striped for differentiation.
- Current state: Shared Quazar typography, palettes, chrome primitives, state
  panels, menubars, dense lists, and most Construct-owned route migrations are
  landed; farm suites cover shell, Editor, Teams, Maps, Files, Bookmarks, Music,
  Device, Storage, and This Node. Workbench and System now consume the shared
  `AppFrame` primitive, covered by focused shell tests. The 2026-08-01 direct-DRM
  captures and deployment details are recorded in
  [`WL-UX-009-DRM-EVIDENCE-2026-07-31.md`](WL-UX-009-DRM-EVIDENCE-2026-07-31.md).
  The verified shell payload is active on `.138` and Dell with `NRestarts=0`;
  `.138` has Dark desktop/narrow and Light/Largest Editor proof; Dell has clean
  Dark Editor/System/Bookmarks/Storage/Phones frames, Dell narrow
  Bookmarks/Storage/Phones proof, and Light/Largest System/Bookmarks/Storage/
  Phones proof. The latest Dell shell payload is `34ebaac4` and its compact
  profile control is suppressed at narrow widths so Control Center owns that
  toggle without covering workspace fields.
  The full RPM dependency transaction remains
  open because its Fedora runtime dependencies do not match the seats; `.15`,
  the remaining routes/states, full matrix, and production readiness remain open.
- Remaining work:

  1. Finish the shared app-frame and Terminal-pattern unified top bar, including
     the per-workspace exemption review. Complete loading/empty/stale/offline/
     error/destructive states, sheets, popovers, tooltips, table/list density,
     icon registry/cache, and centralized expressive motion primitives;
     reduced-motion substitutions are optional compatibility work, not a core
     feature.
  2. Migrate shell chrome and every launchable workspace to those primitives,
     matching Terminal's top-of-space pattern and preserving only explicitly
     approved Maps content-color and focused-VDI pixel exceptions.
  3. Migrate Editor and Terminal internal tabs, toolbars, palettes, sidebars,
     popovers, and status rows without changing editor/terminal behavior. Direct
     Editor entry now collapses both optional sidebars, covered by the focused
     farm test `direct_entry_collapses_all_optional_sidebars`; make
     the side tab bar 25% thinner, two-row, and zebra-striped without clipping.
  4. Apply the shared language to Mesh Teams, This Node, and Construct-owned
     `browser-vm` connection/unavailable/diagnostic states. Do not style the
     guest Chromium viewport or reintroduce host Browser chrome.
  5. Complete Dark/Light, desktop/narrow/large-text, no-overlap, icon
     licensing/raster, expressive motion, and representative live DRM proof.
- Scope: Current design authority, shared `mde-egui` and brand/icon primitives,
  shell-owned chrome, launchable egui workspace frames, and Editor/Terminal
  internal chrome. Behavior contracts, security/auth, full AccessKit rollout,
  guest application UI, and general native-app hosting are out of scope.
- Relevant files/components: `crates/shared/mde-egui/`,
  `crates/shared/mde-theme/`, shell chrome, and all crates registered in the
  embedded surface inventory.
- Dependencies: Coordinate adoption with WL-ARCH-008, WL-FUNC-011, and
  WL-UX-011. WL-UX-011 owns This Node behavior, typed actions, health-score
  integration, auditability, and vendor-pack contracts; WL-UX-009 owns only
  shared visual adoption and render proof. WL-UX-012 owns taskbar chrome,
  WL-FUNC-017 owns MG90 telemetry semantics, and WL-CRIT-006 owns integrated
  production evidence. Shared visual work may proceed independently and must
  not block functional contracts or create a second product epic.
- Acceptance criteria:

  1. Current authority contains no Carbon theme/icon requirement. Quazar
     Dark/Light pass palette, contrast, font, shape, licensed-icon, and
     deterministic screenshot tests.
  2. Construct-owned surfaces use shared frames/navigation/state/dialog/tooltip
     primitives unless an explicit governed exception is documented.
  3. Editor and Terminal internal chrome is migrated; dense tables/lists are the
     default operational idiom; expressive motion is centralized and tested.
  4. Maps content exceptions are marked, focused VDI retains full-screen pixels,
     Car Dark/Light behavior is tested, and guest Chromium receives no
     Construct chrome.
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
  Mesh & System. Controls operate through typed node-local contracts plus
  admin-authorized trusted-session remote actions, preserve mesh reachability,
  expose honest unavailable/degraded states, and bound privileged/OEM writes
  with confirmation, audit, safety limits, and recovery. The operator-facing
  experience follows a Windows 10-style Device Manager model: a unified
  expandable tree, a health-dashboard landing view, and focused detail views
  for technical operators/admins. The existing top-rail health score remains
  the single health authority; selecting it opens this tree with unhealthy
  items highlighted. Monitoring and configuration have equal priority. The
  landing view leads with critical alerts and affected components; a healthy
  node presents a calm all-systems-operational summary. Health severity uses
  color, icon, label, and text. The tree includes Hardware, Services, Storage,
  Network, Users, and System, with persistent badges tied to the existing
  score. Selecting an item opens a dedicated full-page detail view. Actions
  live in a dedicated Actions tab; risky operations show expected impact and
  require confirmation; restart-required changes warn before applying.
  Recent events use compact expandable summaries. Storage uses capacity bars
  with detailed tables in the detail view; Network uses an interface summary
  with topology on demand; Performance uses live charts. Refresh is on demand
  with stale indicators. Unavailable data shows a recovery error. Auditability
  includes per-item operator, timestamp, and outcome history. Permission
  management is role-first, with users grouped under roles. Long-term design
  must remain extensible for future hardware, services, node types, and
  installed vendor packs, which expose clearly separated, discoverable,
  versioned vendor-specific settings in the owning detail view.
  The center also covers the complete OS-management layer expected by mature
  workstation settings surfaces: updates and lifecycle, recovery/reset,
  security/privacy, accounts/sign-in, applications and services, backup/restore,
  diagnostics/logs, printers and peripherals, accessibility, time/language/
  region, and virtualization/remote access. These domains remain under the
  same node authority and must not recreate legacy settings routes.
- Current state: Existing System, Storage, Device Manager, About, Control
  Center, status, BlueZ, display, power, firmware, and input code provide
  partial foundations; controls remain read-only in places, routes are
  duplicated, and no unified typed action boundary covers connectivity, audio,
  input, performance, docks, or OEM capabilities. This Node now has a governed
  eight-section catalog, persistent section/search navigation with operator
  hardware aliases, legacy normalization, and unavailable provider states.
  Its mesh-status model projects bounded typed capability/action rows with
  fail-closed mutations plus truthful interface/CIDR/route/lighthouse/DNS
  connectivity facts; focused This Node tests pass, including narrow, large-text reflow and disabled-action accessibility coverage (18 tests).
  Mesh & System now adds a truthful unavailable/unknown/offline/degraded/connected
  summary with AccessKit status and 3 focused tests.
- Remaining work:

  1. Finish the durable This Node sidebar and search index across every
     provider-backed page; keep Control Center transient and status chrome
     glanceable.
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
     device enablement, and Thunderbolt authorization. Permit only typed,
     admin-authorized actions from trusted sessions when the target is remote.
     Support standard kernel interfaces plus capability-detected Microsoft
     Surface, Dell, Lenovo, HP, and ASUS adapters.
  8. Bound manufacturer writes with arming, confirmation, audit, thermal limits,
     watchdog recovery, and automatic safe-profile fallback. Never expose
     arbitrary sysfs paths, raw MSR/SMI, `/dev/mem`, untyped remote mutation,
     or shell command composition from the UI.
  9. Reconcile Control Center and status chrome with the same typed state:
     connectivity, Bluetooth, sound, LCD/keyboard brightness, power, underlay
     versus mesh health, numeric battery, and microphone/camera indicators.
  10. Finish desktop/narrow/large-text, unsupported-hardware, stale/provider
      failure, destructive confirmation, and physical-device proof using
      WL-UX-009 components.
  11. Replace the current section catalog presentation with the surveyed
      Device Manager-style hierarchy and health-dashboard landing view. Wire
      the existing top-rail score to highlighted unhealthy tree nodes without
      introducing a second score, and provide dedicated full-page details with
      Actions, Events, Performance, Audit, and vendor-pack sections.
  12. Implement the surveyed data states and operator workflows: on-demand
      refresh/stale indicators; prominent unavailable recovery; compact
      expandable events; capacity bars plus detailed storage tables; interface
      summary plus on-demand topology; live resource charts; role-first users;
      per-item audit records; restart warnings; and confirmation dialogs that
      show expected impact.
  13. Define and implement the vendor-pack extension contract. Discover
      installed packs, version and capability status, expose vendor-specific
      settings beside the owning hardware/service, keep standard and vendor
      controls distinct, and fail visibly when a pack is missing, outdated, or
      unavailable. Vendor writes must use the same typed authorization, safety,
      audit, and recovery boundaries as platform controls.
  14. Add the OS-management layer: update/lifecycle status, recovery/reset,
      security/privacy and encryption posture, accounts/sign-in, applications
      and services, backup/restore, diagnostics/logs, printers/peripherals,
      accessibility, time/language/region, and virtualization/remote access.
      Each provider must expose capability, freshness, safe actions, failure
      recovery, and audit state through the unified authority.
  15. Complete the final survey decisions: urgent critical-health alerts;
      configurable persistent notifications with acknowledgement, snooze, and
      escalation; guided recovery actions with expected-impact confirmation;
      role-gated health policy and overrides; explicit live/on-demand refresh
      and stale intervals; and redacted, audit-backed history export.
- Scope: Local workstation hardware state, typed node-local mutation,
  admin-authorized trusted-session remote mutation, diagnostics, settings
  consolidation, capability discovery, quick controls, and safe OEM adapters.
  Arbitrary path writes, lock/PAM replacement, raw privileged interfaces, and
  host application ecosystems are out of scope.
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
     real bounded node-local actions. Typed remote actions require admin
     authorization and a trusted session and never expose arbitrary paths.
  4. Unsupported capabilities stay visible but disabled with a reason; provider
     loss/stale data never appears successful.
  5. Privileged actions are allowlisted, armed, audited, thermally constrained,
     watchdog-protected, and recover automatically to a safe profile.
  6. Control Center and status chrome consume the same authority without
     creating a second durable settings hierarchy.
  7. Desktop, narrow, large-text, degraded, and confirmation states have no
     overlap/clipping; available physical controls pass direct-seat proof and
     unavailable hardware is recorded honestly.
  8. The surveyed Device Manager hierarchy, health-dashboard landing view,
     existing top-rail health-score drill-down, dedicated full-page details,
     Actions/Events/Performance/Audit sections, and role-first Users view are
     implemented without creating a second settings or health authority.
  9. On-demand refresh, stale indicators, recovery errors, expandable event
     summaries, capacity bars plus detail tables, interface summary plus
     topology, live resource charts, restart warnings, and expected-impact
     confirmations are tested at desktop, narrow, and large-text sizes.
  10. Vendor packs are discoverable, versioned, capability-aware, and visibly
      separated from standard controls; missing, outdated, or unavailable
      packs fail honestly, and vendor actions use typed authorization, safety,
      audit, and recovery boundaries.
  11. Updates, recovery, security/privacy, accounts, applications/services,
      backup/restore, diagnostics, peripherals, accessibility, time/language,
      and virtualization/remote access are represented in the unified tree and
      tested for capability discovery, unavailable states, safe actions, audit,
      and responsive rendering.
  12. Critical alerts, notification acknowledgement/snooze/escalation,
      guided recovery, role-gated health policy, explicit refresh/stale
      intervals, and redacted evidence export are tested and audited.
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
  focuses Front Door search. New profiles choose their initial pins during
  first boot; migrated profiles preserve valid existing pins and never silently
  restore a default list. Exactly one underline identifies the focused
  workspace; no running/open indicator exists. Home is an icon-free wallpaper
  and the App Grid is deleted.
- Current state: `nav_bar.rs` owns persisted `Floating`/`Docked` placement,
  full-width 48px Bottom geometry, fixed 40px targets, centered pins, overflow bounds,
  pin persistence, focus underlining, first-boot search, and tested
  slide/melt/Whimsy motion. Bottom reserves an animated clock/icon tray; Start uses
  Front Door search and the side retains its detailed top strip.
  `springboard.rs` is gesture-only: the retired App Grid painter,
  selection/open path, zoom ghost, and grid tests are deleted; 37 nav-bar plus
  14 shell/front-door tests pass. The taskbar consumes the searchable catalog
  directly, with Fleet & Mesh/Workloads aliases and no retired group geometry;
  narrow first-boot controls now have 37/37 farm tests, while full first-boot
  selection remains open. A fresh unsigned Alpha from integrated tip `b77a82de`
  is staged for Dell proofing; evidence is in `docs/ops/promotion-pipeline.md`.
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
  5. Replace `DOCK_LAUNCHER_GROUPS` with a searchable pin catalog containing
     `FleetMesh`, `InfraCode`, `Desktop`, `Terminal`, `MapsLocation`,
     `Communications`, `Files`, `Music`, `Media`, and `Browser`. Present
     `InfraCode` as `Workloads` without renaming its internal enum or breaking
     persisted/deep-link compatibility. Present `FleetMesh` as `Fleet & Mesh`.
     Use a first-boot pin selector for new profiles rather than auto-pinning
     the catalog.
  6. Version `settings-nav-bar.json` with `schema_version`, existing serialized
     placement, and ordered `pinned_surfaces`. Preserve valid existing pins,
     discard unknown surfaces, bound the list to the searchable catalog, and
     send new profiles through first-boot selection. Never silently restore a
     default list after migration; user choices remain authoritative.
  7. Finish first-boot pin selection for new profiles and Fleet & Mesh/
     Workloads exposure without renaming the internal `Surface::InfraCode`
     identifier. Keep pin changes immediate and reject pinning Start, Back,
     Home, overflow, and placement controls.
  8. Delete Springboard tile plates, labels, grid layout, keyboard selection,
     tile activation, open-presence zoom ghosts, and their tests. Preserve the
     wallpaper Home, Home intent, and pull-down-to-search gesture in a reduced
     Home gesture layer.
  9. Pass a focused target to the taskbar: `Home`, `Surface(Surface)`, or
     `DesktopSource(id)`. Paint exactly one centered 18x3 accent underline 2px
     above the bottom. Normalize Workbench, Mesh Map, and Explorer aliases to
     Fleet & Mesh; match active VDI sources to their pinned desktop icon; mark
     Home on wallpaper. Search leaves the underlying marker unchanged. Never
     paint running/open/recent markers or expose taskbar Close actions.
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
  4. New profiles complete first-boot pin selection; migrated profiles preserve
     valid pins without silently restoring defaults. Fleet & Mesh and Workloads
     are available in the searchable catalog. Pin, unpin, restart persistence,
     malformed recovery, and placement preservation are tested; no reorder or
     drag path exists.
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
  preference/geometry/action/search/context/pin/focus/overflow/Home tests. Run
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
