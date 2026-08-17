# Platform Worklist

This is the only active platform worklist. Design notes, evidence ledgers,
runbooks, and operator notes are inputs, not parallel trackers. Historical
implementation diaries remain in docs/worklist-archive/ and are not executable
tasks.

## Current Snapshot - 2026-08-16 production 13.0.0 execution

- **9 active epics:** 3 `Remaining`, 6 `Blocked`, 0 `Needs clarification`.
- **Latest stable integration:** 43 exact hostile gates passed across four farm hosts: `evidence/WORKLIST-2026-08-11-stable-exact-wave-r473.md`.
- **Execution order:** implement the turnkey lifecycle under `WL-FUNC-023`;
  create real release inputs under `WL-REL-006`; re-freeze the exact
  feature-complete `13.0.0` source under `WL-REL-001`; cut and sign the seven
  roles under `WL-REL-002`/`WL-REL-003`; stage the unpublished signed candidate
  on the production topology and run `WL-TEST-002`; complete the final signed
  evidence envelope under `WL-REL-004`; then publish and read back under
  `WL-REL-005`.
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
- **Production qualification topology (operator lock 2026-08-16):** deep
  acceptance for `13.0.0` is exactly Seat 15, Dell, and Surface. Eagle and T480
  are non-gating inspection/deployment-wave seats. Three lighthouses remain
  independently required. Surface is promoted into the `13.0.0` production
  support envelope; ARM64 remains outside that envelope.
  An unpublished signed candidate may be staged on this topology under the
  production-candidate qualification exception in `AI_GOVERNANCE.md`.
  Each target must receive the exact manifest-bound bytes, the red
  `AI-GENERATED-ALERT`, the five-second mutation delay, and an auditable result.
  Historical three- and five-seat evidence stays factual. Lighthouses are not
  test seats and retain their independently governed quorum proof.
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
  never run filler tests. The reproducible Android 14 AOSP/Cuttlefish source
  build is the sole `13.0.0` exception: run it on one temporary, dedicated
  KVM-XCP1 builder created only for that use case, export and independently
  verify its governed artifacts, destroy the builder, then restore the normal
  `.90` farm lane. BigBoy remains the long-pole host for every non-Android job.
- **Rollout lock:** for `13.0.0`, prove each release activity on exactly the
  three selected physical seats (Dell, Seat 15, Surface) and the independently
  required lighthouses. The approved
  promotion-forbidden preview may be distributed to all designated test seats
  and designated test lighthouses, but that wider distribution does not expand
  the test or acceptance requirement. Replace lighthouses one at a time while
  preserving quorum. Publish the red AI-GENERATED-ALERT and wait five seconds
  before each mutation. Recover failures by re-enrollment and corrected-forward
  deployment, never rollback.
- **Story format:** execute stories top-to-bottom. Do not start a story until
  every dependency is green. If a dependency or external resource is absent,
  set the epic to Blocked with the exact missing item; do not invent evidence.

## Active Drain Goal

Implement the unified turnkey seat lifecycle, then cut, self-sign, qualify,
publish, install, and prove production `magic-mesh-v13.0.0` from one exact clean
protected-default-branch revision. Produce all seven canonical roles, retain
fail-closed provenance, require real production evidence, and keep deep live
acceptance within Seat 15, Dell, and Surface.

## Service Release Queue

1. Implement the unified ONBOARD & OFFBOARDING lifecycle.
2. Create and admit real production release inputs.
3. Re-freeze the feature-complete `13.0.0` source on the protected default branch.
4. Build and self-sign all seven canonical roles.
5. Stage the exact unpublished candidate on the six-node production topology.
6. Run installed-seat, provider, direct-DRM, guest/device, and recovery acceptance.
7. Assemble and sign the final provenance/evidence bundle.
8. Publish `magic-mesh-v13.0.0`, verify readback, and complete the staged fleet handoff.

## Story execution contract

Every story below is a self-contained unit. The implementing agent must:
read the named inputs; change only the owned files; produce the named deliverable;
add the stated hostile or regression test; run the stated validation; record the
revision, command, result, and evidence path; and mark the story complete only
when the Done when condition is true. A passing compile without the named
behavioral evidence is not completion.

### WL-REL-007 - Execute the SOL Luna AI production 13.0.0 completion plan

- Status: Remaining
- Priority: P0
- Complexity: Epic
- Problem: the active lifecycle, release-input, source-freeze, build, signing,
  qualification, evidence, and publication epics form one production release
  dependency chain, but their cross-epic execution, farm ownership, temporary
  Android build capacity, and final reconciliation need one explicit governing
  plan so no blocker is skipped or satisfied with historical fixture evidence.
- Required outcome: SOL Luna AI coordinates the owning epics to produce,
  qualify, publish, read back, and archive production
  `magic-mesh-v13.0.0` from one exact clean protected-default-branch revision,
  with exactly seven canonical roles and no fabricated or substituted evidence.
- Current state: the eight owning epics below contain their product and release
  acceptance criteria. A historical promotion-forbidden preview and several
  source-bound receipts exist, but they predate the final lifecycle source and
  cannot satisfy this plan. Surface is approved for `13.0.0` qualification.
  Android 14 AOSP/Cuttlefish requires a one-use builder outside BigBoy's normal
  farm lane; all other long-pole builds remain assigned to BigBoy.
