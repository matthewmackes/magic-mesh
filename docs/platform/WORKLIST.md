# Platform Worklist

This is the only active platform worklist. Design notes, parity ledgers,
operational runbooks, review notes, and `docs/NEEDS-OPERATOR.md` are evidence
sources, not parallel trackers.

The complete pre-rework worklist, including all progress records and historical
reconciliation text, is preserved at
`docs/worklist-archive/2026-07-28-platform-worklist-pre-rework.md`. The exact
pre-lint-compaction state and implementation diaries removed on 2026-08-03 are
preserved in
`docs/worklist-archive/2026-08-03-platform-worklist-pre-lint-compaction.md`.

## Current Snapshot - 2026-08-03 Workloads and five-seat fleet integration

- **13 active epics:** 13 `Remaining`, 0 `Blocked`, 0 `Needs clarification`.
- **P0:** WL-ARCH-008 (standalone old Browser repo plus VM Browser cutover),
  WL-ARCH-009 (process-isolated mackesd and Advanced administration),
  WL-FUNC-011 (Mesh Teams completion and one-product cutover), WL-FUNC-016
  (native text clipboard across seat/mesh/VDI), WL-FUNC-017 (complete Maps,
  navigation, and MG90 radio health), WL-UX-011 (unified This Node hardware
  center), WL-CRIT-006 (production evidence, six-node acceptance, and
  corrected-forward recovery), WL-CRIT-007 (boot/sleep recovery and fleet
  peer return), and WL-FUNC-019 (universal resource/session discovery and
  client browser).
- **P1:** WL-UX-009 (shared Quazar theme and design-language completion),
  WL-UX-012 (full-width Construct taskbar and search-first Home), and
  WL-FUNC-018 (seamless Flatpak Front Door backed by App VMs), and WL-FUNC-020
  (governed Android applications backed by Cuttlefish Android VMs).
- **Delivery policy:** produce service-oriented releases in the queue below.
  Farm lanes may fan out implementation and verification for the current
  service, but a later service does not displace its release proof. An
  engineering preview may ship with named evidence gaps; production promotion
  still requires every selected live gate to pass.
- **Rollout policy:** prove the current service on its named release seat first,
  then deploy that same revision to Dell, Eagle, seat 15, T480, and Surface.
  Every seat update publishes the centered red `AI-GENERATED-ALERT` and waits
  five seconds before mutation. Require all three lighthouses and the complete
  eight-node substrate before production promotion. Failed nodes recover by
  re-enrollment and corrected-forward deployment, not rollback.
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

## Service Release Queue - 2026-08-03

This is an ordering view over the active epics below, not a second worklist.
The operator's service-oriented direction on 2026-08-03 supersedes the earlier
single integrated-preview sequencing in the Survey Decision Register. Only one
release milestone is current; independent farm lanes fan out inside that
milestone, then rejoin at its live exit evidence.

1. **R1 - Chromium Workspace (WL-ARCH-008) - Current.** On Dell, Browser
   selects the admitted `browser-vm` automatically; Chromium produces a live
   frame, keyboard/pointer input works, reconnect resumes the same workload,
   and audible media reaches the Dell sink. Record the immutable image identity,
   encrypted guest credential, five-second seat alert, and deployed revision.
2. **R2 - Remote Sessions and Resources (WL-FUNC-019) - Queued.** One resource
   browser discovers desktops, services, and external providers and launches
   each through a typed, health-proven adapter.
3. **R3 - Flatpak Applications (WL-FUNC-018) - Queued.** A governed starter
   catalog appears in Front Door and each selected Flatpak launches and
   reconnects through an App VM.
4. **R4 - Android Applications (WL-FUNC-020) - Queued.** Workloads lists the
   governed AOSP starter set and launches and reconnects each application
   through an Android VM.
5. **R5 - Native Clipboard (WL-FUNC-016) - Queued.** Bounded UTF-8 clipboard
   text crosses local seat, mesh, and released VDI paths with loop prevention
   and explicit unsupported states.
6. **R6 - Mesh Teams (WL-FUNC-011) - Queued.** Communications is the one
   collaboration product with live messages, files, tasks, clips, calls, and
   governed bridges.
7. **R7 - Maps and Vehicle (WL-FUNC-017) - Queued.** Offline maps, routing,
   location, and MG90 radio health work with freshness and safe-action proof.
8. **R8 - This Node and Administration (WL-UX-011, WL-ARCH-009) - Queued.**
   The unified hardware center and process-isolated advanced administration
   provide typed, audited actions and recovery.
9. **R9 - Construct Experience (WL-UX-009, WL-UX-012) - Queued.** Quazar
   Dark/Light, the full-width taskbar, search-first Home, and responsive
   seat/tablet interaction pass visual signoff.

WL-CRIT-006 and WL-CRIT-007 are cross-release proof obligations. They collect
production evidence and recovery results without becoming competing product
milestones.

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
7. Audio is a first-class production requirement across seats, App VMs,
   Browser VMs, VDI, Mesh Teams, and node-to-node streaming. PulseAudio
   compatibility must be present through PipeWire's Pulse server; ALSA,
   PipeWire, WirePlumber, UCM, codecs, permissions, services, device
   discovery, microphones, speakers, HDMI/Bluetooth, VM audio, and remote
   audio transport must all be installed, routed, observable, recoverable,
   and proven live. Audio may not be marked optional or unavailable for
   production promotion.

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

### This Node interface rethink survey - 2026-08-01

The operator selected a bold workspace redesign while retaining every existing
capability and action. WL-UX-011 is the owning epic; this record is normative
design direction, not a second workstream:

1. Use a technical Device Manager as the primary mental model.
2. Navigate through a hierarchy-first tree, with selected areas opening as
   focused full-page detail views.
3. Make the landing view inventory-first; keep health, alerts, and recovery
   states attached to the relevant inventory items and clearly visible.
4. Use a dense operator-console information layout with progressive disclosure.
5. Put mutations and recovery in a dedicated Actions tab with impact,
   confirmation, audit, and recovery details.
6. Use a high-contrast technical visual language with strong state encoding.
7. Retain one persistent global health score, supplemented by local severity
   and freshness badges; do not create a second health authority.
8. Make the tree the primary discovery mechanism, supported by aliases,
   related-item links, and search rather than search-first navigation.
9. Treat desktop and tablet interaction as equal primary targets, including
   touch-sized controls and responsive dense layouts.
