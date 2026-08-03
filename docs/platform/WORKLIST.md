# Platform Worklist

This is the only active platform worklist. Design notes, parity ledgers,
operational runbooks, review notes, and `docs/NEEDS-OPERATOR.md` are evidence
sources, not parallel trackers.

The complete pre-rework worklist, including all progress records and historical
reconciliation text, is preserved at
`docs/worklist-archive/2026-07-28-platform-worklist-pre-rework.md`.

## Current Snapshot - 2026-08-01 mackesd architecture evaluation integration

- **12 active epics:** 12 `Remaining`, 0 `Blocked`, 0 `Needs clarification`.
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
  WL-FUNC-018 (seamless Flatpak Front Door backed by App VMs).
- **First parallel wave:** preserve/extract the old Browser stack; implement the
  DRM/mesh/VDI clipboard foundation; repair MG90 cadence and introduce the
  complete radio-health contract; finish shared visual primitives and taskbar
  authority; close open Mesh Teams contract rows; establish typed This Node
  capability adapters; and establish the universal resource identity,
  capability, discovery, and transport-adapter contracts.
- **Integration wave:** cut Construct over to `browser-vm`; finish Mesh Teams
  against the shared clipboard/theme contracts; complete the real navigation
  and offline-map cutover; complete This Node using the same state and visual
  primitives; cut Construct over to the full-width search-first taskbar; and
  make Flatpak applications appear as Front Door apps backed by App VMs; and
  cut Remote Sessions over to the primary universal resource browser and
  native session adapters.
- **Delivery policy:** build one integrated engineering-preview release from the
  twelve active epics. Publish the preview with unavailable live evidence documented;
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
- Current state: The old shell Browser engine, helper crates, worker family,
  and Browser-specific host package seams were extracted/preserved in the
  public `magic-mesh-browser-stack` repository and removed from `magic-mesh` by
  `15695dd5`; the remaining `Surface::Browser` is a thin typed guest route.
  Workloads provision `DeliveryType::DesktopVm`, while VDI supports
  RDP/VNC/SPICE and damage-aware texture uploads. The refreshed 85-path
  source/destination inventory and
  fail-closed verifier remain under `docs/design/browser-stack-extraction/` and
  `install-helpers/`. Typed `browser-provision` has the 4-vCPU/8192-MiB/64-GiB
  baseline and tests; `packaging/browser-vm/profile.env` fixes guest identity,
  provenance, Chromium/Sway ownership, transport preference, and the resource
  floor with a fail-closed verifier. The verb now admits only the stable
  workload name `browser-vm`, which is the name Surface::Browser selects; an
  alias cannot create a Browser workload invisible to the surface. Its source
  provenance now requires a pinned
  40-character commit, explicit fail-closed runtime policy, and terminal
  `failed`/`unavailable` guest states; the profile verifier passes once and the
  focused runtime contract passes 17 assertions on farm `.90`. Front Door/AccessKit expose truthful guest
  states; the image-owned validator admits only bounded identity/digest/transport
  records and rejects commands, URLs, paths, symlinks, and extra inputs. Its farm
  contract suite passes; live realization and extraction remain open. The
  extraction verifier now also rejects ignored Browser candidates and requires
  dirty manifest rows to match the current worktree blob, with clean/valid-dirty/
  stale-dirty self-tests passing on farm `.90`. Browser activation now selects
  and resumes a typed `browser-vm` route with explicit transport-unavailable
  state and no host-helper fallback; the focused live-feature and transport
  diagnostic gates each pass 1/1 on the farm. Actual VDI attachment and guest
  framebuffer realization remain open. Browser return-to-chrome now re-arms
  the typed handoff for the same stable workload; the focused resume regression
  passes on BigBoy. The guest runtime contract now requires
  bounded typed `transport-health` evidence (`connected`, `reconnecting`,
  `failed`, or `unavailable`) and rejects missing, malformed, path-shaped,
  symlinked, or extra inputs; its farm contract suite passes on `.50` slot
  `browser-contract-230`. The new `install-helpers/verify-vdi-live-proof.py`
  runner invokes the approved ignored live VNC/SPICE worker tests or the
  existing ignored RDP integration test through an explicit farm host/slot and
  records `observed` only for an exact non-empty guest `FRAME OK` marker, with
  bounded dimensions/hash and no credential or raw-log retention. RDP evidence
  also binds the existing input and tier-reconnect output while ignoring its
  separate `TIER FRAME OK` marker. Its self-test, farm Python check, and farm
  compile of the real RDP test pass on `.50` slot
  `vdi-proof-rdp-helper-20260803`; a bounded real-worker attempt against Dell
  on 2026-08-03 returned `failed` with no framebuffer marker, so actual target
  execution remains unavailable until a real guest endpoint is supplied. The
  VDI seam now records bounded frame cadence, full/partial upload counts, upload
  timing, reconnects, shell repaint counts, and best-effort host process CPU and
  DRM render-GPU busy samples; the host samples are explicitly not guest
  hardware evidence. Its focused metrics test and the 72-test VDI set pass on
  BigBoy. A fresh current-source Fedora 44 Browser VM container and qcow2
  were built and statically verified on BigBoy `.130`; the retained qcow2
  artifact has SHA-256
  `9c5d687c7fa378cb8cfe767bf0d46fb0f55a7889cf5c1eebc1bbfc8003a8c0c6`.
  The image lane now stamps the pinned profile source revision into the
  container label and refuses an image whose label does not match the profile;
  a BigBoy rootless rebuild tagged
  `localhost/magic-mesh-browser-vm-chromium:provenance-20260803` passed that
  verifier (image ID `3768fd23fbffc8cb5da5698d9729912373de83a50d9a36d2acad7088a20adaf2`).
  The guest runtime now emits a guest-owned, mode-0600 bounded
  `runtime-evidence.json` record containing transport health, VA-API status,
  and PipeWire endpoint counts; `audio_status=wired` is explicitly endpoint
  wiring evidence, not proof of audible Chromium playback/capture or recovery.
  The focused runtime contract passes locally and the refreshed BigBoy image
  tagged `localhost/magic-mesh-browser-vm-chromium:runtime-evidence-20260803`
  passes static verification (image ID
  `9c9307cbbf4940c6c825165edf492386e0c4ca811d6637bd6a36975fcddad0f0`).
  This remains guest collection/static evidence, not live GPU/audio/performance
  acceptance.
  A refreshed BigBoy image tagged
  `localhost/magic-mesh-browser-vm-chromium:media-probe-20260803` also passes
  the image contract and guest-local Chromium media probe: the fixed 64x64
  VP8/Opus fixture decoded 4 frames with zero drops. The resulting evidence is
  classified `guest_media_decode`; it is not live GPU, audible-audio, VDI, or
  reconnect acceptance.
  The OpenTofu Browser VM contract now independently rejects undersized or
  digestless direct declarations, binds SPICE to loopback, and keeps
  host-dependent virtio 3D/OpenGL disabled by default until a capability
  preflight proves the accelerated path on the target host.
  Live read-only audit on 2026-08-01
  found `.15` healthy but with no VM and no VNC/RDP/SPICE listener; Dell
  `.225` has shell services but no VDI listener, Eagle is unreachable, and
  `.138` has a host-key conflict requiring operator resolution. A fresh
  read-only jump probe through seat-15's Nebula address `10.42.0.5` also
  timed out to Dell overlay `10.42.0.4` on SSH and RDP, so seat-15 cannot
  currently provide a deployment path.
- Continuation update (2026-08-03): the checked-in cutover now includes
  explicit host-local Browser qcow2 source propagation through the cloud
  renderer, a credential-free KVM/NoCloud seat preflight, and an RDPSND client
  plus bounded PipeWire sink path that remains fail-closed until live PCM is
  observed. The cloud suite passes 236/236 and the live-connect RDP suite
  passes 88/88 on the farm; commits `7816f781` and `ef1dc5ed` are pushed.
  The extraction manifest is current at 85 paths (18 browser-owned, 25
  mixed-purpose, 42 shared). A fresh discovery still reports seat-15
  reachable but without a VM or VDI listener, and Dell `.225`, `.2`, and
  overlay `.4` unavailable; no live guest, GPU, audio, performance, or
  six-node acceptance is claimed.
- Operator-path update (2026-08-03): `8bc40ea5` adds the credential-free
  `packaging/browser-vm/deploy-image.sh` path. Its default preflight is
  read-only; only `publish --apply` can upload, back up, atomically install,
  and digest-verify the qcow2 after remote KVM/qemu-img/sudo/path checks. The
  Browser contract and extraction verifier pass; the live target inventory is
  unchanged, so no publication or live acceptance is claimed.
- Provenance/image update (2026-08-03): profile pin `f930c844` now points at
  source `9d880921`, including the guest-readable NoCloud seed-permission fix.
  A matching Fedora 44 qcow2 was rebuilt and verified on farm `.90` at
  `/home/mm/browser-vm-chromium-qcow2-provenance-20260803/qcow2/disk.qcow2`
  with SHA-256
  `089bbedf259509b60f4aa9ce3fc8582f347d3f73882dd672824499636d68d6d3`;
  `qemu-img check` found no errors. This is an intermediate 10-GiB artifact
  retained for provenance history, not a current publish candidate: the
  deployment and seat preflights now reject it. It is not live Dell evidence;
  Dell remains unreachable and no guest, GPU, audio, performance, reconnect,
  or six-node acceptance is claimed.
- Image-contract correction (2026-08-03): the prior 10-GiB qcow2 output was
  below the declared 64-GiB Browser VM workload floor. `build-image.sh` now
  binds raw/qcow2 output sizing to `BROWSER_VM_DISK_GB`, while deployment and
  seat preflight reject images below 64 GiB. The corrected farm `.90` artifact
  is `/home/mm/browser-vm-chromium-qcow2-final-64g-20260803/qcow2/disk.qcow2` with
  SHA-256
  `d41b322a658e02e7c4303a3d0580e7e702a4e39cb8e2889eb4ed2614c46a946b`;
  `qemu-img check` passed. The image carries source label `dd973836` and
  container image ID `3282faa9a795df6750cd054e13fef2a612e616e3a111b2fe15b9980f8edf081a`.
  It is still only a publish candidate because Dell and the live acceptance
  seats remain unavailable.
- Coordination-fallback hardening update (2026-08-03): the Nebula supervisor
  now records whether its peer directory came from etcd or filesystem fallback
  and refuses to shrink an existing lighthouse roster when etcd is configured
  but temporarily unreachable. This directly covers the observed self-only
  bundle regression without blocking intentional healthy-etcd removals or
  filesystem-only nodes. The focused regression passes on BigBoy `.90` in slot
  `nebula-roster-fallback-clean-20260803`; this is recovery hardening, not live
  Dell or Browser VM acceptance evidence.
- Performance-evidence update (2026-08-03):
  `install-helpers/verify-browser-vm-performance.py` now provides a
  fail-closed validator for a source/image-bound live acceptance record. A pass
  requires five concurrent 1080p tabs for at least 15 minutes, minimum 30 FPS,
  no stall over 500 ms, pointer activity, navigation/session latency, partial
  uploads, hidden repaint, and reconnect recovery. The verifier self-test and
  full Browser contract pass on farm `.50`; no qualifying Dell or seat record
  exists, so this does not claim live performance readiness.
- Live mesh blocker update (2026-08-03): a fresh read-only seat-15 audit found
  its Nebula interface up but no KVM/VM/VDI endpoint, while `mackesd` timed out
  all three etcd endpoints (`10.42.0.1/.2/.3:2379`) and repeatedly reported a
  missing `nebula-lighthouse.service` during config refresh. No live service
  or network mutation was attempted; Dell remains unreachable and the Browser
  VM cannot be published or accepted until the fleet/control-plane recovery is
  operator-resolved.
- Nebula recovery hardening update (2026-08-03): `mackesd` now treats the
  role-specific `nebula-lighthouse.service` reload as best-effort after the
  base `nebula.service` config reload succeeds. This keeps a missing optional
  lighthouse unit from leaving the base transport bundle pending while still
  emitting an operator-visible warning. The new regression passes in the farm
  `mackesd` test on `.90` (`missing_lighthouse_unit_does_not_block_base_config_acknowledgement`);
  this is a recovery fix, not live Dell or six-node acceptance evidence.
- Seat-15 recovery deployment update (2026-08-03): the Fedora 44 full
  workstation RPM was built on BigBoy `.130` in slot
  `seat15-base-f44-20260803`, passed the 90-MiB payload gate at 82.0 MiB, and
  passed a read-only RPM transaction test before installation on seat-15. The
  installed artifact SHA-256 is
  `6dcb49523fd4dfdb57e7467d0e6341c35aa67eff3937d70386c46825ddbeec67` and the
  installed `mackesd` binary SHA-256 is
  `e5564cd5bec225d85719d05ed2b6f8d37cd5969a3241ce6275541f749583668c`.
  `mackesd` and `nebula` are active, but the normal dependency-ordered start
  remains blocked by the etcd-backed `mcnf-cloud-arm-credential.service`.
  Fresh VDI discovery still reports no seat-15 endpoint and Dell unavailable;
  this is mesh recovery evidence only, not Browser VM, GPU, audio, reconnect,
  performance, or six-node acceptance.
- Mesh quorum recovery update (2026-08-03): the three reachable public
  lighthouses (`104.236.118.177`, `46.101.219.245`, and `64.23.131.57`) were
  found with self-only Nebula bundles and no usable replicated lighthouse
  directory. Root-only recovery backups were taken, the known three-entry
  public roster and peer records were restored atomically, and the tested
  `magic-mesh-lighthouse-12.1.6-1.x86_64` package was promoted across all
  three. `mackesd`, Nebula, and etcd are active on every lighthouse;
  `etcdctl endpoint health --cluster` reports all three healthy with a
  three-member quorum. Seat-15 now reaches all three overlay lighthouses and
  etcd health through Nebula `10.42.0.5`. Dell overlay `.4` still drops ping
  and times out on SSH/RDP/SPICE/VNC, and seat-15 still has no KVM or VDI
  endpoint. This clears the mesh-control-plane blocker only; Chromium VM
  publication and live guest acceptance remain open.
- Operator-path update (2026-08-01): `verify-vdi-live-proof.py discover` now
  delegates to the bounded approved-seat inventory and validates its schema;
  Python/self-tests and farm shell verification pass on `.50` slot
  `vdi-discovery-proof-241`.
- Provenance hardening update (2026-08-01): the Browser VM profile verifier
  rejects the all-zero source revision and its contract covers null provenance;
  the focused contract passes on `.50` slot `browser-runtime-provenance-241`.
- Runtime-integrity update (2026-08-01): browser guest runtime input files now
  require root ownership and reject group/other-writable or executable modes;
  insecure ownership/mode fixtures and the full browser profile contract pass
  on `.50` slot `browser-runtime-mode-232`.
- Runtime-evidence update (2026-08-03): the guest `runtime-evidence.json`
  record now has a bounded fail-closed verifier at
  `install-helpers/verify-browser-vm-runtime-evidence.py`; it rejects
  malformed, duplicate, extra, symlinked, executable, and credential-shaped
  records and labels audio as endpoint wiring only, never as audible/live
  playback or recovery. Its self-test and the Browser contract gate pass on
  farm `.50`/`.90`; source-tree profile validation now has an explicit
  `--source` mode for non-root farm checkouts while deployed admission remains
  root-only. The current live audit still finds no guest endpoint or six-node
  bundle.
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
- Current state: The registry in `worker_role.rs` asserts 88 workers and
  `spawn.rs` constructs them in one daemon process.
  `mackesd.service` already has notify/watchdog/restart policy, but its memory
  policy is unbounded and its sandbox is intentionally broad for mixed duties.
  Existing mde-bus state/action/reply topics and exact-body short-lived action
  authorization are reusable foundations.
  This Node has a stable provider-aware catalog and 16 routes, but no Advanced
  route, runtime read model, or mackesd control surface.
- Remaining work:

  1. Baseline every current worker and service dependency on Dell and the
     test nodes. Record RSS, anonymous/shared memory, cgroup current/peak,
     CPU, tasks, queues, cache sizes, retry rates, startup time, and provider
     availability. Capture the boot allocation anomaly before refactoring.
  2. Expand `WorkerSpec` into the single runtime contract: group, criticality,
     capability predicate, cadence, queue/cache bounds, resource budget,
     restart policy, health key, state topic, action namespace, and cleanup
     ownership. Add drift tests proving every spawned worker is registered and
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
  The clipboard composer now uses the canonical text-size bound, preserves
  meaningful leading/trailing whitespace, rejects blank/invalid attribution
  without queueing, and retains rejected drafts; 11 focused tests pass on farm
  `.50`.
- Task convergence update (2026-08-01): completion authored by a departed
  member is covered by an offline convergence regression; the focused farm
  gate passes 1 test with 87 filtered on `.50` slot `func011-task-departed-241`.
