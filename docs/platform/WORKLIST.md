# Platform Worklist

This is the only active platform worklist. Design notes, evidence ledgers,
runbooks, and operator notes are inputs, not parallel trackers. Historical
implementation diaries remain in docs/worklist-archive/ and are not executable
tasks.

## Current Snapshot - 2026-08-19 fully automated production 13.0.0 execution plus feature completion

- **19 active epics:** 2 `Remaining`, 17 `Blocked`, 0 `Needs clarification`.
  Operator survey 2026-08-22: Q9 delete authorized; Q26 Files stays its own
  surface; PR #71 Ready; freeze waits on live FUNC-023 enroll; Geofabrik Maps
  fetch authorized; preflight template then operator secrets; seats+Vitelity go.
- **Latest stable integration:** 43 exact hostile gates passed across four farm hosts: `evidence/WORKLIST-2026-08-11-stable-exact-wave-r473.md`.
- **Execution order:** implement all source-changing lifecycle work under
  `WL-FUNC-023`; record one clean pushed release-candidate revision and epoch
  under `WL-REL-001` S1; materialize and admit the already-selected production
  inputs under `WL-REL-006` against that exact candidate; reconfirm that the
  candidate did not move and promote the same revision to the final source
  freeze; cut and sign the six roles under `WL-REL-002`/`WL-REL-003`; stage
  the unpublished signed candidate on the production topology and run
  `WL-TEST-002`; complete the final signed evidence envelope under
  `WL-REL-004`; then publish and read back under `WL-REL-005`. If source changes
  after input generation begins, invalidate the source-bound receipts and
  repeat input   admission; never solve the dependency by weakening source
  binding.
- **Feature-completion lane (2026-08-19):** `WL-FUNC-024` through `WL-FUNC-033`
  close the remaining gap between implementation-complete and fit for purpose —
  the Communications parity-ledger rulings that never landed, the Calls media
  plane, and the operator-flagged legacy mesh-PBX retirement. They are
  implementation-only, disjoint from the release chain, carry no new testing or
  security-control scope, and are pre-freeze source work executable in parallel
  by disjoint workers under the non-stall contract.
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
  in docs/platform/evidence/. Missing hardware, provider access, credentials, or
  capacity fails the owning automated gate; it is never a passing substitute or
  an interactive operator handoff.
- **Unattended-release lock (2026-08-17):** approval of a signed,
  revision-bound `ReleaseIntentV1` on protected `master` is the sole release
  authorization. From that point the release coordinator must provision,
  build, sign, stage, qualify, publish, read back, clean up, and archive without
  interactive steps. Credentials are named systemd-credential/mde-seal inputs,
  not argv, logs, Git data, or evidence. A failed dependency receives bounded
  automated remediation and retry, then reopens its exact owning `WL-*` story;
  no `operator needed`, manual assertion, unavailable-feature waiver, or
  synthetic production pass is admissible.
- **Android deferral lock (2026-08-17):** all Android and Cuttlefish build,
  package, guest, VDI, input, runtime, hardware, and live-proof capability is
  deferred beyond `13.0.0` and is not a release input, role, gate, artifact, or
  acceptance obligation. The production set is exactly six roles: Workstation
  RPM, Server RPM, Lighthouse RPM, Browser VM, App VM, and bootc image. Preserve
  historical Android evidence as non-promotable history and render Android as
  visibly `Deferred`; reactivation requires a new active epic and a newer
  governance lock after this release.
- **Shared release-proof ownership:** first-release input admission, signed
  artifact/package proof, installed baseline acceptance, corrected-forward
  recovery, and deferred provider/live proofs are owned by `WL-TEST-002`.
  Product epics must not duplicate those rollout tasks; they retain only
  product-specific implementation and integration gaps, and cite `WL-TEST-002`
  when its acceptance is a dependency.
- **Production qualification topology (release lock 2026-08-16):** deep
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
- **Privacy-retention lock (release lock 2026-08-10):** system logs, Bus
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
  report the current five hosts and ten heavy slots (`.50` cap 2, `.90` cap 2,
  `.130` cap 3, `.170` cap 2, `.196` cap 1), and never run filler tests. BigBoy
  remains the long-pole host; no Android-only builder or storage is provisioned
  for `13.0.0`.
- **Rollout lock:** for `13.0.0`, prove each release activity on exactly the
  three selected physical seats (Dell, Seat 15, Surface) and the independently
  required lighthouses. The approved
  promotion-forbidden preview may be distributed to all designated test seats
  and designated test lighthouses, but that wider distribution does not expand
  the test or acceptance requirement. Replace lighthouses one at a time while
  preserving quorum. Publish the red AI-GENERATED-ALERT and wait five seconds
  before each mutation. Recover failures by re-enrollment and corrected-forward
  deployment, never rollback.
- **Objective-qualification lock:** replace human listening, visual review, and
  physical-observer dispositions with hash-bound machine observations from
  permanent self-testing fixtures: DRM/KMS capture, audio chirp correlation,
  HID/sensor/camera/power telemetry, and controlled Cast/DLNA receivers. A
  missing or unhealthy fixture fails its owning gate; all six roles and every
  selected feature must pass before publication.
- **Story format:** execute stories top-to-bottom. Do not start a story until
  every dependency is green. If a dependency is absent, set the epic to
  Blocked with the exact owning story and machine-readable failed gate; do not
  invent evidence or delegate resolution to an operator.

## Active Drain Goal

Implement the unified turnkey seat lifecycle, then cut, self-sign, qualify,
publish, install, and prove production `magic-mesh-v13.0.0` from one exact clean
protected-default-branch revision. Produce all six canonical roles, retain
fail-closed provenance, require real production evidence, and keep deep live
acceptance within Seat 15, Dell, and Surface.

## Service Release Queue

1. Implement the unified ONBOARD & OFFBOARDING lifecycle.
2. Create and admit real production release inputs.
3. Re-freeze the feature-complete `13.0.0` source on the protected default branch.
4. Build and self-sign all six canonical roles.
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

Every `Status: Remaining` epic below carries at least one
`@farm:{cargo …}` payload as part of `Verification method:`. That payload is
what `automation/lib/farm-jobs.sh active` emits, what
`automation/queue/farm-enqueue.sh` pushes to `/farm/queue/*`, and what a
`automation/queue/farm-agent.sh` worker slot picks up. Multiple such markers
on the same epic authorise disjoint parallel workers; each covers a
non-overlapping slice of the epic's `Relevant files/components`. Adding or
retiring an epic changes the queue; do not build a second queue or scheduler
(`AI_GOVERNANCE.md` §10.0.4).

## Parallel drain execution contract

The canonical tick is `install-helpers/drain-coordinator.sh plan`
(preflight + per-node free-slot map + next-N candidate units) or, from the
control host, `automation/drain/ship-coordinator.sh --once` (adds farm
reconcile + needs-review/triage surfaces). Both read one roster —
`install-helpers/farm-topology.sh` (5 dom0s / 10 heavy slots) — and one
queue producer: `automation/lib/farm-jobs.sh active` over this file.

While Remaining epics exist, the coordinator must keep
`min(active_farm_jobs, free_slots)` slots busy. When Remaining epics exist
but `farm-jobs.sh active` returns zero, the responsible agent's next act is
decomposing the top-priority epic into disjoint `@farm:{cargo …}` units,
not starting single-threaded implementation. Idle nodes with Remaining
stories is a process failure, not a resource shortage.

Local heavy `cargo` remains blocked by
`install-helpers/install-drain-guardrails.sh` (exit 97 with a farm redirect).
Do not bypass. Slot GC (`install-helpers/farm-slot-gc.sh`) reclaims stale
`~/magic-mesh-farm-*` dirs on a 20-minute timer; an ENOSPC after admission
is a capacity incident (§10.0.3), not a silent retry.

## Non-stall execution contract

- A blocked story parks only its dependent lane. While any implementation,
  remediation, verification, documentation, or cleanup story is ready,
  `WL-REL-007` remains `Remaining`, Luna immediately assigns the next disjoint
  story, and the drain does not stop for status reporting or clarification.
- Before candidate freeze, synchronize `AI_GOVERNANCE.md`, release schemas,
  helpers, CI, and documentation to the unattended six-role release lock. Until
  that source change lands, continue lifecycle and release-tool implementation
  but do not freeze or publish a candidate.
- Preserve and integrate owned dirty work on the current branch; never discard
  it to obtain a clean receipt. A separate clean protected-`master` checkout is
  created only after reviewed changes merge and `github-required` is enforced
  through the repository ruleset using the scoped release credential.
- Credential preflight loads named systemd/mde-seal credentials, tests them
  without disclosure, renews through supported provider APIs, and retries at
  30 seconds, 2 minutes, and 10 minutes. A still-failing credential parks its
  live/publication lane while source, package, and evidence work continues.
- If a lighthouse rejects SSH, the coordinator uses provider APIs to create a
  replacement with an ephemeral pinned bootstrap key, joins and catches it up,
  retires the unreachable member, and repeats one node at a time while two
  voters remain healthy. No console or password handoff is required.
- Farm admission first reclaims only expired owned slots, then reassigns the
  immutable job. Native Fedora 44 capacity is provisioned on the declared
  BigBoy lane and `rpm-sign` is installed through configuration management;
  neither condition is a request for manual setup.
- Objective qualification uses automatically started software receivers plus
  each seat's DRM readback, speakers/microphones, cameras, input-device
  capability reports, sensors, and RTC wake. Missing observations reopen the
  exact implementation or hardware-support story without blocking unrelated
  lanes; production publication remains fail-closed.

### WL-REL-007 - Execute the SOL Luna AI production 13.0.0 completion plan

- Status: Blocked
- Priority: P0
- Complexity: Epic
- Problem: the active lifecycle, release-input, source-freeze, build, signing,
  qualification, evidence, and publication epics form one production release
  dependency chain, but their cross-epic execution, farm ownership, temporary
  infrastructure remediation, and final reconciliation need one explicit
  governing plan so no blocker is skipped or satisfied with historical fixture evidence.
- Required outcome: one restart-safe release coordinator executes the owning
  epics to produce,
  qualify, publish, read back, and archive production
  `magic-mesh-v13.0.0` from one exact clean protected-default-branch revision,
  with exactly six canonical roles and no fabricated or substituted evidence.