10. Permit a bold new workspace structure while preserving all functionality,
    typed contracts, safe actions, and clear continuity to existing routes.

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
- Current state: The standalone repository preserves the old CEF/Servo stack.
  Browser uses a typed Workloads start/resume controller; RDP is preferred and
  Sunshine/Moonlight explicit. Dell runs immutable Fedora 44 source `1f8bd845…`,
  digest `sha256:17e38205…b0556`, accepted r5/r6 identity, encrypted login, and
  five-second alerts. Pushed RDP client `9660cbd9…` passed three strict runs:
  1920x1080 Chromium, reversible pointer/Escape input, 79.6% reconnect repaint,
  and 99.9% identity. Farm validators passed playback/capture endpoint wiring
  and fixed-fixture A/V decode; VA decode failed a rolled-back GL probe. Exact
  release `6f28404b…` auto-selected Workloads, brokered RDP, published active,
  and rendered Chromium in a native Dell readback. Public repository/host
  removal, sample-backed audio, five-tab performance, and fleet rollout remain;
  see the dated WL-ARCH-008 evidence.
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
     guest identity, Chromium/Sway ownership, RDP preferred transport,
     Sunshine alternate, and `host_browser=false`; the image still must contain
     Chromium, supported GPU/video acceleration, PipeWire integration, and
     guest agents.
  8. Replace shell `web` state with a small VM controller. Browser activation
     resolves and starts/resumes the stable workload, waits for its advertised
     desktop source, and exposes RDP plus Sunshine/Moonlight as explicit display
     paths with transport health and user-visible selection. RDP is preferred
     for the R1 Chromium service release; an unavailable selected transport
     offers the alternate and a preference change rather than silently
     switching. Store the mesh-wide preference in replicated settings, expose
     it in Browser settings and This Node, and apply changes on the next Browser
     launch.
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
      follows mute/volume policy. PulseAudio compatibility, PipeWire,
      WirePlumber, ALSA/UCM, device permissions, VM audio, and the selected
      VDI transport must all be live. A silent fallback or unavailable audio
      evidence fails production acceptance; the engineering preview may not
      promote until `.15` and the six-node testbed pass playback, capture,
      reconnect, and recovery.
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

### WL-ARCH-009 - Process-isolated mackesd and Advanced node administration

- Status: Remaining
- Priority: P0
- Complexity: Epic
- Problem: `mackesd` has grown into an 88-worker platform concentrated in one
  process. Dell evidence shows roughly 3 GiB resident daemon memory and a
  roughly 4.5 GiB cgroup peak after boot, while the current service has task
  accounting but no effective memory ceiling. The worker registry mostly knows
  only name, role rank, and restart policy, so optional providers, queues,
  caches, retries, and process ownership are not uniformly observable. A
  single failure domain makes memory growth, provider outages, and restart
  storms harder to contain. This Node has no authoritative mackesd runtime or
  fleet-administration page.
- Required outcome: Replace the monolithic runtime with six clearly named,
  independently supervised services: `mackesd-control.service`,
  `mackesd-observation.service`, `mackesd-actions.service`,
  `mackesd-data.service`, `mackesd-compute.service`, and
  `mackesd-integrations.service`. Keep Control as the sole persistent-state
  writer; use typed, versioned mde-bus contracts; make worker activation,
  resource budgets, cleanup, retry behavior, and health observable. Add
  `this-node/advanced` under This Node → Mesh & System as a desktop operator
  dashboard with local-first and explicit fleet-wide scope, complete typed
  worker/systemd controls, bounded evidence export, and per-node outcomes.
  The test fleet uses a coordinated hard cut with no dual-running monolith.
- Current state: The 79-worker registry now exposes a validated typed
  `WorkerSpec` for group, criticality, capability activation, cadence, bounded
  queue/cache policy, restart behavior, resource budget, namespace ownership,
  and shutdown cleanup. Group distributions and all registry entries have
  focused farm coverage. `spawn.rs` still constructs one daemon process, those
  budgets are not yet enforced by split systemd services, and This Node has no
  Advanced runtime/control surface.
- Remaining work:

  1. Baseline every current worker and service dependency on Dell and the
     test nodes. Record RSS, anonymous/shared memory, cgroup current/peak,
     CPU, tasks, queues, cache sizes, retry rates, startup time, and provider
     availability. Capture the boot allocation anomaly before refactoring.
  2. Wire the landed `WorkerSpec` into the single enforced runtime contract:
     group, criticality, capability predicate, cadence, queue/cache bounds,
     resource budget, restart policy, health key, state topic, action namespace,
     and cleanup ownership. Add drift tests proving every spawned worker is registered and
     every registered worker maps to exactly one named group.
  3. Implement the six process entrypoints and `mackesd.target`. Assign all
     existing workers exactly once to Control, Observation, Actions, Data,
     Compute, or Integrations. Control owns SQLite migrations/writes and
     shared configuration; other groups use typed bus contracts and bounded
     read models. Remove the old monolithic service from the hard-cut package.
  4. Add structured worker shutdown: cancel child tasks, close subscriptions
     and sockets, release external leases/handles, drain or reject bounded
     queues according to message class, and publish cleanup results.
  5. Add universal resource discipline: do not start unconfigured optional
     workers; use latest-wins telemetry, explicit command rejection, fixed
     TTL/byte/item cache limits, per-group/provider semaphores, jittered
     exponential backoff, circuit breakers, restart-storm suppression, and
     anomaly-triggered sampled allocator profiles.
  6. Install per-group systemd policies for MemoryHigh/MemoryMax, CPU
     weight/quota, TasksMax, IOWeight, watchdog, restart/start limits,
     filesystem protection, address-family restrictions, device allowlists,
     writable paths, and dedicated identities. The first release warns and
     captures evidence at normal budget thresholds; hard kill remains an
     explicit emergency setting rather than automatic normal recovery.
  7. Split the RPM into a lean core plus role/group subpackages. Audit bundled
     system dependencies, install only required Integrations/Compute payloads,
     use dedicated non-root identities for non-privileged groups, and retain
     root only for unavoidable Actions operations.
  8. Publish a strict-versioned `state/mackesd/<node>` snapshot and bounded,
     credential-free `/run/mde/mackesd-status.json` projection. Include group
     resources, worker state, queues, retries, capabilities, effective config,
     staleness, and audit pointers; never include secrets, raw logs, or
     unbounded diagnostics.
  9. Add typed `action/mackesd/*` controls for every supported worker/resource
     setting plus restart, pause, cleanup, refresh, profile apply, and evidence
     export. Validate, preview, transact, verify, and audit every mutation.
     Use existing exact-body, short-lived, single-use authorization; fleet
     configuration uses signed revisions and per-node acknowledgements.
  10. Add `this-node/advanced` to the durable catalog and a dedicated tab. The
      page opens on This Node, exposes an explicit fleet switch, uses typed
      filters plus explicit node checkboxes, shows large resource charts and a
      searchable process-grouped worker roster, keeps stale nodes visible but
      disables unsafe actions, and uses high-contrast white selectable choices
      in both themes.
  11. Add unit, integration, package, protocol, GUI, and live-fleet gates.
      Exercise every group crash, provider outage, queue saturation, restart
      storm, stale snapshot, partial fleet action, cleanup path, and schema
      mismatch. Require Dell to settle below 1 GiB and peak below 2 GiB; run
      the same failure matrix across the entire test fleet and record all
      non-Dell evidence even though Dell is the final memory acceptance gate.
  12. Deploy the hard cut to all test nodes simultaneously. If a node fails,
      preserve diagnostics and correct forward through the governed farm/release
      path; do not add a compatibility monolith or rely on rollback.
- Scope: In scope are mackesd worker/process architecture, memory and CPU
  governance, queue/cache/retry/cleanup policy, typed runtime snapshots and
  actions, Fedora systemd/RPM hardening, the This Node Advanced page, fleet
  targeting, audit/evidence, and test-fleet deployment. Out of scope are raw
  shell/SQL/filesystem controls, arbitrary systemd properties not represented
  by the typed schema, redesign of unrelated This Node providers, and a
  production rollout before the existing platform production gates pass.