- Membership authorization update (2026-08-01): cross-member removal events
  now require the target member or a present owner in both the domain and
  SQLite projection paths; the focused farm gate passes 1 test with 88
  filtered on `.90` slot `func011-membership-removal-243`.
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
     - Bug 7 implementation slice (2026-08-02): reconcile the existing Files
       node-action prototype with real backend/provider truth. Each of the ten
       interactions must dispatch an existing typed Files/Transfers action or
       remain visibly unavailable; node rows must expose per-node sync state;
       link/share and upload destinations must be populated only from live
       advertisements; and the slice must have focused model/view tests plus
       Dell `.138` Dark/Light desktop, narrow, and large-text render evidence.
       Focused farm result: `mde-files-egui --lib` passes 168/168, including
       the ten-action render inventory. Live `.138` proof now passes Dark
       desktop, Dark narrow, Light desktop, and Light/Largest with exact
       payload `5ea5b72594482d8e4158e136b09d1c44407c3a68c85d8c97a17e3f7bd3794e28`;
       provider-advertised Airsonic validation remains open, and this is not a
       readiness claim.
       Live `.138` finding (2026-08-02): the first Dark desktop frame rejected
       the slice because the compact sidebar painted the ten action labels as
       single initials (`B…`, `S…`, etc.). Fix the row geometry so labels and
       state remain readable beside their action controls, then recapture all
       required profiles.
       A second `.138` frame on payload `d284b002a23dc3a1a5db3574fafb651c94ec7403154a133f706d8a9d4b62c935`
       made the verbs readable but exposed excessive button/state wrapping;
       the final action-lane tightening is farm-tested but not yet live-captured.
       The next release gate also exposed a pre-existing `ThisNodeState::health`
       compile error (`self.health_dashboard()`; the provider lives under
       `self.status`). Correct that scoped blocker before producing the final
       validation payload.
       Final payload `5ea5b72594482d8e4158e136b09d1c44407c3a68c85d8c97a17e3f7bd3794e28`
       live result: all four `.138` Dark/Light desktop, narrow, and
       Light/Largest profiles passed direct-DRM visual inspection. The shared
       title allocation reserves a fractional-pixel trailing margin so the
       complete `FILES` identity remains visible at 800 logical pixels.
       Evidence is recorded in `WL-UX-009-DRM-EVIDENCE-2026-07-31.md`; this
       Files slice is validated while WL-UX-009 remains Remaining.
       - Bug 7 review finding (2026-08-02): the ten node-action rows and
         per-peer sync labels are present and farm-tested, but
         `build_targets()` still unconditionally adds `Music Library` and
         `node_actions_section()` therefore presents an Airsonic upload route
         without a live service advertisement. Keep the provider-advertised
         Airsonic destination open; wire the existing service-advertisement
         source into `Peer`/`FileBrowser` before claiming uploader readiness.
       - Bug 7 implementation (2026-08-02): Files now reads the shared
         `state/services/*` records through the Bus and exposes Music Library
         only for a probe-confirmed `Up` Airsonic/Navidrome service. Mesh Share
         and reachable-node destinations remain prepopulated; offline/demo
         backends do not fabricate an uploader. `FileBrowser` refreshes this
         capability with the roster, and the focused farm suite passes 168/168.
         Direct-DRM provider-advertisement proof remains open, so this does not
         close the slice or claim readiness.
       - Live provider check (2026-08-02): `.138` has a probe-confirmed
         Airsonic endpoint at `10.42.0.7:4040` (`health=up`); Dell has no
         confirmed Airsonic record. A feature-correct `.138` capture was
         rejected because the explicit Files proof route produced a Music
         frame. The candidate was removed and `.138` restored to the prior
         payload with the proof override removed; keep the live Files render
         and uploader acceptance open.
       - Proof-route hardening (2026-08-02): the shell now reasserts an
         explicitly requested DRM proof surface at each render frame, preventing
         startup navigation/tray events from contaminating a capture. The
         focused shell proof-route dependency suite passes 16/16 on BigBoy;
         a feature-correct live recapture is still required.
       - Route defect remains reproducible (2026-08-02): after the candidate
         recapture on `.138` under explicit `Dark / Construct / Default / Normal`
         state, `MDE_DRM_PROOF_SURFACE=files` still produced the Auto/clock
         frame. The seat was restored to payload `20955383…`, the proof drop-in
         was removed, and `NRestarts=0` verified. Do not accept the Files live
         render or uploader proof until this route is corrected.
       - Current-payload Terminal Light/Largest desktop review (2026-08-02):
         the initial scaled inspection appeared to show `TE…`, but original-
         resolution inspection and OCR both confirm the complete `TERMINAL`
         identity. No source change was warranted; the reviewed Dell frame is
         accepted as evidence.
     - Current-payload resolution (2026-08-02): the shared title allocation now
       reserves a ceil-rounded galley width plus a trailing margin. Exact
       release `911e781820822cb0be5bb18251147bf34c33f24047c936c5183222ed1e7081c8`
       was installed on `.138`; all four Terminal and all four Editor Dark/Light
       desktop, narrow, and Light/Largest frames passed direct-DRM visual
       inspection. `TERMINAL` remains complete at 800 logical pixels, Editor
       sidebars remain collapsed, and the secure seat restored active/zero
       restarts. Evidence is in the DRM ledger; `.225`, remaining matrix cells,
       and WL-UX-009 readiness remain open.
       - Current-payload Terminal matrix resolution (2026-08-02): exact
         release `20955383f21c4e2a4aee6a519be24905a2c65b2605f52ad3f515dcf8f2b9ae5a`
         was inspected on both `.138` and Dell `.225` in Dark desktop, Dark
         narrow (`800`), Light/Largest desktop, and Light/Largest narrow
         profiles. All eight frames preserve the complete `TERMINAL` identity,
         menu/session controls, body, and footer chrome. Evidence and hashes are
         in the DRM ledger; both seats restored secure login-at-boot, active
         service, and zero restarts. This closes only the current Terminal
         render slice and leaves WL-UX-009 readiness open.
     - Governance finding (2026-08-02): the style-leak gate reports one raw
       `on_hover_text` in the This Node section tree, even though the row also
       uses the shared hover-card helper. Remove the raw egui tooltip and keep
       the combined provider/health explanation in the themed shared tooltip.
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
- Current state: Canonical `event/clipboard/clip` and action verbs exist. DRM
  Copy/Cut/Paste, bounded `CopyText`, shell materialization, VNC state,
  RDP/SPICE capability reporting, KDC/mobile authorization, bounds,
  deduplication, and attribution have focused coverage. Local publishing is
  session-scoped opt-in in Mesh Teams Settings; daemon history has a durable
  cursor and retry-safe acknowledgement. Signed VDI events authorize the exact
  target seat and leave failed writes retryable. KDC has 31 focused tests; VNC
  has a verified ClientCutText/ServerCutText round trip; the shell adapter has
  58 VDI tests. The daemon bridge validates serving peer and target seat with
  source attribution, deduplication, and retryability (28 bridge + 44 sync
  tests). The production adapter now consumes a bounded Bus handoff for
  guest-to-client materialization at the exact signed seat, while client-to-
  guest remains explicitly gated behind ownership of the live VDI protocol
  session; bridge and sync focused suites pass on farm `.130`/`.90`. RDP
  CLIPRDR/SPICE vdagent remain typed unsupported and gated. The shell adapter
  now scans a bounded materialization tail for the exact target seat, exposes
  retryable unavailable state, retains authorized handoffs across repeated
  reads, and keeps local publication opt-in off by default; 16 focused
  communications tests pass on farm `.90` slot `func016-shell-adapter-159`.
  The live KDC worker now owns `ClipboardPlugin`, admits paired clipboard and
  `.connect` packets, drains active-seat materializations, and publishes the
  canonical `state/clipboard/materialize` Bus handoff with peer/packet
  attribution. Direct and fanout paths no longer invoke `wl-copy`; unavailable
  Bus writes remain queued for retry. The plugin suite passes 31 tests on farm
  `.50`; the daemon `kdc_host` gate passes 70 tests on BigBoy, including a
  direct plugin-to-Bus assertion for exact seat, whitespace, and source/packet
  attribution plus honest fanout application status when the Bus is unavailable.
  Mesh Teams clipboard UI/action routing now gates local publication behind the
  session opt-in while keeping remote history visible, preserves exact bounded
  text/hash/source data, retains rejected drafts, and emits typed attach,
  pin/unpin, and delete commands; its focused UI gate passes 15 tests plus one
  action-command regression on farm `.50` slot `func016-collab-ui-207`. The
  shell Communications mount now renders retryable target-seat materialization
  failures visibly in Clipboard mode; its focused communications gate passes
  17 tests on farm `.50` slot `func016-shell-notice-209`.
- VDI ingress hardening update (2026-08-01): Mesh Teams now rejects unsupported
  or malformed VDI clipboard capability reports before command materialization,
  while explicitly bidirectional text remains bounded and admissible; 17
  focused tests pass on `.50` slot `func016-collab-vdi-cap-240`.
- RDP capability update (2026-08-01): the typed RDP clipboard adapter now
  exposes an explicit unsupported status and latches failed legacy reads; the
  focused farm gate passes 1 test with 71 filtered on `.90` slot
  `rdp-clipboard-224`. IronRDP's pinned release still has no CLIPRDR
  implementation, so real RDP clipboard transport remains intentionally open
  rather than falsely reporting success.
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
- Current state: `mackesd` still assumes one managed MG90/direct Ethernet; the live unit reports LTE/WAN and power but no GNSS fix, and OBD parsing is absent.
  Operator update (2026-08-02): a real MG90 and its management credentials are
  confirmed available for live validation. Credentials remain out-of-band and
  must be provisioned only into the root-owned files described by
  `docs/ops/mg90-access.md`; they are not recorded here. This reclassifies the
  live-adapter work from hardware-unavailable to access/proof-ready, but does
  not claim a fresh probe, 30-minute cadence run, or production evidence until
  the pinned host is reached from the selected workstation and the sanitized
  capture is attached.
  Live handoff update (2026-08-02): `.15` is actively publishing a direct-gateway
  v2 mirror for ESN `ND84720078011035` (firmware `4.3.0.1`) with fresh 2-second
  and 5-second records, LTE A active, LTE B standby, GNSS acquiring/no-fix,
  reported power/ignition, and explicit OBD `not_installed`. The complete
  control-plane inventory and sanitized next-AI instructions are in
  [`docs/ops/mg90-live-handoff-2026-08-02.md`](../ops/mg90-live-handoff-2026-08-02.md).
  The unit is online and platform-owned. Credential reconciliation on `.15`
  now passes the pinned SSH probe, MG-LCI general/WAN reads, and all five
  authenticated CherryPy application surfaces; `mackesd` is enabled for
  `/obdii_status/` and receives the response, but correctly reports the payload
  as unsupported until its schema is verified. The detailed credential-safe
  record is in the linked live handoff.
  Raster MBTiles/FTS5 serves one region; Maps owns basemap/search, while Valhalla, route/lane/speed helpers, bearing, and admin actions remain unwired.
  Typed v2 identity/radio/freshness/provenance dual-publishes beside v1; Maps/Car project bounded radio and mirror states, with 13 consumer tests passing.
  The MG90/manager roster has independent poll/heartbeat plans, latest-wins/no-source behavior, and 44 vehicle tests pass. Cached observations heartbeat every
  two seconds without resetting v2 age; the reusable default roster plan carries
  that independent heartbeat as well. Maps HUD/Airspace expose honest unavailable/stale states; route preview has 7 focused tests at `05ba6870`.
  Navigation start rejects malformed or stale selections without mutation; the fresh farm full-crate Maps suite passes 264 tests with no failures. Live adapters
  and route/render cutover remain. Multi-device roster admission now rejects
  unsupported `VehicleStateV2` schema versions before identity acceptance, and
  the vehicle-worker suite passes 76 tests on farm `.50`. The Maps consumer now
  folds a bounded multi-manager MG90 roster with identity/schema/manager-set
  admission, latest-wins selection, retained typed cache, and explicit
  `ResyncingNoFreshSnapshot` when a refresh has no valid fresh row; the full
  Maps suite passes 268 tests on farm `.130`.
  The typed telemetry boundary now distinguishes `NotInstalled`, `Unsupported`,
  and `Failed` OBD/HDOBD probe outcomes, preserves gateway observations, and
  never treats an unverified payload as typed vehicle telemetry; the mesh-types
  focused gate passes 14 tests on farm `.50`, and the daemon vehicle gate passes
  77 vehicle-focused tests with zero failures on farm `.90` slot
  `func017-worker-208`. Offline-region manifests now validate bounded revision,
  artifact identity, size, digest, and traversal safety before atomic activation;
  the focused Maps gate passes 3 tests with 269 filtered on `.90` slot
  `maps-manifest-225`.
- Radio-inventory update (2026-08-01): the v2 inventory now rejects duplicate
  identities and exposes an explicit six-position native layout without
  converting missing rows into installed hardware. The focused mesh-types
  test passes on `.50` slot `func017-radio-slots-233`; direct live adapter and
  multi-manager rollout evidence remain open.
- Vehicle-worker gate update (2026-08-01): the current identity-addressed,
  schema-gated manager route passes 80 vehicle-focused tests with 4,238
  filtered and zero failures on `.90` slot `func017-vehicle-current-243`.
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
  `sha256:4bd2...faa3b5`; full provision/guest/VDI convergence remains open. Front Door now bounds versioned Flatpak favorites, explains requested guest permissions, and presents explicit installing/placement/start/reconnect/unavailable states with disabled primary actions; five focused peer-app presentation/favorite tests pass on farm `.50` slot 156. The daemon launch seam now rejects guest-owned Flatpaks at the legacy host launcher and maps fully identified guest requests to typed `OpenApp` session requests without command/path fallback; 12 focused daemon tests and 33 focused mesh-type tests pass on farm `.50`. The App VM image contract requires a complete immutable `sha256:` base-image digest; image provenance self-tests and the full App VM contract suite pass on farm `.90`. Catalog admission now rejects capabilities outside the supported App VM allow-list before a row can be launchable; four catalog policy tests pass on farm `.90` slot `func018-catalog-policy-192`. The immutable guest validator now applies the same reverse-DNS Flatpak identity rule at image runtime, rejecting single-component and empty-component IDs; the App VM image contract suite passes on farm `.50` slot `func018-guest-identity-214`. The session broker now rejects terminal stale/replayed AppState evidence atomically; its focused farm gate passes 1 test with 4,307 filtered on `.90` slot `appvm-terminal-evidence-214`. The shell App VM rail now suppresses focus/handoff for failed, unavailable, reconnecting, and not-ready states while keeping paused sessions resumable; 8 focused tests pass on `.50` slot `appvm-session-rail-214`. App VM runtime evidence and AppState now carry a backward-compatible monotonic `generation` field; legacy zero-generation JSON still decodes, and the wire slice passes 8 tests on `.50` slot `appvm-generation-wire-215`. The broker integrates strict ordering for numbered evidence while preserving legacy zero-generation behavior; its combined session-broker gate passes 40 tests with 4,270 filtered on `.90` slot `appvm-generation-broker-216`.
- Evidence update (2026-08-01): the generation ordering broker gate now passes
  41 tests with 4,270 filtered on `.90` slot `appvm-generation-broker-218`,
  including atomic rejection of replayed same-identity generations.
- Runtime-admission update (2026-08-01): App VM evidence lookup now selects
  the newest valid record for the exact session/app identity, so unrelated
  session heartbeats cannot block a resume while stale, malformed, and
  terminal matching evidence remain fail-closed; the focused farm gate passes
  9 tests with 4,303 filtered on `.50` slot `appvm-runtime-admission-221`.
- Front Door update (2026-08-01): peer catalog discovery now preserves signed
  permission/lifecycle metadata through bounded retries and reconnecting state,
  rejects cross-node replies, and keeps unsigned/not-installed rows visibly
  unavailable; the focused farm gate passes 11 tests with 1,932 filtered on
  `.90`.
- Guest publisher update (2026-08-01): the App VM cloud-init and image-owned
  launcher now persist a bounded durable generation counter and include it in
  every runtime evidence publication; shell syntax and the full App VM contract
  suite pass on `.90` slot `appvm-generation-guest-223`.
- Terminal evidence update (2026-08-01): the App VM contract suite now exercises
  connected, reconnecting, failed, and unavailable guest-runtime evidence with
  monotonic-generation and replay rejection plus command/path/host-fallback
  refusal; the farm verifier passes on `.90` slot `app-contract-230`.
- Lifecycle hardening update (2026-08-01): the guest launcher now traps
  TERM/INT/HUP, terminates and waits for its Flatpak child, publishes terminal
  failed evidence, and exits the guest compositor to prevent stale VDI paint;
  the contract suite passes on `.50` slot `app-contract-232`.
- Live-seat audit (2026-08-01): `.15` is healthy but has no VM or VDI listener;
  `.225` has no App VM role or VDI listener, so guest realization remains
  unproven.
- Identity-continuity update (2026-08-01): connected, reconnecting, unavailable,
  and terminal App VM evidence is now bound to the admitted session/VM/app
  tuple; the farm contract suite passes on `.50` slot `func018-app-identity-243`.
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
- Current state: `desktop_sources` publishes mesh and mDNS desktop sources for
  RDP, VNC, and SPICE; the shell chooser and VDI layer know those three types.
  `media_sources` publishes a separate media topic with mesh, gateway, and
  Jellyfin mDNS lanes, and `mde-jellyfin` already provides a client/store.
  Peer descriptors expose SSH/RDP/VNC and fixed media probes, while remote
  proofing can manage a user-scoped Sunshine service. There is no shared
  resource identity, capability admission registry, SSDP/UPnP lane, SSH/X11
  session model, Moonlight client, Subsonic adapter, or universal primary
  landing surface; the four-seat Sunshine rollout is not yet live evidence.
- Remaining work:

  1. Define a versioned shared `ResourceIdentity`, `ResourceCard`,
     `TransportCandidate`, `ClientCapability`, `AuthState`, `HealthState`,
     `SourceProvenance`, and bounded action model in mesh types. Publish a
     canonical resource topic (for example `state/resources/catalog`) with
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
  schema-4 source/artifact/SBOM/gate/topology/VDI bindings; `sign-release.sh --evidence`
  publishes signed provenance and checksums. `ci-gate.sh verify` rejects
  non-green, unobserved, stale, or unbound farm results and requires matching
  `github-required=pass`. A fail-closed six-node verifier requires exactly three
  lighthouses, three workstations, fresh artifact-bound recovery evidence, and
  `--require-live`; live sources now also require node-bound, digest-verified
  SSH/console attestation; no real bundle exists. Farm `.90` slot 118 passes both
  `ci-gate.sh --self-test` and `release-evidence.sh --self-test`. The workflow
  now declares a labeled `mcnf-farm` backend plus a digest-verifying
  `github-required` check; runner registration, six-node/live gates, and
  operator-key publication remain open. `release-evidence.sh` is now schema 4:
  any production-pass envelope must include a digest-bound, verifier-produced
  six-node topology descriptor with `live_required=true`; validation reruns the
  live verifier before accepting the envelope and requires the topology
  revision to equal the release source commit. Preview envelopes may carry
  farm-only topology evidence; production envelopes cannot. Production
  envelopes now additionally require source-bound observed VDI framebuffer
  evidence validated by `verify-vdi-live-proof.py`; its release integration
  self-test passes locally and on farm `.50` slots `crit006-vdi-release-236`
  and `crit006-vdi-proof-237`. Farm self-test
  passes, but no real live bundle exists yet. `ci-gate.sh verify` also requires
  a completed green summary with all stage pass records, matching test counts,
  job/slot identity, and an unchanged gate-log digest. The gate now binds the
  status artifact's job ID, build host, and slot to the exact gate-log header,
  rejects control-character identities, and covers mismatched-slot tampering in
  its farm self-test on `.90`. The CI evidence self-test now also rejects
  incomplete green logs, identity mismatches, symlinks, path escapes, and log
  tampering; it passes on farm `.90` slot `crit006-gate-selftest-2`. No live
  GitHub Actions artifact upload/download or branch-protection publication has
  been exercised from this workspace. The six-node verifier now requires
  `healthy → degraded → recovering → healthy`, automatic lighthouse failover,
  node-bound digest-verified attestations, and corrected-forward re-enrollment
  with rollback disabled; its farm self-test passes 2 positive and 14 negative
  cases on `.90` slot `crit006-topology-recovery-209`. The verifier also rejects
  cross-node reuse of one evidence artifact while allowing related claims for
  the same node to share a capture; the expanded self-test passes 2 positive and
  15 negative cases on `.90` slot `crit006-topology-artifact-binding-212`. No
  real six-node bundle exists yet. Live-attestation records now require an
  exact node/transport/revision/timestamp/source-bound JSON marker; relabeled
  farm markers are rejected. The verifier self-test passes 2 positive and 16
  negative cases on `.50` slot `topology-verifier-234`. The stale
  Six-node evidence artifacts are now bounded to 8 MiB before digest reads;
  the expanded verifier self-test passes 2 positive and 17 negative cases on
  `.50` slot `crit006-artifact-bound-242`. The stale
  release-evidence self-test fixture was corrected to use
  the short-SHA gate-log header binding; the full determinism/fail-closed suite
  now passes on farm `.90` slot `crit006-release-fixture-211`. Release evidence
  now rejects symlinked artifacts, manifests, and evidence envelopes; the
  expanded self-test passes on farm `.50` slot `release-evidence-selftest-232`.
  The GitHub farm artifact now includes a byte-size/SHA-256 manifest, and the
  downstream `github-required` verifier checks its revision, job/slot identity,
  symlink safety, and downloaded-file digests before accepting the farm result.
  CI gate hardening now rejects digest-valid green logs containing contradictory
  failed-stage records and requires exactly one green summary; the focused
  self-test passes on `.90` slot `crit006-ci-gate-242`.
  YAML/shell checks and `git diff --check` pass; live GitHub publication remains
  unexercised. Live read-only proof-seat audit on 2026-08-01 found no VDI
  endpoint on `.15` or `.225`; Eagle was unreachable and `.138` requires
  host-key conflict resolution, so no observed production VDI or six-node
  evidence is claimed.
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
- Current state: Live recovery on 2026-08-02 restored the current three-member
  lighthouse quorum (`104.236.118.177`, `46.101.219.245`, and
  `64.23.131.57`), repaired missing cross-lighthouse maps, re-enrolled seat 15
  as `10.42.0.5`, and repaired Dell `.225` as `10.42.0.4`. The watchdog now
  recognizes both legacy and `identity/current` certificates, and
  `mackesd.service` is ordered after Nebula. Live boot-recovery passed the
  mesh, file-plane, etcd, and bus checks at that time; a physical
  suspend/resume proof and six-node corrected-forward evidence bundle are
  still missing.