- Remaining work:
  1. S1 Establish SOL Luna execution ownership and release ordering.
     - Inputs: this worklist, governance locks, farm topology, the eight owning
       epics, and the clean integration branch.
     - Action: keep one Luna integration authority and use two to five workers
       only for disjoint lifecycle, surfaces, inputs, Android, and release
       scopes. Execute `WL-FUNC-023`, `WL-REL-006`, `WL-REL-001`,
       `WL-REL-002`, `WL-REL-003`, pre-publication `WL-TEST-002`,
       `WL-REL-004`, `WL-REL-005`, then final `WL-TEST-002` reconciliation.
     - Deliverable: one dependency/ownership record citing each owning epic and
       its current exact blocker; this epic must not duplicate their product
       implementation or acceptance evidence.
     - Validation: every worker has a disjoint write scope; every mutation and
       gate maps to one owning story; no parallel tracker or filler farm job is
       created.
     - Done when: every ready story has one owner and no downstream story starts
       before its dependencies are green.
  2. S2 Complete the unified lifecycle under WL-FUNC-023.
     - Inputs: `WL-FUNC-023` S1-S18 and its existing authority evidence.
     - Action: complete the typed lifecycle model, mackesd-only resumable
       authority, GUI/TUI parity, authorization, commissioning, artifact
       selection, audit/correction, onboarding, upgrade, warning handling,
       offboarding, reset, fleet execution, packaging, and first-boot behavior.
     - Deliverable: all S1-S18 deliverables and focused farm/live evidence owned
       by `WL-FUNC-023`.
     - Validation: hostile decode, replay, scope change, interruption, reboot,
       package, network, stale-generation, and renderer-parity checks pass.
     - Done when: `WL-FUNC-023` is archived with behavioral evidence and no
       lifecycle client retains an untyped or parallel mutation path.
  3. S3 Produce and admit all governed release inputs under WL-REL-006.
     - Inputs: final integration source candidate, open-source and license
       policies, Maps and image receipt contracts, Kiron assets, and the
       authorized release-key identity.
     - Action: produce the OpenStreetMap-derived Buffalo-Niagara Maps bundle
       clipped to official Erie and Niagara county boundaries using the existing
       Maps approval, producer, materializer, and verifier contracts; enforce
       the aggregate quota and deterministic transport; regenerate App VM,
       bootc, and Kiron receipts; build matching Android 14 x86_64 Cuttlefish
       host/image bytes and current-source guest DEBs; generate the existing
       signer receipt; and materialize canonical private mode-0400 preflight
       inputs outside Git. Do not add a platform-wide trust root, new security
       subsystem, or unrelated runtime security surface.
     - Deliverable: immutable, licensed, current-revision receipts, declarations,
       approvals, signatures, manifests, private argv inputs, and redacted
       inventory required by canonical release preflight.
     - Validation: wrong bytes, provider, license, architecture, role, revision,
       epoch, permissions, links, quota, or path substitution refuse through the
       existing governed verifiers; no new security contract is required.
     - Done when: `release-input-preflight.sh` accepts every real production
       input and no preview fixture or historical receipt is admitted.
  4. S4 Build Android 14 on a one-use KVM-XCP1 host.
     - Inputs: the locked Android 14 manifest, KVM-XCP1 capacity, OpenTofu/XAPI
       authority, and immutable private artifact storage.
     - Action: drain and stop the normal `.90` VM; create
       `mcnf-build-android14` on KVM-XCP1 with 4 vCPU, 18 GiB RAM, a 32 GiB root,
       a dedicated 400 GiB build volume, and bounded 32 GiB swap. Build only the
       matching `aosp_cf_x86_64_phone-userdebug` image and host package in a
       digest-pinned environment. If the local SR cannot admit the volume, add a
       dedicated 400 GiB build SR before starting rather than consuming another
       farm VM's disk.
     - Deliverable: matching `cvd-host_package.tar.gz`, image archive, build
       fingerprint, compatibility identity, image receipt, guest declaration,
       license inventory, capacity record, and OpenTofu plan/apply record.
     - Validation: boot the resulting Cuttlefish instance; prove `cvd`, Android
       boot completion, `adb`, guest package installation, readiness/VDI,
       input, reconnect, and clean shutdown. Export immutable outputs and verify
       their hashes from a separate farm host.
     - Done when: verified outputs are outside the temporary VM, the VM/build
       volume/swap are destroyed, the destroy record is preserved, and the
       restored `.90` lane passes normal farm admission. The temporary builder
       never executes unrelated work or becomes a permanent farm lane.
  5. S5 Freeze source and cut the seven-role candidate.
     - Inputs: completed lifecycle and input epics, protected `master`, required
       GitHub checks, authorized signing material, and Fedora 44 builders.
     - Action: freeze one clean pushed revision and epoch; regenerate all
       source-bound inputs for it; build exactly three unsigned RPMs; seal the
       handoff; sign all three atomically without payload drift; build Browser
       VM and App VM derivatives; and admit Cuttlefish and bootc.
     - Deliverable: one immutable private candidate containing exactly
       Workstation RPM, Server RPM, Lighthouse RPM, Browser VM, App VM,
       Cuttlefish image, and bootc image.
     - Validation: BigBoy runs the non-Android long poles; every permanent farm
       host runs a unique meaningful build or gate; handoff, signature, NEVRA,
       payload, receipt, manifest, collector, and seven-role hostile checks pass.
     - Done when: `WL-REL-001`, `WL-REL-002`, and `WL-REL-003` are complete and
       the unpublished signed candidate binds exactly to the frozen source.
  6. S6 Qualify the unpublished candidate on production topology.
     - Inputs: the signed seven-role candidate, Dell, Seat 15, Surface, three
       lighthouses, provider authority, and corrected-forward recovery identity.
     - Action: inspect all designated seats, then perform deep acceptance on
       exactly Dell, Seat 15, and Surface. Publish the red
       `AI-GENERATED-ALERT` and wait five seconds before every mutation. Upgrade
       lighthouses one at a time while preserving quorum. Eagle and T480 remain
       non-gating inspection/deployment-wave seats.
     - Deliverable: exact installed identity, lifecycle, provider, direct-DRM,
       Maps, collaboration, media/device, guest-role, Surface-hardware,
       resilience, privacy-retention, lighthouse, and recovery evidence owned
       by `WL-TEST-002`.
     - Validation: tested bytes match the candidate; unavailable capabilities
       remain honest; every failure recovers by corrected-forward action or
       re-enrollment, never rollback.
     - Done when: `WL-TEST-002` S1-S7 pass or reopen one exact owning
       implementation blocker with no invented success.
  7. S7 Assemble, sign, publish, and independently read back the release.
     - Inputs: qualified seven-role candidate, gate matrix, SBOM producers,
       release key, release notes, GitHub authority, and package repository.
     - Action: collect exactly seven roles; run all mandatory farm and GitHub
       gates; generate SBOM, compatibility, provenance, checksums, and evidence;
       sign the complete envelope; create signed tag
       `magic-mesh-v13.0.0`; publish the exact asset set; download it into a new
       directory; and atomically promote signed repository metadata only after
       clean-room verification.
     - Deliverable: immutable tag and release, signed seven-role evidence
       bundle, public asset/readback receipt, and signed package-channel receipt.
     - Validation: omitted, extra, changed, stale, linked, unsigned, HOLD, or
       cross-revision files refuse; downloaded bytes reproduce the qualified
       artifact identities and all three RPM roles resolve from the channel.
     - Done when: `WL-REL-004` and `WL-REL-005` are archived and public readback
       agrees exactly with the frozen source and installed candidate.
  8. S8 Reconcile and archive the complete plan.
     - Inputs: every owning epic's evidence, public readback, installed
       acceptance, worklist stewardship rules, and archive dispositions.
     - Action: complete `WL-TEST-002` S8; map every obligation to evidence, a
       reopened implementation story, or one exact external-authority blocker;
       archive every completed owning epic and finally this coordination epic.
     - Deliverable: final signed acceptance index, release disposition, blocker
       inventory, and archive entries.
     - Validation: worklist self-test and lint pass; snapshot counts match; no
       deferred obligation, private secret, temporary Android resource,
       abandoned worktree, or parallel tracker remains.
     - Done when: production `13.0.0` is published and independently verified,
       all completed epics are removed from the active worklist, and any genuine
       external blocker names one concrete operator action.
- Scope: coordination and dependency enforcement for the existing lifecycle and
  release epics, plus the narrowly authorized temporary Android build capacity.
  Existing epics remain the sole owners of implementation and acceptance work.
- Relevant files/components: `docs/platform/WORKLIST.md`, release/farm helpers,
  OpenTofu farm declarations, lifecycle components, release input producers,
  packaging, evidence collectors, and publication verifiers.
- Dependencies: the eight owning epics, authorized signing/publication/provider
  access, KVM-XCP1 capacity, Dell/Seat 15/Surface access, and three lighthouse
  targets. Story-level dependencies are enforced in S1-S8 order.
- Acceptance criteria: one clean source produces exactly seven signed roles;
  real governed inputs pass preflight; the one-use Android builder leaves no
  residual resource; production topology passes; signed evidence and public
  readback agree; no fixture, rollback, filler build, or fabricated proof
  satisfies a gate.
- Verification method: worklist lint, focused hostile tests, meaningful gates
  across all permanent farm hosts, Android live boot proof, exact three-seat and
  three-lighthouse acceptance, signed evidence verification, clean-room public
  readback, and repository query.
- Origin or merged source IDs: operator SOL Luna AI completion plan and
  Android-only KVM-XCP1 build-host exception (2026-08-16).

### WL-FUNC-023 - Create the unified ONBOARD & OFFBOARDING lifecycle

- Status: Remaining
- Priority: P0
- Complexity: Epic
- Problem: setup, enrollment, upgrade, repair, reset, and offboarding are
  fragmented and can leave seats partially active. Seat 15 exposed missing
  identity, etcd, credential, compute, and grouped-service prerequisites.
- Required outcome: create one local-first ONBOARD & OFFBOARDING interface backed
  by one resumable mackesd authority for local or fleet onboarding, upgrade,
  verification/correction, offboarding, reset, and recommissioning.