- Relevant files/components: `crates/mesh/mackesd/src/worker_role.rs`,
  `crates/mesh/mackesd/src/bin/mackesd/spawn.rs`,
  `crates/mesh/mackesd/src/ipc/action_auth.rs`,
  `crates/mesh/mackesd/Cargo.toml`, `packaging/systemd/mackesd.service`,
  `crates/desktop/mde-shell-egui/src/this_node_catalog.rs`,
  `crates/desktop/mde-shell-egui/src/main.rs`, and the existing mde-bus/fleet
  state/action contracts.
- Dependencies: WL-UX-011 owns the durable This Node hierarchy, health-score
  integration, typed node actions, and provider truth; this epic owns the
  mackesd runtime contract and Advanced implementation lane. Coordinate visual
  primitives and contrast with WL-UX-009, integrated release evidence with
  WL-CRIT-006, and role/package contracts with the build/release governance.
  Use `docs/BUILD-ENVIRONMENT.md` and `AI_GOVERNANCE.md §10` for all heavy
  build, test, package, and live-fleet gates.
- Acceptance criteria:

  1. Every current spawned worker has exactly one registry entry, named group,
     capability predicate, budget, queue/cache policy, state topic, health
     key, restart policy, and tested cleanup owner.
  2. The six named services start independently under `mackesd.target`; a
     crashed group can restart without taking unrelated groups down, and no
     old monolithic service runs concurrently.
  3. Control is the only SQLite writer; cross-group state and actions use
     strict-versioned typed mde-bus contracts with bounded bodies and replies.
  4. Unconfigured optional workers do not start; retries, queues, caches,
     task counts, and provider concurrency remain within declared bounds.
  5. Dell settles below 1 GiB for the complete mackesd stack and remains below
     2 GiB during boot; any anomaly has allocator/cgroup attribution and a
     corrective test.
  6. Advanced reports exact group/worker state, resources, queues, retries,
     capabilities, staleness, effective settings, audit pointers, and action
     outcomes without secrets or raw unbounded logs.
  7. Advanced supports complete typed supported-setting administration,
     contextual actions, local-first scope, explicit fleet selection, signed
     revision rollout, partial-success reporting, and stale-node safety.
  8. Every mutation is traceable; compact successful records and detailed
     failure evidence are retained for one day with bounded purge behavior.
  9. Dark/Light selectable choices remain readable on white high-contrast
     surfaces with distinct hover, focus, selected, and disabled states.
  10. All failure-injection, protocol, packaging, worklist, focused GUI, farm,
      and Dell live gates pass, with unavailable hardware/provider evidence
      recorded honestly.
- Verification method: Run `install-helpers/lint-worklist.sh --self-test` and
  the normal worklist lint. Run focused mackesd and shell tests plus farm
  `cargo test -p mackesd`, touched shell tests, package/payload checks, and
  workspace/clippy gates with the longest job on BigBoy. Inspect installed
  systemd unit/cgroup state on every test node. Capture Dell boot/idle memory
  timelines, allocator evidence, service health, and Advanced screenshots in
  Dark/Light desktop layouts. Run the six-node chaos matrix and retain signed,
  redacted evidence bundles; do not claim live hardware proof when unavailable.
- Origin or merged source IDs: 2026-08-01 operator evaluation of mackesd
  growth, Dell boot memory regression, process isolation, performance,
  resilience, Fedora best practice, and fleet-wide Advanced administration.
  Incorporates the 50-question decision pass: under-1-GiB idle target,
  under-2-GiB boot peak, six clear process groups, mde-bus contracts, one-day
  evidence retention, trusted-mesh mutation authority, typed forms, signed
  fleet revisions, and Dell as the final memory gate.

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
- Current state: Shared collaboration types, core, UI, and shell/daemon
  integration cover the major Teams/channel rails, Posts/Files/Calls, Activity,
  composition, Details/search/threads, pin/save/task/membership, notification,
  clipboard, persistence, and bounded provider-state foundations with focused
  farm proof. The parity ledger remains the authority for open rows. Real
  Discord/media/SIP/AI, complete document/file/transfer workflows, migration,
  provider-backed live evidence, and removal of superseded surfaces remain
  open. Exact landed-slice and live-render evidence is preserved in the dated
  pre-lint-compaction snapshot.
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
     and transcription remain absent. Audio must stream between enrolled
     Mesh nodes over the direct path and the elected relay path with bounded
     jitter, loss recovery, mute/volume state, device changes, authorization,
     and reconnect behavior.
  6. Complete full-IDE Markdown document entry, Yrs coauthoring, anchored review
     threads, shared presence/follow mode, external-write three-way review,
     version history, and safe Git behavior.
  7. Complete channel Files and transfer-first workflows against stable file
     references, resumable hash-verified transfers, new-member backfill,
     reference-versus-delete semantics, and the shared operation-progress
     authority. Extend the standalone Files workspace into the node-aware
     control surface: expose enrolled nodes and their sync state, prepopulate
     links/shares and destination metadata, and provide ten automatic
     interactions covering node browse, share/link creation, sync inspection,
     transfer routing, Airsonic upload targeting, retry/resume, conflict
     review, offline queueing, node health, and per-node activity.
     Typed ten-action inventory, per-node sync state, and provider-gated
     Airsonic discovery are farm-tested. Completed Files, Terminal, and Editor
     render slices are recorded in the DRM ledger. The explicit Files proof
     route also produced wrong-surface frames on .138, so a feature-correct
     direct-DRM provider-advertisement proof remains open. Exact payload,
     capture, regression, rollback, and style-gate history is preserved in the
     dated pre-lint-compaction snapshot.
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
     report through the shared progress projection. The Files workspace exposes
     ten tested node-aware interactions, prepopulates every available link/share
     and Airsonic upload destination, and makes per-node sync state identifiable
     without inventing unavailable services.
  6. Direct and SFU-relayed calls carry advancing audio/video/screen frames;
     SIP ingress/egress, device controls, failover, and remote-control
     escalation work; no recording/transcription artifact exists. Node-to-node
     audio streaming is independently proven on direct and relay paths,
     including microphone capture, speaker playback, mute/volume propagation,
     device switching, packet-loss/jitter handling, authorization, and
     reconnect after node or route loss.
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
- Current state: Canonical clip events/actions, direct DRM copy/cut/paste,
  bounded shell/daemon/KDC/mobile materialization, signed seat targeting,
  retry/deduplication/attribution, Mesh Teams opt-in history/actions, and VNC
  clipboard round-trip have focused farm coverage. Guest-to-client Bus handoff
  is wired; client-to-guest remains gated by live protocol ownership. RDP
  CLIPRDR and SPICE vdagent remain explicitly unsupported, and the production
  signed-target adapter plus live VDI proof remain open. Exact test and
  implementation evidence is preserved in the dated pre-lint-compaction
  snapshot.
- Remaining work:

  1. Wire the production OS/guest clipboard adapter for the signed target-seat
     handoff, preserving bounded retry and explicit unavailable-state reporting;
     do not reintroduce Wayland polling.
  2. Route Mesh Teams clipboard UI/actions to the same lane. Persist team
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
- Current state: Versioned multi-manager MG90 identity/radio/freshness
  contracts, bounded Maps/Car projections, roster/cache/resync behavior, OBD
  outcome typing, offline-region admission, and route-start guards are
  farm-tested. Seat 15 has published sanitized MG90 LTE/GNSS/power/ignition
  observations; OBD remains explicitly untyped until its schema is verified.
  Full live-adapter cadence, all-radio discovery, multi-manager action rollout,
  Valhalla navigation, real map/route/vehicle rendering, and end-to-end live
  acceptance remain open. Exact credential-free evidence is preserved in the
  dated pre-lint-compaction snapshot.