- Current state: the eight owning epics contain product and release criteria.
  Surface is approved for `13.0.0`; Android/Cuttlefish is deferred. WL-REL-006
  is parked (freeze / catalog refs / RPM secret). Coordinator leftover is
  FUNC-023 live enroll (no unpublished signed candidate) and FUNC-033 keep
  `own_nebula_ip`. Do not grind `cargo test --workspace` as filler. Evidence:
  `WL-REL-007-2026-08-22-coordinator-park-r1.md`. Exact acceptance is Dell,
  Seat 15, Surface, and three lighthouses.
- Remaining work:
  1. S1 Establish SOL Luna execution ownership and release ordering.
     - Inputs: this worklist, governance locks, farm topology, the eight owning
       epics, and the clean integration branch.
     - Action: keep one integration authority and use two to five workers
       only for disjoint lifecycle, surfaces, inputs, infrastructure, and release
       scopes. Execute `WL-FUNC-023`; establish the `WL-REL-001` S1 candidate
       identity; execute `WL-REL-006` against it; reconfirm and finalize
       `WL-REL-001`; then execute `WL-REL-002`, `WL-REL-003`, pre-publication
       `WL-TEST-002`, `WL-REL-004`, `WL-REL-005`, and final `WL-TEST-002`
       reconciliation.
     - Deliverable: strict signed `ReleaseIntentV1`, `ReleaseStateV1`, and
       `ReleaseStageReceiptV1` contracts plus a restart-safe stage journal under
       `/var/lib/mcnf-release/<revision>/` binding version, source, six roles,
       six targets, credential names, destructive scope, retries, and
       input/output hashes; this journal is execution state, not a worklist.
     - Validation: every worker has a disjoint write scope; every mutation and
       gate maps to one owning story; compare-and-swap stage receipts resume
       after interruption; no parallel tracker or filler farm job is created.
     - Done when: every ready story has one owner and no downstream story starts
       before its dependencies are green.
  2. S2 Complete the unified lifecycle under WL-FUNC-023.
     - Inputs: `WL-FUNC-023` S1-S18 and its existing authority evidence.
     - Action: complete the typed lifecycle model, mackesd-only resumable
       authority, GUI/TUI parity, authorization, commissioning, artifact
       selection, audit/correction, onboarding, upgrade, warning handling,
       offboarding, reset, fleet execution, packaging, and first-boot behavior.
       Mint the enrollment bearer through the existing lifecycle authority and
       wire the existing typed SSH transport seam; pass target-bound enrollment
       material through stdin or a credential descriptor with pinned host
       identity, never argv or command text. Prove minting,
       redaction, refusal, replay, SSH result, and Bus acknowledgement with farm
       fixtures now; defer exact-candidate target execution to `WL-TEST-002`.
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
     - Action: accept the recorded production choices as final and produce the
       OpenStreetMap-derived Buffalo-Niagara Maps bundle
       clipped to official Erie and Niagara county boundaries using the existing
       Maps approval, producer, materializer, and verifier contracts; enforce
       the aggregate quota and deterministic transport; regenerate App VM,
       bootc, and Kiron receipts; generate the existing signer receipt; and
       materialize canonical private mode-0400 preflight
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
  4. S4 Apply the Android/Cuttlefish deferral across release contracts.
     - Inputs: the Android deferral lock, durable governance, release schemas,
       role collectors, preflight, CI, GUI/TUI capability projections, and docs.
     - Action: synchronize every live source and policy surface to the six-role
       `13.0.0` set; reject Android inputs for this version and render the
       capability visibly `Deferred` without building or provisioning it.
     - Deliverable: governance/schema migration, six-role producer/verifier
       contracts, and renderer-neutral deferred-capability state.
     - Validation: source scans and hostile fixtures reject stale seven-role or
       required-Cuttlefish assumptions while preserving historical evidence.
     - Done when: no `13.0.0` gate, helper, role, or UI readiness result requires
       Android bytes, infrastructure, credentials, or live proof.
  5. S5 Freeze source and cut the six-role candidate.
     - Inputs: completed lifecycle and input epics, the clean pushed candidate
       revision used by S3, protected `master`, required GitHub checks,
       authorized signing material, and Fedora 44 builders.
     - Action: fetch and reconfirm that the S3 candidate revision, epoch, and
       tree did not move; promote that unchanged identity to the final freeze
       and rerun canonical preflight. If it changed, invalidate the old receipts
       and return to S3. Once stable, build exactly three unsigned RPMs; seal
       the handoff; verify access to the already-authorized self-signing key;
       sign all three atomically without payload drift; build Browser VM and App
       VM derivatives; and admit bootc.
     - Deliverable: one immutable private candidate containing exactly
       Workstation RPM, Server RPM, Lighthouse RPM, Browser VM, App VM, and
       bootc image.
     - Validation: BigBoy runs the long poles; every permanent farm
       host runs a unique meaningful build or gate; handoff, signature, NEVRA,
       payload, receipt, manifest, collector, and six-role hostile checks pass.
     - Done when: `WL-REL-001`, `WL-REL-002`, and `WL-REL-003` are complete and
       the unpublished signed candidate binds exactly to the frozen source.
  6. S6 Qualify the unpublished candidate on production topology.
     - Inputs: the signed six-role candidate, Dell, Seat 15, Surface, three
       lighthouses, provider authority, and corrected-forward recovery identity.
     - Action: run unattended read-only admission on all designated seats, then
       perform deep acceptance on exactly Dell, Seat 15, and Surface. Publish the red
       `AI-GENERATED-ALERT` and wait five seconds before every mutation. Upgrade
       lighthouses one at a time while preserving quorum. Eagle and T480 remain
       non-gating inspection/deployment-wave seats.
     - Deliverable: exact installed identity, lifecycle, provider, direct-DRM,
       Maps, collaboration, media/device, guest-role, Surface-hardware,
       resilience, privacy-retention, lighthouse, and recovery evidence owned
       by `WL-TEST-002`.
     - Validation: tested bytes match the candidate; DRM, audio, HID, sensor,
       camera, power, Cast/DLNA, provider, and guest evidence is captured by
       objective fixtures; every failure recovers by corrected-forward action
       or re-enrollment, never rollback or manual assertion.
     - Done when: `WL-TEST-002` S1-S7 pass or reopen one exact owning
       implementation blocker with no invented success.
  7. S7 Assemble, sign, publish, and independently read back the release.
     - Inputs: qualified six-role candidate, gate matrix, SBOM producers,
       release key, release notes, GitHub authority, and package repository.
     - Action: collect exactly six roles; require `github-required` through
       branch protection for the frozen revision; consume a typed gate-result
       manifest; generate aggregate six-role SBOM/license, compatibility,
       provenance, checksums, and evidence;
       sign the complete envelope; create signed tag
       `magic-mesh-v13.0.0`; publish the exact asset set; download it into a new
       directory; verify the remote tag and strict asset allowlist; and
       atomically promote signed repository metadata with `repo_gpgcheck=1`
       only after clean-room verification.
     - Deliverable: immutable tag and release, signed six-role evidence
       bundle, public asset/readback receipt, and signed package-channel receipt.
     - Validation: omitted, extra, changed, stale, linked, unsigned, HOLD, or
       cross-revision files refuse; downloaded bytes reproduce the qualified
       artifact identities and all three RPM roles resolve from the channel.
     - Done when: `WL-REL-004` and `WL-REL-005` are archived and public readback
       agrees exactly with the frozen source and installed candidate.
  8. S8 Reconcile and archive the complete plan.
     - Inputs: every owning epic's evidence, public readback, installed
       acceptance, worklist stewardship rules, and archive dispositions.
     - Action: complete `WL-TEST-002` S8; map every obligation to evidence or a
       reopened implementation/infrastructure story; archive every completed
       owning epic and finally this coordination epic in an automated
       post-release documentation commit so the frozen revision does not move.
     - Deliverable: final signed acceptance index, release disposition, blocker
       inventory, and archive entries.
     - Validation: worklist self-test and lint pass; snapshot counts match; no
       deferred obligation, private secret, temporary release resource,
       abandoned worktree, or parallel tracker remains.
     - Done when: production `13.0.0` is published and independently verified,
       all completed epics are removed from the active worklist, and no manual
       release action, deferred feature, or unresolved external handoff remains.
- Scope: coordination and dependency enforcement for the existing lifecycle and
  release epics, including the explicit non-gating Android deferral.
  Existing epics remain the sole owners of implementation and acceptance work.
- Relevant files/components: `docs/platform/WORKLIST.md`, release/farm helpers,
  OpenTofu farm declarations, lifecycle components, release input producers,
  packaging, evidence collectors, and publication verifiers.
- Dependencies: WL-FUNC-023 live enroll after an unpublished signed candidate;
  WL-FUNC-033 keep `own_nebula_ip`; parked WL-REL-006 leftovers; then the
  remaining release chain in S1-S8 order. A failed gate reopens its owning
  story rather than requesting interactive resolution.
- Acceptance criteria: one clean source produces exactly six signed roles;
  real governed inputs pass preflight; production topology passes; signed evidence and public
  readback agree; no fixture, rollback, filler build, or fabricated proof
  satisfies a gate.
- Verification method: worklist lint, focused hostile tests, meaningful gates
  across all permanent farm hosts, exact three-seat and
  three-lighthouse acceptance, signed evidence verification, clean-room public
  readback, and repository query. @farm:{cargo test --workspace}
- Origin or merged source IDs: SOL Luna AI completion plan and Android deferral
  direction (2026-08-17).

### WL-FUNC-023 - Create the unified ONBOARD & OFFBOARDING lifecycle

- Status: Remaining
- Priority: P0
- Complexity: Epic
- Problem: setup, enrollment, upgrade, repair, reset, and offboarding are
  fragmented and can leave seats partially active. Seat 15 exposed missing
  identity, etcd, credential, compute, and grouped-service prerequisites.
- Required outcome: create one local-first ONBOARD & OFFBOARDING interface backed by one resumable mackesd authority for local or fleet onboarding, upgrade,
  verification/correction, offboarding, reset, and recommissioning.