- Current state: the resumable authority and typed contracts cover locking,
  readiness, artifact/capsule admission, destructive confirmation, and terminal
  evidence. Live leave, decommission, role provisioning, service-add,
  first-desktop, spawn-lighthouse, mesh-dns/network/create, invite/join/found,
  and coordinated-upgrade mutations now acquire that authority. Upgrade
  execution requires a typed artifact selection or compatibility digest; downstream package, service, enrollment, renderer, fleet-offboarding, and full execution evidence remain.
  Evidence: `evidence/WL-FUNC-023-2026-08-16-lifecycle-authority-farm-r2.md`,
  `evidence/WL-FUNC-023-2026-08-16-remote-push-farm-r1.md`; focused authority
  core
  passes; live bootstrap still passes a `{{JOIN_TOKEN}}` command
  template where `RunEnroll` requires the minted bearer, so token minting and
  bearer handoff precede SSH wiring.
- Remaining work: GPT Luna execution contract: execute S1-S18 in order; read each story first; change only owned components; record the
  deliverable, farm command, result, revision, and evidence; do not close a story
  from compilation alone.
  1. S1 Define the canonical lifecycle and readiness model.
     - Inputs: governance locks, health contracts, role provisioning, packaging,
       Seat 15 findings, and Surface acceptance contracts.
     - Action: define Onboard, Upgrade, VerifyAndCorrect, Offboard, and
       ResetAndOnboard with resumable intermediate and terminal states.
     - Deliverable: one role/hardware baseline and lifecycle state model consumed
       by provisioning, packaging, auditing, recovery, and qualification.
     - Validation: stale singular mackesd assumptions refuse; role pinning and
       target activity cannot imply readiness.
     - Done when: every applicable package, unit, configuration, mesh, compute,
       UI, and hardware requirement has one owning baseline entry.
  2. S2 Add the typed public contracts.
     - Inputs: mackes-mesh-types, mde-bus conventions, health contracts, and job
       and report schemas.
     - Action: add `OnboardOffboardSessionV1`, `LifecycleIntentV1`,
       `LifecyclePlanV1`, `LifecycleProgressV1`, `SeatReadinessV1`,
       `OffboardingReceiptV1`, and `FleetLifecycleReportV1`.
     - Deliverable: bounded versioned request, plan, progress, state, warning,
       report, and signature contracts.
     - Validation: hostile decode, version, size, target-binding, transition, and
       redaction tests.
     - Done when: no GUI, TUI, CLI, local, or fleet client needs an untyped
       mutation path.
  3. S3 Implement the mackesd lifecycle authority.
     - Inputs: typed contracts, mackesd workers, mde-bus, systemd units, and
       corrected-forward recovery rules.
     - Action: implement a mackesd mode or one-shot service with one lifecycle
       lock, atomic checkpoints, idempotent steps, and resume.
     - Deliverable: local lifecycle authority available before mesh identity and
       complete grouped-service activation.
     - Validation: process, power, network, package, and reboot interruption tests.
     - Done when: no renderer, CLI, or parallel daemon owns lifecycle mutation.
  4. S4 Build the single ONBOARD & OFFBOARDING interface.
     - Inputs: Construct navigation, `magic-setup`, and existing Setup, System,
       Mesh Health, Upgrade, Recovery, and Reset routes.
     - Action: create one local-seat landing view with fleet switching and five
       lifecycle intents; redirect all legacy lifecycle entrypoints into it.
     - Deliverable: equivalent GUI and TUI renderers over one session contract.
     - Validation: identical requests produce identical plans and state.
     - Done when: legacy routes contain no independent lifecycle business logic.
  5. S5 Implement authorization and confirmation.
     - Inputs: mesh trust, local administrator authentication, signed job bundles,
       and mutation-alert governance.
     - Action: allow any trusted node to initiate work; require authority-signed
       destructive authorization and one fleet-level typed phrase.
     - Deliverable: `WIPE <COUNT> SYSTEMS`, `FORCE OFFBOARD <COUNT> SYSTEMS`, and
       per-seat red alert/five-second enforcement.
     - Validation: wrong count, stale authorization, changed scope, replay, and
       unauthorized destructive requests refuse.
     - Done when: destructive target scope cannot change after confirmation.
  6. S6 Implement capsule and token commissioning.
     - Inputs: join-token flow, identity receipts, etcd endpoints, publisher
       credentials, installer handoff, USB, and NoCloud paths.
     - Action: add target-bound `CommissioningCapsuleV1` and QR/token exchange.
     - Deliverable: zero-touch capsule and one-interaction token paths with
       encrypted retryable staging.
     - Validation: expiration, replay, revocation, target mismatch, conflict,
       disconnect, and redaction tests.
     - Done when: bootstrap material is erased only after confirmed enrollment.
  7. S7 Implement operator-controlled artifact selection.
     - Inputs: release catalog, RPM/image inputs, local artifact import, release
       signatures, architecture, and migration metadata.
     - Action: allow signed Stable, Candidate, or Dev selection or another
       supplied artifact; pin exact bytes before planning.
     - Deliverable: artifact browser/import flow with digest and qualification.
     - Validation: changed bytes, mutable references, wrong architecture, and
       unsupported package shape refuse.
     - Done when: the engine never silently substitutes another artifact.
  8. S8 Support confirmed unsigned artifacts.
     - Inputs: selected digest, administrator identity, typed confirmation, and
       readiness reports.
     - Action: require `INSTALL UNSIGNED <SHORT-DIGEST>` and record the digest,
       operator, warning, and confirmation.
     - Deliverable: visible `UnverifiedBuild` state without mesh quarantine.
     - Validation: confirmation cannot authorize different bytes or targets.
     - Done when: a core-health-passing unsigned build may participate normally
       while remaining visibly unverified.
  9. S9 Implement complete audit and discovery.
     - Inputs: canonical baseline, inventory, mesh, identity, etcd, collaboration,
       publishing, compute, storage, UI, and hardware providers.
     - Action: compare observed state with every applicable baseline entry.
     - Deliverable: stable checks with expected/observed state, evidence,
       severity, correction, and result.
     - Validation: planted missing inputs and inactive service groups cannot
       produce Ready.
     - Done when: the full Seat 15 failure pattern is identified in one audit.
  10. S10 Implement planning and VerifyAndCorrect.
      - Inputs: audit result, selected baseline and artifact, repair providers,
        and prerequisite relationships.
      - Action: generate an immutable dependency DAG, review it, and apply
        corrected-forward repairs with bounded retries.
      - Deliverable: resumable audit-plan-correct-reboot-verify workflow.
      - Validation: reordered prerequisites, partial failure, repeated request,
        and restart tests remain idempotent.
      - Done when: unresolved core failures are Blocked with one exact action.
  11. S11 Make onboarding turnkey.
      - Inputs: capsule/token, artifact, identity and authority inputs, packages,
        systemd, mesh, and shell readiness.
      - Action: stage, install, configure, enroll, activate, reboot when needed,
        resume, and verify automatically.
      - Deliverable: zero-touch capsule and one-interaction token onboarding.
      - Validation: clean RPM, bootc, Kickstart/NoCloud, and USB fixture tests.
      - Done when: no manual package, configuration, or systemctl work remains.
  12. S12 Make upgrades turnkey.
      - Inputs: current state and workloads, selected artifact, migrations,
        authority inputs, power, disk, and network state.
      - Action: preflight, preserve valid state, stage replacements, migrate,
        defer restart, converge, resume after reboot, and verify.
      - Deliverable: supported upgrade path with a pending-convergence marker.
      - Validation: supported prior schemas, active workloads, stale units,
        absent inputs, interruption, and resource-pressure tests.
      - Done when: upgrade neither deletes valid state nor needs manual repair.
  13. S13 Implement warning-level capability handling.
      - Inputs: Surface overlay, hardware probes, virtualization checks,
        scheduler capabilities, and bounded retry policy.
      - Action: attempt correction, then classify remaining hardware or
        virtualization failures as `ReadyWithWarnings`.
      - Deliverable: prominent warnings and truthful capability withdrawal.
      - Validation: failed KVM or Surface features remain visible and cannot
        receive incompatible workloads.
      - Done when: warning seats remain usable without claiming failed features.
  14. S14 Implement complete Offboard.
      - Inputs: authority job, inventory, workloads, mesh membership,
        credentials, disk inventory, and replacement capacity.
      - Action: persist, cordon, drain, verify placement, revoke, remove
        membership, and erase the entire system.
      - Deliverable: authority-signed `OffboardingReceiptV1`.
      - Validation: drain failure blocks; force needs new authorization and its
        phrase; offline wipe needs prior durable acceptance.
      - Done when: no reusable identity, workload, mesh expectation, build,
        configuration, credential, or local data remains.
  15. S15 Implement ResetAndOnboard.
      - Inputs: full-wipe authorization, replacement artifact, old identity,
        new capsule/token, and Offboard implementation.
      - Action: revoke the old identity, erase, reinstall, issue a new identity,
        and run ordinary onboarding.
      - Deliverable: one resumable clean-recommission workflow.
      - Validation: old and replacement identities cannot coexist.
      - Done when: Dell or another target can preserve nothing and return new.
  16. S16 Implement fleet execution and coordinator handoff.
      - Inputs: authority inventory, mesh RPC, SSH fallback, persistent jobs,
        seat-wave limits, and lifecycle engine.
      - Action: show all known systems, audit/stage concurrently, mutate in
        bounded waves, and transfer coordination before changing the initiator.
      - Deliverable: persistent fleet session with per-target transport,
        checkpoint, terminal state, and signed aggregate report.
      - Validation: offline target, failover, handoff, reconnect, and mixed-state
        tests.
      - Done when: coordinator reboot, wipe, or disconnect cannot lose the job.
  17. S17 Reconcile package and first-boot behavior.
      - Inputs: `WIZARD_SERVICES`, `ROLE_UNITS`, `meshctl doctor`, RPM scripts,
        systemd units, Kickstart, and bootc first boot.
      - Action: consume the canonical baseline, retain failed enrollment tokens,
        queue convergence, and remove ignored critical activation failures.
      - Deliverable: consistent package, installer, role, doctor, and lifecycle
        behavior.
      - Validation: source scans and hostile package/first-boot fixtures.
      - Done when: no shipped path uses stale units or weak readiness proxies.
  18. S18 Prove and hand off the implementation.
      - Inputs: S1-S17, farm inventory, the three-seat `13.0.0` acceptance lock,
        preview-distribution exception, retention lock, and the
        WL-TEST-002 ownership boundary.
      - Action: run focused unit/integration/hostile farm gates, put the longest
        job on BigBoy, and record product-specific evidence.
      - Deliverable: evidence index, AI/operator runbook, migration notes, and
        exact deferred WL-TEST-002 obligations.
      - Validation: worklist lints pass; detailed history expires within six
        hours; live proof uses exactly Dell, Seat 15, and Surface; any broader preview
        distribution remains manifest-bound and is not counted as proof.
      - Done when: implementation gates pass and exact installed-release/live
        acceptance remains only under WL-TEST-002.