- Remaining work:
  1. Consume WL-UX-009's platform-wide Carbon-requirement retirement and update
     the Maps/Car-specific authority while retaining egui, shared `Style`,
     Construct/Car, HIG principles, and Car Dark/Light modes. Inspect governance
     history first, leave `AGENTS.md` untouched, and do not create a repo-root
     `CLAUDE.md`.
  2. Extend the landed versioned `VehicleState` v2 baseline to multiple MG90s,
     multiple managers, and the full rolling-upgrade removal plan.
  3. Extend the landed bounded `RadioId`/health inventory with live SKU
     discovery and multiple-manager routing. Consumer-side stale/resync/cache
  behavior is now covered for the Maps roster fold; live adapter evidence and
  cross-surface rollout remain.
  The vehicle worker now rejects unsupported v2 schemas before acceptance and
  exposes a typed manager route that skips revoked or unenrolled suppliers while
  preserving an honest no-source/rejected result; its focused farm gate passes
  79 tests with 4,238 filtered on `.90` slot `func017-vehicle-review-239`.
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
- Current state: Signed catalog projection, typed App VM/OpenApp/session/VDI
  contracts, a bounded immutable Wayland/Flatpak image profile, lifecycle and
  generation/replay/identity admission, favorites/permission states, and
  guest-owned launch/evidence policies have focused farm or static-image proof.
  Host-launch fallback remains rejected. Full provision/install/guest-process/
  portal/VDI app-mode convergence, remote signing, data/update/removal policy,
  live GPU/audio/input/reconnect proof, and curated LibreOffice/printing remain
  open. Exact implementation and image evidence is preserved in the dated
  pre-lint-compaction snapshot.
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
     than creating a second authorization plane. The typed App VM profile now
     enforces bounded vCPU (1..=8), memory (1..=32768 MiB), disk (16..=256
     GiB), mandatory network isolation, an eight-capability maximum, and an
     explicit allow-list before image admission or desired-state persistence;
     the pure mesh-type contract and nine daemon provisioning regressions pass
     on farm `.90` slot `func018-app-daemon-191`. Individual catalog rows also
     fail closed when `is_launchable()` is called before catalog projection;
     five catalog policy tests pass on farm `.50` slot
     `func018-catalog-launchable-194`.
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
     work through the declared guest policy; audio is mandatory and must reach
     the host/VDI mixer with working capture and playback. No unrestricted host
     access is granted.
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

### WL-FUNC-019 - Make Remote Sessions the universal resource browser

- Status: Remaining
- Priority: P0
- Complexity: Epic
- Problem: Construct's Remote Sessions view is currently a narrow desktop
  chooser. It does not provide one durable identity for local, mesh, LAN,
  gateway, media, file, VM, container, cloud, or network resources, and its
  supported client transports are not admitted from a typed capability
  registry. Sunshine/Moonlight, SSH, SSH X11 applications, full remote X11
  desktops, Jellyfin, and Subsonic/OpenSubsonic therefore cannot reliably be
  discovered, authenticated, browsed, and opened from the Thin Client's main
  interface. A resource that is visible through one worker but unsupported by
  the shell is misleading; a control that launches arbitrary commands would
  violate the bounded client boundary.
- Required outcome: Remote Sessions is always Construct's primary onboarding
  and resource-browsing surface. It shows one deduplicated card per resource,
  with source, protocol/client capabilities, health, last-seen state, auth
  state, and safe actions. Every resource for which Construct has a native
  client or an explicitly approved typed adapter is exposed here, including
  resources discovered locally, over mesh, on the trusted LAN, through
  configured gateways, or from typed manual sources. Selecting a card hands
  off to the existing native session/client surface while preserving card and
  session state; no arbitrary shell command, plaintext credential, public
  exposure, or UI-only fake capability is introduced.
- Current state: `resources.rs` now provides a strict versioned
  `ResourceIdentity`, card, transport/capability, auth/health, provenance, and
  bounded-action contract with deterministic fingerprints and hostile-input
  tests. Existing desktop and media publishers remain separate projections;
  the shell chooser still knows only RDP/VNC/SPICE. The canonical resource
  topic, capability registry, deduplicating discovery fold, SSDP/UPnP, SSH/X11,
  Moonlight, Subsonic, universal landing surface, and all-seat Sunshine proof
  remain open.
- Remaining work:

  1. Publish the landed versioned `ResourceIdentity`, `ResourceCard`,
     `TransportCandidate`, `ClientCapability`, `AuthState`, `HealthState`,
     `SourceProvenance`, and bounded action model on a canonical resource topic
     (for example `state/resources/catalog`) with
     stable IDs, aliases, last-seen timestamps, failure reasons, and
     capability fingerprints. Keep desktop/media topics as compatibility
     projections during migration, not competing identities.
  2. Build an automatic typed client-capability registry. Admission must be
     based on a registered native client or approved platform adapter, with
     protocol version, OS/guest boundary, auth requirements, feature limits,
     and safe action policy. Adding a supported client must automatically make
     matching discovered resources visible without a new hard-coded chooser
     branch; unsupported or malformed advertisements remain visible as
     unavailable evidence only when useful, never as launchable controls.
  3. Normalize all discovery lanes into that catalog: replicated mesh peer
     descriptors; existing mDNS/DNS-SD; local service/session enumeration;
     trusted-LAN SSDP/UPnP; configured gateway registries; and typed manual
     sources. Use resource identity plus endpoint/capability fingerprints to
     deduplicate one service found by multiple lanes. Bound TTLs, retries,
     interface scope, packet sizes, and concurrency; do not turn discovery
     into an unbounded port scan.
  4. Use the existing Rust `mdns-sd` lane for mDNS/DNS-SD. Add `rupnp` for
     async SSDP/UPnP discovery and control, with explicit interface/trust
     policy. Do not switch to `mdns-sd-discovery` unless an OS-native resolver
     gap is demonstrated. Keep the adapter boundary protocol-agnostic so
     future clients can register without rewriting workers or the shell.
  5. Add native in-shell Sunshine/Moonlight transport support. Use the
     official `moonlight-common-c` core through a narrowly owned Rust FFI
     adapter, bundling and testing its exact ENet dependency; use
     `moonlight-embedded` as a protocol/reference oracle, not as an
     unbounded application embedding. Record the GPL-3-compatible packaging,
     ABI, cross-build, hardware decode, audio, input, pairing, suspend,
     reconnect, and frame-pacing obligations before enabling the client.
  6. Complete the all-seat Sunshine server rollout for T480, Eagle/T470S,
     Basement seat 15, and Dell. Make typed remote-proofing settings the
     source of truth: enabled, LAN plus Nebula/mesh exposure, KMS capture,
     automatic encoder, pairing and local approval, visible shadowing/input
     indicator, remote input, VNC fallback, and 30 FPS. Add the missing
     combined LAN+mesh policy to settings, generated lifecycle/firewall
     policy, status descriptors, and UI. Allow TCP 47984, 47989, 48010 and
     local-only web management on 47990 plus the required UDP 47998-48010
     transport range on LAN and mesh; deny public exposure, disable Sunshine
     UPnP port mapping, and verify listeners/firewall/pairing on every seat
     without rebooting encrypted seats.
  7. Add SSH and X11 adapters. Use `russh` plus `russh-config` for typed SSH
     discovery/session/auth and its X11 forwarding primitives. Use `x11rb`
     only for a local X11 protocol client/display integration. Model both SSH
     X11 application sessions and full existing remote X11 desktop sessions;
     require an explicit display/session endpoint for the latter and never
     infer one by blind scanning. State clearly when a DRM seat lacks a local
     X server and cannot render an X11-forwarded application.
  8. Unify media admission without losing domain-specific clients. Expose
     Jellyfin resources through the existing client and its server OpenAPI
     contract; use a hand-written auth/policy facade and optionally
     `progenitor` for bounded generated OpenAPI transport code. Add an
     `mde-subsonic` adapter for the OpenSubsonic REST JSON/XML contract using
     the `opensubsonic` crate where suitable, covering Navidrome, Airsonic,
     and compatible Subsonic servers. Retain distinct typed adapters for
     DLNA/UPnP, MPD, file shares, and mesh media rather than falsely labeling
     music-only services as Jellyfin.
  9. Replace the chooser-only landing with a browse-first Remote Sessions
     catalog. Cards must show resource class, origin (local/mesh/LAN/gateway),
     available native clients, transport health, trust/auth state, and
     actions such as inspect, pair, connect, retry, forget, or request
     approval. Keep offline cards with last-seen and failed transports; make
     LAN resources visible immediately but action-gated until trust/auth is
     approved. Embed native Construct clients and preserve reconnect/session
     context; typed platform adapters may delegate rendering but may not
     accept arbitrary command or URL execution.
  10. Store credentials/tokens/keys only in the approved secret store and
      pass opaque references to adapters. Add pairing/approval expiry,
      revocation, audit events, per-resource trust, mesh-vs-LAN policy, and
      redaction tests. Expose capability and health reasons without leaking
      secrets. Add migration/versioning for existing desktop/media records,
      operator diagnostics, onboarding copy, and a deterministic unavailable
      state for absent hardware, provider, display, codec, or credentials.