- Current regression audit (2026-08-03): the earlier Dell-online state is no
  longer live. The three lighthouse records and seat-15 (`10.42.0.5`) are
  healthy in the restored etcd directory, but Dell has no current peer record;
  overlay `.4`, LAN `.225`, and direct `.2` are unreachable. Seat-15 still
  lacks `/dev/kvm` and any VDI listener. Treat the earlier Dell repair as
  historical until the machine reappears and passes a fresh boot/recovery
  proof.
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
- Current state: Shared Quazar primitives and most Construct-owned route migrations
  are landed; Workbench, System, Timers, and Phones now consume `AppFrame`. Farm suites
  cover the shell and major workspaces. Direct-DRM evidence and hashes are in
  [`WL-UX-009-DRM-EVIDENCE-2026-07-31.md`](WL-UX-009-DRM-EVIDENCE-2026-07-31.md).
  `.138` has Editor Dark desktop/narrow and Light/Largest proof, plus current-payload
  This Node/About Dark, narrow, and Light/Largest proof. Dell has clean
  Editor/System/Bookmarks/Storage/Files/Mesh Teams/Maps/Phones/Timers Dark,
  narrow, and Light/Largest proof with `NRestarts=0`; Dell's current installed
  payload is `703d3477ce574e30abda5f1d7bdc46834ad16881db8d3a1d727b794fc1222919`.
  The full RPM
  dependency transaction, `.15`, remaining routes/states, full matrix, and
  production readiness remain open. The latest installed validation payload is
  `e37ec5c0df9ac05133c073f481823b5642a9fef85dd0237502f82abb2a9d13dd` on
  `.138`, containing the This Node, Explorer, and Mesh Teams frame migrations
  plus the Files node-action surface. Dell `.225`
  was unreachable during this deployment, so no Dell synchronized-adoption or
  current-payload visual claim is made. Earlier `bbe6a88b…` rows remain
  accepted historical evidence for the Maps/This Node slices they name.
  The earlier `f58b42ba...`, `bccc6e94...` and
  `703d3477...` hashes remain the basis of already accepted representative
  screenshots.
  Current navigation/tray, Files node-integration, and Maps responsive fixes
  were rebuilt as release shell payload
  `bccc6e94dd7dc32427f6b809e92b36fb0c06591fb1af2dd58a18dc399148a3e2` on
  BigBoy with `drm,live-vdi,media-mpv`, then installed directly on Dell
  `.225` and `.138`; both services reported `active` with `NRestarts=0` and
  the installed hashes matched the artifact. Current `.138` proof-only EGL
  readback frames for This Node Dark, This Node Light/Largest narrow, and
  Terminal Dark were written and visually inspected; their hashes are
  `aae809d959f81f9049385c4840ee6dd6fedb4cbaa8c2eceb8a9ae72a90713d5e`,
  `77f336a7e8978cf5c866ca2ff8d95efc7259b8d81745e810801f3848f800c1ac`, and
  `9674a818f45c5b97f18f6fd71d1788d444f60520baf63b84726092b66a67d35f`.
  The empty Desktop/Remote Sessions frame is intentionally not accepted as
  tray evidence because this seat has no live remote session. Proof drop-ins,
  appearance overrides, and boot-curtain overrides were removed after capture;
  `.138` returned to Dark/Default and `require_login_at_boot:true`.
  The Maps Drive HUD now reserves the painter-positioned FAB lane before laying
  out the radio/GNSS health rail and selects a fixed multi-row grid for
  large/largest text; 267 focused Maps tests pass on farm `.90`.
  The current Dell proof slice initially appeared to show retained boot-splash
  pixels in the first proof frame. Direct comparison against the live
  `CONSTRUCT-WALLPAPER1.png` asset remains the Construct/Software Studio/
  Release boot-splash/brand asset, not a selectable wallpaper choice or stale
  scanout; Bing daily picture is the sole supported desktop wallpaper provider.
  The chooser route and current empty-state copy render above it. Keep the proof-only direct-DRM
  handoff path under validation, but do not classify the wallpaper lockup as a
  handoff failure.
  The current dirty validation tree also requires a small compile-integrity repair:
  the `SessionRequest::AppState` matcher must explicitly ignore its newer
  generation field before farm shell tests can run.
  Dell Light/Largest live proof also exposed a palette-resolution defect in the
  wallpaper-backed empty state: Light text was rendered over a backing derived
  from the dark `BG` token, reducing contrast. The backing must resolve through
  the active palette before alpha is applied.
  After resolving palette contrast, the same Light/Largest frame shows the
  large-text status anchor is too low for wallpapers whose lower brand lockup
  occupies the bottom band. Move that anchor into the clear band below the hero
  mark and above the lockup, then recapture the live frame.
  The earlier style-leak finding at `mde-collab-egui/src/lib.rs:914` is resolved
  through the shared tooltip primitive; the style-leak gate is now zero.
  The earlier Device Manager/This Node duplicate title-strip finding is resolved;
  the shared MenuBar remains and the redundant strip is gone, with 81 focused
  Device Manager tests passing.
  The proof-only path now requests `Modifier::Linear`, records the actual locked
  front-buffer object's format/modifier/stride, and fails closed when the driver
  returns a non-linear BO; the production allocation path remains unchanged.
  The earlier dirty Maps compile-integrity finding is resolved with no-behavior-
  change type repairs; the current release payload builds and the focused Maps
  suite remains farm-verified.
  The proof-only DRM telemetry/fail-closed change and Maps compile-integrity
  repairs build as release payload
  `f69e42551724fae346b91bb4965194905d6583f62cf4265394f0fa49a8a8fc7e`.
  This exact payload is installed on Dell and `.138`; both services are active
  with `NRestarts=0`.
  Live Maps Light/Largest and narrow proof for this new payload is still
  required before accepting the fix as validated.
  The current direct-DRM proof captures XR30 as native `x2rgb10le` before PNG
  conversion. On `.138`, the proof allocator rejected linear allocation and
  the actual locked front buffer reported `I915_x_tiled` with stride `7680`;
  the fail-closed guard rejected the run before any screenshot could be
  accepted. Current `.138` Dark desktop, Dark narrow, and Light/Largest About frames were
  visually inspected. Both the proof drop-in and temporary appearance settings
  were removed afterward; `.138` is back on `require_login_at_boot:true` with
  `NRestarts=0`.
  The current `.138` Browser boundary proof also renders the Construct-owned
  connection/unavailable state and preserves the guest boundary; the frame
  contains no claimed Chromium/page pixels. Its inspected PNG and hash are in
  the DRM evidence ledger. The proof seat was restored to secure runtime after
  capture.
  The current `.138` Explorer/Mesh-lens proof shows the shared workspace title
  and menu above Explorer's domain mode/filter controls, with the discovered
  unit card and taskbar separated; the inspected PNG and hash are in the DRM
  evidence ledger. The proof seat was restored to secure runtime afterward.
  The current `.138` Music proof renders the shared MUSIC menu/status chrome and
  an honest unavailable state because no Airsonic credentials are installed;
  this is visual/state evidence only and does not claim that the Subsonic server
  at `172.20.0.2` is connected or listed.
  The current `.138` Media Sources proof renders the shared MEDIA menu, source
  tabs, local-capture/Jellyfin controls, and honest empty-source state without
  overlap; it does not claim a live media backend or Jellyfin source.
  The current `.138` Files proof covers Dark desktop, Light/Largest, and Dark
  narrow on the same release payload; the dense sidebar/list/preview/status
  geometry remains bounded in all three inspected frames. The focused Files
  suite passes 165 tests.
  The first current-payload `.138` Workloads Dark desktop readback was invalid:
  it contained only a dark canvas, the proof readback marker, and the Construct
  watermark, with no WORKLOADS frame or honest route state. Do not accept that
  PNG as visual evidence. Re-run the proof against the canonical
  `Surface::InfraCode` route (`iac`/`infra-code`) with the settle window, then
  visually inspect the replacement before closing the Workloads matrix cell.
  The current `.138` Maps proof found two render-validation failures before
  evidence could be accepted: Light/Largest remapped the dark map canvas text
  to dark ink, and the first 800-logical-width capture was effectively blank.
  Keep Maps open until the canvas contrast is corrected and a fresh narrow DRM
  frame is captured and visually inspected; do not add those failed frames as
  passing evidence. The replacement-payload attempt on both `.138` and Dell
  then captured the direct DRM plane with Intel X-tiled modifier
  `0x100000000000001`; the converted PNGs showed scanline striping and are also
  invalid proof. Both seats were restored to secure runtime with the new binary
  installed, but no replacement Maps frame is accepted yet.
  The next validation task is a proof-compatible direct-DRM capture path for
  this Intel driver (for example, a verified detile/readback route); until that
  is implemented and visually inspected, Maps Light/Largest and narrow proof
  remain open. This is diagnostic hardening, not a production scanout-policy
  change.
  Record the next proof implementation before source changes: when
  `MDE_DRM_PROOF_READBACK` is set, read the rendered EGL framebuffer with
  `glReadPixels` into a CPU-linear RGBA buffer, emit a PNG plus dimensions,
  format, and source metadata, and retain the actual direct-DRM/KMS frame as
  the runtime surface. This proof-only readback must not alter normal scanout,
  must reject incomplete/blank output, and must be visually inspected before
  any Maps Light/Largest or narrow row is accepted.
  The first EGL-readback Light/Largest frame is now valid pixel evidence but
  remains a failed visual proof: Maps' intentionally dark HUD cards inherited
  Light-mode dark text from the shared remapper. The follow-up Maps content
  palette change uses explicit high-contrast map-content text tokens for HUD,
  health rail, alert, speedometer, and FAB labels while leaving shell/sidebar
  Light palette tokens unchanged. Those content tokens must remain distinct
  from shared static token values so the global Light remapper cannot rewrite
  them; it requires a fresh farm build and visual recapture before acceptance.
  The follow-up release payload
  `36bf4864dbae392b3153b2ebf120c5acd09ad05ee58890be5a02cf7659250766` is now
  installed on Dell and `.138` with active services and `NRestarts=0`. Direct
  EGL readback produced visually inspected, non-striped Maps Dark desktop,
  Light/Largest desktop, Dark narrow, and Light/Largest narrow frames; their
  hashes are in the DRM evidence ledger. The responsive banner fix derives
  safe height/text geometry from the live zoom and available width, and the
  `.138` Light/Largest narrow frame was visually inspected and accepted. `.138`
  was restored to `require_login_at_boot:true` after capture. The broader
  workspace matrix and remaining Construct-owned routes remain open.
  The proof-only settle-window follow-up payload
  `9cdc8f1f2f340b493e341c424a0d5c86521179ea4fa15521cc637cfce04b0488` was
  built on farm `.90` (`wl-ux-009-proof-settle-20260801`) and deployed to Dell
  and `.138`; both services reported active with `NRestarts=0`. A 2.5-second
  settled Mesh Teams frame and a 5-second settled Music unavailable frame were
  visually inspected and added to the evidence ledger. The settle window is
  proof-only; normal production timing is unchanged. `.138` was restored to
  `require_login_at_boot:true` after each capture.
  The follow-up Light/Largest narrow slice on the same payload also passed
  visual inspection for Mesh Teams, Music, Terminal, and This Node. These are
  bounded 800-logical-width frames with explicit unavailable/idle states where
  the seat lacks live backend data; their hashes are recorded in the evidence
  ledger. This expands the matrix but does not close the remaining desktop,
  narrow, and large-text coverage for every Construct-owned route.
- Live-render resolution (2026-08-01): current payload `bccc6e94…` Music
  unavailable Dark desktop and Light/Largest narrow frames were visually
  accepted with hashes `79378b49…` and `4521d334…`. The frames show the
  shared MUSIC chrome and honest missing-Airsonic-credentials state; they do
  not claim connectivity to the Subsonic server at `172.20.0.2`.
- Live-render resolution (2026-08-01): current payload `bccc6e94…` Media
  Sources Dark desktop and Light/Largest narrow frames were visually accepted
  with hashes `8baeb270…` and `3d27686e…`. Source tabs, local-capture/Jellyfin
  controls, and the honest empty-source state remain bounded above the taskbar.
  The proof seat was restored to secure login-at-boot with an active service and
  zero restarts; no live media backend or Jellyfin source is claimed.
- Live-render resolution (2026-08-01): current payload `bccc6e94…` Terminal
  Dark desktop and Light/Largest narrow frames were visually accepted with
  hashes `f8372346…` and `1ff9a316…`; the terminal menu, tabs, mesh overview,
  prompt, and taskbar remain readable and separated.
- Live-render resolution (2026-08-01): current payload `bccc6e94…` This Node
  System Dark desktop and Light/Largest narrow frames were visually accepted
  with hashes `dad7a71f…` and `293107bf…`; unified node navigation, display
  controls, health rail, and taskbar remain readable and bounded. These are
  visual/state proofs only and do not claim live hardware reconfiguration.
- Live-render resolution (2026-08-01): current payload `bccc6e94…` Editor
  Dark desktop and Light/Largest narrow frames were visually accepted with
  hashes `755ce34d…` and `32608697…`; nested communications/editor chrome,
  formatting controls, direct-entry collapsed sidebars, document body, and
  details/status regions remain bounded.
- Live-render resolution (2026-08-01): current payload `bccc6e94…` Phones
  Dark desktop and Light/Largest narrow frames were visually accepted with
  hashes `911b8ffc…` and `e22d3eb5…`; the shared title, pairing status, tabs,
  feature card, and remote-input controls remain readable without clipping.
- Live-render resolution (2026-08-01): current payload `bccc6e94…` Car Dark
  desktop and Light/Largest narrow frames were visually accepted with hashes
  `b068e661…` and `175befca…`. The Auto Mode cockpit, zoom-aware instrument
  grid, width-safe telemetry elision, cards, app strip, and taskbar remain
  bounded.
- Live-render resolution (2026-08-01): current payload `bccc6e94…` Browser
  boundary Dark desktop and Light/Largest narrow frames were visually accepted
  with hashes `736a3bea…` and `982786b7…`. The frames contain only
  Construct-owned VM selection, transport guidance, waiting, and unavailable
  states; Chromium guest pixels remain outside the boundary and no guest
  readiness claim is made.
- Live-render finding (2026-08-01): current-payload Explorer Dark desktop and
  Light/Largest narrow proof both show an orphaned wrapper-level `View` control
  above the EXPLORER title. The duplicate consumes top-of-space and does not
  belong to the Explorer shared frame; remove it before accepting the two
  Explorer captures.
- Live-render resolution (2026-08-01): removed the Explorer-only wrapper
  `View` control while preserving the Mesh Map switcher. Farm release build
  `f58b42ba…` passed compilation and was installed on `.138`; the replacement
  Explorer Dark desktop and Light/Largest narrow frames were visually accepted
  with hashes `df371c09…` and `1f9723cb…`. `.138` remained active with zero
  restarts and secure login-at-boot. Dell was unreachable during this deploy,
  so no Dell adoption claim is made.
- Live-render resolution (2026-08-01): exact Explorer-fix payload `f58b42ba…`
  Maps Dark desktop and Light/Largest narrow frames were visually accepted with
  hashes `33de1d86…` and `e36ac4f5…`; the governed map-content palette and
  responsive health/FAB geometry remain intact.
- Live-render resolution (2026-08-01): exact Explorer-fix payload `f58b42ba…`
  Workbench Dark desktop and Light/Largest narrow frames were visually accepted
  with hashes `55b58e11…` and `db1b223c…`; the shared STATE OF THE MESH frame,
  plane rail, node health body, and taskbar remain bounded.
- Live-render resolution (2026-08-01): exact Explorer-fix payload `f58b42ba…`
  Workloads was recaptured against the canonical `infra-code` route with the
  10-second proof-only settle window. The Dark desktop frame was visually
  accepted with hash `676675aa…`; WORKLOADS menu, lifecycle rail, placement
  card, plan-only state, and taskbar remain readable and bounded. The earlier
  `workloads`-target blank frame remains rejected and is not evidence.
- Live-render resolution (2026-08-01): exact Explorer-fix payload `f58b42ba…`
  Music Light/Largest narrow, Media Sources Dark desktop, and Terminal
  Light/Largest narrow frames were visually accepted with hashes `df42b3a1…`,
  `c6621041…`, and `71509a65…`; unavailable/empty backend states remain
  honest and the shared chrome stays bounded.
- Live-render resolution (2026-08-01): exact Explorer-fix payload `f58b42ba…`
  This Node/System, Editor, Phones, and Car Dark desktop frames were visually
  accepted with hashes `5154fcd9…`, `417803a0…`, `4fc968ac…`, and
  `02de499f…`. The shared/system chrome, editor body, unpaired Phones state,
  and Auto Mode cards remain readable and bounded; `.138` was restored to
  secure login-at-boot with zero restarts.
- Live-render resolution (2026-08-01): exact Explorer-fix payload `f58b42ba…`
  This Node/System, Editor, and Phones Light/Largest narrow frames were
  visually accepted with hashes `af02ada0…`, `483a6847…`, and `32f53d03…` at
  the `800` logical-width proof viewport. Large-text controls remain readable
  and bounded; the unused right scanout is intentional proof-viewport space.
- Live-render resolution (2026-08-01): exact Explorer-fix payload `f58b42ba…`
  Car Light/Largest narrow was visually accepted with hash `7591f6fb…`.
  Car deliberately uses the governed AutoSync3 skin, so its dark cockpit is
  the approved Car appearance under the Light profile; the complete large-text
  navigation/media/vehicle layout, app strip, and taskbar remain bounded.
- Live-render resolution (2026-08-01): exact Explorer-fix payload `f58b42ba…`
  This Node/System, Editor, and Phones Light desktop frames were visually
  accepted with hashes `ab0df010…`, `9d8802cf…`, and `975d15e9…`; the Light
  palette, shared chrome, and route-specific bodies remain readable and
  bounded on native `1920x1080` readback.
- Live-render resolution (2026-08-01): exact Explorer-fix payload `f58b42ba…`
  Files Dark desktop and Light/Largest narrow frames were visually accepted
  with hashes `44b27b97…` and `b5824b43…`. The new NODE ACTIONS inventory,
  peer roster, transfer affordances, list/preview geometry, and status row
  remain readable and bounded; this closes the direct-render proof slice for
  Bug 7 without claiming backend connectivity beyond the displayed roster.
- Live-render resolution (2026-08-01): exact Explorer-fix payload `f58b42ba…`
  Browser boundary Dark desktop and Light/Largest narrow frames were visually
  accepted with hashes `aeb104ca…` and `51115664…`. The frames prove only the
  Construct-owned VM selection, transport guidance, and waiting/unavailable
  state; guest Chromium pixels and browser readiness remain explicitly outside
  the host styling and evidence boundary.
