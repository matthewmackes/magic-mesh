# Platform Worklist

This is the only active platform worklist. Design notes, evidence ledgers,
runbooks, and operator notes are inputs, not parallel trackers. Historical
implementation diaries remain in docs/worklist-archive/ and are not executable
tasks.

## Current Snapshot - 2026-08-06 executable story rewrite

- **18 active epics:** 18 `Remaining`, 0 `Blocked`, 0 `Needs clarification`.
- **Execution order:** complete ARCH-010 stories in order; then consume its
  contracts in ARCH-008, ARCH-009, FUNC-019, FUNC-018, and FUNC-020. Run the
  vertical slices FUNC-011/FUNC-016, FUNC-017, FUNC-021, and FUNC-022 next. Integrate
  UX-009, UX-011, UX-012, and UX-013/014 at their named story gates. Close
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
- **Farm lock:** heavy verification is farm-only; route the longest job to
  BigBoy at 172.20.0.130, use explicit MCNF_BUILD_HOST and MCNF_BUILD_SLOT, and
  never run filler tests.
- **Rollout lock:** prove the release seat first, then Dell, Eagle, seat 15,
  T480, Surface, and three lighthouses. Publish the red AI-GENERATED-ALERT and
  wait five seconds before each seat mutation. Recover failures by re-enrollment
  and corrected-forward deployment, never rollback.
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

## Story execution contract

Every story below is a self-contained unit. The implementing agent must:
read the named inputs; change only the owned files; produce the named deliverable;
add the stated hostile or regression test; run the stated validation; record the
revision, command, result, and evidence path; and mark the story complete only
when the Done when condition is true. A passing compile without the named
behavioral evidence is not completion.

## Core Architecture


### WL-ARCH-010 - Make Workloads the sole VM/container runtime, readiness, and presentation authority

- Status: Remaining
- Priority: P0
- Complexity: Epic
- Problem: VM, container, session, console, and shell paths still publish or interpret overlapping lifecycle state; local attachment and capacity admission are not fully
  proven.
- Required outcome: One versioned, persisted, idempotent Workload operation API controls VM/container lifecycle. The reconciler is the only actuator, libvirt/virtqemud is
  the VM adapter, Quadlet/systemd is the container adapter, and the shell uses bounded typed projections. Local Display1/KMS attachment and RDP/SPICE/VNC recovery are
  tested.
- Current state: Typed contracts, journal retention, bounded readers, cancellation, Display1 seams, and several hostile farm tests exist. Workload cleanup now treats
  an already-stopped libvirt domain as an idempotent destroy/undefine boundary, and an independent live-proof helper validates the typed projection and refuses
  missing runtime evidence. Caller migration, real adapters, restart recovery, native KMS/EGL, packaging, and Dell/seat-15 proof remain.
- Remaining work:
- **Cleanup idempotence checkpoint (2026-08-06):** the sole libvirt actuator
  accepts absent/stopped-domain diagnostics during ordered cleanup while still
  refusing unrelated virtqemud failures. Workload `workload_compute` passed
  23/23 on `.90` in `workload-cleanup-idempotence-20260806-r1`.
  `docs/platform/evidence/WL-ARCH-010-2026-08-06-cleanup-idempotence-r1.md`.
- **Admission/live proof checkpoints (2026-08-06/09):** the strict helper validates typed placement, resources, retry, and lease safety. Dell was unreachable; seat 15 lacked
  a revision receipt, typed projection, operation, and attachment generation, so acceptance refused:
  `docs/platform/evidence/WL-ARCH-010-2026-08-06-admission-proof-r1.md`, `docs/platform/evidence/WL-ARCH-010-2026-08-09-dell-seat15-live-acceptance-r15.md`.
- **Native attachment route checkpoint (2026-08-09):** invalid container/protocol attachment fails before effects and live headless Service VMs emit no attachment;
  BigBoy passed 38/38 plus the reachable shell regression: `docs/platform/evidence/WL-ARCH-010-2026-08-09-native-attachment-route-r14.md`.
- **Console authority removal checkpoint (2026-08-08):** the raw console relay,
  cloud console dispatch, shell endpoint reader, obsolete Browser attach
  envelope, and matching live verifier were deleted. Typed Workload Open plus
  authenticated Display1 leases remain; fail-closed lint and focused BigBoy/
  `.90` gates pass. A follow-up bounded channel now lets only the Workload
  reconciler execute cold-migration VM effects; restart journaling remains.
  Evidence: `docs/platform/evidence/WL-ARCH-010-2026-08-08-console-authority-removal-r1.md`,
  `docs/platform/evidence/WL-ARCH-010-2026-08-08-migration-authority-r1.md`.
- **Shell runtime-projection hard cut (2026-08-08):** Console's raw Podman and
  libvirt inventory shortcuts and Datacenter's retired Nova-name heuristic were
  deleted. One typed Workloads link/projection remains; the strengthened
  authority guard and three focused BigBoy shell tests pass. Evidence:
  `docs/platform/evidence/WL-ARCH-010-2026-08-08-shell-runtime-projection-hard-cut-r4.md`.
- **Heartbeat runtime-projection hard cut (2026-08-08):** peer heartbeats no
  longer probe or replicate raw Podman/libvirt inventories. Remote VM desktop
  cards consume the serving node's validated typed Workload snapshot; rolling
  readers discard retired fields, and focused farm/authority gates pass.
  Evidence:
  `docs/platform/evidence/WL-ARCH-010-2026-08-08-heartbeat-runtime-projection-hard-cut-r5.md`.
- **Retired compute-inventory hard cut (2026-08-09):** network probing no longer reads the retired VM roster; typed Workloads owns runtime identity. Evidence:
  `docs/platform/evidence/WL-ARCH-010-2026-08-09-retired-compute-inventory-hard-cut-r6.md`.
- **Datacenter/XCP hard cut (2026-08-09):** VM actions/roster, both XCP workers/crate/topics, and Server/Hypervisor profiles were deleted; retained rows fail closed. Evidence:
  `docs/platform/evidence/WL-ARCH-010-2026-08-09-datacenter-xcp-authority-hard-cut-r7.md`.
- **Legacy compute-create hard cut (2026-08-09):** the orphan `compute/create/*` worker and direct `virt-install` path were deleted; typed Workloads owns create. Evidence:
  `docs/platform/evidence/WL-ARCH-010-2026-08-09-compute-provision-hard-cut-r8.md`.
- **Authority/contract hardening (2026-08-09):** lifecycle/provision bypasses were deleted; attachment identity, restart replay, and Display1 ownership fail closed. Evidence:
  `docs/platform/evidence/WL-ARCH-010-2026-08-09-contract-restart-display1-hardening-r12.md`.
- **Production compile checkpoint (2026-08-09):** cloud authorization exports are reachable outside tests; BigBoy's locked async-services library check passed:
  `docs/platform/evidence/WL-ARCH-010-2026-08-09-cloud-production-compile-r14.md`.
- **Migration journal checkpoint (2026-08-08):** reconciler-owned cold-
  migration commands are atomically journaled before effects, replay pending
  records after restart, clean applied records without repeating effects, and
  pace retryable recovery. Evidence:
  `docs/platform/evidence/WL-ARCH-010-2026-08-08-migration-journal-r2.md`.
- **Distributed migration recovery checkpoint (2026-08-08):** one bounded
  atomic authority now persists all migration cursors, admitted source/target
  and acknowledgement jobs, publish state, wall-clock deadlines, and retained
  relinquish/rollback retries before external effects. BigBoy passed 53/53.
  Live libvirt crash injection and seat lifecycle proof remain. Evidence:
  `docs/platform/evidence/WL-ARCH-010-2026-08-08-distributed-migration-ledger-r3.md`.
- **Contract duplicate-key checkpoint (2026-08-06):** recursive Workload JSON
  admission rejects duplicate top-level and nested keys; `.50` passed 9/9.
  Evidence: `docs/platform/evidence/WL-ARCH-010-2026-08-06-contract-duplicate-keys-r1.md`.
- **Display1 expiry checkpoint (2026-08-06):** lease expiry revokes readiness,
  relay state, and stale sockets; BigBoy passed 7/7. Evidence:
  `docs/platform/evidence/WL-ARCH-010-2026-08-06-display1-expiry-r1.md`.
- **Storage path-boundary checkpoint (2026-08-06):** apply-time virtual-storage
  validation rejects symlinks and outside-root image paths before executor use;
  `.90` passed 1/1. Evidence:
  `docs/platform/evidence/WL-ARCH-010-2026-08-06-storage-path-boundary-r1.md`.
- **Durable journal checkpoint (2026-08-06):** persisted Workload journals reject
  recursive duplicate JSON keys before replay; BigBoy passed 8/8 reconciler tests.
  Evidence: `docs/platform/evidence/WL-ARCH-010-2026-08-06-ledger-duplicate-keys-r1.md`.
- **Shell projection checkpoint (2026-08-06):** duplicate-key node projections
  fail closed; `.50` was ENOSPC during compile, so no pass is claimed. Evidence:
  `docs/platform/evidence/WL-ARCH-010-2026-08-06-shell-projection-duplicate-keys-r1.md`.
- **VDI reconnect checkpoint (2026-08-06):** generation-zero reconnect evidence
  is refused; BigBoy passed 1/1. Evidence:
  `docs/platform/evidence/WL-ARCH-010-2026-08-06-vdi-reconnect-generation-r1.md`.
- **Journal rollback checkpoint (2026-08-06):** failed atomic phase flushes roll
  back in-memory status; BigBoy passed 9/9. Evidence:
  `docs/platform/evidence/WL-ARCH-010-2026-08-06-ledger-flush-rollback-r1.md`.
- **Attachment generation checkpoint (2026-08-06):** stale lease generations
  are rejected; BigBoy passed 1/1. Evidence:
  `docs/platform/evidence/WL-ARCH-010-2026-08-06-status-attachment-generation-r1.md`.
- **Quadlet catalog checkpoint (2026-08-06):** Start/StartAndAttach validate a
  promoted non-empty OCI artifact before systemd or Display1; `.90` passed 2/2.
  Evidence: `docs/platform/evidence/WL-ARCH-010-2026-08-06-quadlet-catalog-r1.md`.
- **Cloud lifecycle retirement checkpoint (2026-08-06):** the cloud worker no
  longer classifies or dispatches legacy VM `instance-*` or container
  `container-*` lifecycle topics; the stale rootless systemd/journal adapter
  was removed. Front Door and Explorer publish typed Workload operations only;
  mackesd refusal coverage passed 6/6 plus 2/2 on BigBoy, and the shell
  lifecycle suite passed 27/27 on `.50` after `.90` ENOSPC rerouting.
  Evidence: `docs/platform/evidence/WL-ARCH-010-2026-08-06-cloud-lifecycle-retire-r1.md`.
- **Quadlet materialization and backend admission checkpoint (2026-08-06):**
  the sole typed container actuator now loads a missing approved OCI archive
  into local Podman storage, atomically installs a hashed-identity rootful
  Quadlet unit, reloads systemd, and removes the unit on destroy; runtime names
  cannot contain Workload `:` separators or collide by simple sanitization.
  BigBoy passed the focused materialization test 1/1. The typed contract now
  exposes separate VM/container storage pools and matching reservation
  admission; `.50` passed 2/2 backend-pool tests. The old cloud deploy handler
  refuses before staging/runner activity, and the shell container lens is
  preview-only; `.50` passed 4/4 IAC tests. The follow-up admission wiring
  below now connects the live reconciler to the backend-specific contract;
  live capacity and storage proof remain open. Evidence:
  `docs/platform/evidence/WL-ARCH-010-2026-08-06-quadlet-admission-r1.md`.
- **Backend admission and managed storage wiring checkpoint (2026-08-06):**
  `workload_compute` now partitions active reservations by VM versus Quadlet
  backend, probes separate live pools, and calls typed backend-specific
  admission. The rootful Quadlet image path and generated unit use the storage
  worker's managed `/var/lib/mde-vms/containers` subtree. Storage layout
  creation rejects symlink, escape, and non-directory substitutions. Browser
  Workload caller and activation/package contracts passed their typed-action
  checks. BigBoy passed the focused reconciler admission test 1/1 and `.90`
  passed the hostile storage-link test 1/1; the focused backend contract gate
  passed 12/12 on `.50`. This checkpoint does not
  claim live Dell/seat-15, native KMS/EGL, packaging-install, or restart proof.
  Evidence:
  `docs/platform/evidence/WL-ARCH-010-2026-08-06-admission-wiring-r1.md`.
- **Dell typed capacity-refusal checkpoint (2026-08-08):** Release 21 on Dell
  published one capability-bound Browser Standard `StartAndAttach` through the
  sole Workload operation lane. Live four-thread admission refused before any
  actuator attempt or Display1 lease, retained the Browser VM shut off, and
  published the expected typed failed state. OpenTofu, Ansible, libvirt, KVM,
  Podman, storage, shell, and all six workers passed the focused live verifier.
  The refusal's inaccurate same-profile remediation was corrected to recommend
  the Small profile and is covered by one focused farm test. A larger-seat
  successful first frame and the remaining lifecycle matrix remain. Evidence:
  `docs/platform/evidence/WL-ARCH-010-2026-08-08-dell-capacity-refusal-r1.md`.
- **Startup readiness checkpoint (2026-08-09):** a not-running VM awaiting guest readiness retries instead of falsely completing stopped; BigBoy passed 37/37:
  `docs/platform/evidence/WL-ARCH-010-2026-08-09-startup-readiness-fail-closed-r13.md`.
  1. S1 Inventory authorities and remove reachability.
     - Objective: enumerate every lifecycle publisher, projection writer, adapter, console reader, and direct shell/backend call.
     - Inputs: repository search, CI authority scan, current evidence.
     - Deliverable: checked-in inventory and negative tests for each retired topic/symbol.
     - Depends on: none.
     - Acceptance: every owner is unique and every retired path is absent or proven unreachable.
     - Validation: run lint-workload-authority, authority scan, and focused farm tests.
     - Done when: inventory, tests, hashes, and evidence file exist.
  2. S2 Freeze bounded Workload contracts.
     - Objective: finish versioned IDs, desired/observed states, operation status, generation, deadline, capacity, and attachment lease types.
     - Inputs: mackes-mesh-types cloud/workload modules and S1 inventory.
     - Deliverable: serde-bounded contracts plus hostile round-trip/schema-skew tests.
     - Depends on: S1.
     - Acceptance: unknown, oversized, stale, replayed, and unauthenticated inputs fail closed.
     - Validation: run focused mackes-mesh-types cargo tests on .50.
     - Done when: all contract tests pass and source hashes are recorded.
  3. S3 Make reconciliation restart-safe.
     - Objective: journal accepted requests before side effects and reconcile with CAS, idempotency, deadlines, cancellation, backoff, and bounded projections.
     - Inputs: S2 contracts and existing journal/reconciler.
     - Deliverable: one reconciler with crash/replay/capacity/cancellation state-machine tests.
     - Depends on: S2.
     - Acceptance: restart never duplicates a domain, container, lease, or side effect.
     - Validation: run workload_compute/reconciler tests on BigBoy.
     - Done when: hostile matrix passes and live recovery evidence is recorded.
  4. S4 Migrate real adapters and callers.
     - Objective: route VM effects only through libvirt/virtqemud and containers only through Quadlet/systemd; migrate Browser, App, Android, Service, and Workloads
       callers.
     - Inputs: S1 inventory, S2 contracts, ARCH-008/019 caller maps.
     - Deliverable: adapter implementations, migrated callers, and no competing worker/topic.
     - Depends on: S2 and S3.
     - Acceptance: one typed StartAndAttach path reaches ready or actionable failure.
     - Validation: run adapter fixtures, caller negative tests, and package checks on BigBoy.
     - Done when: old publishers/readers are deleted and migration evidence is signed.
  5. S5 Enforce admission and storage safety.
     - Objective: reserve bounded CPU/RAM/tasks/I/O and offer only fitting profiles; preview contiguous-space XFS creation without destructive partition changes.
     - Inputs: host capabilities, storage workers, Workload policy.
     - Deliverable: admission policy, UDisks2/parted preview, XFS/SELinux setup, refusal tests.
     - Depends on: S2.
     - Acceptance: unknown capacity, Lighthouse placement, oversubscription, shrink/move/format requests all fail closed.
     - Validation: run admission/storage hostile tests and Tofu/Quadlet/SELinux checks on .90.
     - Done when: safe preview and refusal evidence are present.
  6. S6 Implement authenticated native attachment.
     - Objective: transfer one-use Display1 DMA-BUF leases with peer credentials, nonce, generation, expiry, and SCM_RIGHTS into the existing KMS/EGL owner.
     - Inputs: Display1 broker, mde-egui DRM path, S2 lease contract.
     - Deliverable: FD broker, scanout/damage/input/audio/clipboard bridge, cleanup tests.
     - Depends on: S3 and S4.
     - Acceptance: unsupported formats, expired leases, device loss, resize, crash, and duplicate use clean every resource and never expose an FD on the Bus.
     - Validation: run Display1/DRM/VDI tests and native fixture on BigBoy.
     - Done when: zero-copy metrics and recovery evidence are recorded.
  7. S7 Replace provisioning UX and package policy.
     - Objective: make Workloads UI one typed Open/StartAndAttach stepper and package all adapter, slice, storage, SELinux, and socket policy.
     - Inputs: S4-S6, UX-009 primitives.
     - Deliverable: render-only UI, package units, upgrade cleanup, and GUI regression fixtures.
     - Depends on: S4, S5, S6.
     - Acceptance: UI performs no backend I/O and install/upgrade leaves no deleted process or stale lease.
     - Validation: run shell render, RPM payload, and systemd/package gates on .50.
     - Done when: clean install/upgrade evidence and fixture captures exist.
  8. S8 Prove live lifecycle and recovery.
     - Objective: exercise cold/warm start, native frame/input/audio/clipboard, remote recovery, container health, stop/restart, suspend/rejoin, reboot, and
       corrected-forward upgrade.
     - Inputs: completed S1-S7 and release revision.
     - Deliverable: five-seat and three-lighthouse evidence bundle.
     - Depends on: S7, CRIT-006, CRIT-007.
     - Acceptance: Dell and seat 15 pass first; every required seat and lighthouse rejects unsafe placement and recovers.
     - Validation: farm release gates plus named live-seat commands.
     - Done when: all required artifacts and limitations are recorded; unresolved hardware keeps Status Remaining.
- Scope: Owns Workload contracts, reconciler, adapters, readiness, admission, storage, attachment, presentation, provisioning UX, packaging, and integrated proof. Browser
  guest internals, generic worker runtime, health modal, and release authority are owned elsewhere.
- Relevant files/components: crates/mesh/mackes-mesh-types, crates/mesh/mackesd workers/cloud, mde-shell-egui/iac and vdi, mde-egui/drm, libvirt/OpenTofu,
  Quadlet/systemd, storage and SELinux packaging.
- Dependencies: ARCH-008, ARCH-009, FUNC-019, UX-009, CRIT-006, and CRIT-007 consume this API; none may add a lifecycle or console authority.
- Acceptance criteria:
  1. Duplicate/replayed/stale/deadline/cancelled operations are deterministic and side-effect safe.
  2. Capacity and Lighthouse placement fail closed; no four-thread host receives four guest vCPUs.
  3. Native and recovery transports pass frame, input, audio, clipboard, reconnect, resize, and cleanup tests.
  4. The shell only sends typed intent and renders bounded state.
- Verification method: run lint-workload-authority first; use @farm:{cargo test -p mackes-mesh-types}
  @farm:{cargo test -p mackesd workload_compute}
  @farm:{cargo test -p mde-shell-egui --features live-vdi}
  and BigBoy release/package gates with explicit host and slot; capture live evidence on required seats.
- Origin or merged source IDs: Job One 2026-08-05; archived ARCH-006/007, CRIT-001; VDI zero-copy design; current Dell/seat-15 incidents.

### WL-ARCH-008 - Extract the host Browser stack and replace it with a VM Browser

- Status: Remaining
- Priority: P0
- Complexity: Epic
- Problem: CEF/Servo host Browser code, frame copies, helpers, and package seams still compete with the DRM shell and violate the VM-only application boundary.
- Required outcome: Preserve the old stack with history in matthewmackes/magic-mesh-browser-stack, remove it from magic-mesh, and make Surface::Browser start/resume a
  browser-vm that renders guest Chromium over VDI with focused input and guest-owned chrome.
- Current state: The standalone repository and clean-checkout CI pass and the typed Browser workload path exists; portable live data import, guest image quality,
  audio, and five-seat performance proof remain.
- **Portable migration checkpoints (2026-08-06):** deterministic allowlist, idempotency, symlink, and secret boundaries passed BigBoy and `.50`:
  `docs/platform/evidence/WL-ARCH-008-2026-08-06-portable-profile-r1.md`, `docs/platform/evidence/WL-ARCH-008-2026-08-09-portable-manifest-identity-r2.md`.