- Scope: unified local/fleet lifecycle implementation, GUI/TUI rendering,
  onboarding, upgrade, correction, offboarding, erasure, recommissioning,
  Surface and virtualization checks, artifact selection, package integration,
  and focused product verification. Release publication, wider deployment, and
  exact installed/live acceptance remain in the release chain and WL-TEST-002.
- Relevant files/components: `crates/mesh/mackesd/`,
  `crates/mesh/mde-enroll/`, Construct lifecycle routes, shared mesh types,
  packaging, systemd, Kickstart/bootc, and focused verification helpers.
- Dependencies: mackesd remains the only daemon authority; mde-bus remains the
  only platform bus; typed Workload operations remain the only VM/container
  lifecycle API; WL-TEST-002 owns exact installed and live acceptance.
- Acceptance criteria: ONBOARD & OFFBOARDING is the only human lifecycle
  interface; all renderers share one engine; capsule onboarding is zero-touch;
  token onboarding needs one interaction; upgrades need no manual repair;
  destructive work is authority-bound; Offboard drains and erases completely;
  ResetAndOnboard cannot retain an old identity; unsigned artifacts require
  digest confirmation; core failures block; capability failures remain
  prominent `ReadyWithWarnings`.
- Verification method: focused hostile/unit tests, farm integration and package
  fixtures, GUI/TUI parity, interruption/resume proof, and exactly three physical
  acceptance seats; defer exact release/rollout proof to WL-TEST-002.
- Origin or merged source IDs: operator lifecycle consolidation, Seat 15 and Surface findings, clean-fleet survey, and GPT Luna assignment (2026-08-15).

### WL-REL-001 - Freeze the newest feature-complete release source

- Status: Blocked
- Priority: P0
- Complexity: Epic
- Problem: production `13.0.0` is newer than the latest published tag, and loose historical artifacts do not define one admissible release source.
- Required outcome: freeze one clean, pushed, feature-complete `13.0.0` commit
  on the protected default branch and bind every release input, version surface,
  note, and tag plan to it.
- Current state: revision 1dfe6906609d71da9ee2ce20c860912a09b32855 and epoch
  1786813297 remain recorded in the r2 source-freeze receipt as the clean
  pre-WL-FUNC-023 candidate. It cannot be the final feature-complete release
  source because WL-FUNC-023 must land first. Browser helpers and the shipped
  role chooser resolve to 13.0.0, and the five internal non-release crates are
  documented in docs/RELEASE-VERSIONING.md. S2 farm metadata evidence is in
  `evidence/WL-REL-001-2026-08-16-version-metadata-farm-r1.md`. Re-run S1-S4
  after WL-FUNC-023 and WL-REL-006 are complete.
- Remaining work:
  1. S1 Select the immutable source. BLOCKED: the recorded 1dfe6906 candidate
     predates required WL-FUNC-023 implementation and must be replaced after
     WL-FUNC-023 and WL-REL-006 complete.
     - Inputs: pushed branch, root Cargo.toml, remote branch state, and archived implementation dispositions.
     - Action: fetch remote refs; require an empty worktree; record HEAD, upstream HEAD, commit epoch, Fedora target, and version.
     - Deliverable: docs/platform/evidence/WL-REL-001-source-freeze-r1.md with exact commands and outputs.
     - Validation: source-revision-receipt.sh --repo .; git diff --quiet; git diff --cached --quiet; compare HEAD with upstream.
     - Done when: one non-null 40-character revision and positive epoch identify the clean pushed source.
  2. S2 Verify every version surface. Complete: the three isolated browser
     helper manifests/lockfiles and shipped role chooser resolve to 13.0.0;
     the five non-shipped crates are recorded as packaging/test boundaries in
     docs/RELEASE-VERSIONING.md.
     - Inputs: docs/RELEASE-VERSIONING.md, root and isolated Cargo workspaces, package recipes, CLI/About build identity.
     - Action: run Cargo metadata; compare shipped package versions; scan runtime sources for competing numeric release authorities.
     - Deliverable: bounded version matrix naming each shipped surface, source, observed value, and exception.
     - Validation: farm metadata/package checks on .50; no runtime version authority other than workspace/package reflection.
     - Done when: every current release surface resolves to 13.0.0 or a documented packaging release suffix.
  3. S3 Admit all governed release inputs. BLOCKED: the release-input loader
     and derived-driver converter exist, but no final private preflight object
     exists; the RPM signer receipt has been generated and inspected
     privately for the superseded f095b8ce revision; it must be regenerated for
     the frozen 1dfe6906609d71da9ee2ce20c860912a09b32855 revision at epoch
     1786813297. Maps approval/source, App VM image/catalog receipt,
     Cuttlefish declarations/packages/image receipt, and bootc receipt are
     not admitted for the frozen revision. Maps provider/live proof
     is explicitly deferred to WL-TEST-002; that deferral does not create a
     release-input approval. Do not run a build with historical loose artifacts.
     - Inputs: Maps approval/source, App VM image/catalog receipt, Cuttlefish declarations/packages/image receipt, RPM signer receipt, bootc receipt.
     - Action: create one private strict-object JSON document containing every
       mandatory release-input-preflight argument, then derive the separate
       prepare-driver argv array with `release-input-argv.py
       --emit-driver-arguments`; never hand-edit duplicate argument lists.
     - Deliverable: immutable mode-0400 object and derived driver argument file
       plus redacted input inventory; never commit credentials or private keys.
     - Validation: release-input-preflight.sh against the frozen revision and epoch; missing/substituted input fixture must refuse.
     - Done when: preflight succeeds before any build mutation and every accepted input identifies the frozen revision.
  4. S4 Freeze release notes and tag plan.
     - Inputs: commits since magic-mesh-v12.1.1, archived epic dispositions, current worklist, and user-visible feature set.
     - Action: draft release notes with features, compatibility, known limitations, upgrade path, and corrected-forward recovery.
     - Deliverable: versioned release-note source and exact tag name magic-mesh-v13.0.0.
     - Validation: notes contain no unsupported production/security claim and identify deferred provider/live proof honestly.
     - Done when: notes, tag, source receipt, and input inventory agree on version and revision.