- Live-render resolution (2026-08-01): exact Explorer-fix payload `f58b42ba…`
  Music Dark desktop, Media Light/Largest narrow, and Terminal Dark desktop
  frames were visually accepted with hashes `2facf38c…`, `51432679…`, and
  `929db052…`. Honest unavailable/idle states remain bounded and no backend
  connectivity is claimed.
- Live-render resolution (2026-08-01): exact Explorer-fix payload `f58b42ba…`
  Mesh Teams Dark desktop and Workloads Light/Largest narrow frames were
  visually accepted after the proof-only settle window with hashes
  `d521d2bc…` and `a1f39bdd…`. Mesh Teams shows its team/channel rail,
  Activity feed, collaboration tabs, and details rail; Workloads shows the
  large-text lifecycle rail, placement card, plan-only state, and taskbar on
  the canonical `infra-code` route. The proof seat remained active with zero
  restarts and was restored to secure login-at-boot; this closes two evidence
  cells only and does not establish full-matrix readiness.
- Live-render resolution (2026-08-01): exact Explorer-fix payload `f58b42ba…`
  Workloads Light desktop, Workloads Dark narrow, Mesh Teams Light desktop,
  and Mesh Teams Dark narrow frames were visually accepted after the
  proof-only settle window with hashes `d79c2a0f…`, `0208abe9…`,
  `83c7155e…`, and `f6171491…`. All four frames remain bounded and readable;
  the Workloads captures use the canonical `infra-code` route. The proof seat
  was restored to Dark/Default with `require_login_at_boot:true`, active
  service, and zero restarts. These cells expand evidence coverage only and do
  not establish full-matrix readiness or Dell adoption.
- Live-render resolution (2026-08-01): exact Explorer-fix payload `f58b42ba…`
  Files Light desktop, Files Dark narrow, Explorer Light desktop, and Explorer
  Dark narrow frames were visually accepted with hashes `447a4bdb…`,
  `1d72243d…`, `ef015cd4…`, and `6206cabf…`. Files' node-action inventory,
  peer roster, transfer controls, and list/preview boundary remain bounded;
  Explorer's mode filters, mesh card, and health summary remain bounded. The
  proof seat was restored to secure login-at-boot with an active zero-restart
  service. These cells expand direct-render evidence only and do not claim
  full-matrix readiness or Dell adoption.
- Live-render finding (2026-08-01): the current f58b42ba Maps Light desktop
  readback is pixel-valid but rejected on visual inspection. The honest
  no-offline-map fallback paints its `No offline map data` and installation
  guidance with shared shell `Style::TEXT`/`TEXT_DIM`, which resolves to dark
  ink over the dark map-content canvas in Light mode. The frame hash is
  `48ef0845…`; do not accept it until the fallback uses the governed
  high-contrast map-content palette and a fresh direct-DRM Light desktop frame
  passes visual inspection. This is a source defect, not a scanout failure.
- Live-render resolution (2026-08-01): the no-offline-map fallback now uses
  explicit high-contrast map-content tokens instead of shared shell text. Farm
  package tests passed 273/273, the exact shell release payload
  `bbe6a88c…` was installed on `.138`, and its replacement Maps Light desktop
  direct-DRM EGL readback was visually accepted with hash `0d59d2e1…`.
  `.138` returned to Dark/Default, secure login-at-boot, active service, and
  zero restarts. Dell was not reachable for this replacement deployment, so
  no Dell adoption claim is made.
- Live-render resolution (2026-08-01): exact Explorer-fix payload `f58b42ba…`
  Workbench Light desktop and Dark narrow frames were visually accepted with
  hashes `0f4fab9a…` and `0a597fc5…`. The Light and Dark STATE OF THE MESH
  chrome, This Node plane rail, health dashboard, and taskbar remain bounded
  in both proof geometries; the proof seat was restored to secure
  login-at-boot with an active zero-restart service. These cells expand
  direct-render evidence only and do not establish full-matrix readiness.
- Live-render resolution (2026-08-01): current `.138` payload `bbe6a88b…`
  Music Light desktop/Dark narrow, Media Sources Light desktop/Dark narrow,
  and Terminal Light desktop/Dark narrow frames were visually accepted with
  hashes `a343fa34…`, `50a620d4…`, `453621e3…`, `6b82836b…`, `3c9f2ae3…`,
  and `43c9a93e…`. The frames preserve truthful unavailable/empty states and
  remain bounded in native and `800`-logical-width proof geometries. `.138`
  was restored to secure login-at-boot with an active zero-restart service;
  this closes six evidence cells only and does not establish Dell adoption or
  production readiness.
- Live-render resolution (2026-08-01): current `.138` payload `bbe6a88b…`
  This Node Dark narrow, Editor Dark narrow, Phones Dark narrow, Car Light
  desktop, Car Dark narrow, Browser boundary Light desktop, and Browser
  boundary Dark narrow frames were visually accepted with hashes `9a9d2de5…`,
  `df916ce9…`, `61e248bc…`, `d1ff1660…`, `2eea2104…`, `0d929dcd…`, and
  `9640186a…`. Car retains its approved AutoSync3 cockpit exception. Browser
  proofs show only Construct-owned VM connection/waiting guidance; they do not
  claim guest Chromium pixels or VDI readiness. `.138` was restored to secure
  login-at-boot with an active zero-restart service; these cells do not close
  Dell adoption or production readiness.
- Live-render resolution (2026-08-01): current `.138` payload `bbe6a88b…`
  Bookmarks, This Node → Storage, Timers & Alarms, and This Node → About each
  received Dark desktop, Light desktop, Dark narrow, and Light/Largest narrow
  direct-DRM EGL frames. All sixteen frames were visually accepted: manager,
  storage, timer/alarm, device-inventory, node, peer, and taskbar content stayed
  readable and bounded, with only intentional vertical continuation or unused
  right scanout in narrow proofs. `.138` was restored to secure login-at-boot
  with an active zero-restart service. This closes sixteen evidence cells only;
  Dell adoption, VDI guest readiness, and production readiness remain unclaimed.
- Farm validation finding (2026-08-01): the current dirty tree compiles and
  passes `mde-files-egui` (165/165) and `mde-maps-location-egui` (273/273), but
  the `mde-shell-egui` suite reports 13 failures. Several assertions still
  encode the superseded central-launcher inventory, three-control status-bar
  geometry, or old profile/fallback interaction contracts after the requested
  tray and navigation changes. One chat timestamp failure is environment
  timezone-sensitive, and the remaining Front Door wording/pump failures need
  classification before any expectation is changed. No shell suite or release
  readiness claim is made until each failure is either corrected or explicitly
  evidenced as an unrelated baseline.
- Farm validation resolution (2026-08-01): updated the behavior assertions for
  tray-owned launcher exclusion, the fourth Power status cell, guest-owned
  Browser downloads, the Browser-only remote-session fallback, and the new
  launcher result count. The three focused BigBoy jobs all pass: status-bar
  geometry, Front Door ordering, and VDI fallback transition. The timezone
  expectation now derives from the configured display zone. The shell seam
  assertion is now deterministic: raw Super scan timing remains covered by
  the focused hotkey tests, while the shell test fixes the decoded input at
  the Front Door boundary and explicitly starts on home. Its focused BigBoy
  rerun passes. The final serial BigBoy shell suite passes 1,349/1,349;
  Files and Maps remain green at 165/165 and 273/273 respectively.
- Adoption audit finding (2026-08-01): direct reachability confirms `.138`
  (`172.20.146.138`) is online with the current `e37ec5c0…` shell payload,
  `mde-shell-egui.service` active, and `NRestarts=0`. Dell `.225`
  (`172.20.146.225`) currently returns no route and TCP/22 is unavailable.
  Dell's accepted screenshots remain valid historical visual evidence for the
  payloads named in their capture rows, but they do not prove synchronized
  adoption of the current `.138` payload. No current Dell-adoption claim is
  made. The approved boundary authority remains: focused VDI owns guest pixels,
  Maps owns its governed content-color exception, and `browser-vm` owns
  Chromium pixels and chrome; Construct owns only connection/unavailable/
  diagnostic states around those guests.
- Matrix audit finding (2026-08-01): the current `.138` payload has 30
  accepted direct-DRM rows, including the complete four-cell Bookmarks,
  Storage, Timers, and About slices, but several current-payload route/profile
  cells still rely on older payloads or are absent (notably Files, Mesh Teams,
  Editor, and portions of Music, Media, Terminal, Browser, Car, Maps, and This
  Node). Historical rows remain useful regression evidence, not substitutes
  for current-payload adoption proof. The full current-payload matrix remains
  open.
- Migration finding (2026-08-01): the unified This Node shell route still
  hand-composes its search row, section navigation, and System/Storage/About
  panels without the shared `AppFrame` header. This leaves the central local
  operations workspace outside the same frame contract used by Workbench,
  System, Timers, Phones, and Bookmarks. The migration must preserve the
  searchable section rail and the existing health-score/data seams.
- Migration resolution (2026-08-01): This Node now mounts the shared
  `AppFrame::new("This Node").leading_title()` before its searchable section
  rail and local System/Storage/About panels. The focused BigBoy render test
  passes at 960px, 480px, and 1.5x text zoom, confirming the title and body
  remain paint-reachable across desktop, narrow, and large-text geometry. This
  closes the frame-adoption slice only; current-payload DRM recapture is still
  required before closing its production evidence cells.
- Migration finding (2026-08-02): the standalone Explorer wrapper still
  hand-mounts a title-only shared `MenuBar` before the Explorer domain header.
  Since the wrapper has no menus or status chips, it should use the shared
  `AppFrame` primitive directly and leave Explorer's Mosaic/Hero/IPAM controls
  domain-owned, avoiding two competing title contracts.
- Migration resolution (2026-08-02): replaced the title-only Explorer wrapper
  `MenuBar` with `AppFrame::new("Explorer").leading_title()`, leaving the
  Explorer domain header responsible for Mosaic/Hero/IPAM controls. The focused
  BigBoy render test passes at desktop, narrow, and 1.5x text zoom. The final
  serial BigBoy shell suite passes 1,351/1,351. This closes the wrapper-frame
  adoption slice only; current-payload DRM recapture remains required.
- Migration finding (2026-08-02): the Bookmarks workspace still mounts an
  empty-menu `MenuBar` solely to paint its title, while its detail pane already
  uses `AppFrame`. Replace that title-only wrapper with the shared `AppFrame`,
  preserve the domain search/sort/location strip below it, and retain the
  desktop/narrow/large-text render coverage before accepting the migration.
- Migration resolution (2026-08-02): replaced the title-only Bookmarks wrapper
  with `AppFrame::new("Bookmarks").leading_title()`, preserving the domain
  search/sort/location strip below it. BigBoy passed the focused Bookmarks suite
  41/41 and the serial shell suite 1,351/1,351. Exact payload
  `a4a1a21a0c5fe820b4c3de23a1ae403cbe201628580074df3b133c34cf2fbc18` was
  installed on `.138` with `active`/`NRestarts=0`; Dark desktop, Dark narrow,
  and Light/Largest readbacks were visually inspected and accepted as
  `d5ad5896b372e4fc0b52a0296f54c59790146baa13f88e759a5a7f6f2a4ac3b0`,
  `3cf3276c22584cce235e2c0f30b951be53096192bc01f0f4003376c616d98ead`, and
  `19c2891dff5d1c393ccdedd0f39ae642d925f95f31b4fdafbe3f3518e7385a63`.
  This closes the Bookmarks frame-adoption slice only; current synchronized
  Dell adoption, strict linear scanout, the remaining route/profile matrix,
  and WL-UX-009 readiness remain open.
- Current-payload evidence finding (2026-08-02): the accepted Files node-action
  frames in the DRM ledger belong to the earlier `83fe8f3c…` payload, while the
  installed `.138` payload is now `a4a1a21a…` after the Bookmarks frame slice.
  Recapture Files Dark desktop, Dark narrow, and Light/Largest on the exact
  installed payload before treating the Files node-integration visuals as
  current-payload evidence; retain the existing shared Files menu/domain chrome
  and its ten honest node-action states.
- Live-render finding (2026-08-02): the exact `a4a1a21a…` Files Light/Largest
  readback preserves the mesh roster but pushes the NODE ACTIONS inventory below
  the visible left pane, so Browse/Prepare/Copy/Send/Review/Upload/Transfers
  are not discoverable at the large-text profile. Reject the frame and make the
  node-action region responsive or scrollable without hiding the primary
  node-aware controls, then recapture all three Files cells.
- Live-render follow-up (2026-08-02): after prioritizing the node-action block,
  all ten rows are visible in the large-text frame, but the long Airsonic status
  copy reaches underneath its Upload button. Reserve the action-column width in
  every node-action row and keep the service-ownership copy bounded before the
  final Light/Largest acceptance.

- Live-render follow-up (2026-08-02): the current-payload Dark/narrow readback
  still places Places and Mesh ahead of the node-action inventory, leaving the
  lower node actions below the first viewport. Keep the node-action inventory
  ahead of secondary sidebar content in narrow as well as large-text layouts,
  then recapture the exact payload before closing the Files evidence slice.

- Resolution (2026-08-02): Files now prioritizes the node-action inventory at
  every profile, reserves the action lane in each row, and uses the bounded
  truthful Airsonic state `Music-owned`. BigBoy release slot
  `wl-ux-009-files-narrow-actions-release-20260802` produced payload
  `c2e6f20a01042ed854861a5457ff03caa19f165070f2f9ce71e0172b8c42d7a4`, which
  was installed on `.138` with `active`/`NRestarts=0`; the focused Files suite
  passed 166/166. Exact current-payload Dark desktop, Dark narrow, and
  Light/Largest frames were inspected and recorded in the DRM ledger. This
  closes the Files responsive node-action/current-payload slice only; it does
  not close WL-UX-009 readiness.

- Next validation slice (2026-08-02): after the Files current-payload proof,
  the remaining WL-UX-009 gap is a current-hash direct-DRM matrix for the
  Construct-owned routes that still rely on representative or older-payload
  rows. Start with Music, Media, Phones, Terminal, Editor, Maps, and the
  Browser boundary on `.138`, preserving the guest-UI boundary and secure
  seat-state restoration; accept only visually inspected non-overlapping
  frames and keep Dell synchronized adoption open while `.225` is unreachable.

- Proof-harness finding (2026-08-02): the first current-payload route batch on
  `.138` produced valid Dark desktop frames for Music, Media, Phones, Terminal,
  and Editor, but the scripted restart sequence reached systemd's
  `start-limit-hit` before Maps. Logs show clean proof-readback termination
  (`signal=TERM`, no application crash) after each capture; recover the seat
  with `reset-failed` between proof restarts and split the matrix into smaller
  batches before accepting the route frames.

- Proof-harness finding (2026-08-02): the subsequent narrow and Light/Largest
  route files were written successfully but several readbacks contained only
  the wallpaper clock/backdrop rather than the requested workspace, despite
  the proof logger reporting a completed EGL readback. Reject those cells as
  stale/incorrect-route evidence; validate route selection from the captured
  pixels and use a longer per-route settle/verification step before recording
  any of them.

- Live-render finding (2026-08-02): the exact current-payload Editor
  Light/Largest frame reaches the editor surface, but its internal menu wraps
  `Help` onto a second line and the toolbar becomes a tall multi-row block.
  Reject that cell until the Editor internal chrome has an intentional
  large-text layout with bounded menus, readable controls, and no accidental
  vertical expansion; preserve editor behavior and the existing narrow proof.
- Adoption preparation (2026-08-02): built the current tree on BigBoy with
  `drm,live-vdi,media-mpv` and installed the resulting shell binary on `.138`.
  Artifact and installed `/usr/bin/mde-shell-egui` both hash to
  `e37ec5c0df9ac05133c073f481823b5642a9fef85dd0237502f82abb2a9d13dd`;
  `mde-shell-egui.service` is active with `NRestarts=0`. This proves current
  `.138` binary adoption only. No DRM visual cell is closed until a fresh
  direct-render capture from this hash is visually inspected.
- Proof-harness finding (2026-08-02): the current `.138` Workloads proof route
  was explicitly selected with `MDE_DRM_PROOF_SURFACE=infra-code`, but the
  chooser's normal automatic-popup redirect subsequently changed the first
  rendered surface back to Desktop. The resulting readback contained only the
  desktop backdrop/taskbar and is rejected. During an approved proof run, the
  explicit route must outrank chooser auto-redirect; ordinary boots must retain
  the existing chooser behavior.
- Proof-harness resolution (2026-08-02): proof mode now suppresses only the
  chooser's automatic popup redirect, preserving explicit `MDE_DRM_PROOF_SURFACE`
  routing while leaving ordinary boots unchanged. The corrected release payload
  `e37ec5c0…` passed the full BigBoy shell suite (**1,351 passed, 0 failed**),
  was installed on `.138` with `active`/`NRestarts=0`, and produced three
  inspected Workloads frames: Dark desktop
  `67bdbc920ffa95e89da9f33c6a155dd53992867e18d81007176ed532e7aed4b8`, Dark
  narrow `65a2f548fe2ce904c21cd84d468236dac4dca45672b5fc8597cb958488dedee0`,
  and Light/Largest `91332b390e93dacf7c110d689e7ecf9eaa595cf2e9a4053943190544b98dfd54`.
  This closes the current-payload Workloads slice; the remaining workspace
  matrix stays open.
- Current-payload matrix batch (2026-08-02): recapture the remaining
  Construct-owned route/profile cells on the exact `.138` payload
  `e37ec5c0…`, using the settled EGL readback path and explicit proof routing:
  Files, Mesh Teams/Editor, Phones, Music, Media, Terminal, This Node, Maps,
  and Browser boundary. Capture Dark desktop, Dark narrow, and Light/Largest;
  reject any frame that is stale, blank, overlapping, or claims guest-owned
  pixels. Restore secure login-at-boot and the prior appearance after the batch.
- Current-payload matrix batch resolution (2026-08-02): `.138` produced and
  visually passed 19 route/profile observations on `e37ec5c0…`, covering Files
  Dark desktop/narrow/Light-Largest; Mesh Teams Activity and Editor Dark
  desktop/narrow; Phones Dark desktop; Music Dark desktop/Light-Largest; Media
  Light-Largest; Terminal Dark narrow/Light-Largest; This Node/System Dark
  narrow/Light-Largest; Maps Dark desktop/narrow; and Browser boundary Dark
  desktop/narrow. Their hashes are recorded in the DRM evidence ledger. The
  Maps Light/Largest frame was rejected for faint governed map-canvas empty-state
  copy, and zero-sized captures caused by the seat restart-rate limit were not
  accepted. The seat was restored to secure login-at-boot and the service is
  `active` with `NRestarts=0`; the remaining cells and Dell adoption stay open.
- Live-render finding (2026-08-02): the exact `e37ec5c0…` Maps Light/Largest
  frame still paints the governed “No offline map data” copy underneath the
  opaque/semi-opaque Radio & GNSS health rail. The copy is therefore nearly
  black-on-dark and not readable, despite the map-content palette being bright
  before the later HUD paint. Move the honest empty-state treatment into a
  reserved, unobscured map lane or give it an explicit backing surface, then
  recapture the exact replacement payload before accepting the cell.
- Follow-up live-render finding (2026-08-02): the replacement Maps frame moves
  the empty-state panel clear of the alert lane, but its title uses a bottom
  anchor near the panel's top edge and is clipped beneath the health rail. Keep
  both empty-state lines fully inside the backing panel before the final
  Light/Largest recapture.
- Follow-up live-render finding (2026-08-02): after correcting the text anchor,
  the large-text map geometry still places the panel's top edge too near the
  health rail, so the title remains partially occluded in the Light/Largest
  proof. Lower the whole panel into the bottom-safe lane; do not accept the
  frame until both lines are visibly separated from the rail and alert pills.