- **Display1 migration rollback checkpoint (2026-08-09):** fixed-target cutover restores exact original XML after failed validation; `.90` gates passed:
  `docs/platform/evidence/WL-ARCH-008-2026-08-09-display1-migration-rollback-r3.md`.
- **Host Browser negative-boundary checkpoint (2026-08-08):** host engine/package policy was removed; boundary lint, metadata, and 11/11 `.90` tests pass:
  `docs/platform/evidence/WL-ARCH-008-2026-08-08-host-browser-negative-boundary-r1.md`.
- **Standalone publication checkpoint (2026-08-08):** GitHub `main` is `2b36cedb`; farm and Actions `31277690513` passed worker/clippy/boundary/package jobs:
  `docs/platform/evidence/WL-ARCH-008-2026-08-08-standalone-publication-s1-r1.md`.
- **Live-profile inventory checkpoint (2026-08-09):** accessible seats had no legacy profile; raced sources and failed entries now refuse partial publication while
  credential stores remain untouched. BigBoy passed: `docs/platform/evidence/WL-ARCH-008-2026-08-09-live-profile-inventory-r4.md`.
- Remaining work:
  1. S1 Preserve history and build the standalone repository.
     - Objective: publish a clean clone containing every old Browser source, asset, policy, unit, document, and relevant history.
     - Inputs: current repo commit, Browser inventory, licenses.
     - Deliverable: repository, provenance record, workspace/lockfiles, CI, and clean-clone build log.
     - Depends on: ARCH-010 S2.
     - Acceptance: no path/submodule/Git dependency points back to magic-mesh.
     - Validation: clean clone cargo build/test on BigBoy.
     - Done when: immutable revision and evidence hash are recorded.
  2. S2 Migrate portable Browser data safely.
     - Objective: inventory and idempotently import/export profiles, bookmarks, history, sessions, downloads, policies, and extensions without exposing secrets.
     - Inputs: legacy profile locations and guest image contract.
     - Deliverable: migration tool with imported/skipped/failed counts and redacted fixtures.
     - Depends on: S1.
     - Acceptance: downloads survive; cookies, passwords, passkeys, and sealed credentials never export silently.
     - Validation: migration unit/property tests and secret scan.
     - Done when: two consecutive migrations produce the same result.
  3. S3 Remove host Browser production seams.
     - Objective: delete host crates, workers, engines, package variants, installers, policies, units, and active docs.
     - Inputs: S1 inventory and S2 migration.
     - Deliverable: source/package deletion plus negative reachability scan.
     - Depends on: S1, S2.
     - Acceptance: no mde-web, CEF/Servo host engine, Browser helper, or Browser RPM is reachable.
     - Validation: workspace/package/architecture/supersession gates.
     - Done when: scan is clean in a fresh checkout.
  4. S4 Build and integrate browser-vm.
     - Objective: create the 4-vCPU/8-GiB/64-GiB baseline image and typed Workload profile with Chromium, GPU/video, PipeWire, guest agents, RDP preferred, Sunshine
       alternate, and host_browser=false.
     - Inputs: ARCH-010 adapter/readiness contracts and image builder.
     - Deliverable: reproducible image/profile and readiness fixture.
     - Depends on: S3 and ARCH-010 S4.
     - Acceptance: start/resume exposes the advertised desktop source or an actionable failure.
     - Validation: image/package and Workload tests on BigBoy.
     - Done when: profile hash and readiness evidence exist.
  5. S5 Replace shell Browser with VDI controller.
     - Objective: preserve Construct navigation, focused input, clipboard, source selection, reconnect, and preference without guest chrome mirroring.
     - Inputs: S4, VDI contract, UX-009.
     - Deliverable: controller and render/input/audio regression fixtures.
     - Depends on: S4.
     - Acceptance: switching transport preserves the VM and never silently changes preference.
     - Validation: shell live-vdi cargo tests and rendered captures.
     - Done when: no host helper process exists during the proof.
  6. S6 Prove quality and upgrade behavior.
     - Objective: verify five-tab cadence, damage uploads, navigation latency, guest audio, install/upgrade cleanup, and corrected-forward recovery.
     - Inputs: S1-S5 and release artifacts.
     - Deliverable: timestamped 15-minute metrics, audio proof, RPM proof, and five-seat captures.
     - Depends on: S5, CRIT-006, CRIT-007.
     - Acceptance: >=30 FPS visible target, no unexplained >500ms stall, navigation p95 <=100ms, and no secret/data loss.
     - Validation: farm standalone/magic-mesh gates and live seat commands.
     - Done when: all measurements and unavailable hardware are honestly recorded.
- Scope: Owns old-stack preservation, migration, host removal, Browser VM image/workload, shell VDI behavior, packaging, and proof. Guest Chromium UI and generic Workload
  lifecycle are out of scope.
- Relevant files/components: root manifests, mde-shell-egui web/vdi, old mde-web crates/workers, Browser packaging, image-build, VDI, and sibling browser repository.
- Dependencies: ARCH-010 is blocking; FUNC-016 owns VDI clipboard; UX-009 owns Construct connection/error styling.
- Acceptance criteria:
  1. Old Browser stack builds from its clean standalone clone and is absent from production magic-mesh.
  2. Browser opens the same guest session over RDP/Sunshine with focused input, audio, clipboard, reconnect, and no host engine.
  3. Five-tab performance, package cleanup, and data migration meet the stated thresholds.
- Verification method: standalone and root cargo gates, architecture/secret/package gates, and live video/audio/latency captures on named seats; put the longest build on
  BigBoy.
- Origin or merged source IDs: 2026-07-28 Option 3; archived WL-PERF-003, FUNC-001..004, ARCH-005; browser-perf-native design.

### WL-ARCH-009 - Process-isolated mackesd and unified Workers interface

- Status: Remaining
- Priority: P0
- Complexity: Epic
- Problem: mackesd remains monolithic, worker ownership and resource budgets are incomplete, and duplicate This Node/Fleet/State surfaces obscure runtime truth.
- Required outcome: six independently supervised mackesd groups publish bounded typed runtime snapshots; one Surface::Workers owns worker tree, graph, inspector, Network
  Operations, and staged Action Console; old surfaces and health duplication are removed.
- Current state: all 145 production starts have bounded runtime contracts; six grouped services ship, but complete ownership, providers, UI cutover, and fleet proof remain.
- **SQLite authority complete (2026-08-08):** migrations reduced 61 direct writes to zero; final host/job and process-owner proof passed 24/24, and the empty baseline is enforced:
  `docs/platform/evidence/WL-ARCH-009-2026-08-08-sqlite-authority-zero-r11.md`.
- **Action Console checkpoints (2026-08-08/09):** authenticated generation-bound Preview/Commit/Cancel and canonical digest recomputation fail closed; `.50`/`.90` passed:
  `docs/platform/evidence/WL-ARCH-009-2026-08-08-workers-action-console-s5-r1.md`, `docs/platform/evidence/WL-ARCH-009-2026-08-09-action-console-digest-binding-r8.md`.
- **Runtime census checkpoint (2026-08-09):** any uncensused supervisor worker now refuses the unified projection without advancing generation; BigBoy passed 15/15:
  `docs/platform/evidence/WL-ARCH-009-2026-08-09-unregistered-runtime-refusal-r4.md`.
- **Canonical registry census (2026-08-09):** all reachable starts have one registry row and stable complete-field hash; ansible-pull configuration/cadence moved from
  parallel spawn logic into that authority. BigBoy passed focused census/hostile tests: `docs/platform/evidence/WL-ARCH-009-2026-08-09-registry-census-r9.md`.
- Remaining work:
- **Grouped crash-isolation checkpoint (2026-08-08):** Release 21 proved that
  `Requires=` edges cascaded one integrations crash through all six groups.
  Release 23 replaces grouped ownership edges with ordered `Wants=`, rejects
  regressions in the process-boundary validator, and restarts an already-active
  target during RPM upgrade. Seat 15 proved isolated integrations and control
  crashes while every unaffected PID and restart counter remained unchanged;
  target, mesh-health, and RPM verification stayed healthy. Dell was offline
  for corrected-package deployment. Evidence:
  `docs/platform/evidence/WL-ARCH-009-2026-08-08-group-crash-isolation-r2.md`.
- **Live cgroup-enforcement checkpoint (2026-08-08):** Release 23 on seat 15
  placed all six active groups in distinct cgroup-v2 paths whose effective
  memory, CPU, task, and I/O values matched the package. A bounded transient
  128 MiB allocation under a 16 MiB/no-swap boundary was OOM-killed exactly at
  16 MiB; cleanup left the target and every group active. Evidence:
  `docs/platform/evidence/WL-ARCH-009-2026-08-08-live-cgroup-enforcement-r3.md`.
- **Optional-worker quiescence checkpoint (2026-08-08):** an Android catalog
  importer and Flatpak app catalog without local trust anchors now sleep solely
  on shutdown instead of waking every second. Machine 9 proved no Bus state
  creation and prompt cancellation; the target-file format gate passed. Other
  optional providers still require audit. Evidence:
  `docs/platform/evidence/WL-ARCH-009-2026-08-09-registry-app-quiescence-r6.md`.
- **App-sync quiescence checkpoint (2026-08-09):** an absent shared probe
  inventory now leaves the optional media app-sync provider waiting solely for
  shutdown, without client-state creation or a polling timer; `.50` passed 9/9:
  `docs/platform/evidence/WL-ARCH-009-2026-08-09-app-sync-quiescence-r7.md`.
- **Responder group-isolation checkpoint (2026-08-09):** all 20 raw responder
  and maintenance threads now fail closed outside the process group assigned by
  the canonical registry. Exact/hostile argv and bidirectional registry guards
  passed 4/4 focused farm tests. Live package/cgroup census remains. Evidence:
  `docs/platform/evidence/WL-ARCH-009-2026-08-09-responder-group-isolation-r5.md`.
- **Workers navigation and clock checkpoint (2026-08-07):** `Surface::Workers`
  is now the canonical node-management route; Fleet & Mesh, This Node,
  System, Storage, About, and Phones deep links normalize into it. Phones is a
  Workers → Phones subtab and is absent from the launcher and pin catalog.
  Eastern current and retained timestamps now apply the daylight-saving offset.
  Focused farm route gates passed; the full shell suite passed 1,453 tests with
  five unrelated pre-existing pixel/IaC failures. Evidence:
  `docs/platform/evidence/WL-ARCH-009-2026-08-07-workers-phones-clock-r1.md`.
- **Current release-5 clock binding (2026-08-07):** the fresh artifact hash
  `8219d399ae7abf498f4916c9c43240628bbef02e9ef71971d235db3ada450be3` is
  installed and clean on Dell and seat 15; the Eastern DST regression passed
  1/1 on `.90`. Evidence: `evidence/WL-ARCH-009-2026-08-07-current-clock-r1.md`.
- **Runtime schema checkpoint (2026-08-06):** worker contract, relation,
  timeline, snapshot, change-set request, and change-set result tests reject
  unknown schema versions before admission; `.90` passed 8/8. Evidence:
  `docs/platform/evidence/WL-ARCH-009-2026-08-06-runtime-schema-r1.md`.
  1. S1 Complete the worker registry.
     - Objective: give every spawned worker one canonical ID, group, role, owner, relation, cadence, budget, output, and cleanup policy.
     - Inputs: spawn.rs, worker_role.rs, live unit inventory.
     - Deliverable: bidirectional registry/spawn drift guard and generated inventory.
     - Depends on: none.
     - Acceptance: no exception, duplicate, or unowned worker remains.
     - Validation: registry and drift cargo tests on .50.
     - Done when: inventory hash and negative test evidence are recorded.
  2. S2 Freeze runtime contracts and bounded snapshots.
     - Objective: version WorkerContract, runtime state, relations, timeline, change-set, redaction, freshness, and generation rules.
     - Inputs: S1 and mesh types.
     - Deliverable: bounded credential-free contracts and hostile schema tests.
     - Depends on: S1.
     - Acceptance: unknown versions, stale data, oversized events, and secrets fail closed.
     - Validation: shared-contract cargo tests on .90.
     - Done when: contract evidence is signed.
  3. S3 Assign all node/provider ownership.
     - Objective: map host, desktop, hardware, storage, services, lifecycle, recovery, backup, and virtualization facts/actions to real workers.
     - Inputs: UX-011 provider inventory and current This Node routes.
     - Deliverable: one owner per entity, observation, action, and publication.
     - Depends on: S1, S2.
     - Acceptance: no generic shell branch or duplicate state writer remains.
     - Validation: ownership scan and provider tests.
     - Done when: every capability has a worker and evidence.
  4. S4 Split and isolate the runtime.
     - Objective: ship mackesd-control, observation, actions, data, compute, integrations, and mackesd.target with one SQLite writer.
     - Inputs: S1-S3, ARCH-010 process/resource contracts.
     - Deliverable: units, RPM policy, shutdown, queue, retry, watchdog, and cgroup tests.
     - Depends on: S3.
     - Acceptance: group crash is isolated; optional unconfigured workers quiesce; no monolith ships.
     - Validation: process/chaos/resource cargo tests and package gate on BigBoy.
     - Done when: all six groups start/stop/recover under declared budgets.
  5. S5 Implement Workers and Action Console.
     - Objective: provide synchronized tree/graph/inspector, filters, device inventory, staged preview/commit/cancel, audit, and partial-failure reporting.
     - Inputs: S2-S4 and UX-009.
     - Deliverable: one responsive Surface::Workers and typed action model.
     - Depends on: S4.
     - Acceptance: no page, raw command, arbitrary path, or worker bypasses the console.
     - Validation: shell model/render and action-auth tests.
     - Done when: wide/narrow/largest-text captures and hostile action evidence exist.
  6. S6 Add Network Operations and cut over routes.
     - Objective: implement typed geo/fabric/flow/history projections and remove old This Node/Fleet/State surfaces while keeping Health modal separate.
     - Inputs: FUNC-017 providers, UX-011, health boundary.
     - Deliverable: deterministic graph/time lens, alias map, deleted old routes/docs/tests.
     - Depends on: S3-S5.
     - Acceptance: one Workers destination owns each legacy alias; no health grade/badge lives in Workers.
     - Validation: route/geo/history cargo tests and supersession scan.
     - Done when: source, package, runtime, navigation, and help scans are clean.
  7. S7 Prove fleet isolation and convergence.
     - Objective: run crashes, provider loss, saturation, stale snapshots, staged change, forced partial failure, and corrected-forward recovery.
     - Inputs: S4-S6 and CRIT-006/007.
     - Deliverable: five-workstation/three-lighthouse evidence bundle.
     - Depends on: S6.
     - Acceptance: bounded redacted snapshots converge without secrets or legacy fallback.
     - Validation: farm chaos/package gates and live captures.
     - Done when: every required failure matrix row has evidence.
- Scope: Owns registry/contracts, six services, budgets, snapshots, Workers UI, Network Operations, Action Console, route deletion, packaging, and fleet proof. Workload
  lifecycle, health modal, and provider implementation remain owned elsewhere.
- Relevant files/components: mackesd spawn/worker_role, mesh types, process units/RPM, mde-shell-egui Workers/routes, provider workers, and Network Operations design.
- Dependencies: ARCH-010, UX-009, UX-011, FUNC-017, CRIT-006, and CRIT-007.
- Acceptance criteria:
  1. Registry/spawn drift tests prove exactly one owner for every worker and capability.
  2. Six groups run under budgets with bounded credential-free snapshots and one SQLite writer.
  3. Workers and Action Console are the only node-management surfaces; Health remains a separate modal.
  4. Fleet chaos and five-seat/three-lighthouse evidence passes.
- Verification method: registry, contract, process/chaos, action-auth, route/render, package, format, and live fleet cargo gates; longest job on BigBoy.
- Origin or merged source IDs: 2026-08-01 process isolation evaluation; 2026-08-03 Workers merge survey; 2026-08-04 Network Operations directive.

### WL-FUNC-011 - Build the native Mesh Collaboration Suite and hard-cut legacy collaboration

- Status: Remaining
- Priority: P0
- Complexity: Epic
- Problem: Collaboration is split across legacy Chat, Teams rails, text-only clipboard, duplicate Files/transfers, incomplete Calls media, and an App-VM office path.
- Required outcome: one egui-native Collaboration surface has exactly Alerts, Chat, Calls, Files, Editor, and Clipboard; durable signed transport, real media, native
  office editing, and one executor replace all retired paths.
- Current state: signed envelopes, projections, native Editor foundation, POSIX/CAS Files transfer, and shell mounting exist; Calls providers, cross-node executors,
  office transport, canonical Alerts, migration, and hard cut remain.
- **Transfer executor checkpoints (2026-08-09):** only Local/Copy is admitted; Clipboard names its missing profile/Files/session/generation authority and refuses early.
  `.50` passed 2/2 plus 1/1: `docs/platform/evidence/WL-FUNC-011-2026-08-09-transfer-executor-r7.md`, `docs/platform/evidence/WL-FUNC-011-2026-08-09-transfer-executor-r8.md`.
- **Hard-cut/atomicity checkpoints (2026-08-09):** retired collaboration routes fail closed, and failed SQLite projection preserves clocks/state; BigBoy and `.50` passed:
  `docs/platform/evidence/WL-FUNC-011-2026-08-09-legacy-route-admission-r2.md`, `docs/platform/evidence/WL-FUNC-011-2026-08-09-collab-projection-atomicity-r3.md`.
- **Live-event lane identity checkpoint (2026-08-09):** signed envelopes merge only on an exact space/actor Bus lane; mismatches fail closed. Machine 193 passed 1/1:
  `docs/platform/evidence/WL-FUNC-011-2026-08-09-live-event-lane-identity-r4.md`.
- **Native-office admission checkpoint (2026-08-09):** office containers no longer fall through to lossy text editing; unsafe paths and the absent non-VCL adapter fail
  closed without opening or changing bytes. BigBoy passed 5/5: `docs/platform/evidence/WL-FUNC-011-2026-08-09-native-office-admission-r5.md`.
- **Calls provider lifecycle checkpoint (2026-08-09):** media effects refuse without a compatible provider; cleanup stays available and readiness is re-probed.
  Machine 9 passed 4/4; no production provider is registered: `docs/platform/evidence/WL-FUNC-011-2026-08-09-calls-provider-lifecycle-r6.md`.
- Remaining work:
  1. S1 Reconcile parity and contracts.
     - Objective: map every legacy command, route, state writer, package, and workflow to one of six sections or retirement.
     - Inputs: current parity ledger, collab types/core, archived IDs.
     - Deliverable: versioned bounded contracts for alerts, chat, calls, files, clipboard, editor, office, and transfer.
     - Depends on: none.
     - Acceptance: no conflicting Teams/channel/task/Discord/AI/App-VM office decision remains active.
     - Validation: schema/property/signature cargo tests on .90.
     - Done when: parity map and contract evidence exist.
  2. S2 Ship the six-section shell.
     - Objective: replace Communications and nested rails with one responsive Collaboration surface and contextual settings.
     - Inputs: S1 and UX-009.
     - Deliverable: six-section render/model with stable context header and route tests.
     - Depends on: S1.
     - Acceptance: Transfers is Files content; no legacy route or duplicate notification surface is reachable.
     - Validation: shell render/navigation cargo tests.
     - Done when: Dark/Light/narrow/largest-text captures pass.
  3. S3 Complete signed Chat and Alerts.
     - Objective: deliver durable direct/group chat, threads, attachments, offline replay, local find, delivery state, and canonical alert projection.
     - Inputs: collab event core and signed envelopes.
     - Deliverable: daemon authority, migration importer, and redacted alert/chat fixtures.
     - Depends on: S1, S2.
     - Acceptance: replay, attribution, ordering, and deduplication remain deterministic offline.
     - Validation: collab-core hostile and migration tests on .50.
     - Done when: legacy Chat worker/state/renderer is unreachable.
  4. S4 Complete Calls media and control.
     - Objective: provide direct/group media, provider-neutral SIP gateways, screen share, consented control, reconnect, and mute policy.
     - Inputs: media registry, SIP/RTP pieces, signed control grants.
     - Deliverable: real provider adapters and session lifecycle tests.
     - Depends on: S1, S2.
     - Acceptance: no empty provider or fake connected state; consent and revocation are auditable.
     - Validation: media/provider cargo tests and live call fixture.
     - Done when: provider availability and failure are visible.
  5. S5 Complete Files and transfer execution.
     - Objective: make Files the browser and CAS-backed transfer hub with seven typed executors, safe conflict policy, progress, cancel, retry, and cross-node
       acknowledgement.
     - Inputs: FUNC-016 and existing TransferJobV2/CAS.
     - Deliverable: executor registry, destination-generation commit, and end-to-end transfer evidence.
     - Depends on: S1, S2, FUNC-016 S1-S3.
     - Acceptance: bytes are immutable, destination commits are authenticated, and partial failure is honest.
     - Validation: transfer/CAS cargo tests on BigBoy and cross-node fixture.
     - Done when: all executor rows pass or are explicitly blocked by a named provider.
  6. S6 Complete native Editor and office sessions.
     - Objective: ship Text/Code, Document, Spreadsheet, and Presentation with sandboxed LibreOfficeKit, no VCL/App VM/compositor.
     - Inputs: existing Editor, office session contract, package policy.
     - Deliverable: native session adapter, autosave/recovery, and format fixtures.
     - Depends on: S1, S2.
     - Acceptance: open/edit/save/recover works with bounded files and no host process escape.
     - Validation: editor/office cargo and package tests on BigBoy.
     - Done when: all four kinds have render and persistence evidence.
  7. S7 Integrate Clipboard and hard-cut legacy products.
     - Objective: mount FUNC-016 rich clipboard, migrate useful data, and delete retired Chat/Teams/Files/Editor/Notification paths.
     - Inputs: S2-S6, FUNC-016.
     - Deliverable: migration report, negative scans, and one release surface.
     - Depends on: S3-S6.
     - Acceptance: six sections are the only primary collaboration navigation and no duplicate authority remains.
     - Validation: architecture, secret, supersession, package, and shell gates.
     - Done when: fresh checkout has no retired production route/worker/package.
  8. S8 Prove collaboration release.
     - Objective: run offline/online, permission, media, transfer, editor, clipboard, migration, recovery, and five-seat live acceptance.
     - Inputs: S1-S7, CRIT-006.
     - Deliverable: signed evidence bundle and visual captures.
     - Depends on: S7.
     - Acceptance: missing external media or hardware is recorded as a blocker, never a pass.
     - Validation: farm workspace gates and named live-seat proofs.
     - Done when: release evidence lists every section and failure path.