- Scope: source identity, version authority, mandatory input admission, release notes, and tag planning only; no artifact build or publication.
- Relevant files/components: Cargo.toml, Cargo.lock, isolated Cargo workspaces, docs/RELEASE-VERSIONING.md,
  install-helpers/source-revision-receipt.sh, install-helpers/release-input-preflight.sh.
- Dependencies: WL-FUNC-023 and WL-REL-006 must complete; operator self-sign
  authorization is recorded; exact installed/live proof remains deferred to
  WL-TEST-002.
- Acceptance criteria: one clean pushed revision is frozen; all version surfaces and inputs bind to it; stale artifacts cannot enter later stages.
- Verification method: local read-only Git/version checks, focused farm metadata/package checks, preflight admission, and evidence review.
- Origin or merged source IDs: release recovery of archived WL-BUILD-001, WL-BUILD-003, and WL-CRIT-006 responsibilities.

### WL-REL-006 - Create governed open-source release inputs

- Status: Remaining
- Priority: P0
- Complexity: Epic
- Problem: WL-REL-001 cannot admit the production release while Maps, App VM,
  Cuttlefish, bootc, UX-014 assets, and the private preflight argv exist only as
  missing operator inputs or non-production fixtures.
- Required outcome: create or select real open-source-compatible production
  inputs, bind every byte and license to the frozen source revision, and produce
  the exact non-secret receipts required by the canonical preflight. Fixtures
  may exercise contracts but cannot satisfy a production gate.
- Current state: receipt and local-generation paths exist, but no complete
  current-revision production input set is admitted. App VM S3 has a live
  Fedora receipt and Kiron S6 has passed source-bound package admission.
  Evidence: `docs/platform/evidence/WL-REL-006-2026-08-16-app-vm-receipt-r1.md`,
  `docs/platform/evidence/WL-REL-006-2026-08-16-kiron-assets-r1.md`.
  The RPM lane still requires one mode-0400 private JSON input bound to the
  clean checkout; Maps contract gates are green, and Cuttlefish guest DEBs are
  source-bound and verified. Approved provider bytes, a compatible pinned host
  tools revision, matching Android CI artifacts, and the declaration remain
  outstanding.
  Downstream release epics remain blocked until current `13.0.0` receipts pass;
  the historical preview fixture cannot be reused.
- Remaining work:
  1. S1 Establish the open-source input policy.
     - Inputs: frozen source receipt, Fedora target, architecture, applicable
       licenses, and the existing receipt/verifier contracts.
     - Action: choose reproducible open-source sources or local build recipes
       for each role; record upstream project, license, version, digest method,
       and whether credentials or operator authorization are required.
     - Deliverable: redacted open-source input inventory and license manifest.
      - Validation: every source is redistributable or explicitly operator-gated;
       any fixture substitution follows the governed evidence template and is
       not presented as observed production behavior.
     - Done when: all six input families have a named reproducible source or an
       exact external-provider blocker.
  2. S2 Produce the Maps input.
     - Inputs: open map data/provider, immutable tile or catalog source,
       approved offline-cache policy, frozen source receipt, and license terms.
     - Action: materialize a bounded local Maps catalog/tile set and generate a
       current-revision approval receipt; preserve provider attribution and
       defer live provider proof to WL-TEST-002.
     - Deliverable: immutable Maps source manifest, hashes, attribution, and
       approval receipt.
     - Validation: Maps verifier rejects changed bytes, wrong revision,
       unapproved provider, quota violation, and path substitution.
     - Done when: preflight admits the Maps input without claiming live service.
  3. S3 Produce the App VM input.
     - Inputs: reproducible open-source base image or authorized registry
       manifest, architecture, catalog metadata, and frozen source receipt.
     - Action: build or inspect the base image without pulling unpinned layers;
       produce the canonical App VM base-image receipt and catalog content record.
     - Deliverable: immutable App VM digest, receipt, compatibility metadata, and
       license record.
     - Validation: App VM producer/inspector and build-image admission pass;
       registry or local bytes are bound to the frozen revision.
     - Done when: App VM inputs pass preflight before image-context mutation.
  4. S4 Produce the Cuttlefish input.
     - Inputs: open-source Cuttlefish image or authorized artifact, Android
       release/compatibility identity, architecture, guest package sources,
       and frozen source receipt.
     - Action: build or inspect the image, generate the immutable image receipt,
       and create the guest declaration over exact package bytes.
     - Deliverable: Cuttlefish image receipt, declaration, package manifest,
       hashes, and license record.
     - Validation: guest payload, declaration, image, and preflight verifiers
       reject substitution, wrong provider, architecture, release, or revision.
     - Done when: the Cuttlefish role is admissible without undocumented bytes.
  5. S5 Produce the bootc input.
     - Inputs: reproducible open-source bootc base or authorized registry
       manifest, architecture, expected role, and frozen source receipt.
     - Action: inspect exact manifest bytes and produce the canonical bootc
       digest receipt; integrate receipt consumption into release preflight.
     - Deliverable: immutable bootc receipt and preflight integration evidence.
     - Validation: architecture, role, digest, revision, epoch, and media type
       are all fail-closed; unavailable registry access refuses admission.
     - Done when: preflight consumes the receipt rather than a raw digest.
  6. S6 Create UX-014 release assets.
     - Inputs: existing open-source UI assets, Kiron verifier contract, license
       attribution, frozen source receipt, and required asset dimensions.
     - Action: create the A-F package assets and their manifest using the
       governed asset format; do not claim live hardware proof from screenshots.
     - Deliverable: asset package, manifest, hashes, attribution, and verifier
       evidence.
     - Validation: Kiron verifier accepts the complete set and rejects missing,
       substituted, stale, or unlicensed assets.
     - Done when: WL-REL-003/004 can consume the exact asset manifest.
  7. S7 Materialize private first-release preflight argv.
     - Inputs: all current-revision receipts from S2-S6, RPM signer receipt,
       private paths, target architecture, and release epoch.
     - Action: write one mode-0400 private JSON object outside Git, derive the
       release-driver array from that object, and run release-input-preflight
       before any build mutation.
     - Deliverable: private object path, derived driver-array path, redacted
       input inventory, and preflight transcript.
      - Validation: missing, changed, symlinked, stale, or cross-revision inputs
       refuse; fixture substitutions require the governed evidence record; no
       credentials or private keys enter Git/logs.
     - Done when: WL-REL-001 S3 is green and downstream release work may start.
- Scope: open-source source selection, reproducible input generation, receipts,
  licenses, and preflight admission; no public release or live-seat testing.
- Relevant files/components: install-helpers/release-input-preflight.sh,
  packaging/app-vm, packaging/android, install-helpers/produce-bootc-digest-receipt.py,
  Maps catalog/verifier tools, and the Kiron asset verifier.
