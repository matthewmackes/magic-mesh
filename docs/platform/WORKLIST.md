# Platform Worklist

This is the only active platform worklist. Design notes, evidence ledgers,
runbooks, and operator notes are inputs, not parallel trackers. Historical
implementation diaries remain in docs/worklist-archive/ and are not executable
tasks.

## Current Snapshot - 2026-08-06 executable story rewrite

- **8 active epics:** 8 `Remaining`, 0 `Blocked`, 0 `Needs clarification`.
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
  recovery, and deferred provider/live proofs are owned by `WL-TEST-001`.
  Product epics must not duplicate those rollout tasks; they retain only
  product-specific implementation and integration gaps, and cite `WL-TEST-001`
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
    under WL-TEST-001.

## Story execution contract

Every story below is a self-contained unit. The implementing agent must:
read the named inputs; change only the owned files; produce the named deliverable;
add the stated hostile or regression test; run the stated validation; record the
revision, command, result, and evidence path; and mark the story complete only
when the Done when condition is true. A passing compile without the named
behavioral evidence is not completion.

## Core Architecture


### WL-ARCH-009 - Process-isolated mackesd and unified Workers interface
- Status: Remaining
- Priority: P0
- Complexity: Epic
- **Farm gate (2026-08-14):** BigBoy `mackesd` passed 4,999/4,999; release inputs remain under `WL-TEST-001`: `evidence/WL-ARCH-009-2026-08-14-mackesd-full-farm-gate-r1.md`.
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
  A bounded transient 128 MiB allocation under a 16 MiB/no-swap boundary was OOM-killed exactly at
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
     - Deliverable: at-most-two-workstation/three-lighthouse evidence bundle.
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
  4. Fleet chaos and at-most-two-seat/three-lighthouse evidence passes.
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
- **Native collaboration full gate (2026-08-14):** BigBoy passed 136/136: `evidence/WL-FUNC-011-2026-08-14-collab-egui-full-farm-gate-r1.md`.
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
     - Objective: run offline/online, permission, media, transfer, editor, clipboard, migration, recovery, and at-most-two-seat live acceptance.
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

### WL-FUNC-020 - Expose governed Android applications in Workloads

- Status: Remaining
- Priority: P1
- Complexity: Large
- Problem: Android is represented by partially integrated Cuttlefish layers without a complete signed app catalog, image/provider contract, lifecycle, or honest failure
  UX.
- Required outcome: Workloads exposes governed Android app, outer Android VM, and full Workstation desktop choices; the app path uses a signed AOSP/Cuttlefish image,
  typed start/stop/readiness, VDI presentation, and bounded host isolation.
- Current state: signed catalog/import, provider preflight, crash-safe lifecycle,
  bounded guest relay, typed VDI source, governed Workloads cards/actions, and
  reproducible guest packaging exist; release inputs, nested-KVM execution, and
  live proof remain.
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
- **Android Remote Sessions handoff (2026-08-14):** typed catalog/readiness,
  exact-generation VDI source, authorization refusal, and no-dial behavior
  passed 30/30 on `.90`; nested-KVM/live execution remain:
  `evidence/WL-FUNC-020-2026-08-14-android-remote-session-farm-gate-r1.md`.
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
- **Guest packaging contracts (2026-08-14):** `.90` passed the contract
  Android/Cuttlefish contract, image-manifest, signed guest-payload, and image
  receipt self-tests; BigBoy `.130` passed the full guest-DEB/staging fixtures
  after the tracked Cargo lockfile was refreshed:
  `evidence/WL-FUNC-020-2026-08-14-guest-packaging-contract-farm-gate-r1.md`.
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
     - Objective: verify image provenance, SELinux/cgroup/device isolation, audio/input, reconnect, upgrade, and acceptance on no more than two seats.
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
- **Media UI full farm gate (2026-08-14):** `.90` passed 114/114; H.264 remains transcode-only under the mpv baseline: `evidence/WL-FUNC-021-2026-08-14-media-full-farm-gate-r1.md`.
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
- **Full Music daemon farm gate (2026-08-14):** `mde-musicd` passed 274/274 tests after eliminating the parallel `HOME` mutation race in cover-art proof:
  `evidence/WL-FUNC-021-2026-08-14-music-full-farm-gate-r1.md`.
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
  3. At-most-two-seat visual/audio/package evidence proves the shipped release or names blockers.
- Verification method: use @farm:{cargo test -p mde-musicd}
  @farm:{cargo test -p mde-media-core --features mpv}
  @farm:{cargo test -p mde-media-egui}
  and shell/RPM/live gates with BigBoy for the longest media job.
- Origin or merged source IDs: Spotify-class Music survey; archived WL-FUNC-007 and MEDIA-1..17; 2026-08-05/06 Music and Media evidence.
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
  integrity on no more than two physical seats plus lighthouses, and records
  provider/live/recovery evidence. Product epics depend on this epic only for
  shared rollout proof.
- Current state: Release preflight, signing/finalizer, topology identity,
  artifact binding, farm routing, and corrected-forward contracts exist with
  hostile evidence. The first release still lacks operator-supplied Maps, App
  VM, Cuttlefish, signing, bootc, and installed-provider inputs.
- **Clock release captures:** multi-process/UI/package/live Clock acceptance is
  shared rollout proof owned by `WL-TEST-001`; no extra seat requirement is
  imposed on the completed Clock implementation.