- Current state: dests exist at `/root/mcnf-private/bootstrap-ssh-key` (0600) and
  `bootstrap-known-hosts` (0400); env `bootstrap-ssh.env` (0400). Child-only runner
  sources dests; enroll/offboard/join argv refuse; Construct/onboard/mint children strip dest env.
  `mint-enroll-bearer.py` wraps `enroll-token`; `/root/mcnf-private` dests refuse
  until dest-backed candidate admit; production mutation pins seat-update-warning. Seat 15 enrolled.
  Freeze bar still (1) live mint and (3) enroll/offboard+reenroll + 5s. Evidence:
  `WL-FUNC-023-2026-08-22-live-enroll-prereq-r1.md`,
  `WL-FUNC-023-2026-08-22-bootstrap-identity-provision-r1.md`,
  `WL-FUNC-023-2026-08-22-bootstrap-env-bind-r1.md`,
  `WL-FUNC-023-2026-08-22-bootstrap-env-run-r1.md`, dest-env strip r1, mint child strip r1.
- Remaining work: leftover freeze bar is still (1) mint a real 43-char enroll bearer
  through live lifecycle authority (helper exists; this unit did not invoke Seat 15
  mackesd), (2) child-only runner sources dests for a worker only (login env unset),
  (3) live enroll or authorized offboard/reenroll under red `AI-GENERATED-ALERT` + 5s.
  Seat 15 is a named workstation; first-enroll of that IP needs operator offboard+reenroll. GPT Luna: execute S1-S18 in order.
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
     - Action: add target-bound `CommissioningCapsuleV1` and QR/token exchange;
       authority-mint the bearer and hand it separately from command text.
     - Deliverable: zero-touch capsule and one-interaction token paths with
       encrypted retryable staging.
     - Validation: expiration, replay, revocation, target mismatch, conflict,
       disconnect, and redaction tests.
     - Done when: bootstrap material is erased only after confirmed enrollment.
  7. S7 Implement authority-controlled artifact selection.
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
       authorized issuer, warning, and confirmation.
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
      - Deliverable: evidence index, unattended execution runbook, migration notes, and
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
  interface; all renderers share one engine; capsule and release-driven token
  onboarding consume preloaded credentials without interaction; upgrades need
  no manual repair; destructive work is authority-bound; Offboard drains and erases completely;
  ResetAndOnboard cannot retain an old identity; unsigned artifacts require
  digest confirmation; core failures block; capability failures remain
  prominent `ReadyWithWarnings`.
- Verification method: focused hostile/unit tests, farm integration and package
  fixtures, GUI/TUI parity, interruption/resume proof, and exactly three physical
  acceptance seats; defer exact release/rollout proof to WL-TEST-002. @farm:{cargo test -p mackesd} @farm:{cargo test -p mde-enroll}
- Origin or merged source IDs: lifecycle consolidation direction, Seat 15 and Surface findings, clean-fleet survey, and GPT Luna assignment (2026-08-15).

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
  using the two-phase candidate/reconfirmation decision below.
- Remaining work:
  1. S1 Select the immutable source. BLOCKED on live FUNC-023 enroll, not Draft:
     operator 2026-08-22 marked PR #71 Ready and named this branch HEAD the
     input-generation candidate. Final freeze still waits on a real-seat
     enroll/offboard over SSH, then REL-006 admission and reconfirmation.
     Recorded 1dfe6906 predates FUNC-023 and must not receive new inputs.
     - Inputs: pushed branch, root Cargo.toml, remote branch state, and archived implementation dispositions.
     - Action: fetch remote refs; require an empty worktree; record HEAD,
       upstream HEAD, commit epoch, Fedora target, and version as the input
       candidate. After WL-REL-006 preflight succeeds, fetch again and require
       the same revision, epoch, and tree before declaring it the final freeze.
       Any source change invalidates candidate-bound receipts and returns
       execution to WL-REL-006. Require the exact revision's `github-required`
       result through branch protection/rulesets; a job with that name that is
       not required is not release authority.
     - Deliverable: docs/platform/evidence/WL-REL-001-source-freeze-r1.md with exact commands and outputs.
     - Validation: source-revision-receipt.sh --repo .; git diff --quiet; git diff --cached --quiet; compare HEAD with upstream.
     - Done when: one non-null 40-character revision and positive epoch identify
       the clean pushed source both before input generation and at final
       reconfirmation.
  2. S2 Verify every version surface. Complete: the three isolated browser
     helper manifests/lockfiles and shipped role chooser resolve to 13.0.0;
     the five non-shipped crates are recorded as packaging/test boundaries in
     docs/RELEASE-VERSIONING.md.
     - Inputs: docs/RELEASE-VERSIONING.md, root and isolated Cargo workspaces, package recipes, CLI/About build identity.
     - Action: run Cargo metadata; compare shipped package versions; scan runtime sources for competing numeric release authorities.
     - Deliverable: bounded version matrix naming each shipped surface, source, observed value, and exception.
     - Validation: farm metadata/package checks on .50; no runtime version authority other than workspace/package reflection.
     - Done when: every current release surface resolves to 13.0.0 or a documented packaging release suffix.
  3. S3 Admit all governed release inputs. BLOCKED on executable materialization,
     not unresolved product clarification: the production choices are final, but the
     release-input loader has no final private preflight object. The RPM signer
     receipt has been generated and inspected
     privately for the superseded f095b8ce revision; it must be regenerated for
     the new S1 candidate revision and epoch. The recorded 1dfe6906 revision is
     also superseded and must not receive new release inputs. Maps
     approval/source, App VM image/catalog receipt,
     bootc receipt is not admitted for the frozen revision. Android/Cuttlefish
     is deferred and must not appear in the `13.0.0` preflight object. Maps provider/live proof
     is explicitly deferred to WL-TEST-002; that deferral does not create a
     release-input approval. Execute WL-REL-006 against the S1 candidate and do
     not run a build with historical loose artifacts.
     - Inputs: Maps approval/source, App VM image/catalog receipt, RPM signer
       receipt, bootc receipt, and the six-role deferral migration.
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
- Dependencies: WL-FUNC-023 must complete before the S1 candidate identity is
  selected; WL-REL-006 must complete against that candidate before final S1/S4
  freeze disposition. This is a two-phase reconfirmation, not a circular
  requirement for two source revisions. Signed release intent authorizes
  self-signing; exact installed/live proof remains deferred to WL-TEST-002.
- Acceptance criteria: one clean pushed revision is frozen; all version surfaces and inputs bind to it; stale artifacts cannot enter later stages.
- Verification method: local read-only Git/version checks, focused farm metadata/package checks, preflight admission, and evidence review.
- Origin or merged source IDs: release recovery of archived WL-BUILD-001, WL-BUILD-003, and WL-CRIT-006 responsibilities.

### WL-REL-006 - Create governed open-source release inputs

- Status: Blocked
- Priority: P0
- Complexity: Epic
- Problem: WL-REL-001 cannot admit the production release until the already
  selected Maps, App VM, bootc, and UX-014 inputs are materialized
  as exact candidate-bound bytes and receipts and the private preflight object
  passes; historical or non-production fixtures cannot satisfy that gate.
- Required outcome: create or select real open-source-compatible production
  inputs, bind every byte and license to the clean candidate revision that must
  be reconfirmed unchanged as the final frozen source, and produce the exact
  non-secret receipts required by the canonical preflight. Fixtures may
  exercise contracts but cannot satisfy a production gate.
- Current state: S1 six-role inventory exists
  (`WL-REL-006-2026-08-22-open-source-inventory-r1.md`; no Cuttlefish). Maps dest
  `/var/lib/mde/maps/buffalo-niagara/buffalo-niagara.mbtiles` on BigBoy sha256
  `6d01a543c7a58f323656ce142a0e335e32a3070ecf03f7a9d655138df93f5895`
  (`production_admitted: false`). S3 App VM receipt bound to `aca7573bc`.
  App VM, Browser VM, and bootc ARGs and inventory pins match `3a5e74e6…`.
  Surface `bootc_base` stays null (blocked stack must not guess a digest).
  Leftover is Maps `production_admitted`, real catalog refs, RPM signer after
  freeze, S7 `REPLACE_*`, live-seat dest. Evidence:
  `WL-REL-006-2026-08-22-leftover-park-r1.md`.