- Dependencies: WL-REL-001 S1/S2; external registry/provider access only where
  no reproducible local open-source source is available.
- Acceptance criteria: every mandatory first-release input is reproducible,
  licensed, immutable, current-revision-bound, and admitted by preflight, or is
  recorded as one exact external blocker.
- Verification method: farm-only source/image/package gates, receipt inspectors,
  hostile substitution tests, license review, and canonical preflight.
- Origin or merged source IDs: WL-CRIT-006, WL-FUNC-017, WL-FUNC-018,
  WL-FUNC-020, and the deferred WL-TEST-002 provider-proof queue.

### WL-REL-002 - Cut the complete three-RPM unsigned handoff

- Status: Blocked
- Priority: P0
- Complexity: Epic
- Problem: the release needs same-revision Workstation, Server, and Lighthouse RPMs; the loose artifact store has no admissible complete set.
- Required outcome: build exactly three Fedora 44 RPM roles from the WL-REL-001 source and publish one immutable private production-candidate handoff.
- Current state: hostile prepare-path evidence is in
  `evidence/WL-REL-002-2026-08-16-hostile-boundary-r1.md` and
  `evidence/WL-REL-002-2026-08-16-private-object-driver-r1.md`; the driver
  accepts the strict private object directly, but WL-REL-001 is blocked and
  no current-revision three-RPM handoff exists.
- Remaining work:
  1. S1 Reconfirm the frozen source immediately before build.
     - Inputs: WL-REL-001 source receipt, epoch, preflight argv, clean checkout, and farm topology.
     - Action: verify source receipt again; verify preflight again; reserve BigBoy for full RPM and a distinct farm slot for Server RPM.
     - Deliverable: build invocation record with host, slot, revision, epoch, target, and output parent.
     - Validation: run-first-full-release.sh must refuse dirty, moving, cross-epoch, or non-Fedora-44 input.
     - Done when: both build lanes are pinned before either artifact is admitted.
  2. S2 Build Workstation and Lighthouse RPMs.
     - Inputs: frozen source and admitted inputs.
     - Action: run the full Fedora 44 RPM lane on BigBoy through run-first-full-release.sh prepare.
     - Deliverable: exactly one magic-mesh RPM and one magic-mesh-lighthouse RPM in the private pull directory.
     - Validation: farm command succeeds; rpm -qp reports expected names, version 13.0.0, architecture, and SHA-256 payload digest.
     - Done when: no duplicate, stale, symlinked, mutable, or unexpected full-lane RPM remains in the candidate set.
  3. S3 Build the Server RPM.
     - Inputs: same frozen source and admitted inputs.
     - Action: run the independent Fedora 44 Server RPM lane and pull exactly one magic-mesh-server candidate.
     - Deliverable: one Server RPM from the same revision, version, target, and build policy.
     - Validation: role/name/NEVRA/payload identity and embedded build identity checks pass.
     - Done when: the Server candidate is exact and cannot be confused with Workstation or Lighthouse.
  4. S4 Seal and verify the handoff.
     - Inputs: three unsigned RPM candidates.
     - Action: let run-first-full-release.sh create handoff.json and atomically publish its read-only handoff directory.
     - Deliverable: workstation-unsigned.rpm, server-unsigned.rpm, lighthouse-unsigned.rpm, and handoff.json.
     - Validation: rerun the handoff parser; independently hash files; mutate a private copied fixture and confirm refusal.
     - Done when: all three roles, hashes, sizes, NEVRAs, payload digests, revision, epoch, and Fedora target agree.
- Scope: unsigned RPM construction and immutable handoff only; no signing, derivative images, promotion, or live installation.
- Relevant files/components: install-helpers/run-first-full-release.sh, install-helpers/xcp-build.sh,
  packaging/app-vm, packaging/server-rpm, packaging/browser-vm.
- Dependencies: WL-REL-001.
- Acceptance criteria: one immutable three-role handoff exists; every RPM is exact and same-source; partial or substituted sets refuse.
- Verification method: BigBoy full RPM lane, independent Server farm lane, RPM/build-identity checks, and handoff hostile verification.
- Origin or merged source IDs: archived WL-BUILD-001 and first-release preparation from WL-CRIT-006.

### WL-REL-003 - Self-sign RPMs and produce all derivative release roles

- Status: Blocked
- Priority: P0
- Complexity: Epic
- Problem: a complete release requires three signed RPM roles and four verified image roles; no current-revision seven-role set exists.
- Required outcome: self-sign the exact handoff RPMs without changing payload identity and produce Browser VM, App VM, Cuttlefish, and bootc roles.
- Current state: a private, promotion-forbidden seven-role preview exists
  for `afc24782ca9dc8e2e87f5676e403428a82285da1`, with all three signed RPMs,
  Browser VM, App VM, Cuttlefish, and bootc receipt identities collected and
  re-verified. It is not the final release because WL-REL-001 remains blocked
  on the feature-complete source freeze; durable evidence is recorded in
  `docs/platform/evidence/WL-REL-003-WL-REL-004-preview-afc-r1.md`.
- Remaining work:
  1. S1 Materialize and verify the self-signing boundary.
     - Inputs: project release key, private signing material, RPM signing identity receipt, and WL-REL-002 handoff.
     - Action: confirm public fingerprint and receipt; copy only the three handoff RPMs into one private signing directory.
     - Deliverable: redacted signer identity evidence and exact pre-sign payload identity table.
     - Validation: sign-release.sh --self-test; receipt inspector; rpm -Kv before mutation; no secret bytes enter logs or Git.
     - Done when: one authorized fingerprint is selected and all three inputs match handoff.json exactly.
  2. S2 Sign all three RPM roles atomically.
     - Inputs: verified signing directory.
     - Action: run sign-release.sh --prepare-rpms on all three RPMs in one invocation.
     - Deliverable: signed Workstation, Server, and Lighthouse RPMs plus post-sign identity table.
     - Validation: rpm -Kv verifies signatures; payload digests and NEVRAs equal the unsigned handoff; partial failure leaves no mixed set.
     - Done when: all three signatures verify and no payload identity changed.
  3. S3 Produce RPM candidate manifests and base receipts.
     - Inputs: signed RPMs, frozen source, App/Browser base images, reproducibility receipts, and release key.
     - Action: run each role's canonical supply/candidate verifier and freeze Browser/App base-image profiles.
     - Deliverable: three RPM candidate manifests and exact App VM/Browser VM base receipts.
     - Validation: app-vm, server-rpm, and lighthouse candidate tools accept only their corresponding signed RPM.
     - Done when: every manifest names one immutable artifact, revision, signer, NEVRA, and payload digest.
  4. S4 Build Browser VM and App VM derivatives.
     - Inputs: signed Workstation/Lighthouse RPMs, candidate manifests, base images/receipts, and App catalog inputs.
     - Action: run build-release-derivative-images.sh with an absent private output path.
     - Deliverable: immutable Browser VM and App VM images, manifests, and frozen Browser profile.
     - Validation: image manifest verifiers, qcow2 checks, source revision checks, and hostile substitution fixture.
     - Done when: both derivatives verify and the helper publishes no partial output.
  5. S5 Admit Cuttlefish and bootc roles.
     - Inputs: Cuttlefish artifact/declaration/image receipt and bootc digest receipt/reference/architecture/role.
     - Action: verify governed Cuttlefish bytes and bootc receipt; do not rebuild or relabel ungoverned third-party bytes.
     - Deliverable: Cuttlefish artifact/receipt fields and bootc receipt fields ready for the seven-role plan.
     - Validation: verify-guest-payload.sh, verify-guest-debs.sh, image receipt verifier, and bootc digest receipt verifier.
     - Done when: both roles bind to the frozen revision and reject changed bytes, identity, architecture, or provider.
  6. S6 Create the exact seven-role plan input.
     - Inputs: three signed RPMs/manifests, two derivative images/manifests, Cuttlefish fields, and bootc fields.
     - Action: write one private mcnf-release-output-plan-input JSON object containing exactly the seven canonical roles.
     - Deliverable: immutable plan input and a redacted role inventory.
     - Validation: produce-release-output-plan.py accepts it; missing, duplicate, extra, relative, mutable, or cross-revision inputs refuse.
     - Done when: exactly seven role records are accepted and no artifact path is ambiguous.