- Scope: Owns the one Collaboration surface, signed contracts, Chat, Calls, Files executors, Editor/office, Alerts, migration, packaging, and hard cut. Clipboard
  transport details belong to FUNC-016.
- Relevant files/components: mde-collab-types, mde-collab-core, mackesd collaboration/files workers, mde-shell-egui collaboration/editor, LibreOfficeKit packaging, and
  transfer/CAS modules.
- Dependencies: FUNC-016, ARCH-009, ARCH-010, UX-009, CRIT-006, and CRIT-007.
- Acceptance criteria:
  1. Exactly six primary sections exist and all retired collaboration surfaces are unreachable.
  2. Signed offline replay, real media, CAS transfer, native office, and rich clipboard pass focused hostile tests.
  3. Five-seat release proof records real providers, partial failures, and corrected-forward recovery.
- Verification method: collab/file/editor/media cargo suites, architecture/secret/package gates, visual captures, and live provider tests; route long jobs to BigBoy.
- Origin or merged source IDs: NOTIFY-CHAT, EDITOR-*, FILEMGR-*, TEAMS-*, CLIPBOARD-*, VOICE-*; 2026-08-03 Mesh Collaboration survey.

### WL-FUNC-016 - Native rich clipboard across the DRM seat, mesh, and VDI

- Status: Remaining
- Priority: P0
- Complexity: Epic
- Problem: Clipboard is text-only and cannot safely negotiate rich MIME payloads across local seat, mesh peers, and guest VDI.
- Required outcome: one versioned bounded clipboard contract supports text, HTML, images, files, and typed metadata through direct DRM, authenticated mesh, and VDI paths
  with explicit permission, limits, and cleanup.
- Current state: bounded rich contracts and DRM/mesh/VDI scaffolding exist; live adapters, permissions, cleanup, and proof remain.
- **S1 rich contract (2026-08-08):** V2 offers, generations, secret policy, and denials passed 72/72: `docs/platform/evidence/WL-FUNC-016-2026-08-08-rich-contract-s1-r1.md`.
- **S3 mesh xproc (2026-08-09):** Persist/SQLite/CAS/replay passed 1/1; live nodes remain: `docs/platform/evidence/WL-FUNC-016-2026-08-09-mesh-cross-process-r11.md`.
- **S2 DRM authority (2026-08-08/09):** seat authority passed 19/19; focus-bound asynchronous paste expiry passed 12/12; live proof remains:
  `docs/platform/evidence/WL-FUNC-016-2026-08-08-drm-clipboard-authority-s2-r1.md`, `docs/platform/evidence/WL-FUNC-016-2026-08-09-drm-paste-ownership-r10.md`.
- **VDI transport/permission checkpoints (2026-08-08):** bounded VNC/RDP/SPICE text, one-use/replay/revocation, modal, and redacted audit passed; live guest proof remains:
  `docs/platform/evidence/WL-FUNC-016-2026-08-08-vdi-clipboard-transport-s4-r1.md`, `docs/platform/evidence/WL-FUNC-016-2026-08-08-permission-audit-model-s5-r1.md`.
- **VDI admission checkpoints (2026-08-09):** metadata bounds passed 31/31, lease-capped permission passed 12/12, and post-materialization cancellation passed 13/13:
  `docs/platform/evidence/WL-FUNC-016-2026-08-09-vdi-metadata-admission-r6.md`; `docs/platform/evidence/WL-FUNC-016-2026-08-09-vdi-lease-expiry-r7.md`;
  `docs/platform/evidence/WL-FUNC-016-2026-08-09-materialization-cancellation-r8.md`.
- **Mesh CAS admission (2026-08-09):** Files-backed offers bind source projection and exact canonical bytes; missing bytes defer, while mismatch, duplicate JSON, replay, and
  Files-topic floods fail closed. BigBoy passed 8/8 plus 1/1: `docs/platform/evidence/WL-FUNC-016-2026-08-09-mesh-cas-admission-s3-r9.md`.
- Remaining work:
  1. S1 Define the rich contract.
     - Objective: version MIME offers, selection, payload limits, origin, expiry, generation, and denial reasons.
     - Inputs: collab types and existing clipboard v2.
     - Deliverable: serde-bounded contract and hostile payload tests.
     - Depends on: none.
     - Acceptance: oversized, unknown, stale, secret-bearing, and unsupported payloads fail closed.
     - Validation: shared-contract cargo tests on .50.
     - Done when: contract hash and fixtures are recorded.
  2. S2 Implement local DRM ownership.
     - Objective: connect egui/DRM selection and shortcuts to one clipboard authority without blocking render.
     - Inputs: mde-egui DRM/input and shell clipboard bridge.
     - Deliverable: local provider, paste/copy tests, bounded selection cache.
     - Depends on: S1.
     - Acceptance: focus, cut/copy/paste, ownership loss, and app switch preserve correct MIME.
     - Validation: mde-egui and shell clipboard cargo tests.
     - Done when: local render and shortcut evidence passes.
  3. S3 Implement authenticated mesh transport.
     - Objective: replicate permitted rich payloads over mde-bus with peer identity, expiry, size caps, and no raw paths.
     - Inputs: mde-bus, peer auth, transfer CAS.
     - Deliverable: sender/receiver adapter with deduplication and cleanup.
     - Depends on: S1.
     - Acceptance: unauthorized peers, replay, flood, and unavailable peers are bounded and honest.
     - Validation: bus/property/security cargo tests on .90.
     - Done when: cross-node fixture records exact bytes and denial reasons.
  4. S4 Implement VDI guest transport.
     - Objective: bridge negotiated clipboard to Browser/App/Workload guests through typed VDI messages.
     - Inputs: Workload attachment, VDI session, S1 contract.
     - Deliverable: guest adapter with reconnect and lease-expiry behavior.
     - Depends on: S1, ARCH-010 S6.
     - Acceptance: guest cannot access host secrets or unapproved MIME; reconnect never duplicates payload.
     - Validation: VDI cargo tests and live guest fixture on BigBoy.
     - Done when: all supported MIME types have evidence.
  5. S5 Integrate UI permissions and release proof.
     - Objective: expose user-visible source/target, approval, progress, and failure without a second clipboard store.
     - Inputs: FUNC-011 suite and UX-009.
     - Deliverable: UI model, redacted audit rows, package policy, five-seat proof.
     - Depends on: S2-S4.
     - Acceptance: only approved transfers occur and all limits remain enforced.
     - Validation: shell render, package, and live-seat gates.
     - Done when: evidence bundle covers local/mesh/VDI and cleanup.
- Scope: Rich MIME negotiation and transport across DRM, mesh, VDI, permissions, limits, package policy, and proof. Files application UX and general collaboration
  navigation remain FUNC-011.
- Relevant files/components: mde-egui DRM/input, mde-shell-egui clipboard/VDI, mde-collab-types, mde-bus, transfer/CAS workers, and Workload VDI adapters.
- Dependencies: FUNC-011 consumes the contract and UI; ARCH-010 supplies attachment; UX-009 supplies styling.
- Acceptance criteria:
  1. Text, HTML, image, file, and typed metadata round-trip or fail with a typed reason.
  2. Secret, replay, flood, stale lease, and unauthorized-peer tests pass.
  3. Five-seat local/mesh/VDI evidence shows bounded memory and cleanup.
- Verification method: shared, bus, shell, VDI, package, and live cargo gates with explicit farm routing; record exact payload hashes.
- Origin or merged source IDs: 2026-07-26 operator platform cut; archived clipboard workstreams.

### WL-FUNC-017 - Complete Maps, navigation, and MG90 radio health

- Status: Remaining
- Priority: P0
- Complexity: Epic
- Problem: Maps, weather, Car navigation, GNSS, and MG90 radio data have incomplete provider truth, offline behavior, taskbar entry, and recovery.
- Required outcome: Maps provides production offline maps, turn-by-turn navigation, and a map-first current/1-day/3-day/5-day weather experience. A live weather icon
  and temperature beside the clock deep-link into Maps. Car exposes typed route/vehicle/radio health; MG90 is bounded, reconnectable, multi-manager, and never presents
  fabricated position, weather, forecast, or link state.
- Current state: Typed weather/location providers, catalog-bound cache, UI, launcher, and navigation authority exist; offline data/routes, radio recovery, and live proof remain.
- **Current/forecast provider (2026-08-08):** generation-bound 5/10-minute NWS refresh, provider freshness, bounded cache/retry, and off-runtime I/O passed 8/8 twice;
  live NWS/Maps proof remains: `docs/platform/evidence/WL-FUNC-017-2026-08-08-weather-provider-s3-r1.md`.
- **Atmospheric provider (2026-08-08):** exact nowCOAST WMS identity, bounded PNG/cache, and latest-wins dual-generation viewport admission passed ten focused tests;
  GUI publication/live proof remains: `docs/platform/evidence/WL-FUNC-017-2026-08-08-atmospheric-map-provider-s4-r1.md`.
- **Clock weather launcher (2026-08-08):** typed icon/temperature deep-link and weather→battery→time geometry passed 5/5; installed live captures remain:
  `docs/platform/evidence/WL-FUNC-017-2026-08-08-clock-weather-launcher-s9-r1.md`.
- **Navigation authority (2026-08-09):** route/progress/replay/restart passed 9/9; generation-exhaustion atomicity passed 4/4:
  `docs/platform/evidence/WL-FUNC-017-2026-08-08-navigation-authority-s6-r1.md`; `docs/platform/evidence/WL-FUNC-017-2026-08-09-navigation-generation-atomicity-r2.md`.
- **Offline catalog binding (2026-08-09):** replacement/expiry revokes tiles and schema-v1 upgrades open empty instead of failing; `.90` passed 7/7:
  `docs/platform/evidence/WL-FUNC-017-2026-08-09-offline-catalog-binding-r3.md`.
- **MG90 roster (2026-08-09):** approved selection owns v2 and loss stops claims; `.90` passed 15/15: `docs/platform/evidence/WL-FUNC-017-2026-08-09-mg90-roster-runtime-r5.md`.
- Remaining work:
  1. S1 Freeze provider, location, and weather contracts.
     - Objective: define vehicle, GNSS, radio, route, map tile, weather location, current conditions, forecast, map field, manager, capability, and health schemas with
       source, producer time, fetch time, attribution, freshness, and explicit gaps.
     - Inputs: mesh types, existing MG90 managers, `nws_alert`, `nws_forecast`, `iem_radar`, vehicle fixes, and offline gazetteer results.
     - Deliverable: bounded versioned contracts, topic helpers, normalization rules, and hostile schema tests. Add `action/weather/set-location` plus latest-wins
       `state/weather/location/<host>`, `state/weather/current/<host>`, `state/weather/forecast/<host>`, and `state/weather/map/<host>` projections.
     - Weather bounds: `WeatherLocationMode::{Auto, Manual}`; at most 120 hourly periods and five daily summaries; unit-tagged optional measurements; bounded labels,
       source identifiers, alert references, gaps, and attribution; local date/time-zone identity for aggregation; no zero-filled missing values.
     - Depends on: none.
     - Acceptance: unknown versions/managers, invalid or non-finite coordinates, hostile labels, oversized collections, future/stale timestamps, impossible units, and
       unsupported map fields fail closed without replacing the last known-good snapshot.
     - Validation: mesh-type cargo tests on .90.
     - Done when: round-trip and hostile fixtures pass and the evidence records the exact topics, schema revision, caps, and source hash.
  2. S2 Implement one effective-location authority.
     - Objective: resolve a truthful weather/map point without coupling workstation weather to a vehicle fix.
     - Inputs: S1, fresh same-host GNSS/device/vehicle fixes, persisted settings, and the existing offline geocoder.
     - Deliverable: daemon-owned Auto/Manual resolver, atomic preference persistence, location-change generation, and typed action admission. Auto uses a fresh local fix,
       falls back to the last saved verified place, and reports unavailable when neither exists. Selecting a verified search result enters Manual; `Use Current Location`
       restores Auto. A location change clears mismatched projections immediately and triggers refresh.
     - Depends on: S1.
     - Acceptance: restart preserves mode and verified manual place; stale/wrong-host fixes, replayed actions, invalid search rows, and unsupported coverage never silently
       select a location or retain data for the prior point.
     - Validation: resolver/property tests on .50 with injected clocks, fixes, persistence failures, restarts, and location movement.
     - Done when: Auto, Manual, fallback, unavailable, restart, and location-change traces are recorded.
  3. S3 Produce current conditions and 1/3/5-day forecasts.
     - Objective: make the daemon the only network and forecast aggregation authority for the selected location.
     - Inputs: S1/S2, official NWS `/points`, nearest-station observations, `forecastHourly`, and existing NWS parser/probe seams.
     - Deliverable: default-on Workstation weather worker with strict official-host allowlists, redirects disabled, bounded bodies, timeouts, backoff, last-good caching,
       condition normalization, current observations, 120 retained hourly periods, and five local-day summaries. Preserve the existing vehicle drive-ahead forecast topic
       for Car rather than changing its semantics.
     - Forecast behavior: 1D exposes the next 24 hourly periods; 3D and 5D expose producer-derived local-day high/low, dominant condition, precipitation, and wind
       summaries. Normalize provider text into clear day/night, cloud, rain, wintry, storm, fog, wind, and unavailable; retain original bounded provider text.
     - Cadence: fetch current conditions every five minutes and forecasts every ten minutes; refresh immediately after effective-location generation changes or material
       live-fix movement. Current data older than 90 minutes is stale; older than six hours is unavailable. Never infer freshness from local fetch success alone.
     - Depends on: S2.
     - Acceptance: partial observation/forecast failure is explicit; missing measurements remain absent; daily aggregation respects provider time zone and DST; provider
       loss retains only age-bounded last-good data and never publishes another location's conditions.
     - Validation: injected-probe parser/worker tests on .50 and broad async tests on BigBoy.
     - Done when: live or recorded NWS fixtures prove current, 120-hour, 1D/3D/5D, partial-outage, stale, expiry, retry, restart, and point-change behavior.
  4. S4 Produce bounded live weather map layers.
     - Objective: combine independently controllable radar, alerts, temperature, wind, and clouds without UI-owned network I/O.
     - Inputs: S1/S2, existing IEM/NEXRAD and NWS-alert workers, and official nowCOAST WMS capabilities.
     - Deliverable: reuse animated radar and alert polygon contracts; add validated Web-Mercator PNG snapshots for `ndfd_temperature:air_temperature`,
       `ndfd_wind:wind_velocity`, and `ndfd_sky:total_sky_cover`. Bound frame count, zoom, viewport, pixel dimensions, bytes, producer times, legends, cache keys, and disk
       retention; reject non-PNG/error documents and redirects.
     - Layer policy: Radar and Alerts are independent toggles and default on. Temperature/Wind/Clouds are one exclusive atmospheric selector and Temperature defaults on,
       so unreadable rasters are not stacked. Animate only valid time-ordered frames and expose pause, age, attribution, degradation, and unavailable state.
     - Cadence: retain the existing 60-second radar/alert refresh; fetch atmospheric fields every ten minutes and on location/viewport generation changes with bounded
       cancellation and backoff.
     - Depends on: S2.
     - Acceptance: strict HTTPS allowlists, bounded responses, PNG signature/dimension checks, future/stale frame rejection, cache corruption, provider loss, and viewport
       churn cannot block rendering, leak old-location imagery, or fabricate a successful layer.
     - Validation: overlay contract/parser/cache tests on .90; animation and field render fixtures on BigBoy.
     - Done when: each layer has fresh, stale, unavailable, malformed-payload, and attribution evidence, including explicit unsupported-territory behavior.
  5. S5 Ship offline map catalog and cache.
     - Objective: download, verify, index, expire, and query approved map regions without unbounded disk or network work.
     - Inputs: map provider policy and storage bounds.
     - Deliverable: cache/index worker, offline fixtures, quota/eviction evidence.
     - Depends on: S1.
     - Acceptance: offline query returns only verified tiles or a clear unavailable result.
     - Validation: map/cache property tests and package checks.
     - Done when: quota and corruption recovery pass.
  6. S6 Implement route and navigation authority.
     - Objective: calculate and present turn-by-turn routes with reroute, progress, cancellation, and source attribution.
     - Inputs: S1/S5, typed location and route services.
     - Deliverable: daemon-owned navigation worker and deterministic route fixtures.
     - Depends on: S5.
     - Acceptance: no UI-thread I/O, false position, stale route, or silent reroute.
     - Validation: route simulation cargo tests on BigBoy.
     - Done when: online/offline/reconnect traces are captured.
  7. S7 Implement MG90 radio and manager recovery.
     - Objective: connect multiple approved managers, correlate GNSS/radio health, select a source deterministically, and recover link loss.
     - Inputs: S1, provider credentials/configuration, MG90 hardware.
     - Deliverable: provider adapters, selection policy, audit, and replay tests.
     - Depends on: S1.
     - Acceptance: source loss is visible and no provider claims success without a live response.
     - Validation: provider/fault-injection tests and hardware fixture.
     - Done when: each manager state has evidence or a named blocker.
  8. S8 Build the map-first weather interface in Maps.
     - Objective: make Maps the sole weather destination with fast current/1D/3D/5D understanding and full live-map control.
     - Inputs: S2-S4, existing Maps Map tab/surface, offline geocoder, and UX-009 Style/Visuals.
     - Deliverable: a weather-focused Map mode centered on the selected location, responsive forecast sheet/inspector, Current plus 1D/3D/5D tabs, location search,
       `Use Current Location`, layer controls, legends, timestamps, attribution, keyboard/touch navigation, and explicit loading/stale/unavailable/unsupported states.
     - Layout behavior: preserve map context while the forecast sheet opens; do not invent a vehicle marker for a manual place. Wide views may use a side inspector;
       narrow/largest-text views use a bounded bottom sheet. Forecast tabs retain scroll/focus state and remain usable in Dark/Light and Bottom/Left taskbar layouts.
     - Display units: show Fahrenheit and mph initially while wire values remain unit-tagged. Never substitute zero, hide provider gaps, or imply coverage outside the
       United States and supported NOAA territories.
     - Depends on: S3, S4, UX-009 S1-S3.
     - Acceptance: tab switching, location mode, layer choices, attribution, stale/expiry behavior, responsive layout, and provider recovery are deterministic; render
       performs no Bus, network, clock, persistence, or backend I/O.
     - Validation: headless Maps model/render/input tests on BigBoy for Dark/Light, wide/narrow, largest text, keyboard, touch, fresh/stale/unavailable, and all layers.
     - Done when: deterministic captures and direct-DRM review prove every state with no clipping, hidden meaning, unreadable raster stack, or false location.
  9. S9 Add the clock-adjacent live weather launcher.
     - Objective: expose live weather beside the clock without creating a second launcher, Home widget, tray flyout, or new top-level surface.
     - Inputs: S3, Maps weather deep link, shell Bottom/Left taskbar geometry, UX-012 S1-S3, and the shared icon registry.
     - Deliverable: a full-hit-target monochrome condition icon plus rounded temperature immediately left of the clock in Bottom and Left layouts. Constrained layouts
       collapse to icon-only. Activation selects the existing Maps & Location surface, activates Map, and opens weather mode; the clock continues to open Notification
       Center and all hit regions remain disjoint.
     - Icon policy: add closed registry variants for clear day/night, clouds, rain, wintry weather, storms, fog, wind, and unavailable using existing licensed assets or
       repository-approved replacements. Stale data dims the icon and exposes age; expired/unavailable data shows no temperature and never reuses a prior condition.
     - Depends on: S3, S8, UX-012 S1-S3.
     - Acceptance: correct condition/temperature/freshness survives restart and location changes; responsive icon-only fallback, keyboard activation, accessible label,
       focus, deep link, and weather/clock/tray target separation pass in both placements.
     - Validation: shell projection/navigation/render tests on BigBoy plus direct-DRM Bottom/Left captures.
     - Done when: action traces and reviewed captures prove one click opens weather mode and the clock/tray retain their governed behavior.
  10. S10 Integrate Maps/Car, package, document, and prove release behavior.
      - Objective: close the complete Maps/weather/navigation/vehicle/radio slice with reproducible farm and live evidence.
      - Inputs: S1-S9, ARCH-009/010 authority, UX-009/012, CRIT-006/007, package policy, and required hardware/providers.
      - Deliverable: production wiring and default-on Workstation weather worker; responsive Maps/Car captures; package/service policy; five-seat/MG90/weather evidence;
        updated `docs/design/platform-interfaces.md` and refreshed `docs/design/maps-live-overlays.md` that describes shipped rather than planned providers.
      - Farm routing: rerun `farm-topology.sh table`; use distinct free slots with mesh contracts on `.90`, focused async workers on `.50`, and the longest
        Maps/shell/full gate on BigBoy `.130`. Run worklist self-test before lint, then doc-supersession and style-leak gates.
      - Live matrix: on release seat `.15`, exercise fresh fix, manual override, return to Auto, provider loss/reconnect, restart persistence, Bottom/Left, Dark/Light,
        icon-only fallback, offline maps/routes, sleep/rejoin, radio source loss, and MG90 recovery. Publish the required five-second AI alert before seat mutation.
      - Depends on: S5-S9, ARCH-009, ARCH-010, UX-009, UX-012, CRIT-006, CRIT-007.
      - Acceptance: no GUI-owned provider, network I/O, duplicate destination, fabricated data, secret, unbounded cache, stale installed payload, or undocumented live gap;
        missing hardware/provider access is recorded honestly and cannot become a production pass by inference.
      - Validation: focused cargo gates, full CI gate, package/RPM ownership checks, doc/worklist/style lints, direct-DRM captures, provider traces, and five-seat fleet proof.
      - Done when: one evidence bundle records revision, exact commands/slots/results, source timestamps/attribution, captures, package identity, all live outcomes, and any
        explicit production blocker.