- Scope: In scope are the shared catalog contract, identity/deduplication,
  mesh/LAN/local/gateway/manual discovery, typed client registry, primary
  Remote Sessions UI, Sunshine server rollout and native Moonlight client,
  SSH/X11 modes, Jellyfin, Subsonic/OpenSubsonic, DLNA/UPnP, MPD, file-share,
  auth/trust/secrets, offline retention, and migration of existing desktop and
  media projections. Out of scope are a general-purpose arbitrary protocol
  launcher, public Internet exposure, automatic router port mapping, blind
  network scanning, replacing the existing Jellyfin/media/VDI clients, or
  making every advertised protocol launchable without an approved adapter.
- Relevant files/components: `crates/mesh/mackes-mesh-types/src/peers.rs`,
  `crates/mesh/mackesd/src/workers/desktop_sources.rs`,
  `crates/mesh/mackesd/src/workers/media_sources.rs`,
  `crates/mesh/mackesd/src/descriptors.rs`,
  `crates/desktop/mde-shell-egui/src/chooser/` and `vdi/`,
  `crates/desktop/mde-shell-egui/src/system/mesh.rs`,
  `install-helpers/mde-remote-proofing-apply.py`,
  `packaging/systemd/mde-remote-proofing-plan.*`, `mde-jellyfin`, and the
  existing `docs/design/desktop-chooser.md` / media-source contracts.
- Dependencies: WL-ARCH-008 owns the Browser VM cutover and remains a
  transport consumer; this epic owns the universal resource/session surface
  and its adapter admission. Coordinate with the existing Remote Proofing,
  VDI, media, peer-descriptor, secret-store, systemd, and firewalld contracts;
  do not create separate backend or UI worklist epics for those lanes.
- Acceptance criteria:

  1. Remote Sessions is the first and always-available Construct onboarding
     surface and renders a browseable catalog with one card per deduplicated
     resource, honest unavailable/offline states, and preserved session state.
  2. Mesh, mDNS, local, trusted-LAN SSDP/UPnP, configured gateway, and typed
     manual sources converge into the versioned catalog with bounded retries,
     provenance, TTL/last-seen, health, and capability metadata.
  3. A capability registry automatically exposes every supported native client
     or typed platform adapter and rejects arbitrary commands and unsupported
     launch actions.
  4. All four named seats advertise and serve Sunshine; Dell and every seat
     discover the service over the allowed LAN and Nebula paths, pair with
     local approval, connect through the native Moonlight path, and recover
     from reconnect/suspend/input/audio/frame failures. Public listeners and
     UPnP port mapping remain absent.
  5. SSH resources, SSH-forwarded X11 applications, and explicit full X11
     desktop endpoints are separately labeled, authenticated, and launchable
     only when the corresponding local/client capability is present.
  6. Jellyfin, Subsonic/OpenSubsonic (including Navidrome/Airsonic), and the
     supported DLNA/UPnP, MPD, file-share, and mesh-media resources are
     detected and exposed through the correct typed client or unavailable
     explanation.
  7. Credentials never appear in discovery records/logs; trust, approval,
     revocation, secret references, and LAN-vs-mesh policy are testable and
     visible at the action boundary.
  8. Existing desktop/media consumers remain compatible during migration and
     no duplicate card or stale launch path survives the cutover.
  9. Focused unit/property/contract tests, package/license/ABI checks, and
     farm workspace gates pass; live GUI/transport evidence is captured on
     reachable seats, with every unavailable provider or hardware dependency
     recorded explicitly rather than implied by tests.
- Verification method: Add mesh-type schema, identity/deduplication,
  capability-admission, discovery TTL/retry, mDNS/SSDP fixtures, descriptor,
  secret-redaction, trust, offline-retention, migration, and UI-state tests.
  Add adapter tests for Moonlight pairing/session/reconnect, SSH auth and both
  X11 modes, Jellyfin OpenAPI/auth, OpenSubsonic JSON/XML, DLNA/UPnP, MPD, and
  file shares. Run focused jobs on the build farm plus workspace fmt/clippy,
  worklist/doc/architecture/secret/package gates; put the longest
  moonlight/codec/VDI integration build on BigBoy. Finish with live captures
  from all four seats, LAN+Nebula firewall/listener inspection, pair/connect/
  reconnect/input/audio proof, and honest unavailable-hardware/provider
  records. Use `mdns-sd` and `rupnp`, official `moonlight-common-c`,
  `russh`/`russh-config`, `x11rb`, `opensubsonic`, `progenitor`, `zbus`,
  systemd, and firewalld D-Bus documentation as the implementation references.