- Follow-up live-render finding (2026-08-02): the final Light/Largest frame now
  has readable empty-state text, but the panel border still crosses the wider
  large-text alert pill because the wide-lane threshold is measured in logical
  coordinates. Lower that threshold for the wide route; keep the 800-wide
  narrow route centered and recapture before accepting the cell.
- Follow-up live-render finding (2026-08-02): the threshold-only adjustment did
  not change the exact Light/Largest pixels because the map rectangle remains
  below that logical threshold. Use a clamped right-biased placement for the
  empty-state panel on every map canvas so it cannot cross the left alert lane;
  retain the in-rect clamp as the narrow-layout safety boundary.
- Next validation slice (2026-08-02): validate the current worktree's Maps
  empty-state panel placement and palette against the exact farm release on
  Dell `.138`. Capture Dark desktop, Dark narrow, and Light/Largest direct-DRM
  EGL readbacks with explicit Maps routing; accept only frames whose two-line
  panel is fully visible, separated from the health/alert rail, and free of
  scanout/readback artifacts. Restore secure login-at-boot state after proof.
- Resolution (2026-08-02): the exact `4808bd30…` release passed the Maps
  placement recapture on `.138`. The map-owned empty-state panel is clamped
  right-biased, backed, high-contrast, and fully below the Radio & GNSS rail;
  Dark desktop, Dark narrow (`800` logical width), and Light/Largest frames
  were visually inspected and accepted. Proof hashes and PNGs are recorded in
  the DRM evidence ledger. The Maps empty-state placement slice is closed;
  overall WL-UX-009 readiness, the remaining route/profile cells, and Dell
  `.225` adoption remain open.
- Dell current-payload Maps resolution (2026-08-02): exact synchronized payload
  `08cf147d…` rendered Maps Dark desktop, Dark narrow (`800` logical width), and
  Light/Largest on `.225`. All three frames were visually inspected: the
  right-biased empty-state panel is fully readable, the health/alert content is
  bounded, and no readback artifact or guest-owned pixels are present. Dell was
  restored to `require_login_at_boot:true`, Dark/Default, active service, and
  `NRestarts=0`. This closes only the current Dell Maps slice; the full matrix,
  strict linear scanout, and WL-UX-009 readiness remain open.
- Next validation slice (2026-08-02): recapture the Browser boundary on the
  synchronized `08cf147d…` Dell payload in Dark desktop, Dark narrow (`800`
  logical width), and Light/Largest. Accept only Construct-owned VM selection,
  transport, waiting, or unavailable content; guest Chromium pixels remain
  outside the host evidence boundary. Restore secure seat state after capture.
- Dell current-payload Browser resolution (2026-08-02): exact synchronized
  payload `08cf147d…` rendered the Browser boundary on `.225` in Dark desktop,
  Dark narrow (`800` logical width), and Light/Largest. Every frame contains
  only Construct-owned VM selection, transport, and waiting guidance; no guest
  Chromium pixels are claimed. Dell was restored to
  `require_login_at_boot:true`, Dark/Default, active service, and `NRestarts=0`.
  This closes the Dell Browser boundary slice only; guest VDI readiness, the
  remaining route matrix, strict linear scanout, and WL-UX-009 readiness remain
  open.
- Next validation slice (2026-08-02): recapture Phones on the synchronized
  `08cf147d…` Dell payload in Dark desktop, Dark narrow (`800` logical width),
  and Light/Largest. Confirm the shared title/navigation header, pairing state,
  and remote-input controls remain bounded; accept no stale or blank frame and
  restore secure seat state afterward.
- Live-render finding (2026-08-02): exact synchronized payload `08cf147d…`
  on Dell `.225` renders the Phones title and empty-state copy at `800` logical
  pixels, but the Features, Remote input, and tab body are absent. Reject the
  Dark narrow frame; compare the same payload on `.138` before changing source
  geometry, and do not close the Dell Phones narrow cell from title-only output.
- Proof-harness finding (2026-08-02): the temporary appearance override used
  `density`, but the persisted schema is `layout_profile` plus `text_scale`.
  Dell journal evidence reports `density=Mouse` and `text_scale=Default` during
  the recent purported Light/Largest captures. Those images remain valid Light
  visual evidence only; they do not close any large-text cell. Correct the
  override to `layout_profile=construct`, `text_scale=largest`, and recapture
  affected large-text rows before accepting them.
- Phones narrow follow-up (2026-08-02): adding a dedicated `phones-body`
  `ScrollArea` identity passed the focused 800px headless contract but did not
  change Dell’s live title/empty-state-only frame; the rejected hash remained
  identical after release payload `6538cf7e…`. Keep the Dell Phones narrow cell
  open and do not treat the identity change as a behavioral resolution.
- Farm compile-integrity finding (2026-08-02): the current dirty tree calls
  `show_charge_limit_action` from `thisnode.rs` but has no reachable definition,
  so the focused shell test no longer compiles. The deployed `6538cf7e…`
  artifact predates this incomplete seam; repair or salvage the existing user
  change before building a release that includes the Phones viewport work.
- Resolution (2026-08-02): the farm re-synchronized the current tree and the
  focused Phones contract passed at `800` logical pixels and `text_scale=largest`.
  The alleged missing `show_charge_limit_action` helper was present and
  reachable in the synchronized source; no unrelated This Node repair was
  needed. The release build completed on BigBoy and produced payload
  `20955383f21c4e2a4aee6a519be24905a2c65b2605f52ad3f515dcf8f2b9ae5a`.
- Dell Phones resolution (2026-08-02): payload `20955383…` was deployed to
  `.138` and `.225`; both services are active with `NRestarts=0`. Dell `.225`
  now renders the complete Phones body at `800` logical pixels in Dark/Default
  and Light/Largest. Direct-DRM frames were visually inspected and accepted;
  the prior title-only result is superseded by this evidence. The Phones
  current-payload slice is closed; strict linear scanout, the full route/profile
  matrix, and WL-UX-009 readiness remain open.
- Workloads current-payload validation (2026-08-02): exact release payload
  `20955383…` was exercised through the canonical `infra-code` route on both
  direct-DRM seats. Dell `.225` passed Dark desktop, Dark narrow (`800`), and
  Light/Largest desktop and narrow; `.138` passed the same profiles after a
  proof-only unlock sequence. The WORKLOADS menu, lifecycle rail, placement
  card, plan-only state, capacity bars, and no-selection body were visually
  inspected and remained readable and bounded. An initial `.138` curtain frame
  was discarded and is not evidence. Both seats were restored to
  `require_login_at_boot:true`, Dark/Default, active service, and zero restarts.
  This closes only the current Workloads visual slice; VDI guest readiness,
  strict linear scanout, the remaining route/profile matrix, and WL-UX-009
  readiness remain open.
- Browser boundary current-payload validation (2026-08-02): exact release
  payload `20955383…` passed Browser host-surface proof on both direct-DRM
  seats across Dark desktop, Dark narrow (`800`), and Light/Largest desktop
  and narrow. Every inspected frame contains only Construct-owned VM selection,
  transport, waiting, and unavailable guidance; no guest Chromium pixels are
  claimed. Both seats were restored to `require_login_at_boot:true`,
  Dark/Default, active service, and zero restarts. This closes the Browser
  host/guest-boundary visual slice only; VDI guest readiness, strict linear
  scanout, the remaining route/profile matrix, and WL-UX-009 readiness remain
  open.
- Music current-payload validation (2026-08-02): exact release payload
  `20955383…` passed the explicit `music` route on both direct-DRM seats in
  Dark desktop, Dark narrow (`800`), and Light/Largest desktop and narrow.
  The shared MUSIC menu/status chrome and the honest missing-Airsonic-credentials
  state remain readable and bounded. No connectivity to Subsonic/Airsonic at
  `172.20.0.2` is claimed. Both seats were restored to
  `require_login_at_boot:true`, Dark/Default, active service, and zero restarts.
  This closes only the current Music visual/state slice; the remaining route
  matrix, strict linear scanout, backend readiness, and WL-UX-009 readiness
  remain open.
- Media current-payload validation (2026-08-02): exact release payload
  `20955383…` passed the explicit `media` route on both direct-DRM seats in
  Dark desktop, Dark narrow (`800`), and Light/Largest desktop and narrow.
  The corrected `MEDIA` title, source tabs, local-capture controls, Jellyfin
  fields, and honest no-source state remain readable and bounded; the prior
  narrow `ME` clipping does not reproduce. Both seats were restored to
  `require_login_at_boot:true`, Dark/Default, active service, and zero restarts.
  This closes only the current Media visual slice; live media/Jellyfin backend
  readiness, strict linear scanout, the remaining route matrix, and WL-UX-009
  readiness remain open.
- Terminal current-payload live-render finding (2026-08-02): exact payload
  `20955383…` on Dell `.225` renders the Terminal body and controls at
  `800` logical pixels, but the shared workspace title is clipped to `TER…`.
  Reject the Dell Terminal Dark narrow cell; preserve the full title and
  command/session hit targets within the constrained header before recapturing
  current-payload Terminal profiles. The `.225` desktop frame is accepted as
  separate evidence only; this finding is not a seat-wide readiness claim.
- Corrected appearance-proof resolution (2026-08-02): the proof override now
  uses the real `layout_profile=construct` and `text_scale=largest` fields;
  Dell journal evidence records `scheme=Light`, `density=Mouse`,
  `text_scale=Largest`, and `motion=Normal`. Corrected current payload
  `6538cf7e…` Maps Light/Largest, Browser Light/Largest, and Phones Light
  desktop frames were visually inspected and accepted. The earlier same-named
  Light/Largest rows captured with `text_scale=Default` are retained only as
  Light evidence and do not close large-text cells. Phones Light/Largest narrow
  remains rejected because its body is absent at 800px.
- Next validation slice (2026-08-02): recapture the Browser VM boundary on the
  exact `4808bd30…` `.138` payload in Dark desktop, Dark narrow (`800` logical
  width), and Light/Largest. Confirm the Construct-owned VM selection,
  unavailable/transport guidance, and honest waiting state remain readable and
  bounded while guest Chromium pixels stay outside the host surface. Reject
  any stale, blank, or guest-content frame and restore secure seat state after
  the batch.
- Live-render finding (2026-08-02): Browser boundary Dark desktop and Dark
  narrow pass on the exact `4808bd30…` payload, but Light/Largest exposes a
  shell chrome contrast defect: the bottom tray glyphs and clock/date become
  nearly black on the black taskbar band. Reject that Light/Largest cell until
  the shared taskbar/tray foreground resolves through the active Light palette,
  then recapture Browser and at least one neighboring Light/Largest workspace
  to ensure the fix is global and does not dim Dark mode.
- Resolution (2026-08-02): the bottom taskbar/tray now uses fixed high-contrast
  foreground tokens for its opaque black backing; the top status rail remains
  scheme-aware. Status-bar tests pass 21/21. Payload `728e0dcb…` was deployed
  to `.138` with active service and zero restarts. Browser Dark desktop, Dark
  narrow, and Light/Largest were visually inspected and accepted, with no
  guest Chromium pixels claimed; the neighboring Maps Light/Largest frame also
  remains readable. The Browser boundary/taskbar contrast slice is closed;
  the complete route/profile matrix and WL-UX-009 readiness remain open.
- Build finding (2026-08-02): the current worktree's This Node/System settings
  palette migration leaves four `apply_settings_popup_style(ui.style_mut(),
  Style::color_scheme(ui.ctx()))` calls with overlapping mutable and immutable
  borrows. The release DRM shell cannot be rebuilt until those calls read the
  scheme before taking the mutable style borrow; apply the minimal ordering fix
  and rerun the shell release gate.
- Live-render finding (2026-08-01): Workbench Light/Largest narrow proof exposed
  an orphaned duplicate `View` button at the top-left. The shell's Fleet & Mesh
  wrapper had rendered its legacy `ui.menu_button("View")` above the Workbench
  shared MenuBar; the initial frame was rejected and the duplicate was removed
  before recapture.
- Live-render resolution (2026-08-01): removed the wrapper-level duplicate,
  farm-tested the Workbench menubar suite (15 passed), deployed payload
  `333503d01d85534c929ef05600d7ba5b12194f16f1324a0c5e0a0cc3ce345879` to Dell
  and `.138`, and visually accepted the replacement Workbench Light/Largest
  narrow frame. Both seats remained active with zero restarts; `.138` was
  restored to secure login-at-boot.
- Live-render finding (2026-08-01): current-payload Phones Light/Largest narrow
  proof shows the paired/online header and tabs, but the shared `AppFrame`
  title is absent from the entire top strip in both the Light narrow frame and
  the earlier Dark frame. The frame is rejected until Phones exposes a visible
  shared title/navigation header and is recaptured on the current payload.
- Live-render resolution (2026-08-01): added the shared `AppFrame::leading_title`
  option, used it for Phones, passed the focused Phones suite (25 tests), built
  and deployed payload `3bd01985a670c723b87eefc5a1230344ead763d4b97c7fbb9401ee0b02ff91c8`
  to Dell and `.138`, and visually accepted the replacement Light/Largest
  narrow frame. Both seats remained active with zero restarts; `.138` was
  restored to secure login-at-boot.
- Live-render finding (2026-08-01): current-payload Car Light/Largest narrow
  proof produced only the dark Auto/brand background curve and no Auto Mode
  title, cards, or app strip. The frame is rejected; determine whether the
  proof route is reaching `Surface::AutoHome` after the Car profile boot or
  whether the Auto Mode body is being collapsed, then recapture before closing
  the Car matrix cell.
- Live-render finding (2026-08-01): after the Car proof route was corrected to
  resolve `car`/`auto-home` to `Surface::AutoHome`, the replacement `.138`
  Light/Largest narrow frame rendered the complete Auto Mode cockpit but still
  failed visual acceptance. The left instrument strip's large-text status
  labels and values overlap, and a long lower-left status value is clipped at
  the scanout edge. Keep the Car cell open; make the strip zoom-aware and
  width-safe, farm-test it, then recapture and inspect the live frame.
- Live-render finding (2026-08-01): the first zoom-aware Car strip recapture
  removed the overlap and safely elided the long `RADIO HEALTH` value, but its
  two-column six-row large-text grid still lets the final status row reach the
  taskbar and clip vertically. Keep the Car cell open; use a bounded
  large-text column/row arrangement that fits every selected status tile inside
  the instrument strip before recapturing.
- Farm-integrity finding (2026-08-01): the focused shell gate on the Car-layout
  tree is currently stopped by `crates/desktop/mde-shell-egui/src/backdrop.rs`
  passing `image::ColorType::Rgb8` to the `image 0.25` JPEG encoder, which now
  requires `ExtendedColorType`. Repair the explicit conversion before treating
  the Car layout gate or release artifact as validated.
- Farm-integrity resolution (2026-08-01): `backdrop.rs` now passes the explicit
  `image::ExtendedColorType::Rgb8` value required by the current image API. The
  focused farm gate `cargo test -p mde-shell-egui wallpaper_decode_tests` passed
  1/1 on `.90`; the broader shell suite remains independently open because of
  its unrelated pre-existing timezone, launcher, status-bar, browser-download,
  and remote-session failures.
- Live-render resolution (2026-08-01): the Car strip now uses a three-column
  large-text grid, zoom-aware gauge allocation, and width-safe label/value
  elision. The `image 0.25` JPEG encoder conversion was also repaired. The
  focused farm gate passed 2 tests; release payload
  `736471b1d47f38162107915d36ff7542cd9aba27cc09ba0e49dd0b1aeb9cf46b` was
  deployed to Dell and `.138`, both services stayed active with zero restarts,
  and the replacement `.138` Light/Largest narrow Car frame was visually
  accepted with SHA-256
  `88e022c9427345ee94adb0e164553ba655a8e6ba2c85f501be2407db17d59e5e`.
- Current-payload Car Light/Largest narrow finding (2026-08-02): exact payload
  `20955383f21c4e2a4aee6a519be24905a2c65b2605f52ad3f515dcf8f2b9ae5a` on
  direct DRM seat `.138`, explicitly routed as `MDE_DRM_PROOF_SURFACE=car`,
  rendered the complete Auto Mode cockpit without the prior geometry defects,
  but its Light appearance still painted the SYNC3 dark ground and card
  palette. The frame is rejected for the governed Light cell; make the Car
  surface resolve its paint tokens through the shared Dark/Light palette while
  preserving AutoSync3 for the mode-derived vehicle skin, then farm-test and
  recapture both seats before closing this finding.
- Farm-integrity finding (2026-08-02): the focused `mde-shell-egui` Car test
  gate is blocked by two existing `ThisNode` accessors in
  `system/mod.rs` moving `self.snapshot` through `?` from `&self` (Rust E0507).
  Repair the borrow-only access, rerun the focused Car suite, and preserve the
  unrelated warning-only findings as non-blocking evidence.
- Farm-integrity resolution (2026-08-02): the two `ThisNode` accessors now use
  `self.snapshot.as_ref()?`; the BigBoy focused Car gate passed all 15 tests.
  The feature-correct release build (`drm,live-vdi,media-mpv`) started with
  `drm:true` on both direct seats, but the first live Car proof routed to the
  shell clock/dashboard instead of `Surface::AutoHome`. Both seats were
  restored to their prior active binaries, secure login-at-boot, and
  Dark/Construct/Default/Normal. `.138` is at zero restarts; `.225` reports one
  restart from the rejected proof attempt. The Car live matrix remains open
  pending a corrected route proof.
  Temporary proof overrides were removed and `.138` returned to secure
  `require_login_at_boot:true`.
- Live-render finding (2026-08-01): current payload `736471b1…` Dark desktop
  Phones proof exposes the shared `Phones` title and body, but the paired/online
  status row begins outside the left scanout edge and is visibly clipped. Keep
  the Phones Dark desktop cell open; bound the status row to the AppFrame
  content inset and recapture before accepting it.
- Live-render resolution (2026-08-01): the Phones paired/online status row and
  mesh-identity line now share the AppFrame content inset. The focused Phones
  suite passed 25 tests on farm `.90`; release payload
  `8366b1094571c8ec520166e6af09e342954850bde149f14146613091e5317f4b` was
  installed on `.138`, and the replacement Dark desktop frame was visually
  accepted with SHA-256
  `87f5b3e9a7c3a234b66a513e04b4b875dc87612e53733f214566352ca388799d`.
  The proof overrides were removed afterward; `.138` remained active with zero
  restarts and `require_login_at_boot:true`.
- Live-render finding (2026-08-01): the first current-payload Editor Light
  desktop capture on `.138` was rejected because the seat rendered the secure
  boot curtain/clock rather than the requested Editor route. The PNG is not
  evidence; repeat the proof with the temporary proof-only unlock state, then
  restore `require_login_at_boot:true` and verify zero restarts.
- Live-render resolution (2026-08-01): after the proof-only unlock override,
  current payload `8366b109…` produced a valid `.138` Editor Light desktop
  frame. The shared Mesh Teams/editor chrome, document body, details rail, and
  taskbar were visually inspected and accepted with SHA-256
  `bc48241b3cd0fbed2a9b4f191527cd88437060307d5c83b8f4b1bb2f02fd69c5`.
  The override and appearance fixture were removed afterward; the seat was
  restored to `require_login_at_boot:true`, active, and zero restarts.