- Scope: Owns maps, current/forecast weather, live weather layers, weather location preference, taskbar weather projection/deep link, navigation,
  vehicle/radio/GNSS contracts and workers, Car/Maps surfaces, offline cache, casting/location health, documentation, and proof. Taskbar geometry remains UX-012;
  shared visual primitives remain UX-009; Network Operations presentation belongs to ARCH-009. Do not add a Weather app, launcher catalog entry, Home widget, tray
  flyout, compositor surface, paid/geocoding provider, or GUI-side provider client.
- Relevant files/components: `crates/mesh/mackes-mesh-types/src/{vehicle,nws_alert,nws_forecast,iem_radar}.rs`, new weather contract module,
  `crates/mesh/mackesd/src/workers/{vehicle,nws_alert_overlay,nws_forecast_overlay,iem_radar_overlay}.rs`, new weather worker, `crates/desktop/mde-maps-location-egui/src/`,
  `crates/desktop/mde-shell-egui/src/{nav_bar,status_bar,surfaces}.rs`, `crates/shared/mde-theme/src/brand/icons.rs`, map cache/storage, GNSS/radio providers,
  package policy, `docs/design/{maps-live-overlays,platform-interfaces}.md`, and evidence helpers.
- Dependencies: ARCH-009 worker ownership, ARCH-010 Workload/Bus authority, UX-009 visual primitives, UX-012 taskbar geometry/actions, CRIT-006, and CRIT-007.
- Acceptance criteria:
  1. Offline map and route flows are deterministic and bounded.
  2. MG90 source selection, GNSS freshness, reconnect, and failure are truthful.
  3. Current weather and 1D/3D/5D forecasts are location-correct, bounded, attributed, fresh-or-explicitly-stale, and honest under partial provider loss.
  4. Radar, alerts, temperature, wind, and cloud cover render from daemon-owned validated data with independent truthful layer state.
  5. The weather icon and temperature sit immediately left of the clock, deep-link to Maps weather mode, degrade responsively, and never alter clock/tray semantics.
  6. Five-seat and MG90/weather-provider proof covers live/manual/offline/provider-loss/restart/sleep/rejoin and package upgrade.
- Verification method: contract/property, location/persistence, NWS/nowCOAST/IEM parser and worker, route/cache, provider/fault, Maps/shell model/render/navigation,
  accessibility, package, documentation, and live hardware/provider gates with explicit farm slots; BigBoy runs the longest Maps/shell and route suites.
- Origin or merged source IDs: 2026-07-29 Maps/MG90 review and vehicle/navigation source workstreams; 2026-08-08 operator map-first weather, 1D/3D/5D,
  full live-layer, current/manual-location, and clock-adjacent launcher decisions.

### WL-FUNC-018 - Seamless Flatpak Front Door backed by App VMs

- Status: Remaining
- Priority: P1
- Complexity: Epic
- Problem: Construct lacks a governed way to discover and run Flatpak applications without installing native host apps.
- Required outcome: Front Door searches a signed catalog, starts an isolated App VM through Workloads, displays its Wayland app over VDI, and stops/cleans the session
  predictably.
- Current state: bounded signed catalog admission, deterministic search, and a production-registered fail-closed importer now exist alongside typed App VM/OpenApp/session
  contracts; trust provisioning, image supply, launch readiness, UX, security, and live proof remain.
- **Catalog checkpoints (2026-08-08):** exact-signer admission/ranking and root-owned rollback-safe import passed `.196`; `.170` compiled production:
  `docs/platform/evidence/WL-FUNC-018-2026-08-08-signed-app-catalog-s1-r1.md`, `docs/platform/evidence/WL-FUNC-018-2026-08-08-catalog-importer-s1-r1.md`.
- **App VM profile checkpoint (2026-08-08):** the immutable Wayland/Flatpak contract, supervisor, readiness/provenance, and hostile fixtures passed on `.170`;
  a current built image/hash and live boot remain:
  `docs/platform/evidence/WL-FUNC-018-2026-08-08-app-vm-profile-s2-r1.md`.
- **Runtime admission checkpoints (2026-08-09):** unavailable or cross-VM guest evidence cannot authorize resume or mutate desired state; `.90` passed 25/25 and BigBoy passed
  26/26: `docs/platform/evidence/WL-FUNC-018-2026-08-09-unavailable-runtime-admission-r2.md`, `docs/platform/evidence/WL-FUNC-018-2026-08-09-runtime-vm-identity-r3.md`.
- **App VM timeout cleanup (2026-08-09):** expired post-admission opens revoke the lease and remain `Stopping` until adapter cleanup proves no backend/attachment survives;
  machine 193 passed the hostile regression 1/1: `docs/platform/evidence/WL-FUNC-018-2026-08-09-app-vm-timeout-cleanup-s3-r4.md`.
- Remaining work:
  1. S1 Freeze catalog and identity.
     - Objective: verify signed app metadata, origin, permissions, version, icon, and search ranking.
     - Inputs: catalog projection and trust policy.
     - Deliverable: bounded catalog contract, importer, and ranking tests.
     - Depends on: ARCH-010 S2.
     - Acceptance: unsigned, stale, duplicate, or secret-bearing entries are rejected.
     - Validation: catalog/property cargo tests on .50.
     - Done when: catalog evidence and signature hashes exist.
  2. S2 Build App VM image/profile.
     - Objective: create reproducible image with Flatpak runtime, Wayland guest, agent, GPU/audio policy, and safe resource bounds.
     - Inputs: Workload adapter and image builder.
     - Deliverable: image/profile manifest and readiness probe.
     - Depends on: ARCH-010 S4, S5.
     - Acceptance: image contains only approved runtimes and reports ready/unavailable truthfully.
     - Validation: image/package cargo and shell checks on BigBoy.
     - Done when: image hash and probe trace exist.
  3. S3 Implement typed open/resume/stop.
     - Objective: start one App VM, wait for readiness, attach VDI, and stop it on session close or policy.
     - Inputs: S1/S2 and Workload operation API.
     - Deliverable: controller, idempotency, cancellation, and cleanup tests.
     - Depends on: S2.
     - Acceptance: duplicate opens reuse one session; timeout and crash clean all resources.
     - Validation: Workload/App VM cargo tests.
     - Done when: lifecycle trace proves one operation path.
  4. S4 Integrate Front Door UX.
     - Objective: search, select, approve permissions, show progress, focus input, and report failure in the shared Construct style.
     - Inputs: S1-S3, UX-009/012.
     - Deliverable: render/model fixtures and no-backend-I/O UI.
     - Depends on: S3.
     - Acceptance: no shell process or arbitrary command is launched.
     - Validation: shell render/navigation tests.
     - Done when: Dark/Light/narrow/largest-text captures pass.
  5. S5 Prove security and release behavior.
     - Objective: verify sandbox, resource limits, package upgrade, app data persistence, reconnect, and five-seat acceptance.
     - Inputs: S1-S4 and CRIT-006/007.
     - Deliverable: signed security/package/live evidence.
     - Depends on: S4.
     - Acceptance: host files/secrets are inaccessible and corrected-forward recovery succeeds.
     - Validation: package, SELinux, architecture, and live VDI gates.
     - Done when: every supported provider limitation is named.
- Scope: Owns Flatpak catalog, Front Door, App VM image/lifecycle, VDI UX, policy, package, migration, and proof. Generic Workload and Android lifecycle are out of scope.
- Relevant files/components: app catalog types/workers, mde-shell-egui Front Door/IAC, image-builder, browser/VDI, Quadlet/libvirt packaging.
- Dependencies: ARCH-010, ARCH-009, UX-009, UX-012, CRIT-006, CRIT-007.
- Acceptance criteria:
  1. Signed catalog search opens one isolated App VM through typed Workloads.
  2. Readiness, input, audio, persistence, stop, crash, reconnect, and cleanup are truthful.
  3. Five-seat security and package proof passes without host app installation.
- Verification method: catalog, image, Workload, shell, package, SELinux, and live VDI cargo gates; BigBoy runs image/build jobs.
- Origin or merged source IDs: 2026-07-31 Flatpak Front Door decision and archived app-launch workstreams.

### WL-FUNC-019 - Make Remote Sessions the universal resource browser

- Status: Remaining
- Priority: P0
- Complexity: Epic
- Problem: Remote Sessions is a narrow desktop chooser and does not admit all governed resources, typed capabilities, provenance, or safe actions.
- Required outcome: one universal resource browser discovers peers, VMs, containers, Apps, Android apps, media, files, and services; deduplicates them by stable identity;
  exposes typed Open/Start/Resume/Transfer actions; and never launches an untrusted or ambiguous resource.
- Current state: universal contracts, source adapters/deduplication, a pure searchable Remote Sessions model, and fail-closed typed action routing exist. Complete route
  fixtures, responsive captures, and live recovery proof remain.
- Remaining work:
- **Resource catalog hostile-boundary checkpoint (2026-08-06):** resource
  contract tests cover multi-source cards, duplicate identities, malformed
  provenance, and unknown kinds; the focused farm lane passed 1/1 on `.90`.
  Source/schema, adapter, action, and live recovery proof remain open. Evidence:
  `docs/platform/evidence/WL-FUNC-019-2026-08-06-resource-catalog-hostile-r1.md`.
- **Approved-source adapter checkpoint (2026-08-08):** bounded peer, Workload,
  admitted App VM/Android, Media, and typed file-share projection plus
  deterministic conflict collapse and explicit stale/unavailable states passed
  12/12 focused library tests on `.170`, including generation-bound Workload
  actions:
  `docs/platform/evidence/WL-FUNC-019-2026-08-08-resource-adapters-s2-r1.md`.
- **Remote Sessions presentation checkpoint (2026-08-08):** pure search/filter,
  grouping, badges, provenance/freshness, and failure states passed 4/4 on `.90`:
  `docs/platform/evidence/WL-FUNC-019-2026-08-08-resource-browser-s3-r1.md`.
- **Typed action checkpoint (2026-08-08):** fixed Workload/VDI/clipboard/Android
  routes, accepted-receipt cancellation, persisted signed Bus ingress, and
  hostile bypass refusal passed 22/22 on `.196` plus 10/10 shell fixtures:
  `docs/platform/evidence/WL-FUNC-019-2026-08-08-resource-actions-s4-r1.md`.
- **Wide-LAN Windows checkpoint (2026-08-08):** skipped broad CIDRs now admit
  at most 128 valid observed neighbors and issue a one-time explicit-target
  diagnostic for quiet RDP hosts. The separate Nodes scanner now consumes that
  same bounded fallback instead of discarding valid neighbors outside its local
  `/24`; focused farm tests passed on `.50` and `.90`. A live Windows target was
  not present among the currently observed neighbors, so deployed round-trip
  proof remains:
  `docs/platform/evidence/WL-FUNC-019-2026-08-08-wide-lan-rdp-discovery-s2-r1.md`,
  `docs/platform/evidence/WL-FUNC-019-2026-08-08-nodes-wide-lan-rdp-s2-r2.md`.
- **Probed-RDP resource checkpoint (2026-08-09):** a fresh bounded TCP 3389
  observation now becomes one approval-gated Desktop/RDP resource with an
  authenticated-mirror provenance and trusted-LAN transport; public, malformed,
  stale, and unconfirmed candidates remain non-connectable. Focused farm tests
  passed 3/3. Live Windows address
  discovery and installed connection proof remain. Evidence:
  `docs/platform/evidence/WL-FUNC-019-2026-08-09-probed-rdp-resource-card-s2-r3.md`.
- **Live Windows discovery checkpoint (2026-08-09):** Basement seat 15 now
  explicitly targets the quiet Windows endpoint at `172.20.146.54`; its fresh
  shared probe inventory contains `ms-wbt-server` on TCP 3389. Authenticated
  connection/render proof and publisher-key distribution remain. Evidence:
  `docs/platform/evidence/WL-FUNC-019-2026-08-09-live-windows-rdp-discovery-s5-r1.md`.
- **Resource credential activation checkpoint (2026-08-09):** the base RPM now
  activates the previously inert resource-publisher credential helper through
  a bounded idempotent oneshot before controlled shell restart. `.90` package
  and unit gates passed; live publisher-key distribution remains:
  `docs/platform/evidence/WL-FUNC-019-2026-08-09-resource-credential-activation-r4.md`.
- **Credential readiness checkpoint (2026-08-09):** the bounded oneshot no longer masks missing/invalid publisher credentials, so systemd reports failed readiness while
  preserving boot and read-only catalog access; `.90` gates passed: `docs/platform/evidence/WL-FUNC-019-2026-08-09-resource-credential-readiness-r4.md`.
- **Catalog rollback checkpoint (2026-08-09):** same-publisher rollback/equivocation preserves last-good cards and revokes stale actions; `.50` passed:
  `docs/platform/evidence/WL-FUNC-019-2026-08-09-remote-sessions-catalog-rollback-r5.md`.
- **Live Windows authority checkpoint (2026-08-09):** seat 15 detects `172.20.146.54:3389`; publisher HMAC, accepted-receipt VDI handoff, and Windows login remain absent:
  `docs/platform/evidence/WL-FUNC-019-2026-08-09-rdp-authority-path-r7.md`.
  1. S1 Freeze resource schema and identity.
     - Objective: version resource kind, stable identity, origin, owner, capabilities, freshness, lifecycle, and provenance.
     - Inputs: mesh peers, Workload, app, Android, media, and file types.
     - Deliverable: bounded catalog contract and identity/property tests.
     - Depends on: ARCH-010 S2.
     - Acceptance: collisions, stale records, unknown kinds, and malformed provenance are rejected.
     - Validation: mesh-type cargo tests on .90.
     - Done when: schema and collision evidence are recorded.
  2. S2 Implement source adapters and deduplication.
     - Objective: project each approved source into one catalog with deterministic merge and unavailable state.
     - Inputs: S1, peer directory, Workload, provider workers.
     - Deliverable: adapter registry, merge policy, and hostile stale-source fixtures.
     - Depends on: S1.
     - Acceptance: one resource produces one card and source conflicts remain visible.
     - Validation: adapter/catalog cargo tests on .50.
     - Done when: all source kinds have fixtures.
  3. S3 Implement freshness and presentation.
     - Objective: render search, filters, grouping, capability badges, provenance, and unavailable/reconnecting state from bounded snapshots.
     - Inputs: S1/S2 and UX-009/012.
     - Deliverable: Remote Sessions model/surface and deterministic captures.
     - Depends on: S2.
     - Acceptance: render path performs no Bus/network/backend I/O.
     - Validation: shell model/render cargo tests.
     - Done when: wide/narrow/largest-text evidence passes.
  4. S4 Route typed actions through authority.
     - Objective: issue Open/Start/Resume/Transfer requests with target, generation, authorization, and cancellation through Workload/Bus.
     - Inputs: ARCH-010, FUNC-016, and resource contracts.
     - Deliverable: action adapter and negative bypass tests.
     - Depends on: S2.
     - Acceptance: no raw command, direct lifecycle topic, arbitrary path, or silent target substitution exists.
     - Validation: authority scan and Workload/action cargo tests on BigBoy.
     - Done when: every card action has a typed reply path.
  5. S5 Prove universal discovery and recovery.
     - Objective: exercise peer loss/rejoin, stale catalogs, duplicate sources, action failure, reconnect, and five-seat acceptance.
     - Inputs: S1-S4 and CRIT-006/007.
     - Deliverable: catalog/action/live evidence bundle.
     - Depends on: S4.
     - Acceptance: unavailable and recovery states are honest and bounded.
     - Validation: farm catalog/route gates and live seats/lighthouses.
     - Done when: every resource kind and failure case has evidence.
- Scope: Owns resource identity/catalog, adapters, deduplication, Remote Sessions UI, and typed action routing. Workload mechanics, Music internals, and App/Android guest
  internals remain in their owner epics.
- Relevant files/components: mesh peer/resource/workload types, mackesd catalog workers, mde-shell-egui session/IAC/front door, mde-bus, and provider adapters.
- Dependencies: ARCH-010, ARCH-009, FUNC-016, FUNC-018, FUNC-020, FUNC-021, UX-009, UX-012, CRIT-006/007.
- Acceptance criteria:
  1. Every supported resource appears once with identity, provenance, freshness, and capabilities.
  2. Every action is typed, authorized, generation-bound, cancellable, and observable.
  3. Five-seat/lighthouse loss, rejoin, and recovery produce no fabricated resource or side effect.
- Verification method: schema/adapter/catalog/action cargo suites, authority scans, shell captures, and live fleet proof; use BigBoy for the broad catalog gate.
- Origin or merged source IDs: Remote Sessions surveys and archived resource/session discovery workstreams.

### WL-FUNC-020 - Expose governed Android applications in Workloads

- Status: Remaining
- Priority: P1
- Complexity: Large
- Problem: Android is represented by partially integrated Cuttlefish layers without a complete signed app catalog, image/provider contract, lifecycle, or honest failure
  UX.
- Required outcome: Workloads exposes governed Android app, outer Android VM, and full Workstation desktop choices; the app path uses a signed AOSP/Cuttlefish image,
  typed start/stop/readiness, VDI presentation, and bounded host isolation.
- Current state: signed catalog/import, provider preflight, crash-safe lifecycle, bounded guest relay, typed VDI source, and governed Workloads cards/actions exist;
  release artifacts, remote-session attachment, guest packaging, nested-KVM run, and live proof remain.
- **Catalog/provider checkpoints (2026-08-08):** signed image/package/policy import and image/KVM/capacity/libvirt preflight passed farm gates:
  `docs/platform/evidence/WL-FUNC-020-2026-08-08-android-signed-catalog-s1-r1.md`, `docs/platform/evidence/WL-FUNC-020-2026-08-08-android-provider-preflight-s2-r1.md`.
- **S3 lifecycle/readiness (2026-08-09):** exact-generation recovery and bounded guest relay passed BigBoy; outer-VM loss now revokes retained VDI sources, with `.90` at 6/6:
  `docs/platform/evidence/WL-FUNC-020-2026-08-08-android-lifecycle-s3-r1.md`, `docs/platform/evidence/WL-FUNC-020-2026-08-09-vdi-readiness-revocation-r4.md`.
