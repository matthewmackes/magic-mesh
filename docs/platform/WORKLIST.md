# Platform Worklist

This is the only active platform worklist. Design notes, evidence ledgers,
runbooks, and operator notes are inputs, not parallel trackers. Historical
implementation diaries remain in docs/worklist-archive/ and are not executable
tasks.

## Current Snapshot - 2026-08-06 executable story rewrite

- **19 active epics:** 19 `Remaining`, 0 `Blocked`, 0 `Needs clarification`.
- **Latest stable integration:** 43 exact hostile gates passed across four farm hosts: `evidence/WORKLIST-2026-08-11-stable-exact-wave-r473.md`.
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
- **Shared release-proof ownership:** first-release input admission, signed
  artifact/package proof, installed baseline acceptance, corrected-forward
  recovery, and deferred provider/live proofs are owned by `WL-TEST-001`.
  Product epics must not duplicate those rollout tasks; they retain only
  product-specific implementation and integration gaps, and cite `WL-TEST-001`
  when its acceptance is a dependency.
- **Test-seat cap (operator lock 2026-08-10):** no validation, rollout proof,
  capture, chaos, recovery, or acceptance activity may require or exercise more
  than three physical test seats. The default release set is Dell, seat 15, and
  Surface; an epic may substitute another named seat when its hardware is the
  subject, but must remain at three or fewer. Historical five-seat evidence stays
  factual but creates no forward five-seat requirement. Lighthouses are not test
  seats and retain their independently governed three-node quorum proof.
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
- **Rollout lock:** prove each release activity on no more than three selected
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
    under WL-TEST-001.

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
- Problem: overlapping VM/container/session lifecycle state and incomplete local attachment/capacity proof.
- Required outcome: idempotent Workload API owns lifecycle; reconciler actuates libvirt/Quadlet; shell consumes bounded projections with Display1/KMS and RDP/SPICE/VNC recovery.
- Current state: typed authority, the 1,612-test shell gate, and the `mackesd` all-target strict-Clippy/canonical serial gates are green. KMS/EGL live proof and
  repository-wide strict Clippy remain.
- **mackesd all-target quality gate (2026-08-12):** strict Clippy, 4,924 core serial tests, and authority guards are green:
  `evidence/WL-ARCH-010-2026-08-12-mackesd-all-target-gate-r1.md`.
- **Native DRM/PRIME + shell wiring (2026-08-11):** native gates passed 3/3; DRM shell build and VDI module passed 33/33. Live KMS/Display1 remains:
  `evidence/WL-ARCH-010-2026-08-11-native-drm-prime-boundary-r474.md`,
  `evidence/WL-ARCH-010-2026-08-11-shell-drm-build-r475.md`, `evidence/WL-ARCH-010-2026-08-11-vdi-drm-module-r477.md`.
- Remaining work: **Recovered lease deadline (2026-08-10):** expired attachment leases refused; BigBoy: `evidence/WL-ARCH-010-2026-08-10-recovered-lease-deadline-r158.md`.
- **Validating capacity exclusion (2026-08-11):** `.90` exact-fit regression: `docs/platform/evidence/WL-ARCH-010-2026-08-11-validating-capacity-exclusion-r218.md`.
- **VM identity bound (2026-08-11):** BigBoy passed bounded domain/network XML identities: `docs/platform/evidence/WL-ARCH-010-2026-08-11-vm-identity-bound-r221.md`.
- **VM resource-efficiency/Dom0 reserve (2026-08-10):** CPU pinning, bounded queues, qcow2 discard, shared non-Dom0 pools, and Dom0 reserve passed focused farm gates:
  `docs/platform/evidence/WL-ARCH-010-2026-08-10-guest-discard-efficiency-r148.md`, `docs/platform/evidence/WL-ARCH-010-2026-08-10-shared-guest-cpu-pool-r152.md`.
- **Storage probe timeout (2026-08-11):** hanging `df` fails closed; BigBoy passed 1/1: `evidence/WL-ARCH-010-2026-08-11-storage-probe-timeout-r232.md`.
- **Bounded mountinfo (2026-08-11):** storage protection/topology refuse over-1 MiB input; BigBoy passed 1/1: `evidence/WL-ARCH-010-2026-08-11-mountinfo-bound-r233.md`.
- **Bounded compute expose probes (2026-08-11):** firewalld/NM/interface probes time out; `.90` passed 1/1: `evidence/WL-ARCH-010-2026-08-11-compute-expose-timeout-r234.md`.
- **Bounded compute migration probe (2026-08-11):** local `ip` path is timeout-bound; BigBoy passed 1/1: `evidence/WL-ARCH-010-2026-08-11-compute-migrate-ip-bound-r233.md`.
- **Libvirt start replay:** active VM resumes readiness; unrelated errors fail; `.90` 1/1: `evidence/WL-ARCH-010-2026-08-11-libvirt-start-replay-r237.md`.
- **VM/image cleanup:** `evidence/WL-ARCH-010-2026-08-10-vm-overlay-failure-cleanup-r167.md`, `evidence/WL-ARCH-010-2026-08-10-virtual-image-failure-cleanup-r168.md`.
- **Attachment cleanup:** cancellation durably detaches before exact lease revocation; `.50` 1/1: `evidence/WL-ARCH-010-2026-08-11-cancel-presentation-revocation-r258.md`.
- **Terminal detach:** persist before revoke; flush failure withholds effects; BigBoy 1/1: `evidence/WL-ARCH-010-2026-08-11-terminal-detach-ordering-r298.md`.
- **Storage safety:** `.90` passed partition geometry, mountpoint, and storage-name refusal: `evidence/WL-ARCH-010-2026-08-10-partition-geometry-refusal-r174.md`,
  `evidence/WL-ARCH-010-2026-08-10-mountpoint-safety-r175.md`, `evidence/WL-ARCH-010-2026-08-10-storage-name-safety-r177.md`.
- **Label bound (2026-08-11):** 255-byte/control refusal before filesystem commands; BigBoy: `evidence/WL-ARCH-010-2026-08-11-storage-label-admission-r225.md`.
- **Virtual output bound (2026-08-11):** qemu-img drains both streams, retaining 64 KiB each; BigBoy: `evidence/WL-ARCH-010-2026-08-11-virtual-storage-output-bound-r225.md`.
- **Bounded workload capacity probe (2026-08-11):** `/proc/meminfo` input is capped at 64 KiB; BigBoy passed 1/1: `evidence/WL-ARCH-010-2026-08-11-workload-meminfo-bound-r228.md`.
- **Display1 disconnect:** dead relay/input authority is revoked before readiness; `.90` 1/1: `evidence/WL-ARCH-010-2026-08-11-display1-pre-presentation-disconnect-r246.md`.
- **Display1 socket generation:** stale cleanup preserves a newer inode; BigBoy 1/1: `docs/platform/evidence/WL-ARCH-010-2026-08-11-display1-socket-generation-r286.md`.
- **Capacity authority:** completed workloads and prior running generations survive failed retries; `.50`/`.90` 1/1 each:
  `evidence/WL-ARCH-010-2026-08-11-completed-workload-reservation-r367.md`, `evidence/WL-ARCH-010-2026-08-11-failed-retry-reservation-r391.md`.
- **Post-ack presentation:** disconnect revokes frames; BigBoy 1/1: `evidence/WL-ARCH-010-2026-08-11-display1-post-presentation-revocation-r368.md`.
- **Generic session identity:** active IDs cannot retarget workload/route; `.90` 1/1: `evidence/WL-ARCH-010-2026-08-11-generic-session-identity-r369.md`.
- **VDI input generation:** replacement/retry/resize revoke input until a fresh frame; BigBoy 1/1: `evidence/WL-ARCH-010-2026-08-11-vdi-input-generation-r372.md`.
- **Native attachment:** lease bounds/revocation/relay reset passed: `evidence/WL-ARCH-010-2026-08-10-bounded-attachment-lease-window-r165.md`,
  `evidence/WL-ARCH-010-2026-08-10-recovered-ready-without-lease-r166.md`, `evidence/WL-ARCH-010-2026-08-10-display1-relay-loss-reset-r166.md`.
- **Uncommitted lease:** rejected transitions revoke it before final outcomes; `.90`: `evidence/WL-ARCH-010-2026-08-10-uncommitted-attachment-revocation-r124.md`.
- **Cleanup idempotence (2026-08-06):** sole libvirt actuator passed 23/23 on `.90`: `docs/platform/evidence/WL-ARCH-010-2026-08-06-cleanup-idempotence-r1.md`.
- **Dell Display1/RDP:** D-Bus QEMU boots a GL head with disk identity and RDP ready: `evidence/WL-ARCH-008-WL-ARCH-010-2026-08-09-dell-display1-rdp-release26-r92.md`.
- **Admission/live proof:** helper passed placement/resource/retry/lease; Dell was unreachable; seat 15 acceptance refused for missing receipt/projection/operation/generation.
  Evidence: `docs/platform/evidence/WL-ARCH-010-2026-08-06-admission-proof-r1.md`, `docs/platform/evidence/WL-ARCH-010-2026-08-09-dell-seat15-live-acceptance-r15.md`.
- **Native attachment route (2026-08-09):** invalid routes fail before effects; farm proof: `docs/platform/evidence/WL-ARCH-010-2026-08-09-native-attachment-route-r14.md`.
- **Console authority removal:** Workload owns migration/restart and raw console/cloud/shell/Browser paths are gone:
  `evidence/WL-ARCH-010-2026-08-09-restart-journal-r16.md`, `evidence/WL-ARCH-010-2026-08-09-restart-cancellation-ownership-r17.md`,
  `evidence/WL-ARCH-010-2026-08-08-console-authority-removal-r1.md`, `evidence/WL-ARCH-010-2026-08-08-migration-authority-r1.md`.
- **Shell runtime projection:** raw Podman/libvirt/Nova shortcuts are gone; BigBoy passed: `evidence/WL-ARCH-010-2026-08-08-shell-runtime-projection-hard-cut-r4.md`.
- **Heartbeat runtime-projection hard cut (2026-08-08):** peer heartbeats no longer probe or replicate raw Podman/libvirt inventories; remote VM cards consume the
  serving node's typed Workload snapshot, and rolling readers discard retired fields. Evidence:
  `docs/platform/evidence/WL-ARCH-010-2026-08-08-heartbeat-runtime-projection-hard-cut-r5.md`.
- **Retired compute-inventory hard cut (2026-08-09):** network probing no longer reads the retired VM roster; typed Workloads owns runtime identity. Evidence:
  `docs/platform/evidence/WL-ARCH-010-2026-08-09-retired-compute-inventory-hard-cut-r6.md`.
- **Datacenter/XCP hard cut (2026-08-09):** VM actions/roster, both XCP workers/crate/topics, and Server/Hypervisor profiles were deleted; retained rows fail closed. Evidence:
  `docs/platform/evidence/WL-ARCH-010-2026-08-09-datacenter-xcp-authority-hard-cut-r7.md`.
- **Legacy compute-create hard cut (2026-08-09):** the orphan `compute/create/*` worker and direct `virt-install` path were deleted; typed Workloads owns create. Evidence:
  `docs/platform/evidence/WL-ARCH-010-2026-08-09-compute-provision-hard-cut-r8.md`.
- **Authority/contract hardening (2026-08-09):** lifecycle/provision bypasses were deleted; attachment identity, restart replay, and Display1 ownership fail closed. Evidence:
  `docs/platform/evidence/WL-ARCH-010-2026-08-09-contract-restart-display1-hardening-r12.md`.
- **Cloud/Workload recovery checkpoints (2026-08-09):** production authorization and late/replaced Bus activation preserve durable mutation/reply output:
  `docs/platform/evidence/WL-ARCH-010-WL-ARCH-009-2026-08-09-cloud-bus-transaction-recovery-r68.md`,
  `docs/platform/evidence/WL-ARCH-010-WL-ARCH-009-2026-08-09-workload-compute-bus-recovery-r70.md`,
  `docs/platform/evidence/WL-ARCH-010-WL-ARCH-009-2026-08-09-compute-expose-bus-transaction-recovery-r89.md`.
- **Runtime-authority checkpoints (2026-08-09):** direct inventory outside `workload_compute` is rejected. Cloud, Cuttlefish, and both storage walls now consume bounded
  typed Workloads projections; their direct libvirt/Podman runtime rosters and obsolete helpers are deleted. Evidence:
  `docs/platform/evidence/WL-ARCH-010-2026-08-09-runtime-inventory-authority-scanner-r97.md`,
  `docs/platform/evidence/WL-ARCH-010-WL-FUNC-020-2026-08-09-cuttlefish-workload-authority-r101.md`,
  `docs/platform/evidence/WL-ARCH-010-2026-08-09-virtual-storage-workloads-authority-r98.md`,
  `docs/platform/evidence/WL-ARCH-010-2026-08-09-physical-storage-workloads-authority-r102.md`.
- **Migration journal checkpoint (2026-08-08):** cold-migration commands are journaled before effects, replay pending records after restart, clean applied records without
  repeated effects, and pace retryable recovery. Evidence:
  `docs/platform/evidence/WL-ARCH-010-2026-08-08-migration-journal-r2.md`.
- **Distributed migration recovery checkpoint (2026-08-08/09):** one bounded authority persists all cursors, jobs, outboxes, deadlines, and pre-effect claims.
  BigBoy hostile gates cover same-path replacement; returned/join failures after terminal claims become durable Indeterminate state without repeats. Live proof remains:
  `docs/platform/evidence/WL-ARCH-010-2026-08-08-distributed-migration-ledger-r3.md`,
  `docs/platform/evidence/WL-ARCH-009-WL-ARCH-010-2026-08-09-compute-migrate-bus-transaction-recovery-r84.md`.
- **Contract duplicate-key checkpoint (2026-08-06):** recursive Workload JSON rejects duplicate keys; `.50` passed 9/9. Evidence:
  `docs/platform/evidence/WL-ARCH-010-2026-08-06-contract-duplicate-keys-r1.md`.
- **Display1/clipboard/audio authority checkpoints (2026-08-06/10):** readiness, damage, packet-safe frame/FD delivery, lease-bound focused QEMU input, local VM
  audio admission and obsolete clipboard removal are proven; live hardware proof remains. Evidence: `docs/platform/evidence/WL-ARCH-010-2026-08-09-display1-seqpacket-r21.md`,
  `docs/platform/evidence/WL-ARCH-010-2026-08-10-display1-input-audio-r23.md`, `docs/platform/evidence/WL-ARCH-010-2026-08-09-clipboard-authority-hard-cut-s6.md`.
- **Storage Bus transaction checkpoint (2026-08-09):** stable reads precede effects; late/replaced storage and failed publication correct forward without repeated operations.
  BigBoy passed six exact gates: `docs/platform/evidence/WL-ARCH-009-WL-ARCH-010-2026-08-09-storage-bus-transaction-recovery-r79.md`.
- **Durable journal checkpoint (2026-08-06):** persisted journals reject recursive duplicate JSON keys before replay; BigBoy passed 8/8 reconciler tests.
  Evidence: `docs/platform/evidence/WL-ARCH-010-2026-08-06-ledger-duplicate-keys-r1.md`.
- **VDI reconnect checkpoint (2026-08-06):** generation-zero reconnect evidence is refused; BigBoy passed 1/1. Evidence:
  `docs/platform/evidence/WL-ARCH-010-2026-08-06-vdi-reconnect-generation-r1.md`.
- **Journal rollback checkpoint (2026-08-06):** failed atomic phase flushes roll back in-memory status; BigBoy passed 9/9. Evidence:
  `docs/platform/evidence/WL-ARCH-010-2026-08-06-ledger-flush-rollback-r1.md`.
- **Attachment generation checkpoint (2026-08-06):** stale lease generations are rejected; BigBoy passed 1/1. Evidence:
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
  passed 12/12 on `.50`; live Dell/seat-15, native KMS/EGL, packaging-install, and restart proof remain. Evidence:
  `docs/platform/evidence/WL-ARCH-010-2026-08-06-admission-wiring-r1.md`.
- **Dell typed capacity-refusal checkpoint (2026-08-08):** Release 21 published one capability-bound Browser Standard `StartAndAttach`; live four-thread admission
  refused before effects, retained the VM shut off, and produced typed failure. OpenTofu, Ansible, libvirt, KVM, Podman, storage, shell, and all six workers passed.
  Remediation now recommends Small; larger-seat first-frame and lifecycle proof remain:
  `docs/platform/evidence/WL-ARCH-010-2026-08-08-dell-capacity-refusal-r1.md`.
- **Startup/readiness recovery checkpoint (2026-08-09):** stopped guests fail closed; KVM publication recovers replacement. Evidence:
  `docs/platform/evidence/WL-ARCH-010-2026-08-09-startup-readiness-fail-closed-r13.md`, `docs/platform/evidence/WL-ARCH-010-WL-ARCH-009-2026-08-09-kvm-health-bus-recovery-r73.md`.
  Browser catalog/boot boundary: `docs/platform/evidence/WL-ARCH-008-WL-ARCH-010-2026-08-09-browser-vm-catalog-boot-r80.md`.
- **Compute firewall outcome checkpoint (2026-08-09):** root-owned result journaling, restart-safe reply recovery, honest partial/failed projections, and exact Mesh
  removal identity passed machine 194:
  `docs/platform/evidence/WL-ARCH-010-2026-08-09-compute-firewall-outcome-r18.md`.
- **PTY/session broker recovery checkpoints (2026-08-09):** both survive late/replaced Bus storage, skip retained transient opens/lifecycle rows, and execute forward work:
  `docs/platform/evidence/WL-ARCH-010-WL-UX-012-2026-08-09-pty-bus-recovery-r24.md`,
  `docs/platform/evidence/WL-ARCH-010-WL-ARCH-009-WL-CRIT-007-2026-08-09-session-bus-replacement-r71.md`.
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
     - Deliverable: at-most-three-seat and three-lighthouse evidence bundle.
     - Depends on: S7, CRIT-006, CRIT-007.
     - Acceptance: Dell and seat 15 pass first; every selected test seat (maximum three) and lighthouse rejects unsafe placement and recovers.
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
  3. Native/recovery transports pass frame, input, audio, clipboard, reconnect, resize, and cleanup tests; the shell sends typed intent and renders bounded state.
- Verification method: run lint-workload-authority first; use @farm:{cargo test -p mackes-mesh-types}
  @farm:{cargo test -p mackesd workload_compute}
  @farm:{cargo test -p mde-shell-egui --features live-vdi}; BigBoy release/package gates use explicit host/slot and capture live evidence.
- Origin or merged source IDs: Job One 2026-08-05; archived ARCH-006/007, CRIT-001; VDI zero-copy design; current Dell/seat-15 incidents.
### WL-ARCH-008 - Extract the host Browser stack and replace it with a VM Browser

- Status: Remaining
- Priority: P0
- Complexity: Epic
- Problem: CEF/Servo host Browser code, frame copies, helpers, and package seams still compete with the DRM shell and violate the VM-only application boundary.
- Required outcome: Preserve the old stack with history in matthewmackes/magic-mesh-browser-stack, remove it from magic-mesh, and make Surface::Browser start/resume a
  browser-vm that renders guest Chromium over VDI with focused input and guest-owned chrome.
- Current state: Standalone CI and the typed Browser path pass; portable import, guest image/audio quality, and three-seat performance proof remain.
- **Portable migration checkpoints (2026-08-06):** deterministic allowlist, idempotency, symlink, and secret boundaries passed BigBoy and `.50`:
  `docs/platform/evidence/WL-ARCH-008-2026-08-06-portable-profile-r1.md`, `docs/platform/evidence/WL-ARCH-008-2026-08-09-portable-manifest-identity-r2.md`.
- **Display1 rollback checkpoint (2026-08-09):** exact XML restoration passed `.90`: `docs/platform/evidence/WL-ARCH-008-2026-08-09-display1-migration-rollback-r3.md`.
- **Host Browser negative-boundary checkpoint (2026-08-08):** host engine/package policy was removed; boundary lint, metadata, and 11/11 `.90` tests pass:
  `docs/platform/evidence/WL-ARCH-008-2026-08-08-host-browser-negative-boundary-r1.md`.
- **Browser VM artifact-identity checkpoint (2026-08-09):** exact 4/8192/64 profile and bounded qcow2/raw manifests reject stale, hostile, or unsupported artifacts;
  machine 194 passed the focused contract gates: `docs/platform/evidence/WL-ARCH-008-WL-ARCH-010-2026-08-09-browser-vm-image-contract-r72.md`.
  A real admitted 64-GiB qcow2 then passed `qemu-img` integrity: `docs/platform/evidence/WL-ARCH-008-2026-08-09-browser-vm-real-image-r77.md`.
- **Dell Display1/RDP (2026-08-09):** guest RDP boot proof: `docs/platform/evidence/WL-ARCH-008-WL-ARCH-010-2026-08-09-dell-display1-rdp-release26-r92.md`.
- **Host-browser profile (2026-08-10):** manifest self-test passed: `docs/platform/evidence/WL-ARCH-008-2026-08-10-host-browser-profile-refusal-r156.md`.
- Remaining work:
- **Browser runtime path:** xrdp cannot redirect executable lookup outside immutable guest entrypoints; `.196` self-test:
  `evidence/WL-ARCH-008-2026-08-11-browser-runtime-path-r443.md`.
- **Bookmark clock generation:** transplanted clocks cannot roll back snapshot history; `.50` 1/1: `evidence/WL-ARCH-008-2026-08-11-bookmark-clock-generation-r430.md`.
- **App-VM base authority:** conflicting duplicate base declarations fail verification; `.196` passed: `evidence/WL-ARCH-008-2026-08-11-app-vm-base-declaration-r379.md`.
- **Session restart readiness (2026-08-11):** the broker demotes recovered
  historical `Active` rows before first convergence, replacing stale shared
  ready state until a forward authorized reconnect; `.50` passed 1/1:
  `docs/platform/evidence/WL-ARCH-008-2026-08-11-session-restart-readiness-r265.md`.
- **Browser reconnect identity (2026-08-11):** exact replay preserves the live route/transport while retargeting fails closed; BigBoy passed 1/1:
  `docs/platform/evidence/WL-ARCH-008-2026-08-11-browser-reconnect-identity-r245.md`.
- **Lifecycle request correlation (2026-08-11):** Browser start/resume terminal
  rows now require the exact published request ID; BigBoy passed 14/14:
  `docs/platform/evidence/WL-ARCH-008-2026-08-11-browser-request-correlation-r476.md`.
- **Early file-count admission (2026-08-11):** Browser migration carries the
  remaining `MAX_FILES` budget into traversal and refuses the next source entry
  before retaining more candidates; `.50` passed the self-test:
  `docs/platform/evidence/WL-ARCH-008-2026-08-11-file-count-admission-r223.md`.
- **Portable bundle integrity (2026-08-10):** payload size/hash, symlink,
  duplicate, and unexpected-file checks passed `.90`:
  `docs/platform/evidence/WL-ARCH-008-2026-08-10-portable-bundle-integrity-r176.md`.
- **Special-node refusal (2026-08-10):** `.90` passed the migration boundary
  fixture that refuses an allowlisted FIFO instead of silently omitting it:
  `docs/platform/evidence/WL-ARCH-008-2026-08-10-special-node-refusal-r185.md`.
- **Portable publication integrity:** unsafe parents and unrelated outputs fail closed; replacement atomically preserves one complete bundle. Farm gates passed:
  `docs/platform/evidence/WL-ARCH-008-2026-08-10-output-parent-integrity-r192.md`,
  `docs/platform/evidence/WL-ARCH-008-2026-08-11-atomic-bundle-replacement-r279.md`.