- Remaining work:
  1. S1 Establish the open-source input policy.
     - Inputs: candidate source receipt, Fedora target, architecture, applicable
       licenses, and the existing receipt/verifier contracts.
     - Action: six-role redacted inventory is recorded; do not reopen source
       selection. Browser VM Containerfile pin now matches the admitted index
       `3a5e74e6…`. Leftover is Maps `production_admitted`, App catalog real
       refs, RPM signer receipt after freeze (WL-REL-001), S7 `REPLACE_*`, and
       live-seat dest (WL-TEST-002).
     - Deliverable: redacted open-source input inventory and license manifest.
      - Validation: every source is redistributable and its credential/preflight
       requirements are machine-verifiable;
       any fixture substitution follows the governed evidence template and is
       not presented as observed production behavior.
     - Done when: all admitted input families have a named reproducible source and an
       automated producer/verifier; missing inputs fail the owning story.
  2. S2 Produce the Maps input.
     - Inputs: digest-pinned Geofabrik NY PBF and TIGER 2024 county zip now on
       BigBoy `/home/mm/mcnf-maps-sources` (2026-08-22 fetch evidence); approved
       offline-cache policy, candidate source receipt, and license terms.
     - Action: TIGER zip clip-detect admits Erie 36029 / Niagara 36063.
       Extract wrote official-county GeoJSON `erie-niagara.geojson` (exactly
       those two GEOIDs; sidecar `mcnf-maps-tiger-clip`;
       `production_admitted: false`). Osmium clipped NY PBF to
       `erie-niagara.osm.pbf` (sidecar `mcnf-maps-pbf-clip`;
       `production_admitted: false`; official bbox, not envelope-shrunk).
       Remaining: BigBoy dest inspect of dest-root OSM-derived raster
       passed (`inspect_mbtiles`, quota 262144; sidecar
       `mcnf-maps-dest-inspect`; `production_admitted: false`).
       Candidate-bound dest receipt exists (`bind_receipt` /
       `verify_receipt`; kind `mcnf-maps-mbtiles-receipt`; sidecar
       `.mbtiles.receipt.json`; `production_admitted: false`; not
       production admission; not Dell/Seat 15/Surface). Envelope admits
       official TIGER clip. Leftover is `production_admitted` (needs the
       real candidate-bound provider object / freeze) and live-seat dest
       (WL-TEST-002). Clipped PBF is not
       MBTiles admission. Fixture PNG raster is not production
       admission. Preserve PBF, boundary, clip,
       renderer/style/font identities, ODbL attribution, aggregate quota,
       and deterministic transport. Never fetch public OSM tiles; defer
       installed runtime proof to WL-TEST-002.
     - Deliverable: immutable `buffalo-niagara.mbtiles`, source/build manifest,
       hashes, attribution, license, approval receipt, and package install path.
     - Validation: verify MBTiles schema, PNG payloads, TMS coordinates,
       bounds/zoom/quota, source hashes, revision, and installed-byte identity;
       changed bytes, wrong provider, or path substitution refuse.
     - Done when: preflight and release assembly preserve the exact MBTiles file
       that the production GUI will open.
  3. S3 Produce the App VM input.
     - Inputs: digest-pinned `quay.io/fedora/fedora-bootc:44`, architecture,
       the approved real application refs/licenses, and candidate source receipt.
     - Action: inspect the immutable base manifest, deterministically generate
       the production catalog, provision the system Flatpak remote named
       `curated`, and bind installation of the exact refs into the App VM image.
       Current-revision App VM receipt is bound to `aca7573bc` on `.90` slot 0
       (`WL-REL-006-2026-08-22-app-vm-receipt-r1.md`). Historical `0e0cd1b3`
       receipt is stale vs HEAD. Containerfile pin now matches admitted index
       `3a5e74e6…` (`WL-REL-006-2026-08-22-app-vm-base-pin-r1.md`). Leftover is
       catalog/`curated` remote plus S7 App-catalog `REPLACE_*`; do not claim
       the release-input gate closed.
     - Deliverable: immutable App VM digest, base receipt, catalog publication
       object, exact-ref inventory, compatibility metadata, and license record.
     - Validation: App VM producer/inspector and build-image admission pass;
       registry or local bytes are bound to the frozen revision.
     - Done when: App VM inputs and real `curated` catalog pass preflight before
       image-context mutation; fixture IDs and mutable tags refuse.
  4. S4 Remove Android/Cuttlefish from production input admission. Complete:
     `release-input-argv.py`, `release-input-preflight.sh`, the output
     producer/collector, and `run-first-full-release.sh` admit only the
     six-role release set; hostile loader, preflight, plan/collector, and
     release-resume tests reject stale Cuttlefish-bearing release inputs.
     - Inputs: Android deferral lock, private-object schema, release driver,
       output-plan producer, role collector, CI, and historical fixtures.
     - Action: remove Android/Cuttlefish fields from the `13.0.0` strict input
       object and six-role release path; preserve old fixtures only as explicit
       non-production compatibility tests and refuse them as release inputs.
     - Deliverable: versioned six-role schema migration plus producer,
       preflight, collector, and hostile-fixture evidence.
     - Validation: no Android path, byte, provider, builder, package, or receipt
       is required; stale seven-role and Cuttlefish-bearing production objects refuse.
     - Done when: canonical preflight reaches the other inputs without an
       Android dependency and the deferred capability is visible but non-gating.
  5. S5 Produce the bootc input.
     - Inputs: digest-pinned `quay.io/fedora/fedora-bootc:44`, architecture,
       canonical role `all-roles`, and candidate source receipt.
     - Action: inspect exact manifest bytes and produce the canonical bootc
       digest receipt; integrate receipt consumption into release preflight.
       Current-revision `all-roles` receipt is bound to `479ec2b8c` on `.170`
       slot 0 (`WL-REL-006-2026-08-22-bootc-all-roles-r1.md`). Historical
       `52fd0793`/`base` receipt is stale. Containerfile ARG now names the
       same admitted index (no new dest). Leftover is Maps `production_admitted`,
       App catalog real refs, RPM signer after freeze, S7 `REPLACE_*`, live-seat
       dest, Surface `bootc_base` still null; do not claim the gate closed.
     - Deliverable: immutable bootc receipt and preflight integration evidence.
     - Validation: architecture, role, digest, revision, epoch, and media type
       are all fail-closed; unavailable registry access refuses admission.
     - Done when: preflight consumes the receipt rather than a raw digest and
       rejects legacy `base` or `unified-seat-server` role identities.
  6. S6 Create UX-014 release assets.
     - Inputs: existing open-source UI assets, Kiron verifier contract, license
       attribution, candidate source receipt, and required asset dimensions.
     - Action: create the A-F package assets and their manifest using the
       governed asset format; do not claim live hardware proof from screenshots.
     - Deliverable: asset package, manifest, hashes, attribution, and verifier
       evidence.
     - Validation: Kiron verifier accepts the complete set and rejects missing,
       substituted, stale, or unlicensed assets.
     - Done when: WL-REL-003/004 can consume the exact asset manifest.
  7. S7 Materialize private first-release preflight argv.
     - Inputs: all current-revision receipts from S2-S6, App catalog object, RPM
       signer receipt, private paths, target architecture, and release epoch.
     - Action: write one mode-0400 private JSON object outside Git, derive the
       release-driver array from that object, and run release-input-preflight
       before any build mutation. Bootc `all-roles` receipt is now a private
       dest (`/root/mcnf-private/bootc-all-roles-digest.json`) and a bootc-bound
       argv object exists; App VM / Maps / RPM fields stay `REPLACE_*`.
       Template was not overwritten. Do not claim preflight passed.
     - Deliverable: private object path, derived driver-array path, redacted
       input inventory, and preflight transcript.
      - Validation: missing, changed, symlinked, stale, or cross-revision inputs
       refuse; fixture substitutions require the governed evidence record; no
       credentials or private keys enter Git/logs.
     - Done when: WL-REL-001 S3 is green and downstream release work may start.
- Scope: open-source source selection, reproducible input generation, receipts,
  licenses, and preflight admission; no public release or live-seat testing.
- Relevant files/components: install-helpers/release-input-preflight.sh,
  packaging/app-vm, install-helpers/produce-bootc-digest-receipt.py,
  Maps catalog/verifier tools, and the Kiron asset verifier.
- Dependencies: WL-FUNC-023 live enroll before freeze; WL-REL-001 S1 candidate
  identity and S2 version matrix; operator-approved curated Flatpak refs; the
  governed RPM signer secret after freeze; WL-TEST-002 for live-seat dest.
  Do not invent catalog refs. Do not guess Surface `bootc_base` while blocked.
- Acceptance criteria: every mandatory first-release input is reproducible,
  licensed, immutable, current-revision-bound, and admitted by preflight; no
  fixture, unavailable input, or external handoff can satisfy the gate.
- Verification method: farm-only source/image/package gates, receipt inspectors,
  hostile substitution tests, license review, and canonical preflight.
  @farm:{cargo build --workspace}
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
     - Action: verify source receipt again; verify preflight again; admit a
       native Fedora 44 builder on XEN-BIGBOY for the full RPM lane and a
       distinct native Fedora 44 slot for Server RPM. If the `.130` Fedora 42 VM
       conflicts with the `.131` Fedora 44 builder, drain and stop `.130`, run
       the admitted release lanes, then restore `.130`; container-F44 output is
       compatibility evidence, not the production RPM cut.
     - Deliverable: build invocation record with host, slot, revision, epoch, target, and output parent.
     - Validation: run-first-full-release.sh must refuse dirty, moving, cross-epoch, or non-Fedora-44 input.
     - Done when: both build lanes are pinned before either artifact is admitted.
  2. S2 Build Workstation and Lighthouse RPMs.
     - Inputs: frozen source and admitted inputs.
     - Action: run the full native Fedora 44 RPM lane on the admitted BigBoy
       builder through run-first-full-release.sh prepare.
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
- Problem: a complete release requires three signed RPM roles and three verified image roles; no current-revision six-role set exists.
- Required outcome: self-sign the exact handoff RPMs without changing payload identity and produce Browser VM, App VM, and bootc roles.
- Current state: a private, promotion-forbidden historical seven-role preview
  exists for `afc24782ca9dc8e2e87f5676e403428a82285da1`, including now-deferred
  Cuttlefish bytes. It cannot define the final six-role set and remains
  non-promotable. WL-REL-001 also remains blocked
  on the feature-complete source freeze; durable evidence is recorded in
  `docs/platform/evidence/WL-REL-003-WL-REL-004-preview-afc-r1.md`. Self-signing
  authorization is recorded. Do not ask for another signing-policy decision;
  load the matching private key from a named system credential into an
  ephemeral mode-0700 keyring and fail the signing gate if verification fails.
- Remaining work:
  1. S1 Materialize and verify the self-signing boundary.
     - Inputs: project release key, private signing material, RPM signing identity receipt, and WL-REL-002 handoff.
     - Action: ensure `rpm-sign` is present on the signing host, require public
       fingerprint `06B1C27EA0E08A225155EB3314018AA1497DDC7C`, confirm its
       receipt, import that system credential into an ephemeral
       keyring, and copy only the three handoff RPMs into one private signing
       directory. Destroy the keyring after signing.
     - Deliverable: redacted signer identity evidence and exact pre-sign payload identity table.
     - Validation: sign-release.sh --self-test; receipt inspector; rpm -Kv before mutation; no secret bytes enter logs or Git.
     - Done when: the exact governed fingerprint is selected, every other secret
       key refuses, and all three inputs match handoff.json exactly.
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
     - Action: run build-release-derivative-images.sh exactly once with an
       absent private output path, then construct the release-output plan from
       those exact derivative files; never collect an earlier input-plan pair
       and silently rebuild a second set.
     - Deliverable: immutable Browser VM and App VM images, manifests, and frozen Browser profile.
     - Validation: image manifest verifiers, qcow2 checks, source revision checks, and hostile substitution fixture.
     - Done when: both derivatives verify and the helper publishes no partial output.
  5. S5 Admit the bootc role.
     - Inputs: bootc digest receipt, reference, architecture, and `all-roles` identity.
     - Action: verify the governed bootc receipt; do not rebuild or relabel ungoverned third-party bytes.
     - Deliverable: bootc receipt fields ready for the six-role plan.
     - Validation: bootc digest receipt and source-revision verifiers reject
       changed bytes, identity, architecture, role, or provider.
     - Done when: bootc binds to the frozen revision and Android remains absent.
  6. S6 Create the exact six-role plan input.
     - Inputs: three signed RPMs/manifests, two derivative images/manifests, and bootc fields.
     - Action: write one private mcnf-release-output-plan-input JSON object containing exactly the six canonical roles.
     - Deliverable: immutable plan input and a redacted role inventory.
     - Validation: produce-release-output-plan.py accepts it; missing, duplicate, extra, relative, mutable, or cross-revision inputs refuse.
     - Done when: exactly six role records are accepted and no artifact path is ambiguous.