- **S4 governed Workloads UX (2026-08-08):** daemon-cache-bound signed cards, typed lifecycle, responsive rendering, and WebRTC handoff passed 6/6 on `.170`;
  authorized Remote Sessions consumption and fail-closed no-dial refusal passed 2/2; live decoder/captures remain. Evidence:
  `docs/platform/evidence/WL-FUNC-020-2026-08-08-governed-android-ux-s4-r1.md`.
- **Release-artifact admission (2026-08-09):** schema-v2 readiness binds the release, package manifest, architecture/compatibility, and canonical installed tool digest;
  machine 193 package/verifier gates passed: `docs/platform/evidence/WL-FUNC-020-2026-08-09-release-artifact-admission-s2-s5-r5.md`.
- Remaining work:
  1. S1 Freeze Android catalog/image contracts.
     - Objective: define signed app identity, package/version, image digest, permissions, capabilities, resource profile, and guest readiness.
     - Inputs: Android mesh types and provider policy.
     - Deliverable: bounded contracts, importer, and hostile tests.
     - Depends on: ARCH-010 S2.
     - Acceptance: unsigned, stale, incompatible, or over-limit entries fail closed.
     - Validation: mesh-type cargo tests on .50.
     - Done when: catalog/image hashes and fixtures exist.
  2. S2 Implement image and provider admission.
     - Objective: verify AOSP/Cuttlefish image, host capability, nested virtualization, and provider health before placement.
     - Inputs: S1, CloudRunner, node capabilities.
     - Deliverable: provider adapter, preflight, and refusal diagnostics.
     - Depends on: S1.
     - Acceptance: no unsupported host receives Android and no fake ready state is emitted.
     - Validation: provider/property cargo tests and package checks on BigBoy.
     - Done when: preflight matrix is evidenced.
  3. S3 Integrate typed app lifecycle.
     - Objective: start one outer VM, install/launch/stop one approved app, and reclaim resources through Workload operations.
     - Inputs: S1/S2 and ARCH-010 S3/S4.
     - Deliverable: lifecycle adapter, generation/cancel/retry tests, and VDI source.
     - Depends on: S2.
     - Acceptance: duplicate/cancel/crash/restart never leaks VM, app, lease, or process.
     - Validation: Workload/Android cargo tests on BigBoy.
     - Done when: end-to-end operation trace exists.
  4. S4 Render governed Android UX.
     - Objective: show app cards, permission/approval, progress, VDI input, unavailable state, and cleanup in Workloads/Remote Sessions.
     - Inputs: S3 and UX-009/012.
     - Deliverable: render/model fixtures and typed action wiring.
     - Depends on: S3.
     - Acceptance: shell never launches adb, qemu, or package commands directly.
     - Validation: shell render/authority tests.
     - Done when: responsive captures and refusal states pass.
  5. S5 Prove security, package, and live behavior.
     - Objective: verify image provenance, SELinux/cgroup/device isolation, audio/input, reconnect, upgrade, and five-seat acceptance.
     - Inputs: S1-S4 and CRIT-006/007.
     - Deliverable: signed package/security/live evidence.
     - Depends on: S4.
     - Acceptance: host secrets/files are inaccessible and provider failures remain actionable.
     - Validation: package/SELinux/VDI/live hardware gates.
     - Done when: unavailable Cuttlefish hardware/provider is explicitly named.
- Scope: Owns Android catalog/image/provider, outer VM/app lifecycle, VDI UX, policy, packaging, and proof. Generic Workload, Remote Sessions catalog, and native Music
  are out of scope.
- Relevant files/components: mesh Android/provider types, mackesd CloudRunner/Cuttlefish workers, Workloads/IAC shell, image-builder, libvirt/VDI packaging.
- Dependencies: ARCH-010, FUNC-019, UX-009, UX-012, CRIT-006, CRIT-007.
- Acceptance criteria:
  1. Signed app/image identity and host preflight gate every operation.
  2. One typed operation controls VM/app start, readiness, input, stop, cancel, retry, and cleanup.
  3. Security/package/live evidence proves no host escape or invented readiness.
- Verification method: contract/provider/Workload/shell/package/SELinux cargo gates and named Cuttlefish/live-seat proof; BigBoy runs the Android image gate.
- Origin or merged source IDs: 2026-08-03 governed Android Workloads decision and archived Android/App VM workstreams.

### WL-FUNC-021 - Deliver the Spotify-class Music workspace and service parity
- Status: Remaining
- Priority: P1
- Complexity: Epic
- Problem: Music has a direct Airsonic panel and incomplete daemon authority, media playback, library/Jellyfin, offline cache, discovery, casting, handoff, and live proof.
- Required outcome: a near-Spotify workspace uses daemon-owned typed catalog, queue, playback, bookmarks, cache, and source authority; mde-media-core provides real mpv
  frame/audio playback; Media UI covers local/Jellyfin/library flows; discovery, DLNA/cast, peer handoff, and live visual/audio proof pass.
- Current state: daemon-owned catalog/queue/cache, typed playback, artwork, browse/detail, and signed radio pass. Release 11 executes on all five seats;
  named 38 Special, Black Ice, and Podcast details pass on Dell without the old
  stale error; one daemon owns each seat; CPU/NWS and provider loss pass. Live renderer and handoff proof remain.
- **Daemon projection validation checkpoint (2026-08-06):** invalid newer `MusicWorkspaceSnapshotV1` content is refused and the last valid projection is retained;
  revision zero is rejected; Music UI 4/4 `.50`, daemon validation 1/1 `.90`. Evidence: `docs/platform/evidence/WL-FUNC-021-2026-08-06-projection-validation-r2.md`.
- **Media hardening (2026-08-06):** media-core 250/250 on BigBoy; four bounded Music proof-helper self-tests pass; live renderer and second-seat proof remain open. Evidence:
  `docs/platform/evidence/WL-FUNC-021-2026-08-06-media-hardening-r2.md`; boundary: `evidence/WL-FUNC-021-2026-08-06-media-source-projection-r1.md`.
- **Provider restart (2026-08-09):** selected source survives restart with fallbacks; `.90` passed: `docs/platform/evidence/WL-FUNC-021-2026-08-09-provider-restart-binding-r4.md`.
- Remaining work:
- **Named-detail/activation/NWS release-11 checkpoint (2026-08-08):** identity-bound details, one daemon/shell per seat, Dell records, and five-seat recovery pass:
  `docs/platform/evidence/WL-FUNC-021-2026-08-08-seat-activation-release10-r1.md`; `docs/platform/evidence/WL-FUNC-021-2026-08-08-nws-recovery-release11-r1.md`.
- **Signed live-radio checkpoint (2026-08-08):** native F44 release 8 is live
  on all five seats with host-encrypted Music credentials. Dell and seat 15
  pass exact retained C-SPAN signed Play/Stop; Dell sink capture is non-silent
  (2,621,440 bytes, 287,035 non-zero samples, peak 20,092, RMS 1,677.73).
  T480/Eagle/Surface mutating playback and human speaker judgment remain open.
  Evidence: `docs/platform/evidence/WL-FUNC-021-2026-08-08-live-radio-release8-r1.md`.
- **Library checkpoint (2026-08-06):** typed collections replace Airsonic rows; UI 44/44 on `.50`, fmt `.90`; `evidence/WL-FUNC-021-2026-08-06-daemon-library-r1.md`.
- **Search checkpoint (2026-08-06):** retained typed search renders; provider search is fallback; UI 45/45 `.50`; `evidence/WL-FUNC-021-2026-08-06-daemon-search-r1.md`.
- **Drain guards (2026-08-06):** search replay and duplicate Jellyfin identities pass `.90`; live-seat RPM ownership self-test and read-only probe pass. Evidence:
  `docs/platform/evidence/WL-FUNC-021-2026-08-06-search-replay-r1.md`, `docs/platform/evidence/WL-FUNC-021-2026-08-06-media-source-identity-r1.md`.
- **Jellyfin cache checkpoint (2026-08-07):** atomic verified cache; zero-byte/truncated entries refused (BigBoy 2/2);
  mde-jellyfin 114/114 (90 unit/12 browse/2 outage/9 playback/1 doctest), Media UI 104/104; live download/network-loss and package proof remain open.
  `docs/platform/evidence/WL-FUNC-021-2026-08-07-jellyfin-current-r1.md`;
- **mpv/recovery (2026-08-06):** BigBoy fixture 1/1 nonblank; media-core 239/239 retry/resume; `evidence/WL-FUNC-021-2026-08-06-media-recovery-r1.md`; live proof remains.
- **Daemon Album/download/workerless checkpoint (2026-08-06):** Home, Library, and Search open retained albums; detail emits typed play without `LoadAlbum` worker requests.
  Library, Album, and Downloads publish bounded actions; daemon is 168/168 on `.90`, Music UI is 47/47 on `.50`, and embedded construction starts no worker.
  `docs/platform/evidence/WL-FUNC-021-2026-08-06-managed-download-r1.md`, `docs/platform/evidence/WL-FUNC-021-2026-08-06-embedded-workerless-r1.md`.
- **Typed target handoff checkpoint (2026-08-06):** bounded peer heartbeats project honestly; fresh idle mesh seats publish typed `transfer`, stale/owning peers remain browse-only.
  Music UI is 48/48 on `.50`, format is clean, and the hostile test covers `peer:seat-15`; live owner-yield/resume and DLNA/provider/package proof remain open.
  `docs/platform/evidence/WL-FUNC-021-2026-08-06-target-handoff-r1.md`, `docs/platform/evidence/WL-FUNC-021-2026-08-06-peer-targets-r1.md`.
- **Cast checkpoint (2026-08-06):** bounded discovery and real DLNA `SetAVTransportURI`/`Play`/`Seek` are fixture-verified; media-core is 240/240 on BigBoy and format-clean.
  Loopback renderer acceptance is recorded in `docs/platform/evidence/WL-FUNC-021-2026-08-06-cast-bounds-r1.md`; live renderer, Chromecast, mesh-owner, and seat proof remain open.
- **Live provider-loss checkpoint (2026-08-08):** release 11 on seat 15 passed a controlled healthy → provider loss → healthy transition with cached catalog/state available.
  The daemon stayed active with zero restarts and the seat-local firewall rule was removed; two-catalog outage and audible stream continuity remain open.
  `docs/platform/evidence/WL-FUNC-021-2026-08-08-live-provider-loss-release11-r1.md`.
- **Provider-loss loopback checkpoint (2026-08-06):** bounded FIN/reset and zero fallback requests are transport/policy proof only, not live provider/daemon/
  decoder/hardware proof. `install-helpers/verify-music-network-loss.sh`;
  `docs/platform/evidence/WL-FUNC-021-2026-08-06-network-loss-loopback-r1.md`.
- **Provider-loss reconnect checkpoint (2026-08-06):** the native engine now
  retries a failed Subsonic stream from the audible playhead using bounded
  `timeOffset` resumes, clears buffered-ahead samples before retry, preserves
  the complete-track cache, and refuses arbitrary direct/radio URLs. BigBoy
  passed the full mde-musicd suite at 176/176, focused engine lane at 21/21,
  and reconnect-timeout lane at 2/2. Controlled live provider loss/recovery now
  passes on seat 15 while the daemon and cached typed surfaces stay available;
  audible in-progress stream continuity remains open.
  `docs/platform/evidence/WL-FUNC-021-2026-08-06-network-loss-reconnect-r1.md`, `docs/platform/evidence/WL-FUNC-021-2026-08-06-reconnect-timeout-r1.md`.
- **Cast loopback checkpoint (2026-08-06):** a bounded local renderer accepts
  discovery, description, `SetAVTransportURI`, `Play`, and finite `Seek`, while
  malformed and non-finite seeks are refused and the listener is cleaned up.
  Live DLNA/Chromecast, mesh-owner, and seat-handoff proof remain open.
  `install-helpers/verify-music-cast-loopback.sh`;
  `docs/platform/evidence/WL-FUNC-021-2026-08-06-cast-loopback-r1.md`.
- **Roaming admission (2026-08-06):** 11/11; live two-seat proof open. Evidence: `docs/platform/evidence/WL-FUNC-021-2026-08-06-roaming-admission-r1.md`.
- **Two-seat handoff checkpoints (2026-08-08/09):** fixtures prove exact-once queue/playhead transfer; physical preflight refused Eagle's release mismatch and stale peer
  without mutation (`.50`/`.90` passed): `evidence/WL-FUNC-021-2026-08-08-two-seat-owner-handoff-r1.md`,
  `docs/platform/evidence/WL-FUNC-021-2026-08-09-physical-two-seat-handoff-preflight-r5.md`,
  `docs/platform/evidence/WL-CRIT-007-WL-FUNC-021-2026-08-09-eagle-release23-alignment-r7.md`.
- **Cast runtime audit checkpoint (2026-08-06):** read-only seat inspection
  found no physical UPnP renderer, usable Chromecast discovery path, target
  cast-receiver unit, or second admitted peer. Typed mesh transfer and the
  separate Media DLNA implementation remain fixture/source-proven only;
  physical renderer, Chromecast, mesh-owner, and two-seat continuity proof
  remain open.
  `docs/platform/evidence/WL-FUNC-021-2026-08-06-cast-runtime-audit-r1.md`.
- **Cast-admission checkpoint (2026-08-06):** URLs, titles, and HTTP endpoints reject oversized/control-bearing input before the network gate; BigBoy tests
  passed 20/20. Live renderer, Chromecast, mesh-owner, and seat proof remain open. Evidence: `docs/platform/evidence/WL-FUNC-021-2026-08-06-cast-admission-r1.md`.
- **Two-catalog outage checkpoint (2026-08-06):** source projection retains two admitted variants under one logical queue track.
  Failed-first/healthy-second decoding is fixture-verified.
  BigBoy fixture 1/1, source projection 1/1, and full mde-musicd 173/173; live provider outage, mid-track resume, and hardware/package proof remain open.
  `docs/platform/evidence/WL-FUNC-021-2026-08-06-two-catalog-outage-r1.md`.
- **Jellyfin outage (2026-08-06):** known-good cache survives failures; truncated manifests refused; 90 unit/12 browse/1 outage/9 playback/doctest pass; live proof remains.
  `docs/platform/evidence/WL-FUNC-021-2026-08-06-jellyfin-outage-r1.md`.
- **GUI authority checkpoint (2026-08-08):** embedded and standalone Music both
  consume daemon projections, start no provider/playback worker, and fail closed
  without an authenticated writer. Focused `.50` regression passed. Evidence:
  `docs/platform/evidence/WL-FUNC-021-2026-08-08-standalone-daemon-authority-r1.md`.
- **Renderer recovery checkpoint (2026-08-08):** renderer failure revokes playback/MPRIS authority; reacquisition resumes the exact finite track at its audible
  position unless a control cancels it. Two hostile `.50` regressions passed; physical PipeWire/audible and two-seat proof remain. Evidence:
  `docs/platform/evidence/WL-FUNC-021-2026-08-08-renderer-recovery-r1.md`.
- **Real-mpv Media UI checkpoint (2026-08-07):** mde-media-egui 110/110; mde-media-core mpv 257 unit,
  1 real-mpv fixture, and 1 doctest passed. Loading clears stale video frames.
  Physical renderer, provider-loss, handoff, and second-seat proof remain open.
  Evidence: `docs/platform/evidence/WL-FUNC-021-2026-08-07-media-render-clear-r1.md`.
- **Continuation (2026-08-07):** mde-musicd 182/182, roaming root-loss 18/18,
  reconnect 8/8, mesh-router 26/26, and Dell CPU proof max 385‰/mean 283‰
  passed. Evidence: `docs/platform/evidence/WL-FUNC-021-2026-08-06-roaming-root-loss-r1.md`,
  `docs/platform/evidence/WL-FUNC-021-2026-08-06-reconnect-loop-audit-r1.md`, and `docs/platform/evidence/WL-FUNC-021-2026-08-06-cpu-bridge-r1.md`.
- **Live-boundary continuation (2026-08-07):** provider-loss loopback proves
  same-provider `timeOffset=1` recovery with no fallback; cast found zero physical SSDP/Chromecast targets. Dell auth material provisioned, signed mutation refused,
  then Dell became unreachable. Shell signing now matches daemon hostname canonicalization; the Fedora 44 release-5 RPM rebuilt and passed payload gates. Live loss, renderer,
  Handoff, auth, and rotation remain; canonical peer filenames, bounded state, and DLNA Stop rollback pass 15/15, 27/27, 1/1; physical renderer/two-seat proof unavailable.
  Evidence: `docs/platform/evidence/WL-FUNC-021-2026-08-07-provider-loss-audit-r1.md`,
  `docs/platform/evidence/WL-FUNC-021-2026-08-07-cast-runtime-audit-r2.md`, `docs/platform/evidence/WL-FUNC-021-2026-08-07-auth-runtime-guard-r1.md`.
  New: `evidence/WL-FUNC-021-2026-08-07-peer-roster-canonical-r1.md`, `evidence/WL-FUNC-021-2026-08-07-cast-seek-rollback-r1.md`.
  CPU mitigations are farm-verified; gateway phases pass focused tests; Dell/seat-15 release CPU proof passes.
  `evidence/WL-FUNC-021-2026-08-07-runtime-status-phase-coalescing-r1.md`, `evidence/WL-FUNC-021-2026-08-07-mesh-status-dedupe-r1.md`.
  Phased media/control-plane, cast/reconnect, mde-musicd cadence guards, Music UI poll cadence, gateway survey phases are farm-verified; five-seat CPU/NWS recovery remains open.
- **Live provider audio checkpoint (2026-08-06):** real Airsonic track `23427`
  completed through `mde-musicd` while a bounded PipeWire default-sink monitor
  captured 26.8 MiB of 48 kHz stereo s32le; 6,287,357/6,717,440 samples were
  nonzero and playback returned 0. Temporary capture files were removed.
  Provider/network-loss resume, physical-speaker judgment, and authenticated
  mutation delivery remain open.
  `docs/platform/evidence/WL-FUNC-021-2026-08-06-live-audio-capture-r1.md`.
- **Live Music DRM checkpoint (2026-08-06):** seat 15 produced a settled
  1920x1080 direct-DRM EGL frame (`DrmFourcc(XR30)`) with SHA-256
  `3a7ec14c51a5a46dde509c2b6c57cba5920cdfb8af5da19917d20a385ff5a199`.
  The generic Construct-Home verifier correctly rejected the taskbar-shaped
  profile; the new Music-specific verifier self-test passed and accepted the
  frame with 15 separated foreground components. The temporary drop-in was
  removed and the service returned active with zero restarts. Full rendered
  Music acceptance, provider/network-loss resume, handoff, and package proof
  remain open.
  `docs/platform/evidence/WL-FUNC-021-2026-08-06-live-drm-frame-r1.md`;
  `install-helpers/verify-music-drm-proof.py`.
- **RPM/install checkpoint (2026-08-06):** native F44 `.131` release 5 passed build/payload/size gates; base 83.5 MiB; Dell live proof passed (CPU max `437‰`, mean `218‰`).
  Seat 15 remained release 4. Evidence: `docs/platform/evidence/WL-FUNC-021-2026-08-06-dell-release5-cpu-r1.md`.
- **Artwork/pagination checkpoint (2026-08-07):** mde-musicd 199/199, mde-music-egui 64/64, shell route 1/1, UI format pass; release-6
  `magic-mesh-12.1.6-6.x86_64` is live on Dell and seat 15 (87,591,150 bytes; SHA-256 `eb9d6194b6a03a835a4b533f124260a39afbdb8297d81da410fdedf45f6d225e`).
  Both live gates pass with `NRestarts=0`; album offsets 0, 100, and 1600 return distinct rows, final 70/`has_more=false`; album/podcast art are local JPEGs.
  C-SPAN lacks a token; open: renderer, provider-loss, cast, handoff, radio playback, five-seat CPU/NWS.
  Evidence: `docs/platform/evidence/WL-FUNC-021-2026-08-07-music-artwork-release6-r1.md`.
- **Mutation authorization delivery audit (2026-08-06):** the audit confirms
  that the daemon's legacy HMAC verifier remains fail-closed, while the Music
  lane now uses a dedicated domain-separated Ed25519 capability. The root DRM
  shell alone receives the encrypted private seed; `mde-musicd` receives only a
  validated public key, with exact-body digest, scope, expiry, and replay
  checks. Shared types passed 431/431 tests and mde-musicd passed 174/174 on
  the farm. Host provisioning and package paths are source-verified; live
  authorized mutation delivery and installed-seat rotation proof remain open.
  `docs/platform/evidence/WL-FUNC-021-2026-08-06-auth-delivery-audit-r2.md`.