- Scope: self-signing, candidate manifests, derivative generation, and plan input; no final evidence signing, publication, or installation.
- Relevant files/components: install-helpers/sign-release.sh, install-helpers/build-release-derivative-images.sh,
  install-helpers/produce-release-output-plan.py, packaging/app-vm, packaging/browser-vm, packaging/android, bootc receipt tools.
- Dependencies: WL-REL-002 and local access to authorized self-signing key material.
- Acceptance criteria: three RPM signatures verify without payload drift; four derivative roles verify; exactly seven roles bind to one revision.
- Verification method: signing and role-specific verifiers, derivative hostile suite, plan producer, and independent hash/identity comparison.
- Origin or merged source IDs: archived WL-BUILD-001, WL-BUILD-003, WL-FUNC-016, WL-FUNC-017, and WL-CRIT-006 release roles.

### WL-REL-004 - Assemble the signed seven-role release evidence bundle

- Status: Blocked
- Priority: P0
- Complexity: Epic
- Problem: publication is forbidden until all artifacts, manifests, gates, SBOM data, checksums, and provenance form one exact signed bundle.
- Required outcome: collect and verify all seven roles, execute mandatory release gates, and sign one immutable publication envelope.
- Current state: the canonical seven-role plan and collector pass for the
  private historical `afc24782` preview, including fresh App VM and Browser VM manifest
  verification. The collection is promotion-forbidden and still lacks the
  signed provenance/SBOM/gate envelope, clean-room publication readback, and
  final source-freeze authority required to close this epic. Evidence:
  `docs/platform/evidence/WL-REL-003-WL-REL-004-preview-afc-r1.md`.
- Remaining work:
  1. S1 Resume and collect the seven-role output.
     - Inputs: WL-REL-002 handoff, WL-REL-003 derivative argv and plan input, frozen revision, and Fedora target.
     - Action: run run-first-full-release.sh resume into an absent private output path.
     - Deliverable: collection-plan.json, release-outputs.json, verified derivatives, and promotion-forbidden output directory.
     - Validation: resume compares signed RPM payloads to the handoff and collectors re-run every canonical owning verifier.
     - Done when: collection is atomic, immutable, revision-bound, and contains exactly seven verified roles.
  2. S2 Execute the canonical gate matrix.
     - Inputs: release-gate-matrix.json, frozen revision, collected artifacts, and all named evidence commands.
     - Action: run every mandatory gate; route heavy package/workspace gates to the farm and preserve exact commands/results.
     - Deliverable: complete gate manifest with pass/fail, owner, command, artifact, revision, and timestamps.
     - Validation: verify-release-gate-matrix.py --expected-revision; omitted, vacuous, stale, or altered gate results refuse.
     - Done when: all mandatory gates are genuinely green or the epic is marked Blocked with the exact failing implementation.
  3. S3 Generate SBOM and release evidence.
     - Inputs: seven-role collection, dependency closure outputs, build identities, and gate manifest.
     - Action: run existing SBOM/evidence producers; bind every artifact hash and candidate manifest into one evidence envelope.
     - Deliverable: SBOM manifest, evidence JSON, release-output inventory, and artifact-to-source traceability table.
     - Validation: all evidence paths are immutable regular files; hashes and source revisions match collector output.
     - Done when: every published artifact has one verifiable source, role, checksum, signer, manifest, and gate lineage.
  4. S4 Sign checksums and provenance.
     - Inputs: exact artifacts, SBOM manifest, gate manifest, evidence envelope, and authorized self-signing identity.
     - Action: run sign-release.sh --evidence on the complete artifact set in one final publication directory.
     - Deliverable: PROVENANCE.json, SHA256SUMS, SHA256SUMS.asc, and signed evidence bundle.
     - Validation: detached signature, checksum, provenance, signer identity, exact artifact set, and inode/race protections verify.
     - Done when: fresh verification succeeds and any changed, added, missing, symlinked, or unbound file refuses.
  5. S5 Preflight remote publication.
     - Inputs: final bundle, planned tag, repository identity, workflow evidence, and release notes.
     - Action: run verify-github-release-binding.sh without publishing.
     - Deliverable: publication-readiness evidence naming exact tag, revision, assets, hashes, and signer.
     - Validation: hostile artifact-root, identity, set, path, symlink, and unbound-log cases remain rejected.
     - Done when: the complete bundle is ready for one atomic logical publication and nothing else is admitted.
- Scope: evidence collection, release gates, SBOM, provenance, signatures, and publication preflight; no public mutation or seat installation.
- Relevant files/components: install-helpers/run-first-full-release.sh, produce-release-output-plan.py,
  collect-release-outputs.py, release-gate-matrix.json, verify-release-gate-matrix.py, sign-release.sh.
- Dependencies: WL-REL-003.
- Acceptance criteria: one signed immutable seven-role evidence bundle passes all mandatory gates and rejects any artifact-set drift.
- Verification method: farm gates, collector and gate verifiers, SBOM/evidence checks, detached-signature verification, and publication preflight.
- Origin or merged source IDs: archived WL-BUILD-003 and WL-CRIT-006 production-evidence responsibilities.

### WL-REL-005 - Publish and promote the newest complete release

- Status: Blocked
- Priority: P0
- Complexity: Epic
- Problem: version 13.0.0 has no immutable current tag or complete public asset set, and partial candidates must never enter the package channel.
- Required outcome: publish one immutable tag and GitHub release, verify all assets by readback, then atomically expose only signed package metadata.
- Current state: tags end at magic-mesh-v12.1.1; WL-REL-004 has no signed seven-role bundle, so publication is correctly refused.
- Remaining work:
  1. S1 Reconfirm publication authority and remote state.
     - Inputs: WL-REL-001 tag plan, WL-REL-004 readiness evidence, GitHub remote, and package repository destination.
     - Action: verify tag and release do not exist; verify frozen revision is pushed; verify authenticated publication access.
     - Deliverable: pre-publication remote-state record.
     - Validation: any existing conflicting tag/release, wrong repository, moving revision, incomplete bundle, or missing authority stops work.
     - Done when: the target names are absent and the exact revision/bundle is ready.
  2. S2 Create and push the immutable tag.
     - Inputs: frozen revision, exact tag name, and release-note title.
     - Action: create a signed annotated magic-mesh-v13.0.0 tag and push that tag only.
     - Deliverable: remote tag object and tag-signature evidence.
     - Validation: local and remote tag dereference to the frozen revision; tag signature verifies.
     - Done when: no branch tip or alternate commit can masquerade as the release tag.
  3. S3 Publish the GitHub release and exact assets.
     - Inputs: remote tag, release notes, seven artifacts, manifests, SBOM, gates, provenance, checksums, and signatures.
     - Action: create one release and upload the complete admitted set; do not publish a draft/partial set as final.
     - Deliverable: public release URL, asset inventory, sizes, hashes, and publication receipt.
     - Validation: verify-github-release-binding.sh against remote metadata; asset count and names equal the admitted bundle.
     - Done when: every required asset is downloadable and no unadmitted asset is attached.
  4. S4 Verify downloaded bytes independently.
     - Inputs: fresh private download directory and published release.
     - Action: download every asset; verify SHA256SUMS.asc, checksums, provenance, SBOM/gates, RPM signatures, and role identities.
     - Deliverable: clean-room readback transcript and downloaded-asset digest table.
     - Validation: no local artifact path is reused; all downloaded bytes match the signed bundle.
     - Done when: public readback independently reconstructs the exact seven-role release identity.
  5. S5 Promote signed package metadata atomically.
     - Inputs: verified published RPMs, signed repository policy, HOLD boundary, and current channel metadata.
     - Action: stage metadata privately; ensure HOLD/unsigned candidates are excluded; atomically publish signed repodata.
     - Deliverable: repository metadata receipt and package query output for all three RPM roles.
     - Validation: fresh repository query resolves only signed admitted NEVRAs; partial/unsigned fixture cannot enter metadata.
     - Done when: package clients can resolve the complete release and no stale or unsigned higher candidate blocks upgrades.
  6. S6 Hand off to installed acceptance.
     - Inputs: publication receipt, download verifier results, package/image references, and corrected-forward recovery identity.
     - Action: update WL-TEST-002 with exact release inputs and select exactly Dell, Seat 15, and Surface as physical proof seats.
     - Deliverable: acceptance handoff naming immutable artifacts, seats, lighthouses, providers, and rollback-forbidden recovery plan.
     - Validation: all references resolve and every seat mutation requires the governed alert/wait sequence.
     - Done when: WL-TEST-002 can begin without guessing any release, package, image, seat, or recovery identity.