- Scope: self-signing, candidate manifests, derivative generation, and plan input; no final evidence signing, publication, or installation.
- Relevant files/components: install-helpers/sign-release.sh, install-helpers/build-release-derivative-images.sh,
  install-helpers/produce-release-output-plan.py, packaging/app-vm, packaging/browser-vm, and bootc receipt tools.
- Dependencies: WL-REL-002 and the automated signing-credential preflight.
- Acceptance criteria: three RPM signatures verify without payload drift; three image roles verify; exactly six roles bind to one revision.
- Verification method: signing and role-specific verifiers, derivative hostile suite, plan producer, and independent hash/identity comparison.
- Origin or merged source IDs: archived WL-BUILD-001, WL-BUILD-003, WL-FUNC-016, WL-FUNC-017, and WL-CRIT-006 release roles.

### WL-REL-004 - Assemble the signed six-role release evidence bundle

- Status: Blocked
- Priority: P0
- Complexity: Epic
- Problem: publication is forbidden until all artifacts, manifests, gates, SBOM data, checksums, and provenance form one exact signed bundle.
- Required outcome: collect and verify all six roles, execute mandatory release gates, and sign one immutable publication envelope.
- Current state: the historical seven-role plan and collector pass for the
  private historical `afc24782` preview, including fresh App VM and Browser VM manifest
  verification. The collection is promotion-forbidden and still lacks the
  signed provenance/SBOM/gate envelope, clean-room publication readback, and
  final source-freeze authority required to close this epic. Evidence:
  `docs/platform/evidence/WL-REL-003-WL-REL-004-preview-afc-r1.md`.
- Remaining work:
  1. S1 Resume and collect the six-role output.
     - Inputs: WL-REL-002 handoff, WL-REL-003 derivative argv and plan input, frozen revision, and Fedora target.
     - Action: run run-first-full-release.sh resume into an absent private output path.
     - Deliverable: collection-plan.json, release-outputs.json, verified derivatives, and promotion-forbidden output directory.
     - Validation: resume compares signed RPM payloads to the handoff and collectors re-run every canonical owning verifier.
     - Done when: collection is atomic, immutable, revision-bound, and contains exactly six verified roles.
  2. S2 Execute the canonical gate matrix.
     - Inputs: the revision-independent release-gate-matrix template, frozen
       revision, collected artifacts, and all named evidence commands.
     - Action: generate the matrix for the frozen revision, run every mandatory
       gate, route heavy package/workspace gates to the farm, and preserve a
       typed result manifest with command, owner, timestamps, artifact, and
       revision.
     - Deliverable: complete gate manifest with pass/fail, owner, command, artifact, revision, and timestamps.
     - Validation: verify-release-gate-matrix.py --expected-revision; omitted, vacuous, stale, or altered gate results refuse.
     - Done when: all mandatory gates are genuinely green or the epic is marked Blocked with the exact failing implementation.
  3. S3 Generate SBOM and release evidence.
     - Inputs: six-role collection, dependency closure outputs, build identities, and gate manifest.
     - Action: generate an aggregate SBOM/license manifest requiring exactly the
       six canonical roles; bind every artifact hash, candidate manifest, and
       gate result into one evidence envelope.
     - Deliverable: six-role SBOM/license manifest, evidence JSON,
       release-output inventory, and artifact-to-source traceability table.
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
- Dependencies: WL-REL-003 and prepublication WL-TEST-002 S1-S7 qualification
  of those exact private candidate bytes.
- Acceptance criteria: one signed immutable six-role evidence bundle passes all mandatory gates and rejects any artifact-set drift.
- Verification method: farm gates, collector and gate verifiers, SBOM/evidence checks, detached-signature verification, and publication preflight.
- Origin or merged source IDs: archived WL-BUILD-003 and WL-CRIT-006 production-evidence responsibilities.

### WL-REL-005 - Publish and promote the newest complete release

- Status: Blocked
- Priority: P0
- Complexity: Epic
- Problem: version 13.0.0 has no immutable current tag or complete public asset set, and partial candidates must never enter the package channel.
- Required outcome: publish one immutable tag and GitHub release, verify all assets by readback, then atomically expose only signed package metadata.
- Current state: tags end at magic-mesh-v12.1.1; WL-REL-004 has no signed
  six-role bundle, so publication is correctly refused. Publication access is
  an S1 execution-time verification, not a presumed current blocker. Never
  substitute an unsigned or promotion-forbidden preview if access is absent.
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
     - Inputs: remote tag, release notes, six artifacts, manifests, SBOM, gates, provenance, checksums, and signatures.
     - Action: create a draft release, upload the complete admitted set without
       clobbering, verify the remote draft through the strict tag/asset/readback
       verifier, then publish it; never expose a partial set as final.
     - Deliverable: public release URL, asset inventory, sizes, hashes, and publication receipt.
     - Validation: verify-github-release-binding.sh against remote metadata; asset count and names equal the admitted bundle.
     - Done when: every required asset is downloadable and no unadmitted asset is attached.
  4. S4 Verify downloaded bytes independently.
     - Inputs: fresh private download directory and published release.
     - Action: download every asset; verify SHA256SUMS.asc, checksums, provenance, SBOM/gates, RPM signatures, and role identities.
     - Deliverable: clean-room readback transcript and downloaded-asset digest table.
     - Validation: no local artifact path is reused; all downloaded bytes match the signed bundle.
     - Done when: public readback independently reconstructs the exact six-role release identity.
  5. S5 Promote signed package metadata atomically.
     - Inputs: verified downloaded RPMs, signed repository policy, HOLD
       boundary, and current channel metadata.
     - Action: stage metadata privately from the downloaded release RPMs;
       ensure HOLD/unsigned candidates are excluded; sign `repomd.xml`; require
       `repo_gpgcheck=1`; re-read the remote package branch and publish only by
       checked fast-forward.
     - Deliverable: repository metadata receipt and package query output for all three RPM roles.
     - Validation: fresh repository query resolves only signed admitted NEVRAs; partial/unsigned fixture cannot enter metadata.
     - Done when: package clients can resolve the complete release and no stale or unsigned higher candidate blocks upgrades.
  6. S6 Hand off to post-publication acceptance reconciliation.
     - Inputs: publication receipt, download verifier results, package/image references, and corrected-forward recovery identity.
     - Action: update WL-TEST-002 with exact release inputs and select exactly Dell, Seat 15, and Surface as physical proof seats.
     - Deliverable: acceptance handoff naming immutable artifacts, seats, lighthouses, providers, and rollback-forbidden recovery plan.
     - Validation: all references resolve and every seat mutation requires the governed alert/wait sequence.
     - Done when: WL-TEST-002 S8 can compare public bytes with the already
       qualified private candidate without guessing any identity.
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
- Current state: pre-release harnesses pass; qualification waits for the signed
  six-role candidate. Topology is Dell, Seat 15, Surface, three lighthouses.
  Operator 2026-08-22: those seats may be mutated (red alert + 5s) when the
  unpublished candidate exists; use sealed Vitelity/SIP creds. A failure
  reopens its owning provider/infrastructure story; no feature waiver.
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
     - Done when: every required provider path passes; absence or failure keeps
       the release red and reopens the owning provider/infrastructure story.
  4. S4 Capture Construct direct-DRM acceptance.
     - Inputs: selected display seat, exact release, Dark/Light and required text/layout profiles.
     - Action: capture shell, taskbar, Front Door, Workers, Kiron/health, Maps, Editor, Music, Files, and key error states.
     - Deliverable: native readback images/metadata, hashes, route identity,
       dimensions, and machine-verifier disposition.
     - Validation: captures come from the direct-DRM seat and exact release;
       deterministic pixel/geometry/route checks reject boot curtains, stale
       routes, or clipped required controls without manual review.
     - Done when: required visual routes pass or reopen a named implementation epic.
  5. S5 Prove media and physical integrations.
     - Inputs: audio/video fixtures, authorized Cast/DLNA devices, catalog/server paths, and network-loss controls.
     - Action: test playback, cache/offline, audio/video, renderer recovery, Cast, DLNA, typed handoff, and provider loss.
     - Deliverable: device identity, media command/result, continuity, loss, recovery, and CPU/package observations.
     - Validation: device/provider discovery is real; media state never claims success after transport failure.
     - Done when: every required integration passes objective transport,
       rendered-media, audio-correlation, loss, and recovery checks.
  6. S6 Prove guest and device roles.
     - Inputs: signed Browser VM, App VM, and bootc artifacts plus governed
       Workloads compute and GPU/audio/input fixtures.
     - Action: schedule each guest on a capability-advertised KVM node, then
       launch and reconnect Browser/VDI/App/bootc roles from each proof seat;
       test input, audio, GPU, upgrade identity, and failure recovery. A seat
       without local KVM uses the governed remote Workloads path rather than
       blocking the run. Android is visibly `Deferred` and is not launched,
       scored, or accepted for this release.
     - Deliverable: artifact-to-runtime identity, readiness, connection, detach/reconnect, and failure evidence.
     - Validation: runtime bytes match signed artifacts; missing capability is visible and cannot become a healthy state.
     - Done when: every guest role passes exact identity and lifecycle checks;
       missing hardware or fixtures fail and reopen their owning story.
  7. S7 Execute recovery and resilience drills.
     - Inputs: installed baseline, corrected-forward candidate, service/network/storage controls, and recovery verifier.
     - Action: test process restart, display/session recovery, lock/sleep, network/storage loss, generation change, reboot, and re-enrollment.
     - Deliverable: pre-failure, failure, correction, and post-recovery evidence for each drill.
     - Validation: verify-corrected-forward-recovery.py; no rollback satisfies recovery; data/history retention rules remain enforced.
     - Done when: failures converge by corrected-forward action without invented health or unrecorded data loss.
  8. S8 Reconcile and archive acceptance.
     - Inputs: all S1-S7 evidence and every archived source-epic proof queue.
     - Action: map results to owning epics; reopen implementation or
       infrastructure regressions; create the final release disposition.
     - Deliverable: signed acceptance index, blocker list, reopened work references, and WL-TEST-002 archive disposition.
     - Validation: every obligation has passing evidence or a reopened owning
       story; no external-input waiver or manual assertion is accepted.
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