- **Mutation authorization package audit (2026-08-06):** base-RPM asset,
  manifest, systemd, helper self-test, and package dependency checks found
  that the provisioner also requires `openssl` and `curl`; those hard RPM
  requirements are now declared in `crates/mesh/mackesd/Cargo.toml` and are
  present in the fresh base RPM header. Installed-seat provisioning, mutation,
  and rotation proof remain open.
  `docs/platform/evidence/WL-FUNC-021-2026-08-06-auth-package-audit-r2.md`;
  prior audit: `docs/platform/evidence/WL-FUNC-021-2026-08-06-auth-package-audit-r1.md`.
- **Reusable live-seat gate (2026-08-06):**
  `install-helpers/verify-music-live-seat.sh --self-test` passes without SSH,
  and its bounded read-only default run passes on seat 15: `mde-musicd` active
  with `NRestarts=0`, ping answered, canonical `get-state` answered, and
  canonical `list-albums` answered. The explicit play probe was also run
  against live song `23427`, bounded at 15 seconds with no client process left
  behind; this does not claim audible or rendered acceptance. The helper uses
  no secret output and caps SSH/command/probe timeouts.
- **Queue durability checkpoint (2026-08-09):** synced atomic replacement preserves the last-good queue and cleans failed temporary writes; `.50` passed 14/14:
  `docs/platform/evidence/WL-FUNC-021-2026-08-09-queue-atomic-persistence-r1.md`.
  1. S1 Freeze catalog/provider authority.
     - Objective: make mde-musicd the only catalog/source/queue authority for Subsonic, local, Jellyfin, and approved providers.
     - Inputs: music types/domain, resource catalog, Jellyfin store.
     - Deliverable: bounded source contracts, provider selection, credentials redaction, and hostile tests.
     - Depends on: FUNC-019 S1/S2.
     - Acceptance: UI cannot invent a server, source, track, or queue state.
     - Validation: mde-musicd and Jellyfin cargo tests on .50.
     - Done when: source snapshots and provider failure evidence exist.
  2. S2 Prove real playback.
     - Objective: decode real audio/video with mpv, publish frame/audio/position/error, and recover from pause/seek/end/network loss.
     - Inputs: S1, mde-media-core, PipeWire/fixture assets.
     - Deliverable: engine adapter and nonblank-frame/resolved-audio fixtures.
     - Depends on: S1.
     - Acceptance: no fake success, silent fallback, unbounded event queue, or stale position.
     - Validation: mde-media-core --features mpv cargo tests/doctests on BigBoy.
     - Done when: frame/audio metrics and failure traces are recorded.
  3. S3 Ship workspace and daemon-owned controls.
     - Objective: render Home, Browse, Search, Queue, Now Playing, albums, artists, playlists, bookmarks, and typed transport controls.
     - Inputs: S1/S2 and UX-009/012.
     - Deliverable: Music UI model/render and signed Bus action integration.
     - Depends on: S1, S2.
     - Acceptance: GUI has no competing worker/store/playback authority.
     - Validation: mde-music-egui and shell Music cargo tests on .50.
     - Done when: responsive captures and action traces pass.
  4. S4 Complete library, Jellyfin, cache, and bookmarks.
     - Objective: load saved servers/profiles, browse/play libraries, download/cache bounded content, resume supported bookmarks, and report unavailable data honestly.
     - Inputs: S1-S3, Jellyfin store, cache policy.
     - Deliverable: library/cache/bookmark flows with atomic persistence and offline fixtures.
     - Depends on: S3.
     - Acceptance: credentials remain 0600; crash preserves old or new complete store; offline uses verified cache only.
     - Validation: music/Jellyfin/cache cargo tests and secret scan.
     - Done when: two-catalog and network-loss evidence exists.
  5. S5 Complete discovery, cast, and peer handoff.
     - Objective: discover bounded targets, perform typed DLNA/mesh handoff, seek after play, and preserve owner-yield/target-resume semantics.
     - Inputs: media cast core, peer/resource contracts.
     - Deliverable: discovery/cast/handoff adapters and refusal tests.
     - Depends on: S2-S4 and FUNC-019 S4.
     - Acceptance: malformed targets, nonfinite positions, failed seek, and conflicting owners fail closed.
     - Validation: media cast cargo tests on BigBoy and live DLNA/peer fixture.
     - Done when: handoff evidence names every unavailable target.
  6. S6 Complete release proof.
     - Objective: verify visual/audio playback, controls, cache, cast, handoff, package, RPM, Dell, and seat-15 acceptance.
     - Inputs: S1-S5 and CRIT-006.
     - Deliverable: signed Music/Media evidence bundle and rendered captures.
     - Depends on: S5.
     - Acceptance: live gaps remain explicit and do not become green by inference.
     - Validation: farm music/media suites, RPM gates, and named live-seat commands.
     - Done when: all required provider, hardware, and package results are recorded.
- Scope: Owns Music workspace/service, Media Player core/UI/Jellyfin, catalog/playback/cache/bookmarks, discovery/casting/handoff, shell integration, packaging, and
  proof. Generic Workload and collaboration transport remain elsewhere.
- Relevant files/components: mde-musicd, mde-music-egui, mde-media-core, mde-media-egui, mde-jellyfin, shell Music mount, Bus/resource contracts, PipeWire/mpv, and
  RPM/live scripts.
- Dependencies: FUNC-019, ARCH-010, UX-009, UX-012, CRIT-006/007.
- Acceptance criteria:
  1. Daemon authority and real mpv frame/audio playback pass hostile and fixture tests.
  2. Library/Jellyfin/cache/bookmark, discovery/cast, handoff, and network-loss flows are typed and bounded.
  3. Five-seat visual/audio/package evidence proves the shipped release or names blockers.
- Verification method: use @farm:{cargo test -p mde-musicd}
  @farm:{cargo test -p mde-media-core --features mpv}
  @farm:{cargo test -p mde-media-egui}
  and shell/RPM/live gates with BigBoy for the longest media job.
- Origin or merged source IDs: Spotify-class Music survey; archived WL-FUNC-007 and MEDIA-1..17; 2026-08-05/06 Music and Media evidence.
### WL-FUNC-022 - Deliver the Construct Clock and distributed mesh alarms

- Status: Remaining
- Priority: P1
- Complexity: Epic
- Problem: Construct has only a shell-owned Timers & Alarms panel, a five-zone hand-coded display clock, conflicting clock-click routes, no complete World Clock or
  Stopwatch, no durable daemon scheduler, and no governed way to ring selected mesh peers or use Music/radio sources without duplicating provider authority.
- Required outcome: one egui-native Clock surface provides World Clock, Alarms, Timers, and Stopwatch with AOSP DeskClock-derived procedures under Quazar styling. The
  visible clock opens Clock, a dedicated bell opens Notification Center, mackesd owns persisted scheduling and signed multi-peer execution, and mde-musicd remains the
  only Music/radio/NPR source and playback authority.
- Current state: Signed contracts, durable scheduling/convergence, governed audio, and Clock/bell chrome exist; multi-process/UI/package/live proof remains.
- **Contract/peer checkpoints (2026-08-08):** bounded Clock contracts and delivery/loss/rejoin/replay/global Stop passed `.196`:
  `docs/platform/evidence/WL-FUNC-022-2026-08-08-clock-contracts-s1-r1.md`, `docs/platform/evidence/WL-FUNC-022-2026-08-08-clock-peer-convergence-s2-r1.md`.
- **Scheduler/restart checkpoint (2026-08-09):** weekday/DST execution and durable alarm auto-silence/audio stop passed the 7/7 Clock suite:
  `docs/platform/evidence/WL-FUNC-022-2026-08-08-clock-scheduler-s2-r1.md`; `docs/platform/evidence/WL-FUNC-022-2026-08-09-weekday-alarm-dst-r2.md`;
  `docs/platform/evidence/WL-FUNC-022-2026-08-09-auto-silence-restart-r4.md`.
- **Clock audio checkpoint (2026-08-08):** durable signed Start/Stop/Snooze replay and the 3,000 ms audibility fallback passed 7/7 on `.196`:
  `docs/platform/evidence/WL-FUNC-022-2026-08-08-clock-audio-s3-r1.md`.
- **Clock UI checkpoint (2026-08-08):** projection, Jiff/IANA time, actions, and fail-closed behavior passed 5/5 on `.50`; evidence:
  `docs/platform/evidence/WL-FUNC-022-2026-08-08-clock-ui-s4-r1.md`.
- **Multi-process peer acceptance (2026-08-09):** independent processes reopened Bus/SQLite state for signed delivery, rejoin, local opt-out, and global Stop/Snooze
  convergence; machine 9 passed 1 parent plus 14 child ticks: `docs/platform/evidence/WL-FUNC-022-2026-08-09-multi-process-peer-acceptance-s2-r5.md`.
- Remaining work:
- **Resolve/preview checkpoint:** typed isolated preview and governed local-file admission passed 13 focused tests on `.196`:
  `docs/platform/evidence/WL-FUNC-022-2026-08-08-clock-resolve-preview-s3-r1.md`.
- **Clock/bell chrome checkpoint:** direct Clock routing, dedicated bell/unread lifecycle, and non-regressions passed 31 focused tests on `.50`:
  `docs/platform/evidence/WL-FUNC-022-2026-08-08-clock-chrome-s5-r1.md`.
  1. S1 Freeze Clock, calendar, audio-reference, and mesh-target contracts.
     - Objective: define one bounded versioned model for clocks, AOSP-style alarms, multiple timers, stopwatch mirrors, occurrence state, settings, targets, and audio.
     - Inputs: AOSP DeskClock revision `04e481f37e0b52b74c5a5c7b78b662d1f94e3478`, existing timer folds, mde-musicd `ContentRef`, peer identity/signing,
       Bus action/reply conventions, system tzdata, and workspace-pinned `jiff = "0.2.21"` with system-zoneinfo support.
     - Deliverable: `ClockCommandV1`, `ClockSnapshotV1`, `ClockScheduleV1`, `ClockOccurrenceV1`, `ClockTargetState`, `ClockAudioRef`, and `ClockSettingsV1`; constants for
       `action/clock/command/<target-node>`, `state/clock/<node>`, `event/notify/clock/<node>`, and `reply/<request-id>`; strict validation and topic constructors.
     - Contract behavior: alarms support one next occurrence or selected weekdays, label, enable, sound, and capability-gated vibration. Timers retain original duration,
       absolute deadline, pause state, expiry/overtime, and targets. Stopwatch state names one origin and read-only mirrors. All lists, labels, IDs, targets, laps, and bodies
       have explicit caps; unknown fields, duplicate keys, invalid civil times, unsupported zones, bad signatures, replay, and stale revisions fail closed.
     - Time behavior: use Jiff against platform `/usr/share/zoneinfo` rather than hand-coded DST. Clock format is fixed 24-hour; the long date is full weekday plus numeric
       day (`Monday 8`); only the World Clock hero shows seconds. The This Node IANA zone is primary and event/audit timestamps remain UTC.
     - Depends on: ARCH-009 S1/S2 and FUNC-021 S1/S3.
     - Acceptance: DST gaps/folds, one-time and weekly alarms, timer recovery, malformed contracts, signature/replay, schema skew, and all cap boundaries are deterministic.
     - Validation: mesh-types and pure Clock contract/property tests on `.90` with injected wall and monotonic clocks.
     - Done when: evidence records exact schemas, topic names, caps, default settings, AOSP reference revision, and hostile fixture results.
  2. S2 Implement the daemon scheduler, persistence, and multi-peer convergence.
     - Objective: remove scheduling authority from the render loop and make every selected capable Workstation execute an eagerly delivered schedule independently.
     - Inputs: S1, mackesd grouped-worker runtime, sole SQLite writer, enrolled peer roster, mesh action transport, and expected-state/rejoin projections.
     - Deliverable: supervised Clock worker, atomic Clock tables/ledger, deadline queue, occurrence journal, target receipts, per-origin blocklist, state publisher, and recovery.
     - Target behavior: new items target only the current node by default. Users may add enrolled peers advertising Clock executor/audio capability. Every selected target
       rings; a recipient may disable or remove its copy locally; the source sees that target state without changing other targets. Signed requests from trusted enrolled
       peers are accepted and visibly name their origin; blocked origins are rejected before persistence or effects.
     - Acknowledgement behavior: Snooze or Stop silences the acting node immediately and propagates to every reachable target. Occurrence ID plus actor-clock/event ID makes
       concurrent actions converge; Stop wins an exact tie. Delivered targets execute while the source is offline. A schedule first received after its due occurrence is
       marked missed and never rings late.
     - Recovery behavior: persist effects before publication. Active timers use absolute deadlines and honor elapsed wall time across shell restart, reboot, and suspend;
       locally persisted expired timers alert on recovery. Alarms recovered beyond their configured silence window become missed. No GUI frame is required for execution.
     - Depends on: S1 and ARCH-009 S3.
     - Acceptance: crash at every persistence/publication boundary is idempotent; duplicate delivery, reordering, origin loss, target loss/rejoin, opt-out, blocking, and
       concurrent global acknowledgement converge without duplicate or stale ringing.
     - Validation: Clock worker/store/fault tests on `.50`; multi-process Bus and signed peer fixtures on BigBoy.
     - Done when: restart and three-peer traces prove persisted deadlines, all-target execution, global acknowledgement, local opt-out, and missed-late delivery.
  3. S3 Add queue-independent Clock audio through Music and the seat audio authority.
     - Objective: let alarms/timers use bundled tones, local files, Music tracks, podcasts, NPR hourly news, and live radio without raw URLs or a second catalog/player.
     - Inputs: S1/S2, mde-musicd workspace catalog and engine, PipeWire/WirePlumber seat controls, NPR News Now source identity `500005` and official feed
       `https://feeds.npr.org/500005/podcast.xml`, and licensed bundled tones.
     - Deliverable: typed resolve/preview/start/stop Clock-audio requests in mde-musicd, stable catalog references, one-shot playback isolated from the user queue, source
       status/result replies, alarm-volume policy, and exact duck/restore handling.
     - Source behavior: Music owns discovery, credentials, provider URLs, custom radio, and member-station streams. Ship a governed NPR News Now preset that resolves the
       newest hourly episode at ring time and retain a separate configured NPR live-station choice. Clock stores only stable source identity plus a bundled fallback tone.
     - Failure behavior: external audio gets three seconds to begin; absent, stale, malformed, unauthorized, or unreachable content immediately starts the fallback and
       reports why. Alerts duck other seat audio to 25 percent and restore exact prior levels. Clock playback never mutates Music queue, ownership, history, or bookmarks.
     - Settings behavior: global alarm snooze defaults to 10 minutes; auto-silence defaults to 10 minutes then records Missed; alarm/timer crescendo defaults off; volume
       keys offer Volume, Snooze, or Stop with Volume as default. Timer sound/vibration and per-alarm sound follow the AOSP settings model.
     - Depends on: S1/S2 and FUNC-021 S1-S3.
     - Acceptance: provider/network loss, source deletion, invalid references, timeout, fallback, duck/restore, simultaneous Music playback, and daemon restart remain honest.
     - Validation: mde-musicd and PipeWire fixture tests on BigBoy plus bounded NPR feed/radio fixtures; no live network call occurs in a render test.
     - Done when: evidence proves each source kind, queue isolation, three-second fallback, audible non-silent output, and exact volume restoration.
  4. S4 Replace Timers with the complete AOSP-derived Clock surface.
     - Objective: render familiar AOSP DeskClock information hierarchy and procedures through shared Quazar components without copying Android runtime or visual assets.
     - Inputs: S1-S3, mde-egui Style/Motion/navigation, Surface taxonomy, Front Door, app switcher, icon registry, and the licensed AOSP behavior reference.
     - Deliverable: `Surface::Clock`, daemon projection/client, World Clock, Alarms, Timers, Stopwatch, Clock settings, responsive navigation, empty/loading/error states, and
       deterministic render fixtures. Remove `Surface::Timers` and stale shell-owned scheduling code instead of retaining a compatibility surface.
     - Navigation behavior: Clock is searchable and appears in the app switcher but is excluded from the pin catalog. It always opens World Clock. Wide layouts use a
       section sidebar; narrow layouts use a top World Clock/Alarms/Timers/Stopwatch selector so no second bottom bar competes with the Construct taskbar.
     - World Clock behavior: lead with a large digital primary clock, seconds, `Monday 8`, and This Node zone. Maintain one manually ordered mixed city/mesh-peer list over
       the full searchable IANA catalog. Preserve an offline peer's saved position but hide its row until the peer is online; never substitute UTC or stale peer state.
     - Alarm behavior: use AOSP-style time creation and progressive expanded rows, not the superseded advanced recurrence editor. Sort enabled alarms by next occurrence,
       then disabled alarms. Ring with an actionable banner containing Snooze and Stop; auto-silence produces a missed record instead of silently disappearing.
     - Timer/stopwatch behavior: support multiple named timers with start/pause/resume/reset/delete, Add 1 minute, and overdue count-up. Stopwatch provides start, lap,
       pause, and reset; laps are ephemeral. Selected peers may display a mirrored stopwatch, but only the origin can control it and stale origin state is labeled/frozen.
     - Depends on: S1-S3 and UX-009 S1-S4.
     - Acceptance: render performs no Bus, network, provider, persistence, or scheduling I/O; every action is typed; unavailable state never fabricates time or delivery.
     - Validation: shell model/navigation/render tests on BigBoy for Dark/Light, wide/narrow, largest text, keyboard, pointer, touch, and every active/empty/failure state.
     - Done when: reviewed deterministic captures prove all four sections and action traces prove every control reaches the sole daemon authority.
  5. S5 Cut over clock, bell, banners, Notification Center, and lock curtain.
     - Objective: give Clock and Notification Center separate persistent entries while preserving weather, battery, health, placement, gestures, and focused-VDI behavior.
     - Inputs: S4, UX-012 S1-S3, FUNC-017 S9, notification ring/toast bridge, health modal, taskbar placements, and secure curtain.
     - Deliverable: clock-to-Clock route, dedicated bell with bounded unread badge, actionable Clock banners, retained Clock notification rows, and idle Clock curtain mode.
     - Chrome behavior: Bottom and Left clock targets open Clock directly; no compact chooser or tray flyout exists. The bell opens Notification Center and shows `99+` at
       the cap; opening marks visible rows read and Clear All removes them. Existing pull-down/search access remains. Weather still opens Maps, health still opens its one
       modal, and weather/battery/clock/bell/placement hit regions and focus order remain disjoint.
     - Alert behavior: alarm banners expose Snooze/Stop; timer banners expose Add 1 minute/Stop; neither navigates away or takes over the screen. Events also enter the
       bounded Notification Center history and existing signed Mesh Teams alert fold. Audio and due state continue if the shell or Clock surface is closed.
     - Curtain behavior: the existing secure lock curtain owns a dark low-glare idle Clock view with 24-hour time, seconds, `Monday 8`, next alarm time/label, and active
       timer summaries. It reveals no peer list and does not weaken PAM, input capture, or focused-VDI gesture guards.
     - Depends on: S4, UX-012 S1-S3, and FUNC-017 S9.
     - Acceptance: clock and bell execute only their assigned routes; banners remain actionable over ordinary and VDI surfaces; no target overlaps or duplicate authority.
     - Validation: shell chrome/input/notification/curtain tests on BigBoy plus Bottom/Left direct-DRM captures on seat `.15`.
     - Done when: action traces and reviewed captures prove the route cutover, badge lifecycle, alert actions, lock privacy, and weather/battery/health non-regression.
  6. S6 Hard-cut legacy state, package, document, and prove release behavior.
     - Objective: land the Clock contract without silently importing obsolete alarms or leaving contradictory docs, routes, services, or installed payloads.
     - Inputs: S1-S5, package policy, platform interface authority, worklist stewardship, CRIT-006/007, and installed-seat upgrade paths.
     - Deliverable: fresh Clock database, one-time display-zone migration, package/service updates, design/governance updates, lint gates, farm evidence, and live fleet proof.
     - Migration behavior: do not read or import `timers-alarms.json`; leave the user file untouched for manual rollback and start Clock with no alarms/timers. Convert the
       five legacy display-zone values to `America/New_York`, `America/Chicago`, `America/Denver`, `America/Los_Angeles`, or `UTC` and persist the IANA result atomically.
     - Documentation behavior: add `docs/design/construct-clock.md` as design authority, update `platform-interfaces.md` and AI governance for clock→Clock and bell→
       Notification Center, update UX-012 dependencies, and remove or supersede prose that claims the clock opens Notification Center or the retired Timers surface.
     - License behavior: pin AOSP DeskClock revision `04e481f37e0b52b74c5a5c7b78b662d1f94e3478` as a behavior reference only. Use shared registry icons and original egui
       layout code; any directly adapted Apache-2.0 code retains headers/NOTICE. Do not add Android dependencies, Android assets, a native APK, a second launcher, or a tray flyout.
     - Depends on: S1-S5, UX-012, FUNC-021, CRIT-006, and CRIT-007.
     - Acceptance: fresh install and upgrade both start deterministically; legacy files are untouched; installed services own the right payload; all live gaps are explicit.
     - Validation: worklist self-test/lint, doc-supersession/style/bus/layer gates, Clock/Music/shell package tests, RPM payload checks, and named live-seat/fleet commands.
     - Done when: one evidence bundle binds revision, farm hosts/slots/results, direct-DRM captures, audio metrics, package identity, and six-node execution/recovery outcomes.