- **Browser source-parent integrity (2026-08-10):** symlinked/non-directory
  ancestors are rejected before bundle publication; farm self-tests passed:
  `docs/platform/evidence/WL-ARCH-008-2026-08-10-source-parent-integrity-r212.md`.
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
     - Objective: create the 3-vCPU/8-GiB/64-GiB Dell-safe baseline image and typed Workload profile with Chromium, GPU/video, PipeWire, guest agents, RDP preferred, Sunshine
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
     - Deliverable: timestamped 15-minute metrics, audio proof, RPM proof, and captures from no more than three selected seats.
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
- Verification method: standalone and root cargo gates,
  architecture/secret/package gates, and live video/audio/latency captures on
  no more than three selected seats; put the longest build on
  BigBoy.
- Origin or merged source IDs: 2026-07-28 Option 3; archived WL-PERF-003, FUNC-001..004, ARCH-005; browser-perf-native design.
### WL-ARCH-009 - Process-isolated mackesd and unified Workers interface
- Status: Remaining
- Priority: P0
- Complexity: Epic
- Problem: mackesd remains monolithic, worker ownership and resource budgets are incomplete, and duplicate This Node/Fleet/State surfaces obscure runtime truth.
- Required outcome: six supervised groups publish bounded snapshots; Surface::Workers owns worker tree/graph/inspector/Network Operations/Action Console; remove duplicate surfaces.
- Current state: six groups ship; ownership/UI cutover remains; fleet/package/live proof is post-release: `evidence/WL-ARCH-009-2026-08-11-link-traffic-process-group-r463.md`.
- Remaining work:
- **SQLite authority complete (2026-08-08):** 61 direct writes fell to zero; final 24/24 proof: `docs/platform/evidence/WL-ARCH-009-2026-08-08-sqlite-authority-zero-r11.md`.
- **Action Console evidence (2026-08-08/09):** generation/digest gates: `docs/platform/evidence/WL-ARCH-009-2026-08-09-action-console-digest-binding-r8.md`.
- **Runtime census/aggregate (2026-08-09):** 160 starts fail closed without stable rows; live proof: `evidence/WL-ARCH-009-2026-08-09-release29-runtime-status-live-r104.md`.
- **Runtime freshness (2026-08-10):** empty aggregates expire; BigBoy passed: `docs/platform/evidence/WL-ARCH-009-2026-08-10-runtime-aggregate-freshness-r153.md`.
- **Readiness republish:** failure invalidates healthy probe caches; `.50` 1/1: `evidence/WL-ARCH-009-2026-08-11-readiness-publication-recovery-r301.md`.
- **Bounded job input (2026-08-11):** signed playbooks cap at 1 MiB before digest/apply; BigBoy: `evidence/WL-ARCH-009-2026-08-11-job-playbook-bound-r226.md`.
- **Bounded mesh-DNS directory (2026-08-11):** over-12-peer directories fail closed; BigBoy passed 1/1: `evidence/WL-ARCH-009-2026-08-11-mesh-dns-directory-bound-r229.md`.
- **Bounded Nebula systemctl (2026-08-11):** hung commands die at 2 seconds with 8 KiB caps; BigBoy passed 1/1: `evidence/WL-ARCH-009-2026-08-11-nebula-systemctl-bound-r230.md`.
- **Bounded Netdata overlay IP (2026-08-11):** source files cap at 256 bytes before trim; BigBoy passed 1/1: `evidence/WL-ARCH-009-2026-08-11-netdata-overlay-bound-r231.md`.
- **Fleet reconcile retry:** failed attempts remain due; `.50` passed 1/1: `docs/platform/evidence/WL-ARCH-009-WL-CRIT-007-2026-08-11-fleet-reconcile-retry-r276.md`.
- **Metrics slow-export recovery (2026-08-11):** missed exporter ticks skip bursts; focused gates are complete: `evidence/WL-ARCH-009-2026-08-11-metrics-interval-skip-r222.md`.
- **Service-catalog canonical file (2026-08-11):** crash-left staging files cannot enable uncommitted services; BigBoy passed 1/1:
  `docs/platform/evidence/WL-ARCH-009-2026-08-11-service-catalog-canonical-file-r312.md`.
- **Bounded DC health probe (2026-08-11):** Dom0 SSH hangs fail closed; BigBoy passed 1/1: `evidence/WL-ARCH-009-2026-08-11-dc-health-dom0-timeout-r233.md`.
- **Flat Workers catalog:** leaf-only navigation; BigBoy passed 1/1: `evidence/WL-ARCH-009-2026-08-11-flat-workers-catalog-r236.md`.
- **HTTPS policy (r159):** fallback rejects unsafe configuration; BigBoy passed: `docs/platform/evidence/WL-ARCH-009-2026-08-10-https-policy-loader-r159.md`.
- **HTTPS policy source-parent integrity (2026-08-10):** `.90` passed symlinked-ancestor refusal: `docs/platform/evidence/WL-ARCH-009-2026-08-10-https-policy-parent-r214.md`.
- **Live duplicate owner:** Dell refused a second Control owner; installed owner stayed active: `evidence/WL-CRIT-006-WL-CRIT-007-2026-08-10-release32-f44-three-seat-r126.md`.
- **Cross-process owner:** six groups hold shared-root leases and refuse duplicates; BigBoy: `evidence/WL-ARCH-009-2026-08-10-cross-process-worker-owner-r118.md`.
- **Renamed service owner:** noncanonical packaged units cannot launch `mackesd serve`; `.90` self-test: `evidence/WL-ARCH-009-2026-08-11-renamed-service-owner-r383.md`.
- **Pre-activation ownership:** `serve --group` claims its kernel lease before effects; BigBoy passed: `evidence/WL-ARCH-009-2026-08-11-pre-activation-process-owner-r249.md`.
- **Symlink-safe group leases:** seat `.90` passed kernel-enforced lock-leaf refusal: `docs/platform/evidence/WL-ARCH-009-2026-08-10-group-lease-symlink-r179.md`.
- **Grouped crash isolation:** Release 23 replaced cascading `Requires=` with ordered `Wants=`; seat 15 preserved every unaffected PID/restart counter
  through integrations and control crashes:
  `evidence/WL-ARCH-009-2026-08-08-group-crash-isolation-r2.md`. CI now runs the package verifier, rejecting peer lifecycle coupling and non-exact/unlimited cgroup policy:
  `evidence/WL-ARCH-009-2026-08-11-exact-group-policy-r250.md`.
- **Live cgroup-enforcement checkpoint (2026-08-08):** Release 23 on seat 15 placed six groups in distinct cgroup-v2 paths with package-matched CPU, memory, task, and I/O limits.
  A bounded transient
  128 MiB allocation under a 16 MiB/no-swap boundary was OOM-killed exactly at
  16 MiB; cleanup left the target and every group active. Evidence:
  `docs/platform/evidence/WL-ARCH-009-2026-08-08-live-cgroup-enforcement-r3.md`.
- **Optional-worker quiescence checkpoint (2026-08-08):** an Android catalog
  importer and Flatpak app catalog without local trust anchors now sleep solely
  on shutdown instead of waking every second. Machine 9 proved no Bus state
  creation and prompt cancellation; the target-file format gate passed. Other
  optional providers still require audit. Evidence:
  `docs/platform/evidence/WL-ARCH-009-2026-08-09-registry-app-quiescence-r6.md`.
- **App-sync quiescence checkpoint (2026-08-09):** absent inventory leaves the optional provider shutdown-only without state or polling; `.50` passed 9/9:
  `docs/platform/evidence/WL-ARCH-009-2026-08-09-app-sync-quiescence-r7.md`.
- **Responder group-isolation checkpoint (2026-08-09):** all 20 raw responder
  and maintenance threads now fail closed outside the process group assigned by
  the canonical registry. Exact/hostile argv and bidirectional registry guards
  passed 4/4 focused farm tests. Live package/cgroup census remains. Evidence:
  `docs/platform/evidence/WL-ARCH-009-2026-08-09-responder-group-isolation-r5.md`.
- **Nebula dispatcher ownership checkpoint (2026-08-09):** Control and Observation own distinct registered adapters; the other groups fail closed. Machine 196 passed 4/4
  admission guards; complete worker-role rerun remains: `docs/platform/evidence/WL-ARCH-009-2026-08-09-nebula-dispatcher-ownership-r95.md`.
- **Metrics collector recovery checkpoint (2026-08-09):** missing textfile directory is recreated; symlink substitution fails closed; temporary files are cleaned on failure.
  Machine 194 passed 3/3; Dell deployment proof remains: `docs/platform/evidence/WL-ARCH-009-2026-08-09-metrics-collector-recovery-r10.md`.
- **Metrics bucket ownership:** non-finite bounds are discarded, sorted, and deduplicated; `.90` passed: `evidence/WL-ARCH-009-2026-08-10-metrics-bucket-normalization-r184.md`.
- **Metrics observation ownership checkpoint (2026-08-10):** shared histogram admission discards non-finite observations before they poison `_sum`, `_count`, or bucket publication;
  `.90` passed the hostile regression:
  `docs/platform/evidence/WL-ARCH-009-2026-08-10-metrics-observation-finiteness-r194.md`.
- **Compute Bus recovery (2026-08-09):** complete reads and durable pending output preserve Cloud/storage/workload/migration/scheduler truth across late/replaced Bus:
  `docs/platform/evidence/WL-ARCH-010-WL-ARCH-009-2026-08-09-cloud-bus-transaction-recovery-r68.md`,
  `docs/platform/evidence/WL-ARCH-010-WL-ARCH-009-2026-08-09-compute-expose-bus-transaction-recovery-r89.md`,
  `docs/platform/evidence/WL-ARCH-010-WL-ARCH-009-2026-08-09-workload-compute-bus-recovery-r70.md`,
  `docs/platform/evidence/WL-ARCH-009-WL-ARCH-010-2026-08-09-compute-migrate-bus-transaction-recovery-r84.md`,
  `docs/platform/evidence/WL-ARCH-010-WL-ARCH-009-2026-08-09-scheduler-bus-transaction-recovery-r75.md`.
- **Action Bus recovery checkpoint (2026-08-09):** startup retries Bus open/tail priming as one fail-closed activation, skips retained actions, and executes one
  forward signed action exactly once; BigBoy passed three tests:
  `docs/platform/evidence/WL-ARCH-009-2026-08-09-action-bus-recovery-r14.md`.
- **Copilot Bus recovery checkpoint (2026-08-09):** late activation skips retained asks and answers one forward signed ask exactly once; machine 196 passed three tests:
  `docs/platform/evidence/WL-ARCH-009-2026-08-09-copilot-bus-recovery-r16.md`.
- **Session/desktop-source replacement recovery (2026-08-09):** broker, Roaming, and source roster folds preserve state while skipping retained replacement rows. Gates:
  `docs/platform/evidence/WL-ARCH-010-WL-ARCH-009-WL-CRIT-007-2026-08-09-session-bus-replacement-r71.md`,
  `docs/platform/evidence/WL-FUNC-019-WL-ARCH-009-WL-CRIT-007-2026-08-09-session-roaming-bus-replacement-r78.md`,
  `docs/platform/evidence/WL-ARCH-009-2026-08-09-desktop-sources-bus-recovery-r83.md`.
- **Vehicle transaction recovery (2026-08-09):** replaced Bus storage preserves staged state; a reboot journal prevents repeats. BigBoy passed four exact gates:
  `docs/platform/evidence/WL-FUNC-017-WL-ARCH-009-2026-08-09-vehicle-bus-transaction-recovery-r67.md`.
- **Media-source Bus recovery checkpoint (2026-08-09):** discovery survives late and same-path-replaced storage and republishes the complete roster without restart.
  Exact machine-193/196 gates passed: `docs/platform/evidence/WL-FUNC-021-WL-ARCH-009-2026-08-09-media-sources-bus-recovery-r27.md`,
  `docs/platform/evidence/WL-FUNC-021-WL-ARCH-009-2026-08-09-media-sources-bus-replacement-r81.md`.
- **Media-server Bus recovery checkpoint (2026-08-09):** bounded manifest folds recover late/replaced Bus without partial projection; machine 9 passed eight exact gates:
  `docs/platform/evidence/WL-FUNC-021-WL-ARCH-009-2026-08-09-media-server-bus-transaction-recovery-r82.md`.
- **Notification/transfer Bus recovery checkpoint (2026-08-09):** monitoring and transfer effects survive late/replaced storage; complete registry reads and durable
  identity-bound result receipts prevent lost acknowledgements or repeated transfer effects. Focused farm gates:
  `docs/platform/evidence/WL-ARCH-009-WL-FUNC-011-2026-08-09-notify-bus-recovery-r32.md`,
  `docs/platform/evidence/WL-ARCH-009-WL-FUNC-011-2026-08-09-notify-bus-replacement-r85.md`,
  `docs/platform/evidence/WL-FUNC-016-WL-FUNC-019-WL-ARCH-009-2026-08-09-transfer-bus-transaction-recovery-r69.md`.
- **Clipboard-sync recovery checkpoint (2026-08-09):** six lanes now activate
  and read atomically across late/replaced Bus storage without replaying retained mutations. BigBoy passed four exact tests:
  `docs/platform/evidence/WL-FUNC-016-WL-ARCH-009-2026-08-09-clipboard-sync-bus-recovery-r38.md`.
- **Bookmarks/ad-filter Bus recovery checkpoint (2026-08-09):** both workers
  recover after late storage, atomically skip retained mutations, admit first
  post-activation commands, and preserve durable state. Machine 9 and BigBoy
  passed five exact tests:
  `docs/platform/evidence/WL-FUNC-021-WL-ARCH-009-2026-08-09-bookmarks-bus-recovery-r30.md`,
  `docs/platform/evidence/WL-ARCH-009-WL-FUNC-021-2026-08-09-adfilter-bus-recovery-r31.md`.
- **Datacenter job recovery (2026-08-09):** late startup folds history; unreadable replies cannot regress terminal jobs. Machine 196 passed three exact tests:
  `docs/platform/evidence/WL-ARCH-009-2026-08-09-dc-jobs-bus-recovery-r37.md`.
- **Datacenter audit recovery checkpoint (2026-08-09):** request/output snapshots recover late storage and prevent duplicate projections; machine 9 passed three exact tests:
  `docs/platform/evidence/WL-ARCH-009-2026-08-09-dc-auditor-bus-recovery-r39.md`.
- **Scheduled-snapshot recovery (2026-08-09):** schedule/history reads precede effects; failed result publication cannot repeat effects. Machine 193 passed four exact tests:
  `docs/platform/evidence/WL-ARCH-009-2026-08-09-dc-snap-scheduler-bus-recovery-r42.md`.
- **Navigation recovery (2026-08-09):** reads/publication precede cursor commit; failed writes retry without another provider call. Machine 9 passed six exact tests:
  `docs/platform/evidence/WL-FUNC-017-WL-ARCH-009-2026-08-09-navigation-bus-transaction-recovery-r44.md`.
- **Clock transaction recovery (2026-08-09):** durable command/audio state survives late/replaced Bus and output failure. Machine 194 passed four exact tests:
  `docs/platform/evidence/WL-FUNC-022-WL-ARCH-009-2026-08-09-clock-bus-replacement-r86.md`.
- **Service-catalog projection recovery checkpoint (2026-08-09):** reads/derivations complete before output; write failure remains retryable. BigBoy passed seven exact tests:
  `docs/platform/evidence/WL-FUNC-019-WL-ARCH-009-2026-08-09-service-aggregator-bus-recovery-r45.md`.
- **CUPS action recovery checkpoint (2026-08-09):** both lanes activate atomically; failed replies retry in-process without repeating sync. Machine 193 passed three exact tests:
  `docs/platform/evidence/WL-ARCH-009-2026-08-09-cups-sync-bus-recovery-r46.md`.
- **Weather-location recovery checkpoint (2026-08-09):** complete action/vehicle reads precede effects; failed reads cannot look absent. Machine 9 passed seven exact tests:
  `docs/platform/evidence/WL-FUNC-017-WL-ARCH-009-2026-08-09-weather-location-bus-recovery-r47.md`.
- **Health/Units/forecast recovery checkpoints (2026-08-09):** reads stage before effects and failed replies retry; machines 194/193 and BigBoy passed focused gates:
  `docs/platform/evidence/WL-UX-013-WL-ARCH-009-2026-08-09-health-reconciler-bus-recovery-r48.md`,
  `docs/platform/evidence/WL-UX-013-WL-ARCH-009-2026-08-09-node-grade-bus-recovery-r66.md`,
  `docs/platform/evidence/WL-UX-013-WL-ARCH-009-2026-08-09-node-availability-bus-transaction-recovery-r88.md`,
  `docs/platform/evidence/WL-FUNC-019-WL-ARCH-009-2026-08-09-unit-aggregator-bus-recovery-r49.md`,
  `docs/platform/evidence/WL-FUNC-017-WL-ARCH-009-2026-08-09-weather-forecast-bus-recovery-r50.md`.
- **Atmospheric-map recovery checkpoint (2026-08-09):** late/replaced Bus authority recovers and cache persistence precedes publication; machine 9 passed two exact tests:
  `docs/platform/evidence/WL-FUNC-017-WL-ARCH-009-2026-08-09-weather-atmosphere-bus-recovery-r51.md`.
- **Airspace publication recovery checkpoint (2026-08-09):** failed Bus writes retain one MG90 survey for retry without reprobing; machine 196 passed two exact tests:
  `docs/platform/evidence/WL-FUNC-017-WL-ARCH-009-2026-08-09-airspace-bus-recovery-r52.md`.
- **Onboarding/Voice/mDNS Bus recovery (2026-08-09):** service-add, target apply, Voice, and relay recover late storage and skip retained mutations;
  incomplete reads defer effects. Machines 9/194 and BigBoy passed eight exact tests:
  `docs/platform/evidence/WL-ARCH-009-2026-08-09-service-onboard-bus-recovery-r34.md`,
  `docs/platform/evidence/WL-ARCH-009-2026-08-09-onboard-apply-bus-recovery-r35.md`,
  `docs/platform/evidence/WL-FUNC-011-WL-ARCH-009-2026-08-09-voice-provision-bus-recovery-r36.md`,
  `docs/platform/evidence/WL-ARCH-009-2026-08-09-mdns-relay-bus-transaction-recovery-r91.md`.
- **Catalog/overlay recovery checkpoints (2026-08-09):** staged state and exact context rechecks now gate publication; BigBoy and machines 193/194/9 passed focused tests:
  `docs/platform/evidence/WL-FUNC-018-WL-ARCH-009-2026-08-09-app-catalog-bus-recovery-r55.md`,
  `docs/platform/evidence/WL-FUNC-018-WL-ARCH-009-2026-08-09-android-catalog-bus-recovery-r57.md`,
  `docs/platform/evidence/WL-FUNC-018-WL-ARCH-009-2026-08-09-peer-app-launch-bus-transaction-recovery-r87.md`,
  `docs/platform/evidence/WL-FUNC-017-WL-ARCH-009-2026-08-09-air-quality-replacement-suppression-r65.md`,
  `docs/platform/evidence/WL-FUNC-017-WL-ARCH-009-2026-08-09-iem-radar-bus-recovery-r59.md`,
  `docs/platform/evidence/WL-FUNC-017-WL-ARCH-009-2026-08-09-nws-alert-bus-recovery-r60.md`,
  `docs/platform/evidence/WL-FUNC-017-WL-ARCH-009-2026-08-09-earthquake-overlay-bus-recovery-r61.md`,
  `docs/platform/evidence/WL-FUNC-017-WL-ARCH-009-2026-08-09-firms-overlay-bus-recovery-r62.md`,
  `docs/platform/evidence/WL-FUNC-017-WL-ARCH-009-2026-08-09-transit-overlay-bus-recovery-r63.md`,
  `docs/platform/evidence/WL-FUNC-017-WL-ARCH-009-2026-08-09-wildfire-overlay-bus-recovery-r64.md`,
  `docs/platform/evidence/WL-FUNC-017-WL-ARCH-009-2026-08-09-aircraft-overlay-bus-recovery-r53.md`,
  `docs/platform/evidence/WL-FUNC-017-WL-ARCH-009-2026-08-09-caltrans-overlay-bus-recovery-r54.md`,
  `docs/platform/evidence/WL-FUNC-017-WL-ARCH-009-2026-08-09-traffic-overlay-bus-recovery-r56.md`.
- **Workers navigation and clock checkpoint (2026-08-07):** `Surface::Workers` is the canonical node-management route; prior node routes normalize into it.
  Phones is a Workers subtab absent from launcher/pins; Eastern timestamps apply DST. Focused routes passed; the shell suite passed 1,453 tests with five baseline failures.
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
     - Deliverable: at-most-three-workstation/three-lighthouse evidence bundle.
     - Depends on: S6.
     - Acceptance: bounded redacted snapshots converge without secrets or legacy fallback.
     - Validation: farm chaos/package gates and live captures.
     - Done when: every required failure matrix row has evidence.
- Scope: Owns registry/contracts, six services, budgets, snapshots, Workers UI, Network Operations, Action Console, route deletion, packaging, and fleet proof;
  Workload lifecycle, health modal, and provider implementation remain elsewhere.
- Relevant files/components: mackesd spawn/worker_role, mesh types, process units/RPM, mde-shell-egui Workers/routes, provider workers, and Network Operations design.
- Dependencies: ARCH-010, UX-009, UX-011, FUNC-017, CRIT-006, and CRIT-007.
- Acceptance criteria:
  1. Registry/spawn drift tests prove exactly one owner for every worker and capability.
  2. Six groups run under budgets with bounded credential-free snapshots and one SQLite writer.
  3. Workers and Action Console are the only node-management surfaces; Health remains a separate modal.
  4. Fleet chaos and at-most-three-seat/three-lighthouse evidence passes.
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
- **CAS read-only replay:** canonical bytes are sealed and substitution fails closed; `.196` 1/1: `evidence/WL-FUNC-011-2026-08-11-cas-readonly-replay-r377.md`.
- **CAS purge inode:** concurrent replacements cannot redirect destructive purge; `.50` 1/1: `evidence/WL-FUNC-011-2026-08-11-cas-purge-inode-binding-r428.md`.
- **Import-map inode:** hard-link aliases cannot mutate migration replay authority; BigBoy 1/1:
  `evidence/WL-FUNC-011-2026-08-11-import-map-inode-r449.md`.
- **Actor-log authenticity:** unsigned/invalid/future-schema envelopes fail before durable admission; `.196` 1/1: `evidence/WL-FUNC-011-2026-08-11-actor-log-authenticity-r375.md`.
- **Pipeline signer verification:** actor substitution cannot escape authoring; BigBoy 1/1: `evidence/WL-FUNC-011-2026-08-11-pipeline-signer-verification-r413.md`.
- **Descriptor source generation:** post-hash replacement fails closed; BigBoy 1/1: `evidence/WL-FUNC-011-2026-08-11-descriptor-source-generation-r416.md`.
- **Files CAS registration (2026-08-11):** authenticated staging, worker admission, projection, and rollback passed 15/15 on BigBoy:
  `docs/platform/evidence/WL-FUNC-011-2026-08-11-cas-stream-staging-r275.md`.
- **Calls proof attribution (2026-08-11):** incompatible adapters and altered/vacuous requirements fail before provider evidence; the exact farm gate is capacity-blocked:
  `docs/platform/evidence/WL-FUNC-011-2026-08-11-call-media-proof-attribution-r261.md`.
- **Calls readiness restart:** missing/corrupt readiness revokes stale media proof; BigBoy 1/1: `evidence/WL-FUNC-011-2026-08-11-calls-readiness-restart-r297.md`.
- **Actor-log path identity (2026-08-11):** misplaced `(space, actor)` events fail before append and after restart; `.50` passed 1/1:
  `docs/platform/evidence/WL-FUNC-011-2026-08-11-actor-log-path-identity-r311.md`.
- **Conflicting duplicate checkpoint (2026-08-10/11):** event-ID conflicts fail closed in merge, batch, and the restart-safe actor log; exact replay remains idempotent.
  `.90` passed both exact gates:
  `docs/platform/evidence/WL-FUNC-011-2026-08-10-conflicting-event-duplicates-r157.md`.