- **Health transition/live captures:** direct modal transition and installed-seat
  captures are shared rollout proof owned by `WL-TEST-001`; no additional seat
  requirement is imposed on product implementation.
- **Rich clipboard guest/provider proof:** implementation is archived after
  farm verification of Files/CAS guest-image admission (mackesd 1/1) and the
  live-vdi shell image boundary (2/2). Remaining Windows/guest/provider and
  installed-seat captures are shared rollout proof owned here; no extra seat
  requirement is imposed on the product implementation. See
  `docs/worklist-archive/2026-08-14-wl-func-016-closure.md`.
- **Maps/weather/navigation/MG90 proof:** implementation is archived after the
  complete Maps farm gate passed 324/324. Remaining live NWS/Maps/MG90/provider,
  package, and installed-seat captures are shared rollout proof owned here; no
  extra seat requirement is imposed on the product implementation. See
  `docs/worklist-archive/2026-08-14-wl-func-017-closure.md`.
- **Remote Sessions proof:** implementation is archived after catalog/action,
  Windows discovery, and media-equivocation gates passed. Remaining
  authenticated Windows login/render, publisher credential distribution, route
  captures, and live recovery are shared rollout proof owned here; no extra seat
  requirement is imposed on the product implementation. See
  `docs/worklist-archive/2026-08-14-wl-func-019-closure.md`.
- **Construct taskbar/Home proof:** implementation is archived after geometry,
  typed action, pin identity, responsive boundary, and wallpaper inode gates
  passed. Remaining direct-DRM/package/installed-seat captures are shared
  rollout proof owned here; no extra seat requirement is imposed on the product
  implementation. See `docs/worklist-archive/2026-08-14-wl-ux-012-closure.md`.
- **Browser VM quality proof:** implementation is archived after portable
  migration, host-boundary, image identity, reconnect, lifecycle, and runtime
  executable-path gates passed. Remaining guest image/audio quality, upgrade,
  performance, and installed-seat captures are shared rollout proof owned here;
  no extra seat requirement is imposed on the product implementation. See
  `docs/worklist-archive/2026-08-14-wl-arch-008-closure.md`.
- **Flatpak App-VM proof:** implementation is archived after signed catalog,
  App-VM image/profile, RPM supply, launch/readiness, lifecycle, cleanup, and
  capability gates passed. Remaining current-image, live-boot, package, and
  installed-seat captures are shared rollout proof owned here; no extra seat
  requirement is imposed on the product implementation. See
  `docs/worklist-archive/2026-08-14-wl-func-018-closure.md`.
- **Workload authority proof:** implementation is archived after Workload API,
  reconciler, storage/admission, Display1/VDI, capacity, cleanup, shell, and
  package boundaries passed. Remaining KMS/EGL, live capacity/first-frame,
  package-install, and installed-seat captures are shared rollout proof owned
  here; no extra seat requirement is imposed on the product implementation. See
  `docs/worklist-archive/2026-08-14-wl-arch-010-closure.md`.
- **Recovery/fleet proof:** implementation is archived after boot ordering,
  identity, Nebula/etcd/Syncthing rejoin, session restore, fleet retry,
  lighthouse scope, and corrected-forward recovery boundaries passed. Remaining
  privileged deployment, physical sleep/rejoin, inaccessible-voter rollout, and
  fleet-matrix captures are shared rollout proof owned here; no extra seat
  requirement is imposed on the product implementation. See
  `docs/worklist-archive/2026-08-14-wl-crit-007-closure.md`.
- **KIRON asset/live proof boundary:** UX-014 retains responsibility for the
  ToastHost, asset admission, timeline, fallback, and interaction
  implementation. Once a governed A–F scene/audio package exists, its signed
  package, installed-seat captures, and live renderer/audio evidence are shared
  rollout proof owned here; no additional seat requirement is imposed on UX-014.
- **Release-gate proof boundary:** CRIT-006’s remaining operator-supplied
  release execution is shared rollout proof owned here; it is not repeated as
  a product-epic implementation requirement.
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
  gates, governed payloads, and source identity on no more than two physical
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
  3. Baseline installed proof passes on no more than two physical test seats, with independent lighthouse quorum evidence and no multi-seat expansion requirement.
  4. Every deferred product/provider/recovery scenario has a dated evidence record or a precise external blocker; no product epic duplicates this rollout queue.
  5. Corrected-forward recovery succeeds without rollback, stale payload admission, privacy-epoch violation, or duplicate authority.
- Verification method: farm-only build/package/signing gates with the longest
  job on BigBoy; hostile release self-tests; package inspection; live proof on
  one or two named seats; lighthouse quorum and corrected-forward evidence.
- Origin or merged source IDs: CRIT-006/007 release boundary, operator
  two-seat lock, and deferred proof obligations previously repeated across
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
     - Deliverable: Workers device_inventory view and at-most-two-seat/three-lighthouse evidence.
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
- **Kiron asset inode (2026-08-14):** multiply-linked scenes cannot retain
  mutation authority across restart; `.196` passed the exact self-test:
  `evidence/WL-UX-014-2026-08-14-kiron-asset-inode-farm-gate-r1.md`.
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
       upgrade, and runtime on no more than two seats.
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
  3. Asset provenance, package, farm, and at-most-two-seat evidence is reproducible.
- Verification method: health/toast/asset/renderer/accessibility cargo gates,
  package/license checks, golden/video/waveform captures, and live proof on no
  more than two seats; BigBoy
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