- Scope: Owns Clock contracts, worker, persistence, World Clock/Alarm/Timer/Stopwatch UX, selected-peer execution, Clock audio seam, clock/bell routing, lock-clock content,
  migration, packaging, documentation, and proof. Music retains catalog/provider/credentials/general playback; Notification Center retains general history; UX-012 retains
  taskbar geometry; health and weather retain their existing authorities.
- Relevant files/components: mesh Clock types and mackesd worker/store, mde-musicd Clock-audio seam, shell Clock/chrome/notification/curtain, mde-egui Style/Motion, package
  policy, platform-interface/governance docs, and evidence helpers.
- Dependencies: ARCH-009, FUNC-017, FUNC-021, UX-009, UX-012, CRIT-006, CRIT-007.
- Acceptance criteria:
  1. The AOSP-derived four-section Clock is responsive, 24-hour, IANA/DST-correct, searchable but not pinnable, and fully driven by typed daemon projections/actions.
  2. Alarms/timers survive restart/reboot/suspend, execute on all selected capable peers, converge global Snooze/Stop, honor local opt-out/blocking, and never ring late delivery.
  3. Bundled/local/Music/podcast/NPR/radio audio is catalog-owned, queue-isolated, bounded, audible, ducked/restored, and falls back within three seconds without raw URLs.
  4. Clock, bell, weather, battery, health, placement, Notification Center, banners, and lock curtain retain distinct truthful actions in Bottom and Left placements.
  5. Fresh install, non-importing upgrade, package, direct-DRM/audio, and six-node failure/recovery evidence prove the shipped behavior or name an exact blocker.
- Verification method: contracts on `.90`, worker/store and focused shell tests on `.50`, longest Music/shell/render/fault suites on BigBoy `.130`, then RPM and seat `.15`
  direct-DRM/physical-audio proof followed by six-node target/rejoin/suspend/reboot/source-loss/acknowledgement acceptance. Use explicit farm host/slot variables.
- Origin or merged source IDs: 2026-08-08 Clock Interface 50-question operator survey; AOSP DeskClock reference; existing shell Timers & Alarms implementation; UX-012
  clock/tray, FUNC-017 clock-weather, FUNC-021 Music/radio, Notification Center, and curtain workstreams.

### WL-CRIT-006 - Production evidence, six-node acceptance, and corrected-forward recovery
- Status: Remaining
- Priority: P0
- Complexity: Epic
- Problem: Static tests are strong but one signed release gate does not yet prove CI authority, farm topology, package integrity, five-seat behavior, lighthouses,
  recovery, and corrected-forward deployment together.
- Required outcome: GitHub required checks and farm evidence bind one revision; signed schema-5 evidence proves six-node/five-seat acceptance, package/runtime integrity,
  recovery, and corrected-forward promotion without rollback.
- Current state: evidence schema/signing, topology, recovery, and live/VDI helpers exist; current release binding, seats, lighthouse convergence, and complete failure
  matrix remain.
- **Farm expansion (2026-08-08):** XEN-196 is a verified fifth build node; topology is 5/5 with 10 slots and `.196` passed `mde-bus` 425/425:
  `docs/platform/evidence/WL-CRIT-006-2026-08-08-farm-xen196-r1.md`.
- **Artifact claim checkpoint (2026-08-09):** one capture cannot satisfy independent node/scenario claims; `.90` passed 2 positive and 18 negative fixtures:
  `docs/platform/evidence/WL-CRIT-006-2026-08-09-six-node-artifact-claim-r2.md`.
- **Farm capacity checkpoint (2026-08-09):** sync refuses below the bounded remote `/home` reserve before creating a partial slot; machine 196 passed refusal/success:
  `docs/platform/evidence/WL-CRIT-006-2026-08-09-farm-sync-capacity-r3.md`.
- **Live collector binding checkpoint (2026-08-09):** rehashed arbitrary pass bytes and split role candidates now fail closed; BigBoy and `.90` passed verifier and release
  self-tests: `docs/platform/evidence/WL-CRIT-006-2026-08-09-live-collector-binding-r4.md`.
- **Governed candidate checkpoint (2026-08-09):** CI now derives exact role/runtime digests from immutable final RPMs; the stale `jiff` lock edge is repaired.
  BigBoy gates passed, but the current candidate remains unbuilt: `docs/platform/evidence/WL-CRIT-006-2026-08-09-governed-candidate-path-r5.md`.
- Remaining work:
  1. S1 Define release gate matrix.
     - Objective: list every required check, seat, node, artifact, threshold, owner, and evidence filename for one revision.
     - Inputs: governance, current CI, all active P0/P1 epics.
     - Deliverable: machine-readable gate matrix and linted release plan.
     - Depends on: none.
     - Acceptance: no required gate is implied or duplicated.
     - Validation: worklist/governance/supersession lint.
     - Done when: matrix is reviewed and source revision is pinned.
  2. S2 Bind and sign evidence.
     - Objective: make every result include revision, command, environment, timestamp, hash, and limitation under schema 5.
     - Inputs: evidence helpers and S1.
     - Deliverable: signed evidence bundle and invalid-signature fixtures.
     - Depends on: S1.
     - Acceptance: missing, stale, altered, or unsigned evidence cannot promote.
     - Validation: release-evidence cargo/script tests on .90.
     - Done when: verifier accepts only the intended revision.
  3. S3 Run farm/CI/package gates.
     - Objective: execute required checks on explicit farm slots with BigBoy as long pole and publish artifacts.
     - Inputs: pinned revision and S1/S2.
     - Deliverable: CI run, RPM/payload report, test counts, and logs.
     - Depends on: S2.
     - Acceptance: required GitHub checks are the authoritative merge gate.
     - Validation: full farm cargo/package/secret/architecture gates.
     - Done when: all required checks are green or named blockers are carried.
  4. S4 Run fleet and live-seat acceptance.
     - Objective: deploy the same revision to release seat, Dell, Eagle, seat 15, T480, Surface, and three lighthouses with alert protocol.
     - Inputs: S3, enrollment roster, rollout policy.
     - Deliverable: runtime, GUI, network, audio, VDI, and package captures.
     - Depends on: S3.
     - Acceptance: no stale installed payload or missing seat/lighthouse is treated as pass.
     - Validation: named live-seat and lighthouse scripts.
     - Done when: every matrix row has direct evidence.
  5. S5 Exercise failure and corrected-forward recovery.
     - Objective: inject process, network, sleep, reboot, provider, package, and peer failures and recover by re-enrollment/corrected forward.
     - Inputs: S4 and CRIT-007.
     - Deliverable: fault traces, recovery logs, and no-rollback proof.
     - Depends on: S4.
     - Acceptance: no data loss, secret leak, false health, or service restart storm.
     - Validation: chaos/recovery farm and live commands.
     - Done when: failure matrix and remediation records are signed.
  6. S6 Promote or block honestly.
     - Objective: verify all gates and either publish production promotion or retain Remaining with exact blockers.
     - Inputs: S1-S5.
     - Deliverable: signed release decision and archive entry on closure.
     - Depends on: S5.
     - Acceptance: promotion is impossible with any missing hard gate.
     - Validation: release verifier and final worklist lint.
     - Done when: decision is reproducible from the evidence bundle.
- Scope: Owns release gate authority, schema/signing, farm/CI/package/live evidence, topology, rollout, failure injection, and promotion decision. Feature implementation
  remains in its owner epic.
- Relevant files/components: AI_GOVERNANCE, CI workflow, install-helpers release/evidence/farm scripts, package manifests, docs/platform/evidence, five-seat and
  lighthouse tooling.
- Dependencies: all P0/P1 feature epics, CRIT-007, and the active repository revision.
- Acceptance criteria:
  1. One revision has complete signed farm, package, live-seat, lighthouse, and recovery evidence.
  2. GitHub required checks and verifier reject missing, altered, stale, or mismatched evidence.
  3. Promotion uses corrected-forward recovery and archives the closed epic.
- Verification method: worklist/governance/doc/secret/supersession lints, farm cargo/package gates, release verifier, and named live scripts; longest job on BigBoy.
- Origin or merged source IDs: 2026-07-30 fit-for-purpose audit and archived release/acceptance IDs.

### WL-CRIT-007 - Boot, sleep/resume, and fleet peer return recovery

- Status: Remaining
- Priority: P0
- Complexity: Epic
- Problem: boot and laptop sleep can leave Nebula, mackesd, Syncthing, etcd, and desktop state stale or duplicated.
- Required outcome: every enrolled workstation and lighthouse returns to one authenticated identity, one healthy daemon/session, synchronized substrate, and visible
  recovery state after boot, sleep, reboot, network transition, or corrected-forward upgrade.
- Current state: eight identities and recovery helpers exist; the rejoin helper
  now requires a successful `mackesd leave --yes`, validates that stale
  certificate/key/role state is gone, and rejects unsupported roles before
  joining. Ordering, desktop restoration, and fleet convergence proof remain.
- Remaining work:
- **Identity teardown checkpoint (2026-08-06):** `rejoin-v11-mesh.sh
  --self-test` and the farm `.50` lane
  `crit007-rejoin-selftest-20260806-r1` passed; failed leave or residual
  identity now refuses corrected-forward join. Systemd ordering and
  destructive live rejoin remain open. Evidence:
  `docs/platform/evidence/WL-CRIT-007-2026-08-06-rejoin-identity-r1.md`.
- **Boot order/local identity checkpoint (2026-08-08):** Nebula now rejects
  unsafe, mixed, stale, or untrusted local identity before startup; etcd,
  Syncthing, six mackesd groups, and the shell follow the verified boot graph.
  `.90` systemd/hostile fixtures and all three role-package guards passed.
  Distributed collision authority and live reboot/sleep proof remain. Evidence:
  `docs/platform/evidence/WL-CRIT-007-2026-08-08-boot-order-identity-s1-r1.md`.
- **Peer recovery checkpoint (2026-08-08):** post-resume/network-return recovery
  now refuses offline mutation, coalesces triggers, restores Nebula with bounded
  backoff, then requires configured etcd and Syncthing before XDG/grouped
  mutation. `.90` fault/systemd checks passed. Live laptop/fleet convergence
  remains. Evidence:
  `docs/platform/evidence/WL-CRIT-007-2026-08-09-substrate-order-r2.md`.
- **Recovery role-admission checkpoint (2026-08-09):** recovery now refuses an
  unsupported, malformed, or duplicate role before network, lock, or service
  mutation; BigBoy passed all 9 deterministic fixtures:
  `docs/platform/evidence/WL-CRIT-007-2026-08-09-role-admission-r3.md`.
- **Lighthouse coordination checkpoint (2026-08-09):** missing etcd membership now refuses recovery before mutation; machine 194 passed the full fixture:
  `docs/platform/evidence/WL-CRIT-007-2026-08-09-lighthouse-coordination-admission-r4.md`.
- **Lighthouse desktop-scope checkpoint (2026-08-09):** healthy peer return skips Workstation-only XDG restoration with zero service mutation; BigBoy and machine 194
  passed the complete fixture: `docs/platform/evidence/WL-CRIT-007-2026-08-09-lighthouse-desktop-scope-r5.md`.
- **Eagle rollout preflights (2026-08-09):** Eagle was inspected without mutation; release 12 lacks recovery, while the available release-23 bytes are unsigned,
  source-unbound, and incomplete, so no warning or rollout ran: `docs/platform/evidence/WL-CRIT-007-2026-08-09-eagle-recovery-preflight-r6.md`,
  `docs/platform/evidence/WL-CRIT-007-WL-FUNC-021-2026-08-09-eagle-release23-alignment-r7.md`.
- **Workload/session recovery checkpoint (2026-08-08):** terminal Display1
  recovery now reattaches only the latest valid exact generation and revokes
  superseded, expired, mismatched, orphaned, or stopped-workload leases without
  invoking lifecycle apply/cancel. `.90` passed 3/3; live first-frame proof
  remains: `docs/platform/evidence/WL-CRIT-007-2026-08-08-workload-session-recovery-s3-r1.md`.
- **Corrected-forward Release 21 checkpoint (2026-08-08):** the Fedora 44
  package passed integrity, ABI, payload, transaction, and installed-file
  verification. Warned reboots on seat `.15` and Dell `.225` changed both boot
  IDs and recovered one identity, six unique grouped workers, strict
  coordination quorum, Syncthing/Bus, one shell, and all communal XDG binds.
  Dell also passed explicit network-return recovery while its shut-off Browser
  VM disk remained unchanged. The three persisted lighthouse voters were
  recovered without deleting data and returned to active `3/3` health after
  stale roster and saturated Nebula transport correction. Physical
  suspend/resume and the remaining Eagle, T480, Surface, and lighthouse matrix
  remain. Evidence:
  `docs/platform/evidence/WL-CRIT-007-2026-08-08-corrected-forward-s4-r1.md`.
  1. S1 Define boot dependency order and identity guard.
     - Objective: order network, Nebula, mackesd, etcd, Syncthing, shell, and workload services with one stale-identity cleanup path.
     - Inputs: systemd units, enrollment config, mesh-health checks.
     - Deliverable: unit dependencies and hostile duplicate-identity tests.
     - Depends on: none.
     - Acceptance: no service starts before required identity/network readiness.
     - Validation: systemd syntax and shell cargo tests.
     - Done when: boot graph and tests are recorded.
  2. S2 Implement sleep/network rejoin.
     - Objective: detect suspend/resume and network changes, refresh Nebula, restore etcd/Syncthing, and publish bounded state.
     - Inputs: S1 and mesh health worker.
     - Deliverable: rejoin state machine, backoff, and offline/online fixtures.
     - Depends on: S1.
     - Acceptance: one identity and one session return without duplicate peers or writes.
     - Validation: fault-injection cargo tests and live sleep/network probe.
     - Done when: rejoin trace shows convergence.
  3. S3 Recover workloads and desktop state.
     - Objective: reconcile workload sessions, VDI leases, shell state, and local cache after reboot/suspend.
     - Inputs: ARCH-010 and S2.
     - Deliverable: restart/replay/re-attach tests and recovery UI.
     - Depends on: S2.
     - Acceptance: stale leases die, valid sessions resume, and failures are actionable.
     - Validation: Workload/VDI cargo tests and seat proof.
     - Done when: no duplicate VM/container/session exists.
  4. S4 Prove fleet rollout and corrected forward.
     - Objective: execute boot/sleep/reboot/upgrade recovery on all seats and lighthouses.
     - Inputs: S1-S3, CRIT-006.
     - Deliverable: signed recovery matrix.
     - Depends on: S3.
     - Acceptance: failed nodes re-enroll and recover without rollback or data loss.
     - Validation: farm package gates and live seat/lighthouse scripts.
     - Done when: all matrix rows have evidence or named blockers.
- Scope: Owns identity, systemd ordering, Nebula/etcd/Syncthing rejoin, workload/desktop recovery, upgrade cleanup, and proof. Feature-specific behavior remains with its
  owner epic.
- Relevant files/components: mackesd/mde-shell systemd units, Nebula/mesh-health, etcd/Syncthing, Workload/VDI recovery, enrollment and rollout scripts.
- Dependencies: ARCH-010, ARCH-009, CRIT-006, and the current eight-node roster.
- Acceptance criteria:
  1. Boot, sleep, network transition, reboot, and upgrade restore one authenticated peer/session.
  2. Stale identities, leases, rows, and processes are removed or surfaced as actionable failure.
  3. All seats/lighthouses have direct recovery evidence.
- Verification method: systemd/shell/Workload cargo gates, farm package checks, fault injection, and live recovery scripts; BigBoy runs the broadest gate.
- Origin or merged source IDs: operator boot/sleep peer-return bug and archived recovery incidents.

## User Interface And Experience

### WL-UX-009 - Complete the shared Quazar workspace design language

- Status: Remaining
- Priority: P1
- Complexity: Epic
- Problem: Construct surfaces still diverge in typography, palette, icons, spacing, responsive layout, and motion despite shared primitives.
- Required outcome: every Construct-owned egui surface uses the shared Quazar Style/Visuals, approved fonts/icons, Dark/Light appearances, responsive geometry, semantic
  state language, and bounded motion with no hand-rolled surface styling.
- Current state: shared style and many primitives exist; adoption gaps, icon audit, responsive outliers, and integrated visual proof remain.
- **Workspace-state checkpoint (2026-08-09):** shared panels stay bounded and use active Light tokens at narrow touch geometry; `.50` passed 12/12:
  `docs/platform/evidence/WL-UX-009-2026-08-09-workspace-state-responsive-light-r1.md`.
- Remaining work:
  1. S1 Freeze tokens, fonts, and icon registry.
     - Objective: define the shared Style/Visuals values, licensed fonts, icon semantics, and state colors in one module/registry.
     - Inputs: mde-egui style, platform interfaces, icon assets.
     - Deliverable: registry, license manifest, and drift lint.
     - Depends on: none.
     - Acceptance: no new raw surface style or unlicensed icon is accepted.
     - Validation: style/icon cargo tests and license scan on .50.
     - Done when: registry hash and scan are recorded.
  2. S2 Migrate Construct surfaces.
     - Objective: replace local colors, spacing, typography, and icon choices in shell, Workers, Collaboration, Music, Maps, Browser connection, and Health.
     - Inputs: S1 and owner epic route models.
     - Deliverable: touched surfaces using shared primitives and negative raw-style scan.
     - Depends on: S1.
     - Acceptance: no Construct-owned surface bypasses Style/Visuals.
     - Validation: focused crate tests and architecture scan.
     - Done when: all active surfaces are inventoried and migrated.
  3. S3 Implement responsive and appearance states.
     - Objective: make wide, narrow, tablet, largest-text, Dark, Light, disabled, stale, and unavailable layouts readable and operable.
     - Inputs: S1/S2 and render fixtures.
     - Deliverable: deterministic screenshot fixtures and layout tests.
     - Depends on: S2.
     - Acceptance: no clipping, overlap, hidden control, or contrast failure in supported states.
     - Validation: egui/surface cargo render tests on BigBoy.
     - Done when: fixture set and human review record exist.
  4. S4 Integrate motion and interaction policy.
     - Objective: use centralized DRM-aware motion, focus/keyboard semantics, and event-only repaint without a second loop.
     - Inputs: mde-egui motion/DRM and governance.
     - Deliverable: motion/focus fixtures and repaint bounds.
     - Depends on: S2.
     - Acceptance: no continuously repainting idle surface or per-widget timing authority.
     - Validation: motion/render cargo tests and direct-DRM capture.
     - Done when: motion traces and reduced-motion compatibility evidence exist.
  5. S5 Prove visual consistency.
     - Objective: review all shipped Construct surfaces and package fonts/icons/styles in one release.
     - Inputs: S1-S4 and CRIT-006.
     - Deliverable: signed Dark/Light/large-text capture set and package report.
     - Depends on: S3, S4.
     - Acceptance: human review finds no competing design language.
     - Validation: farm shell tests, RPM payload checks, and named seat captures.
     - Done when: evidence is linked and limitations are explicit.
- Scope: Owns shared egui style, typography, icon registry/licensing, responsive layout, semantic states, motion integration, and visual proof. It does not own guest
  Browser chrome, health evaluation, or taskbar product behavior.
- Relevant files/components: crates/shared/mde-egui style/visuals/motion, shell surface modules, icon/font assets, platform interface/design docs, render fixtures,
  packaging.
- Dependencies: ARCH-008, ARCH-009, FUNC-011, FUNC-017, FUNC-019, FUNC-021, UX-011/012/013/014.
- Acceptance criteria:
  1. All Construct-owned surfaces use one Style/Visuals and licensed registry.
  2. Dark/Light/responsive/largest-text/stale/unavailable captures are legible.
  3. Motion, focus, repaint, package, and human review evidence pass.
- Verification method: style/icon/render cargo gates, license/architecture scans, RPM checks, and direct-DRM/Sunshine captures; longest render gate on BigBoy.
- Origin or merged source IDs: 2026-07-26 unified Quazar theme survey and archived visual workstreams.

### WL-UX-011 - Node hardware providers and safe controls for Workers

- Status: Remaining
- Priority: P0
- Complexity: Epic
- Problem: node hardware and OS observations/actions are incomplete, duplicated, and not consistently capability-driven or safe.
- Required outcome: credential-free Workers providers publish bounded sourced hardware/OS entities and only allow capability-gated, generation-bound, audited safe
  controls for Wi-Fi, audio, display, input, storage, printers, services, power, and virtualization.