- **Alert action ID admission (2026-08-10):** unsafe IDs are rejected before
  lookup/signing; `.90` passed with `.50` format proof:
  `docs/platform/evidence/WL-FUNC-011-2026-08-10-alert-action-id-admission-r210.md`.
- **Files name-operation checkpoint (2026-08-10):** New Folder and Rename are reachable through the existing `FileOps` authority, with bounded validation and atomic
  no-replace rename; machines 193/9 passed seven focused tests: `docs/platform/evidence/WL-FUNC-011-2026-08-10-files-name-operations-r23.md`.
- **Alert delivery restart checkpoint (2026-08-09):** successful Bus/fallback
  delivery now persists bounded no-follow receipts across daemon restart;
  failed delivery retries, and traversal/forged-symlink IDs fail safely.
  Machine 196 passed the exact hostile boundary:
  `docs/platform/evidence/WL-FUNC-011-2026-08-09-alert-delivery-restart-r18.md`.
- **Federation Bus recovery checkpoint (2026-08-09):** enforcement now retries
  late Bus availability without daemon restart and folds valid trust actions
  queued during startup exactly once. Machine 194 passed three exact tests:
  `docs/platform/evidence/WL-FUNC-011-2026-08-09-federation-bus-recovery-r15.md`.
- **Collaboration Bus recovery checkpoint (2026-08-09):** activation now
  retries Bus open and atomically primes every transient lane while preserving
  durable event/log replay; one forward command projects exactly once. Machine
  193 passed three exact recovery/backfill tests:
  `docs/platform/evidence/WL-FUNC-011-WL-ARCH-009-2026-08-09-collab-bus-recovery-r22.md`.
- **Chat Bus recovery checkpoint (2026-08-09):** six mutable lanes now
  tail-prime atomically while signed messages and alert history replay without
  duplicate toasts; one forward send executes once after late storage. Machine
  9 passed four exact activation/recovery tests:
  `docs/platform/evidence/WL-FUNC-011-WL-ARCH-009-2026-08-09-chat-bus-recovery-r26.md`.
- **Notification recovery checkpoint (2026-08-09):** replacement activation skips retained Cloud alerts, preserves failed folds, and primes lanes idempotently:
  `docs/platform/evidence/WL-ARCH-009-WL-FUNC-011-2026-08-09-notify-bus-replacement-r85.md`.
- **Transfer duplicate-admission checkpoint (2026-08-10):** the daemon refuses a replayed
  legacy transfer ID instead of replacing an already-running ledger row; `.90` passed the
  hostile replacement regression: `docs/platform/evidence/WL-FUNC-011-2026-08-10-transfer-duplicate-admission-r186.md`.
- **V2 transfer no-replace checkpoint (2026-08-10):** new typed transfer admission now
  commits with an atomic same-directory no-replace install, closing the check-then-replace
  race that could let a concurrent replay overwrite the first durable row. `.90` passed the
  exact regression: `docs/platform/evidence/WL-FUNC-011-2026-08-10-v2-transfer-no-replace-r195.md`.
- **Destination-generation acknowledgement (2026-08-11):** byte-only commits fail; exact advanced generations and lost-ack replay pass. `.90` passed 1/1:
  `docs/platform/evidence/WL-FUNC-011-2026-08-11-destination-generation-ack-r259.md`.
- **V2 staging restart:** stale residue cannot block bounded create-new retry; BigBoy 1/1: `evidence/WL-FUNC-011-2026-08-11-v2-staging-restart-r306.md`.
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
     - Objective: run offline/online, permission, media, transfer, editor, clipboard, migration, recovery, and at-most-three-seat live acceptance.
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
  3. Three-seat-maximum release proof records real providers, partial failures, and corrected-forward recovery.
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
- **Invalid replacement revocation:** rejected local replacements revoke stale offer/request authority; `.170` 1/1:
  `evidence/WL-FUNC-016-2026-08-11-invalid-replacement-revocation-r376.md`.
- **Native offer revocation:** invalid provider replacement revokes stale DRM selection authority; BigBoy exact:
  `evidence/WL-FUNC-016-2026-08-11-native-offer-revocation-r457.md`.
- **Future VDI envelope admission (2026-08-11):** future-dated clipboard envelopes are rejected before replay admission; `.90` passed the exact regression:
  `docs/platform/evidence/WL-FUNC-016-2026-08-11-vdi-future-envelope-r216.md`.
- **Materialization envelope expiry (2026-08-11):** one-use descriptor authority expires at the earlier lease or envelope deadline; BigBoy passed the exact regression:
  `docs/platform/evidence/WL-FUNC-016-2026-08-11-envelope-expiry-r219.md`.
- **V2 checkpoint ordering (2026-08-11):** a failed durable cursor write stops later rich-envelope materialization; BigBoy passed 1/1:
  `docs/platform/evidence/WL-FUNC-016-2026-08-11-v2-checkpoint-ordering-r241.md`.
- **V2 consent ordering (2026-08-11):** a consent-withheld row blocks later cursor advance until retry; `.170` passed 1/1:
  `docs/platform/evidence/WL-FUNC-016-2026-08-11-v2-consent-ordering-r294.md`.
- **Consent epoch revocation:** re-enable cannot resurrect prior-epoch clipboard content; `.90` 1/1: `evidence/WL-FUNC-016-2026-08-11-consent-epoch-revocation-r393.md`.
- **Consent checkpoint failure:** a failed durable consent write cannot disclose queued content before or after restart; `.90` 1/1:
  `evidence/WL-FUNC-016-2026-08-11-consent-checkpoint-failure-r441.md`.
- **RDP advertised generation:** delayed old requests cannot read unadvertised replacement content; `.50` 1/1: `evidence/WL-FUNC-016-2026-08-11-rdp-advertised-generation-r412.md`.
- **RDP guest-key generation:** a restarted endpoint cannot adopt a replacement TLS key before credentials; BigBoy 1/1:
  `evidence/WL-FUNC-016-2026-08-11-rdp-guest-key-generation-r447.md`.
- **VDI replay expiry:** `.90` passed bounded expired-session cleanup before fresh clipboard admission:
  `docs/platform/evidence/WL-FUNC-016-2026-08-10-vdi-replay-expiry-r182.md`.
- **VDI replay retention:** `.90` passed refusal of an older replay after a newer
  shorter-lived sequence; the lane retains the longest expiry observed:
  `docs/platform/evidence/WL-FUNC-016-2026-08-10-vdi-replay-retention-r185.md`.
- **Guest HTML safety checkpoint (2026-08-10):** active guest CF_HTML is refused before host publication; seat 90 passed the exact live-connect regression:
  `docs/platform/evidence/WL-FUNC-016-2026-08-10-guest-html-safety-r160.md`.
- **RDP bitfield admission (2026-08-10):** malformed 40-byte `BI_BITFIELDS` headers are refused before image materialization; `.50` passed live-connect:
  `docs/platform/evidence/WL-FUNC-016-2026-08-10-rdp-bitfield-admission-r157.md`.
- **Rich-session replay-capacity checkpoint (2026-08-10):** expired signed collaboration sessions release bounded ledger capacity before fresh intake while newer
  replay expiry remains monotonic; machine 9 passed the exact regression: `docs/platform/evidence/WL-FUNC-016-2026-08-10-rich-session-replay-capacity-r121.md`.
- **RDP CF_HTML:** bounded offsets, stale replies, and registered-format equivocation fail closed; `.50`/BigBoy gates passed:
  `docs/platform/evidence/WL-FUNC-016-2026-08-10-rdp-cf-html-r125.md`,
  `docs/platform/evidence/WL-FUNC-016-2026-08-11-rdp-html-format-equivocation-r283.md`.
- **RDP session declaration (2026-08-11):** endpoint/user/domain/geometry substitution is rejected before transport effects; BigBoy passed 1/1:
  `docs/platform/evidence/WL-FUNC-016-2026-08-11-rdp-session-declaration-r295.md`.
- **RDP duplicate-response checkpoint (2026-08-10):** an unsolicited CLIPRDR
  format-data response is now treated as a replay and cannot erase an already
  admitted clipboard value; the focused exact farm regression is recorded in
  `docs/platform/evidence/WL-FUNC-016-2026-08-10-rdp-duplicate-response-r194.md`.
- **RDP image materialization checkpoint (2026-08-10):** host-to-guest PNG/JPEG
  now crosses the one-use permission gate through an exact lease/command-bound,
  root-local Files descriptor authority, bounded decode, and validated
  CF_DIBV5 negotiation. Four focused farm gates passed on `.50`, `.90`, `.170`,
  and `.196`; guest-to-host images and live Windows proof remain:
  `docs/platform/evidence/WL-FUNC-016-WL-ARCH-010-2026-08-10-rdp-image-materialization-r138.md`.
- **RDP guest image admission (2026-08-11):** CF_DIB/CF_DIBV5 responses are
  format-bound, structurally bounded, replay-safe, and production-reachable.
  Until daemon Files/CAS ingest exists the live shell emits truthful
  `FilesProviderUnavailable` and drops raw bytes; exact gates are
  capacity-deferred and the guest-to-host image gap remains open:
  `docs/platform/evidence/WL-FUNC-016-2026-08-11-rdp-guest-image-admission-r270.md`.
- **Expired consent capacity checkpoint (2026-08-10):** every clipboard consent sweep removes expired authority before admission, including an empty sweep; machine 193
  passed the exact denial regression: `docs/platform/evidence/WL-FUNC-016-2026-08-10-consent-capacity-cleanup-r22.md`.
- **Permission replay-expiry checkpoint (2026-08-09):** terminal replay marks
  now expire at their admitting envelope/lease boundary; newer terminal
  sequences extend both high-water and retention monotonically, while renewed
  signed authority can reuse sequencing at exact expiry. Machine 9 passed both
  focused boundary tests:
  `docs/platform/evidence/WL-FUNC-016-2026-08-09-replay-mark-expiry-s5-r16.md`.
- **Mesh replay-expiry retention (2026-08-10):** restart recovery preserves the
  longest expiry across newer shorter generations; `.90` passed:
  `docs/platform/evidence/WL-FUNC-016-2026-08-10-mesh-replay-expiry-r205.md`.
- **Mesh expired-replay cleanup checkpoint (2026-08-09):** expired source/session
  high-water marks are removed before generation validation, so a stale hostile
  generation cannot block a valid session reuse. Machine 9 passed the exact
  hostile-generation fixture:
  `docs/platform/evidence/WL-FUNC-016-2026-08-09-mesh-expired-replay-r17.md`.
- **Clipboard bridge Bus checkpoint (2026-08-09):** startup now retries late
  Bus availability after a fail-closed tail prime; live read failures retain
  cursor/pending work and recover one queued action exactly once. Machine 194
  passed five exact recovery/replay tests:
  `docs/platform/evidence/WL-FUNC-016-2026-08-09-clipboard-bridge-bus-recovery-r20.md`.
- **Clipboard sync Bus checkpoint (2026-08-09):** startup preserves durable
  receive checkpoints, skips retained mutation lanes, and defers every effect after an incomplete six-lane read. BigBoy passed four exact tests:
  `docs/platform/evidence/WL-FUNC-016-WL-ARCH-009-2026-08-09-clipboard-sync-bus-recovery-r38.md`.
- **VDI orphan-gate cleanup checkpoint (2026-08-09):** disconnected transport tickets now fail visibly, release their permission gate, retain stale-sequence replay
  protection, and admit newer rich-MIME reconnect work. BigBoy passed the exact hostile HTML reconnect regression 1/1:
  `docs/platform/evidence/WL-FUNC-016-2026-08-09-vdi-orphan-gate-cleanup-r21.md`.
- **Transfer transaction checkpoint (2026-08-09):** complete Files registry reads and generation-bound durable result receipts recover replacement without repeated copy.
  Machine 9 passed 12 exact gates: `docs/platform/evidence/WL-FUNC-016-WL-FUNC-019-WL-ARCH-009-2026-08-09-transfer-bus-transaction-recovery-r69.md`.
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
     - Deliverable: UI model, redacted audit rows, package policy, and proof on no more than three seats.
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
  3. At-most-three-seat local/mesh/VDI evidence shows bounded memory and cleanup.
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
- Current state: typed providers exist; offline data and live proof remain. Evidence: `evidence/WL-FUNC-017-2026-08-11-mg90-source-generation-r470.md`.
- **Current/forecast provider (2026-08-08):** generation-bound 5/10-minute NWS refresh, provider freshness, bounded cache/retry, and off-runtime I/O passed 8/8 twice;
  live NWS/Maps proof remains: `docs/platform/evidence/WL-FUNC-017-2026-08-08-weather-provider-s3-r1.md`.
- **Atmospheric provider (2026-08-08):** exact nowCOAST WMS identity, bounded PNG/cache, and latest-wins dual-generation viewport admission passed ten focused tests;
  GUI publication/live proof remains: `docs/platform/evidence/WL-FUNC-017-2026-08-08-atmospheric-map-provider-s4-r1.md`.
- **Clock weather launcher:** typed routing/geometry passed 5/5; live captures remain: `docs/platform/evidence/WL-FUNC-017-2026-08-08-clock-weather-launcher-s9-r1.md`.
- **Navigation authority (2026-08-09):** route/progress/replay/restart passed 9/9; generation-exhaustion atomicity passed 4/4:
  `docs/platform/evidence/WL-FUNC-017-2026-08-08-navigation-authority-s6-r1.md`; `docs/platform/evidence/WL-FUNC-017-2026-08-09-navigation-generation-atomicity-r2.md`.
- **Offline catalog binding (2026-08-09):** replacement/expiry revokes tiles and schema-v1 upgrades open empty instead of failing; `.90` passed 7/7:
  `docs/platform/evidence/WL-FUNC-017-2026-08-09-offline-catalog-binding-r3.md`.
- **MG90 roster (2026-08-09):** approved selection owns v2 and loss stops claims; `.90` passed 15/15: `docs/platform/evidence/WL-FUNC-017-2026-08-09-mg90-roster-runtime-r5.md`.
- Remaining work:
- **Status weather identity (2026-08-11):** conditions require exact effective-location coordinates; BigBoy passed 1/1:
  `docs/platform/evidence/WL-FUNC-017-2026-08-11-status-weather-coordinate-r308.md`.
- **MG90 failover safety:** source publication epoch preserved across manager loss; `.90`: `docs/platform/evidence/WL-FUNC-017-2026-08-10-mg90-nonselected-loss-r193.md`.
- **MG90 radio refresh failure (2026-08-11):** retained Cellular/Wi-Fi rows become stale and lose active-path claims; `.50` passed 1/1:
  `docs/platform/evidence/WL-FUNC-017-2026-08-11-mg90-radio-stale-r304.md`.
- **Offline timeline (r160):** impossible access order fails closed; BigBoy passed `docs/platform/evidence/WL-FUNC-017-2026-08-10-offline-timeline-r160.md`.
- **Offline basemap admission:** unsafe candidates fail closed; `.90` 6/6; live proof remains: `docs/platform/evidence/WL-FUNC-017-2026-08-10-basemap-region-admission-r145.md`.
- **Offline index recovery:** hostile metadata fails closed; machine 193: `docs/platform/evidence/WL-FUNC-017-2026-08-09-offline-index-corruption-recovery-r4.md`.
- **Basemap cache reload:** `.90` passed atomic replacement: `docs/platform/evidence/WL-FUNC-017-2026-08-10-basemap-cache-revalidation-r215.md`.
- **Weather cache recovery:** restart binds source identity and rejects malformed state; `.194`: `evidence/WL-FUNC-017-2026-08-09-weather-cache-identity-r6.md`.
- **Atmospheric cache quarantine:** malformed bytes leave authority before fallback; `.90` 1/1: `evidence/WL-FUNC-017-2026-08-11-atmosphere-cache-quarantine-r237.md`.
- **Atmospheric viewport restart:** retained geometry/generation match source identity; BigBoy 1/1: `evidence/WL-FUNC-017-2026-08-11-atmosphere-viewport-restart-r305.md`.
- **Location provenance:** same-generation substitution discards snapshots; BigBoy 1/1: `evidence/WL-FUNC-017-2026-08-11-location-provenance-revalidation-r374.md`.
- **Future cache fallback (2026-08-11):** `.50` passed: `docs/platform/evidence/WL-FUNC-017-2026-08-11-future-cache-fallback-r219.md`.
- **Governed route provider:** signed bounded loopback and stale-result refusal; `.90` 2/2: `evidence/WL-FUNC-017-2026-08-11-provider-route-freshness-r255.md`.
- **Navigation source inodes:** replacement routes and hard-linked gazetteers fail closed; `.90`/BigBoy 1/1:
  `evidence/WL-FUNC-017-2026-08-11-navigation-authority-inode-r406.md`, `evidence/WL-FUNC-017-2026-08-11-gazetteer-inode-r450.md`.
- **Route identity:** replacement geometry cannot reuse the active ID; BigBoy 1/1: `evidence/WL-FUNC-017-2026-08-11-route-identity-replacement-r296.md`.
- **Navigation action retry:** cursors follow effects and publication retries without recalculation; machines 193/9 passed seven tests:
  `evidence/WL-FUNC-017-2026-08-09-navigation-action-retry-r7.md`, `evidence/WL-FUNC-017-WL-ARCH-009-2026-08-09-navigation-bus-transaction-recovery-r44.md`.
- **Vehicle audit-truth checkpoint (2026-08-09):** an MG90 reboot reports
  `audited=true` only after its AdminAction row commits; audit failure preserves
  the applied reboot while returning `audited=false` and a bounded error. BigBoy passed both exact fixtures:
  `docs/platform/evidence/WL-FUNC-017-2026-08-09-vehicle-audit-truth-r8.md`.
- **Vehicle crash/Bus transaction checkpoint (2026-08-09):** durable reboot
  claims/results prevent duplicate gateway and audit effects, while staged
  roster/publication state recovers late or replaced storage. BigBoy passed
  four exact gates:
  `docs/platform/evidence/WL-FUNC-017-WL-ARCH-009-2026-08-09-vehicle-bus-transaction-recovery-r67.md`.
- **Weather-location Bus recovery checkpoint (2026-08-09):** durable authority
  survives unavailable storage, and complete weather-action/vehicle-fix reads
  precede mutation or projection. Machine 9 passed seven exact tests:
  `docs/platform/evidence/WL-FUNC-017-WL-ARCH-009-2026-08-09-weather-location-bus-recovery-r47.md`.
- **Weather-forecast transaction checkpoint (2026-08-09):** late/replaced Bus
  storage recovers, effective location is rechecked after provider I/O, and both
  requested projections serialize before writes. Machine 193 passed three exact tests:
  `docs/platform/evidence/WL-FUNC-017-WL-ARCH-009-2026-08-09-weather-forecast-bus-recovery-r50.md`.
- **Atmospheric-map transaction checkpoint (2026-08-09):** exact location/viewport identity is rechecked after NOAA I/O; fresh cache precedes publication. Machine 9 passed:
  `docs/platform/evidence/WL-FUNC-017-WL-ARCH-009-2026-08-09-weather-atmosphere-bus-recovery-r51.md`.
- **Airspace publication checkpoint (2026-08-09):** late Bus startup recovers,
  and a failed write retries the same bounded survey without another MG90
  probe. Machine 196 passed two exact tests:
  `docs/platform/evidence/WL-FUNC-017-WL-ARCH-009-2026-08-09-airspace-bus-recovery-r52.md`.
- **Vehicle-overlay transaction checkpoints (2026-08-09):** aircraft, Caltrans,
  and NCDOT recheck context after provider I/O; failed reads/writes commit no state:
  `docs/platform/evidence/WL-FUNC-017-WL-ARCH-009-2026-08-09-aircraft-overlay-bus-recovery-r53.md`,
  `docs/platform/evidence/WL-FUNC-017-WL-ARCH-009-2026-08-09-caltrans-overlay-bus-recovery-r54.md`,
  `docs/platform/evidence/WL-FUNC-017-WL-ARCH-009-2026-08-09-traffic-overlay-bus-recovery-r56.md`.
- **Environmental-overlay transaction checkpoints (2026-08-09):** exact post-I/O context, late/replaced storage, failed writes, validators, and transition suppression
  now gate Air Quality, IEM radar, NWS alerts, Earthquake, and Transit; machines 193/194/9 and BigBoy passed focused hostile recovery tests:
  `docs/platform/evidence/WL-FUNC-017-WL-ARCH-009-2026-08-09-air-quality-bus-recovery-r58.md`.
  `docs/platform/evidence/WL-FUNC-017-WL-ARCH-009-2026-08-09-air-quality-replacement-suppression-r65.md`.
  `docs/platform/evidence/WL-FUNC-017-WL-ARCH-009-2026-08-09-iem-radar-bus-recovery-r59.md`.
  `docs/platform/evidence/WL-FUNC-017-WL-ARCH-009-2026-08-09-nws-alert-bus-recovery-r60.md`.
  `docs/platform/evidence/WL-FUNC-017-WL-ARCH-009-2026-08-09-earthquake-overlay-bus-recovery-r61.md`.
  `docs/platform/evidence/WL-FUNC-017-WL-ARCH-009-2026-08-09-firms-overlay-bus-recovery-r62.md`.
  `docs/platform/evidence/WL-FUNC-017-WL-ARCH-009-2026-08-09-transit-overlay-bus-recovery-r63.md`.
  `docs/platform/evidence/WL-FUNC-017-WL-ARCH-009-2026-08-09-wildfire-overlay-bus-recovery-r64.md`.
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
      - Deliverable: production wiring and default-on Workstation weather worker; responsive Maps/Car captures; package/service policy; at-most-three-seat/MG90/weather evidence;
        updated `docs/design/platform-interfaces.md` and refreshed `docs/design/maps-live-overlays.md` that describes shipped rather than planned providers.
      - Farm routing: rerun `farm-topology.sh table`; use distinct free slots with mesh contracts on `.90`, focused async workers on `.50`, and the longest
        Maps/shell/full gate on BigBoy `.130`. Run worklist self-test before lint, then doc-supersession and style-leak gates.
      - Live matrix: on release seat `.15`, exercise fresh fix, manual override, return to Auto, provider loss/reconnect, restart persistence, Bottom/Left, Dark/Light,
        icon-only fallback, offline maps/routes, sleep/rejoin, radio source loss, and MG90 recovery. Publish the required five-second AI alert before seat mutation.
      - Depends on: S5-S9, ARCH-009, ARCH-010, UX-009, UX-012, CRIT-006, CRIT-007.
      - Acceptance: no GUI-owned provider, network I/O, duplicate destination, fabricated data, secret, unbounded cache, stale installed payload, or undocumented live gap;
        missing hardware/provider access is recorded honestly and cannot become a production pass by inference.
      - Validation: focused cargo gates, full CI gate, package/RPM ownership checks, doc/worklist/style lints, direct-DRM captures,
        provider traces, and fleet proof on no more than three seats.
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
  6. At-most-three-seat and MG90/weather-provider proof covers live/manual/offline/provider-loss/restart/sleep/rejoin and package upgrade.
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
- **Catalog replacement authority:** replaced Flatpak state cannot retain launch authority; `.90` 1/1: `evidence/WL-FUNC-018-2026-08-11-flatpak-catalog-replacement-r461.md`.
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
- **Governed App-VM RPM supply (2026-08-11):** local image builds admit one
  bounded immutable `magic-mesh` RPM, verify its governed signature and exact
  compile-time source revision before/after staging and inside the build, and
  enable DNF local signature checking; repo installs verify both ELF identities
  and exact owning-RPM SHA-256 manifests before layering. Hostile fixtures passed:
  `docs/platform/evidence/WL-FUNC-018-2026-08-11-governed-rpm-supply-r253.md`.
- **Bounded persistence recovery (2026-08-11):** retained App-catalog and
  durable-cursor reads refuse data beyond declared limits; `.90` passed:
  `docs/platform/evidence/WL-FUNC-018-2026-08-11-bounded-persistence-read-r224.md`.
- **Durable App-catalog restart cursor (2026-08-11):** committed rows are checkpointed and skipped after restart without emitting an idempotent replay; `.90` passed:
  `docs/platform/evidence/WL-FUNC-018-2026-08-11-app-catalog-restart-cursor-r216.md`.