## Feature Completion

These epics close the remaining gap between implementation-complete and fit
for purpose: the Communications parity-ledger rulings that never landed, the
Calls media plane, and the operator-flagged legacy mesh-PBX retirement. They
are implementation-only and disjoint from the release chain; each rides the
story execution contract above.

### WL-FUNC-024 - Carry live audio and video media in Communications Calls

- Status: Blocked
- Priority: P1
- Complexity: Epic
- Problem: Communications Calls ships the complete call UI and convergent
  command set, but every transport seam is a marked media-plane follow-up: no
  audio or video ever flows, so the workgroup's calls product cannot make a
  call.
- Required outcome: a call started from the Calls UI carries live audio (and
  offered video) between seats over WebRTC P2P, group calls ride an elected
  LiveKit SFU with P2P failover, and PSTN legs terminate through the LiveKit
  SIP gateway reusing the mde-voice-hud softphone, with all media state owned
  by typed mackesd verbs and never by the renderer.
- Current state: in-tree media publish + VoiceAccounts consume landed.
  Mute/DTMF fail closed without a published MediaSessionV1. Leftover is live
  media/SFU/PSTN. Seats run `magic-mesh-12.1.6-35`; PSTN depends on FUNC-030
  (Blocked); no unpublished signed candidate. Evidence:
  `WL-FUNC-024-2026-08-22-live-leftover-park-r1.md`.
- Remaining work: leftover is still live media/SFU/PSTN after a
  current-revision unpublished candidate is installed.
  1. S1 Add the typed media contracts.
     - Inputs: mde-collab-types versioning conventions and the calls.rs command
       set.
     - Action: add `MediaSessionV1`, `MediaTrackKind`, and
       `MediaSessionStateV1` (covering device-absent, permission-denied,
       reconnecting, and failed) to `crates/shared/mde-collab-types/`.
     - Deliverable: bounded versioned contracts consumed by worker and UI.
     - Validation: hostile decode, version, and size coverage in the crate
       suite.
     - Done when: no media fact crosses the Bus as untyped JSON.
  2. S2 Implement the mackesd media worker for WebRTC P2P.
     - Inputs: the collab signaling topics and seat audio capture/playback.
     - Action: one worker negotiates offer/answer over the existing signaling,
       binds the seat audio device, and publishes
       `state/calls/media/<session>` readiness.
     - Deliverable: one-to-one audio calls between two seats.
     - Validation: the loopback/chirp fixture proves frames flow; device
       absence and permission denial publish typed unavailable states.
     - Done when: mute and DTMF act on the live leg.
  3. S3 Add the elected LiveKit SFU path for group calls.
     - Inputs: the existing leader/lighthouse election machinery.
     - Action: elect the SFU host, join group sessions through it, and fail
       back to P2P when no SFU is healthy.
     - Deliverable: group calls with an honest SFU-degraded fallback.
     - Validation: SFU loss mid-call renegotiates without a fake connected
       state.
     - Done when: a three-seat group call carries audio with election
       evidence.
  4. S4 Terminate PSTN legs through the LiveKit SIP gateway.
     - Inputs: `VoiceAccounts`, `run_agent_accounts`, `lift_if_legacy` (the
       Q15 ruling), and the gateway configuration from WL-FUNC-030.
     - Action: drive the split/shared-outbound-aware agent path, lift flat
       legacy accounts once, and bridge gateway legs into calls.
     - Deliverable: outbound and inbound PSTN calls from the Calls UI.
     - Validation: a legacy flat account lifts once; split accounts never
       double-register.
     - Done when: a governed provider credential completes a PSTN leg, or the
       absent provider stays visibly unavailable.
  5. S5 Bind the Calls UI to the live plane.
     - Inputs: the S1-S4 topics and states plus the calls.rs marked seams.
     - Action: route `SendDtmf` and `SetCallMuted` to the live sender,
       enumerate real devices, attach and detach camera and screen tracks,
       and delete every media-plane follow-up marker.
     - Deliverable: the existing call bar drives real media.
     - Validation: no marker remains; every control has an observable media
       effect or an honest unavailable state.
     - Done when: calls.rs carries no recorded-intent-only path.
  6. S6 Failure honesty and recovery.
     - Inputs: the S2-S5 states.
     - Action: peer drop, SFU unreachable, device unplug, and permission
       revocation each walk the typed reconnecting/failed ladder with an
       operator-visible reason.
     - Deliverable: bounded auto-reconnect plus a manual re-dial affordance.
     - Validation: forced-drop fixtures land on the typed states, never a
       stuck connected state.
     - Done when: every failure mode names its state and recovery in
       evidence.
- Scope: collab-types contracts, one mackesd media worker, mde-voice-hud
  account driving, and the calls.rs bindings. No new security subsystem and
  no new consent surface; existing mesh trust and call consent apply.
- Relevant files/components: `crates/desktop/mde-collab-egui/src/calls.rs`,
  `crates/shared/mde-collab-types/`,
  `crates/services/mde-voice-hud/src/sip.rs`,
  `crates/mesh/mackesd/src/workers/`.
- Dependencies: WL-FUNC-030 for the operator-facing gateway configuration the
  PSTN leg consumes; WL-REL-002 unpublished signed candidate plus red alert +
  5s before any live media/PSTN seat mutation.
- Acceptance criteria: two seats complete an audio call with objective tone
  correlation, mute and DTMF act on the live leg, a group call rides the SFU,
  a PSTN leg lands through the gateway, and every failure renders a typed
  honest state.
- Verification method: focused per-crate farm gates plus the existing audio
  chirp-correlation fixture on the qualification seats; evidence under
  docs/platform/evidence/.
  @farm:{cargo test -p mde-collab-types} @farm:{cargo test -p mackesd}
  @farm:{cargo test -p mde-collab-egui} @farm:{cargo test -p mde-voice-hud}
- Origin or merged source IDs: WL-FUNC-011 parity ledger Q11/Q15 and the
  calls.rs media-plane follow-up markers.

### WL-FUNC-025 - Surface the full Files POSIX operation set

- Status: Blocked
- Priority: P1
- Complexity: Medium
- Problem: file-manager design lock 1 requires the full POSIX plus archive
  operation set, but the egui Files surface wires only New Folder and Rename;
  new-file, duplicate, compress, extract, symlink, and hardlink have no
  reachable command even though the backend engine is complete.
- Required outcome: every lock-1 operation is reachable from the Files menubar
  and context menu and executes through the existing FileOps/OpKind/archive
  engine with the standard confirm, progress, and cancel treatment.
- Current state: S1-S3 surface wiring is in-tree. Q26: Files stays its own OS
  surface. Read-only 2026-08-22: no Files persist files on Dell, Seat 15, or
  Surface; seats run `magic-mesh-12.1.6-35`. Leftover is live mesh-tree and
  archive-queue evidence. Evidence:
  `WL-FUNC-024-2026-08-22-live-leftover-park-r1.md`.
- Remaining work: leftover is live local/mesh and archive-queue evidence, not
  a missing command.
  1. S1 New File and Duplicate.
     - Inputs: the shared name dialog in dialogs.rs and `OpKind::Copy`.
     - Action: add a `NewFile` name-dialog variant that creates an empty
       regular file through `FileOps` with exists-refusal, and a Duplicate
       command that copies each focused row into its own parent under a
       `name (copy)` suffix with the standard conflict dialog.
     - Deliverable: both commands on the menubar and context menu.
     - Validation: existing-name, read-only-directory, and cross-backend rows
       refuse honestly.
     - Done when: both operations execute on local and mesh-mounted trees.
  2. S2 Compress and Extract.
     - Inputs: `OpKind::Compress`/`Extract`, `ArchiveFormat`, and the
       op-queue progress UI.
     - Action: context-menu entries enqueue the existing op kinds with a
       format picker; extraction is the extract-here/extract-to pair.
     - Deliverable: archive create and extract with progress and cancel.
     - Validation: path-traversal members refuse; cancel leaves no
       half-archive.
     - Done when: a zip and a tar.gz round-trip through the queue on the
       surface.
  3. S3 Symlink and Hardlink.
     - Inputs: `FileOps::symlink` and `FileOps::hard_link`.
     - Action: an Advanced submenu creates links beside the focused row with
       link-target validation.
     - Deliverable: both link types with honest cross-device and
       existing-path errors.
     - Validation: hardlink across devices and symlink escaping a mesh mount
       refuse.
     - Done when: `symlink_metadata` reports the created link on reload.
- Scope: mde-files-egui surface wiring plus at most additive helpers in
  mde-files. No engine rewrite and no new store.
- Relevant files/components:
  `crates/desktop/mde-files-egui/src/{dialogs.rs,model/mod.rs,view.rs,menubar.rs}`,
  `crates/services/mde-files/src/{opqueue.rs,fileops.rs,archive.rs}`.
- Dependencies: WL-REL-002 unpublished signed candidate; operator live Files
  use on a current-revision seat.
- Acceptance criteria: all six operations are reachable, execute through the
  existing engine, report progress and cancel honestly, and hostile paths
  refuse.
- Verification method: focused mde-files and mde-files-egui farm gates; no
  live hardware required.
  @farm:{cargo test -p mde-files-egui} @farm:{cargo test -p mde-files}