- Navigation-rail design decision (2026-08-01): preserve the navigation rail in
  both desktop and narrow layouts. Add Fleet & Mesh, Music, Media, Phones, and
  This Node as five individual notification-tray/tool-tray icons in the
  default order Fleet & Mesh, Music, Media, Phones, This Node; keep their glyphs monochrome, show a
  subtle active indicator, and expose workspace name, status, and keyboard
  shortcut on hover/focus. This is a navigation presentation change only and
  must continue using the shared icon registry, AppFrame routing, and existing
  rail geometry; no rail removal or collapse is authorized. Move the Pin
  placement control to the far-right end of the rail/taskbar controls and
  render it as a monochrome Windows 10-style Show Desktop glyph, with the
  existing placement behavior unchanged.
- Launcher de-duplication requirement (2026-08-01): every workspace exposed by
  the notification/tool tray must be removed from the central launcher catalog
  and its grouped launcher views. Keep direct routing, search/keyboard access,
  the persistent navigation rail, and the five tray icons intact; this is a
  presentation de-duplication, not a workspace retirement.
- Pin placement correction (2026-08-01): in the bottom taskbar, the
  monochrome Windows 10-style Show Desktop glyph must sit immediately to the
  right of the clock, rather than to the left of the notification/tool-tray
  cluster. Preserve the existing taskbar-placement action and keep the control
  reachable in both desktop and narrow layouts.
- This Node tray-icon correction (2026-08-01): remove the notification-dot or
  badge treatment from the This Node tool-tray glyph. It must remain a clean
  monochrome workspace icon; active state continues to use the shared subtle
  tray indicator rather than a notification dot.
- Leading Search alignment correction (2026-08-01): remove the unused blank
  leading space before the Search/Start control in the taskbar and rail. Keep
  the control's fixed hit target, focus treatment, and neighboring Back/Home
  spacing stable after the leading-edge alignment.
- Bookmarks workspace investigation (2026-08-01): determine whether the
  standalone Bookmarks workspace provides capabilities not already owned by
  Browser or Files. If it is only a duplicate launcher surface, retire its
  workspace/catalog entry while preserving bookmark storage and Browser/Files
  access; do not remove underlying bookmark data or actions.
- Bookmarks investigation resolution (2026-08-01): retain the workspace. It
  owns folder CRUD, import/export, CRDT/mesh synchronization, bulk selection,
  drag reorder/move, and link checking; Browser bookmark/history search is only
  a consumer and Files is not an equivalent manager. No safe removal boundary
  exists without a separate migration design.
- Power indicator requirement (2026-08-01): add a live Windows 10-style
  battery/power-management indicator to the shared notification/tool tray.
  Match the existing monochrome status-control pattern, expose a truthful
  tooltip/status detail, and source the value from the existing local power
  read model rather than painting a decorative indicator.
- Files node-integration expansion (2026-08-01): make Files a meaningful
  multi-node workspace with ten node-aware interactions: (1) node-scoped
  browse, (2) prepopulated share targets, (3) prepopulated link targets,
  (4) one-click send-to-node, (5) receive/inbox view, (6) per-node sync
  status, (7) conflict queue, (8) node availability/offline state, (9)
  Airsonic-aware upload destination prepopulation, and (10) node activity and
  transfer history. Preserve honest unavailable states and existing access
  boundaries; no invented transfer success or server capability.
- Files node-integration validation (2026-08-02): the Files implementation now
  renders the ten-item NODE ACTIONS inventory beside the live mesh roster and
  routes supported controls through the existing peer-mount, transfer-target,
  shared-clipboard, and Transfers seams. Airsonic/Subsonic-compatible Music
  Library upload is prefilled through the existing daemon-owned symbolic music
  lane; live server discovery and credential readiness remain worker-owned and
  are not inferred by Files. Farm gate
  `MCNF_BUILD_HOST=172.20.0.130`, slot `bug7-files-node-20260802`,
  `cargo test -p mde-files-egui --lib`: **165 passed, 0 failed**.
- Files node-actions follow-up (2026-08-02): convert the inventory's supported
  status rows into direct controls backed by existing Files seams (peer mount,
  shared clipboard, send, Downloads/inbox, and Transfers). Keep rows disabled
  or informational where the current model has no truthful action, rather than
  adding a decorative or unimplemented command.
- Files node-actions pane routing finding (2026-08-02): the new node-action
  controls must target the active pane in dual-pane mode. A hard-coded pane 0
  would make Browse, Copy, Send, and Receive act on a different pane than the
  operator is viewing; record and test the active-pane routing before the next
  validation capture.
- Files node-actions resolution (2026-08-02): supported rows now expose direct
  `Open`, `Prepare`, `Copy`, `Send`, `Review`, `Upload`, and `Transfers` controls
  backed by the existing action dispatcher. Browse opens the first reachable
  peer through the real mount path; Receive opens the node Downloads/inbox; no
  reachable peer or no worker still renders as an honest state. Farm validation
  on BigBoy slot `bug7-files-node-actions-pane-routing-20260802` passes **167
  passed, 0 failed**; pane-dependent controls now target the active pane in
  dual-pane mode.
- Files node-actions current-payload validation (2026-08-02): build and deploy
  the pane-routing correction to `.138`, then recapture Dark desktop, Dark
  narrow, and Light/Largest Files frames. Accept only frames showing the full
  ten-action inventory with no action/status overlap; restore secure state and
  leave Dell `.225` adoption and WL-UX-009 readiness open.
- Files node-actions live-render finding (2026-08-02): the corrected route
  renders the Files inventory on Dark desktop and Light/Largest, but the
  800-logical-width Dark narrow frame loses the sidebar entirely because long
  action/status rows expand the side panel beyond the available layout. Cap
  the sidebar and truncate the status copy within its reserved action lane;
  recapture the narrow cell before accepting the current-payload slice.
- Files node-actions current-payload resolution (2026-08-02): capped the Files
  sidebar, bounded node-row summaries to the reserved action lane, and
  recaptured exact release `de4c6bbe…` on `.138`. Dark desktop, Dark narrow at
  800 logical width, and Light/Largest all show the complete ten-action
  inventory without overlap; accepted PNG hashes and service restoration are
  recorded in `WL-UX-009-DRM-EVIDENCE-2026-07-31.md`. This closes the Files
  node-action slice only; `.225`, strict linear scanout, remaining matrix, and
  WL-UX-009 readiness remain open.
- Live-render finding (2026-08-02): Dell `.225` Phones Light/Largest narrow
  current-payload proof rejects the frame because the Remote input card is
  clipped by the taskbar boundary. Keep the cell open; make the Phones content
  region scroll-safe or otherwise reserve the taskbar-safe viewport, then
  recapture the exact candidate before acceptance.
- This Node current-payload validation slice (2026-08-02): validate the
  unified `this-node` route on exact installed release `de4c6bbe…` at `.138`
  across Dark desktop, Dark narrow (`MDE_DRM_PROOF_LOGICAL_WIDTH=800`), and
  Light/Largest. Inspect health-score linkage, provider/status hierarchy,
  action affordance boundaries, clipping, and taskbar contrast; reject any
  stale/incorrect route. Restore secure state and close only this evidence
  slice.
- This Node current-payload validation resolution (2026-08-02): exact release
  `de4c6bbe…` on `.138` passed Dark desktop, Dark narrow at 800 logical width,
  and Light/Largest visual inspection. The shared frame, provider-aware rail,
  health linkage (`97% charging` plus display count), live display state, and
  scroll continuation remain readable and bounded; proof hashes and secure
  restoration are recorded in `WL-UX-009-DRM-EVIDENCE-2026-07-31.md`. This
  closes this validation slice only; `.225`, strict linear scanout, the
  remaining matrix, and WL-UX-009 readiness remain open.
- Music current-payload validation slice (2026-08-02): validate the explicit
  `music` route on exact installed release `de4c6bbe…` at `.138` across Dark
  desktop, Dark narrow (`800` logical width), and Light/Largest. Inspect the
  shared frame, status/taskbar contrast, empty/unavailable state, and honest
  missing-Airsonic-credentials boundary; do not infer connectivity to
  Subsonic server `172.20.0.2`. Restore secure state and close only accepted
  evidence cells.
- Music current-payload validation resolution (2026-08-02): exact release
  `de4c6bbe…` on `.138` passed Dark desktop, Dark narrow at 800 logical width,
  and Light/Largest visual inspection. The shared Music frame, taskbar
  contrast, and honest missing-Airsonic-credentials state are recorded with
  hashes in `WL-UX-009-DRM-EVIDENCE-2026-07-31.md`; no Subsonic connectivity is
  claimed. This closes the Music validation slice only; `.225`, strict linear
  scanout, the remaining matrix, and WL-UX-009 readiness remain open.
- Media current-payload validation slice (2026-08-02): validate the explicit
  `media` route on exact installed release `de4c6bbe…` at `.138` across Dark
  desktop, Dark narrow (`800` logical width), and Light/Largest. Inspect shared
  frame adoption, source tabs/controls, honest local/Jellyfin unavailable
  states, taskbar contrast, and any VDI/guest boundary; reject stale or
  wallpaper-only readbacks. Restore secure state and close only accepted cells.
- Media live-render finding (2026-08-02): exact `.138` Dark narrow proof clips
  the shared workspace title from `MEDIA` to `ME` at 800 logical width while
  the source controls remain visible. Reject the narrow cell; preserve the
  complete title and menu hit targets within the constrained header before
  recapturing the current payload. Dark desktop and Light/Largest remain
  provisional until the narrow correction is proven.
- Media live-render resolution (2026-08-02): the shared menubar now reserves a
  measured, unwrapped title galley and gives narrow layouts a dedicated identity
  row plus horizontally scrollable command row. Farm suites passed `mde-egui`
  **269/269** and `mde-media-egui` **104/104**. Exact release
  `c75e01456f56902d92d36a4ebe8b9f68822e74c00670cb82b090b3bd05909dac` on `.138`
  passed visually inspected Media Dark desktop, Dark narrow at 800 logical
  width, and Light/Largest frames; hashes and links are in
  `WL-UX-009-DRM-EVIDENCE-2026-07-31.md`. Secure state was restored with the
  service active and zero restarts. This closes the Media validation slice only;
  `.225`, strict linear scanout, the remaining route/profile matrix, and
  WL-UX-009 readiness remain open.
- Editor current-payload validation slice (2026-08-02): validate the explicit
  `editor` route on the exact installed `.138` payload
  `c75e01456f56902d92d36a4ebe8b9f68822e74c00670cb82b090b3bd05909dac` across
  Dark desktop, Light desktop, Dark narrow (`800` logical width), and
  Light/Largest. Inspect direct-entry sidebar collapse, shared top chrome,
  editor tabs/palettes/status rows, no-overlap behavior, taskbar contrast, and
  the approved focused-VDI boundary; reject stale, wallpaper-only, or guest-
  viewport claims. Restore secure state and close only visually inspected cells.
- Editor live-render finding (2026-08-02): exact `.138` Dark narrow proof keeps
  direct-entry sidebars collapsed and the full `EDITOR` title visible, but clips
  the right end of the editor toolbar at 800 logical pixels (the zoom control is
  cut off). Reject the narrow cell; preserve every toolbar action through a
  bounded horizontal command strip or equivalent responsive layout before
  recapturing the current payload.
- Editor live-render resolution (2026-08-02): the Editor compact threshold now
  folds the width-heavy Standard/Formatting controls into their existing `»`
  overflow at 800 logical pixels, keeping every action reachable without
  clipping. Farm package tests passed **407/407**. Exact release
  `807f2430d61bbd8316410c6360d8beaba2de55dba643933f641ac4d59ab5ecd7` on `.138`
  passed visually inspected Dark desktop, Light desktop, Dark narrow, and
  Light/Largest Editor frames. The direct-entry optional sidebars remained
  collapsed; the embedded guest/VDI viewport was not claimed. Hashes and
  evidence links are in `WL-UX-009-DRM-EVIDENCE-2026-07-31.md`. Secure state
  was restored with the service active and zero restarts. This closes the
  Editor validation slice only; `.225`, strict linear scanout, the remaining
  route/profile matrix, and WL-UX-009 readiness remain open.
- Terminal current-payload validation slice (2026-08-02): validate the explicit
  `terminal` route on exact installed `.138` payload
  `807f2430d61bbd8316410c6360d8beaba2de55dba643933f641ac4d59ab5ecd7` across
  Dark desktop, Light desktop, Dark narrow (`800` logical width), and
  Light/Largest. Inspect the Terminal-pattern shared top bar, session/tab
  chrome, command/status rows, no-overlap behavior, taskbar contrast, and
  truthful unavailable/session states; reject stale or wallpaper-only proof.
  Restore secure state and close only visually inspected cells.
- Shared-frame migration finding (2026-08-02): Mesh Teams still mounts a
  title-only shared `MenuBar` before its domain rails. Replace that wrapper with
  the shared `AppFrame` primitive, retain the Mesh Teams channel/app chrome as
  the approved domain header, and add desktop/narrow/large-text render coverage
  before accepting the migration.
- Shared-frame migration resolution (2026-08-02): Mesh Teams now uses the
  shared `AppFrame` for its workspace identity strip while retaining its
  domain-specific channel/app rails. The focused render test covers 1280px
  desktop, 800px narrow, and 1.5x large-text frames; farm package gate
  `wl-ux-009-mesh-teams-frame-20260802` reports **129 passed, 0 failed**.
- Current-payload adoption resolution (2026-08-02): the release shell built on
  BigBoy slot `wl-ux-009-current-adoption-20260802` was installed on `.138` and
  verified as payload `e37ec5c0…`; `mde-shell-egui.service` is `active` with
  `NRestarts=0`. The exact payload produced an inspected CPU-linear EGL
  readback desktop frame, but this does not satisfy the complete current-payload
  desktop/narrow/large-text matrix.
- Exact current-payload matrix follow-up (2026-08-02): after the Maps correction,
  `.138` was recaptured on payload `83fe8f3c…` through the proof-only CPU-linear
  EGL path. Twenty-seven inspected Files, Mesh Teams, Editor, Phones, Music,
  Media, Terminal, This Node, and Browser-boundary Dark desktop/narrow or
  Light/Largest cells are recorded in the DRM evidence ledger, with the seat
  restored to secure Dark/Default, `require_login_at_boot:true`, active, and
  zero restarts. This closes only those named evidence cells; the remaining
  route/profile matrix, strict linear scanout, Dell synchronized adoption, and
  WL-UX-009 readiness remain open.
- Governance finding (2026-08-02): the corrected Maps empty-state backing panel
  still minted two UI chrome colors directly in `mde-maps-location-egui`, so
  `lint-style-leaks.sh` reported two leaks. Move those map-owned panel tokens
  into the shared `mde-egui::Style` registry with a backing assertion, then
  rerun the gate and preserve the accepted visual evidence as the prior exact
  payload proof.
- Governance resolution (2026-08-02): Maps now consumes shared
  `Style::map_empty_panel()` and `Style::map_empty_border()` tokens, with a
  backing shared-style assertion. The style-leak gate is clean at zero; farm
  gates pass `mde-egui` **269/269** on `.90` slot
  `wl-ux-009-style-registry-rerun-20260802` and Maps **273/273** on BigBoy slot
  `wl-ux-009-maps-style-registry-rerun-20260802`.
- Governance resolution (2026-08-02): This Node now routes the combined
  provider/health explanation through the shared themed hover-card helper
  without a raw `egui` tooltip modifier. The style-leak gate is clean at zero,
  and the full `mde-shell-egui` farm binary suite passes **1363/1363** on
  `.90` slot `wl-ux-009-tooltip-suite-20260802`. This source-level correction
  is not yet represented by a newly deployed live-render payload; the existing
  `911e7818…` evidence remains valid for the title-allocation correction only.
- Current-source adoption slice (2026-08-02): `.138` is healthy on payload
  `911e7818…` and `.225` is healthy on payload `4bfb57fb…`, both active with
  zero restarts. Build the current source on BigBoy, deploy the exact resulting
  payload to both seats, verify secure login-at-boot state, and recapture the
  remaining high-value Construct-owned route/profile cells before accepting any
  synchronized-adoption claim.
- Current-source adoption resolution (2026-08-02): BigBoy release slot
  `wl-ux-009-current-source-adoption-20260802` produced payload
  `15efa11a2cd84563cef4af6d94455df0a286b5b0b49a4065e52a4d00d541cac2`.
  The exact binary is installed on both `.138` and `.225`; both services are
  active with zero restarts. `.138` was restored to secure
  `require_login_at_boot:true`, Dark/Default appearance, and no proof override.
  This proves binary adoption only; `.225` has no current visual capture, so
  synchronized visual adoption and WL-UX-009 readiness remain open.
- Current-payload This Node proof resolution (2026-08-02): exact payload
  `15efa11a…` on direct DRM `.138` passed visual inspection for Dark desktop,
  Dark narrow (`800` logical width), and Light/Largest. Evidence is recorded in
  the DRM ledger. The narrow frame is bounded with intentional unused scanout;
  the large-text frame keeps the lower System content within its scroll
  boundary without overlap or clipping. This closes only the named This Node
  visual cells.
- Governance finding (2026-08-02): the This Node power-profile action still
  attaches a raw `egui` hover tooltip, so the style-leak gate reports one raw
  hover-text leak. Replace it with the shared themed hover-card helper and
  rerun the focused shell suite and style gate before accepting this correction.
- Governance resolution (2026-08-02): This Node power-profile actions now use
  the shared themed hover-card helper. The style-leak gate is clean at zero and
  the focused This Node farm suite passes **34/34** on `.90` slot
  `wl-ux-009-tooltip-power-suite-20260802`. This source correction is not yet
  included in the deployed `15efa11a…` visual payload; its live proof remains
  scoped to the exact recorded payload.
- Live-render finding (2026-08-02): exact payload `15efa11a…` on Dell `.225`
  renders the This Node Dark desktop and Light/Largest detail body against the
  `1366x768` viewport with the lower `SYSTEM` heading clipped at the viewport
  boundary; the Dark narrow proof collapses to the title with the body absent.
  Reject these three frames. Give the unified This Node body an explicit
  vertical scroll boundary, farm-test the responsive contract, rebuild, and
  recapture `.225` before accepting its visual cells.
- Live-render resolution (2026-08-02): the unified This Node body now sits in
  an explicit vertical scroll boundary. Release payload `08cf147d…` is
  installed on both `.138` and `.225`, each active with zero restarts; `.225`
  was restored to secure `require_login_at_boot:true` and Dark/Default. The
  exact `.225` Dark desktop and Light/Largest frames no longer clip the lower
  System content and are recorded as body-geometry evidence. The `.225` Dark
  narrow frame still paints only the title and remains rejected; the full
  bottom-tray behavior is also not claimed from this slice.
- Narrow differential resolution (2026-08-02): the same exact `08cf147d…`
  payload renders the full This Node Dark narrow body on direct-DRM `.138`
  (hash recorded in the DRM ledger), while `.225` still paints only the title
  at the same `800` logical width. This localizes the remaining narrow gap to
  Dell `.225` runtime/layout conditions rather than the shared This Node
  source; do not convert the `.138` pass into a `.225` or farm-wide claim.
- Narrow differential follow-up (2026-08-02): compare Terminal and Files Dark
  narrow direct-DRM frames on Dell `.225` at the exact `08cf147d…` payload.
  Use the results to separate a workspace-specific This Node route defect from
  a seat-wide narrow viewport condition; accept no frame without visual
  inspection and restore secure state after the batch.