- **Authenticated first-launch handoff (2026-08-11):** cold boot publishes signed `StartAndAttach` then identity-bound VDI `OpenApp`; replay is effect-idempotent.
  BigBoy/`.90` passed 5/5:
  `docs/platform/evidence/WL-FUNC-018-2026-08-11-first-launch-cold-boot-r239.md`.
- **App-open identity:** active sessions bind catalog revision/capabilities/resume; stale/future substitution emits no extra effects; `.50`/`.90` 1/1 each:
  `evidence/WL-FUNC-018-2026-08-11-app-open-declaration-identity-r285.md`, `evidence/WL-FUNC-018-2026-08-11-active-app-catalog-revision-r390.md`.
- **Restart readiness:** recovered `Connected` requires a forward generation; `.50` 1/1: `evidence/WL-FUNC-018-WL-ARCH-010-2026-08-11-app-vm-restart-readiness-r403.md`.
- **Front Door serving route:** unsafe node IDs fail before App launch emission; `.50` 1/1: `evidence/WL-FUNC-018-2026-08-11-front-door-serving-route-r424.md`.
- **App-VM base variable:** hard-coded substitute bases fail the image contract; `.50` passed: `evidence/WL-FUNC-018-2026-08-11-app-vm-base-variable-r408.md`.
- **App-VM ExecStart authority:** one active canonical runtime is required; `.196` self-test: `evidence/WL-FUNC-018-WL-ARCH-008-2026-08-11-app-vm-execstart-authority-r425.md`.
- **App-VM base image ID:** mutable tags cannot substitute build inputs; `.196` self-test:
  `evidence/WL-FUNC-018-WL-ARCH-008-2026-08-11-app-vm-base-image-id-r427.md`.
- **App session client binding:** restart cannot rebind another initiating seat; `.90` 1/1: `evidence/WL-FUNC-018-WL-ARCH-010-2026-08-11-app-session-client-binding-r433.md`.
- **Front Door equivocation (2026-08-11):** conflicting declarations suppress only their app identity; BigBoy passed 1/1:
  `docs/platform/evidence/WL-FUNC-018-2026-08-11-front-door-equivocation-r299.md`.
- **Launch-action admission checkpoint (2026-08-10):** installed Flatpak rows without exact `launch` authority are withheld before App-VM projection; seat 90 passed:
  `docs/platform/evidence/WL-FUNC-018-2026-08-10-launch-action-admission-r162.md`.
- **Blocked App-VM authorization checkpoint (2026-08-10):** stale, unavailable, or malformed blocked Flatpak rows now fail before root authorization or Bus payload;
  `.90` passed the focused regression: `docs/platform/evidence/WL-FUNC-018-2026-08-10-blocked-appvm-authorization-r127.md`.
- **App VM generation rollback (2026-08-10):** later lower-generation runtime
  rows are refused; `.90` passed:
  `docs/platform/evidence/WL-FUNC-018-2026-08-10-appvm-generation-rollback-r156.md`.
- **App VM target admission (2026-08-10):** empty, control-bearing, and
  path-like serving/client peer or VM identities are refused before session
  roster mutation; `.50` passed the hostile regression:
  `docs/platform/evidence/WL-FUNC-018-2026-08-10-appvm-target-admission-r186.md`.
- **App VM capability admission (2026-08-10):** catalog-backed Front Door
  requests now apply the closed App VM capability policy before root
  authorization; unsupported host capabilities cannot reach the authorizer.
  `.90` passed the hostile regression:
  `docs/platform/evidence/WL-FUNC-018-2026-08-10-appvm-capability-admission-r192.md`.
- **Admitted capability projection (2026-08-10):** App-VM sessions reject
  unsupported host capabilities at projection; `.90` passed:
  `docs/platform/evidence/WL-FUNC-018-2026-08-10-admitted-capability-projection-r206.md`.
- **Catalog side-effect retry checkpoint (2026-08-09):** import cursors advance
  only after governed projection/status effects succeed, and expiry retains
  authority until its retraction publishes. Machine 194 passed the exact
  failure/retry regression:
  `docs/platform/evidence/WL-FUNC-018-2026-08-09-catalog-side-effect-retry-s1-r5.md`.
- **App Catalog Bus transaction checkpoint (2026-08-09):** cursor, catalog,
  watermark, status, recovery, and Bus identity stage until required writes
  succeed. BigBoy passed 10 module tests plus two exact recovery cases:
  `docs/platform/evidence/WL-FUNC-018-WL-ARCH-009-2026-08-09-app-catalog-bus-recovery-r55.md`.
- **Android Catalog Bus checkpoint (2026-08-09):** late/replaced storage
  replays durable authority and retries failed publication before cursor/state
  commit. BigBoy passed two exact recovery cases:
  `docs/platform/evidence/WL-FUNC-018-WL-ARCH-009-2026-08-09-android-catalog-bus-recovery-r57.md`.
- **Peer-app launch recovery (2026-08-09):** durable effect claims prevent ambiguous relaunch; late/replaced Bus results correct forward. Machine 9 passed four gates:
  `docs/platform/evidence/WL-FUNC-018-WL-ARCH-009-2026-08-09-peer-app-launch-bus-transaction-recovery-r87.md`.
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
     - Objective: verify sandbox, resource limits, package upgrade, app data persistence, reconnect, and acceptance on no more than three seats.
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
  3. Three-seat-maximum security and package proof passes without host app installation.
- Verification method: catalog, image, Workload, shell, package, SELinux, and live VDI cargo gates; BigBoy runs image/build jobs.
- Origin or merged source IDs: 2026-07-31 Flatpak Front Door decision and archived app-launch workstreams.

### WL-FUNC-019 - Make Remote Sessions the universal resource browser

- Status: Remaining
- Priority: P0
- Complexity: Epic
- Problem: Remote Sessions is a narrow desktop chooser and does not admit all governed resources, typed capabilities, provenance, or safe actions.
- Required outcome: one universal resource browser discovers peers, VMs, containers, Apps, Android apps, media, files, and services; deduplicates them by stable identity;
  exposes typed Open/Start/Resume/Transfer actions; and never launches an untrusted or ambiguous resource.
- Current state: contracts, adapters/deduplication, a pure searchable model, and fail-closed actions exist; route fixtures, captures, and live recovery remain.
- Remaining work:
- **Action-reply generation:** stale receipts cannot become cancellation handles; `.170` 1/1: `evidence/WL-FUNC-019-2026-08-11-action-reply-generation-r281.md`.
- **Peer freshness:** hostile remote identities cannot authorize resources; BigBoy 1/1: `evidence/WL-FUNC-019-2026-08-11-peer-directory-freshness-r287.md`.
- **Stale peer health:** expired membership cannot retain `healthy`; BigBoy 1/1: `evidence/WL-FUNC-019-2026-08-11-stale-peer-resource-health-r394.md`.
- **Peer-card admission:** malformed rows cannot authorize downstream reads; BigBoy 1/1: `evidence/WL-FUNC-019-2026-08-11-peer-card-admission-r402.md`.
- **Desktop heartbeat:** zero/future observations cannot authorize resources; BigBoy 1/1: `evidence/WL-FUNC-019-2026-08-11-desktop-heartbeat-freshness-r307.md`.
- **mDNS name collision:** LAN names cannot inject peer transports; `.170` 1/1: `evidence/WL-FUNC-019-2026-08-11-mdns-name-collision-r405.md`.
- **Fresh-probe actions:** only fresh probe-confirmed services expose action; `.90` passed: `evidence/WL-FUNC-019-2026-08-11-service-action-fresh-probe-r217.md`.
- **Service-record freshness:** replay cannot renew stale/zero/future health; `.170` 1/1: `evidence/WL-FUNC-019-2026-08-11-service-record-freshness-r415.md`.
- **Catalog generation:** mid-stage advancement and malformed forward snapshots revoke mixed/retained launch authority; `.170`/BigBoy exact:
  `evidence/WL-FUNC-019-2026-08-11-retained-source-stage-generation-r419.md`, `evidence/WL-FUNC-019-2026-08-11-resource-snapshot-revocation-r454.md`.
- **Android readiness:** Start remains unavailable until live guest readiness; `.90` passed: `evidence/WL-FUNC-019-2026-08-11-android-readiness-action-r220.md`.
- **Service action schema admission (2026-08-11):** future schemas and unsafe
  correlation IDs fail before capability targeting; `.50` passed:
  `docs/platform/evidence/WL-FUNC-019-2026-08-11-service-action-schema-r220.md`.
- **Service-action admission (2026-08-10):** invalid authentication, target, issuance, expiry, or ambiguity fails closed; `.50` passed:
  `docs/platform/evidence/WL-FUNC-019-2026-08-10-service-action-admission-r159.md`.
- **Failed-probe launch admission checkpoint (2026-08-10):** an enabled service
  whose latest endpoint test failed remains unavailable and cannot expose a typed
  `launch` action; seat 90 passed the focused regression:
  `docs/platform/evidence/WL-FUNC-019-2026-08-10-failed-service-probe-launch-r164.md`.
- **Peer-record nofollow:** `.90` passed final-symlink refusal at the record-open boundary:
  `docs/platform/evidence/WL-FUNC-019-2026-08-10-peer-record-nofollow-r180.md`.
- **Manual-store root nofollow:** `.50` passed refusal of a symlinked manual-source store directory:
  `docs/platform/evidence/WL-FUNC-019-2026-08-10-manual-store-root-nofollow-r185.md`.
- **Persisted manual-source admission:** `.90` passed refusal of invalid
  host/name records restored from the JSON store, so persisted data cannot
  bypass request validation:
  `docs/platform/evidence/WL-FUNC-019-2026-08-10-manual-store-admission-r193e.md`.
- **Manual-source metadata replacement (2026-08-11):** authenticated updates to
  one stable endpoint now atomically replace durable/published metadata without
  creating a duplicate; BigBoy passed 1/1:
  `docs/platform/evidence/WL-FUNC-019-2026-08-11-manual-source-metadata-replacement-r238.md`.
- **Service route-isolation checkpoint (2026-08-10):** a ready Service/Launch action cannot cross-route into Workloads authority; `.90` passed the exact fixture:
  `docs/platform/evidence/WL-FUNC-019-2026-08-10-service-route-isolation-r142.md`.
- **Ambiguous peer identity isolation (2026-08-10):** divergent hostname claims
  cannot authorize downstream resource reads; `.90` passed:
  `docs/platform/evidence/WL-FUNC-019-2026-08-10-peer-identity-isolation-r207.md`.
- **Seat-15 RDP resource/provenance checkpoint (2026-08-10):** Release 32 preserves SSH/22 and RDP/3389 independently, publishes the available approval-gated Desktop card
  plus matching discovery revision, and now retains provenance for the full bounded probe lease; `.90` passed the exact lifetime regression. Authenticated login/render
  remains: `docs/platform/evidence/WL-FUNC-019-2026-08-10-seat15-rdp-resource-provenance-r129.md`.
- **Desktop clipboard capability checkpoint (2026-08-10):** the universal
  desktop adapter now advertises the bounded text clipboard channel already
  implemented by RDP, VNC, and SPICE instead of hiding it behind a
  display/input-only capability. Seat 15 currently detects its available
  approval-gated Windows RDP card; authenticated login/render remains:
  `docs/platform/evidence/WL-FUNC-019-2026-08-10-desktop-clipboard-capability-r139.md`.
- **RDP host-discovery checkpoint (2026-08-10):** bounded fast and deep probes preserve the default privileged discovery set and add TCP/3389 without `-Pn` or
  target/port expansion; machine 9 passed the exact regression. Seat-15 deployment and live authenticated-card proof remain:
  `docs/platform/evidence/WL-FUNC-019-2026-08-10-rdp-host-discovery-r117.md`.
- **Peer-App target-binding checkpoint (2026-08-10):** legacy rows with an explicit cross-peer node are discarded before becoming resource/action targets; machine 193
  passed the exact regression: `docs/platform/evidence/WL-FUNC-019-2026-08-10-peer-app-target-binding-r22.md`.
- **Resource credential retry checkpoint (2026-08-10):** transient SecretStore startup failures retry with a bounded systemd rate while absent/invalid credentials stay
  terminal; machine 193 passed focused hostile cases: `docs/platform/evidence/WL-CRIT-007-WL-FUNC-019-2026-08-10-resource-credential-retry-r110.md`.
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
- **Stale desktop-roster checkpoint (2026-08-10):** future-dated and older-than-five-minute
  desktop source rosters are withheld before they can revive an approval-gated RDP card;
  `.90` passed the exact regression. Authenticated login/render proof remains:
  `docs/platform/evidence/WL-FUNC-019-2026-08-10-stale-desktop-roster-r151.md`.
- **Zero-observation checkpoint (2026-08-10):** desktop/RDP projection refuses
  sources with no observation timestamp; `.50` passed:
  `docs/platform/evidence/WL-FUNC-019-2026-08-10-zero-observation-r154.md`.
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
- **Media stable-ID equivocation:** conflicting raw rows are suppressed before redaction/deduplication while unrelated cards survive; exact gate deferred:
  `docs/platform/evidence/WL-FUNC-019-2026-08-11-media-stable-id-equivocation-r272.md`.
- **Live Windows authority checkpoints (2026-08-09):** seat 15 detects RDP; signed Open/revocation passed on `.196`, and the formerly absent shared publisher key is sealed.
  Installed credential activation/live login remain: `evidence/WL-FUNC-019-2026-08-09-rdp-authority-handoff-r8.md`,
  `evidence/WL-FUNC-019-2026-08-09-resource-publisher-key-r9.md`.
- **RDP scan freshness checkpoint (2026-08-09):** live seat 15 proof found
  `172.20.146.54:3389` disappearing because a four-minute scan consumed its
  five-minute lease before publication. Snapshots now stamp completion and
  slow cycles skip the extra cadence delay. Focused machine-194 tests pass;
  Release 23 still lacks the typed Desktop/RDP projection and needs corrected
  deployment:
  `docs/platform/evidence/WL-FUNC-019-2026-08-09-rdp-scan-completion-freshness-r18.md`.
- **Quiet-Windows RDP checkpoint (2026-08-09):** the bounded local `/24` scan now admits ping-silent hosts only after TCP 3389 succeeds, then independently fingerprints RDP.
  BigBoy passed three exact gates: `docs/platform/evidence/WL-FUNC-019-WL-UX-013-2026-08-09-rdp-lan-detection-r74.md`.
- **Seat 15 RDP catalog-TTL closure (2026-08-09):** release 27 kept the typed card available at 149.777 seconds, then renewed it across two scans; Dell was also upgraded.
  `docs/platform/evidence/WL-FUNC-019-WL-UX-013-WL-ARCH-009-2026-08-09-release27-rdp-continuity-r100.md`.
- **Seat 15 Release 24 checkpoint (2026-08-09):** the clean Fedora 44 artifact
  passed real-RPM gates and a dry-run, then installed after the visible warning.
  All daemon groups and publisher credential activation passed; consecutive
  scans retained `172.20.146.54:3389` as an available typed Desktop/RDP card
  with an approval-gated connect action. Authenticated login/render remains:
  `docs/platform/evidence/WL-FUNC-019-2026-08-09-seat15-release24-r19.md`.
- **Transfer-backed Files checkpoint (2026-08-09):** stable registry snapshots and durable corrected-forward results bind transfer actions across Bus replacement.
  Machine 9 passed 12 exact gates: `docs/platform/evidence/WL-FUNC-016-WL-FUNC-019-WL-ARCH-009-2026-08-09-transfer-bus-transaction-recovery-r69.md`.
- **Manual-source transaction checkpoint (2026-08-09):** RDP/VNC additions and
  removals commit to the live roster only when the strict bounded store agrees,
  including post-rename sync errors. Machine 9 passed three exact failure and
  corrected-forward fixtures:
  `docs/platform/evidence/WL-FUNC-019-2026-08-09-manual-source-transaction-r20.md`.
- **Desktop Bus recovery checkpoint (2026-08-09):** late Bus storage no longer
  permanently removes desktop discovery; startup retries are bounded, prime
  cursors once, skip stale actions, and publish forward without restart.
  Machine 9 passed three exact tests:
  `docs/platform/evidence/WL-FUNC-019-2026-08-09-desktop-bus-recovery-r21.md`.
- **Seat remote-input recovery checkpoint (2026-08-09):** Bus open and both
  transient control tails now activate atomically after late storage; retained
  arm/input controls are skipped and one forward consented input runs exactly
  once. Machine 196 passed two exact tests:
  `docs/platform/evidence/WL-FUNC-019-WL-CRIT-007-2026-08-09-seat-remote-input-bus-recovery-r23.md`.
- **Mesh-mount Bus recovery checkpoint (2026-08-09):** mount lifecycle polling
  survives late storage, atomically skips retained host actions, executes first
  requests on new host topics, and defers convergence on read failure. Machine
  194 passed three exact tests:
  `docs/platform/evidence/WL-ARCH-010-WL-FUNC-019-2026-08-09-mesh-mount-bus-recovery-r29.md`.
- **Service Aggregator transaction checkpoint (2026-08-09):** desktop,
  SSH/X11, and UPnP inputs plus catalog/discovery/attestation derivation stage before publication; failures do not claim success. BigBoy passed seven exact tests:
  `docs/platform/evidence/WL-FUNC-019-WL-ARCH-009-2026-08-09-service-aggregator-bus-recovery-r45.md`.
- **Unit Aggregator transaction checkpoint (2026-08-09):** strict cloud reads
  and staged first-seen state precede mirror publication; failed replies retry
  without cursor advance. BigBoy passed 67 module tests and seven hostile cases:
  `docs/platform/evidence/WL-FUNC-019-WL-ARCH-009-2026-08-09-unit-aggregator-bus-recovery-r49.md`.
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
     - Objective: exercise peer loss/rejoin, stale catalogs, duplicate sources, action failure, reconnect, and acceptance on no more than three seats.
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
  3. At-most-three-seat/lighthouse loss, rejoin, and recovery produce no fabricated resource or side effect.
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
- **Catalog/provider (2026-08-08):** signed import and image/KVM/capacity/libvirt preflight passed:
  `docs/platform/evidence/WL-FUNC-020-2026-08-08-android-signed-catalog-s1-r1.md`, `docs/platform/evidence/WL-FUNC-020-2026-08-08-android-provider-preflight-s2-r1.md`.
- **S3 lifecycle/readiness (2026-08-09):** recovery, guest relay, and VDI revocation passed:
  `docs/platform/evidence/WL-FUNC-020-2026-08-08-android-lifecycle-s3-r1.md`, `docs/platform/evidence/WL-FUNC-020-2026-08-09-vdi-readiness-revocation-r4.md`.
- **S4 governed UX (2026-08-08):** signed cards, lifecycle, rendering, handoff, and no-dial refusal passed:
  `docs/platform/evidence/WL-FUNC-020-2026-08-08-governed-android-ux-s4-r1.md`.
- **Release admission (2026-08-09):** schema-v2 readiness binding passed: `docs/platform/evidence/WL-FUNC-020-2026-08-09-release-artifact-admission-s2-s5-r5.md`.
- **Future-issued catalog (2026-08-10):** provider preflight refuses catalogs issued after the admission clock; `.90` passed:
  `docs/platform/evidence/WL-FUNC-020-2026-08-10-future-issued-catalog-r153.md`.
- Remaining work:
- **Typed Android Workload start (2026-08-11):** governed outer-VM `Start`
  validates the declaration and publishes a signed, generation-bound,
  replay-stable operation; clean BigBoy slot 2 passed 1/1:
  `docs/platform/evidence/WL-FUNC-020-2026-08-11-typed-workload-start-r256.md`.
- **Corrupt catalog restart (2026-08-11):** invalid durable state cannot become empty authority or switch identity; BigBoy passed 1/1:
  `docs/platform/evidence/WL-FUNC-020-2026-08-11-corrupt-catalog-restart-r293.md`.
- **Signed Android desired definition (2026-08-11):** provision re-verifies the durable catalog, exact artifact/package provenance, capacity, and provider before
  persistence; BigBoy passed 1/1:
  `docs/platform/evidence/WL-FUNC-020-2026-08-11-signed-desired-definition-r263.md`.
- **Typed lifecycle delegation:** signed-catalog-bound Start/Stop use Workloads only; Cancel stays refused without a prior request ID. BigBoy passed 2/2:
  `docs/platform/evidence/WL-FUNC-020-2026-08-11-android-lifecycle-delegation-r273.md`.
- **Bounded Android host probes (2026-08-11):** `/proc` and nested-KVM sysfs reads reject oversized host text before parsing; BigBoy passed 1/1:
  `evidence/WL-FUNC-020-2026-08-11-android-host-probe-bound-r227.md`.
- **Bounded cloud replay cleanup (2026-08-11):** expired nonce rows reject symlinks and payloads over 128 bytes before parsing; BigBoy passed 1/1:
  `evidence/WL-FUNC-020-2026-08-11-cloud-gate-nonce-bound-r227.md`.
- **Authenticated Cuttlefish relay (2026-08-11):** the production guest
  transport rejects writable, substituted, ownership-drifted, or peer-credential-
  mismatched Unix relays before sending governed request bytes; BigBoy passed 1/1:
  `docs/platform/evidence/WL-FUNC-020-2026-08-11-authenticated-cuttlefish-relay-r248.md`.
- **VDI source identity checkpoint (2026-08-10):** Cuttlefish VDI sources now require current guest-ready state plus
  matching workload, image provenance, and generation. `.90` passed a hostile mismatched-workload regression:
  `docs/platform/evidence/WL-FUNC-020-2026-08-10-vdi-source-identity-r187.md`.
- **Android catalog identity checkpoint (2026-08-10):** higher-revision signed imports cannot switch catalog identity; seat 90 passed:
  `docs/platform/evidence/WL-FUNC-020-2026-08-10-android-identity-continuity-r161.md`.
- **Android catalog state-parent checkpoint (2026-08-10):** cache replay and replacement refuse symlinked or non-directory parent components; seat 90 passed the hostile regression:
  `docs/platform/evidence/WL-FUNC-020-2026-08-10-catalog-state-parent-nofollow-r196.md`.
- **Stale Android generation admission (2026-08-10):** non-ready or stale
  Cuttlefish operations stop before backend contact; `.90` passed:
  `docs/platform/evidence/WL-FUNC-020-2026-08-10-stale-generation-admission-r210.md`.
- **Cuttlefish readiness revocation (2026-08-11):** failed refresh revokes retained launch/VDI authority before backend contact; `.50` passed 1/1:
  `docs/platform/evidence/WL-FUNC-020-2026-08-11-cuttlefish-readiness-revocation-r315.md`.
- **Future guest inventory:** future observations cannot fabricate fresh readiness; BigBoy 1/1: `evidence/WL-FUNC-020-2026-08-11-future-guest-inventory-r382.md`.
- **Guest exchange generation:** pre-restart inventory cannot authorize current readiness; `.196` 1/1: `evidence/WL-FUNC-020-2026-08-11-cuttlefish-exchange-generation-r436.md`.
- **Guest readiness publication:** parent/staging substitution cannot redirect the receipt; `.196` self-test:
  `evidence/WL-FUNC-020-2026-08-11-guest-readiness-publication-r445.md`.
- **VDI host canonicalization:** hostile aliases cannot cross mesh-host authority; BigBoy 1/1:
  `evidence/WL-FUNC-020-2026-08-11-vdi-host-canonicalization-r448.md`.