- Origin or merged source IDs: WL-FUNC-011 parity ledger Q26
  (build-new:file-manager-posix-ops), file-manager design lock 1.

### WL-FUNC-026 - Persist per-folder Files view preferences

- Status: Blocked
- Priority: P2
- Complexity: Small
- Problem: file-manager design lock 20 says view and sort persist per folder,
  but `FolderPrefs` is an in-memory `HashMap` lost on every restart.
- Required outcome: per-folder view mode, sort order, and show-hidden survive
  a shell restart.
- Current state: persist path is in-tree. Read-only 2026-08-22:
  `files-folder-prefs.json` is absent on Dell, Seat 15, and Surface; seats
  run `magic-mesh-12.1.6-35`. Leftover is live restart evidence. Evidence:
  `WL-FUNC-024-2026-08-22-live-leftover-park-r1.md`.
- Remaining work: leftover is live restart evidence (fixtures do not satisfy
  production).
  1. S1 Serialize on mutation and hydrate at construction.
     - Inputs: the editor-egui.json precedent (JSON under the mcnf config
       directory) and `FolderPrefs`.
     - Action: derive serde on `FolderPrefs`, write
       `<config>/mcnf/files-folder-prefs.json` debounced on mutation, hydrate
       at `FileBrowser` construction, cap the map with least-recently-used
       eviction, and degrade corrupt, oversized, or symlinked files to
       defaults with an honest note.
     - Deliverable: durable per-folder preferences.
     - Validation: corrupt JSON, oversize, and symlinked prefs files fall
       back to defaults without panicking.
     - Done when: a restart preserves a changed view, sort, and show-hidden
       for a visited folder.
- Scope: the mde-files-egui model only.
- Relevant files/components:
  `crates/desktop/mde-files-egui/src/model/mod.rs`.
- Dependencies: WL-REL-002 unpublished signed candidate; operator live Files
  use then restart on a current-revision seat.
- Acceptance criteria: preferences survive restart, stay bounded on disk, and
  hostile files degrade to defaults.
- Verification method: focused mde-files-egui farm gate.
  @farm:{cargo test -p mde-files-egui}
- Origin or merged source IDs: WL-FUNC-011 parity ledger Q28
  (build-new:file-manager-folderprefs-persist), file-manager design lock 20.

### WL-FUNC-027 - Add persisted user bookmarks to the Files Places sidebar

- Status: Blocked
- Priority: P2
- Complexity: Small
- Problem: file-manager design lock 21 requires user-pinnable bookmarks, but
  the Places sidebar is a fixed set plus live mesh peers; the capability was
  never built.
- Required outcome: operators pin, rename, reorder, and remove their own
  Places entries, persisted across restarts.
- Current state: bookmark store is in-tree. Read-only 2026-08-22:
  `files-bookmarks.json` is absent on Dell, Seat 15, and Surface; seats run
  `magic-mesh-12.1.6-35`. Leftover is live restart/navigate evidence.
  Evidence: `WL-FUNC-024-2026-08-22-live-leftover-park-r1.md`.
- Remaining work: leftover is live restart/navigate evidence, not a missing
  store.
  1. S1 Add the bookmark store and sidebar section.
     - Inputs: the FolderPrefs JSON precedent (WL-FUNC-026) and the existing
       Places render path.
     - Action: a bounded `<config>/mcnf/files-bookmarks.json` store with
       path validation and a count cap; pin/unpin from the focused row;
       rename, reorder, and remove in place; render a user section above the
       fixed places while mesh peers stay a distinct live section.
     - Deliverable: durable user bookmarks.
     - Validation: hostile paths, duplicate pins, and corrupt stores refuse
       or degrade honestly.
     - Done when: a pinned folder survives restart and navigates on
       activation.
- Scope: mde-files-egui only.
- Relevant files/components:
  `crates/desktop/mde-files-egui/src/model/mod.rs`,
  `crates/desktop/mde-files-egui/src/view.rs`.
- Dependencies: WL-REL-002 unpublished signed candidate; operator live Files
  pin then restart on a current-revision seat.
- Acceptance criteria: pin, rename, reorder, and remove persist and activate;
  the store is bounded and hostile input refuses.
- Verification method: focused mde-files-egui farm gate.
  @farm:{cargo test -p mde-files-egui}
- Origin or merged source IDs: WL-FUNC-011 parity ledger Q27
  (build-new:file-manager-bookmarks), file-manager design lock 21.

### WL-FUNC-028 - Build the recurring-mirror (sync-pair) producer

- Status: Blocked
- Priority: P2
- Complexity: Medium
- Problem: the design-locked recurring rsync mirror is code-complete but
  unreachable: `TransferVerb::{SaveSyncPair, RemoveSyncPair}` are drained and
  tested and `SyncPairStore` plus the worker scheduler are live, yet no CLI
  or GUI producer exists, so the feature silently drops.
- Required outcome: operators create, edit, list, and remove recurring sync
  pairs from the CLI and from Communications Transfers; execution stays on
  the existing worker.
- Current state: in-tree CLI/GUI and persist/fold landed. Read-only 2026-08-22:
  Dell, Seat 15, and Surface run `magic-mesh-12.1.6-35`; `mackesd transfer` has
  no `sync-pair` subcommand. Live next-run/last-result cannot be proven until a
  current-revision unpublished signed candidate is installed (WL-REL-002).
  Evidence: `WL-FUNC-028-2026-08-22-installed-cli-gap-r1.md`.
- Remaining work: leftover is live Bus / operator-visible next-run and
  last-result on a real pair after that RPM is on an acceptance seat.
  1. S1 Add the CLI producer.
     - Inputs: TransferCmd conventions and the Save/Remove verbs.
     - Action: add `mackesd transfer sync-pair add|remove|list` posting the
       existing typed verbs with interval, source, destination, and bwlimit.
     - Deliverable: the CLI manages pairs end to end.
     - Validation: malformed intervals and unknown pair ids refuse.
     - Done when: a CLI-added pair appears in the store and schedules.
  2. S2 Add the Transfers-mode editor.
     - Inputs: the Communications transfers.rs mirror and the same verbs.
     - Action: a sync-pair editor (create/edit/remove, interval,
       source/destination, bwlimit) that publishes the verbs and mirrors
       `SyncPairStore` projections, never a second progress authority.
     - Deliverable: GUI parity with the CLI.
     - Validation: identical requests through CLI and GUI produce identical
       store records.
     - Done when: next-run and last-result come from the worker and
       unreachable peers stay visibly degraded.
- Scope: the mackesd CLI plus the collab-egui Transfers mode; the worker
  engine is reused unchanged.
- Relevant files/components: `crates/mesh/mackesd/src/bin/mackesd.rs`,
  `crates/mesh/mackesd/src/cli/transfer.rs`,
  `crates/mesh/mackesd/src/workers/transfers/`,
  `crates/desktop/mde-collab-egui/src/transfers.rs`.
- Dependencies: WL-REL-002 unpublished signed candidate; no seat package
  mutation without that candidate plus red alert + 5s.
- Acceptance criteria: CLI and GUI both manage pairs; the existing worker
  executes them; no second store or scheduler appears.
- Verification method: focused mackesd transfers and mde-collab-egui farm
  gates.
  @farm:{cargo test -p mackesd} @farm:{cargo test -p mde-shell-egui}
  @farm:{cargo test -p mde-collab-egui}
- Origin or merged source IDs: WL-FUNC-011 parity ledger Q34
  (build-new:recurring-mirror-producer), transfers design lock.

### WL-FUNC-029 - Build the Fleet voice-admin panel in Communications Activity

- Status: Blocked
- Priority: P2
- Complexity: Medium
- Problem: fleet voice provisioning (Vitelity DIDs, routing, failover,
  shared-outbound, cutover) is a leader/operator function whose worker and
  verbs are live, but no surface publishes them since the iced Workbench
  retired.
- Required outcome: Communications Activity carries the fleet voice-admin
  panel publishing the existing action/voice verbs and rendering the
  state/voice topics.
- Current state: Fleet voice-admin + `hydrate_voice` landed. Leftover is live
  Vitelity. Operator 2026-08-22 allows seat+Vitelity mutation only when an
  unpublished signed candidate exists; none does. Seats still run
  `magic-mesh-12.1.6-35`. Evidence:
  `WL-FUNC-028-2026-08-22-installed-cli-gap-r1.md`.
- Remaining work: leftover is live Vitelity on a current-revision seat, not a
  missing Activity section.
  1. S1 Panel over the existing verbs.
     - Inputs: the voice_provision verb bodies (provision, DID route,
       failover, shared config) and the Activity mode layout.
     - Action: add the Fleet/Voice admin section to Activity with an account
       provisioning form, DID routing table, per-node failover policy,
       shared-outbound toggle, and cutover control, publishing the typed
       verbs and rendering retained state/voice projections with freshness.
     - Deliverable: the leader/operator voice console inside Communications.
     - Validation: invalid DIDs, unknown nodes, and conflicting routes refuse
       at the verb boundary; the panel renders honestly empty without a
       provisioned account.
     - Done when: every published verb round-trips to a visible state/voice
       projection.
- Scope: collab-egui Activity plus at most additive projection reads; the
  worker contract is unchanged.
- Relevant files/components:
  `crates/desktop/mde-collab-egui/src/activity.rs`,
  `crates/mesh/mackesd/src/workers/voice_provision.rs`.
- Dependencies: WL-REL-002 unpublished signed candidate; operator lock that
  seats+Vitelity go only with that candidate plus red alert + 5s.
- Acceptance criteria: provision, DID-route, failover, and shared-config all
  publish from the panel and land in retained state; no mackesd contract
  change.
- Verification method: focused mde-collab-egui and mackesd voice_provision
  farm gates.
  @farm:{cargo test -p mde-collab-egui} @farm:{cargo test -p mackesd}
- Origin or merged source IDs: WL-FUNC-011 parity ledger Q8
  (build-new:fleet-voice-admin@Activity).

### WL-FUNC-030 - Build the mesh SIP-gateway config control in Communications Activity

- Status: Blocked
- Priority: P2
- Complexity: Small
- Problem: the mesh-wide SIP gateway responder
  (`action/voip/{set,get,clear}-gateway`) still runs, but its only documented
  publisher was the retired iced Workbench, so the workgroup gateway is
  unconfigurable from any live surface.