- Origin or merged source IDs: User decisions from the Remote Sessions
  discovery interview: native plus approved adapters; all client-capable
  resources; mesh/LAN/local/gateway discovery; browse-first cards; one card
  with many transports; secret-store credentials; offline retention; visible
  but action-gated LAN trust; embedded native handoff; and automatic typed
  capability admission. Absorbs the Sunshine/Moonlight, SSH/X11, Jellyfin,
  Subsonic, and universal-resource-browser request into one epic. Existing
  lineage is `docs/design/desktop-chooser.md`, `docs/design/peer-directory.md`,
  `docs/design/mesh-media-player.md`, WL-ARCH-008, and the Remote Proofing
  evidence. External research references are `mdns-sd`, `rupnp`,
  `moonlight-common-c`, `moonlight-embedded`, `russh`, `x11rb`, Jellyfin
  OpenAPI, OpenSubsonic, `opensubsonic`, `progenitor`, `zbus`, systemd, and
  firewalld upstream documentation.

### WL-FUNC-020 - Expose governed Android applications in Workloads

- Status: Remaining
- Priority: P1
- Complexity: Large
- Problem: Workloads models Android as a two-layer Cuttlefish VM and can
  provision the outer domain, but it does not yet project guest applications as
  durable workload entries. A user therefore cannot see which AOSP applications
  an Android image contains, whether each package is ready, or launch one through
  a typed guest boundary. Treating package names or `adb shell` strings as
  generic commands would violate the bounded Thin Client action model.
- Required outcome: Workloads presents three honest application/workload
  families: the dedicated Chromium VM, Android applications backed by an
  admitted Cuttlefish Android VM, and Flatpak applications backed by App VMs.
  Its Android section shows a governed AOSP starter catalog, per-VM package and
  readiness evidence, and typed launch actions. Launching an available entry
  starts or resumes its Android VM, opens the inner guest display, and dispatches
  only a closed `MAIN` plus `LAUNCHER` intent; unavailable images, packages,
  providers, capacity, or transports remain visible with exact reasons.
- Current state: `DeliveryType::AndroidVm`, `android-provision`, the Cuttlefish
  OpenTofu module, outer-VM lifecycle, and inner VNC/WebRTC console modeling are
  present. `android_apps.rs` now defines a versioned nine-app AOSP starter set,
  closed package and launch-intent identities, strict inventory validation, and
  honest availability/readiness states. The active lifecycle-first Android
  Plan and Run routes project those entries as integration-pending and
  non-launchable. A live guest
  inventory provider, image/package contract, typed dispatch worker, session
  handoff, persistence, and Cuttlefish proof remain open.
- Remaining work:

  1. Make the Workloads information architecture explicitly expose Chromium VM,
     Android applications, and Flatpak/App VM entries without merging their
     lifecycle or security boundaries. Keep Front Door as the normal Flatpak app
     launch surface while Workloads owns the backing App VM lifecycle.
  2. Add a versioned Android guest-inventory provider keyed by stable Android VM
     identity. Report image provenance, package identity/version, launcher
     resolvability, guest boot state, observation age, and exact unavailable
     reasons through bounded records; reject unknown fields, duplicate apps,
     arbitrary components, URIs, extras, flags, and command strings.
  3. Pin and verify the supported Cuttlefish image manifest for Browser,
     Calendar, Camera, Clock, Contacts, Files, Gallery/Photos, Calculator, and
     Settings. Where an upstream image omits an app, either supply it through the
     governed image build or report `image unavailable`; never fabricate an
     installed state.
  4. Add an authorized `action/android/app-launch` contract and `mackesd` worker
     that accepts only the closed catalog identity plus target Android VM. Resolve
     the package through the guest provider and dispatch the typed launcher
     intent without shell interpolation, arbitrary `adb` arguments, or host app
     execution. Publish correlated request, audit, readiness, and result records.
  5. Connect launch to the existing Workloads/session-broker/VDI lifecycle: place
     or resume the outer VM, wait for `cvd` and guest readiness, select the inner
     VNC/WebRTC head, preserve focused input/audio/clipboard policy, and recover
     across reconnect, suspend, and placement-node loss.
  6. Fold live inventory into the pending Android cards, including per-VM scope,
     offline retention, stale evidence, progress, retry, and inspect actions.
     Enable Launch only when image, package, guest, authorization, capacity, and
     console transport are all admitted.
  7. Produce a real Cuttlefish image and nested-KVM lifecycle proof on a capable
     placement host, then launch each starter app from reachable Workstation
     seats and capture guest-owned frame, focused input, audio where applicable,
     reconnect, failure, and unavailable-image evidence.
- Scope: In scope are the governed AOSP starter catalog, Android image/package
  manifest, per-VM guest inventory, typed launcher intent, Workloads projection,
  existing Cuttlefish lifecycle/session/console integration, persistence,
  authorization, recovery, and live proof. Out of scope are arbitrary APK
  upload, Play Store or proprietary Google applications, unrestricted Android
  intents, host-native Android execution, shell/command launchers, and collapsing
  Android apps into Flatpak App VMs.
- Relevant files/components:
  `crates/mesh/mackes-mesh-types/src/android_apps.rs`,
  `crates/mesh/mackes-mesh-types/src/cloud.rs`,
  `crates/desktop/mde-shell-egui/src/iac/android_apps.rs`,
  `crates/desktop/mde-shell-egui/src/iac/mod.rs`,
  `crates/mesh/mackesd/src/workers/`, `infra/tofu/cloud/main.tf`,
  `infra/tofu/cloud/modules/android_vm/`, Cuttlefish image packaging, and the
  existing Workloads/session-broker/console-broker/VDI contracts.
- Dependencies: Coordinate the top-level Workloads presentation with
  WL-ARCH-008's Chromium VM and WL-FUNC-018's Flatpak/App VM ownership. Reuse
  WL-CRIT-006 admission and live-evidence discipline. Live closure requires a
  placement node with KVM, nested virtualization, and enough memory/storage for
  the current Android profile; a Workstation seat may remain a client only.
- Acceptance criteria:

  1. Workloads visibly and separately presents the Chromium VM, Android apps,
     and Flatpak/App VM family with stable identities and honest lifecycle state.
  2. The nine governed starter apps appear in stable order with package,
     category, target Android VM, availability, readiness, image provenance,
     observation age, and actionable failure reason.
  3. Launch is enabled only for a fresh, installed, ready package in an admitted
     Android VM and emits only the closed `MAIN` plus `LAUNCHER` intent; hostile,
     malformed, duplicated, oversized, stale, or command-shaped records fail
     closed before authorization or guest contact.
  4. Selecting an app places or resumes exactly one typed Android VM session,
     opens the inner Cuttlefish display through the existing console/VDI path,
     and preserves focused input plus supported audio/clipboard policy without a
     host-native or arbitrary-command fallback.
  5. Missing image packages, guest/provider loss, insufficient capacity,
     authorization denial, console failure, and reconnect/suspend loss are
     visible, auditable, retryable where safe, and never reported as success.
  6. Catalog, inventory, action, worker, session, UI, image, and hostile-input
     tests pass on the farm; a capable live host proves real Cuttlefish boot and
     at least one frame/input launch for every available starter entry.
- Verification method: Add mesh-type schema/round-trip/property tests; hostile
  catalog and inventory fixtures; package-manifest and image-provenance gates;
  worker authorization, replay, timeout, redaction, and dispatch tests; shell
  pending/ready/stale/unavailable render tests; and session/console reconnect
  coverage. Run independent farm jobs with the longest Cuttlefish/image gate on
  BigBoy, plus worklist/doc/package/secret gates. Finish with live nested-KVM
  guest package inventory and per-app frame/input evidence from reachable seats,
  recording unavailable hardware or omitted upstream packages explicitly.