- **Expired catalog replay:** replacement Bus activation retains only anti-rollback identity; BigBoy 1/1: `evidence/WL-FUNC-020-2026-08-11-expired-catalog-bus-replacement-r384.md`.
- **Catalog Bus generation:** replay/import progress cannot cross a replaced index; `.170` 1/1: `evidence/WL-FUNC-020-2026-08-11-catalog-bus-generation-r401.md`.
- **Retry generation:** terminal retry cannot relabel running power; `.196` 1/1: `evidence/WL-FUNC-020-WL-ARCH-010-2026-08-11-cuttlefish-failed-retry-generation-r407.md`.
  - **Outer-VM runtime authority (2026-08-09):** Cuttlefish consumes one validated Workloads row; unavailable authority and same-ID containers fail closed, and direct
    libvirt roster is deleted. Machine 9 passed 13/13: `docs/platform/evidence/WL-ARCH-010-WL-FUNC-020-2026-08-09-cuttlefish-workload-authority-r101.md`.
  - **Signed release-artifact admission (2026-08-09):** schema v3 requires one bounded detached signature from the pinned installed MCNF key before provisioning; missing,
    invalid, substituted, or changed artifacts fail closed. BigBoy passed the real GPG/dearmor package gate:
    `docs/platform/evidence/WL-FUNC-020-2026-08-09-signed-release-artifact-admission-r102.md`.
  - **S1 importer retry boundary (2026-08-09):** transient persistence/publication failure no longer acknowledges a signed catalog row; terminal refusals still
    advance and the repaired retry publishes exactly once. Machine 9 exact regression passed 1/1:
    `docs/platform/evidence/WL-FUNC-020-2026-08-09-android-import-side-effect-retry-s1-r6.md`.
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
     - Objective: verify image provenance, SELinux/cgroup/device isolation, audio/input, reconnect, upgrade, and acceptance on no more than three seats.
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
- Required outcome: daemon-owned typed music catalog/queue/playback/cache; real mpv audio/video; local/Jellyfin, discovery, cast, handoff, and live proof.
- Current state: release 11/daemon authority run on five seats; Dell/CPU/NWS/provider-loss pass; Bus fold:
  `evidence/WL-FUNC-021-WL-ARCH-009-2026-08-09-media-server-bus-transaction-recovery-r82.md`; renderer/audio/cast/handoff remain.
- **Projection validation:** bad snapshots retain last-good; zero is refused; UI 4/4 `.50`, daemon 1/1 `.90`: `evidence/WL-FUNC-021-2026-08-06-projection-validation-r2.md`.
- **Media hardening (2026-08-06):** media-core 250/250 on BigBoy; four bounded Music proof-helper self-tests pass.
  Live renderer/provider acceptance is owned by `WL-TEST-001`; no second-seat proof is required. Evidence: `docs/platform/evidence/WL-FUNC-021-2026-08-06-media-hardening-r2.md`.
- **Provider consistency (2026-08-09):** restart selection and stale fallback invalidation passed `.90`; evidence: `evidence/WL-FUNC-021-2026-08-09-provider-restart-binding-r4.md`.
- **Music Bus replacement (2026-08-10):** `.90` passed: `docs/platform/evidence/WL-FUNC-021-2026-08-10-music-bus-reopen-r158.md`.
- **Bounded media config (2026-08-11):** shared-folder JSON caps at 64 KiB and rejects symlinks; BigBoy: `evidence/WL-FUNC-021-2026-08-11-media-config-bound-r226.md`.
- **Navidrome command timeout (2026-08-11):** systemctl/setup calls fail closed at the shared deadline; BigBoy: `evidence/WL-FUNC-021-2026-08-11-navidrome-command-timeout-r226.md`.
- **Bounded service registration hostname (2026-08-11):** `/etc/hostname` caps at 255 bytes; BigBoy passed 1/1: `evidence/WL-FUNC-021-2026-08-11-service-hostname-bound-r230.md`.
- **Bounded Navidrome commands (2026-08-11):** systemctl uses shared 15s boundary; BigBoy passed 3/3: `evidence/WL-FUNC-021-2026-08-11-navidrome-command-bound-r231.md`.
- **Navidrome setup bound (2026-08-11):** re-provision shares 15s timeout; `.90` 3/3: `evidence/WL-FUNC-021-2026-08-11-navidrome-setup-timeout-r232.md`.
- Remaining work: **Artwork byte bound (2026-08-11):** non-regular/over-4M reads and oversized writes refuse; `.50`: `evidence/WL-FUNC-021-2026-08-11-artwork-byte-bound-r222.md`.
- **Revoked renderer generation:** device loss blocks in-flight audio/queue commit; `.170` 1/1: `evidence/WL-FUNC-021-2026-08-11-revoked-renderer-generation-r429.md`.
- **Transcode generation binding:** source/session substitution fails closed; `.196` 1/1: `evidence/WL-FUNC-021-2026-08-11-transcode-generation-binding-r432.md`.
- **Queue persistence rollback:** failed durable writes roll memory back and report failure; BigBoy 1/1: `evidence/WL-FUNC-021-2026-08-11-queue-persistence-rollback-r410.md`.
- **Media-source heartbeat:** impossible retained observations cannot restore reachability; `.50` 1/1: `evidence/WL-FUNC-021-2026-08-11-media-source-heartbeat-r313.md`.
- **Provider identity:** endpoint equivocation revokes fallback; `.50` 1/1: `evidence/WL-FUNC-021-2026-08-11-provider-identity-equivocation-r314.md`.
- **Manifest/Jellyfin identities:** forged/duplicate manifest items and server IDs fail closed; `.196`/`.50` 1/1 each:
  `evidence/WL-FUNC-021-2026-08-11-media-manifest-item-identity-r317.md`, `evidence/WL-FUNC-021-2026-08-11-jellyfin-server-identity-r316.md`.
- **Jellyfin cache/sync:** exact digests and URL segments reject content/path substitution; `.50`/`.196` 1/1 each:
  `evidence/WL-FUNC-021-2026-08-11-jellyfin-cache-digest-r319.md`, `evidence/WL-FUNC-021-2026-08-11-jellyfin-sync-path-identity-r320.md`.
- **Jellyfin metadata generation:** stale/equivocal replay cannot roll back cache; BigBoy 1/1: `evidence/WL-FUNC-021-2026-08-11-jellyfin-metadata-generation-r411.md`.
- **Playlist persistence:** symlink-safe atomic saves/loads; `.50` 1/1: `evidence/WL-FUNC-021-2026-08-11-playlist-symlink-atomicity-r318.md`.
- **Finite resume:** nonfinite samples preserve valid durable state; `.50` 1/1: `evidence/WL-FUNC-021-2026-08-11-finite-resume-state-r321.md`.
- **Roaming lease:** playback arms only after exact durable ownership; `.50` 1/1: `evidence/WL-FUNC-021-2026-08-11-roaming-lease-publication-r322.md`.
- **Party election:** same-sequence deterministic winners replace losing authority; `.196` 1/1: `evidence/WL-FUNC-021-2026-08-11-party-election-key-r324.md`.
- **Jellyfin transcode authority:** hostile redirects cannot escape server/item identity; `.196` 1/1: `evidence/WL-FUNC-021-2026-08-11-jellyfin-transcode-authority-r323.md`.
- **Proxy commitment/route authority:** post-commit failure and in-flight Bus replacement fail closed; `.196` 1/1 each:
  `evidence/WL-FUNC-021-2026-08-11-airsonic-response-commit-r325.md`, `evidence/WL-FUNC-021-2026-08-11-jellyfin-inflight-route-r326.md`.
- **Music cache/retry:** exact path identity and saturating backoff survive hostile inputs; `.196` 1/1 each:
  `evidence/WL-FUNC-021-2026-08-11-music-cache-path-identity-r327.md`, `evidence/WL-FUNC-021-2026-08-11-reconnect-backoff-overflow-r328.md`.
- **Cast/stream identities:** renderer equivocation and ambiguous network authorities fail closed; `.196` 1/1 each:
  `evidence/WL-FUNC-021-2026-08-11-cast-discovery-equivocation-r329.md`, `evidence/WL-FUNC-021-2026-08-11-stream-authority-admission-r330.md`.
- **Jellyfin request paths:** remote IDs cannot escape endpoint segments; `.196` 1/1: `evidence/WL-FUNC-021-2026-08-11-jellyfin-client-path-identity-r331.md`.
- **Player/control generations:** replacement signals and malformed numeric controls fail closed; BigBoy 1/1 each:
  `evidence/WL-FUNC-021-2026-08-11-player-replacement-generation-r332.md`, `evidence/WL-FUNC-021-2026-08-11-finite-media-controls-r333.md`.
- **Library/browse identities:** playback paths and series trees reject substituted durable/provider state; BigBoy/`.196` 1/1 each:
  `evidence/WL-FUNC-021-2026-08-11-library-playback-path-identity-r334.md`, `evidence/WL-FUNC-021-2026-08-11-jellyfin-browse-series-identity-r335.md`.
- **Frame layout authority:** malformed RGBA geometry/length cannot prove playback; `.196` 1/1: `evidence/WL-FUNC-021-2026-08-11-frame-layout-authority-r336.md`.
- **Production Navidrome:** failed store repair stays withdrawn; BigBoy 1/1: `evidence/WL-FUNC-021-2026-08-11-production-navidrome-withdrawal-r337.md`.
- **Subtitle/redirect authority:** ambiguous subtitle sources and Jellyfin cross-authority redirects fail closed; `.170` 1/1 each:
  `evidence/WL-FUNC-021-2026-08-11-subtitle-source-admission-r338.md`, `evidence/WL-FUNC-021-2026-08-11-jellyfin-redirect-authority-r339.md`.
- **Transport redirect authority:** caller policy cannot forward credentials to a provider-selected host; `.90` 1/1:
  `evidence/WL-FUNC-021-2026-08-11-jellyfin-transport-redirect-r442.md`.
- **Lockscreen media identity:** replacement playback cannot inherit retained controls; BigBoy exact:
  `evidence/WL-FUNC-021-2026-08-11-lockscreen-media-identity-r455.md`.
- **Video/audio generations:** replacement frame adjustments and sink choice explicitly reset; `.170`/`.196` 1/1 each:
  `evidence/WL-FUNC-021-2026-08-11-video-adjustment-revocation-r340.md`, `evidence/WL-FUNC-021-2026-08-11-audio-sink-revocation-r342.md`.
- **Provider item equivocation:** OpenSubtitles and Jellyfin conflicting identities fail closed; `.170` 1/1 each:
  `evidence/WL-FUNC-021-2026-08-11-opensubtitles-equivocation-r341.md`, `evidence/WL-FUNC-021-2026-08-11-jellyfin-item-admission-r343.md`.
- **Capture path authority:** device aliases/traversal fail closed; `.170` 1/1: `evidence/WL-FUNC-021-2026-08-11-capture-device-path-authority-r344.md`.
- **Codec/yt-dlp authority:** baseline capabilities and extracted URLs fail closed without optional/runtime authority; `.170` 1/1 each:
  `evidence/WL-FUNC-021-2026-08-11-universal-codec-capability-r345.md`, `evidence/WL-FUNC-021-2026-08-11-ytdlp-authority-boundary-r346.md`.
- **Smoke proof:** success requires Playing, audio, and nonblank frame; `.170` 1/1: `evidence/WL-FUNC-021-2026-08-11-media-smoke-proof-integrity-r347.md`.
- **Media roster authority:** publication watermark and malformed-state revocation prevent stale gateways; `.196`/`.170` 1/1 each:
  `evidence/WL-FUNC-021-2026-08-11-media-roster-watermark-r348.md`, `evidence/WL-FUNC-021-2026-08-11-media-roster-revocation-r349.md`.
- **Media menu identity:** stale track actions cannot target replacement media; `.170` 1/1: `evidence/WL-FUNC-021-2026-08-11-media-menu-track-identity-r350.md`.
- **Music server generation:** old-server playback stops before replacement authority; `.90` 1/1: `evidence/WL-FUNC-021-2026-08-11-music-server-generation-revocation-r351.md`.
- **Music source equivocation:** provider order cannot invent reachability/capabilities; `.90` 1/1: `evidence/WL-FUNC-021-2026-08-11-music-source-equivocation-r352.md`.
- **Music catalog/radio identity:** conflicting current rows and withdrawn details cannot publish stale UI intent; `.170` 1/1 each:
  `evidence/WL-FUNC-021-2026-08-11-music-current-catalog-equivocation-r354.md`, `evidence/WL-FUNC-021-2026-08-11-music-radio-detail-revalidation-r353.md`.
- **Airsonic/queue identity:** substituted songs and conflicting queue IDs fail closed; `.90`/`.170` 1/1 each:
  `evidence/WL-FUNC-021-2026-08-11-airsonic-track-identity-r355.md`, `evidence/WL-FUNC-021-2026-08-11-queue-entry-identity-r356.md`.
- **Queue/mpv generations:** framed CAS and ordered current-load events reject substituted durable/frame state; `.90` 1/1 each:
  `evidence/WL-FUNC-021-2026-08-11-queue-cas-framing-r357.md`, `evidence/WL-FUNC-021-2026-08-11-mpv-frame-generation-r358.md`.
- **Credential/state files:** no-follow bounded regular-file reads reject substitution; `.90` 1/1 each:
  `evidence/WL-FUNC-021-2026-08-11-credential-file-identity-r359.md`, `evidence/WL-FUNC-021-2026-08-11-state-file-nofollow-r362.md`.
- **Daemon/seat identity:** kernel singleton and PipeWire serial/process binding revoke duplicate/recycled owners; `.90` 1/1 each:
  `evidence/WL-FUNC-021-2026-08-11-daemon-kernel-singleton-r360.md`, `evidence/WL-FUNC-021-2026-08-11-seat-audio-object-identity-r361.md`.
- **MPRIS generation:** seek binds the audible track, not an advanced queue cursor; `.170` 1/1: `evidence/WL-FUNC-021-2026-08-11-mpris-audible-generation-r363.md`.
- **Workspace request binding:** durable IDs bind exact digests; `.50` 1/1: `evidence/WL-FUNC-021-2026-08-11-workspace-request-binding-r244.md`.
- **Action-ledger privacy:** restart enforces the six-hour epoch and rejects future rows; `.50` 1/1: `evidence/WL-FUNC-021-2026-08-11-action-ledger-privacy-r309.md`.
- **Live-ledger saturation:** full live epochs reject new mutations without eviction; `.90` 1/1: `evidence/WL-FUNC-021-2026-08-11-live-ledger-saturation-r364.md`.
- **Audible fallback/loss:** inaudible candidates cannot suppress fallback; audible loss drains its valid tail and preserves handoff; focused farms 1/1 each:
  `evidence/WL-FUNC-021-2026-08-11-audible-fallback-authority-r366.md`, `evidence/WL-FUNC-021-2026-08-11-audible-provider-loss-tail-r385.md`.
- **Bounded bookmark link probe (2026-08-11):** HTTP curl hangs fail closed; `.50` passed 1/1: `evidence/WL-FUNC-021-WL-ARCH-009-2026-08-11-bookmark-probe-timeout-r235.md`.
- **PipeWire dump bound (2026-08-11):** `pw-dump` output capped at 16 MiB before JSON parsing; `.50` passed: `evidence/WL-FUNC-021-2026-08-11-pw-dump-bound-r223.md`.
- **Cast URL admission:** unsafe/local/credential-bearing URLs refused; BigBoy: `evidence/WL-FUNC-021-2026-08-10-cast-media-url-admission-r184.md`.
- **Direct URL admission:** malformed/credential-bearing/unsafe URLs refused; `.90`: `evidence/WL-FUNC-021-2026-08-10-direct-media-url-admission-r214.md`.
- **Named-detail/activation/NWS release-11 checkpoint (2026-08-08):** identity-bound details, one daemon/shell per seat, Dell records, and five-seat recovery pass:
  `docs/platform/evidence/WL-FUNC-021-2026-08-08-seat-activation-release10-r1.md`; `docs/platform/evidence/WL-FUNC-021-2026-08-08-nws-recovery-release11-r1.md`.
- **Signed live-radio:** release 8 Dell/15 C-SPAN capture passed; remaining seats/judgment open: `evidence/WL-FUNC-021-2026-08-08-live-radio-release8-r1.md`.
- **Library checkpoint (2026-08-06):** typed collections replace Airsonic rows; UI 44/44 on `.50`, fmt `.90`; `evidence/WL-FUNC-021-2026-08-06-daemon-library-r1.md`.
- **Search checkpoint (2026-08-06):** retained typed search renders; provider search is fallback; UI 45/45 `.50`; `evidence/WL-FUNC-021-2026-08-06-daemon-search-r1.md`.
- **Drain guards (2026-08-06):** search replay and duplicate Jellyfin identities pass `.90`; live-seat RPM ownership self-test and read-only probe pass.
- **Cache checkpoints:** truncation refused; replacement keeps last-good; live/package proof open; evidence: `evidence/WL-FUNC-021-2026-08-09-cache-index-atomic-r8.md`.
- **Music cache completeness (2026-08-10):** `.90` passed truncated/replaced-file refusal: `docs/platform/evidence/WL-FUNC-021-2026-08-10-cache-completeness-r208.md`.
- **mpv/recovery checkpoints:** retry/resume passed 239/239; real nonblank playback plus playlist/replacement continuation passed 3/3 on BigBoy.
  Live proof remains: `evidence/WL-FUNC-021-2026-08-06-media-recovery-r1.md`, `evidence/WL-FUNC-021-2026-08-09-mpv-playlist-continuation-r11.md`.
- **Daemon Album/download/workerless:** retained albums emit typed play; bounded actions pass daemon 168/168 and UI 47/47; construction starts no worker.
  `docs/platform/evidence/WL-FUNC-021-2026-08-06-managed-download-r1.md`, `docs/platform/evidence/WL-FUNC-021-2026-08-06-embedded-workerless-r1.md`.
- **Typed target handoff checkpoint (2026-08-06):** fresh idle peers publish typed `transfer`; stale/owning peers remain browse-only. `.50` passed 48/48; live proof remains:
  `docs/platform/evidence/WL-FUNC-021-2026-08-06-target-handoff-r1.md`, `docs/platform/evidence/WL-FUNC-021-2026-08-06-peer-targets-r1.md`.
- **Handoff routing:** owner/bystander pumps preserve another seat's completion; BigBoy 12/12; evidence: `evidence/WL-FUNC-021-08-09-handoff-target-routing-r95.md`.
- **Cast:** bounds passed; live open: `evidence/WL-FUNC-021-2026-08-06-cast-bounds-r1.md`, `evidence/WL-FUNC-021-2026-08-09-chromecast-async-discovery-r12.md`.
- **Live provider loss:** seat 15 recovered with zero restarts; audible continuity remains: `evidence/WL-FUNC-021-2026-08-08-live-provider-loss-release11-r1.md`.
- **Provider-loss reconnect:** bounded `timeOffset` resume clears buffered-ahead samples, preserves cache, and refuses arbitrary URLs; focused gates pass.
  Seat 15 recovers the provider while daemon/cached projections remain available; audible in-progress continuity remains open.
  `docs/platform/evidence/WL-FUNC-021-2026-08-06-network-loss-reconnect-r1.md`, `docs/platform/evidence/WL-FUNC-021-2026-08-06-reconnect-timeout-r1.md`.
- **Zero-audio failover:** empty streams cannot suppress fallback; `.196` 1/1: `evidence/WL-FUNC-021-2026-08-11-zero-audio-provider-failover-r289.md`.
- **Cast loopback:** bounded discovery/control/seek passes; live proof open: `evidence/WL-FUNC-021-2026-08-06-cast-loopback-r1.md`.
- **Two-seat handoff:** exact-once transfer, mismatch/stale refusal, and atomic records pass `.50`/`.90`/`.170`; live boundary:
  `evidence/WL-FUNC-021-2026-08-08-two-seat-owner-handoff-r1.md`, `evidence/WL-FUNC-021-2026-08-09-handoff-atomic-r9.md`.
- **Cast runtime audit:** no physical renderer, usable Chromecast path, receiver unit, or second admitted peer was found; typed paths remain fixture-proven.
  Physical renderer, Chromecast, and mesh-owner receiver implementation remain open.
  Any resulting installed-seat or continuity capture is coordinated by `WL-TEST-001`, with no two-seat proof requirement for this epic.
  `docs/platform/evidence/WL-FUNC-021-2026-08-06-cast-runtime-audit-r1.md`.
- **Cast-admission checkpoint (2026-08-06):** URLs, titles, and HTTP endpoints reject oversized/control-bearing input before the network gate; BigBoy tests
  passed 20/20. Live renderer, Chromecast, and mesh-owner receiver implementation remain open; installed-seat capture is owned by `WL-TEST-001`.
  Evidence: `docs/platform/evidence/WL-FUNC-021-2026-08-06-cast-admission-r1.md`.
- **Two-catalog outage checkpoint (2026-08-06):** source projection retains two admitted variants under one logical queue track.
  Failed-first/healthy-second decoding and BigBoy gates pass; live outage, mid-track resume, and hardware/package proof remain open.
  `docs/platform/evidence/WL-FUNC-021-2026-08-06-two-catalog-outage-r1.md`.
- **Jellyfin outage:** known-good cache survives failures; truncated manifests refused; live proof remains: `evidence/WL-FUNC-021-2026-08-06-jellyfin-outage-r1.md`.
- **GUI authority:** both Music surfaces consume daemon projections, start no provider/playback worker, and require an authenticated writer; `.50` passed:
  `docs/platform/evidence/WL-FUNC-021-2026-08-08-standalone-daemon-authority-r1.md`.
- **Renderer recovery:** failure revokes playback/MPRIS; reacquisition resumes the exact finite track unless cancelled; live audible/two-seat proof remains:
  `docs/platform/evidence/WL-FUNC-021-2026-08-08-renderer-recovery-r1.md`.
- **Real-mpv UI (2026-08-07):** frames clear; 110/110 UI and 257 tests passed; physical proof remains: `docs/platform/evidence/WL-FUNC-021-2026-08-07-media-render-clear-r1.md`.
- **Continuation:** daemon 182/182, roaming 18/18, reconnect 8/8, router 26/26, and Dell CPU passed:
  `evidence/WL-FUNC-021-2026-08-06-roaming-root-loss-r1.md`, `evidence/WL-FUNC-021-2026-08-06-reconnect-loop-audit-r1.md`.
- **Live boundary:** same-provider resume and package/gateway gates pass; no physical cast target was found and Dell later became unreachable.
  Live loss, renderer, handoff, auth/rotation, physical cast/two-seat, and three-seat CPU/NWS remain open.
  `evidence/WL-FUNC-021-2026-08-07-provider-loss-audit-r1.md`, `evidence/WL-FUNC-021-2026-08-07-cast-runtime-audit-r2.md`.
- **Seat-15 CPU:** bounded samples found Syncthing convergence; load stayed below capacity and no daemon was pegged; steady-state retest remains:
  `evidence/WL-FUNC-021-2026-08-10-seat15-cpu-attribution-r147.md`, `evidence/WL-FUNC-021-2026-08-10-seat15-cpu-retest-r157.md`.
- **Idle-state coalescing:** transition/heartbeat-preserving suppression passes BigBoy: `evidence/WL-FUNC-021-2026-08-10-idle-state-coalescing-r155.md`.
- **State revision replay:** rollback/equivocation fails closed; BigBoy passed 1/1: `docs/platform/evidence/WL-FUNC-021-2026-08-11-state-revision-replay-r269.md`.
- **Live provider audio:** Airsonic track `23427` completed with 6,287,357/6,717,440 monitored samples nonzero; temporary capture was removed.
  Provider-loss resume, speaker judgment, and authenticated mutation remain open.
  `docs/platform/evidence/WL-FUNC-021-2026-08-06-live-audio-capture-r1.md`.
- **Live Music DRM:** seat 15 produced a settled 1920x1080 direct-DRM EGL frame; the Music verifier accepted 15 separated foreground components.
  The temporary drop-in was removed, service returned active with zero restarts, and full acceptance/loss/handoff/package proof remains open.
  `docs/platform/evidence/WL-FUNC-021-2026-08-06-live-drm-frame-r1.md`;
  `install-helpers/verify-music-drm-proof.py`.