- Required outcome: Communications Activity owns gateway configuration; the
  responder and gateway.toml contract stay unchanged; the existing workgroup
  gateway.toml migrates in place.
- Current state: gateway form / in-place hydrate landed. Read-only 2026-08-22:
  no `gateway.toml` on Dell, Seat 15, or Surface; seats run
  `magic-mesh-12.1.6-35`. Live Bus leftover waits on a current-revision RPM
  plus a migrated workgroup toml. Evidence:
  `WL-FUNC-028-2026-08-22-installed-cli-gap-r1.md`.
- Remaining work: leftover is live Bus + migrated workgroup toml, not a
  missing GUI publisher.
  1. S1 Gateway section in Activity.
     - Inputs: the three verb bodies and redaction contract in ipc/voip.rs.
     - Action: a bounded gateway form (host, port, credentials) publishing
       set-gateway, a present/absent readout from get-gateway, and a
       confirmed clear-gateway; the write path never echoes the password
       back, and the readout renders the redacted shape.
     - Deliverable: gateway configuration from Communications.
     - Validation: malformed hosts and replayed clears refuse; the password
       never renders.
     - Done when: set, get, and clear round-trip on a live Bus and the
       migrated gateway.toml loads unchanged.
- Scope: collab-egui Activity only; the responder is untouched.
- Relevant files/components:
  `crates/desktop/mde-collab-egui/src/activity.rs`,
  `crates/mesh/mackesd/src/ipc/voip.rs`.
- Dependencies: WL-REL-002 unpublished signed candidate; a migrated workgroup
  `gateway.toml` on an acceptance seat.
- Acceptance criteria: the full set/get/clear cycle works from the panel with
  the redaction contract intact.
- Verification method: focused mde-collab-egui and mackesd voip farm gates.
  @farm:{cargo test -p mde-collab-egui} @farm:{cargo test -p mackesd}
- Origin or merged source IDs: WL-FUNC-011 parity ledger Q12
  (build-new:sip-gateway-config@Activity).

### WL-FUNC-031 - Build the per-document mesh co-edit share-session UI

- Status: Blocked
- Priority: P2
- Complexity: Medium
- Problem: the Yrs CRDT co-editing library rides the embedded editor, but no
  user-reachable action starts or joins a share session; only the Mesh Map
  badge observes the wire, so mesh co-editing is a protocol without a
  product.
- Required outcome: from Documents mode an operator shares the focused
  document to a space, participants join with view/edit permission and
  follow-mode, and the session lifecycle is visible and closable by its
  owner.
- Current state: show() mounts live_document_share_session(); sibling pub+wire
  landed. Leftover is live two-seat co-edit. Seats run `magic-mesh-12.1.6-35`;
  no unpublished signed candidate. Evidence:
  `WL-FUNC-024-2026-08-22-live-leftover-park-r1.md`.
- Remaining work: leftover is live two-seat co-edit evidence only, not a
  missing mount wire.
  1. S1 Share-session lifecycle UI.
     - Inputs: the documents.rs marked seams, the collab_session library, and
       space membership from the collab store.
     - Action: a Share control on the focused document that starts a session
       into a chosen space, a participant list with a follow-mode toggle, and
       owner close; joining from the space's live-session picker.
     - Deliverable: start, join, follow, and close from the surface.
     - Validation: non-members and closed sessions refuse honestly; owner
       close detaches every follower.
     - Done when: two seats co-edit one document with visible cursors.
  2. S2 External-write three-way merge.
     - Inputs: the documents.rs snapshot-emit marker (last-shared-base).
     - Action: merge external file changes against the last shared base
       instead of overwriting the live CRDT buffer.
     - Deliverable: no silent clobber of in-flight co-edits.
     - Validation: a concurrent external write merges or surfaces a typed
       conflict, never a lost edit.
     - Done when: the Phase-3c markers are gone from documents.rs.
- Scope: collab-egui Documents mode and its use of the carried library; no
  transport or CRDT changes.
- Relevant files/components:
  `crates/desktop/mde-collab-egui/src/documents.rs`,
  `crates/desktop/mde-collab-egui/src/fixture.rs`,
  `crates/desktop/mde-editor-egui/`.
- Dependencies: WL-REL-002 unpublished signed candidate; two current-revision
  seats and an operator share session.
- Acceptance criteria: share, join, follow, and close work between two seats;
  external writes merge safely; no Phase-3c marker remains.
- Verification method: focused mde-collab-egui farm gates with the
  two-instance session fixture.
  @farm:{cargo test -p mde-collab-egui}
- Origin or merged source IDs: WL-FUNC-011 parity ledger Q21
  (defer-followup), documents.rs Phase-3c markers.

### WL-FUNC-032 - Reserve the Transfers hotkeys

- Status: Blocked
- Priority: P3
- Complexity: Small
- Problem: no global accelerator opens Transfers, so the industry-standard
  downloads chord (Ctrl+J) risks colliding with future bindings.
- Required outcome: Ctrl+J opens Communications Transfers from any Construct
  surface and one in-mode accelerator starts a new transfer; both are
  registered in the shared keymap.
- Current state: catalog + apply refuse landed. Leftover is live-surface
  proof from every Construct surface. Seats run `magic-mesh-12.1.6-35`; no
  unpublished signed candidate. Evidence:
  `WL-FUNC-024-2026-08-22-live-leftover-park-r1.md`.
- Remaining work: leftover is live-surface proof from every Construct
  surface, not a missing binding.
  1. S1 Register both bindings.
     - Inputs: the hotkeys.rs table and the Communications mode router.
     - Action: bind Ctrl+J to Surface::Communications in Transfers mode and
       an in-mode New Transfer accelerator; render both in the Hotkeys
       settings section.
     - Deliverable: two documented bindings.
     - Validation: the bindings fire from every surface and never shadow text
       editing inside Documents or Terminal.
     - Done when: the hotkeys catalog lists both.
- Scope: the shell keymap and Communications mode only.
- Relevant files/components:
  `crates/desktop/mde-shell-egui/src/hotkeys.rs`,
  `crates/desktop/mde-collab-egui/src/transfers.rs`.
- Dependencies: WL-REL-002 unpublished signed candidate; current-revision
  Construct on a used acceptance seat.
- Acceptance criteria: both accelerators work and are catalogued with no
  focus-context shadowing.
- Verification method: focused mde-shell-egui and mde-collab-egui farm gates.
  @farm:{cargo test -p mde-shell-egui} @farm:{cargo test -p mde-collab-egui}
- Origin or merged source IDs: WL-FUNC-011 parity ledger
  (build-new:reserve-transfer-hotkeys).

### WL-FUNC-033 - Retire the legacy mesh-PBX stack and dead parity rows

- Status: Remaining
- Priority: P2
- Complexity: Large
- Problem: the operator-confirmed-dead Kamailio/RTPengine mesh-PBX stack and
  several orphaned modules still ship and spawn, carrying config writers, a
  worker, a CLI verb, and systemd units that the pure-Rust softphone and
  Communications Calls never touch.
- Required outcome: the retired stack and dead rows are deleted in one sweep;
  the tree builds and runs without them; the parity ledger's retire rows cite
  the deleting revision.
- Current state: operator Q9 signoff landed 2026-08-22. Stack deleted; S1
  live-negative 2026-08-20 and read-only reread 2026-08-22 (all three seats
  inactive/not-found; no kamailio/rtpengine process; evidence
  `WL-FUNC-033-2026-08-22-fleet-negative-reread-r1.md`). Ledger retire rows
  cite deleting revisions. README telephony bullet names the softphone path
  only. Leftover is still keep `own_nebula_ip` in lib `voip_rtt.rs`.
  Keep lint `install-helpers/lint-func033-keep.sh` is in ci-gate
  POLICY_LINTS and requires a live caller (`WL-FUNC-033-2026-08-22-keep-caller-lint-r1.md`).
- Remaining work: leftover is still keep `own_nebula_ip` in lib
  `voip_rtt.rs`. Then:

  1. S1 Confirm no live seat runs the stack.
     - Inputs: fleet inventory and systemd unit states.
     - Action: verify no enrolled seat runs kamailio-mde or rtpengine-mde for
       real SIP before deletion (the ledger's pre-deploy check).
     - Deliverable: a recorded fleet-wide negative.
     - Validation: any positive finding parks this epic again.
     - Done when: the check is evidence-cited.
  2. S2 Delete the mesh-PBX stack.
     - Inputs: the S1 evidence.
     - Action: remove the crate, voice modules, worker, CLI verb, and units;
       drop workspace membership and packaging references.
     - Deliverable: a tree with no Kamailio/RTPengine path.
     - Validation: build, clippy, and the full farm gate pass; no spawn site
       or seed topic references the deleted pieces.
     - Done when: greps for the deleted modules return only archive and
       ledger references.
  3. S3 Delete the orphaned and never-wired rows.
     - Inputs: ledger Q10, Q13, Q29, and Q33 retire rulings.
     - Action: remove roster.rs and resolve.rs, and the dead View arms with
       their list() branches and responders (keeping fleet-files, files-inbox,
       and file-ops). SendToEntry is Toolbar + ContextMenu only. Keep
       `own_nebula_ip` in lib `voip_rtt.rs`.
     - Deliverable: no orphan module remains.
     - Validation: the full farm gate passes; the retained responders'
       contract tests stay green.
     - Done when: the parity ledger's retire rows cite the deleting
       revision (landed). Leftover is `own_nebula_ip` (keep).
- Scope: deletion only; no replacement security or policy surface.
- Relevant files/components: the paths named in Current state plus packaging
  and systemd references.
- Dependencies: Q9 signed 2026-08-22; re-confirm the 2026-08-20 fleet-negative
  before deleting remaining voip_rtt spawn sites.
- Acceptance criteria: the workspace builds and gates green without the
  deleted stack; no live reference remains; the ledger cites the revision.
- Verification method: focused mackesd farm gate plus module-reference greps
  recorded in evidence. @farm:{cargo test -p mackesd}
- Origin or merged source IDs: WL-FUNC-011 parity ledger Q9, Q10, Q13, Q29,
  and Q33 retire rulings.