- Narrow differential resolution (2026-08-02): exact `08cf147d…` Dell `.225`
  Terminal and Files Dark narrow frames both render complete, bounded bodies;
  the remaining `.225` narrow failure is specific to This Node. The accepted
  hashes and evidence links are in the DRM ledger. Dell was restored to secure
  Dark/Default with `require_login_at_boot:true`, active service, and zero
  restarts. This does not close the This Node narrow cell.
- Current-payload This Node resolution (2026-08-02): the exact release
  `20955383f21c4e2a4aee6a519be24905a2c65b2605f52ad3f515dcf8f2b9ae5a` was
  explicitly routed to `this-node` after removing a stale duplicate Terminal
  proof override. Both `.138` and Dell `.225` now pass inspected Dark desktop,
  Dark narrow (`800`), Light/Largest desktop, and Light/Largest narrow frames;
  the complete body anchors and node tabs are visible, with large-text content
  bounded by the intentional vertical scroll region. The eight hashes are in
  the DRM ledger. Both seats returned to secure login-at-boot, active service,
  and zero restarts. This closes only the current This Node visual slice.
- Headless narrow-contract follow-up (2026-08-02): the existing This Node
  shared-frame test only requires non-empty primitives and the `This Node` title,
  so it could pass while Dell paints title-only output. Strengthen the test at
  the exact `800` logical-width case to require body anchors (`Find a section`,
  `Workspace`, and `Overview`) before accepting any source or live-render
  resolution for the Dell narrow cell.
- Headless narrow-contract resolution (2026-08-02): the farm test now covers
  `800` logical pixels and requires all three This Node body anchors in addition
  to non-empty primitives and the title. It passes `1/1` on `.90` under slot
  `wl-ux-009-this-node-body-suite-20260802b`; the Dell `.225` title-only live
  frame therefore remains an evidence-backed runtime/layout defect, not a
  headless-test omission. Keep the Dell narrow cell open.
- Direct-DRM proof finding (2026-08-02): `.138` rejects the strict linear
  scanout proof because linear allocation returns `Invalid argument` and the
  front buffer is `XR30` with `I915_x_tiled`. The proof therefore fails closed;
  no invalid scanout screenshot is accepted. Temporary proof environment was
  removed and the service returned to `active` with `NRestarts=0`. Keep
  WL-UX-009 `Remaining` until live render coverage passes.
- Live-render finding (2026-08-01): current payload `4cf6732e…` Maps
  Light/Largest proof is not accepted. The desktop frame loses readable
  status-card text, and the 800-logical-width frame lets the right-side
  control rail overlap the search banner and health content. Keep the Maps
  Light/Largest desktop and narrow cells open; make the route's large-text
  geometry and palette explicit, farm-test it, then recapture and inspect both
  direct-DRM frames.
- Follow-up live-render finding (2026-08-01): after the lane fix, the Maps
  Light/Largest narrow frame still shows the map-local “Acquiring GPS” chip
  colliding with the larger alert pill. The narrow layout must retain one clear
  primary alert without duplicating that status chip; record the suppression as
  a focused responsive behavior and recapture the Light/Largest narrow frame.
- Live-render resolution (2026-08-01): the Maps route now reserves the FAB
  lane in the banner, keeps Light-mode health-card text on the governed
  map-content palette, and suppresses the redundant map-local GPS chip on
  narrow canvases. Focused Maps tests pass; payload `bccc6e94…` is installed
  on both Dell seats with active services and zero restarts. The replacement
  `.138` Light/Largest narrow EGL frame was visually inspected and accepted
  with SHA-256 `e36ac4f5331f65b5a1fcc80de8a13ad35ea12c2076a8a821f8e86da2881cf51c`.
- Live-render resolution (2026-08-01): the remaining Maps Dark desktop,
  Light/Largest desktop, and Dark narrow cells were recaptured on the exact
  `bccc6e94…` payload and visually accepted with hashes
  `33de1d86…`, `29f68032…`, and `154c43df…`. The four-cell Maps
  Dark/Light desktop/narrow slice is now evidence-backed; the broader
  Construct-owned route matrix remains open.
- Backdrop validation update (2026-08-01): palette-resolved status backing,
  large-text anchoring, wallpaper selection/cache, and watermark routing pass
  24 focused shell tests with 1,920 filtered on `.90` slot
  `ux009-backdrop-palette-244`; live DRM Maps recapture remains open.
- Wallpaper policy update (2026-08-01): the Settings surface now exposes Bing
  daily picture as the sole wallpaper provider, clears legacy custom-page
  configuration during normalization, and forces Bing on for migrated settings;
  the focused normalization test passes on farm `.90` slot
  `ux009-bing-only-247`.
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
- Current state: Existing System, Storage, Device Manager, About, Control
  Center, status, BlueZ, display, power, firmware, and input code provide
  partial foundations; controls remain read-only in places, routes are
  duplicated, and no unified typed action boundary covers connectivity, audio,
  input, performance, docks, or OEM capabilities. This Node now has a governed
  eight-section catalog, persistent section/search navigation with operator
  hardware aliases, legacy normalization, and unavailable provider states.
  Its mesh-status model projects bounded typed capability/action rows with
  fail-closed mutations plus truthful interface/CIDR/route/lighthouse/DNS
  connectivity facts; provider loss now retains the last valid projection with an explicit stale banner and bounded reason instead of silently
  dropping the node; readable snapshots older than 90 seconds are also marked stale; stale capability and action rows are degraded until refresh; the Workbench now exposes a real on-demand `Refresh now` control over the same bounded poll seam; focused This Node tests pass, including narrow, large-text reflow and disabled-action accessibility coverage (21 tests, farm `.90` slot 143).
  Workbench rendering and refresh affordance coverage passes 15 tests on farm `.90` slot 143.
  Mesh & System now adds a truthful unavailable/unknown/offline/degraded/connected
  summary with AccessKit status and 3 focused tests.
  The mesh-status writer now adds bounded NetworkManager/ModemManager link
  observations (provider, interface, state, and up only) without serializing
  connection profiles, SSIDs, APNs, SIM/operator data, or credentials; shell
  and embedded-Python syntax checks pass.
  The same writer now publishes only bounded active/advertised power-profile
  names from `powerprofilesctl`; This Node renders the observation and keeps
  profile mutation explicitly unavailable until the trusted System provider
  authorization path is unified. Focused This Node (21 tests) and Workbench
  (15 tests) suites pass on farm `.90` slot 150.
  The landing surface now presents a typed Device Manager-style Status/Devices
  & Peripherals/System tree and health dashboard using the existing snapshot
  authority, with expanded operator search vocabulary and explicit stale/
  unavailable details; the focused shell This Node filter passes 13 tests on
  farm `.50`. Connectivity projections now carry typed, read-only recovery
  guidance (`refresh snapshot` or `await provider publication`); stale retained
  rows are prevented from appearing actionable; the focused This Node filter
  passes 23 tests with zero failures on farm `.50` slot
  `ux011-recovery-206`. The typed NetworkManager/ModemManager boundary now
  rejects malformed or credential-shaped interface/device identifiers, filters
  unsafe externally supplied observations during merge, and correctly maps
  disconnected cellular state; focused type tests pass 1/1 on `.90` slot
  `network-observation-types-215`, with rustfmt passing on
  `network-observation-rustfmt-216`; the daemon-focused gate passes 6/6 with
  4,305 filtered on `.90` slot `network-observation-daemon-220` after the
  `.50` lane exhausted its 424 MiB free farm disk during linking.
  The durable This Node index now exposes 16 stable provider-aware routes with
  deterministic child-page search ranking and explicit unavailable reasons;
  its focused catalog gate passes 8 tests on farm `.90`. The This Node Display &
  Sound surface now folds bounded credential-free PulseAudio-compatibility,
  PipeWire graph, WirePlumber, ALSA/UCM, playback, capture, and recovery facts,
  rendering missing providers as unavailable and failed lanes as attention;
  its focused audio projection gate passes 1 test with 1,943 filtered on farm
  `.90` slot `ux011-audio-shell-227`. The daemon's bounded audio collector now
  covers PulseAudio compatibility, PipeWire, WirePlumber, ALSA/UCM, playback,
  capture, and recovery with 9 focused tests passing and 4,306 filtered on
  `.90`; the mesh-status writer publishes the flattened credential-free
  projection consumed by This Node.
- Provider-state update (2026-08-01): This Node now exposes typed recovery
  guidance for stale/unavailable providers and preserves bounded credential-free
  audio facts without fabricating healthy hardware; 24 focused tests pass on
  farm `.90`.
- Provider-state update (2026-08-02): the This Node plane now exposes a typed
  `Open detail` workflow for all 16 governed parent/child routes. Each
  full-page detail view reuses the inventory health/stale authority and
  existing connectivity, audio, power, service, mesh, and capability
  projections; unavailable details remain visibly fail-closed. The focused
  BigBoy shell gate passes 27 This Node tests, including rendering every
  governed detail route.
- Unified-route integration update (2026-08-02): the shell's This Node section
  selector now reaches the existing typed System/Seat provider seams for
  Network, Displays, Mouse/Input, Power, Theme, and Mesh identity instead of
  returning early to a dead unavailable card. Legacy System/Storage/About
  aliases remain normalized, and the focused shell This Node gate passes 20
  tests including narrow/large-text frame and provider-route coverage.
- Global-health drilldown update (2026-08-02): selecting the existing status/
  grade cluster now uses the typed workspace handoff to open `Surface::ThisNode`
  while individual volume, network, brightness, and power controls retain
  their Control Center behavior. The focused BigBoy status-bar gate passes 23
  tests, including the drilldown route and no-duplicate-overlay assertion.
- Health-tree continuity update (2026-08-02): the unified This Node tree now
  consumes the existing snapshot health projection, shows a severity glyph and
  readable local state beside every governed section, and polls that projection
  while This Node is directly mounted so a global-health drilldown cannot show
  a stale tree solely because Workbench is hidden. The focused BigBoy shell
  gate passes 21 tests, including tree, provider-route, responsive, and
  large-text coverage.
- Actions-contract update (2026-08-02): each finite This Node action now carries
  explicit typed metadata for expected impact, confirmation/arming, trusted
  session authorization, audit fields, and bounded recovery. The UI renders
  that contract beside the still-disabled provider row, keeps stale state
  degraded, and exports the full reason through AccessKit. The focused BigBoy
  This Node suite passes 28 tests, including the contract-completeness and
  disabled-accessibility checks.
- OS-management route truth update (2026-08-02): the existing durable routes
  for Users & Accounts, Backup & Restore, Diagnostics & Logs, and Virtualization
  & Remote Access now carry explicit unavailable provider reasons instead of
  inheriting false availability from Mesh & System. Their full-page details
  render a bounded recovery/provider card; Services, Updates, and Mesh & System
  retain their published snapshot facts. BigBoy catalog tests pass 8/8 and the
  This Node detail suite passes 28/28.
- Diagnostics evidence update (2026-08-02): Diagnostics & Logs is now a usable
  full-page route over the existing bounded snapshot authority, exposing service
  health, mesh peers/leader, and resource telemetry with the same truthful
  stale/unknown states as the dashboard. Raw journal entries remain explicitly
  unavailable pending a fixed, redacted, size-limited operator-log provider;
  no fabricated log content crosses the UI boundary.
- Privacy projection update (2026-08-02): the credential-free mesh-status
  boundary now publishes a bounded default-microphone mute bit and camera-device
  count; This Node renders both alongside an explicitly unavailable camera
  privacy provider state. Device presence is not treated as permission, no
  names/paths/applications/credentials cross the boundary, and stale/missing
  facts remain unknown. BigBoy This Node tests pass 29/29; shell and embedded
  Python syntax checks pass.
- Bluetooth projection update (2026-08-02): the snapshot writer now publishes
  bounded BlueZ adapter, power, and device-count observations without names,
  MAC addresses, pairing keys, trust material, service UUIDs, or agent data.
  Connectivity renders missing BlueZ explicitly unavailable and keeps pairing
  and trust behind the typed provider boundary. BigBoy This Node tests pass
  30/30; shell and embedded Python syntax checks pass.
- Resource telemetry update (2026-08-02): the node snapshot now publishes
  bounded aggregate CPU count/load, memory capacity/availability, and root
  filesystem capacity/usage facts without process names, mount paths, user
  names, or device identities. This Node exposes the facts in inventory and
  Power & Performance/Hardware detail views with capacity bars, marks partial
  hardware coverage as attention, and keeps stale snapshots visibly degraded.
  The Local Telemetry capability is now available only when a bounded fact is
  present; BigBoy This Node tests pass 31/31, with shell and embedded Python
  syntax checks passing.
- Live performance history update (2026-08-02): Power & Performance and the
  shared Performance detail anatomy now retain at most 60 five-second samples
  of normalized aggregate CPU load, memory use, and root-storage use. The
  dense chart preserves textual labels/current values for keyboard and
  assistive-technology users, breaks lines across missing facts, and remains
  presentation-only over the existing snapshot authority. BigBoy This Node
  tests pass 36/36.
- Power-source projection update (2026-08-02): the snapshot now publishes
  bounded aggregate battery count/percentage/status and AC-online facts from
  the kernel power-supply interface without supply names, serials, models, or
  sysfs paths. Power & Performance renders those facts beside the read-only
  profile state and preserves unknown values when no source provider exists.
  BigBoy This Node tests pass 32/32; shell and embedded Python syntax checks
  pass.
- Local power-detail update (2026-08-02): the full-page This Node Power &
  Performance route now consumes the existing trusted typed UPower seat seam
  for detailed local battery rows, charge state, time-to-empty/time-to-full,
  energy rate, and AC-source state. Provider absence remains explicitly
  unavailable; detailed identity and estimates stay out of the world-readable
  mesh-status projection. The narrow/large-text responsive regression was
  tightened with a two-line connectivity disclosure, and the complete BigBoy
  This Node suite passes 35/35.
- Display/input observation update (2026-08-02): the snapshot now publishes
  bounded DRM connector/connected/mode counts, aggregate backlight count and
  level, and evdev event-device count without connector names, sysfs paths,
  input names, event streams, or serials. Display & Sound and Input now have
  discoverable full-page read-only inventory routes; display/input writes remain
  behind typed seat authorization and recovery contracts. BigBoy This Node
  projection tests pass 33/33 and catalog tests pass 8/8; shell and embedded
  Python syntax checks pass.
- Local device-inventory detail update (2026-08-02): This Node's full-page
  Display & Sound route now consumes the trusted typed DRM provider for bounded
  connector identity, physical size, preferred mode, and advertised mode lists;
  Input now consumes the typed kernel keyboard-LED provider for per-device
  brightness. Missing local providers remain visibly unavailable, mode/refresh/
  arrangement writes stay on the confirmation-gated DisplayLayout seam, and
  the complete BigBoy This Node suite passes 35/35.
- Local audio-detail update (2026-08-02): the full-page Display & Sound route
  now consumes the trusted typed PipeWire graph for master, playback, capture,
  host/VM/mesh origin, volume, and mute rows. Local stream names stay outside
  the world-readable mesh-status projection; audio writes continue through the
  dedicated confirmation-gated mixer Actions seam. Missing PipeWire remains
  visibly unavailable, and the complete BigBoy This Node suite passes 35/35.
- Hardware/storage observation update (2026-08-02): the snapshot now publishes
  bounded block-device count/capacity/removable count plus thermal-zone peak
  and fan-sensor counts without block names, hwmon labels, sysfs paths, or raw
  sensor streams. Hardware and Storage details render these facts beside the
  existing filesystem capacity bars; thermal/storage writes remain fail-closed
  until a typed bounded provider exists. BigBoy This Node tests pass 34/34;
  shell and embedded Python syntax checks pass.
- Typed action seam update (2026-08-02): This Node's power-profile Actions row
  now receives the live System provider through the Workbench seam. It offers
  only non-active profiles simultaneously advertised by the fresh mesh
  observation and the typed local seat snapshot, requires a visible second-click
  impact confirmation, and dispatches the existing `SetPowerProfile` provider
  action; stale, mismatched, unavailable, and refused states remain fail-closed.
  Focused BigBoy gates pass This Node 34/34 and System 68/68; the Workbench
  render fixture compiles with the shared provider seam.
- Unified Actions reachability update (2026-08-02): the durable This Node shell
  now exposes an Inventory/Actions workspace switch beside the hierarchy. The
  Actions view reuses the same typed System provider and confirmation state as
  Workbench, remains explicit when the snapshot is stale, and reflows at
  desktop, narrow, and large-text sizes. BigBoy This Node tests pass 34/34;
  the unified Actions render gate passes 1/1 across all three geometry cases.
- Users/Accounts observation update (2026-08-02): the root snapshot now
  publishes bounded aggregate local-account, interactive-account, and
  admin-policy-group counts only when `/etc/passwd` and `/etc/group` are
  readable. This Node exposes a discoverable Users & Accounts detail route;
  usernames, home paths, shells, memberships, credentials, and sign-in state
  remain outside the shared boundary, while account mutations stay fail-closed
  until a typed identity provider exists. BigBoy This Node tests pass 35/35;
  shell syntax and diff checks pass.
- Personalization continuity update (2026-08-02): the durable This Node catalog
  now treats Personalization as available because the unified shell hierarchy
  reaches the existing typed System/Theme provider for persisted theme,
  wallpaper, layout, text-scale, motion, and clock controls. The legacy detail
  bridge now describes that provider-backed continuity instead of claiming the
  provider is absent. Catalog tests pass 9/9 and the full This Node detail/render
  suite passes 35/35 on BigBoy; responsive route coverage remains included.
- Child-route hierarchy update (2026-08-02): the durable shell tree now keeps
  the catalog's child routes under the selected parent branch, so Storage,
  Peripherals, Users, Services, Updates, and other detail routes are reachable
  without expanding the initial inventory frame beyond the Actions workspace.
  Parent routes continue to open the typed System provider; child routes retain
  their own This Node detail authority. The focused shell This Node filter passes
  22/22 across responsive and large-text coverage, and the underlying This Node
  detail/projection suite passes 35/35 on BigBoy.
- Network topology disclosure update (2026-08-02): the Connectivity detail now
  keeps interface and CIDR facts in the compact summary and places default route,
  lighthouse, and DNS facts behind an explicit responsive `Topology details`
  disclosure. The disclosure remains read-only and names the typed
  NetworkManager/ModemManager safety boundary rather than implying mutation
  support from observations.
- Detail-anatomy update (2026-08-02): every available full-page This Node detail
  now exposes consistent collapsed Actions, Events, Performance, Audit, and
  Vendor packs sections. Performance reuses bounded CPU/memory/root-storage
  telemetry; Events, audit records, and vendor packs remain explicitly
  unavailable until their typed providers publish evidence, so the UI does not
  fabricate operator history or installed extensions. The governed full-page
  render gate passes on BigBoy.
- Action evidence update (2026-08-02): the unified System provider now emits a
  bounded redacted outcome record for real This Node dispatches across power,
  Bluetooth, display, brightness, audio, network, input, and session actions.
  Full-page Events and Audit panes consume those records with fixed action
  labels, accepted/refused outcome, UTC timestamp, and local trusted-session
  scope; provider paths, device identities, credentials, and raw errors remain
  excluded. Empty histories still render an explicit no-records state.
  BigBoy This Node tests pass 36/36.
- Diagnostics provider update (2026-08-02): Diagnostics & Logs now runs a
  fixed, read-only `journalctl` warning/error query off the render thread.
  Output is bounded to 32 lines, 240 characters per line, and 12 KiB total;
  known credential, token, and network-secret fields are redacted before This
  Node renders them. Provider refusal, absence, empty evidence, and returned
  entries retain distinct truthful states, and no user-supplied query or raw
  stderr crosses the UI boundary. BigBoy This Node tests pass 37/37.