- **RPM/install:** F44 release 5 passed payload/size and Dell CPU proof; seat 15 remained release 4: `evidence/WL-FUNC-021-2026-08-06-dell-release5-cpu-r1.md`.
- **Artwork/pagination:** daemon/UI/shell gates pass; release 6 is live on Dell/seat 15 with distinct bounded pages and local JPEG art.
  Open: renderer, provider-loss, cast, handoff, radio playback, and three-seat CPU/NWS.
  Evidence: `docs/platform/evidence/WL-FUNC-021-2026-08-07-music-artwork-release6-r1.md`.
- **Mutation authorization delivery:** domain-separated Ed25519 capabilities bind digest/scope/expiry/replay; daemon receives only the public key.
  Shared types 431/431 and daemon 174/174 pass; live authorized mutation and installed-seat rotation remain open.
  `docs/platform/evidence/WL-FUNC-021-2026-08-06-auth-delivery-audit-r2.md`.
- **Mutation authorization package:** RPM assets/systemd/helper and dependency checks declare required `openssl`/`curl` in the fresh base header.
  Installed-seat provisioning, mutation, and rotation proof remain open.
  `docs/platform/evidence/WL-FUNC-021-2026-08-06-auth-package-audit-r2.md`;
  prior audit: `docs/platform/evidence/WL-FUNC-021-2026-08-06-auth-package-audit-r1.md`.
- **Reusable live-seat gate:** self-test and bounded seat-15 read-only checks pass; the 15-second song probe left no client process and claims no audible/rendered acceptance.
- **Queue durability:** atomic replacement preserves last-good and cleans failed staging; `.50` 14/14: `evidence/WL-FUNC-021-2026-08-09-queue-atomic-persistence-r1.md`.
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
  3. At-most-three-seat visual/audio/package evidence proves the shipped release or names blockers.
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
- Current state: Signed contracts, durable scheduling/convergence, governed audio, and Clock/bell chrome exist; multi-process/UI/package/live proof is post-release acceptance.
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
- **Clock-audio payload authority:** same-generation retries cannot substitute an active occurrence's source; BigBoy 1/1:
  `evidence/WL-FUNC-022-2026-08-11-clock-audio-payload-authority-r373.md`.
- **Ringing schedule authority:** restart cannot graft replacement payloads onto a ringing occurrence; `.90` 1/1:
  `evidence/WL-FUNC-022-2026-08-11-ringing-schedule-authority-r370.md`.
- **Timer/alarm action boundary:** timers cannot enter alarm-only ringing or Snooze paths; `.90` 1/1:
  `evidence/WL-FUNC-022-2026-08-11-timer-snooze-boundary-r371.md`.
- **Disabled-alarm snooze:** disabling cancels retained snooze generations; `.170` 1/1: `evidence/WL-FUNC-022-2026-08-11-disabled-alarm-snooze-r417.md`.
- **Clock action payload:** retained controls cannot authorize replaced daemon payloads; BigBoy exact:
  `evidence/WL-FUNC-022-2026-08-11-clock-action-payload-r453.md`.
- **Occurrence payload binding (2026-08-11):** active generations reject conflicting audio/volume; `.90` passed 1/1:
  `docs/platform/evidence/WL-FUNC-022-2026-08-11-clock-occurrence-payload-binding-r365.md`.
- **Peer stopwatch repair binding (2026-08-14):** lower-revision repairs now require the deterministic origin-generated request identity bound to target, stopwatch, and revision.
  BigBoy `172.20.0.130` passed the Clock worker suite 36/36:
  `evidence/WL-FUNC-022-2026-08-14-peer-stopwatch-repair-binding-r1.md`.
- **Peer convergence probe budget (2026-08-11):** retry-suppressed peer probes
  are capped independently at 512 per tick, preventing large retained snapshots
  from consuming unbounded convergence work; evidence:
  `docs/platform/evidence/WL-FUNC-022-2026-08-11-peer-probe-budget-r223.md`.
- **Clock local-target admission (2026-08-11):** locally authored schedules and stopwatch mirrors reject unapproved peers while approved peers persist; `.50` passed:
  `docs/platform/evidence/WL-FUNC-022-2026-08-11-clock-target-admission-r217.md`.
- **Replay cursor recovery checkpoint (2026-08-10):** duplicate Clock request
  replays cannot regress or clear the durable Bus action cursor; `.50` passed
  the hostile stale/`NULL` cursor regression:
  `docs/platform/evidence/WL-FUNC-022-2026-08-10-clock-replay-cursor-r187.md`.
- **Stopwatch elapsed-deadline checkpoint (2026-08-10):** overdue running stopwatches fail closed during admission and recovery; BigBoy passed:
  `docs/platform/evidence/WL-FUNC-022-2026-08-10-stopwatch-elapsed-deadline-r161.md`.
- **Peer stopwatch transport checkpoint (2026-08-10):** approved targeted mirrors preserve origin/revision and hostile variants fail closed; BigBoy passed 2/2:
  `docs/platform/evidence/WL-FUNC-022-2026-08-10-peer-stopwatch-transport-r146.md`.
- **Peer command budget checkpoint (2026-08-10):** Clock schedule, stopwatch, and
  acknowledgement convergence emits at most 128 peer commands per tick, leaving
  later work for the next bounded tick; BigBoy passed the exact regression:
  `docs/platform/evidence/WL-FUNC-022-2026-08-10-peer-command-budget-r152.md`.
- **Ringing schedule-removal checkpoint (2026-08-10):** removal now persists a terminal acknowledgement, queues Music Stop, and cancels pending snooze children before
  deleting the schedule; `.90` passed: `docs/platform/evidence/WL-FUNC-022-2026-08-10-ringing-schedule-removal-r128.md`.
- **Audio replay cursor monotonicity checkpoint (2026-08-10):** Clock action and
  Music audio-status replay boundaries advance only for newer Bus ULIDs, so
  stale or reordered status delivery cannot regress a consumed cursor; focused
  farm result is recorded in
  `docs/platform/evidence/WL-FUNC-022-2026-08-10-audio-replay-cursor-monotonic-r195.md`.
- **Exact peer schedule convergence (2026-08-10):** same-revision Clock
  schedules require exact payload equality; `.90` passed:
  `docs/platform/evidence/WL-FUNC-022-2026-08-10-peer-schedule-convergence-r198.md`.
- **Peer stopwatch conflict repair (2026-08-10):** newer conflicting payloads
  trigger repair; `.90` passed:
  `docs/platform/evidence/WL-FUNC-022-2026-08-10-peer-stopwatch-repair-r211.md`.
- **Single Clock sample (2026-08-11):** each tick reuses one wall-clock value
  across validation and publication; `.90` passed:
  `docs/platform/evidence/WL-FUNC-022-2026-08-11-single-clock-sample-r221.md`.
- **Ringing-audio restart recovery (2026-08-11):** restart reasserts ringing with a fresh TTL and deterministic effect ID; `.90` passed 1/1:
  `docs/platform/evidence/WL-FUNC-022-2026-08-11-ringing-audio-restart-r242.md`.
- **Command-generation-loss recovery (2026-08-11):** a command committed before
  Bus publication failure is recovered from durable authority even after the
  transient Bus generation loses the command; `.50` passed 1/1:
  `docs/platform/evidence/WL-FUNC-022-2026-08-11-command-generation-loss-r284.md`.
- **Deadline publication repair (2026-08-11):** a deadline commit followed by
  Bus publication failure reloads durable authority and repairs on the next
  sweep without duplicating its occurrence or audio effect; focused farm gate
  passed 1/1:
  `docs/platform/evidence/WL-FUNC-022-2026-08-11-deadline-publication-repair-r267.md`.
- **Clock Bus recovery (2026-08-09):** complete reads survive late/replaced storage; failed commits/publications/acks retry. Machine 194 passed four exact tests:
  `docs/platform/evidence/WL-FUNC-022-WL-ARCH-009-2026-08-09-clock-bus-replacement-r86.md`.
- **Clock documentation/package hard-cut checkpoint (2026-08-09):** the visible
  clock and dedicated bell have distinct canonical routes, daemon-only schedule
  authority is documented, and CI/package lint rejects the retired Timers
  surface, shell scheduler/store, stale route prose, and installed payload.
  Machine 9 passed the focused source/package lint fixtures:
  `docs/platform/evidence/WL-FUNC-022-2026-08-09-clock-doc-package-hardcut-s6-r15.md`.
- **Display-zone migration checkpoint (2026-08-09):** five legacy values migrate atomically to IANA; unknown values and legacy alarms remain untouched; machine 9 passed 5/5.
  Evidence: docs/platform/evidence/WL-FUNC-022-2026-08-09-display-zone-migration-s6-r14.md.
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
     - Done when: one evidence bundle binds revision, farm hosts/slots/results, direct-DRM captures, audio metrics, package identity, and three-seat-plus-lighthouse recovery.
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
  5. Fresh install, non-importing upgrade, package, direct-DRM/audio, and three-seat-plus-lighthouse recovery evidence prove behavior or name an exact blocker.
- Verification method: contracts on `.90`, worker/store and focused shell tests on `.50`, longest Music/shell/render/fault suites on BigBoy `.130`, then RPM and seat `.15`
  direct-DRM/physical-audio proof followed by acceptance on at most three selected seats and the separately governed lighthouses. Use explicit farm host/slot variables.
- Origin or merged source IDs: 2026-08-08 Clock Interface 50-question operator survey; AOSP DeskClock reference; existing shell Timers & Alarms implementation; UX-012
  clock/tray, FUNC-017 clock-weather, FUNC-021 Music/radio, Notification Center, and curtain workstreams.

### WL-CRIT-006 - Production evidence, single-node acceptance, and corrected-forward recovery
- Status: Remaining
- Priority: P0
- Complexity: Epic
- Problem: Static tests are strong but one signed release gate does not yet prove CI authority, farm topology, package integrity, three-seat behavior, lighthouses,
  recovery, and corrected-forward deployment together.
- Required outcome: GitHub required checks and farm evidence bind one revision;
  signed schema-5 evidence proves baseline acceptance on one selected physical
  test node/seat, package/runtime integrity, recovery, and corrected-forward
  promotion without rollback. The first full release is gated by build,
  package, signing, and artifact-integrity checks; live proofs and acceptance
  are post-release obligations. Additional physical nodes, seats, or
  lighthouses are optional follow-up evidence and are not release blockers.
- Current state: signing exists; live release proof remains. Evidence: `evidence/WL-CRIT-006-WL-ARCH-009-2026-08-11-worker-executable-generation-r467.md`.
- **Farm expansion (2026-08-08):** XEN-196 is a verified fifth build node; topology is 5/5 with 10 slots and `.196` passed `mde-bus` 425/425:
  `docs/platform/evidence/WL-CRIT-006-2026-08-08-farm-xen196-r1.md`.
- **Artifact claim checkpoint (2026-08-09):** one capture cannot satisfy independent node/scenario claims; `.90` passed 2 positive and 18 negative fixtures:
  `docs/platform/evidence/WL-CRIT-006-2026-08-09-six-node-artifact-claim-r2.md`.
- **Farm capacity checkpoint (2026-08-09):** sync refuses below the bounded remote `/home` reserve before creating a partial slot; machine 196 passed refusal/success:
  `docs/platform/evidence/WL-CRIT-006-2026-08-09-farm-sync-capacity-r3.md`.
- **Live collector binding checkpoint (2026-08-09):** rehashed arbitrary pass bytes and split role candidates now fail closed; BigBoy and `.90` passed verifier and release
  self-tests: `docs/platform/evidence/WL-CRIT-006-2026-08-09-live-collector-binding-r4.md`.
- **Governed candidate checkpoints (2026-08-09):** final-RPM digests and role compatibility are enforced; BigBoy built both RPMs and collector accepted `832726b0`.
  Bytes remain unsigned/undeployed: `evidence/WL-CRIT-006-2026-08-09-governed-candidate-path-r5.md`, `evidence/WL-CRIT-006-2026-08-09-current-candidate-r8.md`.
- Remaining work:
- Shared first-release signing, package, installed-seat, provider, and
  corrected-forward proof execution is owned by `WL-TEST-001`; this epic keeps
  only the release-gate contracts and verifier foundations it supplies.
- **Two-stage mandatory signing evidence (2026-08-11):** operator-only RPM
  preparation embeds signatures without publication output; final publication
  binds validated input/output inodes, atomically publishes no-replace files,
  requires matching evidence, and refuses unsigned/replaced RPMs or hostile
  output paths before success. The finalizer now consumes private stable-inode
  snapshots and rechecks original bytes and directory membership immediately
  before publication; signer and 15-fixture finalizer self-tests passed:
  `docs/platform/evidence/WL-CRIT-006-2026-08-11-mandatory-signing-evidence-r246.md`.
- **Signing rollback:** failed publication preserves substituted paths and restores every RPM in a failed batch; focused farm self-tests passed:
  `evidence/WL-CRIT-006-2026-08-11-signing-partial-rollback-r300.md`, `evidence/WL-CRIT-006-2026-08-11-multi-rpm-signing-rollback-r387.md`.
- **Exact production topology roster (2026-08-11):** schema-5 publication
  cross-binds verified topology identities to the gate manifest's selected
  baseline seat; additional seats/lighthouses are optional; helper self-test passed:
  `docs/platform/evidence/WL-CRIT-006-2026-08-11-exact-topology-roster-r251.md`.
- **Farm orchestrator timeout boundary (2026-08-11):** etcd curl range/get calls kill hung children and fail closed; BigBoy passed 1/1:
  `evidence/WL-CRIT-006-2026-08-11-farm-orchestrator-timeout-r227.md`.
- **Evidence identity binding (2026-08-10):** the release verifier rejects
  cross-wired seat evidence filenames; `.90` passed 1 valid and 16 hostile
  fixtures: `docs/platform/evidence/WL-CRIT-006-2026-08-10-evidence-identity-r177.md`.
- **CI source identity:** authoritative gates reject dirty, untracked,
  unresolved, or mid-run-mutated source; `.90` self-test passed:
  `evidence/WL-CRIT-006-2026-08-11-ci-source-identity-r397.md`.
- **Release gate identity bounds (2026-08-10):** oversized authenticated farm
  identities are rejected; the `.90` CI-gate self-test passed:
  `docs/platform/evidence/WL-CRIT-006-2026-08-10-release-identity-bound-r211.md`.
- **Release-binding inode stability (2026-08-11):** final descriptors are read
  through one stable descriptor and both pathname replacement and same-inode
  mutation fail without changing authenticated gate evidence. `.50` self-test
  passed:
  `docs/platform/evidence/WL-CRIT-006-2026-08-11-release-binding-inode-stability-r288.md`.
- **Publisher-attestation inode:** byte-identical replacement fails validation; `.130` self-test passed: `evidence/WL-CRIT-006-2026-08-11-publisher-attestation-inode-r398.md`.
- **Process namespace identity:** namespace directives cannot substitute the packaged binary; `.196` self-test:
  `evidence/WL-CRIT-006-WL-ARCH-009-2026-08-11-process-namespace-identity-r423.md`.
- **Release signer identity:** ambiguous/substituted primary keys roll back publication; `.196` self-test: `evidence/WL-CRIT-006-2026-08-11-release-signer-identity-r431.md`.
- **Release-evidence inode:** replacement/mutation during capture fails closed; `.90` self-test: `evidence/WL-CRIT-006-2026-08-11-release-evidence-inode-r435.md`.
- **Finalizer candidate inode:** post-verification replacement fails with identical bytes; `.170` self-test: `evidence/WL-CRIT-006-2026-08-11-finalizer-candidate-inode-r437.md`.
- **CI log inode:** promotion verification binds digest and semantics to one opened log inode; `.196` self-test: `evidence/WL-CRIT-006-2026-08-11-ci-log-inode-r439.md`.
- **Bootc image ID:** mutable tags cannot switch candidate bytes during verification; `.196` self-test:
  `evidence/WL-CRIT-006-2026-08-11-bootc-image-id-r444.md`.
- **Finalizer inode:** byte-identical replacement fails stable hashing; `.170` self-test passed: `evidence/WL-CRIT-006-2026-08-11-finalizer-artifact-inode-r404.md`.
- **Gate command-control boundary (2026-08-11):** shell control syntax is
  rejected in bounded release commands while safe parameter expansion remains
  allowed; `.50` passed 18 hostile fixtures:
  `docs/platform/evidence/WL-CRIT-006-2026-08-11-command-control-boundary-r219.md`.
- **Command-substitution boundary (2026-08-11):** corrected the command
  validator's `$(` gap while retaining safe `${MCNF_*}` expansion; `.50`
  passed 19 hostile fixtures:
  `docs/platform/evidence/WL-CRIT-006-2026-08-11-command-substitution-boundary-r222.md`.
- **Release-32 native-F44 three-seat checkpoint (2026-08-10):** an F42 candidate was rejected before install; the corrected signed F44 artifact then passed integrity,
  transaction, package, grouped-runtime, and Dell Browser-VM preservation on exactly Dell, seat 15, and Surface:
  `docs/platform/evidence/WL-CRIT-006-WL-CRIT-007-2026-08-10-release32-f44-three-seat-r126.md`.
- **Release-31 three-seat upgrade checkpoint (2026-08-10):** signed Fedora 44 bytes passed transaction, payload, grouped-runtime, and shell proof on Dell, seat 15,
  and Surface while preserving Dell's Browser VM: `docs/platform/evidence/WL-CRIT-006-WL-CRIT-007-2026-08-10-release31-three-seat-upgrade-r113.md`.
- **Explicit release-gate matrix checkpoint (2026-08-09):** the historical r9
  matrix named 19 required GitHub, farm/package, five-seat, three-lighthouse,
  and seven failure/recovery gates; the verifier rejects incomplete, duplicate,
  reordered, optional, or revision-mismatched plans. Machine 196 passed one
  positive and 12 hostile fixtures:
  `docs/platform/evidence/WL-CRIT-006-2026-08-09-release-gate-matrix-r9.md`.
  The 2026-08-10 operator cap supersedes the five-seat portion: the current
  matrix must require exactly Dell, seat 15, and Surface, never more than three
  physical test seats in one activity. The machine-enforced matrix, hostile
  checks, and collector proof are recorded in
  `docs/platform/evidence/WL-CRIT-006-2026-08-10-three-seat-acceptance-cap-r12.md`.
- **Remote provenance execution checkpoint (2026-08-10):** the first F44
  release-30 cut exposed and rejected an `env export` wrapper defect before
  packaging; recipes now execute as one quoted remote Bash program, and `.170`
  passed the focused nested-export regression gate:
  `docs/platform/evidence/WL-CRIT-006-2026-08-10-remote-provenance-execution-r11.md`.
- **Unique evidence claims (2026-08-10):** release verification rejects one
  artifact reused across independent gates; canonical matrix validation passed
  17 required gates and 15 hostile fixtures:
  `docs/platform/evidence/WL-CRIT-006-2026-08-10-unique-evidence-claims-r155.md`.
- **Singleton evidence claims (2026-08-11):** repeated manifests/topology/attestations/verdicts fail without replacing output; BigBoy self-test passed:
  `docs/platform/evidence/WL-CRIT-006-2026-08-11-singleton-evidence-claims-r303.md`.
- **Artifact inode revalidation:** validation hashes one opened inode and rechecks path/metadata; same-size replacement fails; focused farms passed:
  `evidence/WL-CRIT-006-2026-08-10-artifact-revalidation-r186.md`, `evidence/WL-CRIT-006-2026-08-11-artifact-hash-inode-binding-r380.md`.
- **Evidence inode stability (2026-08-11):** validation reads one opened evidence
  inode and recursively verifies a private, digest-bound gate-manifest snapshot;
  atomic replacement and same-inode mutation fail closed. Hostile local
  self-tests passed:
  `docs/platform/evidence/WL-CRIT-006-2026-08-11-evidence-inode-stability-r260.md`.
- **Production matrix identity gate (2026-08-10):** a production `pass` now
  invokes the canonical release-matrix verifier and refuses an incomplete,
  reordered, or source-revision-mismatched gate manifest; the helper self-test
  covers both the canonical matrix and a hostile revision. Farm proof:
  `docs/platform/evidence/WL-CRIT-006-2026-08-10-production-matrix-identity-r190.md`.
- **Required three-seat command boundary (2026-08-10):** the matrix verifier
  now rejects both whitespace and equals-form `--inspect-seat` arguments in a
  required seat command, preventing optional Eagle/T480 inspections from being
  smuggled into the Dell/seat-15/Surface release baseline. Farm proof:
  `docs/platform/evidence/WL-CRIT-006-2026-08-10-three-seat-command-boundary-r196.md`.
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
  4. S4 Run post-release baseline live-seat acceptance.
     - Objective: deploy the same revision to one selected physical test seat
       with alert protocol. Additional seats or lighthouses may be exercised,
       but are optional and non-blocking.
     - Inputs: S3, enrollment roster, rollout policy.
     - Deliverable: runtime, GUI, network, audio, VDI, and package captures.
     - Depends on: S3 and the first full release.
     - Acceptance: after the first full release, no stale installed payload or
       missing baseline seat is treated as pass. This post-release acceptance
       does not block producing the first full release.
     - Validation: the named live-seat script for the selected baseline seat.
     - Done when: the baseline seat has direct evidence; optional additional-seat/lighthouse rows do not gate completion.
  5. S5 Exercise post-release failure and corrected-forward recovery.
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
- Relevant files/components: AI_GOVERNANCE, CI workflow, install-helpers release/evidence/farm scripts, package manifests, docs/platform/evidence, baseline live-seat
  tooling; optional multi-seat/lighthouse tooling remains non-blocking.
- Dependencies: all P0/P1 feature epics, CRIT-007, and the active repository revision.
- Acceptance criteria:
  1. The first full release has complete signed farm, package, and
     artifact-integrity evidence. Post-release baseline single-seat and
     recovery evidence follows; optional multi-seat/lighthouse evidence may be
     added but is not required.
  2. GitHub required checks and verifier reject missing, altered, stale, or mismatched evidence.
  3. Promotion uses corrected-forward recovery and archives the closed epic.
- Verification method: worklist/governance/doc/secret/supersession lints, farm cargo/package gates, release verifier, and named live scripts; longest job on BigBoy.
- Origin or merged source IDs: 2026-07-30 fit-for-purpose audit and archived release/acceptance IDs.

### WL-CRIT-007 - Boot, sleep/resume, and fleet peer return recovery
- Status: Remaining
- Priority: P0
- Complexity: Epic
- Problem: boot and laptop sleep can leave Nebula, mackesd, Syncthing, etcd, and desktop state stale or duplicated.
- Required outcome: enrolled nodes/lighthouses recover one identity/session and synchronized substrate across boot, sleep, reboot, network change, and upgrade.
- Current state: bounded recovery exists; ordering, desktop restore, and fleet proof remain. Latest: `evidence/WL-CRIT-007-2026-08-11-peer-return-transition-r464.md`.
- Remaining work:
- Shared rollout proof is owned by `WL-TEST-001`; this epic keeps recovery implementation and fixtures.
- **Etcd restart authority:** etcd outage cannot use stale filesystem health; `.196` 1/1: `evidence/WL-CRIT-007-2026-08-11-etcd-restart-source-authority-r381.md`.
- **Fleet retry:** failure cannot defer the corrected next poll; `.90` 1/1: `evidence/WL-CRIT-007-2026-08-11-fleet-reconcile-corrected-forward-r396.md`.
- **Startup return retry:** transient instability cannot strand retained absence; `.90` 1/1: `evidence/WL-CRIT-007-2026-08-11-startup-return-retry-r399.md`.
- **Boot etcd authority:** failed directory reads cannot substitute stale filesystem peers; `.50` 1/1: `evidence/WL-CRIT-007-2026-08-11-boot-etcd-source-authority-r392.md`.
- **Boot overlay generation:** marker replacement/duplicate IP ownership blocks readiness; `.170` 1/1: `evidence/WL-CRIT-007-2026-08-11-boot-overlay-generation-r434.md`.
- **Missed-wake restart:** retained Sleeping returns after network stability; BigBoy 1/1: `evidence/WL-CRIT-007-2026-08-11-missed-wake-restart-r245.md`.
- **Expired-intent generation:** expiry cannot ignore newer Bus generation; BigBoy 1/1: `evidence/WL-CRIT-007-2026-08-11-expired-intent-generation-r290.md`.
- **Availability class:** restart and older Bus rows cannot substitute class; `.50`/`.90` 1/1 each: `evidence/WL-CRIT-007-2026-08-11-node-availability-device-class-r388.md`,
  `evidence/WL-CRIT-007-2026-08-11-availability-class-chain-r420.md`.