- Current state: device catalog and typed provider scaffolding exist; provider coverage, conflict/history, safe actions, and fleet proof remain.
- **Device-control ownership checkpoint (2026-08-09):** privileged controls now
  require an exact match on provider host, category, name, sysfs path, and
  driver; forged and foreign-host targets cannot reach mutation. `.90` passed
  16/16 focused tests:
  `docs/platform/evidence/WL-UX-011-2026-08-09-device-control-ownership-r1.md`.
- **Device-control generation checkpoint (2026-08-09):** stale inventory timestamps cannot reach mutation; `.90` passed 6 contract, 17 executor, and 1 shell test:
  `docs/platform/evidence/WL-UX-011-2026-08-09-device-control-generation-r2.md`.
- **Device-control authorization checkpoint (2026-08-09):** exact-body, short-lived, single-use root-shell capabilities now gate the fixed executor; machine 9 passed
  contract, executor, and shell hostile regressions: `docs/platform/evidence/WL-UX-011-2026-08-09-device-control-authorization-r3.md`.
- Remaining work:
  1. S1 Freeze provider/entity/action contracts.
     - Objective: define source, freshness, capability, entity, conflict, history, export, and action schemas with redaction.
     - Inputs: worker contracts, existing This Node providers, UX-011 survey.
     - Deliverable: bounded versioned contracts and hostile tests.
     - Depends on: ARCH-009 S2.
     - Acceptance: secrets, arbitrary properties, stale generations, and unknown actions fail closed.
     - Validation: mesh-type/provider cargo tests on .90.
     - Done when: contract evidence and source hashes exist.
  2. S2 Implement observation providers.
     - Objective: publish Wi-Fi, network, audio, display, input, storage, printer, service, power, privacy, and virtualization facts with one owner each.
     - Inputs: Fedora APIs and device inventory policy.
     - Deliverable: provider workers, source evidence, unavailable states.
     - Depends on: S1.
     - Acceptance: no fabricated value and no provider can publish another provider's entity.
     - Validation: provider unit/property cargo tests.
     - Done when: coverage matrix is complete or blockers named.
  3. S3 Implement safe staged controls.
     - Objective: preview, authorize, execute, audit, cancel, and recover allowlisted controls only.
     - Inputs: S1/S2 and Workers Action Console.
     - Deliverable: action adapters and refusal/partial-failure tests.
     - Depends on: S2 and ARCH-009 S5.
     - Acceptance: no raw shell, arbitrary path, secret, stale generation, or unconfirmed mutation is accepted.
     - Validation: action-auth and package cargo tests on BigBoy.
     - Done when: every control has preview/result evidence.
  4. S4 Integrate Workers and fleet proof.
     - Objective: render device-by-type/topology/entity details, conflicts, history, scans, and redacted exports across the fleet.
     - Inputs: S1-S3 and UX-009.
     - Deliverable: Workers device_inventory view and five-seat/three-lighthouse evidence.
     - Depends on: S3.
     - Acceptance: stale/failed providers remain visible and export contains no credentials.
     - Validation: shell render, package, and live provider gates.
     - Done when: every supported provider has direct evidence or a named blocker.
- Scope: Owns node observation providers, entity/output/action contracts, safe controls, Workers device inventory, conflict/history/export, package, and proof. Workers
  process split and generic navigation belong ARCH-009.
- Relevant files/components: mesh provider types, mackesd host/device/storage/service/lifecycle workers, shell Workers/device inventory, Fedora
  NetworkManager/udev/pipewire/storage/printer APIs, package policies.
- Dependencies: ARCH-009, ARCH-010 action authority, UX-009, CRIT-006/007.
- Acceptance criteria:
  1. Every provider has one owner, bounded sourced output, freshness, and unavailable state.
  2. Every mutation is staged, capability/generation-bound, audited, cancellable, and safe.
  3. Device inventory and fleet proof expose conflicts without secrets or fabricated data.
- Verification method: provider/property/action cargo suites, authority/security/package scans, render fixtures, and live provider captures; BigBoy runs broad provider
  checks.
- Origin or merged source IDs: 2026-07-26 node hardware and safe-controls survey.

### WL-UX-012 - Full-width Construct taskbar and search-first Home

- Status: Remaining
- Priority: P1
- Complexity: Epic
- Problem: taskbar placement, Start/Search, Home, pins, clock/tray, and health entry still diverge from the operator-locked full-width Construct contract.
- Required outcome: a 48px full-width taskbar supports Bottom/Left placement, icon-only Start/Search/Back/Home, user-managed centered pins, right-side placement control,
  clock/tray semantics, and Bing-wallpaper Home with no second launcher.
- Current state: placement and full-width geometry scaffolding exist; exact icon/action semantics, persistence, responsive behavior, and five-seat proof remain.
- **Live battery (2026-08-08):** the primary UPower percentage/icon is immediately left of the clock in both placements; `.90` passed 24/24 focused status tests:
  `docs/platform/evidence/WL-UX-012-2026-08-08-live-battery-left-clock-r1.md`.
- **Taskbar identity checkpoint (2026-08-09):** connected sessions and pinned desktops now have disjoint typed egui identities and hit regions; BigBoy passed 49/49:
  `docs/platform/evidence/WL-UX-012-2026-08-09-taskbar-control-identity-r2.md`.
- **Narrow geometry checkpoint (2026-08-09):** center controls are admitted only when a physical 40px slot exists, preserving More at 480px and preventing Home overlap at
  320px; `.50` passed 50/50: `docs/platform/evidence/WL-UX-012-2026-08-09-narrow-center-geometry-r3.md`.
- Remaining work:
  1. S1 Freeze geometry and placement.
     - Objective: implement 48px Bottom/Left geometry, safe areas, display ownership, and persisted placement defaults.
     - Inputs: shell navigation, UX-009 Style/Visuals, platform interfaces.
     - Deliverable: placement model and layout fixtures.
     - Depends on: UX-009 S1.
     - Acceptance: taskbar is full width, never overlaps content, and restores a valid placement.
     - Validation: shell render cargo tests on .50.
     - Done when: wide/narrow/largest-text captures pass.
  2. S2 Implement icon actions and Front Door.
     - Objective: make Start open Front Door search, Search focus search, Back navigate, Home open Bing-wallpaper Home, and no Start menu exist.
     - Inputs: S1, Front Door, Home route.
     - Deliverable: typed action map and navigation tests.
     - Depends on: S1.
     - Acceptance: each icon has one action and never launches a raw command or second launcher.
     - Validation: shell navigation cargo tests.
     - Done when: action trace and negative route scan pass.
  3. S3 Implement pins, status slots, clock/bell geometry, and health anchor.
     - Objective: persist centered pins and reserve disjoint typed slots for placement, FUNC-017 weather, battery, clock, bell, tray, and Health without owning their actions.
     - Inputs: S1/S2, UX-013 health authority, and FUNC-017 weather projection/deep link.
     - Deliverable: bounded settings, taskbar projection slots, disjoint weather/battery/clock/bell/tray geometry, and migration tests.
     - Depends on: S2.
     - Acceptance: pins survive restart; every slot remains reachable and non-overlapping; owning surfaces bind actions later; no tray flyout is introduced.
     - Validation: model/property/render cargo tests.
     - Done when: persistence and deep-link evidence exists.
  4. S4 Prove responsive and release behavior.
     - Objective: verify Bottom/Left, Dark/Light, large text, lock, multi-display, session switching, package upgrade, and five-seat captures.
     - Inputs: S1-S3, UX-009, CRIT-006/007.
     - Deliverable: deterministic captures and rollout evidence.
     - Depends on: S3.
     - Acceptance: no clipping, hover-only meaning, duplicate launcher, or focus loss.
     - Validation: shell cargo, package, and live-seat gates.
     - Done when: every required state is directly reviewed.
- Scope: Owns Construct taskbar geometry, placement, actions, pins, status/clock/tray, Home anchor, health entry, persistence, and proof. Guest Browser chrome, Workers
  content, and health evaluation remain elsewhere.
- Relevant files/components: shell nav/taskbar/home/front-door, mde-egui style/motion/input, health/taskbar bridge, settings persistence, render fixtures, package.
- Dependencies: UX-009, UX-013, ARCH-009, FUNC-019, CRIT-006/007.
- Acceptance criteria:
  1. Full-width 48px Bottom/Left taskbar and icon-only actions match the lock.
  2. Pins, placement, clock/tray, health deep link, and Home persist and remain bounded.
  3. Five-seat responsive/package proof passes without a second launcher.
- Verification method: shell model/render/navigation cargo gates, package checks, and direct-DRM/Sunshine captures on named seats.
- Origin or merged source IDs: 2026-07-29 taskbar/Home operator lock and archived dock workstreams.

### WL-UX-013 - System and Mesh Health history and expected-state intelligence

- Status: Remaining
- Priority: P0
- Complexity: Epic
- Problem: the centered Health modal lacks complete expected-state intent, adaptive durations, history/recurrence, safe recovery, and truthful transition handling.
- Required outcome: one centered System and Mesh Health authority distinguishes expected absence from outage, computes A-F grades from signed bounded evidence, keeps
  active issues above paged history, supports filters/detail/recurrence/export, and offers only governed recovery.
- Current state: health contracts, worker, Bus projection, and A-F policy exist;
  the expected-state boundary now covers max-timestamp return and rejects
  oversized availability TTLs. Expected-state publishers, transition
  evaluation, history/detail, recovery/export, and five-seat proof remain.
- Remaining work:
- **Expected-state boundary checkpoint (2026-08-06):** the health contract
  suite covers `Sleeping → Returned` at the `u64::MAX` boundary and refuses an
  overlong TTL; `.50` passed 1/1. Evidence:
  `docs/platform/evidence/WL-UX-013-2026-08-06-health-boundary-r1.md`.
- **Durable ingress checkpoint (2026-08-08):** exact approved-publisher health
  ingress now rejects replay/rollback and atomically preserves its bounded
  per-observer cursor/ledger across restart; `.170` passed 24/24:
  `docs/platform/evidence/WL-UX-013-2026-08-08-health-ingress-checkpoint-s2-r1.md`.
- **Projection freshness checkpoint (2026-08-09):** the roster fold cannot
  outlive its earliest admitted source or the ten-minute contract maximum;
  `.90` passed 14/14 health tests including hostile `u64::MAX` validity:
  `docs/platform/evidence/WL-UX-013-2026-08-09-projection-freshness-r2.md`.
- **Recovery target checkpoint (2026-08-09):** a condition cannot authorize remediation on another node; machine 194 passed 13/13:
  `docs/platform/evidence/WL-UX-013-2026-08-09-recovery-target-binding-r3.md`.
- **Grade E authority checkpoint (2026-08-09):** two distinct active required warnings produce E without duplicate-delivery inflation; machines 9 and 194 passed the
  shared and worker suites: `docs/platform/evidence/WL-UX-013-WL-UX-014-2026-08-09-grade-e-authority-r5.md`.
- **History/selection checkpoint (2026-08-09):** paint-time history retains only the ordered top eight node rows, and live reorder/removal cannot silently move the
  selected detail target. Machine 9 passed both focused tests: `docs/platform/evidence/WL-UX-013-2026-08-09-history-selection-r6.md`.
  1. S1 Freeze health and expected-state contracts.
     - Objective: version bounded signed observations, expected absence, transitions, durations, grades, evidence, and redaction.
     - Inputs: health types, lifecycle/network/maintenance sources.
     - Deliverable: contract/property/schema-skew tests.
     - Depends on: ARCH-009 S2 and UX-011 S1.
     - Acceptance: stale, replayed, contradictory, malformed timestamps and secrets fail closed.
     - Validation: health cargo tests on .90.
     - Done when: contract evidence is signed.
  2. S2 Implement evaluation and escalation.
     - Objective: publish expected state, distinguish planned sleep/shutdown/maintenance from outage, and apply device-aware escalation without false emergencies.
     - Inputs: S1, provider facts, CRIT-007 transitions.
     - Deliverable: evaluator, grade policy, transition fixtures.
     - Depends on: S1.
     - Acceptance: normal laptop/wireless transitions never fabricate warning/critical state.
     - Validation: health/fault-injection cargo tests.
     - Done when: every planned/unplanned case has a trace.
  3. S3 Implement history, detail, filters, and recurrence.
     - Objective: retain bounded active/history records, sort/filter/aggregate recurrence, page 24-hour data, and preserve selection on live updates.
     - Inputs: S1/S2 and retention policy.
     - Deliverable: modal model, history store, hostile paging/filter tests.
     - Depends on: S2.
     - Acceptance: active issues stay above history and no unbounded query materializes.
     - Validation: health/UI cargo tests on BigBoy.
     - Done when: all filter combinations and boundary durations pass.
  4. S4 Implement governed recovery and export.
     - Objective: preview/authorize safe refresh/retry, show progress/partial failure, and emit redacted support bundles.
     - Inputs: S1-S3, ARCH-009 Action Console.
     - Deliverable: recovery adapter, audit records, export verifier.
     - Depends on: S3.
     - Acceptance: arbitrary commands, secrets, stale targets, and unconfirmed mutations are rejected.
     - Validation: action-auth/export cargo tests and secret scan.
     - Done when: successful and failed recovery traces exist.
  5. S5 Integrate modal and prove transitions.
     - Objective: render wide/narrow/largest-text states and test boot/sleep/network/maintenance/outage/rejoin on all seats/lighthouses.
     - Inputs: S1-S4, UX-009/012, CRIT-006/007.
     - Deliverable: visual/live evidence bundle.
     - Depends on: S4.
     - Acceptance: Health is not duplicated in Workers, Collaboration, or Notification Center.
     - Validation: shell render, package, and live transition gates.
     - Done when: every planned/unplanned transition is directly evidenced.
- Scope: Owns health wire/evaluation/history/detail/recovery/export/modal and live proof. Workers owns node management; taskbar owns entry; Kiron owns presentation; no
  second health page or ledger.
- Relevant files/components: mesh health types/workers, lifecycle/network/maintenance publishers, health_modal, taskbar/Workers deep links, action-audit/export,
  systemd/network integration.
- Dependencies: ARCH-009, UX-009, UX-011, UX-012, CRIT-006, CRIT-007.
- Acceptance criteria:
  1. Expected absence, outage, stale, rejoin, grade, duration, recurrence, and remediation are deterministic.
  2. Modal history/detail/filter/export and safe recovery are bounded and redacted.
  3. Five-seat/lighthouse proof shows no false emergency or duplicate authority.
- Verification method: health/property/fault/UI/package cargo gates, secret scans, and direct transition captures; longest health suite on BigBoy.
- Origin or merged source IDs: 2026-08-04 System and Mesh Health survey and archived health authority work.

### WL-UX-014 - Grade-specific cinematic Kiron health lower thirds

- Status: Remaining
- Priority: P1
- Complexity: Epic
- Problem: KIRON has generic typed toasts but no governed A-F payload, authored scenes, audio, ticker, fallback ladder, or bounded health interaction.
- Required outcome: one ToastHost renders six license-clean A-F health scenes and recovery transitions from UX-013 authority, with exact dwell/audio, grouping/ticker,
  safe deep links, live-3D/pre-rendered/static fallback, and no second renderer or sound owner.
- Current state: ToastHost queue, A-F health schema/mapping, sound bridge, motion, and DRM/GLES seams exist; assets, renderer, fallback, ticker, and live proof remain.
- **F-grade backlog checkpoint (2026-08-09):** the hold-until-ack queue is capped at 64 waiters without displacing admitted critical FIFO; BigBoy passed 34/34:
  `docs/platform/evidence/WL-UX-014-2026-08-09-f-grade-backlog-bound-r1.md`.
- **Shared KIRON contract (2026-08-09):** canonical UX-013 grade/generation/timing metadata maps into one ToastHost with safe Workers routing. Grade E now has sole-policy
  production and exact 15-second critical dwell; unknown grades fail closed. Machines 9, 194, and 196 passed focused gates:
  `docs/platform/evidence/WL-UX-014-2026-08-09-shared-health-kiron-contract-r4.md`,
  `docs/platform/evidence/WL-UX-013-WL-UX-014-2026-08-09-grade-e-authority-r5.md`.
- Remaining work:
  1. S1 Freeze authority, payload, and queue.
     - Objective: extend one ToastHost with bounded HealthKironAlert, grouping, severity order, dwell, acknowledgement, and redaction rules.
     - Inputs: UX-013 health contract, mde-egui toast/motion.
     - Deliverable: schema, queue state machine, hostile coalescing/ack tests.
     - Depends on: UX-013 S1-S2.
     - Acceptance: no health recalculation, second queue, ticker store, or duplicate sound path exists.
     - Validation: toast/property cargo tests on .50.
     - Done when: queue traces and schema evidence exist.
  2. S2 Produce governed scenes and audio.
     - Objective: author six original A-F scenes plus recovery transitions and audio with reproducible manifests, hashes, licenses, and size bounds.
     - Inputs: approved art/audio sources and licensing policy.
     - Deliverable: source assets, glTF/pre-rendered/static tiers, waveform/manifest package.
     - Depends on: S1.
     - Acceptance: missing, foreign, oversized, or unlicensed assets fail packaging.
     - Validation: asset/license/package scripts and manifest tests.
     - Done when: reproducible package is verified.
  3. S3 Implement timeline and render tiers.
     - Objective: run deterministic entry/action/settle/morph/recovery/exit timelines on wgpu and direct DRM/GLES with live 3D, pre-rendered, then static fallback.
     - Inputs: S1/S2, mde-egui motion/DRM.
     - Deliverable: backend-neutral state machine, renderer, admission, device-loss recovery.
     - Depends on: S2.
     - Acceptance: exact A=3, B=5, C=6, D=10, E=15, F-until-ack dwell and no duplicate sound.
     - Validation: renderer/property/golden/video cargo tests on BigBoy.
     - Done when: all tiers produce matching semantic traces and captures.
  4. S4 Compose ticker, controls, and interruption policy.
     - Objective: render full-width one-third lower third, fixed ticker, node/device/duration text, safe Workers deep link, lock/immersive/multi-display/redaction policy,
       and audited read-only refresh.
     - Inputs: S1-S3, ARCH-009 Workers, UX-012 taskbar.
     - Deliverable: responsive composition and action/redaction fixtures.
     - Depends on: S3.
     - Acceptance: E/F interrupt per policy; all mutations remain Action Console preview/confirm.
     - Validation: shell render/action/accessibility cargo tests.
     - Done when: every policy state has a capture.
  5. S5 Prove live performance and release.
     - Objective: exercise all grades, fallback, audio, GPU loss, suspend/resume, lock, immersive, reduced motion, multi-display, package upgrade, and five-seat runtime.
     - Inputs: S1-S4 and CRIT-006/007.
     - Deliverable: frames, videos, waveforms, package manifest, and live evidence.
     - Depends on: S4.
     - Acceptance: admitted 1920x1080/60 target or honest fallback; no idle repaint, restart loop, or false health.
     - Validation: farm renderer/package gates and named live-seat scripts.
     - Done when: every required runtime result is evidenced or blocked explicitly.
- Scope: Owns health-grade Kiron schema/queue/scenes/audio/render/fallback/ticker/controls/accessibility/package/proof. UX-013 owns evaluation/history; ARCH-009 owns
  Workers; ordinary alerts and hardware OSD remain unchanged.
- Relevant files/components: mde-egui toast/motion/drm, shell toast bridge/health/taskbar/Workers/audio, mesh health types, governed assets/manifests, RPM/render capture
  tooling.
- Dependencies: UX-013, ARCH-009, UX-009, UX-012, FUNC-011, CRIT-006.
- Acceptance criteria:
  1. One ToastHost renders six distinct grades, correct dwell/audio/grouping/ack, and no duplicate authority.
  2. Live/pre-rendered/static tiers preserve semantics across device loss and all responsive/interruption states.
  3. Asset provenance, package, farm, and five-seat evidence is reproducible.
- Verification method: health/toast/asset/renderer/accessibility cargo gates, package/license checks, golden/video/waveform captures, and live five-seat proof; BigBoy
  runs the longest renderer job.
- Origin or merged source IDs: 2026-08-04 cinematic A-F Kiron survey and archived KIRON/toast workstreams.

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
### Historical validation note — retired Mesh Teams responsive test contract

This note records pre-cutover evidence only. Its Teams/channel/Tasks rails and
responsive contract are retired by WL-FUNC-011 and are not acceptance criteria
for the Mesh Collaboration Suite. The earlier headless failures came from
rendering at 1000px while asserting desktop-only rail/Details content rather
than from a Dell runtime regression; preserve that fact for provenance, but do
not update or carry the superseded fixtures into the new six-section surface.

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