- OS-management index update (2026-08-02): the durable This Node search index
  now includes dedicated Security & Privacy, Accessibility, and Time,
  Language & Region child routes. Each route is assigned to the existing
  hierarchy, has operator search aliases, and renders an explicit unavailable
  provider reason until a typed authority is connected; it does not create a
  duplicate settings surface or imply unsupported capability. BigBoy catalog
  tests pass 9/9.
- Security & Privacy detail update (2026-08-02): the Security & Privacy child
  route now consumes the existing bounded microphone mute and camera-device
  observations instead of hiding all privacy evidence behind an unavailable
  page. Camera permission, encryption, firewall, and security-policy state
  remain separately and explicitly unavailable; device presence is never
  treated as permission. The full BigBoy shell suite passes 1,373/1,373,
  including This Node detail rendering and desktop/narrow/large-text frame
  coverage.
- Accessibility/time continuity update (2026-08-02): This Node's Accessibility
  route now reads the durable System text-scale and motion preferences, while
  Time, Language & Region reads the durable display clock zone. Both routes
  preserve the existing settings authority and explicitly identify missing
  screen-reader, assistive-device, language, locale, keyboard-region, and host
  time-sync providers. The full BigBoy shell suite passes 1,373/1,373.
- Vendor-pack contract update (2026-08-02): the shared This Node projection now
  accepts at most eight sanitized vendor-pack manifests, each with bounded name,
  version, status, and capability metadata. Unknown statuses fail closed;
  manifests cannot create routes or executable actions, and the detail anatomy
  keeps vendor metadata visibly separate from standard controls. Hostile-input
  coverage and the full focused This Node suite pass 38/38 on BigBoy.
- Hardware-center provider update (2026-08-02): the Hardware detail route now
  surfaces the trusted local DRM connector/mode and keyboard-backlight probes
  when the durable System provider is mounted, while preserving explicit
  provider-continuity and unavailable states otherwise. Raw sysfs paths,
  firmware controls, thermal streams, fan identities, and OEM writes remain
  outside the UI until bounded typed providers supply authorization, limits,
  audit, and recovery. Focused This Node render/projection verification is
  complete on BigBoy with 38/38 passing.
- Local sensor-provider update (2026-08-02): `mde-seat` now owns a fixed-root,
  read-only thermal/hwmon provider in the off-render-thread seat snapshot. It
  bounds the thermal-zone list, labels, temperatures, and fan-input count,
  exposes no kernel paths or mutation verbs, and folds missing class roots into
  a typed unavailable state. The Hardware detail view renders these local facts
  beside the existing aggregate snapshot and keeps fan curves, platform
  profiles, CPU/GPU limits, firmware, and OEM writes fail-closed. BigBoy
  `mde-seat` passes 107/107 and the focused This Node suite passes 38/38.
- Platform-profile observation update (2026-08-02): the same fixed-root local
  provider now folds the standard kernel current platform profile and at most
  eight sanitized choices into Hardware detail. This is read-only capability
  evidence; profile mutation still requires a separate trusted worker with
  confirmation, audit, thermal limits, watchdog recovery, and safe fallback.
  BigBoy `mde-seat` and focused This Node tests pass 107/107 and 38/38,
  respectively.
- Platform-profile action update (2026-08-02): the Actions workflow now offers
  only fresh kernel-advertised platform-profile choices from the trusted local
  hardware provider. Each change is visibly armed and confirmed, revalidated
  immediately before the fixed provider write, recorded through the existing
  redacted audit seam, and reports refusal without optimistic state. Vendor,
  fan-curve, CPU/GPU-limit, firmware, and raw OEM controls remain unavailable.
  BigBoy focused This Node tests pass 38/38 and `mde-seat` passes 107/107.
- Vendor-pack discovery update (2026-08-02): the root mesh-status writer now
  discovers at most eight regular, non-symlink JSON manifests from the fixed
  `/usr/share/mde/vendor-packs/` package boundary. Names, versions, statuses,
  and capabilities are bounded and sanitized; unknown status fails closed, and
  the published metadata cannot create routes, commands, or executable actions.
  Embedded Python parsing, shell syntax, and diff checks pass; live installed
  vendor-pack evidence remains unavailable when no manifests are published.
- Bluetooth Actions update (2026-08-02): This Node Actions now exposes a typed
  Bluetooth-power row when both the fresh aggregate snapshot and live System
  BlueZ provider advertise an adapter target. The first click arms confirmation;
  the second dispatches the existing typed seat action, with provider refusal,
  toast/error, and no optimistic success preserved. The action-contract gate
  and fail-closed Actions render gate pass on BigBoy.
- Bluetooth device Actions update (2026-08-02): This Node Actions now reaches
  the existing typed BlueZ pairing-manager verbs for pair, connect, disconnect,
  trust/untrust, and forget. Provider-issued adapter/device targets and current
  bond/link state are revalidated before dispatch; pairing registers the typed
  agent, every verb requires visible confirmation, and refused writes preserve
  the last confirmed state. BigBoy action-contract and responsive fail-closed
  Actions gates pass; physical peripheral pairing proof remains hardware-gated.
- Power/session Actions update (2026-08-02): This Node Actions now exposes
  typed logind Lock, Suspend, Hibernate, Reboot, and Power Off rows from the
  fresh capability probe. Refused capabilities stay disabled with their policy
  label; host-down verbs require a second visible confirmation and provider
  errors never claim success. BigBoy action-contract and responsive fail-closed
  Actions gates pass; physical suspend/reboot proof remains hardware-gated.
- Display Actions update (2026-08-02): This Node Actions now exposes a typed
  connected-display output toggle when fresh display facts and the live seat
  layout agree on a target. The action reuses the System `DisplayLayout`
  last-console interlock, requires visible confirmation, and reports refused
  provider transitions without optimistic success. Typed-action and responsive
  fail-closed Actions gates pass on BigBoy.
- Audio Actions update (2026-08-02): This Node Actions now exposes a typed
  master playback mute toggle only when fresh audio facts and a live System
  mixer target agree. Confirmation precedes the existing `SetMixerMuted` seam;
  provider refusal preserves the last confirmed state and visible error.
- Audio volume Actions update (2026-08-02): This Node Actions now exposes
  bounded ten-percent master playback volume steps only when fresh audio facts
  and a live typed PipeWire mixer target agree. Confirmation precedes the
  existing `SetMixerVolume` seam; provider refusal preserves the last confirmed
  volume and visible error. BigBoy action-contract and fail-closed render gates
  pass.
- Audio capture/privacy update (2026-08-02): the typed PipeWire graph now folds
  `Stream/Input/Audio` nodes into a separate capture collection instead of
  treating microphone state as unavailable by construction. This Node Actions
  offers microphone mute only with fresh audio facts and a live capture target;
  it confirms before dispatching the existing typed mixer mute seam and preserves
  provider refusal. mde-seat capture folding and BigBoy action-contract and
  fail-closed Actions gates pass.
- Direct-seat input update (2026-08-02): the persisted This Node tap-to-click
  policy now crosses the shared runtime handoff into the DRM/libinput seat.
  Every touch-capable device is configured when libinput replays or adds it,
  using libinput's typed tap configuration rather than compositor-only state.
  The DRM-feature shell compile and fail-closed Actions gate pass on BigBoy;
  physical tap proof remains hardware-gated.
- Keyboard backlight provider update (2026-08-02): This Node now folds typed
  kernel `kbd_backlight` LED targets separately from LCD backlights and exposes
  bounded ten-percent keyboard brightness steps only when fresh input facts and
  a live provider target agree. Target/max validation, confirmation, refusal
  handling, and recovery metadata are preserved; physical keyboard proof
  remains hardware-gated.
- Pointer/touch Actions continuity update (2026-08-02): This Node Actions now
  reaches the existing persisted Mouse & Touch policy and native DRM/libinput
  handoff for bounded pointer-speed steps and tap-to-click changes. Fresh input
  observations gate both controls; each requires visible confirmation and keeps
  the direct-seat policy, audit, and recovery contract explicit. BigBoy typed
  action-contract and fail-closed Actions gates pass; physical touchpad proof
  remains hardware-gated.
- Touch/gesture Actions update (2026-08-02): This Node Actions now reaches the
  persisted direct-seat policy for touchscreen input, two-finger scrolling, and
  edge gestures through the same DRM/libinput handoff as System. Fresh input
  observations gate all three controls; each requires visible confirmation and
  retains typed audit/recovery metadata. BigBoy action-contract and fail-closed
  responsive render gates pass; physical touch/gesture proof remains
  hardware-gated.
- Power & battery Actions update (2026-08-02): This Node Actions now exposes
  bounded five-percent charge-stop cap steps only when fresh power facts and a
  live typed `charge_control_end_threshold` target agree. Confirmation precedes
  the existing typed sysfs seat write; provider refusal preserves the last
  confirmed cap and visible error. BigBoy action-contract and fail-closed
  Actions gates pass.
- NetworkManager provider update (2026-08-02): the seat now folds bounded
  NetworkManager Ethernet/Wi-Fi/cellular device state and the global Wi-Fi
  radio property through a typed system-D-Bus client. This Node Actions offers
  a Wi-Fi radio toggle only when fresh underlay facts and a live provider target
  agree; it requires confirmation, preserves mesh/profile safety boundaries,
  and surfaces provider refusal without optimistic success. The client rejects
  credential-shaped identifiers; the full mde-seat suite passes 101/101 and
  This Node action/fail-closed render gates pass on BigBoy.
- NetworkManager link-action update (2026-08-02): This Node Actions now exposes
  a confirmed disconnect for a live connected non-loopback underlay device,
  using the provider-issued NetworkManager device path and typed `Device.Disconnect`
  call. Mesh overlay targets are protected; profiles, credentials, DNS, and
  routes are untouched; provider refusal leaves the cache unchanged. BigBoy
  NetworkManager path/provider tests pass 3/3, with action-contract and
  fail-closed render gates passing.
- NetworkManager profile inventory update (2026-08-02): the typed local
  NetworkManager client now inventories `Settings.Connection` records through
  bounded provider paths, sanitized labels, and typed Ethernet/Wi-Fi/cellular/
  WireGuard/VPN families without exposing SSIDs, APNs, UUIDs, routes, DNS, or
  secrets. This Node Actions shows the inventory while keeping activation,
  APN/DNS/proxy changes, and imported VPN mutation unavailable until a typed
  SecretAgent provider supplies authorization, confirmation, and recovery.
  BigBoy mde-seat profile tests pass 4/4; action-contract and fail-closed
  responsive render gates pass.
- Display brightness Actions update (2026-08-02): This Node Actions now exposes
  bounded dim/brighten steps when fresh display facts and a live typed internal
  backlight or external DDC target agree. Each step shows the target and new
  percentage, requires confirmation, validates the provider-issued target and
  panel range, and retains the last confirmed value when the seat refuses a
  write. BigBoy action-contract and fail-closed responsive render gates pass.
- Display continuity remains intentionally split: the System surface retains
  the existing typed mode, refresh, and arrangement intent controls, while the
  unified Actions workflow currently exposes output enablement and brightness;
  live modeset application remains integration-gated by the DRM runner.
- Display mode Actions update (2026-08-02): This Node Actions now exposes the
  first connected output's live connector-advertised alternate modes through
  the existing typed `DisplayLayout` intent seam. Mode targets are revalidated
  against the current connector probe, require visible confirmation, and retain
  the runner-gated live-modeset limitation rather than claiming a false apply.
  BigBoy action-contract and fail-closed responsive render gates pass.
- Display arrangement Actions update (2026-08-02): This Node Actions now
  exposes confirmed left/right nudges for each enabled connected output through
  the existing provider-issued `MonitorId` and `DisplayLayout` intent seam.
  Multi-display and fresh-connector checks gate the controls; saved order is
  revalidated before dispatch and live DRM re-apply remains explicitly
  integration-gated. BigBoy action-contract and fail-closed responsive render
  gates pass.
- Hierarchy navigation update (2026-08-02): the inventory-first landing rows and
  expanded Device Manager hierarchy now provide real navigation into the
  existing durable `ThisNodeView::Detail` routes instead of being display-only
  summaries. Section-to-route resolution is bounded by the catalog, preserves
  the shared provider/health authority, and the focused This Node suite passes
  39/39 on BigBoy; unsupported providers remain visibly unavailable after
  navigation.
- Updates/lifecycle detail update (2026-08-02): the durable Updates & Lifecycle
  route now exposes a bounded installed-version versus newest-mesh-version
  posture with explicit current, available, stale, and unknown states. It does
  not turn mesh version comparison into a package-manager claim or enable unsafe
  update/reset controls; lifecycle mutation remains visibly provider-gated with
  authorization, confirmation, audit, and recovery requirements.
- Storage detail update (2026-08-02): the durable Storage route now renders the
  surveyed capacity-bar plus detail-table anatomy from bounded aggregate root
  and storage facts. Missing capacity remains unavailable, stale values are
  marked before action, and device identity/health are not fabricated from the
  credential-free snapshot boundary.
- Users/Accounts role view update (2026-08-02): the durable Users & Accounts
  route now presents a role-first posture grouping administrative policy groups,
  interactive users, and all accounts as separate bounded rows. It keeps
  usernames, membership, privilege names, and credentials out of the snapshot;
  identity mutations remain unavailable until a typed admin-authorized provider
  supplies confirmation, audit, and recovery.
- Camera inventory provider update (2026-08-02): the mesh-status writer now
  falls back to a bounded, fixed `/dev/video[0-9]*` character-device discovery
  when no explicit camera count provider is published. Only the count crosses
  the snapshot boundary; paths, names, capture permissions, and camera privacy
  state remain unavailable until a typed privacy provider publishes them.
- Audit export update (2026-08-02): the This Node Audit disclosure now offers
  an atomic export to the deterministic user-state directory using a bounded
  `mde.this-node.audit.v1` schema. The export contains only action, outcome,
  timestamp, and a fixed local trusted-session operator label; provider paths,
  device identities, credentials, and raw errors remain excluded, with write
  failures shown explicitly.
- Local storage detail update (2026-08-02): the trusted seat hardware provider
  now inventories at most 32 fixed-root `/sys/block` devices with bounded
  kernel names, capacity, removable, and rotational observations. Storage and
  Hardware details render the local table while the world-readable snapshot
  remains aggregate-only; destructive storage actions remain unavailable until
  a typed, confirmed provider exists. Hostile-name and bounded-provider tests
  cover the new seam.
- Firmware/dock inventory update (2026-08-02): the trusted seat hardware
  provider now reads bounded product/firmware labels from the fixed DMI class
  root and at most 16 sanitized Thunderbolt/dock authorization observations.
  Hardware detail renders these as local capability evidence; firmware updates
  and dock authorization remain visibly unavailable until typed privileged
  providers supply confirmation, safety, audit, and recovery contracts.
- Health-alert continuity update (2026-08-02): This Node now links node-sourced
  rows from the existing shared AlertInbox on the inventory landing view. It
  provides durable Acknowledge and 15-minute Snooze commands through the signed
  collab action path, with severity/source/detail display and explicit command
  errors; it does not create a second alert store. Focused This Node tests pass
  40/40 on BigBoy.
- Local underlay continuity update (2026-08-02): the Connectivity detail now
  renders the live trusted-seat NetworkManager probe beside mesh-status facts,
  including bounded Wi-Fi radio/link state and credential-free Ethernet, Wi-Fi,
  cellular, WireGuard, and VPN profile inventory. Provider loss is explicit;
  profile inventory is capped at 32 records and activation/secret-bearing
  changes remain behind the typed SecretAgent boundary. BigBoy network-provider
  tests pass 5/5 and focused This Node tests pass 40/40.
- Local Bluetooth continuity update (2026-08-02): the Connectivity detail now
  renders the trusted-seat BlueZ adapter and known-device projection beside
  aggregate mesh Bluetooth facts, including powered/scanning/pairable state,
  connection/pairing/trust state, and reported peripheral battery. BlueZ folds
  at most 8 adapters and 64 devices; provider loss remains explicit and all
  pair/connect/trust/forget/scan writes stay in the typed audited Actions path.
  BigBoy BlueZ provider tests pass 10/10 and focused This Node tests pass 40/40.
- Direct-seat input continuity update (2026-08-02): the Input detail now shows
  the persisted System/libinput policy beside typed keyboard-backlight facts:
  pointer speed, tap-to-click, two-finger scroll, touchscreen input, and edge
  gestures. The detail reads the same authority used by the native handoff;
  changes remain confirmation/audit-gated in Actions. Focused This Node tests
  pass 40/40 on BigBoy.
- Local power-provider continuity update (2026-08-02): the Power & Performance
  detail now keeps independent local power-profile and charge-limit probes
  visible alongside UPower battery/source facts. A missing battery provider no
  longer hides a live profile/charge-limit state; unavailable probes remain
  explicit, and profile/charge changes remain confirmation/audit-gated in
  Actions. Focused This Node tests pass 40/40 on BigBoy.
- Local privacy continuity update (2026-08-02): Security & Privacy now shows
  trusted local PipeWire capture routes and their actual mute state, while
  keeping route names local and camera permission explicitly unavailable until
  a typed camera-privacy provider exists. Microphone changes continue through
  the typed confirmation/audit Actions path. Focused This Node tests pass 40/40
  on BigBoy.
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
- Current state: `nav_bar.rs` owns persisted `Floating`/`Docked` placement,
  full-width 48px Bottom geometry, fixed 40px targets, 24px icons, 4px gaps,
  centered pins, overflow bounds,
  pin persistence, focus underlining, first-boot search, and tested
  slide/melt/Whimsy motion. Bottom reserves an animated clock/icon tray; Start uses
  Front Door search and the side retains its detailed top strip.
  `springboard.rs` is gesture-only: the retired App Grid painter,
  selection/open path, zoom ghost, and grid tests are deleted; 37 nav-bar plus
  14 shell/front-door tests pass in the earlier shell slice. The taskbar consumes the searchable catalog
  directly, with Fleet & Mesh/Workloads aliases and no retired group geometry;
  narrow first-boot controls now have 37/37 farm tests (farm `.90` slot 132),
  and first-boot selection now bounds persisted order, preserves intentional
  empty selections, and rejects duplicate/unsupported transient entries (six
  focused tests pass on farm `.50` slot 153). The focused workspace marker is now a centered 18x3
  accent with a 2px bottom gap; the latest aliases, Start/Search, first-boot,
  and singular-focus slice passes 43 focused nav-bar tests on farm `.90` slot
  `ux012-taskbar-focus-193`. In the Fleet & Mesh workspace, Workbench, Mesh Map, and Explorer are
  now selected through a checked View menu instead of a persistent top selector
  rail; farm compilation passed for `mde-shell-egui`. The latest focused taskbar
  slice covers aliases, Start/Search, first-boot selection, and singular focus
  underlining with 43 nav-bar tests passing on farm `.90` slot
  `ux012-taskbar-focus-193`. A fresh unsigned Alpha from integrated tip `b77a82de`
  is staged for Dell proofing; evidence is in `docs/ops/promotion-pipeline.md`.
  The latest farm review recompiles the shell and passes all 43 focused nav-bar
  tests on `.50` slot `ux012-nav-review-238`, including centered focus, bounded
  first-boot selection, catalog aliases, and full-width tray composition.
  Legacy pin aliases now migrate to canonical Fleet & Mesh and This Node pins
  while preserving order and intentional empty selections; 44 nav-bar tests
  pass on `.50` slot `ux012-nav-migration-245`.
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