- **Host-state Bus inode:** same-path replacement blocks mutation pending fresh seat state; `.50` 1/1: `evidence/WL-CRIT-007-2026-08-11-host-state-bus-inode-r310.md`.
- **Restart barrier:** gaps clear readiness and force probes; `.50`/BigBoy passed 2/2: `evidence/WL-CRIT-007-2026-08-11-restart-readiness-barrier-r252.md`.
- **All-home XDG preflight:** `.50` refused hostile targets before mount mutation: `docs/platform/evidence/WL-CRIT-007-2026-08-10-xdg-all-home-preflight-r178.md`.
- **Missing Workstation session return:** additive restore passed `.90`: `docs/platform/evidence/WL-CRIT-007-2026-08-10-session-return-r205.md`.
- **Host snapshot freshness:** stale mirrors cannot authorize; `.50` passed 1/1: `docs/platform/evidence/WL-CRIT-007-2026-08-11-host-snapshot-freshness-r271.md`.
- **Nebula restart:** retained overlay readiness retracts until reload and active verification; focused farms 1/1 each:
  `evidence/WL-CRIT-007-2026-08-11-nebula-overlay-readiness-r274.md`, `evidence/WL-CRIT-007-2026-08-11-nebula-restart-revalidation-r389.md`.
- **Mirror readiness:** restart and rollback retract DNF readiness until corrected-forward; BigBoy 1/1 each:
  `evidence/WL-CRIT-007-2026-08-11-mirror-restart-readiness-r277.md`, `evidence/WL-CRIT-007-2026-08-11-mirror-generation-rollback-r418.md`.
- **Stale-session guard:** orphaned shells block duplicate recovery before XDG mutation; `.90`: `evidence/WL-CRIT-007-2026-08-10-stale-session-guard-r213.md`.
- **Post-etcd boundary:** recovery re-attests network before Syncthing mutation; `.90` fixtures: `evidence/WL-CRIT-007-2026-08-11-recovery-substrate-boundary-r218.md`.
- **SSH overlay IP:** invalid values block drop-in/reload; BigBoy 1/1: `evidence/WL-CRIT-007-2026-08-11-sshd-overlay-admission-r231.md`.
- **Bounded SSH overlay (2026-08-11):** file and reset/reload commands are bounded; BigBoy passed 9/9: `evidence/WL-CRIT-007-2026-08-11-sshd-bounded-r233.md`.
- **Syncthing registry bound (2026-08-10):** hostile output is capped before CLI mutation; BigBoy passed and seat 15 remained non-pegged:
  `docs/platform/evidence/WL-CRIT-007-2026-08-10-syncthing-registry-cap-r158.md`.
- **Bounded boot command output (2026-08-11):** `systemctl` readiness output is capped at 4096 bytes and oversized producers are killed; BigBoy passed 1/1:
  `evidence/WL-CRIT-007-2026-08-11-boot-command-output-bound-r229.md`.
- **Release-32 corrected-forward checkpoint (2026-08-10):** Dell, seat 15, and Surface returned from the signed F44 upgrade with exact package bytes, Nebula, Construct,
  target, and all six groups active; Surface root-key access is now direct:
  `docs/platform/evidence/WL-CRIT-006-WL-CRIT-007-2026-08-10-release32-f44-three-seat-r126.md`.
- **Lighthouse release-11 checkpoint (2026-08-10):** the signed package restores the omitted secret helper and mode-`0444` identity; `.1` passed RPM, grouped-runtime,
  three-voter quorum, peer-publication, watchdog, and recipient convergence after Dell scope-preserved four secrets to six registered recipients. Rollout to the two
  inaccessible voters remains:
  `docs/platform/evidence/WL-CRIT-006-WL-CRIT-007-2026-08-10-lighthouse-release11-payload-r116.md`.
- **Dell release-31 boot-status checkpoint (2026-08-10):** a warned reboot cut total boot from 90.390s to 56.811s, started the shell at 23.650s, and handed off its splash
  at 45.338s with grouped services and Browser VM recovered: `docs/platform/evidence/WL-CRIT-007-2026-08-10-dell-release31-boot-status-r115.md`.
- **Non-blocking grouped-upgrade checkpoint (2026-08-10):** after release 31 exposed a >60-second synchronous target restart, the next RPM now queues grouped daemon
  convergence without retaining the transaction lock; shell replacement remains synchronous: `docs/platform/evidence/WL-CRIT-007-2026-08-10-nonblocking-grouped-upgrade-r114.md`.
- **Release-31 three-seat return checkpoint (2026-08-10):** Dell, seat 15, and Surface returned from corrected-forward package replacement with exact payloads,
  all six grouped services, Nebula, and zero-restart shells active: `docs/platform/evidence/WL-CRIT-006-WL-CRIT-007-2026-08-10-release31-three-seat-upgrade-r113.md`.
- **Resource credential boot-retry checkpoint (2026-08-10):** transient SecretStore ordering failures retry every 30 seconds under a six-start/five-minute ceiling;
  terminal secret/configuration failures do not retry or mask failure: `docs/platform/evidence/WL-CRIT-007-WL-FUNC-019-2026-08-10-resource-credential-retry-r110.md`.
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
  `docs/platform/evidence/WL-CRIT-007-2026-08-09-substrate-order-r2.md`,
  `docs/platform/evidence/WL-CRIT-007-2026-08-10-recovery-readiness-order-r149.md`.
- **Post-lock network attestation checkpoint (2026-08-10):** recovery rechecks
  network readiness after acquiring its single-flight lock and refuses all
  mutation if the link disappeared; seat 90 passed the stale-network fixture:
  `docs/platform/evidence/WL-CRIT-007-2026-08-10-post-lock-network-attestation-r163.md`.
- **Overlay-to-substrate attestation checkpoint (2026-08-10):** recovery
  rechecks physical network readiness after Nebula's TUN address becomes ready,
  refusing configured substrate and downstream mutation when the link then
  disappears; `.90` passed the injected fault fixture:
  `docs/platform/evidence/WL-CRIT-007-2026-08-10-overlay-substrate-attestation-r184.md`.
- **Syncthing reconcile bound (2026-08-10):** timer/manual runs now serialize
  and bound CLI/registry calls, preventing stalled overlap from amplifying CPU;
  `.50` passed the device-scope self-test:
  `docs/platform/evidence/WL-CRIT-007-2026-08-10-syncthing-reconcile-bound-r154.md`.
- **Peer-publication failover checkpoint (2026-08-09):** Dell exposed a false
  healthy state when one reachable etcd voter could not commit. Client
  operations now remember and fail over to a committing member, heartbeat
  success is stamped only after the own-row transaction, and stale publication
  fails watchdog health. Dell and seat 15 are live again; lighthouse `.1` repair
  and full-fleet convergence remain:
  `docs/platform/evidence/WL-CRIT-007-2026-08-09-peer-publication-failover-r16.md`.
- **Health-generation restart checkpoint (2026-08-09):** Dell's restarted
  producer reset to generation 0 while durable ingress retained 4,527, freezing
  health history behind correct replay rejection. First-cycle generation now
  recovers from the durable canonical publication floor; machine 193 passed the
  exact rollback fixture:
  `docs/platform/evidence/WL-CRIT-007-WL-UX-013-2026-08-09-health-generation-restart-r17.md`.
- **Boot Readiness Bus checkpoint (2026-08-09):** service startup without a
  user data root now selects the documented shared `/run/mde-bus` spool instead
  of permanently terminating the readiness authority; machine 193 passed the
  exact fallback test:
  `docs/platform/evidence/WL-CRIT-007-2026-08-09-boot-readiness-bus-fallback-r18.md`.
- **Host-state Bus recovery checkpoint (2026-08-09):** host control now retries
  late storage, atomically skips retained mutations, folds durable seat state,
  and defers actions when the authorization mirror is unreadable. Machine 193
  passed two exact tests:
  `docs/platform/evidence/WL-ARCH-009-WL-CRIT-007-2026-08-09-host-state-bus-recovery-r33.md`.
- **Session Bus recovery checkpoints (2026-08-09):** unreadable storage defers convergence; late/replaced indexes preserve the live roster, skip retained rows, and admit
  forward actions. Machines 193/196 passed focused gates: `docs/platform/evidence/WL-ARCH-010-WL-CRIT-007-2026-08-09-session-bus-loss-r19.md`,
  `docs/platform/evidence/WL-ARCH-010-WL-ARCH-009-WL-CRIT-007-2026-08-09-session-bus-replacement-r71.md`.
- **Session-roaming Bus checkpoint (2026-08-09):** roaming now retries late Bus
  startup, folds queued policy after recovery, and defers destructive
  convergence whenever the action log is unreadable. BigBoy passed three exact
  recovery tests:
  `docs/platform/evidence/WL-ARCH-010-WL-CRIT-007-2026-08-09-session-roaming-bus-recovery-r21.md`.
  Same-path replacement proof: `docs/platform/evidence/WL-FUNC-019-WL-ARCH-009-WL-CRIT-007-2026-08-09-session-roaming-bus-replacement-r78.md`.
- **Compute-migration Bus checkpoint (2026-08-09):** late startup now folds
  outage-queued migration state from durable cursors, and all four Bus lanes
  must read before any migrate/apply/relinquish/rollback effect. BigBoy passed
  four exact recovery/durability tests:
  `docs/platform/evidence/WL-ARCH-010-WL-CRIT-007-2026-08-09-compute-migrate-bus-recovery-r25.md`.
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
- **Eagle additive recovery checkpoint (2026-08-09):** release 29 exposed a repeated asynchronous target-restart loop. Recovery now starts the target and only missing
  groups without stopping healthy processes; `.50` passed the full fixture. Privileged Eagle deployment remains:
  `docs/platform/evidence/WL-CRIT-007-2026-08-09-eagle-additive-group-recovery-r8.md`.
- **T480 lighthouse/restart checkpoint (2026-08-10):** a stale epoch-0 bundle repeatedly regenerated retired lighthouse endpoints; corrected-forward roster repair
  restored all three overlays. The watchdog now restores only missing groups and limits unreachable-overlay Nebula restarts to one per 600 seconds; `.50` passed the
  hostile fixture and T480 held healthy across the timer: `docs/platform/evidence/WL-CRIT-007-2026-08-10-t480-lighthouse-recovery-r106.md`.
- **Lighthouse quorum-capacity checkpoint (2026-08-10):** all three 512-MiB
  voters were OOM/swap-starved and continuously lost raft leadership. A
  one-at-a-time, CPU/RAM-only DigitalOcean resize put every voter at 1 GiB
  without changing its 10-GB disk, etcd data, membership, or identity. Term
  5534 then held with all three indexes converged; 25/25 seat overlay paths,
  every grouped service, fresh publication, and Dell's unchanged Browser VM
  passed a watchdog hold. Lighthouse `.1` also moved corrected-forward to the
  signed grouped release 9; `.2`/`.3` package access remains explicit:
  `docs/platform/evidence/WL-CRIT-007-2026-08-10-lighthouse-quorum-capacity-r107.md`.
- **Authoritative retirement checkpoint (2026-08-10):** destructive
  lighthouse retirement no longer counts replicated directory rows. It reads
  authoritative membership, directly probes every exact member, and refuses
  before revoke/removal/deletion unless enough reachable started voters
  survive. The current three-voter fleet therefore requires a converged fourth
  member before `.2` or `.3` replacement:
  `docs/platform/evidence/WL-CRIT-007-2026-08-10-authoritative-retire-gate-r108.md`.
- **Workload/session recovery checkpoint (2026-08-08):** terminal Display1
  recovery now reattaches only the latest valid exact generation and revokes
  superseded, expired, mismatched, orphaned, or stopped-workload leases without
  invoking lifecycle apply/cancel. `.90` passed 3/3; live first-frame proof
  remains: `docs/platform/evidence/WL-CRIT-007-2026-08-08-workload-session-recovery-s3-r1.md`.
- **Dell/bootc truthful boot status (2026-08-09):** warned release 25 removed blank boot gating; Construct activated at 28.434s before mesh at 58.966s. Release 27 retains
  the live kernel policy, and immutable bootc now has parity. Warned release 29
  reboot acceptance reconfirmed the correction: Construct was active at 25.986s,
  handed off its splash around 43s, and preceded mesh convergence at 57.526s;
  the running persistent 8-GiB Browser VM survived intact:
  `docs/platform/evidence/WL-CRIT-007-2026-08-09-dell-boot-status-release25-r18.md`,
  `docs/platform/evidence/WL-CRIT-007-2026-08-09-bootc-truthful-boot-status-r101.md`,
  `docs/platform/evidence/WL-CRIT-007-2026-08-09-dell-boot-status-release29-r105.md`.
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
     - Objective: execute boot/sleep/reboot/upgrade recovery on no more than three selected physical seats per activity and on the required lighthouses.
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
  3. Every selected test seat (maximum three) and required lighthouse has direct recovery evidence.
- Verification method: systemd/shell/Workload cargo gates, farm package checks, fault injection, and live recovery scripts; BigBoy runs the broadest gate.
- Origin or merged source IDs: operator boot/sleep peer-return bug and archived recovery incidents.

### WL-TEST-001 - First-release, installed-seat, and provider proof boundary

- Status: Remaining
- Priority: P0
- Complexity: Epic
- Problem: Product epics have substantial implementation and farm evidence, but
  their shared closure boundary is scattered across deferred release inputs,
  signed package production, installed-seat validation, provider access, and
  corrected-forward recovery. This encourages false closure or repeated proof.
- Required outcome: one bounded authority admits the exact source revision and
  mandatory inputs, verifies the signed first release, proves package/runtime
  integrity on no more than three physical seats plus lighthouses, and records
  provider/live/recovery evidence. Product epics depend on this epic only for
  shared rollout proof.
- Current state: Release preflight, signing/finalizer, topology identity,
  artifact binding, farm routing, and corrected-forward contracts exist with
  hostile evidence. The first release still lacks operator-supplied Maps, App
  VM, Cuttlefish, signing, bootc, and installed-provider inputs.
- Remaining work:
- **Admit release inputs:** obtain governed Maps approval/source/verifier, App
  VM trust receipt/key and base digest, Cuttlefish declaration/signature/
  packages/readiness relay/VDI agent/image receipt, RPM signing receipt, and
  bootc digest receipt. Run `release-input-preflight.sh` on one pinned revision.
- **Cut the signed release:** run `run-first-full-release.sh prepare` through
  the farm, perform operator-only signing/finalization, then run the canonical
  seven-role output plan, collector, and release-gate verifier. No unsigned or
  substituted artifact may reach promotion.
- **Baseline package proof:** verify NEVRA, payload digests, manifests, role
  gates, governed payloads, and source identity on no more than three physical
  test seats. Lighthouses retain independent quorum proof and are not seats.
- **Post-release product proof:** execute deferred provider/runtime captures
  referenced by the product epics and CRIT-007. Use one named seat when enough;
  expand only to three when the invariant is genuinely cross-seat.
- **Corrected-forward recovery:** exercise boot, sleep/resume, peer return,
  provider loss, package restart, stale-payload refusal, and corrected-forward
  deployment. Preserve the six-hour privacy boundary and never rollback.
- **Evidence disposition:** record each role/scenario with revision, command,
  farm host/slot or named seat, digest, and result under
  `docs/platform/evidence/`; archive superseded rollout diaries. Missing
  external providers remain precise blockers, not product-epic failures.
- Scope: Owns shared release admission, artifact/package/signing proof, baseline
  installed-seat acceptance, provider/live coordination, and recovery. Product
  behavior, provider implementation, UI, and daemon ownership remain elsewhere.
- Relevant files/components: release preflight/full-release helpers, release
  plan/collector/verifier, `xcp-build.sh`, `automation/promotion/`, packaging,
  and `docs/platform/evidence/`.
- Dependencies: ARCH-008, ARCH-009, ARCH-010, CRIT-006, CRIT-007, FUNC-017, FUNC-018, FUNC-020, FUNC-021, FUNC-022, UX-011, and operator-supplied release inputs.
- Acceptance criteria:
  1. One clean pinned revision and epoch pass release-input preflight with all mandatory receipts and bounded external artifacts.
  2. Signed Workstation, Server, and Lighthouse outputs plus the canonical seven-role plan pass identity, digest, package, and source-revision verification.
  3. Baseline installed proof passes on no more than three physical test seats, with independent lighthouse quorum evidence and no five-seat requirement.
  4. Every deferred product/provider/recovery scenario has a dated evidence record or a precise external blocker; no product epic duplicates this rollout queue.
  5. Corrected-forward recovery succeeds without rollback, stale payload admission, privacy-epoch violation, or duplicate authority.
- Verification method: farm-only build/package/signing gates with the longest
  job on BigBoy; hostile release self-tests; package inspection; live proof on
  one to three named seats; lighthouse quorum and corrected-forward evidence.
- Origin or merged source IDs: CRIT-006/007 release boundary, operator
  two/three-seat lock, and deferred proof obligations previously repeated across
  active epics.

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
- **Carbon icon registry drift gate (2026-08-10):** exact 44-asset parity,
  symbolic SVG, safe-name, and Apache-2.0 checks passed:
  `docs/platform/evidence/WL-UX-009-2026-08-10-carbon-registry-r213.md`.
- **Finite motion restart:** corrupt/non-finite timelines settle without repaint loops; `.50` 1/1: `evidence/WL-UX-009-2026-08-11-motion-finite-restart-r426.md`.
- **Disabled status tone:** unavailable workspaces cannot retain live semantic colors; BigBoy exact:
  `evidence/WL-UX-009-2026-08-11-disabled-status-tone-r456.md`.
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
- Current state: typed providers exist; coverage, safe actions, and fleet proof remain. Latest: `evidence/WL-UX-011-2026-08-11-hardware-staging-generation-r472.md`.
- **Device-control ownership checkpoint (2026-08-09):** privileged controls now
  require an exact match on provider host, category, name, sysfs path, and
  driver; forged and foreign-host targets cannot reach mutation. `.90` passed
  16/16 focused tests:
  `docs/platform/evidence/WL-UX-011-2026-08-09-device-control-ownership-r1.md`.
- **Device-control generation checkpoint (2026-08-09):** stale inventory timestamps cannot reach mutation; `.90` passed 6 contract, 17 executor, and 1 shell test:
  `docs/platform/evidence/WL-UX-011-2026-08-09-device-control-generation-r2.md`.
- **Device-control authorization checkpoint (2026-08-09):** exact-body, short-lived, single-use root-shell capabilities now gate the fixed executor; machine 9 passed
  contract, executor, and shell hostile regressions: `docs/platform/evidence/WL-UX-011-2026-08-09-device-control-authorization-r3.md`.
- **Unavailable provider control (2026-08-10):** `.90` passed: `docs/platform/evidence/WL-UX-011-2026-08-10-unavailable-control-r207.md`.
- Remaining work:
- **Inventory generation:** delayed pre-restart probes cannot replace newer truth; `.196` 1/1: `evidence/WL-UX-011-2026-08-11-device-inventory-generation-r386.md`.
- **Inventory staging identity:** symlink/hard-link substitution cannot redirect publication; `.90` 1/1: `evidence/WL-UX-011-2026-08-11-inventory-staging-nofollow-r421.md`.
- **Sysfs identity equivocation (2026-08-11):** aliases deduplicate and conflicts suppress only their hardware identity; BigBoy passed 1/1:
  `docs/platform/evidence/WL-UX-011-2026-08-11-sysfs-identity-equivocation-r278.md`.
- **Phone physical identity (2026-08-11):** exact duplicates collapse and conflicting device IDs suppress only that phone; BigBoy passed 1/1:
  `docs/platform/evidence/WL-UX-011-2026-08-11-phone-identity-equivocation-r292.md`.
- **Physical block provider bound (2026-08-11):** virtual rows are filtered before the 256-physical-device budget; `.50` passed 1/1:
  `docs/platform/evidence/WL-UX-011-2026-08-11-physical-block-provider-bound-r246.md`.
- **Device command timeout (2026-08-11):** fixed helpers terminate on a 30-second deadline; `.90` passed: `docs/platform/evidence/WL-UX-011-2026-08-11-command-timeout-r224.md`.
- **CONNECT text bound (2026-08-11):** state/Caddy reads cap content at 128 KiB; BigBoy: `docs/platform/evidence/WL-UX-011-2026-08-11-connect-managed-text-bound-r225.md`.
- **CONNECT policy bound (2026-08-11):** exposure/DDNS reads cap at 128 KiB; BigBoy passed 1/1: `evidence/WL-UX-011-2026-08-11-connect-config-bound-r233.md`.
- **Unavailable control all-verbs admission (2026-08-11):** unresolved provider state blocks Enable, Disable, Reload Module, and Rescan Bus without sysfs mutation;
  BigBoy passed:
  `docs/platform/evidence/WL-UX-011-2026-08-11-unavailable-control-all-verbs-r217.md`.
- **Power-supply inventory bound (2026-08-11):** published power-supply
  entities are capped at 64 in deterministic lexical order; `.50` passed the
  oversized fixture:
  `docs/platform/evidence/WL-UX-011-2026-08-11-power-supply-bound-r220.md`.
- **Bounded hardware ID databases (2026-08-11):** `pci.ids`/`usb.ids` reads cap at 16 MiB before parsing; BigBoy passed 1/1:
  `evidence/WL-UX-011-2026-08-11-ids-database-bound-r229.md`.
- **Bounded hardware probes (2026-08-11):** inventory commands fail closed on hangs; BigBoy passed 1/1: `evidence/WL-UX-011-2026-08-11-hardware-probe-timeout-r233.md`.
- **Device-control cancellation checkpoint (2026-08-09):** a signed cancellation
  can atomically claim only the exact still-pending request; late/refused
  cancellation cannot replace its eventual execution result, and the shell
  refuses a second mutation while retaining that identity. BigBoy passed six
  exact contract, executor, and shell tests:
  `docs/platform/evidence/WL-UX-011-2026-08-09-device-control-cancellation-r14.md`.
- **Surface Pro 5/6 contract checkpoint (2026-08-09):** one shared bounded
  observation/action contract now gates exact Pro 5 SKU and Pro 6 identity,
  fail-closed activation, exact device-scoped firmware apply, read-only
  camera/fingerprint discovery, sole-runner DRM mode changes, and local-only
  typed shell projections. Fixed iptsd/SAM activation, a crash-safe local MOK
  authority minter, revision/collector-bound deployment preflight, console-only
  SSH recovery preflight, and hash-bound Pro 6-then-Pro 5 physical recorder now
  fail closed around the remaining operator steps. A responsive Surface card
  fixture covers 320/480/960 px, touch/mouse density, large text, and both
  color schemes without exposing dead activation/reboot controls. Exact locked
  sources, unsigned Fedora 44 userspace RPM producer proof, and a deterministic
  signed-stack finalization contract exist. Image promotion still fails closed
  pending the matching kernel-module signing key, kernel build capacity, an
  approved SSH public-key artifact/fingerprint, release-signed artifact
  publication, governed seat access, and direct Pro 6 then Pro 5 physical
  proof. Focused farm contract, daemon, shell, producer, provenance, and
  collector gates passed; no physical acceptance is claimed:
  `docs/platform/evidence/WL-UX-011-2026-08-09-surface-pro56-contract-r15.md`.