- Origin or merged source IDs: 2026-08-03 operator decision that Workloads must
  eventually list the Chromium VM, a set of AOSP Android apps, and Flatpaks.
  Continues the archived WL-ARCH-007 Android VM preparation and live-proof gaps
  without reopening that broader Workloads cockpit epic or duplicating
  WL-ARCH-008/WL-FUNC-018.

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
- Current state: Schema-4 release evidence, signing, CI/farm binding, strict
  topology/recovery/live-attestation validators and collector, VDI evidence,
  and SBOM/package/health/audit/backup foundations have fail-closed self-tests.
  The live fleet currently shows three lighthouses plus five healthy
  Workstations with unique overlays, including corrected-forward T480 and new
  Surface enrollment. Production still lacks required-check publication,
  drill-ledger-backed topology/recovery evidence, live VDI/audio proof, and a
  signed promotion bundle; current health is not historical drill proof.
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
  8. Make audio a hard release gate: every production candidate proves local
     PulseAudio-compatible playback/capture, PipeWire/WirePlumber graph health,
     VM/VDI audio, Mesh Teams calls, and node-to-node direct/relay streaming
     on the six-node testbed. `.15` must pass the same gate; a missing or
     broken PulseAudio compatibility layer fails promotion.
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
  11. Audio passes on all six nodes: `pactl`/PulseAudio compatibility,
       `pw-cli`/PipeWire graph, WirePlumber policy, ALSA/UCM device discovery,
       speaker playback, microphone capture, HDMI/Bluetooth where present,
       Browser/App VM/VDI audio, Mesh Teams calls, and direct plus relayed
       node-to-node streams. The `.15` evidence bundle includes live playback,
       capture, reconnect, and recovery proof.
- Verification method: Run worklist, governance, documentation, secret,
  package, compatibility, and provenance lints; run GitHub required checks and
  farm gates with BigBoy carrying the long pole; run parallel six-node tests for
  join, failover, recovery, state convergence, resource pressure, retention,
  and VDI reconnect; perform signed-artifact and Fedora transaction checks;
  preserve live evidence bundles and explicit unavailable-hardware notes.
- Origin or merged source IDs: Fit-for-purpose audit 2026-07-30 (`AUD-01`
  through `AUD-07`, `AUD-09`, and `AUD-17` through `AUD-25`); operator-selected
  production gate, topology, recovery, trust, retention, and testbed decisions.

### WL-CRIT-007 - Boot, sleep/resume, and fleet peer return recovery

- Status: Remaining
- Priority: P0
- Complexity: Epic
- Problem: Mesh members can boot or return from laptop sleep with stale Nebula
  state, missing lighthouse maps, duplicate overlay identities, or an
  unavailable coordination quorum. Peers then remain absent or report errors
  even when the local services appear active. The recovery watchdog also
  ignored migrated identities stored under `identity/current`.
- Required outcome: Every enrolled seat and lighthouse returns to a unique
  overlay identity, current lighthouse roster, healthy etcd coordination,
  active mackesd/Syncthing health, and visible peer presence after boot,
  suspend/resume, underlay changes, and one lighthouse loss. Recovery must be
  corrected-forward and must not require credential disclosure or manual
  certificate editing.
- Current state: Eight unique overlay identities cover the three-lighthouse
  quorum and five Workstations. Surface is `peer:SURFACE`/`10.42.0.7`; T480's
  old-CA state was backed up, removed via leave, and re-enrolled as
  `peer:T480`/`10.42.0.8` with etcd/Syncthing restored. Join keeps an existing
  role pin authoritative; leave removes old authority pins without following
  symlinks, reports failure, and permits a new pin only after successful teardown.
  Seat 15's reboot exposed a stale lighthouse override and three unshared
  Syncthing peers; both were root-only backed up and corrected. It now reaches
  all seven overlay nodes, has 4/4 folder peer connections, and passes
  mesh-health. Physical laptop suspend/resume, lighthouse loss, and
  drill-ledger-backed corrected-forward evidence remain missing; current health
  alone does not close recovery.
- Remaining work:
  1. Add a durable enrollment/overlay identity collision check that refuses to
     start a node when its certificate address is already claimed by another
     active peer or lighthouse.
  2. Make current lighthouse roster materialization boot- and resume-safe;
     stale retired underlay addresses must not overwrite a healthy roster.
  3. Add explicit post-resume recovery for Nebula, etcd clients, mackesd, and
     Syncthing with bounded backoff and no restart storm.
  4. Run a physical boot, suspend/resume, underlay reconnect, lighthouse-loss,
     and corrected-forward re-enrollment drill on Dell, Eagle, and seat 15.
  5. Resolve the seat's Documents bind-mount gate and disk-headroom warning,
     then regenerate node-bound evidence for production acceptance.
- Scope: Nebula identity/configuration, systemd ordering, mesh-health recovery,
  etcd/Syncthing coordination and peer presence, enrollment/re-enrollment,
  and six-node live evidence. It does not own MG90 telemetry semantics, which
  remain under WL-FUNC-017.
- Relevant files/components: `packaging/systemd/mackesd.service`,
  `packaging/systemd/nebula.service.d/10-mesh-recovery.conf`,
  `install-helpers/mesh-health-check.sh`, `install-helpers/verify-boot-recovery.sh`,
  `install-helpers/setup-etcd.sh`, Nebula enrollment/materialization,
  `docs/ops/mesh-boot-resume-diagnosis-2026-08-02.md`, and the live six-node
  topology verifier.
- Dependencies: WL-CRIT-006 production evidence and corrected-forward
  recovery; the current etcd/Syncthing substrate; Nebula enrollment and CA
  authority; physical Dell, Eagle, and seat hardware.
- Acceptance criteria:
  1. A fresh boot and a suspend/resume cycle leave each node with the same
     unique overlay address, current lighthouse maps, and active Nebula.
  2. etcd endpoint health, leader election, peer heartbeats, and Syncthing
     connections recover without manual service intervention.
  3. `mackesd peers` reports all available seats and lighthouses online with
     no duplicate overlay addresses or retired lighthouse endpoints.
  4. The recovery watchdog acts on current-generation identities and remains
     bounded under repeated underlay loss and resume events.
  5. A node can be corrected-forward re-enrolled with preserved evidence and
     no credentials in logs, Git, or worklist records.
  6. The six-node boot/resume/lighthouse-loss evidence bundle passes the
     production verifier and documents any unavailable physical proof.
- Verification method: Run the worklist self-test and shell syntax checks;
  use farm lanes for code/build gates; use real Dell, Eagle, and seat hardware
  for reboot and suspend/resume; capture systemd, Nebula, etcd, Syncthing,
  peer-directory, overlay-reachability, and node-bound evidence before and
  after each transition.
- Origin or merged source IDs: Operator-reported boot/sleep peer-return bug
  (2026-08-02); live recovery evidence in
  `docs/ops/mesh-boot-resume-diagnosis-2026-08-02.md`.

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
- Current state: Shared Quazar primitives and most Construct-owned route
  migrations are landed, with focused farm suites and representative direct-DRM
  evidence in the governed ledger. Multiple Dell and .138 Dark/Light, narrow,
  and large-text slices are accepted, but evidence spans several payloads.
  Current-payload full-matrix proof, remaining route/state adoption, package
  transaction, style/asset cleanup, and production live acceptance remain open.
  Exact payload, capture, regression, and resolution history is preserved in
  the dated pre-lint-compaction snapshot.