- Scope: tag, GitHub release, asset readback, signed package metadata promotion, and acceptance handoff.
- Relevant files/components: Git remote/tag tooling, GitHub release workflow, verify-github-release-binding.sh,
  packaging/repo, dnf-channel helpers, release notes, and WL-TEST-002.
- Dependencies: WL-REL-004.
- Acceptance criteria: tag, release, assets, signatures, provenance, and package metadata agree exactly; no partial release is visible.
- Verification method: remote tag/release readback, clean-room asset verification, repository queries, and HOLD/partial promotion refusal.
- Origin or merged source IDs: archived WL-BUILD-001, WL-BUILD-003, and WL-CRIT-006 publication responsibilities.

### WL-TEST-002 - Install and prove the newest complete release

- Status: Blocked
- Priority: P1
- Complexity: Epic
- Problem: exact-release installation, providers, direct-DRM rendering, guest/device integrations, and corrected-forward recovery need live proof.
- Required outcome: qualify the exact unpublished production candidate on Dell,
  Seat 15, and Surface, prove the three-lighthouse topology, then verify the
  same bytes after WL-REL-005 publication. Eagle and T480 remain non-gating
  inspection/deployment-wave seats.
- Current state: pre-release harnesses pass; candidate qualification is blocked on the current-source seven-role candidate and real production inputs.
- Remaining work:
  1. S1 Admit the unpublished signed candidate.
     - Inputs: WL-REL-003 candidate manifest, signed RPM and image identities,
       real production inputs, and selected seats.
     - Action: verify candidate bytes; record seat hardware, authorization, current package, target package, and recovery identity.
     - Deliverable: installed-acceptance plan and pre-mutation baseline.
     - Validation: tested bytes equal the immutable candidate manifest; exactly Dell, Seat 15, and Surface are selected for deep acceptance.
     - Done when: exact inputs and targets are unambiguous and recoverable by corrected-forward deployment.
  2. S2 Install and verify the baseline.
     - Inputs: admitted package/image references and governed mutation plan.
     - Action: publish the red AI-GENERATED-ALERT, wait five seconds, install, reboot only if required, and collect baseline observations.
     - Deliverable: installed NEVRAs/build identities, service states, display/audio/network/storage inventory, and restart/rejoin evidence.
     - Validation: package, mackesd, shell, About, watermark, welcome, and mesh-help versions agree.
     - Done when: each selected seat boots the exact release with honest degraded states and no stale build identity.
  3. S3 Prove collaboration and authorized providers.
     - Inputs: governed SIP/provider credentials, collaboration identities, the three-seat `13.0.0` maximum, and signed release.
     - Action: test Calls lifecycle, mute, consent, revocation, reconnect, chat/alerts, files/transfers, editor, and clipboard.
     - Deliverable: redacted provider/readiness, command, failure, and recovery evidence.
     - Validation: missing providers remain visible; no fake connected state; signed attribution and revocation remain auditable.
     - Done when: available provider paths pass and unavailable paths are named external blockers, not passes.
  4. S4 Capture Construct direct-DRM acceptance.
     - Inputs: selected display seat, exact release, Dark/Light and required text/layout profiles.
     - Action: capture shell, taskbar, Front Door, Workers, Kiron/health, Maps, Editor, Music, Files, and key error states.
     - Deliverable: native readback images/metadata, hashes, route identity, dimensions, and human-review disposition.
     - Validation: captures come from the direct-DRM seat and exact release; boot curtains, stale routes, or clipped required controls fail.
     - Done when: required visual routes pass or reopen a named implementation epic.
  5. S5 Prove media and physical integrations.
     - Inputs: audio/video fixtures, authorized Cast/DLNA devices, catalog/server paths, and network-loss controls.
     - Action: test playback, cache/offline, audio/video, renderer recovery, Cast, DLNA, typed handoff, and provider loss.
     - Deliverable: device identity, media command/result, continuity, loss, recovery, and CPU/package observations.
     - Validation: device/provider discovery is real; media state never claims success after transport failure.
     - Done when: each available integration passes or has one exact external blocker.
  6. S6 Prove guest and device roles.
     - Inputs: signed Browser VM, App VM, Cuttlefish, bootc artifacts, GPU/audio/input fixtures, and nested-KVM capability.
     - Action: launch and reconnect Browser/VDI/App/Android roles; test input, audio, GPU, upgrade identity, and failure recovery.
     - Deliverable: artifact-to-runtime identity, readiness, connection, detach/reconnect, and failure evidence.
     - Validation: runtime bytes match signed artifacts; missing capability is visible and cannot become a healthy state.
     - Done when: every available guest role passes exact identity and lifecycle checks or records a named hardware blocker.
  7. S7 Execute recovery and resilience drills.
     - Inputs: installed baseline, corrected-forward candidate, service/network/storage controls, and recovery verifier.
     - Action: test process restart, display/session recovery, lock/sleep, network/storage loss, generation change, reboot, and re-enrollment.
     - Deliverable: pre-failure, failure, correction, and post-recovery evidence for each drill.
     - Validation: verify-corrected-forward-recovery.py; no rollback satisfies recovery; data/history retention rules remain enforced.
     - Done when: failures converge by corrected-forward action without invented health or unrecorded data loss.
  8. S8 Reconcile and archive acceptance.
     - Inputs: all S1-S7 evidence and every archived source-epic proof queue.
     - Action: map results to owning epics; reopen implementation regressions; retain external blockers; create final release disposition.
     - Deliverable: signed acceptance index, blocker list, reopened work references, and WL-TEST-002 archive disposition.
     - Validation: every deferred obligation has evidence, a reopened implementation item, or one exact external-input blocker.
     - Done when: no obligation is silently dropped and the epic can be removed from the active worklist.
- Scope: exact-release admission, exactly Dell, Seat 15, and Surface as physical proof seats, providers, direct DRM, media/devices, guests, resilience, and reconciliation.
- Relevant files/components: docs/platform/release-evidence, install-helpers release/live/recovery verifiers,
  packaging installed-identity tools, direct-DRM capture helpers, and archived epic dispositions.
- Dependencies: WL-REL-003 for unpublished-candidate S1-S7; WL-REL-005 for
  post-publication S8; authorized provider inputs, selected Dell/Seat 15/Surface
  hardware, and the three-seat `13.0.0` qualification lock.
- Acceptance criteria: tested bytes match the signed release; states remain honest; recovery is corrected-forward; every deferred proof is reconciled.
- Verification method: focused farm gates followed by exact installed three-seat live checks on Dell, Seat 15, and Surface with redacted evidence and independent readback.
- Origin or merged source IDs: WL-TEST-001 proof boundary and deferred queues from archived UX, Music, Collaboration, guest, and recovery epics.

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