- **Surface Pro 5/6 first-class runtime checkpoint (2026-08-10):** shared
  bounded enable/MOK and firmware-result contracts replace private shell wire
  mirrors; `surface_enable` has no reboot authority or legacy arm state; staged
  MOK routes authority-free to the existing host-state Power workflow. A
  separately authorized one-frame camera proof discards all pixels and now
  hash-binds collector, physical-record, and promotion input. Device Inventory
  renders exact Pro 5/6 Surface summaries fleet-wide while every remote control
  remains refused. Final adversarial farm gates passed 19 shared and 120 daemon
  Surface tests; focused shell, camera, firmware, fleet, and hostile parser
  gates also pass. Physical acceptance and promotion still fail closed pending
  the kernel signing key/capacity, release-signed artifact set, approved SSH key
  artifact, governed canonical-seat access, and direct Pro 6 then Pro 5 proof:
  `docs/platform/evidence/WL-UX-011-2026-08-10-surface-pro56-first-class-runtime-r16.md`.
- **Surface pending-action cancellation checkpoint (2026-08-10):** Surface
  enable/MOK and exact-device firmware apply now accept a separately signed,
  exact-target cancellation only before the local worker claims effects.
  The original r17 Bus-claim architecture was rejected by adversarial review
  and is retained only as superseded history. Correct-forward r18 uses a
  root-owned descriptor-anchored journal, terminal publication outbox, and
  Bus-independent crash recovery; it also makes `prepare`/`seal` the canonical
  race-free camera acceptance flow and aligns all twelve physical checks.
  Final farm gates passed 10 journal, 17 shared, 2 daemon recovery, 6 CLI, and
  1 shell test, plus wrapper and collector/recorder/promotion hostile suites:
  `docs/platform/evidence/WL-UX-011-2026-08-10-surface-cancellation-journal-acceptance-seal-r18.md`.
- **Physical network-interface provider checkpoint (2026-08-10):** Device
  Inventory now publishes bounded, credential-free physical wired/Wi-Fi
  interfaces with exact control-compatible sysfs identity and truthful link
  state; `.90` passed the exact regression:
  `docs/platform/evidence/WL-UX-011-2026-08-10-network-interface-provider-r19.md`.
- **Sensor inventory bound (2026-08-10):** thermal/hwmon entities are capped
  and selected deterministically without credential-shaped payloads; `.90`
  passed: `docs/platform/evidence/WL-UX-011-2026-08-10-sensor-cap-r155.md`.
- **Deterministic thermal source checkpoint (2026-08-10):** sysfs thermal zones
  are sorted before the bounded provider limit, preventing directory-order
  churn in hardware summaries; `.50` passed the focused hostile-sensor gate:
  `docs/platform/evidence/WL-UX-011-2026-08-10-thermal-zone-order-r153.md`.
- **Sysfs control nofollow checkpoint (2026-08-10):** provider-planned control
  writes refuse a replaced final symlink before any effect; `.90` passed the
  hostile daemon regression:
  `docs/platform/evidence/WL-UX-011-2026-08-10-sysfs-control-nofollow-r183.md`.
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
     - Deliverable: Workers device_inventory view and at-most-three-seat/three-lighthouse evidence.
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
- Current state: placement and full-width geometry scaffolding exist; exact icon/action semantics, persistence, responsive behavior, and three-seat proof remain.
- **Live battery (2026-08-08/09):** the primary UPower percentage/icon is immediately left of the clock in both placements; `.90` passed the two exact source/geometry tests:
  `docs/platform/evidence/WL-UX-012-2026-08-09-live-battery-r13.md`.
- **Taskbar identity checkpoint (2026-08-09):** connected sessions and pinned desktops now have disjoint typed egui identities and hit regions; BigBoy passed 49/49:
  `docs/platform/evidence/WL-UX-012-2026-08-09-taskbar-control-identity-r2.md`.
- **Narrow geometry checkpoint (2026-08-09):** center controls are admitted only when a physical 40px slot exists, preserving More at 480px and preventing Home overlap at
  320px; `.50` passed 50/50: `docs/platform/evidence/WL-UX-012-2026-08-09-narrow-center-geometry-r3.md`.
- **Taskbar action map (2026-08-10):** typed Start/Search/Back/Home map passed; conflicting cycle deleted: `docs/platform/evidence/WL-UX-012-2026-08-10-taskbar-action-map-r4.md`.
- **Taskbar pin identity (2026-08-10):** BigBoy passed stable deduplication: `docs/platform/evidence/WL-UX-012-2026-08-10-taskbar-pin-dedupe-r156.md`.
- **Front Door command boundary:** unsafe/oversized `>` input refuses before terminal activation; BigBoy passed: `evidence/WL-UX-012-2026-08-10-command-input-boundary-r187.md`.
- **Short Left rail:** 320×160 containment and placement escape passed BigBoy: `evidence/WL-UX-012-2026-08-11-left-placement-escape-r280.md`.
- **Future preference schema:** untrusted placement/pins fail to empty Bottom; `.50` 1/1: `evidence/WL-UX-012-2026-08-11-future-preference-schema-r422.md`.
- Remaining work:
- **Home wallpaper inode:** replaced or non-regular cache paths cannot redirect decode; BigBoy exact:
  `evidence/WL-UX-012-2026-08-11-home-wallpaper-inode-r452.md`.
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
     - Objective: verify Bottom/Left, Dark/Light, large text, lock, multi-display, session switching, package upgrade, and captures on no more than three seats.
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
  3. At-most-three-seat responsive/package proof passes without a second launcher.
- Verification method: shell model/render/navigation cargo gates, package checks, and direct-DRM/Sunshine captures on no more than three selected seats.
- Origin or merged source IDs: 2026-07-29 taskbar/Home operator lock and archived dock workstreams.

### WL-UX-013 - System and Mesh Health history and expected-state intelligence

- Status: Remaining
- Priority: P0
- Complexity: Epic
- Problem: the centered Health modal lacks complete expected-state intent, adaptive durations, history/recurrence, safe recovery, and truthful transition handling.
- Required outcome: one centered System and Mesh Health authority distinguishes expected absence from outage, computes A-F grades from signed bounded evidence, keeps
  active issues above paged history, supports filters/detail/recurrence/export, and offers only governed recovery.
- Current state: signed A-F authority exists; live proof remains. Evidence: `evidence/WL-UX-013-2026-08-11-health-history-capacity-r460.md`.
- Remaining work:
- **Bounded heartbeat fallback (2026-08-11):** peer heartbeat recovery uses
  the existing regular-file byte bound before JSON parsing; `.90` passed:
  `docs/platform/evidence/WL-UX-013-2026-08-11-heartbeat-byte-bound-r224.md`.
- **Expected-absence legacy fold (2026-08-11):** planned absence suppresses false outage while missed return escalates through shared policy; `.90` passed 2/2:
  `docs/platform/evidence/WL-UX-013-2026-08-11-expected-absence-legacy-fold-r240.md`.
- **Post-intent heartbeat (2026-08-11):** later heartbeat evidence supersedes stale expected absence after restart; `.196` passed 1/1:
  `docs/platform/evidence/WL-UX-013-2026-08-11-post-intent-heartbeat-r291.md`.
- **Canonical expected-state transition (2026-08-11):** bounded intent keeps declared absence informational and escalates missed return;
  isolated `.90` gate passed 1/1: `docs/platform/evidence/WL-UX-013-2026-08-11-canonical-expected-state-r254.md`.
- **Truthful recovery timing (2026-08-11):** resolved health history preserves
  the final positive observation and records detection of recovery separately,
  so incident duration cannot absorb an unobserved gap; `.90` passed 1/1:
  `docs/platform/evidence/WL-UX-013-2026-08-11-truthful-recovery-timing-r247.md`.
- **History privacy epoch:** restart and fresh publications reject resolved incidents beyond six hours; focused farms passed 1/1 each:
  `evidence/WL-UX-013-2026-08-11-history-privacy-epoch-r302.md`, `evidence/WL-UX-013-2026-08-11-resolved-history-privacy-r378.md`.
- **Active-condition continuity:** forward generations cannot silently erase active issues; `.170` 1/1: `evidence/WL-UX-013-2026-08-11-active-condition-continuity-r438.md`.
- **Health-modal privacy:** hostile projection text cannot expose secrets or local paths; `.170` 1/1: `evidence/WL-UX-013-2026-08-11-health-modal-privacy-r440.md`.
- **Decommissioned health projection cleanup (2026-08-11):** retired publishers are evicted from ledger, cursor, projection, and restart checkpoint after staged roster reads;
  `.90` passed:
  `docs/platform/evidence/WL-UX-013-2026-08-11-health-decommissioned-projection-r217.md`.
- **Missing projection repair (2026-08-11):** retained exact health state
  restores a deleted derived projection without a new Bus message; `.90`
  passed:
  `docs/platform/evidence/WL-UX-013-2026-08-11-missing-projection-repair-r221.md`.
- **Bounded firewall history (2026-08-11):** retention refuses non-regular or
  over-4-MiB JSONL before rewrite; BigBoy passed 1/1:
  `evidence/WL-UX-013-2026-08-11-firewall-history-bound-r228.md`.
- **Bounded firewall journal (2026-08-11):** oversized `journalctl` output is
  rejected before cursor advancement; BigBoy passed 1/1:
  `evidence/WL-UX-013-2026-08-11-firewall-journal-bound-r230.md`.
- **Bounded alert relay input (2026-08-11):** alert JSON refuses symlinks and
  over-64-KiB payloads before parsing; BigBoy passed 1/1:
  `evidence/WL-UX-013-2026-08-11-alert-relay-bound-r228.md`.
- **Future health freshness:** `.50` passed refusal of zero/future-dated snapshots:
  `docs/platform/evidence/WL-UX-013-2026-08-10-future-health-freshness-r181.md`.
- **Device-inventory provenance:** future-dated or foreign-host inventory cannot
  contribute an A grade; `.196` 1/1:
  `evidence/WL-UX-013-2026-08-11-device-inventory-provenance-r395.md`.
- **Modal generation authority:** fresh-timestamp rollback cannot erase outages; `.50` 1/1: `evidence/WL-UX-013-2026-08-11-modal-generation-authority-r414.md`.
- **Duplicate active-condition admission:** `.90` passed refusal of repeated active
  `(scope, id)` identities while preserving repeated resolved records for recurrence:
  `docs/platform/evidence/WL-UX-013-2026-08-10-duplicate-active-condition-r185.md`.
- **Condition lifecycle identity admission (2026-08-10):** active/resolved
  identity splits are rejected; `.90` passed:
  `docs/platform/evidence/WL-UX-013-2026-08-10-condition-lifecycle-identity-r215.md`.
- **Health expiry projection checkpoint (2026-08-10):** expired checkpoint state and stale invalid projections fail closed without touching symlinks; BigBoy passed:
  `docs/platform/evidence/WL-UX-013-2026-08-10-health-expiry-projection-r160.md`.
- **Live health expiry (2026-08-10):** expired retained projections are evicted
  without daemon restart; `.90` passed:
  `docs/platform/evidence/WL-UX-013-2026-08-10-live-expiry-r206.md`.
- **Availability duplicate-precedence checkpoint (2026-08-10):** duplicate evidence is classified before at-capacity overflow in forward and reversed order, while a
  distinct extra node remains `CapacityExceeded`; machine 194 passed the exact regression: `docs/platform/evidence/WL-UX-013-2026-08-10-availability-duplicate-precedence-r13.md`.
- **Expected-state boundary checkpoint (2026-08-06):** the health contract
  suite covers `Sleeping → Returned` at the `u64::MAX` boundary and refuses an
  overlong TTL; `.50` passed 1/1. Evidence:
  `docs/platform/evidence/WL-UX-013-2026-08-06-health-boundary-r1.md`.
- **Durable ingress checkpoint (2026-08-08):** exact approved-publisher health
  ingress now rejects replay/rollback and atomically preserves its bounded
  per-observer cursor/ledger across restart; `.170` passed 24/24:
  `docs/platform/evidence/WL-UX-013-2026-08-08-health-ingress-checkpoint-s2-r1.md`.
- **Health-ingress Bus recovery checkpoint (2026-08-09):** all bounded files
  and publisher lanes stage before ledger, cursor, projection, or checkpoint
  effects; failed reads defer the complete candidate. Machine 194 passed five exact tests:
  `docs/platform/evidence/WL-UX-013-WL-ARCH-009-2026-08-09-health-reconciler-bus-recovery-r48.md`.
- **Producer restart-generation checkpoint (2026-08-09):** a restarted health
  producer now advances from its durable canonical publication floor instead of
  resetting below ingress replay state; machine 193 passed the exact Dell
  rollback fixture:
  `docs/platform/evidence/WL-CRIT-007-WL-UX-013-2026-08-09-health-generation-restart-r17.md`.
- **Projection freshness checkpoint (2026-08-09):** the roster fold cannot
  outlive its earliest admitted source or the ten-minute contract maximum;
  `.90` passed 14/14 health tests including hostile `u64::MAX` validity:
  `docs/platform/evidence/WL-UX-013-2026-08-09-projection-freshness-r2.md`.
- **Status-cell provenance checkpoint (2026-08-10):** expired health evidence
  renders `Stale`, never green `OK`; missing evidence remains unavailable,
  resolved conditions are excluded, and fresh informational expected absence
  remains non-outage. Machine 193 passed the exact focused test:
  `docs/platform/evidence/WL-UX-013-2026-08-10-stale-status-cell-r12.md`.
- **Future-heartbeat checkpoint (2026-08-10):** future-dated heartbeats resolve
  to `Unreachable` instead of fresh health; `.90` passed:
  `docs/platform/evidence/WL-UX-013-2026-08-10-future-heartbeat-r155.md`.
- **Recovery target checkpoint (2026-08-09):** a condition cannot authorize remediation on another node; machine 194 passed 13/13:
  `docs/platform/evidence/WL-UX-013-2026-08-09-recovery-target-binding-r3.md`.
- **Final-boundary recovery authority (2026-08-11):** the Health modal
  revalidates current canonical condition/scope, snapshot generation, offered
  remediation, and confirmation before Bus publication; BigBoy passed 1/1:
  `docs/platform/evidence/WL-UX-013-2026-08-11-final-recovery-authority-r257.md`.
- **Action-result progress (2026-08-11):** exact node/mesh publisher identity and generation bind terminal results; unresolved conditions report partial failure. `.50` passed 1/1:
  `docs/platform/evidence/WL-UX-013-2026-08-11-action-result-progress-r262.md`.
- **Bounded action-result contract (2026-08-11):** unknown, oversized, secret-bearing, future, or malformed rows fail before journal/publication/presentation; its
  exact extension gate is capacity-blocked:
  `docs/platform/evidence/WL-UX-013-2026-08-11-action-result-contract-r264.md`.
- **Action-result replay conflict (2026-08-11):** restart recovery acknowledges
  an existing result only on complete typed equality; a conflicting body under
  the same audit ID leaves the genuine journal and cursor intact. BigBoy passed
  1/1: `docs/platform/evidence/WL-UX-013-2026-08-11-action-result-replay-conflict-r266.md`.
- **Grade E authority checkpoint (2026-08-09):** two distinct active required warnings produce E without duplicate-delivery inflation; machines 9 and 194 passed the
  shared and worker suites: `docs/platform/evidence/WL-UX-013-WL-UX-014-2026-08-09-grade-e-authority-r5.md`.
- **History/selection checkpoint (2026-08-09):** paint-time history retains only the ordered top eight node rows, and live reorder/removal cannot silently move the
  selected detail target. Machine 9 passed both focused tests: `docs/platform/evidence/WL-UX-013-2026-08-09-history-selection-r6.md`.
- **Recurrence aggregation checkpoint (2026-08-09):** repeated stable condition
  identities now occupy one bounded history row with an exact same-node count;
  representative choice is deterministic under reversed input and history
  still materializes at most eight rows. BigBoy passed the exact hostile test:
  `docs/platform/evidence/WL-UX-013-2026-08-09-recurrence-aggregation-r7.md`.
- **History-window checkpoint (2026-08-09):** the bounded recurrence page now
  admits only resolved same-node records inside the snapshot's inclusive
  24-hour window; future, unresolved, and older high-severity rows cannot
  displace valid history. BigBoy passed the exact hostile filter test:
  `docs/platform/evidence/WL-UX-013-2026-08-09-history-window-filter-r8.md`.
- **History severity-filter checkpoint (2026-08-10):** the Health detail view
  now applies All/Warning/Critical selection before recurrence aggregation and
  the eight-row cap, while preserving node and 24-hour bounds. BigBoy passed
  the exact regression plus the four-test related history suite:
  `docs/platform/evidence/WL-UX-013-2026-08-10-history-severity-filter-r140.md`.
- **Health-action publication checkpoint (2026-08-09):** missing, unreadable,
  or unwritable local Bus state now produces a visible bounded modal error;
  confirmed recovery intent remains pending until its generation- and
  target-bound request is durably published. Machine 196 passed the exact
  hostile publication fixture:
  `docs/platform/evidence/WL-UX-013-2026-08-09-health-action-publication-r9.md`.
- **Health-result durability checkpoint (2026-08-09):** a root-owned bounded
  local journal prevents repeated remediation and recovers exact terminal
  replies across store, Bus, or restart faults. Machine 193 passed five focused
  durability/security fixtures:
  `docs/platform/evidence/WL-UX-013-2026-08-09-health-result-durability-r10.md`.
- **Node-grade transaction checkpoint (2026-08-09):** late/replaced Bus recovery, durable remediation results, and strictly forward canonical/Bus generations passed
  five focused machine-194 gates: `docs/platform/evidence/WL-UX-013-WL-ARCH-009-2026-08-09-node-grade-bus-recovery-r66.md`.
- **Node-availability recovery (2026-08-09):** durable truth corrects forward across late/replaced Bus storage without committing partial ledger state. Machine 193 passed 4/4:
  `docs/platform/evidence/WL-UX-013-WL-ARCH-009-2026-08-09-node-availability-bus-transaction-recovery-r88.md`.
- **Expired expected-absence checkpoint (2026-08-10):** an expired durable expected-state
  intent remains readable for audit but is not republished onto the health Bus; the next
  live transition wins without stale projection. BigBoy passed the exact regression:
  `docs/platform/evidence/WL-UX-013-2026-08-10-expired-expected-absence-r151.md`.
- **Quiet-Windows discovery checkpoint (2026-08-09):** ping-silent local hosts reach bounded TCP 3389 fingerprinting without widening the `/24`; BigBoy passed:
  `docs/platform/evidence/WL-FUNC-019-WL-UX-013-2026-08-09-rdp-lan-detection-r74.md`.
- **Seat 15 RDP catalog-TTL closure (2026-08-09):** release 27 kept the card available beyond the old two-minute cutoff and renewed it across two scans:
  `docs/platform/evidence/WL-FUNC-019-WL-UX-013-WL-ARCH-009-2026-08-09-release27-rdp-continuity-r100.md`.
- **Canonical health Bus-twin checkpoint (2026-08-10):** an exact signed Bus
  twin of already-staged canonical health now advances its cursor without a
  false replay rejection; non-identical equal/older generations still fail
  closed. `.90` passed the exact regression:
  `docs/platform/evidence/WL-UX-013-2026-08-10-health-bus-twin-r122.md`.
- **Expected-return revision checkpoint (2026-08-10):** sleep/maintenance
  idempotency now binds the requested return duration, so a revised deadline
  publishes a new generation while exact and saturating retries remain stable;
  `.90` passed the exact regression:
  `docs/platform/evidence/WL-UX-013-2026-08-10-expected-return-revision-r123.md`.
- **Support-export checkpoint (2026-08-09):** the modal now writes one explicit,
  bounded/redacted support bundle through a private no-follow path and a synced
  atomic transaction, with honest failures and bounded top-N materialization.
  Machine 196 and BigBoy passed eight exact security/UI checks:
  `docs/platform/evidence/WL-UX-013-2026-08-09-health-support-export-r11.md`.
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
     - Objective: render wide/narrow/largest-text states and test
       boot/sleep/network/maintenance/outage/rejoin on no more than three
       selected physical seats per activity and on the required lighthouses.
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
  3. At-most-three-seat/lighthouse proof shows no false emergency or duplicate authority.
- Verification method: health/property/fault/UI/package cargo gates, secret scans, and direct transition captures; longest health suite on BigBoy.
- Origin or merged source IDs: 2026-08-04 System and Mesh Health survey and archived health authority work.

### WL-UX-014 - Grade-specific cinematic Kiron health lower thirds

- Status: Remaining
- Priority: P1
- Complexity: Epic
- Problem: KIRON has generic typed toasts but no governed A-F payload, authored scenes, audio, ticker, fallback ladder, or bounded health interaction.
- Required outcome: one ToastHost renders six license-clean A-F health scenes and recovery transitions from UX-013 authority, with exact dwell/audio, grouping/ticker,
  safe deep links, live-3D/pre-rendered/static fallback, and no second renderer or sound owner.
- Current state: A-F authority exists; assets and live proof remain. Evidence: `evidence/WL-UX-014-2026-08-11-node-grade-observer-generation-r468.md`.
- **F-grade backlog checkpoint (2026-08-09):** the hold-until-ack queue is capped at 64 waiters without displacing admitted critical FIFO; BigBoy passed 34/34:
  `docs/platform/evidence/WL-UX-014-2026-08-09-f-grade-backlog-bound-r1.md`.
- **Shared KIRON contract (2026-08-09):** canonical UX-013 grade/generation/timing metadata maps into one ToastHost with safe Workers routing. Grade E now has sole-policy
  production and exact 15-second critical dwell; unknown grades fail closed. Machines 9, 194, and 196 passed focused gates:
  `docs/platform/evidence/WL-UX-014-2026-08-09-shared-health-kiron-contract-r4.md`,
  `docs/platform/evidence/WL-UX-013-WL-UX-014-2026-08-09-grade-e-authority-r5.md`.
- **Future-timestamp admission checkpoint (2026-08-10):** the ToastHost health
  boundary refuses a KIRON alert whose lifecycle start or observation timestamp
  is ahead of the seat clock; the focused hostile regression is recorded in
  `docs/platform/evidence/WL-UX-014-2026-08-10-future-timestamp-admission-r194.md`.
- **Kiron asset admission (2026-08-10):** self-test passed coverage/license/size/path/digest rules: `docs/platform/evidence/WL-UX-014-2026-08-10-kiron-asset-admission-r212.md`.
- Remaining work:
- **Kiron asset inode:** multiply-linked scenes cannot retain mutation authority across restart; `.196` self-test:
  `evidence/WL-UX-014-2026-08-11-kiron-asset-inode-r446.md`.
- **Health toast watermark (2026-08-11):** stale replay stays refused after queue removal and overflow fails closed; `.170` passed 1/1:
  `docs/platform/evidence/WL-UX-014-2026-08-11-health-toast-watermark-r282.md`.
- **Grade E/F lifecycle (2026-08-11):** timed grade E cannot enter grade F's acknowledgement-only path; `.90` passed 1/1:
  `docs/platform/evidence/WL-UX-014-2026-08-11-grade-e-f-lifecycle-r243.md`.
- **Delayed timeline:** elapsed drains timed alerts but stops at grade-F acknowledgement; `.90` 1/1: `evidence/WL-UX-014-2026-08-11-delayed-toast-timeline-r400.md`.
- **KIRON generation bridge:** stale/equal replay cannot replace grade F; `.90` 1/1: `evidence/WL-UX-014-2026-08-11-kiron-generation-bridge-r409.md`.
- **Status-grade watermark:** foreign/rollback/equivocal health cannot relabel chrome; BigBoy 1/1:
  `evidence/WL-UX-014-2026-08-11-status-grade-watermark-r451.md`.
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
     - Objective: exercise all grades, fallback, audio, GPU loss,
       suspend/resume, lock, immersive, reduced motion, multi-display, package
       upgrade, and runtime on no more than three seats.
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
  3. Asset provenance, package, farm, and at-most-three-seat evidence is reproducible.
- Verification method: health/toast/asset/renderer/accessibility cargo gates,
  package/license checks, golden/video/waveform captures, and live proof on no
  more than three seats; BigBoy
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