- Remaining work:

  - Execution update (2026-08-01): host Browser packaging/runtime payloads were
    removed from the working tree and the VM-only activation guard now evicts
    stale host-helper tabs before selecting `browser-vm`. The local candidate
    standalone tree remains audit-only: it lacks a root workspace, clean-clone
    build proof, and complete dependency closure. Cargo/workspace and release
    build cleanup, accepted standalone publication, host-source removal, and
    live VDI framebuffer proof remain open.

  0. Re-run the direct-DRM EGL-readback matrix against the current installed
     `.138` payload `f58b42ba…` (and separately against Dell after it is
     reachable) for every Construct-owned route that still relies on
     representative or older-payload evidence (Editor, Terminal, Phones, Car,
     and any remaining Workbench/Infra, Music, Media, Browser boundary, Mesh
     Teams, or This Node cells). Inspect Dark desktop, Light desktop, narrow,
     and Light/Largest frames, retain only non-overlapping readable captures,
     and restore secure login-at-boot state after each proof batch. The
     proof-only settle window is implemented and farm-tested; retain it for
     asynchronous/expressive routes, with normal production timing unchanged.
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
  Maps Drive HUD large-text geometry is now evidence-backed: the floating-action-
  button lane is reserved and the health rail uses the fixed multi-row layout;
  focused Maps tests and Light/Largest DRM evidence cover this slice. Broader
  Maps state and full-route matrix coverage remain in items 2 and 5.
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
  experience follows a technical Device Manager model: a unified expandable
  tree, an inventory-first landing view, and focused full-page detail views for
  technical operators/admins. The existing top-rail health score remains the
  single health authority; selecting it opens this tree with unhealthy items
  highlighted. Monitoring and configuration have equal priority. Critical
  alerts, affected components, stale data, and recovery actions remain visible
  within the inventory landing view; a healthy node presents a calm
  all-systems-operational summary. Health severity uses
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
- Current state: This Node now has a governed catalog/search/hierarchy,
  health/stale provider projections, 16 routes, full-page details, Actions and
  audit views, and typed credential-free providers for network, BlueZ, audio,
  display, input, power, storage, firmware, privacy, and other local capability
  evidence with focused farm proof. Many controls remain unavailable or
  integration-gated. Legacy-route removal, complete privileged mutation
  coverage, broad OS-management consolidation, and live device, DRM, audio,
  recovery, Surface, and fleet evidence remain open. Exact implementation
  history is preserved in the dated pre-lint-compaction snapshot.
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
  16. Onboard and close the Microsoft Surface physical validation seat
      `172.20.146.79` (Surface Pro 6, Fedora 44 Server) against the current
      workstation-seat baseline. The seat currently has DRM nodes but no
      `magic-mesh` package, no pinned Workstation role, and inactive `mackesd`
      and `mde-shell-egui` services; the live reference seat `.138` is on
      `magic-mesh-12.1.6-1` with the Workstation role and both services active.
      First cut a native Fedora 44 workstation RPM on the farm's halted
      `mcnf-build-f44` builder after BigBoy capacity is available; do not
      interrupt active BigBoy jobs or install the existing F42-linked artifact
      whose FFmpeg sonames are incompatible with Fedora 44. Verify the new RPM
      has the Fedora 44 FFmpeg sonames and ships the complete shell/daemon/
      Browser/media payload, then deploy it, pin/provision the Workstation
      role, join or enroll the seat through the typed onboarding path, and
      verify zero-restart `mackesd` plus stable DRM shell scanout. Exercise
      every Surface-relevant This Node capability and safe action that the
      hardware exposes: display/modeset/brightness, keyboard backlight,
      pointer/touch/pen/gestures, Wi-Fi/Ethernet/Bluetooth, PipeWire audio and
      capture privacy, camera presence/privacy, battery/AC/charge limit,
      lid/sleep/thermal/performance telemetry, storage, sensors, firmware,
      docks/Thunderbolt, mesh reachability, and audit/recovery behavior.
      Record each absent provider or unsupported Surface feature as an honest
      disabled/unavailable result; never infer capability from DMI alone or
      fabricate a successful write. Capture desktop, narrow, large-text,
      stale/provider-loss, refusal, recovery, and physical-input proof, restore
      secure login-at-boot state after destructive or proof-only batches, and
      add the seat to the enrolled Workstation inventory only after evidence is
      complete.
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
  unavailability for hardware that cannot be exercised. For the Surface seat,
  additionally require a native Fedora 44 RPM dependency/payload audit,
  role-provision and zero-restart service evidence, direct-DRM scanout, and a
  capability-by-capability proof ledger covering the Surface hardware matrix
  and all typed refusal/audit/recovery paths before declaring the seat
  baseline complete.
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
- Current state: Navigation owns persisted placement, full-width 48px Bottom
  geometry, fixed targets, centered and bounded pins, migration/first-boot
  selection, singular focus marking, Start/Search routing, aliases, and motion
  with focused farm coverage. Springboard is gesture-only and the App Grid is
  removed. Dark/Light material, complete workspace-identity/focus handling,
  large-overflow accessibility, and the final responsive live-render matrix
  remain open. Exact implementation and proof history is preserved in the dated
  pre-lint-compaction snapshot.
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
  7. Integrate the bounded first-boot pin selection into the remaining
     profile/persistence acceptance path and Fleet & Mesh/Workloads exposure
     without renaming the internal `Surface::InfraCode` identifier. Keep pin
     changes immediate and reject pinning Start, Back, Home, overflow, and
     placement controls.
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
### Current validation note — Mesh Teams responsive test contract

The Mesh Teams surface intentionally omits its nested app/channel rails, Details
pane, and channel header below the 1024px workspace breakpoint. The existing
headless tests rendered at 1000px while asserting desktop-only rail/Details
content, so their failures are stale test fixtures rather than a Dell runtime
regression. The Tasks body likewise now presents the specific `Channel tasks`
heading and no longer paints a redundant standalone `Tasks` label. Reconcile
those tests to their intended desktop/narrow contracts before the next farm
gate; no production readiness claim follows from this correction.

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
- Inventory freshness rollup update (2026-08-02): the inventory-first landing
  card now keeps the existing global health score as the sole health authority
  and adds a compact local-provider rollup for `fresh`, `stale`, and `awaiting`
  observations across diagnostics, services, locale, printers, backup,
  applications, and local security. Detail cards retain their per-provider age
  badges; the landing summary is only a bounded count, not a second score.
  Focused This Node tests pass 48/48 on BigBoy, including distinct-state
  freshness coverage.
- Critical-alert recovery workflow update (2026-08-02): This Node's linked
  shared AlertInbox now promotes structured affected-component, current-impact,
  and recovery-guidance fields on the inventory landing. Typed inline recovery
  actions are reachable from the same signed notification authority; safe
  actions dispatch directly and destructive actions require an explicit
  arm-and-confirm step. Acknowledgement and 15-minute snooze remain durable
  shared commands, with no second local alert store. Focused This Node tests
  pass 48/48 on BigBoy.
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
